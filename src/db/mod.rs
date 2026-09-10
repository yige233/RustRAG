//! Vector Database module using SQLite and sqlite-vec
use rusqlite::{Connection, Result};
use sqlite_vec::sqlite3_vec_init;
use std::path::Path;
use std::sync::Once;
use tracing::info;

pub mod documents;
pub mod models;
pub mod relations;
pub mod search;

const SCHEMA_SQL_PREFIX: &str = r#"
CREATE TABLE IF NOT EXISTS documents (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    filename TEXT NOT NULL UNIQUE,
    indexed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    modified_at DATETIME NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_filename ON documents(filename);

CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    content TEXT NOT NULL,
    FOREIGN KEY (document_id) REFERENCES documents(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_document_id ON chunks(document_id);
"#;

const SCHEMA_SQL_SUFFIX: &str = r#"
CREATE TABLE IF NOT EXISTS code_metadata (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    chunk_id INTEGER NOT NULL UNIQUE,
    symbol_name TEXT,
    symbol_type TEXT NOT NULL,
    language TEXT NOT NULL,
    start_line INTEGER,
    end_line INTEGER,
    parent_symbol TEXT,
    signature TEXT,
    FOREIGN KEY (chunk_id) REFERENCES chunks(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_code_symbol ON code_metadata(symbol_name);
CREATE INDEX IF NOT EXISTS idx_code_language ON code_metadata(language);
CREATE INDEX IF NOT EXISTS idx_code_type ON code_metadata(symbol_type);

CREATE TABLE IF NOT EXISTS code_relations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_chunk_id INTEGER NOT NULL,
    target_chunk_id INTEGER,
    relation_type TEXT NOT NULL,
    target_name TEXT NOT NULL,
    target_file TEXT,
    confidence REAL DEFAULT 1.0,
    FOREIGN KEY (source_chunk_id) REFERENCES chunks(id) ON DELETE CASCADE,
    FOREIGN KEY (target_chunk_id) REFERENCES chunks(id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_rel_source ON code_relations(source_chunk_id);
CREATE INDEX IF NOT EXISTS idx_rel_target ON code_relations(target_chunk_id);
CREATE INDEX IF NOT EXISTS idx_rel_type ON code_relations(relation_type);
CREATE INDEX IF NOT EXISTS idx_rel_name ON code_relations(target_name);

CREATE TABLE IF NOT EXISTS system_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS word_mapping (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_word TEXT NOT NULL,
    target_word TEXT NOT NULL,
    source_lang TEXT DEFAULT 'zh',
    confidence REAL DEFAULT 1.0,
    source_document TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(source_word, target_word, source_lang)
);

CREATE INDEX IF NOT EXISTS idx_word_source ON word_mapping(source_word);
CREATE INDEX IF NOT EXISTS idx_word_target ON word_mapping(target_word);
CREATE INDEX IF NOT EXISTS idx_word_lang ON word_mapping(source_lang);
"#;

/// Build the full schema SQL for a given vector dimension.
fn schema_sql(dimensions: usize) -> String {
    format!(
        "{SCHEMA_SQL_PREFIX}\nCREATE VIRTUAL TABLE IF NOT EXISTS vec_chunks USING vec0(\n    embedding float32[{dimensions}]\n);\n{SCHEMA_SQL_SUFFIX}"
    )
}

/// Default vector dimension when not specified (matches DashScope text-embedding-v4 default).
pub const DEFAULT_VEC_DIMENSIONS: usize = 1024;

static INIT_VEC: Once = Once::new();

/// Register sqlite-vec as a SQLite auto-extension. Must be called BEFORE any
/// `Connection::open()` — SQLite only applies auto-extensions to connections
/// created after registration.
fn init_sqlite_vec() {
    INIT_VEC.call_once(|| unsafe {
        // SAFETY: `sqlite3_vec_init` has the sqlite3 auto-extension entry-point
        // signature (`fn(db, err, api)`). The transmute converts the extern "C"
        // function pointer to the exact type `sqlite3_auto_extension` expects.
        #[allow(clippy::missing_transmute_annotations)]
        let func = std::mem::transmute(sqlite3_vec_init as *const ());
        rusqlite::ffi::sqlite3_auto_extension(Some(func));
    });
}

use r2d2::{ManageConnection, Pool};

#[derive(Clone)]
pub struct SqliteManager {
    path: Option<std::path::PathBuf>,
}

impl ManageConnection for SqliteManager {
    type Connection = Connection;
    type Error = rusqlite::Error;

    fn connect(&self) -> std::result::Result<Self::Connection, Self::Error> {
        // Register sqlite-vec BEFORE opening the connection so the auto-extension
        // is applied when SQLite creates the database handle.
        init_sqlite_vec();
        let conn = if let Some(p) = &self.path {
            Connection::open(p)?
        } else {
            Connection::open_in_memory()?
        };

        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )?;

        Ok(conn)
    }

    fn is_valid(&self, conn: &mut Self::Connection) -> std::result::Result<(), Self::Error> {
        conn.execute_batch("SELECT 1")
    }

    fn has_broken(&self, _: &mut Self::Connection) -> bool {
        false
    }
}

/// A wrapper around a SQLite connection pool initialized with sqlite-vec and the application schema.
#[derive(Clone)]
pub struct Db {
    pub pool: Pool<SqliteManager>,
}

impl Db {
    pub fn get_conn(&self) -> Result<r2d2::PooledConnection<SqliteManager>> {
        self.pool.get().map_err(|e| {
            rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e.to_string()))
        })
    }

    /// Open a database connection pool at the given path and initialize the schema
    /// with the configured vector dimension. Rejects mismatched existing databases.
    pub fn open<P: AsRef<Path>>(path: P, dimensions: usize) -> Result<Self> {
        let path = path.as_ref();
        info!(
            "Initializing database: {} (vec dim={})",
            path.display(),
            dimensions
        );

        let manager = SqliteManager {
            path: Some(path.to_path_buf()),
        };
        let pool = r2d2::Pool::builder()
            .max_size(15) // sufficient for concurrent read/write and search
            .build(manager)
            .map_err(|e| {
                rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e.to_string()))
            })?;

        // Initialize schema using the first connection
        let conn = pool.get().map_err(|e| {
            rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e.to_string()))
        })?;

        // Detect sqlite-vec once at first open
        if let Ok(ver) = conn.query_row("SELECT vec_version()", [], |row| row.get::<_, String>(0)) {
            info!("sqlite-vec version: {ver}");
        }

        // Resolve the dimension of an already-created vec_chunks table (metadata first,
        // then schema text). CREATE ... IF NOT EXISTS will not change an existing table,
        // so a mismatch must fail before we pretend the open succeeded.
        let meta_dim = stored_dimension(&conn)?;
        let schema_dim = detect_existing_vec_dimensions(&conn)?;
        let existing = meta_dim.or(schema_dim);

        if let Some(existing) = existing {
            if existing != dimensions {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(1),
                    Some(format!(
                        "vector dimension mismatch: database was created with {existing} dims, \
                         config requests {dimensions}. Delete vectors.db and re-index, or set \
                         embedding.dimensions back to {existing}."
                    )),
                ));
            }
        }

        conn.execute_batch(&schema_sql(dimensions))?;

        if meta_dim.is_none() {
            conn.execute(
                "INSERT OR REPLACE INTO system_metadata (key, value) VALUES ('vec_dimensions', ?)",
                [dimensions.to_string().as_str()],
            )?;
        }

        info!("Database initialized successfully");

        Ok(Self { pool })
    }

    pub fn get_metadata(&self, key: &str) -> Result<Option<String>> {
        let conn = self.get_conn()?;
        let res = conn.query_row(
            "SELECT value FROM system_metadata WHERE key = ?",
            [key],
            |row| row.get(0),
        );
        match res {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn set_metadata(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.get_conn()?;
        conn.execute(
            "INSERT INTO system_metadata (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = CURRENT_TIMESTAMP",
            [key, value],
        )?;
        Ok(())
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::open_in_memory_with_dim(DEFAULT_VEC_DIMENSIONS)
    }

    pub fn open_in_memory_with_dim(dimensions: usize) -> Result<Self> {
        let manager = SqliteManager { path: None };
        let pool = r2d2::Pool::builder()
            .max_size(1) // Single connection so all queries hit the initialized schema
            .build(manager)
            .map_err(|e| {
                rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e.to_string()))
            })?;

        let conn = pool.get().map_err(|e| {
            rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(1), Some(e.to_string()))
        })?;
        conn.execute_batch(&schema_sql(dimensions))?;
        Ok(Self { pool })
    }
}

