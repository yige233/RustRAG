# 更新日志

RustRAG 全部重要版本变更记录。

[🇬🇧 English Changelog](CHANGELOG.md)

---

## v3.1.0 — 正确性与并发加固

系统性修复 v3.0.0 审查问题，补齐关系抽取链路，并强化检索/安全边界。

### 功能补齐

- **代码关系真正可用**：索引代码文件时抽取 calls / imports / inherits，写入 `code_relations`；`search_relations` 不再恒空。关系按**最内层符号**归属，避免 class/method 双重计数。
- **ESM/CJS 变体可索引**：`.mjs` / `.cjs` / `.mts` / `.cts` 正确路由为 Code（此前 allowlist 命中却全部 failed）。
- **向量 KNN 检索**：改用 sqlite-vec `embedding MATCH ... k = N`，有过滤时 over-fetch 再 LIMIT，避免全表暴力排序。
- **`embedding.max_concurrent` 生效**：按波次并发请求 Embedding API。
- **词典二次抽取**：`build_dictionary` 额外从「中文注释 + 符号」提取映射。

### 可靠性

- Embedder 初始化失败**不再静默回退 Mock**，search/index 返回明确错误。
- `manage_document` 删除不存在文档时返回失败，不再假成功。
- `Db::open` 按配置维度建表；从 metadata 与已有 `vec_chunks` schema 校验维度，不匹配拒绝启动。
- 单文件索引与目录同步统一路径规范化（`normalize_system_path`）。
- 数据目录创建失败直接退出；批量 index 返回每文件错误详情。

### 安全

- MCP `index` / `frontmatter` 路径限制在 `document_patterns` 基目录内（Windows 大小写不敏感）。
- frontmatter 不存在文件先报 `file not found`。

### 清理

- 移除已废弃的 `compute` / `model` 配置段（旧 JSON 字段会被忽略）。
- `updater` 缓存目录改为 `data_dir`（此前误用 `db_path` 文件路径）。
- HTTP 日志与实际 bind 一致（`127.0.0.1`）；PDF 抽取页数上限 500。
- `bin/query` 改为 CLI 参数；架构文档与 README 同步到 API Embedder。

### 迁移说明

从 v3.0.0 升级通常**无需**删除 `vectors.db`（仍为 1024 维）。若曾用其它 `dimensions` 建库，请保持配置一致或删除库重建。旧配置中的 `compute` / `model` 可删可留。

---

## v3.0.0 — Embedding API 重构

将本地 ONNX 推理引擎替换为 **OpenAI 兼容 Embedding API**。

- **API 向量化**：支持 DashScope、Ollama、OpenAI、Azure OpenAI 等 `/v1/embeddings`。
- **1024 维向量**（默认），秒级启动，运行时内存约 50MB。
- 移除 `ort`、`tokenizers`、`bytemuck`、`indicatif`。
- 智能批处理 + 指数退避重试；数据目录默认 `~/.rustrag/`。

**破坏性变更**（自 v2.x）：删除旧 `vectors.db`；配置改为 `embedding` 段；移除 `compute` / `model`。

---

## v2.4.3 — sqlite-vec 初始化顺序修复

修复 sqlite-vec 自动扩展注册顺序 —— 将 `sqlite3_auto_extension` 调用移至 `Connection::open()` **之前**，确保 r2d2 连接池中每个连接都正确加载扩展。

- **修复初始化顺序**：SQLite 自动扩展仅对注册后创建的连接生效。
- **注意**：需搭配 v2.4.1+（`sqlite-vec 0.1.9`）使用。

## v2.4.2 — 文件监听器修复

修复后台文件监听器的 Bug：即使 `exclude_patterns` 中配置了 `target`、`node_modules` 等忽略目录，热重载期间这些目录内的文件仍会被意外索引。

- **监听器过滤增强**：全面应用 `exclude_patterns` 规则（底层 `ignore::OverrideBuilder`）。

## v2.4.1 — sqlite-vec 升级

维护版本，将 `sqlite-vec` 从 `0.1.7-alpha.10` 升级至稳定版 `0.1.9`。

- 修复部分平台 `no such function: vec_version` 报错。
- **注意**：从 v2.4.0 或更早升级时，需删除 `vectors.db` 并重启。

## v2.4.0 — 多格式文档支持

将 RustRAG 从代码索引引擎升级为通用文档 RAG 引擎。

- **24 种文件类型**：纯文本、JSON、YAML、TOML、CSV、HTML、PDF、DOCX、电子表格（XLS/XLSX/XLSB/ODS）。
- **格式专属分块**：保留结构信息（JSON 键路径、CSV 表头等）。
- **新依赖**（纯 Rust）：`lopdf`、`docx-rs`、`calamine`、`scraper`、`toml`、`csv`。
- 补入 `.jsx`/`.tsx` 到支持的代码扩展名。

## v2.3.0 — 安全加固与代码质量

系统化代码审查修复 26 项问题：

- **安全**：修复 Windows 路径校验，限制 MCP 工具任意文件读取，HTTP 绑定 localhost。
- **可靠性**：`assert_eq!` panic 替换为错误传播，修复索引计数器。
- **配置**：无效 JSON 返回错误而非静默回退；启动时校验维度一致性。
- **国际化**：新增日语（平假名/片假名）和韩语（韩文）识别。
- **性能**：ONNX 线程自动检测，`LazyLock` 缓存 `LanguageConfig`。
- **代码质量**：移除 PHP 死代码，修复 TOCTOU 竞态，添加 `// SAFETY:` 文档。

## v2.2.0 — 架构深度重构

针对高并发和异步可靠性的架构重构：

- **连接池**：`r2d2` 封装 `sqlite-vec`，安全多线程访问。
- **异步网络**：版本检查器从 `reqwest::blocking` 迁移至原生异步。
- **配置安全**：消除配置加载中的 TOCTOU 竞态。

## v2.1.0 — 高级功能改进

增强功能、性能和可靠性。

## v2.0.0 — ONNX 模型迁移

从 `model.onnx`（470MB）迁移至 `model_O4.onnx`（235MB）：

- ONNX Level 4 图优化 —— 向量输出一致，无需重建索引。
- 下载体积和运行时内存均减半（~500MB → ~250MB）。
- 自动检测并清理旧版模型文件。

## v1.3.7 — 配置热重载

基于 `RwLock` 的配置与模型实例热重载：

- GPU 推理引擎热重载（修改 `device` 策略后下次请求自动生效）。
- 配置文件变更自动同步文件监听任务。

## v1.3.6 — 硬件加速优化版

- 多平台 GPU 加速（CUDA、TensorRT、DirectML、CoreML），安全 CPU 回退。
- `batch_size` 调优与 `compute.fallback_to_cpu` 降级处理。
- 后台文件监听与增量同步。
- SQLite WAL 模式，解决并发写入锁定。
- 颗粒度 MCP 错误处理。

## v1.2.0 & v1.1.0 — 性能与体积优化版

- **INT8 标量量化**：`FLOAT[384]` → `INT8[384]`，磁盘占用降低约 75%。
- **ONNX Level 3 图优化**：提升 CPU 推理性能。
- **动态清理**：修改 `exclude_patterns` 后自动清除过期文档。

> ⚠️ 从 v1.1.x 升级需手动删除旧 `vectors.db`。
