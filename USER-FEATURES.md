# Attune — 用户形态功能表

> 从**用户视角**列功能 — 我作为某种用户能做什么、不能做什么、操作流程怎样.
> 与 `FEATURES.md` (代码模块视角) 互补.

## 1. 用户形态总览

| 形态 | 标识 | 网络要求 | 数据归属 | 价格 |
|------|------|---------|---------|------|
| **离线 self-host** | `LoggedOut` (未登录) | 永不联网也能用 | 全部本地 vault | 免费 |
| **免费会员** | `Free` (云端账号) | 仅注册 / 登录时联网 | 本地 vault + 云端账号 (邮箱+密码) | 免费 |
| **付费会员** | `Paid` (云端 license) | 启动时验证 license, 30 天离线缓存 | 本地 vault + 云端 license + LLM 配额 | 付费 |

## 2. 形态 × 功能矩阵

| 功能 | 离线 | 免费 | 付费 |
|------|:----:|:----:|:----:|
| **本地 vault** (Argon2id+AES-256-GCM, 主密码加密) | ✅ | ✅ | ✅ |
| **文档 ingest** (PDF/DOCX/MD/code/图片 OCR) | ✅ | ✅ | ✅ |
| **全文搜索** (tantivy + jieba 中文分词) | ✅ | ✅ | ✅ |
| **向量搜索** (usearch HNSW) | ✅ | ✅ | ✅ |
| **混合搜索** (RRF + cutoff) | ✅ | ✅ | ✅ |
| **本地知识库目录关联** (`attune link-folder`) | ✅ | ✅ | ✅ |
| **OCR 引擎切换** (PP-OCR / Tesseract) | ✅ | ✅ | ✅ |
| **批注** (含 AI 批注角度 plugin) | ✅ | ✅ | ✅ |
| **聚类** (HDBSCAN) | ✅ | ✅ | ✅ |
| **Chat 问答** (RAG + LLM) | 需自配 LLM | 需自配 LLM | 云端 gateway 自动 |
| **自配 LLM API key** | ✅ 完全自由 | ✅ 完全自由 | ❌ 锁定 (云端下发) |
| **自配 embedding 模型** | ✅ | ✅ | ❌ 锁定 (云端推荐) |
| **install free plugin** (社区) | ✅ `attune plugin-install` | ✅ | ❌ 锁定 (云端按 license 自动装) |
| **install paid plugin** | ❌ 需 license decrypt key | ❌ | ✅ 自动 `attune sync-plugins` |
| **设备绑定 1:2** | N/A (无账号) | ✅ 自动 | ✅ 严格 enforce |
| **离线工作** | 永久 | 永久 (本地 vault) | 30 天 cached license, 之后需联网 |
| **vault backup / export** | ✅ `attune vault-export` | ✅ | ✅ |
| **跨设备同步** | 手动 export/import | 手动 export/import | 手动 export/import (隐私优先, 不自动同步) |

## 3. 形态切换路径

### 3.1 离线 → 免费会员

```bash
# 用户操作
attune login alice@example.com --cloud-url https://accounts.attune.ai
# → POST /signup (如果未注册) OR /login
# → 写 ~/.config/npu-vault/license.json (含 free license code)
# → POST /member/login-token (server 端 state=Free)
```

效果:
- 大部分配置仍可改 (LLM/插件自由)
- 设备绑定生效 (防多设备共享账号)

### 3.2 免费 → 付费

```bash
# 用户在 cloud accounts /licenses 页面付款 → admin 生成 paid license
# 客户端:
attune login alice@example.com  # 重新拉 license, 写 cache
attune sync-plugins              # 自动下载 + 安装 entitled pro 插件 (law-pro / patent-pro / ...)
# 重启 attune-server
```

效果:
- LLM / 模型 / 插件 锁定, 由 cloud gateway 下发
- pluginhub 推什么 pro 插件就装什么
- 月度 LLM token 配额生效

### 3.3 任何形态 → 退出登录

```bash
attune lock                       # 关 vault
# (manual) rm ~/.config/npu-vault/license.json  # 删 cache
# 服务器端: curl POST /api/v1/member/logout
```

## 4. UI / Web 界面用户路径

| 路径 | 离线 | 免费 | 付费 |
|------|------|------|------|
| 打开 `http://127.0.0.1:18900/` | ✅ 进入 Wizard 引导 | ✅ + 自动登录拉 license | ✅ + 显示 pro 插件 / 配额 |
| Settings → LLM 配置 | 全字段可改 | 全字段可改 | LLM/模型/key 灰显 🔒 (UI 来自 `GET /member/locks`) |
| Settings → Plugin 装载 | 手动 install/uninstall | 手动 | 灰显 🔒 (自动 sync) |
| Settings → 本地知识库目录 | 可关联 | 可关联 | 可关联 (隐私自管) |
| 触发 agent (chat 命中) | 提示装 plugin | 提示装 plugin | iframe `/forms/{plugin}/{form}` 自动加载 |

## 5. CLI 命令分形态可用性

