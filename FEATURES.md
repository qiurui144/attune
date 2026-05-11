# Attune (OSS) — 代码功能列表

> 一份完整的代码功能清单, 用于代码 review / 文档审计 / 测试覆盖核查.
> 每条 feature 含: ID / 模块 / 测试覆盖 / 端到端可达性.

## 1. 模块功能矩阵 (attune-core)

| ID | 模块 | 主要 API | 测试 |
|----|------|---------|------|
| **C-VAULT** | `vault.rs` | setup / unlock / lock / dek_db / change_password / 设备 secret | unit |
| **C-CRYPTO** | `crypto.rs` | Argon2id 派生 / AES-256-GCM / 字段加密 / zeroize | unit |
| **C-STORE** | `store.rs` | rusqlite + 字段级加密 / item CRUD / FTS5 队列 | unit + integration |
| **C-CHUNKER** | `chunker.rs` | 滑窗分块 + 章节切割 | unit |
| **C-PARSER** | `parser.rs` | PDF/DOCX/MD/code 解析 + bytes 入口 | unit + integration |
| **C-EMBED** | `embed.rs` | Ollama / ONNX / openai_compat embedding provider | unit + ignored e2e |
| **C-LLM** | `llm.rs` | LlmProvider trait (chat / chat_with_history / **chat_multimodal**) + OpenAI compat (统一协议 + vision content array) + Ollama + Attachment (Image/TextFile) | unit + 3 multimodal + ignored e2e |
| **C-CHAT** | `chat.rs` (pub(crate)) | ChatEngine / Citation / confidence parse | unit + integration |
| **C-CHUNKER** | `chunker.rs` | DEFAULT_CHUNK_SIZE / DEFAULT_OVERLAP | unit |
| **C-CLUSTER** | `clusterer.rs` | HDBSCAN 聚类 | unit |
| **C-CLASSIFIER** | `classifier.rs` | LLM 文档分类 | unit |
| **C-INDEX** | `index.rs` | tantivy + usearch | unit + integration |
| **C-OCR** | `ocr/` | PP-OCRv5 + pdftoppm + extract_text_from_pdf | unit + ignored e2e |
| **C-ASR** | `asr.rs` | whisper.cpp subprocess | unit |
| **C-WORKFLOW** | `workflow.rs` | YAML workflow + 事件触发 | unit + integration |

## 2. Plugin 协议层 (v2)

| ID | 模块 | 功能 | 测试 |
|----|------|------|------|
| **P-LOADER** | `plugin_loader.rs` | PluginManifest v2 (pricing/resources/registers_case_kinds/skills/agents/mcps/ui) | unit + integration (paid 加密 / 明文 / trust 联动) |
| **P-LOADER-ENC** | `plugin_loader::from_dir_with_key` | 自动识别 plugin.yaml.enc 解密装载 | integration |
| **P-REGISTRY** | `plugin_registry.rs` | scan + 5 查询 API (skills/agents/mcps/case_kind/chat_trigger) | 19 unit + 10 generic_plugins_test |
| **P-REG-CHAT** | `plugin_registry::match_chat_trigger` | regex/keywords 匹配 + priority + exclude_patterns | 5 unit |
| **P-SIG** | `plugin_sig.rs` | Ed25519 keygen / sign / verify_loose / verify_strict / verify_with_key | 14 unit |
| **P-ENC** | `plugin_encryption.rs` | Argon2id + AES-GCM yaml 加密 + trust↔pricing 联动校验 | 7 unit |
| **P-DISPATCH** | `capability_dispatch.rs` | subprocess + timeout + exit_code (0/2/-1) | 8 unit |
| **P-RUNNER** | `agent_runner.rs` | run_agent_subprocess + format_for_chat | 5 unit |
| **P-SYNC** | `plugin_sync.rs` | 拉云端 entitled_plugins → download → verify → install | 7 unit |

## 3. Skill / Agent / MCP 三角色

| ID | 模块 | 功能 | 测试 |
|----|------|------|------|
| **S-DATE** | `skills/parse_chinese_date.rs` | 中文日期 → ISO 8601 (含中文数字大写) | 13 unit |
| **S-ENTITY** | `skills/extract_entities.rs` | 人名 / 日期 / 金额 / 地点 / 组织 (纯规则) | 11 unit |
| **S-CLASS** | `skills/classify_chunk_kind.rs` | 8 类 chunk 分类 (借条/合同/流水/聊天/收据/判决/身份/其他) | 10 unit |
| **S-SUM** | `skills/summarize_text.rs` | LLM 摘要 + summarize_document_set (默认禁) | 6 unit |
| **A-CLASS** | `agents/document_classifier.rs` | 编排 3 skill → ClassifiedEvidence | 6 unit + e2e |
| **A-TRAIT** | `agents/mod.rs::Agent` | trait + AgentOutput<T> (computation/audit_trail/red_lines/missing/followups/confidence) | unit |
| **MCP-CLIENT** | `mcp_client.rs` | stdio JSON-RPC + 心跳 + 重启 + id 路由 + transaction lock | 7 unit |

## 4. 案件库 / 设备 / 加密

