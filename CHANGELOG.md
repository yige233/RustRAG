# Changelog

All notable releases of RustRAG.

[🇨🇳 中文更新日志](CHANGELOG_ZH.md)

---

## v3.1.0 — Correctness & Concurrency Hardening

Systematic fix pass over the v3.0.0 review findings: wire up code relations, harden search/security, and clean dead config.

### Features

- **Code relations work end-to-end**: calls / imports / inherits extracted at index time into `code_relations`; `search_relations` is no longer empty. Edges attributed to the **innermost** symbol (no class/method double-count).
- **ESM/CJS variants indexable**: `.mjs` / `.cjs` / `.mts` / `.cts` route as Code (previously allowlisted but always `failed`).
- **Vector KNN search**: sqlite-vec `embedding MATCH ... k = N`; filters over-fetch then LIMIT (no full-table distance sort).
- **`embedding.max_concurrent` implemented**: wave-parallel Embedding API requests.
- **Dictionary second pass**: `build_dictionary` also mines Chinese comments near symbols.

### Reliability

- Embedder init failure **no longer silently falls back to Mock** — tools return a clear error.
- `manage_document` delete of a missing document returns failure (was fake success).
- `Db::open` builds schema from configured dimensions and validates against metadata + existing `vec_chunks` schema; mismatch refuses to start.
- Single-file index shares path normalization with directory sync.
- Data-dir create failure exits; batch index returns per-file error details.

### Security

- MCP `index` / `frontmatter` paths restricted to `document_patterns` base dirs (Windows compared case-insensitively).
- Missing frontmatter target reports `file not found` first.

### Cleanup

- Removed deprecated `compute` / `model` config sections (old JSON keys ignored).
- Updater cache uses `data_dir` (was incorrectly using `db_path` file path).
- HTTP log matches bind (`127.0.0.1`); PDF extract page cap 500.
- `bin/query` takes CLI args; architecture docs synced to API Embedder.

### Migration

v3.0.0 → v3.1.0 usually needs **no** `vectors.db` wipe (still 1024-dim). If you used another `dimensions`, keep config consistent or delete the DB. `compute` / `model` keys may be removed or left ignored.

---

## v3.0.0 — Embedding API Rewrite

Replaced the local ONNX runtime with an **OpenAI-compatible Embedding API** backend.

- API embeddings (DashScope / Ollama / OpenAI / Azure OpenAI, …), default 1024-dim.
- Second-level startup, ~50MB runtime memory; dropped `ort` / `tokenizers` / `bytemuck` / `indicatif`.
- Smart batching + exponential backoff; data dir defaults to `~/.rustrag/`.

**Breaking** (from v2.x): delete old `vectors.db`; switch to the `embedding` config section; drop `compute` / `model`.

---

## v2.4.3 — sqlite-vec Init Order Fix

Fixes the sqlite-vec auto-extension registration order — `sqlite3_auto_extension` is now called **before** `Connection::open()`, ensuring the extension is loaded on every connection in the r2d2 pool.

- **Fix sqlite-vec init order**: Auto-extension must be registered before the connection is opened.
- **Note**: Requires v2.4.1+ (`sqlite-vec 0.1.9`).

## v2.4.2 — File Watcher Fix

Fixes a bug where ignored directories (`target`, `node_modules`) were still being indexed during hot-reloads despite being in `exclude_patterns`.

- **Watcher Exclude Patterns**: Now fully respects `exclude_patterns` via `ignore::OverrideBuilder`.

## v2.4.1 — sqlite-vec Upgrade

Maintenance release upgrading `sqlite-vec` from `0.1.7-alpha.10` to stable `0.1.9`.

- Fixes `no such function: vec_version` on some platforms.
- **Note**: Delete existing `vectors.db` and restart if upgrading from v2.4.0 or earlier.

## v2.4.0 — Multi-Format Document Support

Expands RustRAG from code-only indexing to a universal document RAG engine.

- **24 file types**: Plain text, JSON, YAML, TOML, CSV, HTML, PDF, DOCX, XLS/XLSX/XLSB/ODS.
- **Format-specific chunking**: Preserves structural info (JSON key paths, CSV headers, etc.).
- **New dependencies** (pure Rust): `lopdf`, `docx-rs`, `calamine`, `scraper`, `toml`, `csv`.
- Added `.jsx`/`.tsx` to supported code extensions.

## v2.3.0 — Security & Code Quality

Systematic code review addressing 26 issues:

- **Security**: Fixed Windows path validation, restricted arbitrary file reads, HTTP binds localhost.
- **Reliability**: Replaced `assert_eq!` panics with error propagation, fixed indexer counters.
- **Config**: Invalid JSON returns errors instead of silent defaults; dimension validation at startup.
- **i18n**: Japanese (Hiragana/Katakana) and Korean (Hangul) language detection.
- **Performance**: Auto-detected ONNX threads, `LazyLock` cached `LanguageConfig`.
- **Code Quality**: Removed dead PHP paths, fixed TOCTOU race, added `// SAFETY:` docs.

## v2.2.0 — Architecture Refactor

Major refactor for high concurrency and async reliability:

- **Connection pooling**: `r2d2` with `sqlite-vec` for safe multi-threaded access.
- **Async networking**: Migrated update checker from `reqwest::blocking` to native async.
- **Config safety**: Resolved TOCTOU race conditions in config loading.

## v2.1.0 — Advanced Improvements

Enhanced features, performance, and reliability.

## v2.0.0 — ONNX Model Migration

Migrated from `model.onnx` (470MB) to `model_O4.onnx` (235MB):

- ONNX Graph Optimization Level 4 — identical vectors, no re-indexing.
- Download size halved, runtime memory halved (~500MB → ~250MB).
- Auto-detects and removes legacy model files.

## v1.3.7 — Config Hot-Reload

`RwLock`-based hot-reloading for configurations and model instances:

- GPU inference engine hot-reload on config change.
- Dynamic config & file watcher sync.

## v1.3.6 — Hardware Acceleration

- Multi-platform GPU acceleration (CUDA, TensorRT, DirectML, CoreML) with CPU fallback.
- `batch_size` tuning and `compute.fallback_to_cpu` fault tolerance.
- Real-time file watching with incremental sync.
- SQLite WAL mode for concurrent write safety.
- Granular MCP error reporting.

## v1.2.0 & v1.1.0 — Performance & Compression

- **INT8 scalar quantization**: `FLOAT[384]` → `INT8[384]`, 75% storage reduction.
- **ONNX Level 3 graph optimization**: Improved CPU inference.
- **Automated cleanup**: Stale document purge on filter changes.

> ⚠️ Upgrading from v1.1.x requires deleting the existing `vectors.db`.