| 命令 | 离线 | 免费 | 付费 |
|------|:----:|:----:|:----:|
| `attune setup / unlock / lock / status` | ✅ | ✅ | ✅ |
| `attune insert / get / list` | ✅ | ✅ | ✅ |
| `attune ocr <image>` | ✅ | ✅ | ✅ |
| `attune deploy` | ✅ | ✅ | ✅ |
| `attune vault-export / vault-import` | ✅ | ✅ | ✅ |
| `attune login <email>` | N/A | ✅ | ✅ |
| `attune sync-plugins` | N/A | 装免费 plugin (entitled 一般为空) | 装 entitled paid plugins |
| `attune link-folder <path>` | ✅ | ✅ | ✅ |
| `attune plugin-list / plugin-install / plugin-uninstall` | ✅ | ✅ | install/uninstall 锁定 (UI 提示) |
| `attune plugin-keygen / plugin-sign / plugin-verify-sig` | ✅ 开发者 | ✅ | ✅ |
| `attune plugin-encrypt / plugin-decrypt` | ✅ 开发者 | ✅ | ✅ |
| `attune plugin-publish` (上传 pluginhub) | ❌ 需 admin token | ❌ | ❌ (仅 plugin 发布者) |

## 6. 设置项锁定矩阵 (per `GET /api/v1/member/locks`)

| 字段 | 离线 | 免费 | 付费 |
|------|:----:|:----:|:----:|
| `llm_endpoint` | ✏️ | ✏️ | 🔒 |
| `llm_model` | ✏️ | ✏️ | 🔒 |
| `llm_api_key` | ✏️ | ✏️ | 🔒 |
| `embedding_model` | ✏️ | ✏️ | 🔒 |
| `ocr_engine` | ✏️ | ✏️ | ✏️ (本地装载, 可换) |
| `data_dir` | ✏️ | ✏️ | 🔒 (防误操作) |
| `local_folder_links` | ✏️ | ✏️ | ✏️ (用户隐私) |
| `plugin_install` | ✏️ | ✏️ | 🔒 (云端自动) |
| `plugin_uninstall` | ✏️ | ✏️ | 🔒 (防误删) |
| `vault_password` | ✏️ | ✏️ | ✏️ (用户主密码自管) |
| `device_binding` | ✏️ | 🔒 | 🔒 |
| `backup_destination` | ✏️ | ✏️ | ✏️ (用户隐私) |

## 7. 数据隐私边界

| 数据 | 永远本地 | 同步云端 |
|------|---------|---------|
| **vault 加密内容** (item / annotation / chat history) | ✅ | ❌ |
| **本地知识库目录文件** | ✅ | ❌ |
| **本地配置 / settings** | ✅ | ❌ |
| **vault 主密码** | ✅ 仅用户脑 + chmod 600 device.key | ❌ 永不上传 |
| **device fingerprint** (host/os/cpu hash) | ✅ | ✅ (1:2 绑定 enforce 用) |
| **账号 email / 订阅状态** | — | ✅ |
| **license_code** (签名 token) | ✅ cached ~/.config/npu-vault/license.json (chmod 600) | ✅ |
| **LLM 调用记录** | ✅ (chat history 在本地 vault) | gateway 统计 token 用量 (不存 prompt 内容) |

## 8. 设备绑定 1:2 规则 (per attune-plugin-protocol §10)

| 场景 | 行为 |
|------|------|
| 第 1 台新设备 | 自动注册 ✅ |
| 第 2 台新设备 | 自动注册 ✅ |
| 第 3 台新设备 | 409 → UI 提示选择踢下线某台 OR 取消 |
| 同设备重新 login | 自动续期 30 天 license, 不占新 slot |
| 设备离线 > 30 天 | cached license 过期, 需联网刷新一次 |
| 用户主动 deactivate | 该设备 30s 后无法用 license_token |

## 9. 离线工作时长

| 形态 | 离线续航 |
|------|---------|
| 离线 self-host | 永久 (从不联网) |
| 免费会员 | 永久 (本地 vault, 不依赖 license 验证) |
| 付费会员 | 30 天 (cached license 有效期, 之后需联网 1 次刷新) |

## 10. 用户决策导引

```
我刚装 attune, 想试试 → 离线 self-host (skip login, attune setup → 用)
我想用云端推荐配置 + 简单设置 → 免费会员 (attune login)
我想要 pro 插件 (律师/财务/医疗等) + 云端 LLM gateway → 付费会员 (购买 license, attune sync-plugins)
我是 plugin 开发者 → 任意形态 + 用 `plugin-keygen / plugin-sign / plugin-publish` 发布
```

## 11. 与本仓 FEATURES.md (代码视角) 互补

- `FEATURES.md` 列**代码模块** (plugin_loader, cloud_client, agent_runner...)
- `USER-FEATURES.md` (本文) 列**用户能做什么**

两者交叉点:
- 代码 `SettingsLocks` (FEATURES.md §3) → 用户 "字段锁定矩阵" (USER-FEATURES.md §6)
- 代码 `accounts_client / cloud_client` (FEATURES.md §4) → 用户 "形态切换" (USER-FEATURES.md §3)
- 代码 `plugin_sync` (FEATURES.md §2) → 用户 "免费→付费 自动装 pro 插件" (USER-FEATURES.md §3.2)
