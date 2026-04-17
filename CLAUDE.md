# attune

个人知识库 + 记忆增强系统。通过 Chrome 扩展在 AI 对话和日常浏览中自动捕获、检索、注入知识，利用 Ollama / NPU / iGPU 闲置算力处理 embedding。

## 双产品线架构

本仓库包含两条并行的产品线，共享 Chrome 扩展协议（`/api/v1/*`）：

1. **Python 原型线** (`src/npu_webhook/`) — 实验/验证
   - FastAPI + ChromaDB + SQLite FTS5
   - 快速迭代新特性和算法
   - 78 tests，持续增长

2. **Rust 商用线** (`rust/`) — 生产/发布
   - Axum + rusqlite + tantivy + usearch + hdbscan
   - 加密模型：Argon2id + AES-256-GCM + Device Secret
   - 定位：**私有 AI 知识伙伴**（主动进化 + 对话式 + 混合智能，详见 `docs/superpowers/specs/2026-04-17-product-positioning-design.md`）
   - TLS NAS 模式 + 嵌入式 Web UI (9 标签页) + Chrome 扩展兼容
   - AI 自动分类 + HDBSCAN 聚类 + 编程/法律/专利/售前行业插件
   - 浏览器自动化网络搜索（chromiumoxide 驱动系统 Chrome，零 API 费用）
   - SkillClaw 风格后台自动技能进化（失败信号 → LLM 扩展词 → 静默生效）
   - 行为画像 + 画像导出/导入 + WebDAV 远程目录
   - 237+ tests（210 attune-core + 27 attune-server），独立 README/DEVELOP/RELEASE
   - 最新里程碑：v0.5.x 改名为 Attune + 浏览器搜索重构完成

**测试策略**：`docs/TESTING.md` 固化了产品级测试方案 — 六层测试金字塔、GitHub 真实知识仓库作为语料（rust-lang/book、CyC2018/CS-Notes 等版本固化）、golden set 质量回归、禁止随机测试数据。添加任何 feature 前先参考该文档的测试矩阵。

Python 验证后，择优特性迁移到 Rust 商用线。对应开发时根据任务选择目录：
- 涉及算法实验、ML 集成、快速原型 → 改 Python 端
- 涉及加密、性能、打包分发、生产部署 → 改 Rust 端

## 技术栈（Python 原型线）

- 后端: FastAPI + Uvicorn, Python 3.11+
- 向量库: ChromaDB (嵌入式, cosine 相似度)
- 全文搜索: SQLite FTS5 + jieba 分词（LIKE 回退）
- Embedding: Ollama bge-m3 (默认) / ONNX Runtime (CPU/DirectML/ROCm) / OpenVINO (Intel NPU/iGPU)
- Chrome 扩展: Manifest V3 + Preact + Vite 多阶段构建
- 打包: PyInstaller + AppImage (Linux) / NSIS (Windows)

## 技术栈（Rust 商用线，rust/）

- 后端: Axum 0.8 + Tokio + rustls TLS
- 数据库: rusqlite + 字段级 AES-256-GCM 加密
- 全文搜索: tantivy 0.22 + tantivy-jieba（中文分词）
- 向量搜索: usearch HNSW + f16 量化
- 加密: argon2 + aes-gcm + zeroize 纯 Rust 密码学
- Web UI: 嵌入式单页 HTML + vanilla JS（`include_str!`）
- CLI: clap + rpassword
- AI 分类: Ollama chat (qwen2.5) + hdbscan 聚类 + 编程/法律插件
- 分发: 单一静态二进制 28 MB（含 TLS + 搜索引擎 + Web UI + 分类引擎）

## 已实现模块（Phase 0-3）

### 后端
- `main.py` — lifespan 全链路初始化、路由注册、认证中间件
- `config.py` — YAML 配置 + Pydantic Settings，默认模型 bge-m3, device auto
- `core/embedding.py` — OllamaEmbedding (HTTP API) / ONNXEmbedding / OpenVINO (Phase 4)
- `core/search.py` — RRF 混合搜索引擎 + 两阶段层级检索 (search_relevant) + 动态注入预算
- `core/chunker.py` — 滑动窗口分块 + extract_sections() 语义章节切割
- `core/parser.py` — 文件解析 (MD/TXT/代码/PDF/DOCX) + parse_bytes() 内存解析
- `db/sqlite_db.py` — SQLite (schema/CRUD/FTS5/embedding 队列，含 level/section_idx)
- `db/chroma_db.py` — ChromaDB 封装
- `scheduler/queue.py` — Embedding 队列 Worker (后台线程，metadata 含 level/section_idx)
- `indexer/watcher.py` — watchdog 多目录监听
- `indexer/pipeline.py` — 解析→两层入队（章节 Level1 + 段落块 Level2）→存储→embedding 管道
- `platform/detector.py` — 芯片级硬件检测 + 驱动匹配 + 一键安装命令
- `tray.py` — 系统托盘入口（pystray + uvicorn daemon 线程）
- API: ingest / upload / search / items / index / status / settings / models / ws

