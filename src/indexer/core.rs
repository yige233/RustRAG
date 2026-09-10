use crate::config::Config;
use crate::db::Db;
use crate::embedder::Embedder;
use crate::indexer::markdown;
use chrono::{DateTime, Utc};
use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// Normalizes a path to absolute format, stripping Windows UNC prefixes.
pub fn normalize_system_path(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let s = canonical.to_string_lossy();
    let s = if let Some(stripped) = s.strip_prefix(r"\\?\") {
        stripped
    } else {
        s.as_ref()
    };
    s.replace('\\', "/")
}

/// File type classification for routing to the appropriate indexer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FileType {
    Markdown,
    Code,
    Text,
}

/// Classify a file extension into a FileType for routing.
/// Returns `None` for unsupported extensions.
pub fn classify_extension(ext: &str) -> Option<FileType> {
    match ext {
        "md" => Some(FileType::Markdown),
        "rs" | "go" | "py" | "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" => {
            Some(FileType::Code)
        }
        "txt" | "log" | "json" | "yaml" | "yml" | "toml" | "csv" | "html" | "htm" | "pdf"
        | "docx" | "xls" | "xlsx" | "xlsb" | "ods" => Some(FileType::Text),
        _ => None,
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CodeSyncResult {
    pub indexed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
}

/// Embed texts without blocking the Tokio worker when possible.
///
/// `block_in_place` panics on current-thread runtimes (unit tests), so fall
/// back to a direct call there. Production `#[tokio::main]` is multi-thread.
fn embed_batch_off_executor<E: Embedder + ?Sized>(
    embedder: &E,
    texts: &[&str],
) -> Result<Vec<Vec<f32>>, crate::embedder::EmbedderError> {
    use tokio::runtime::{Handle, RuntimeFlavor};
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| embedder.embed_batch(texts))
        }
        _ => embedder.embed_batch(texts),
    }
}

/// Shared process-wide relation extractor (tree-sitter queries are expensive to compile).
fn get_relation_extractor()
-> Result<&'static crate::indexer::relations::RelationExtractor, Box<dyn std::error::Error>> {
    use crate::indexer::relations::RelationExtractor;
    static CELL: OnceLock<Result<RelationExtractor, String>> = OnceLock::new();
    match CELL.get_or_init(|| RelationExtractor::new().map_err(|e| e.to_string())) {
        Ok(extractor) => Ok(extractor),
        Err(msg) => Err(msg.clone().into()),
    }
}

/// Assign each relation line to the innermost symbol chunk that contains it.
fn owning_chunk_index(
    chunks: &[crate::indexer::code_parser::CodeChunk],
    line: usize,
) -> Option<usize> {
    chunks
        .iter()
        .enumerate()
        .filter(|(_, c)| line >= c.start_line && line <= c.end_line)
        .min_by_key(|(_, c)| c.end_line.saturating_sub(c.start_line))
        .map(|(i, _)| i)
}

