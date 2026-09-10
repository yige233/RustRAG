# RustRAG

[![License: MIT/Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE-MIT)
[![CI](https://github.com/Michaol/RustRAG/actions/workflows/ci.yml/badge.svg)](https://github.com/Michaol/RustRAG/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

[🇨🇳 中文文档](README_ZH.md) · [📋 Changelog](CHANGELOG.md)

A high-performance RAG (Retrieval-Augmented Generation) MCP Server written in Rust.

> **40× token reduction** — indexes your codebase and documents, retrieves only the most relevant context for AI assistants via configurable semantic vectors (default 1024-dim).

---

## Latest Release (v3.1.0)

v3.1.0 hardens the v3.0.0 Embedding API rewrite: code relations are wired end-to-end, ESM/CJS variants index correctly, vector search uses sqlite-vec KNN, and embedder failures no longer silently degrade.

### v3.0.0 Highlights (API rewrite)

- **API-based Embedding** — Switched from local ONNX model (~235MB download, ~500MB memory) to any OpenAI-compatible `/v1/embeddings` API. Supports DashScope, Ollama, OpenAI, Azure OpenAI, and more.
- **1024-Dimensional Vectors** — Upgraded from 384-dim to 1024-dim (float32), significantly improving semantic search precision.
- **Instant Startup** — No model download or ONNX Runtime loading. Server starts in under 1 second.
- **90% Memory Reduction** — Runtime memory drops from ~500MB (ONNX) to ~50MB (API client).
- **4 Fewer Dependencies** — Removed `ort`, `tokenizers`, `bytemuck`, `indicatif`.
- **Smart Batching** — Adaptive batch sizing based on text length prevents API payload size errors.
- **Exponential Backoff Retry** — Up to 3 attempts with retryability classification (429/5xx → retry, 4xx → fail fast).
- **New File Formats** — Added `.mjs`, `.cjs`, `.mts`, `.cts` (28 total supported formats).
- **Absolute Path Data Directory** — Database and state stored in `~/.rustrag/` by default, no more project directory pollution.

### Migration from v2.x

This is a **breaking change**. To migrate:

1. Delete the old `vectors.db` (schema incompatible — 384 vs 1024 dimensions)
2. Update `config.json` to include the new `embedding` section (see [Quick Start](#2-configure))
3. Set your API key via config or environment variable (`RAG_API_KEY`, `DASHSCOPE_API_KEY`, or `OPENAI_API_KEY`)
4. Remove `compute` and `model` sections from config (no longer used; ignored since v3.1.0)

### Upgrading from v3.0.0 to v3.1.0

Usually **no** `vectors.db` wipe is required. Keep `embedding.dimensions` consistent with the DB; a mismatch fails fast at startup.

[📋 Full changelog](CHANGELOG.md)

---

## Features

- **7 MCP Tools** — search, index, list_documents, manage_document, frontmatter, search_relations, build_dictionary
- **28 Supported Formats** — Code (Rust, Go, Python, TypeScript, JavaScript, +ESM/CJS variants), Markdown, plain text, structured data (JSON, YAML, TOML, CSV), HTML, PDF, DOCX, spreadsheets
- **KNN Vector Search** — SQLite + sqlite-vec, configurable `float32` dimensions (default 1024)
- **Code Intelligence** — Tree-sitter AST chunking + calls/imports/inherits relation extraction
- **Multilingual Dictionary** — CJK↔English symbol mapping (including comment-adjacent symbols)
- **Any OpenAI-Compatible API** — DashScope, Ollama (local), OpenAI, Azure OpenAI, DeepSeek, SiliconFlow
- **Path Allowlist** — MCP file tools restricted to `document_patterns` base directories
- **High Concurrency** — Async background syncing with robust pagination for 10k+ files
- **Cross-Platform** — macOS (Intel/ARM), Linux (x64/ARM64), Windows (x64)

## Quick Start

### 1. Install

Download from [Releases](https://github.com/Michaol/RustRAG/releases) or build from source:

```bash
git clone https://github.com/Michaol/RustRAG.git
cd RustRAG
cargo build --release
```

The binary will be at `target/release/rustrag` (or `rustrag.exe` on Windows).

### 2. Configure

Create a `config.json` in your project root (auto-generated with defaults on first run):

```json
{
  "document_patterns": ["./"],
  "exclude_patterns": ["**/node_modules/**", "**/target/**", "**/.git/**"],
  "file_extensions": [
    "md", "rs", "go", "py",
    "js", "mjs", "cjs", "jsx",
    "ts", "mts", "cts", "tsx",
    "txt", "log",
    "json", "yaml", "yml", "toml", "csv",
    "html", "htm",
    "pdf", "docx", "xls", "xlsx", "xlsb", "ods"
  ],
  "chunk_size": 500,
  "search_top_k": 5,
  "embedding": {
    "api_url": "https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings",
    "api_key": "",
    "api_model": "text-embedding-v4",
    "dimensions": 1024,
    "batch_size": 32,
    "max_concurrent": 5,
    "timeout_secs": 30
  }
}
```

Set your API key via environment variable (recommended) or directly in `api_key`:

```bash
# Any of these environment variables are supported:
export RAG_API_KEY="sk-your-api-key"
export DASHSCOPE_API_KEY="sk-your-api-key"
export OPENAI_API_KEY="sk-your-api-key"
```

#### Switching Providers

| Provider | `api_url` | `api_model` | Dimensions |
|---|---|---|---|
| DashScope | `https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings` | `text-embedding-v4` | 1024 |
| Ollama (local) | `http://localhost:11434/v1/embeddings` | `nomic-embed-text` | 768 |
| OpenAI | `https://api.openai.com/v1/embeddings` | `text-embedding-3-small` | 1536 |
| DeepSeek | `https://api.deepseek.com/v1/embeddings` | `deepseek-embedding` | 1024 |
| SiliconFlow | `https://api.siliconflow.cn/v1/embeddings` | `BAAI/bge-large-zh-v1.5` | 1024 |

> **Note**: When switching providers, update `dimensions` to match the model output and delete the existing `vectors.db` (schema is created from `dimensions` on open and checked via `system_metadata`; a mismatch refuses to start).

### 3. Add to MCP Client

#### Claude Desktop / Cursor / Antigravity IDE

Add to your MCP configuration:

```json
{
  "mcpServers": {
    "rustrag": {
      "command": "/absolute/path/to/rustrag",
      "args": ["--config", "/path/to/your/project/config.json"]
    }
  }
}
```

#### 🌩️ Advanced: Remote SSH Mode

Install RustRAG on a remote server and pipe it to your local IDE via SSH:

```json
{
  "mcpServers": {
    "rustrag-remote": {
      "command": "ssh",
      "args": [
        "user@remote.server.ip",
        "/absolute/path/to/rustrag",
        "--config",
        "/remote/project/config.json"
      ]
    }
  }
}
```

> Configure SSH keys (`ssh-keygen -t ed25519` + `ssh-copy-id`) for passwordless authentication, since MCP clients cannot prompt for passwords.

## CLI Options

| Flag             | Default       | Description                             |
| ---------------- | ------------- | --------------------------------------- |
| `--config`, `-c` | `config.json` | Path to configuration file              |
| `--log-level`    | `info`        | Log level (trace/debug/info/warn/error) |
| `--skip-sync`    | false         | Skip initial document sync              |
| `--transport`    | `stdio`       | Transport mode: `stdio` or `http`       |
| `--port`         | `8765`        | HTTP port (used if transport=`http`)    |
| `--version`      | —             | Display version and exit                |

## MCP Tools

| Tool               | Description                                                             |
| ------------------ | ----------------------------------------------------------------------- |
| `search`           | Semantic vector search with optional directory/filename filters         |
| `index`            | Index documents or code files using AST-aware chunking                  |
| `manage_document`  | Remove a document from the index or force re-index                      |
| `list_documents`   | List all indexed documents                                              |
| `frontmatter`      | Add or update YAML frontmatter in a markdown file                       |
| `search_relations` | Search code relationships (calls, imports, inherits)                    |
| `build_dictionary` | Extract CJK↔English term mappings from code                             |

## Architecture

```
src/
├── lib.rs              # Module exports
├── main.rs             # CLI + startup sequence
├── config.rs           # Configuration loading/validation
├── frontmatter.rs      # YAML frontmatter operations
├── updater.rs          # Version update checker (GitHub API)
├── watcher.rs          # File system watcher (hot reload)
├── db/                 # SQLite + sqlite-vec vector database
│   ├── mod.rs          # Schema (float32[1024]) + connection pool
│   ├── models.rs       # Data models
│   ├── documents.rs    # Document CRUD operations
│   ├── search.rs       # Vector similarity search (cosine distance)
│   └── relations.rs    # Code relationship queries
├── embedder/           # Text embedding
│   ├── mod.rs          # Embedder trait definition
│   ├── api.rs          # OpenAI-compatible API client (smart batching + retry)
│   └── mock.rs         # Mock embedder (testing)
├── indexer/            # Document & code indexing
│   ├── core.rs         # Differential sync engine
│   ├── markdown.rs     # Markdown chunking
│   ├── text_parser.rs  # Multi-format extraction (PDF, DOCX, XLSX, etc.)
│   ├── code_parser.rs  # Tree-sitter code parsing
│   ├── relations.rs    # Code relationship extraction
│   ├── dictionary.rs   # Multilingual dictionary
│   └── languages.rs    # Language-specific Tree-sitter queries
└── mcp/                # MCP protocol layer
    ├── server.rs       # Server setup (stdio + HTTP transport)
    └── tools.rs        # 7 tool handler implementations
```

## Supported Languages

| Language   | Extensions                        | Parser                 |
| ---------- | --------------------------------- | ---------------------- |
| Rust       | `.rs`                             | tree-sitter-rust       |
| Go         | `.go`                             | tree-sitter-go         |
| Python     | `.py`                             | tree-sitter-python     |
| TypeScript | `.ts` `.tsx` `.mts` `.cts`       | tree-sitter-typescript |
| JavaScript | `.js` `.jsx` `.mjs` `.cjs`       | tree-sitter-javascript |
| Markdown   | `.md`                             | pulldown-cmark         |

## Supported Document Formats

| Format         | Extensions                          | Parser / Library       |
| -------------- | ----------------------------------- | ---------------------- |
| Plain Text     | `.txt`, `.log`                      | `fs::read_to_string`   |
| JSON           | `.json`                             | `serde_json`           |
| YAML           | `.yaml`, `.yml`                     | `serde_yaml`           |
| TOML           | `.toml`                             | `toml`                 |
| CSV            | `.csv`                              | `csv`                  |
| HTML           | `.html`, `.htm`                     | `scraper`              |
| PDF            | `.pdf`                              | `lopdf`                |
| Word           | `.docx`                             | `docx-rs`              |
| Spreadsheet    | `.xls`, `.xlsx`, `.xlsb`, `.ods`   | `calamine`             |

## Building from Source

**Prerequisites:** Rust 1.85+

```bash
cargo build --release
```

## Testing

```bash
# Run all tests (88 unit + 6 integration)
cargo test --all

# Run integration tests only
cargo test --test integration_test

# Lint (zero warnings expected)
cargo clippy -- -D warnings
```

## License

Licensed under either of

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