### Chrome 扩展
- `content/detector.js` — 平台适配器 (ChatGPT/Claude/Gemini, extractMessage/isComplete/setInputContent)
- `content/capture.js` — MutationObserver 对话捕获 (djb2 去重, 2s debounce)
- `content/injector.js` — 无感前缀注入 (capture phase 拦截，动态预算)
- `content/indicator.js` — 4 状态指示器 (disabled/processing/captured/offline)
- `background/worker.js` — 消息路由 + 去重缓存 (session storage) + 30s 健康检查 + 会话感知加权
- `popup/Popup.jsx` — 连接状态 / 统计 / 注入开关
- `options/Options.jsx` — 后端地址 / 注入模式 / 排除域名 / 测试连接
- `sidepanel/` — 搜索 / 时间线 / 文件 (拖拽上传, uid 并发安全) / 状态
- `shared/messages.js` — 统一消息类型（含 FILE_UPLOADED）+ 通信辅助
- `shared/api.js` — 后端 API 封装 (动态 baseUrl, 含 uploadFile)

## 产品决策记录

- **Chat 流式输出**：attune Chat 不实现流式输出（SSE streaming）。等待 LLM 响应期间，Web UI 显示加载指示器（spinner）即可。原因：本地 0.6B-3B 模型响应快，云端 API 等待时有 loading 状态满足体验需求，实现复杂度不值得。

## 开发规范

- Python 代码使用 ruff 格式化和 lint（line-length=120）
- 类型注解: 所有公开函数必须有类型注解
- 测试放 `tests/` 目录, 使用 pytest（当前 78 个测试: 36 后端单元 + 42 扩展 E2E）
- 扩展 E2E 测试使用 Playwright Chromium（非 Google Chrome）
- 调试代码放 `tmp/`, 使用后删除
- API 路径前缀: `/api/v1/`
- 后端端口: 18900
- 使用 venv 管理 Python 依赖
- pip 使用清华源

## 项目结构

- `src/npu_webhook/` — Python 后端
- `extension/` — Chrome 扩展（Manifest V3 + Preact + Vite）
- `packaging/` — 打包配置（PyInstaller/AppImage/NSIS）
- `.github/workflows/` — CI/CD
- `tests/` — 测试代码 + conftest.py

## Rust 商用线跨平台兼容规范

### 目标平台矩阵

attune 必须在以下平台 + 硬件组合上可编译、可运行、测试通过：

| 平台 | 架构 | Rust target | 状态 |
|------|------|-------------|------|
| Linux x86_64 | Intel/AMD CPU | `x86_64-unknown-linux-gnu` | 主开发平台 |
| Linux x86_64 + NVIDIA GPU | Intel/AMD CPU + CUDA GPU | 同上，Ollama 自动使用 GPU | 验证 |
| Linux aarch64 | ARM64 (树莓派/NAS) | `aarch64-unknown-linux-gnu` | 交叉编译 |
| Windows x86_64 | Intel/AMD CPU | `x86_64-pc-windows-msvc` | 交叉编译或原生 |
| Windows x86_64 + NVIDIA GPU | Intel/AMD CPU + CUDA GPU | 同上，Ollama 使用 GPU | 验证 |

### 跨平台编译注意事项

**纯 Rust 依赖**（零跨平台风险）：
- argon2, aes-gcm, zeroize, hmac, sha2 — 纯 Rust 密码学
- tantivy, tantivy-jieba — 纯 Rust 全文搜索
- hdbscan — 纯 Rust 聚类
- axum, tokio, tower-http, reqwest, rustls — 纯 Rust 网络栈
- serde, serde_json, serde_yaml, clap, chrono, uuid — 纯 Rust 工具

**含 C/C++ 绑定的依赖**（需要交叉编译工具链）：
- `rusqlite (bundled)` — 内嵌 SQLite C 源码编译，需要 C 编译器（`cc` crate 自动检测）
- `usearch` — C++ HNSW 实现，需要 C++ 编译器，Windows 需要 MSVC

**交叉编译指南**：
```bash
# Linux → Windows (需要 mingw-w64 或 MSVC 交叉编译器)
rustup target add x86_64-pc-windows-gnu
# usearch 的 C++ 代码可能需要额外配置，建议在 Windows 原生编译

# Linux → aarch64 (需要 aarch64 交叉编译器)
sudo apt install gcc-aarch64-linux-gnu g++-aarch64-linux-gnu
rustup target add aarch64-unknown-linux-gnu
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build --target aarch64-unknown-linux-gnu --release
```