/// Extract call/import/inherit relations for a code file and insert them.
///
/// Relations are attributed once per edge, to the innermost enclosing symbol.
/// `target_chunk_id` is resolved within the same file via a symbol→chunk map.
pub fn store_code_file_relations(
    db: &Db,
    real_path: &Path,
    db_path: &str,
    code_chunks: &[crate::indexer::code_parser::CodeChunk],
    chunk_ids: &[i64],
) -> Result<usize, Box<dyn std::error::Error>> {
    use crate::db::models::CodeRelation as DbRelation;

    let Some(lang) = code_chunks.first().map(|c| c.language.as_str()) else {
        return Ok(0);
    };

    let content = std::fs::read(real_path)?;
    let extractor = get_relation_extractor()?;
    let file_rels = extractor.extract_relations(&content, lang, db_path, "")?;

    // symbol_name → chunk_id for same-file targets (avoids N+1 lookups)
    let mut symbol_to_chunk: HashMap<&str, i64> = HashMap::new();
    for (i, chunk) in code_chunks.iter().enumerate() {
        if let Some(id) = chunk_ids.get(i) {
            symbol_to_chunk
                .entry(chunk.symbol_name.as_str())
                .or_insert(*id);
        }
    }

    let mut db_rels: Vec<DbRelation> = Vec::new();
    for rel in &file_rels {
        let Some(idx) = owning_chunk_index(code_chunks, rel.source_line) else {
            continue;
        };
        let Some(&source_chunk_id) = chunk_ids.get(idx) else {
            continue;
        };
        let chunk = &code_chunks[idx];
        let target_chunk_id = symbol_to_chunk.get(rel.target_name.as_str()).copied();
        db_rels.push(DbRelation {
            id: 0,
            source_chunk_id,
            target_chunk_id,
            relation_type: rel.relation_type.as_str().to_string(),
            target_name: rel.target_name.clone(),
            target_file: rel.target_file.clone(),
            confidence: 1.0,
            source_name: Some(chunk.symbol_name.clone()),
            source_file: Some(db_path.to_string()),
        });
    }

    let inserted = db_rels.len();
    if !db_rels.is_empty() {
        db.insert_relations(&db_rels)?;
    }
    Ok(inserted)
}

pub struct Indexer<'a, E: Embedder + ?Sized> {
    pub db: Arc<Db>,
    pub embedder: &'a E,
    pub chunk_size: usize,
    pub config: Arc<Config>,
}

impl<'a, E: Embedder + ?Sized> Indexer<'a, E> {
    pub fn new(db: Arc<Db>, embedder: &'a E, chunk_size: usize, config: Arc<Config>) -> Self {
        Self {
            db,
            embedder,
            chunk_size,
            config,
        }
    }

    /// Checks if a file extension is supported
    fn is_supported_extension(&self, ext: &str) -> bool {
        self.config.is_file_extension_supported(ext)
    }

    /// Indexes all supported files in a directory with differential sync
    pub async fn index_directory<P: AsRef<Path>>(
        &mut self,
        dir: P,
        force: bool,
    ) -> Result<CodeSyncResult, Box<dyn std::error::Error>> {
        let dir = dir.as_ref();
        let dir_str = normalize_system_path(dir);

        // PHASE 4: Config Hot Reload & Hash Check
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&self.config.exclude_patterns, &mut hasher);
        std::hash::Hash::hash(&self.config.file_extensions, &mut hasher);
        let config_hash = std::hash::Hasher::finish(&hasher).to_string();
        let meta_key = format!("dir_hash:{}", dir_str);

        {
            let db_guard = self.db.clone();
            let stored = db_guard.get_metadata(&meta_key).unwrap_or(None);
            let should_rebuild = stored.as_deref() != Some(config_hash.as_str());

            if should_rebuild && !force {
                tracing::info!(
                    "Configuration filter change detected for {}. Purging previous index.",
                    dir.display()
                );
                if let Ok(removed) = db_guard.delete_documents_by_prefix(&dir_str) {
                    tracing::info!("Purged {} stale documents due to config change.", removed);
                }
                let _ = db_guard.set_metadata(&meta_key, &config_hash);
            } else if force && should_rebuild {
                let _ = db_guard.set_metadata(&meta_key, &config_hash);
            }
        }

        // Get existing documents from DB map(filename -> modified_at)
        let existing_docs = {
            let db_guard = self.db.clone();
            db_guard.list_documents()?
        };

        let mut visited_paths = std::collections::HashSet::new();

        let mut result = CodeSyncResult::default();

        // Build overrides from config
        let mut overrides = OverrideBuilder::new(dir);
        for pattern in &self.config.exclude_patterns {
            let _ = overrides.add(&format!("!{}", pattern));
        }
        let override_matcher = overrides
            .build()
            .unwrap_or_else(|_| OverrideBuilder::new(dir).build().unwrap());