/// Helper to serialize a float32 vector into a raw byte blob for sqlite-vec.
pub fn serialize_vector_f32(vec: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vec.len() * 4);
    for &v in vec {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

fn stored_dimension(conn: &Connection) -> Result<Option<usize>> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM system_metadata WHERE key = 'vec_dimensions'",
            [],
            |row| row.get(0),
        )
        .ok();
    Ok(stored.and_then(|s| s.parse().ok()))
}

/// Parse `float32[N]` out of an existing `vec_chunks` CREATE statement.
fn detect_existing_vec_dimensions(conn: &Connection) -> Result<Option<usize>> {
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'vec_chunks'",
            [],
            |row| row.get(0),
        )
        .ok();
    let Some(sql) = sql else {
        return Ok(None);
    };
    // e.g. ... embedding float32[1024] ...
    let re_start = "float32[";
    let Some(start) = sql.find(re_start) else {
        return Ok(None);
    };
    let rest = &sql[start + re_start.len()..];
    let Some(end) = rest.find(']') else {
        return Ok(None);
    };
    Ok(rest[..end].parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_init() {
        let db = Db::open_in_memory().expect("Failed to open in-memory DB");

        // Verify tables exist
        let conn = db.get_conn().unwrap();
        let tables: usize = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('documents', 'chunks', 'vec_chunks', 'code_metadata', 'code_relations', 'word_mapping');",
            [],
            |row| row.get(0),
        ).unwrap();

        // sqlite_master virtual tables might create multiple internal tables, but the main count should be 6
        assert_eq!(tables, 6);
    }

    #[test]
    fn test_detect_existing_vec_dimensions_from_schema() {
        let db = Db::open_in_memory_with_dim(768).unwrap();
        let conn = db.get_conn().unwrap();
        let dim = detect_existing_vec_dimensions(&conn).unwrap();
        assert_eq!(dim, Some(768));
    }

    #[test]
    fn test_open_rejects_dimension_mismatch_on_existing_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vec.db");
        // Create a 768-dim DB
        {
            let _ = Db::open(&path, 768).expect("open 768");
        }
        // Re-open with 1024 must fail (schema already exists, no/old metadata)
        let result = Db::open(&path, 1024);
        assert!(result.is_err(), "dimension mismatch must be rejected");
        let msg = format!("{:?}", result.err().unwrap());
        assert!(
            msg.contains("dimension mismatch") || msg.contains("768"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_serialize_vector_f32() {
        let vec = vec![1.0f32, 0.0, -1.0];
        let bytes = serialize_vector_f32(&vec);
        // 3 floats × 4 bytes each = 12 bytes
        assert_eq!(bytes.len(), 12);
        // Verify first float (1.0) in little-endian
        assert_eq!(&bytes[0..4], &1.0f32.to_le_bytes());
        // Verify second float (0.0)
        assert_eq!(&bytes[4..8], &0.0f32.to_le_bytes());
        // Verify third float (-1.0)
        assert_eq!(&bytes[8..12], &(-1.0f32).to_le_bytes());
    }
}