### GPU / NPU 兼容性

**核心原则**: attune **本身不直接使用 GPU/NPU**。所有 AI 推理通过 HTTP 调用外部 Ollama 服务完成。因此：

- **NVIDIA GPU**: 用户安装 NVIDIA 驱动 + CUDA → 安装 Ollama → Ollama 自动检测并使用 GPU → attune 通过 HTTP 调用，零 GPU 代码
- **AMD GPU (ROCm)**: Ollama 支持 ROCm → 同上
- **Intel iGPU/NPU**: Ollama + OpenVINO 后端（实验性） → 同上
- **纯 CPU**: Ollama CPU 模式，qwen2.5:3b 在现代 CPU 上 2-3 秒/条分类

**开发时不需要 GPU**：所有测试使用 MockLlmProvider + MockEmbeddingProvider，CI 无需 GPU。

### Ollama 多平台安装

| 平台 | 安装命令 | GPU 自动检测 |
|------|---------|-------------|
| Linux | `curl -fsSL https://ollama.com/install.sh \| sh` | NVIDIA (CUDA), AMD (ROCm) |
| Windows | 下载 OllamaSetup.exe | NVIDIA (CUDA) |
| macOS | `brew install ollama` 或下载 .dmg | Apple Silicon (Metal) |

安装后统一使用：
```bash
ollama pull bge-m3      # embedding 模型
ollama pull qwen2.5:3b  # chat/分类模型
```

### Rust 代码跨平台规范

在 attune 的 Rust 代码中，必须遵守以下跨平台规则：

1. **文件路径**: 使用 `std::path::PathBuf` 和 `dirs` crate，禁止硬编码 `/` 或 `\`
2. **权限**: `#[cfg(unix)]` 保护 `set_permissions(0o600)` 等 Unix 特有调用
3. **进程管理**: 使用 `std::process::Command` 跨平台，不依赖 shell 特性
4. **网络**: 使用 `reqwest` + `rustls`（纯 Rust TLS），不依赖系统 OpenSSL
5. **临时文件**: 使用 `tempfile` crate，不硬编码 `/tmp`
6. **换行符**: 文件解析不假设 `\n`，使用 `.lines()` 方法自动处理 `\r\n`
7. **编码**: 文件读取使用 `String::from_utf8_lossy` 容错，不 panic
8. **C/C++ 依赖**: `rusqlite` 用 `bundled` feature 自带 SQLite；`usearch` 需要 C++ 编译器，CI 矩阵必须验证
9. **条件编译**:
   ```rust
   // 正确: 用 cfg 保护平台特定代码
   #[cfg(unix)]
   { std::fs::set_permissions(&path, Permissions::from_mode(0o600))?; }
   
   // 错误: 不要直接调用 Unix API
   // std::os::unix::fs::PermissionsExt  // 仅在 #[cfg(unix)] 内使用
   ```

### CI 构建矩阵（规划）

```yaml
strategy:
  matrix:
    include:
      - os: ubuntu-latest
        target: x86_64-unknown-linux-gnu
        name: Linux x86_64
      - os: ubuntu-latest
        target: aarch64-unknown-linux-gnu
        name: Linux aarch64
        cross: true
      - os: windows-latest
        target: x86_64-pc-windows-msvc
        name: Windows x86_64
```

每个 target 需要：
1. `cargo build --target $target --release` — 编译通过
2. `cargo test` — 仅在 native target 运行（交叉编译不跑测试）
3. 产物上传为 release artifact

### 测试隔离规范

所有测试必须满足以下跨平台约束：
- 使用 `tempfile::TempDir` 创建临时目录，不依赖 `/tmp`
- 不假设 Ollama 可用 — 使用 `MockLlmProvider` / `NoopProvider`
- 不假设 GPU 存在 — 纯 CPU 测试
- 不使用 `std::process::Command("sh")` — 如果需要进程交互，用跨平台方式
- SQLite `PRAGMA` 在所有平台行为一致（WAL 模式在 Windows/Linux 都支持）

## 芯片-驱动匹配

detector.py 中维护了精确匹配表:
- Intel: INTEL_NPU_CHIPS (meteor_lake/lunar_lake/arrow_lake) + INTEL_IGPU_CHIPS (alder~arrow)
- AMD: AMD_NPU_CHIPS (phoenix/hawk_point/strix_point/krackan_point)
- 每个芯片条目包含: PCI ID、最低内核版本、固件路径、最低驱动版本、已知问题
- /models/check API 输出完整检测报告 + 一键安装命令