| ID | 模块 | 功能 | 测试 |
|----|------|------|------|
| **CASE-META** | `case_metadata.rs` | CaseMetadata + Party + classified_evidence 持久化 | 4 unit |
| **DEV-BIND** | `device_binding.rs` | DeviceFingerprint + License + 1:2 状态机 | 5 unit |
| **DEV-CLIENT** | `accounts_client.rs` | HTTP client → cloud accounts (register/deactivate/verify) | 3 unit |
| **CLOUD-CLIENT** | `cloud_client.rs` | login/signup/me/list_licenses (FastAPI) + cookie 自动管理 | 4 unit |
| **LICENSE** | `license.rs` | LicenseClaims + Ed25519 签名 + base64 code + 离线校验 | 9 unit |
| **MEMBER** | `member_session.rs` | MemberState 3 档 (LoggedOut/Free/Paid) + SettingsLocks 6 字段 (vault_password/local_folder_links/cloud_llm/plugin_install/plugin_uninstall/ocr_profiles) | 6 unit |
| **LIC-CACHE** | `license_cache.rs` | ~/.config/npu-vault/license.json 持久化 (chmod 600), attune-server 启动读 + scan_with_key 自动解密 paid plugin | 5 unit |

## 5. UI runtime

| ID | 模块 | 功能 | 测试 |
|----|------|------|------|
| **UI-FORM** | `ui_runtime.rs` | yaml FormSchema → HTML (text/number/date/select/textarea/checkbox + XSS escape) | 4 unit |

## 6. attune-server 路由

| ID | endpoint | 功能 | 测试 |
|----|---------|------|------|
| **R-VAULT** | `/api/v1/vault/*` | setup/unlock/lock/change_password/device-secret | unit + integration |
| **R-UPLOAD** | `/api/v1/upload` | multipart + 100MB 限制 + backpressure | unit + integration |
| **R-INGEST** | `/api/v1/ingest` | text 写入 + 队列入 | unit + integration |
| **R-SEARCH** | `/api/v1/search` | RRF + cutoff + 黑名单 | unit + integration |
| **R-CHAT** | `/api/v1/chat` | RAG + plugin route 提示 + 三层 hallu 防御 | unit + integration |
| **R-PLUGIN** | `/api/v1/plugins` | 列出已装 plugin + match_trigger | unit |
| **R-FORM** | `/api/v1/forms/{plugin}/{form}` | GET HTML 表单 + POST 提交 → agent | 1 integration |
| **R-MEMBER** | `/api/v1/member/{state,locks,login-token,logout}` | 会员状态 + lock 决策源 | 1 integration |

## 7. attune-cli 子命令

| ID | 命令 | 功能 |
|----|------|------|
| **CLI-VAULT** | setup / unlock / lock / status | 基础 vault 管理 |
| **CLI-OCR** | ocr <image> | 单文件 OCR 测试 |
| **CLI-DEPLOY** | deploy | Linux 一键部署 Ollama |
| **CLI-PK-GEN** | plugin-keygen | Ed25519 keypair |
| **CLI-PK-SIGN** | plugin-sign | 写 plugin.sig |
| **CLI-PK-VERIFY** | plugin-verify-sig | 校验 sig |
| **CLI-PK-ENC** | plugin-encrypt | yaml → yaml.enc |
| **CLI-PK-DEC** | plugin-decrypt | yaml.enc → yaml |
| **CLI-PK-CHK** | plugin-verify | 装载链路完整校验 |
| **CLI-PK-INST** | plugin-install | 装到 ~/.local/share/attune/plugins/ |
| **CLI-PK-LIST** | plugin-list | 看已装 |
| **CLI-PK-RM** | plugin-uninstall | 删 |
| **CLI-LOGIN** | login <email> | 登录 cloud accounts |
| **CLI-SYNC** | sync-plugins | 自动装 entitled pro 插件 |
| **CLI-LINK** | link-folder <path> | 关联本地知识库目录 |

## 8. 一键部署 / 安装脚本

| ID | 脚本 | 功能 |
|----|------|------|
| **DEPLOY-CLOUD** | (引用) /data/company/cloud/cloud.sh | 服务器端 4 服务一键 |
| **INSTALL-LOCAL** | `scripts/install-local.sh` | 本地 build + setup + login + sync + systemd |
| **SMOKE-SERVER** | `scripts/smoke-test.sh` | server 启动 + API 健康 |
| **SMOKE-CLI** | `scripts/smoke-test-cli.sh` | 7 CLI 命令冒烟 |

## 9. 测试金字塔

```
        E2E (Playwright + 真集成)
       ─────────────────────────
      Integration (跨模块, ~30 tests)
     ─────────────────────────────────
    Unit (单模块, ~734 tests in attune-core)
   ──────────────────────────────────────
  Smoke (CLI 冒烟 7, Server 冒烟 N)
 ──────────────────────────────────────────
Cargo clippy + cargo check (静态)
```

## 10. 已知约束

- attune (OSS) **不内置任何行业 agent** — civil_loan_agent 等在 attune-pro
- paid plugin yaml 加密载入需要 ATTUNE_PLUGIN_KEY env (设备 license token)
- OCR / LLM 走 subprocess / HTTP, 不直接 link C++
- Web UI vite bundle 不在本仓 build, dist/ checked in
- 跨平台: Linux/Win/macOS — aarch64 (K3 一体机) 交叉编译