        // Walk builder respects .gitignore by default
        let walker = WalkBuilder::new(dir)
            .hidden(false)
            .overrides(override_matcher)
            .build();

        for entry in walker.into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                continue;
            }

            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if !self.is_supported_extension(ext) {
                continue;
            }

            // Enforce consistent absolute system paths for all documents
            let path_str = normalize_system_path(path);
            visited_paths.insert(path_str.clone());

            let metadata = entry.metadata()?;
            let mod_time: DateTime<Utc> = metadata.modified()?.into();

            let mut needs_indexing = true;
            let mut was_update = false;

            if let Some(existing_time) = existing_docs.get(&path_str) {
                if !force && mod_time.timestamp() == existing_time.timestamp() {
                    result.skipped += 1;
                    needs_indexing = false;
                } else {
                    was_update = true;
                }
            }

            if needs_indexing {
                let success = match classify_extension(ext) {
                    Some(FileType::Markdown) => self
                        .index_markdown(path, &path_str, mod_time)
                        .await
                        .map_err(|e| {
                            tracing::warn!(file = %path.display(), error = %e, "markdown index failed");
                            e
                        })
                        .is_ok(),
                    Some(FileType::Code) => self
                        .index_code_file(path, &path_str, mod_time)
                        .await
                        .map_err(|e| {
                            tracing::warn!(file = %path.display(), error = %e, "code index failed");
                            e
                        })
                        .is_ok(),
                    Some(FileType::Text) => self
                        .index_text_file(path, &path_str, mod_time)
                        .await
                        .map_err(|e| {
                            tracing::warn!(file = %path.display(), error = %e, "text index failed");
                            e
                        })
                        .is_ok(),
                    None => {
                        tracing::debug!(
                            file = %path.display(),
                            ext = ext,
                            "unsupported extension, skipping"
                        );
                        false
                    }
                };

                if success {
                    result.indexed += 1;
                    if was_update {
                        result.updated += 1;
                    } else {
                        result.added += 1;
                    }
                } else {
                    result.failed += 1;
                }
            }
        }

        // Phase 2: Stale Cleanup — collect stale paths, then delete in a single batch
        let dir_str = normalize_system_path(dir);
        let stale_paths: Vec<&str> = existing_docs
            .keys()
            .filter(|p| p.starts_with(&dir_str) && !visited_paths.contains(p.as_str()))
            .map(|p| p.as_str())
            .collect();

        if !stale_paths.is_empty() {
            let db_guard = self.db.clone();
            if let Ok(removed) = db_guard.delete_documents_batch(&stale_paths) {
                result.removed += removed;
            }
        }

        Ok(result)
    }

    pub async fn index_file(&self, path: &Path) -> Result<bool, Box<dyn std::error::Error>> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if !self.is_supported_extension(ext) {
            return Ok(false);
        }

        let metadata = match path.metadata() {
            Ok(m) => m,
            Err(_) => return Ok(false),
        };
        let mod_time: DateTime<Utc> = match metadata.modified() {
            Ok(m) => m.into(),
            Err(_) => return Ok(false),
        };
        let path_str = normalize_system_path(path);

        let success = match classify_extension(ext) {
            Some(FileType::Markdown) => {
                self.index_markdown(path, &path_str, mod_time).await.is_ok()
            }
            Some(FileType::Code) => self
                .index_code_file(path, &path_str, mod_time)
                .await
                .is_ok(),
            Some(FileType::Text) => self
                .index_text_file(path, &path_str, mod_time)
                .await
                .is_ok(),
            None => false,
        };

        Ok(success)
    }

    async fn index_markdown(
        &self,
        real_path: &Path,
        db_path: &str,
        mod_time: DateTime<Utc>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let chunks = markdown::parse_markdown(real_path, self.chunk_size)?;
        if chunks.is_empty() {
            return Ok(());
        }

        let text_refs: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();

        // Vectorize chunks (blocking HTTP — keep off the async executor)
        let vectors = embed_batch_off_executor(self.embedder, &text_refs)?;

        // Map to models::Chunk for DB insertion
        let db_chunks: Vec<crate::db::models::Chunk> = chunks
            .iter()
            .map(|c| crate::db::models::Chunk {
                position: c.position,
                content: c.content.as_str(),
            })
            .collect();

        // Write to DB
        {
            let db_guard = self.db.clone();
            db_guard.insert_document(db_path, mod_time, &db_chunks, &vectors)?;
        }

        Ok(())
    }

    /// Index a code file using Tree-sitter AST parsing.
    ///
    /// Parses the file into symbol-level chunks (functions, classes, methods),
    /// generates embeddings from enriched text (`language symbol_name: content`),
    /// stores them with full code metadata, then extracts and stores call/import/inherit relations.
    async fn index_code_file(
        &self,
        real_path: &Path,
        db_path: &str,
        mod_time: DateTime<Utc>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::indexer::code_parser::CodeParser;

        let mut parser = CodeParser::new()?;
        let code_chunks = parser.parse_file(real_path)?;
        if code_chunks.is_empty() {
            return Ok(());
        }

        // Generate embedding text enriched with language + symbol context
        let text_refs: Vec<String> = code_chunks.iter().map(|c| c.get_embedding_text()).collect();
        let text_str_refs: Vec<&str> = text_refs.iter().map(|s| s.as_str()).collect();

        // Vectorize (blocking HTTP — keep off the async executor)
        let vectors = embed_batch_off_executor(self.embedder, &text_str_refs)?;

        // Convert indexer::CodeChunk → db::models::CodeChunk
        let db_chunks: Vec<crate::db::models::CodeChunk> = code_chunks
            .iter()
            .enumerate()
            .map(|(i, c)| crate::db::models::CodeChunk {
                chunk: crate::db::models::Chunk {
                    position: i,
                    content: &c.content,
                },
                symbol_name: Some(c.symbol_name.as_str()),
                symbol_type: &c.symbol_type,
                language: &c.language,
                start_line: Some(c.start_line),
                end_line: Some(c.end_line),
                parent_symbol: c.parent_symbol.as_deref(),
                signature: Some(c.signature.as_str()),
            })
            .collect();

        // Write to DB with code metadata; capture chunk IDs for relations
        let chunk_ids = self
            .db
            .insert_code_document(db_path, mod_time, &db_chunks, &vectors)?;

        store_code_file_relations(&self.db, real_path, db_path, &code_chunks, &chunk_ids)?;

        Ok(())
    }

    /// Index a text/structured/document file.
    /// Extracts text using format-specific logic, chunks it, embeds, and stores.
    async fn index_text_file(
        &self,
        real_path: &Path,
        db_path: &str,
        mod_time: DateTime<Utc>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let chunks = crate::indexer::text_parser::extract_and_chunk(real_path, self.chunk_size)?;
        if chunks.is_empty() {
            return Ok(());
        }

        let text_refs: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
        let vectors = embed_batch_off_executor(self.embedder, &text_refs)?;

        let db_chunks: Vec<crate::db::models::Chunk> = chunks
            .iter()
            .map(|c| crate::db::models::Chunk {
                position: c.position,
                content: c.content.as_str(),
            })
            .collect();

        {
            let db_guard = self.db.clone();
            db_guard.insert_document(db_path, mod_time, &db_chunks, &vectors)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::embedder::mock::MockEmbedder;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_classify_extension_includes_esm_cjs_variants() {
        assert_eq!(classify_extension("mjs"), Some(FileType::Code));
        assert_eq!(classify_extension("cjs"), Some(FileType::Code));
        assert_eq!(classify_extension("mts"), Some(FileType::Code));
        assert_eq!(classify_extension("cts"), Some(FileType::Code));
        assert_eq!(classify_extension("java"), None);
    }

    #[test]
    fn test_owning_chunk_index_prefers_innermost_span() {
        use crate::indexer::code_parser::CodeChunk;
        let outer = CodeChunk {
            content: String::new(),
            position: 0,
            symbol_name: "Foo".into(),
            symbol_type: "class".into(),
            language: "python".into(),
            start_line: 1,
            end_line: 20,
            parent_symbol: None,
            signature: String::new(),
        };
        let inner = CodeChunk {
            content: String::new(),
            position: 1,
            symbol_name: "bar".into(),
            symbol_type: "function".into(),
            language: "python".into(),
            start_line: 5,
            end_line: 8,
            parent_symbol: Some("Foo".into()),
            signature: String::new(),
        };
        let chunks = vec![outer, inner];
        // Line 6 is inside both; innermost (bar) wins
        assert_eq!(owning_chunk_index(&chunks, 6), Some(1));
        // Line 2 is only inside Foo
        assert_eq!(owning_chunk_index(&chunks, 2), Some(0));
        // Line 30 matches nothing
        assert_eq!(owning_chunk_index(&chunks, 30), None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_indexer_differential_sync() {
        let temp_dir = tempdir().unwrap();
        let dir_path = temp_dir.path();

        // Create some files
        let file1 = dir_path.join("file1.md");
        fs::write(&file1, "Content 1").unwrap();

        let file2 = dir_path.join("file2.md");
        fs::write(&file2, "Content 2").unwrap();

        let db = Db::open_in_memory().unwrap();
        let db_arc = Arc::new(db);
        let embedder = MockEmbedder::default();
        let mut indexer = Indexer::new(
            db_arc.clone(),
            &embedder,
            500,
            Arc::new(crate::config::Config::default()),
        );

        // First sync
        let res1 = indexer.index_directory(dir_path, false).await.unwrap();
        assert_eq!(res1.added, 2);
        assert_eq!(res1.indexed, 2);
        assert_eq!(res1.skipped, 0);

        // Second sync immediately - should skip both
        let res2 = indexer.index_directory(dir_path, false).await.unwrap();
        assert_eq!(res2.added, 0);
        assert_eq!(res2.updated, 0);
        assert_eq!(res2.indexed, 0);
        assert_eq!(res2.skipped, 2);

        // Third sync with force=true - should update both
        let res3 = indexer.index_directory(dir_path, true).await.unwrap();
        assert_eq!(res3.added, 0);
        assert_eq!(res3.updated, 2);
        assert_eq!(res3.indexed, 2);
        assert_eq!(res3.skipped, 0);

        // Check DB
        let docs = db_arc.list_documents().unwrap();
        assert_eq!(docs.len(), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_index_code_file_stores_relations() {
        let temp_dir = tempdir().unwrap();
        let file = temp_dir.path().join("sample.rs");
        fs::write(
            &file,
            r#"
fn helper() {}

fn process() {
    helper();
    external_function();
}
"#,
        )
        .unwrap();

        let db = Arc::new(Db::open_in_memory().unwrap());
        let embedder = MockEmbedder::default();
        let indexer = Indexer::new(
            db.clone(),
            &embedder,
            500,
            Arc::new(crate::config::Config::default()),
        );

        let path_str = normalize_system_path(&file);
        indexer
            .index_code_file(&file, &path_str, chrono::Utc::now())
            .await
            .expect("index code file");

        let rels = db
            .find_symbol_relations("helper", "incoming", Some("calls"))
            .unwrap();
        assert!(
            !rels.is_empty(),
            "process() should record a calls relation to helper"
        );

        // Each edge should be attributed once (no double-count from outer/inner)
        let process_out = db
            .find_symbol_relations("process", "outgoing", Some("calls"))
            .unwrap();
        let helper_targets: Vec<_> = process_out
            .iter()
            .filter(|r| r.target_name == "helper")
            .collect();
        assert_eq!(
            helper_targets.len(),
            1,
            "helper call must be attributed exactly once"
        );
    }
}
