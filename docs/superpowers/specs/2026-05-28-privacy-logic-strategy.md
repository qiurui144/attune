# Privacy Logic Strategy — attune SSOT

> 状态: Draft v1（2026-05-28 立案）
> 触发: 用户 v1.0 暴击 #8「我们的隐私逻辑策略也需要明确展示」
> Spec 责任人: 隐私 / Security 子领域
> 关联代码: `rust/crates/attune-core/src/{vault.rs, pii/, cloud_client.rs, sync/, chat.rs}` + `rust/crates/attune-server/src/routes/dsar.rs`
> 关联前置: `docs/superpowers/specs/2026-03-31-npu-vault-design.md`（vault 加密）+ `docs/superpowers/specs/2026-04-17-product-positioning-design.md`（1Password 式私密 positioning）

---

## 1. 目标定位

**用户痛点 / Why now**

attune 整个产品 positioning 一句话:**Private AI Knowledge Companion — Local-first, globally augmented, increasingly attuned to your expertise**。CLAUDE.md 三产品矩阵节明示「数据完全隔离: attune 的 vault / 批注 / chat / Project 永远在用户本地（或用户自己的 K3）」,但实际隐私边界散落于:

- `vault.rs`（DB 加密 Argon2id + AES-256-GCM）
- `pii/`（出网 redactor 3 层流水线）
- `cloud_client.rs`（cloud accounts SaaS 登录 / pro license / LLM gateway）
- `sync/webdav.rs`（用户自配 WebDAV 远端目录）
- `chat.rs`（LLM provider 调用,可能云端）

→ 用户无法在一处看清「我的数据**何时**、**为什么**、**到哪里**离开本机」。这是 1Password 类产品的核心承诺 — 没有 SSOT 等同于没有承诺。

**产品 positioning 对齐**

| 三产品矩阵层 | 隐私默认 | 本 spec 覆盖 |
|---|---|---|
| attune (OSS 通用) | 本地优先,5 个出网点全 opt-out | ✅ 主战场 |
| attune-pro (个人行业) | plugin 层继承 OSS 隐私契约 | ✅（plugin 不允许放宽 OSS 默认） |
| attune-enterprise (B2B SaaS) | 不在 attune 范围,有自己的 DPIA / DPA | ❌ 边界,本 spec 不覆盖 |

本 spec 是**OSS attune 端隐私 SSOT**。任何后续 capability 引入新出网点必须先扩本 spec → user 评审 → 才能 implementation。

**核心承诺(对外可宣传四条)**

1. **本地原生默认**: vault DB / 索引 / 向量 / 文件原文 — 全部 stay local,加密码学边界默认即开
2. **出网最少化**: 出网点固化为 5 个,全部用户可见可关
3. **PII 自动脱敏**: 任何走云端 LLM / Web search 的文本必经 `pii::Redactor` 3 层流水线;placeholder 在响应中还原
4. **用户主权操作**: GDPR Art.15 / Art.17 / Art.20 + 中国 PIPL §44-50 全部 DSAR 操作有 endpoint

---

## 2. 范围边界

**做(本 spec 责任)**:

- 列出 attune **现有** + **v1.1 内可预见**的全部出网点(穷举法,不允许"还有别的")
- 为每个出网点定义:数据形态 / 默认开/关 / opt-out UI 路径 / 数据残留风险 / 缓解
- 加密边界 SSOT(vault 内 / vault 外 / 内存中 / 网络中 各阶段)
- DSAR 操作矩阵(本地数据 / cloud 端数据)
- Telemetry / Crash report 策略
- 第三方 LLM provider 数据残留 audit + 用户告知文案
- Privacy audit 自检脚本(后续 ship 前 gate)

**不做(留给其他 spec 或 v.x+)**:

- 端到端加密同步协议设计(留 v1.2 spec)— 本 spec 仅约束「WebDAV 出网 = 用户自托管 = attune 不负责传输层加密」
- 联邦学习 / 差分隐私模型训练(v2.x,attune 不训练用户数据)
- attune-enterprise B2B 端 DPIA(独立产品,独立合规) → 走 attune-enterprise 自己的 spec
- 法律文本最终 publish 形态(本 spec 给 model;最终 ToS/Privacy 由律师定稿,见 v1.0.8)
- 加密算法升级路径(Argon2id 参数 / AES 模式选型) — 沿用 vault spec

**v1.0.x 实施口径**:本 spec 不引入新代码,只**梳理 + 落 SSOT 文档 + Privacy UI 页面**。代码层加固分到:

| v 版本 | 增量 |
|---|---|
| v1.0.4 | DSAR 完整(已 #166 完成 base) |
| v1.0.8 | Privacy publish 终态(ICP) |
| v1.1.0 | Telemetry opt-in toggle UI(若决定加 telemetry) |

---

## 3. 架构数据流

### 3.1 ASCII 数据流图(出网 5 点 + 加密边界)

```
                          ┌──────────────── 本机用户磁盘 ────────────────┐
                          │                                                  │
   用户操作               │   ┌─────────── Vault Boundary ───────────┐    │
   (CLI / Web UI /        │   │  master_key (Argon2id from password)  │    │
   Chrome extension)      │   │   ├ dek_db  → AES-256-GCM SQLite       │    │
        │                 │   │   ├ dek_idx → AES-256-GCM tantivy idx  │    │
        ▼                 │   │   └ dek_vec → AES-256-GCM usearch HNSW │    │
   ┌──────────┐           │   │                                          │    │
   │ Ingest   │──parse──▶│   │  items.content / annotations / chat_log │    │
   │ (file /  │           │   │  / project_data / skill_state           │    │
   │ paste /  │           │   │                                          │    │
   │ extension)│           │   └──────────────────────────────────────────┘    │
   └──────────┘           │           │                                        │
                          │           │ (vault unlocked, in-RAM zeroize)       │
                          │           ▼                                        │
                          │   ┌────────────────────────┐                       │
                          │   │ Search / RAG engine    │                       │
                          │   │ (BM25 + HNSW + rerank) │                       │
                          │   │  — 纯本地, 零出网       │                       │
                          │   └─────────┬──────────────┘                       │
                          │             │                                       │
                          │             │ snippets + user query                 │
                          │             ▼                                       │
                          │   ┌────────────────────────┐                       │
                          │   │ pii::Redactor (L1+L2+L3)│ ← PII placeholder   │
                          │   │   12 patterns + dict + NER │                  │
                          │   └─────────┬──────────────┘                       │
                          │             │ redacted prompt                       │
                          │             │                                       │
                          └─────────────┼───────────────────────────────────────┘
                                        │
            ╔═══════════════════════════╪═══════════════════════════════════╗
            ║       OUT-OF-BOUND (5 网络出网点,全部 opt-out)                  ║
            ╠═══════════════════════════╪═══════════════════════════════════╣
            ║                           │                                     ║
            ║   ① LLM token call ◀──────┘                                    ║
            ║      ├ Attune Pro Gateway (gateway.engi-stack.com)             ║
            ║      ├ BYOK OpenAI / Anthropic / Gemini / DeepSeek             ║
            ║      └ 本地 Ollama (零出网, 默认推荐路径)                       ║
            ║                                                                 ║
            ║   ② Cloud SaaS sync (cloud_client.rs)                          ║
            ║      ├ accounts SSO (login / session / pro entitlement)        ║
            ║      ├ pluginhub (pro plugin pack 下载)                         ║
            ║      └ DSAR endpoint (用户主动)                                 ║
            ║                                                                 ║
            ║   ③ WebDAV remote (sync/webdav.rs)                             ║
            ║      └ 用户自配 Nextcloud / NAS — attune 不在中间               ║
            ║                                                                 ║
            ║   ④ Web search (web_search_browser.rs)                         ║
            ║      └ 浏览器自动化 Bing/Google 公共网页(无 API key)            ║
            ║                                                                 ║
            ║   ⑤ Telemetry / Crash report                                   ║
            ║      └ 默认关闭, 用户主动 opt-in 才发送                          ║
            ║                                                                 ║
            ╚═════════════════════════════════════════════════════════════════╝
```

### 3.2 DB tables(隐私相关)

| Table | 加密 | 内容 | 触发清除 |
|---|---|---|---|
| `items` | dek_db | 文件 / paste / 浏览捕获 content | `DELETE /items/:id`,DSAR Art.17 |
| `annotations` | dek_db | 批注 source=user/ai | DSAR Art.17 |
| `chat_history` | dek_db | 用户对话 log | "清空对话历史" UI |
| `skill_state` | dek_db | self_evolving skill 扩展词 / 失败信号 | 用户禁用 skill 即清 |
| `browse_signals` | dek_db | extension G1 浏览 dwell/scroll/copy | `DELETE /browse_signals?domain=` |
| `cloud_session` | dek_db | cloud accounts session cookie | 登出立删 |
| `telemetry_queue` (待加) | dek_db | 待上报 telemetry 事件 | opt-out 即清 |

**加密边界声明**:

- vault 上锁状态下,**所有上述 table 内容在磁盘上是 AES-256-GCM 密文**,即使 disk 物理被偷也不可读
- vault unlocked 后,DEK 只在进程 RAM(`UnlockedKeys { dek_db, dek_idx, dek_vec }`),`zeroize` crate 保证 process exit / lock 时清零
- master_key 派生用 Argon2id(64MB memory cost),抵 GPU 暴力破解
- 4 小时 session TTL(`SESSION_TTL_SECS = 4 * 3600`),超时自动 lock

### 3.3 network boundary

| 出网点 | 协议 | 默认 | 加密 in transit |
|---|---|---|---|
| ① LLM token | HTTPS POST | 关(wizard 引导后开) | TLS 1.2+ via rustls |
| ② cloud SaaS | HTTPS REST | 关(用户主动登录后开) | TLS 1.2+ via rustls |
| ③ WebDAV | HTTPS / HTTP | 关(用户自配后开) | 取决于用户配置,HTTP 警告 |
| ④ Web search | HTTPS(headless Chrome) | 关(用户开关后开) | TLS via Chromium |
| ⑤ Telemetry | HTTPS POST | **永远关,需 opt-in** | TLS via rustls |

---

## 4. 模块边界

| crate / module / file | 隐私职责 |
|---|---|
| `attune-core/src/vault.rs` | master_key + DEK 派生,lock/unlock/seal 状态机 |
| `attune-core/src/crypto.rs` | AES-256-GCM + Argon2id + Key32 zeroize |
| `attune-core/src/pii/mod.rs` | redact_batch + restore 出网中间件 |
| `attune-core/src/pii/patterns.rs` | L1 正则(12 PII 模式) |
| `attune-core/src/pii/dictionary.rs` | L1 词典 |
| `attune-core/src/pii/ner.rs` | L2 ONNX NER(T1+ 可下载) |
| `attune-core/src/cloud_client.rs` | cloud accounts 会话 + DSAR proxy 客户端 |
| `attune-core/src/sync/webdav.rs` | WebDAV 出网,凭证存 vault 加密配置 |
| `attune-core/src/chat.rs` | RAG → redact → LLM → restore |
| `attune-core/src/web_search_browser.rs` | 浏览器自动化 web search |
| `attune-server/src/routes/dsar.rs` | DSAR REST 端点(export / delete / rectify) |
| `attune-server/src/routes/settings.rs` | privacy 设置(opt-in/out) |
| `attune-server/ui/src/views/PrivacyView.tsx`(新) | 隐私 dashboard + 数据流可视化 + 出网开关 |

**新文件清单**:

- `attune-server/ui/src/views/PrivacyView.tsx` — 用户可见的隐私 dashboard
- `docs/PRIVACY.md` — user-facing 隐私 SOP(英文优先 + zh 节内,per 全局文档铁律)
- `docs/PRIVACY-AUDIT-CHECKLIST.md` — internal 月度 audit checklist
- `scripts/privacy-audit.sh` — grep 守卫 + LLM call inventory + telemetry call inventory

---

## 5. API 契约

### 5.1 Privacy 设置 endpoint

```
GET /api/v1/privacy/status
  → {
      "vault": { "state": "unlocked", "unlocked_since": "...", "ttl_remaining_secs": 12345 },
      "outbound": {
        "llm": { "enabled": true, "provider": "ollama-local|byok|gateway", "endpoint": "..." },
        "cloud_saas": { "enabled": false, "session_active": false },
        "webdav": { "enabled": false, "remote": null },
        "web_search": { "enabled": false },
        "telemetry": { "enabled": false }
      },
      "redactor": { "patterns_active": 12, "ner_loaded": true, "llm_redact_loaded": false },
      "last_dsar_export": null
    }

PATCH /api/v1/privacy/settings
  body: { "telemetry": false, "web_search": false, ... }
  → { "ok": true, "applied": {...} }

POST /api/v1/privacy/lock
  → vault 立即 lock,DEK 清零

POST /api/v1/privacy/wipe-cloud-session
  → cloud_session table 清,cookie 失效
```

### 5.2 DSAR endpoint(沿用 v1.0.4 #166)

```
POST /api/v1/dsar/export       — GDPR Art.15 + Art.20 数据导出(JSON + 附件)
POST /api/v1/dsar/delete       — GDPR Art.17 erasure(本地 + cloud 双删)
POST /api/v1/dsar/rectify      — GDPR Art.16 改正
GET  /api/v1/dsar/audit-log    — 用户自己的 DSAR 历史
```

详见 `attune-server/src/routes/dsar.rs` 现实现 + `DSAR-USER-GUIDE.md`。

### 5.3 Audit log schema

```json
{
  "id": "uuid",
  "ts": "ISO8601",
  "kind": "outbound_llm_call | dsar_export | settings_changed | vault_lock | webdav_sync",
  "outcome": "ok | error | refused",
  "redacted_meta": {
    "outbound_endpoint": "https://gateway.engi-stack.com/v1/chat/completions",
    "bytes_sent": 1234,
    "model": "deepseek-v4-pro"
  }
}
```

**绝不入 audit log**:user prompt 原文 / LLM 响应原文 / 密钥 / cookie / token。

---

## 6. 扩展点(新增 audit 项 / 新 redactor pattern / 新出网点)

### 6.1 新出网点引入流程

任何 PR 引入新的网络出网 call(`reqwest::get` / `tokio_tungstenite::connect` / 任何子进程调外部 HTTP) **必须**:

1. 同 PR 改 `docs/superpowers/specs/2026-05-28-privacy-logic-strategy.md` §3 出网点表
2. 在 `routes/privacy.rs` 加 toggle field
3. 默认 `false`(用户必须主动开)
4. UI `PrivacyView.tsx` 加可见开关
5. `scripts/privacy-audit.sh` grep 守卫 0 输出

未走完 = 拒绝 merge。

### 6.2 新 PII pattern 引入

`attune-core/src/pii/patterns.rs`:

```rust
pub fn register_pattern(name: &str, regex: &str, placeholder: &str);
```

- 测试用例 ≥3 真实样本进 `tests/golden/pii_<name>.yaml`
- F1 ≥ 0.95(per pii spec)

### 6.3 plugin 层隐私继承

attune-pro plugin pack 通过 `with_redactor(custom_patterns)` 注入行业 PII (病案号 / 案号 / 专利号),**不能放宽** OSS 12 patterns 默认集 — `Redactor::with_extra()` 只增不减。

---

## 7. 错误处理 + 边界 case

| 边界 | 行为 |
|---|---|
| Vault locked 时 LLM call | 返回 `401 Locked` + 错误码 `vault-locked`,前端跳锁屏 |
| Redactor L2 NER 加载失败 | 降级到 L1,UI 提示「PII 保护降级,建议本地 LLM」 |
| Cloud session 过期 | 静默自动 sign-out,UI 跳登录(不阻断本地操作) |
| WebDAV 凭证错(401 / TLS fail) | sync paused,UI 红色 banner,**不重试**避免凭证耗尽 |
| Telemetry queue 满(用户从 opt-in → opt-out) | 立即清队列,不允许「最后一批」上报 |
| DSAR export 中途崩溃 | export 走 atomic temp + rename,失败留 partial 但不污染主 DB |
| LLM call 上游残留(无法验证) | UI 在 model card 显示对应 provider 的「Data retention: 30d / does not train on user data / ...」,用户进入即看到 |
| Crash report 含 vault path / 用户名 | scrub: 路径 normalize 到 `<HOME>/.../attune`,用户名 hash,**绝不**含 master_key / DEK |

---

## 8. 成本契约

| 隐私组件 | 成本层 | 触发 |
|---|---|---|
| `pii::Redactor` L1(regex+dict) | 🆓 零成本 CPU | 每次 outbound LLM/web 自动跑 |
| `pii::Redactor` L2(ONNX NER) | ⚡ 本地算力,~10-50ms/chunk | 加载后默认开;低硬件可降级到 L1 |
| `pii::Redactor` L3(LLM 脱敏) | 💰 时间/金钱 OR ⚡ 本地 LLM | T3+ 硬件 / K3 默认开;笔电 opt-in |
| Vault encrypt / decrypt(AES-256-GCM) | 🆓 CPU 亚毫秒级 | 每次读写 DB 自动 |
| DSAR export(全 vault → JSON+attach) | ⚡ 本地算力,数秒-分钟取决于体量 | 用户主动按钮 |
| Telemetry 发送(若 opt-in) | 🆓 网络字节级 | 每 24h 一次 |
| Privacy audit `scripts/privacy-audit.sh` | 🆓 CPU 秒级 | CI / 本地 pre-commit |

**UI 显示规则**:任何「开/关一个出网点」的 toggle 旁必须显示**对应成本**(per CLAUDE.md 成本感知契约)。例:启用 LLM gateway → `~$0.001/1K tok via DeepSeek`;启用 WebDAV → `本地带宽,无 token`。

---

## 9. 测试矩阵

| 类型 | 下限 | 例子 |
|---|---|---|
| **Golden case**(PII redactor) | ≥10 真实样本 / pattern,12 pattern × 10 = ≥120 | `pii/tests/golden/phone_*.yaml` 等 |
| **Property test**(redact↔restore 可逆) | proptest 1000 case | `redact(s) → restore(LLM_resp) == 原 PII 字符串` |
| **Boundary**(vault state 转移) | ≥5 case | sealed→locked / locked→unlocked / TTL 边界 / re-lock /  破口尝试 |
| **Error**(out-of-bound fail) | ≥10 case | LLM 502 / cloud 401 / WebDAV TLS fail / DSAR partial / disk full / cookie expire / NER 缺失 / pattern panic / network DNS fail / TTL 过期途中 |
| **Integration**(端到端隐私) | ≥3 真 LLM call | (1) 含 PII prompt → LLM 看到 placeholder → 响应还原 PII;(2) DSAR export → import → 数据对齐 hash;(3) opt-out telemetry 后 0 网络包(tcpdump 验) |
| **Adversarial** | ≥5 case | prompt inject 让 LLM 输出 placeholder 真值 / DSAR 伪请求(没登录) / cookie 偷 / 浏览器 extension 越权 / WebDAV 中间人 |
| **审计脚本** | `scripts/privacy-audit.sh` 0 输出 | 所有出网 grep / telemetry call / api key 硬编码 |

**6 类下限对应**:happy / edge / error / adversarial / 多用户 / 资源耗尽 — 全部覆盖。

**multi-seed**:LLM-based L3 redact ≥3 seed 跑,F1 ≥ 0.85(per Agent 验证铁律 LLM 阈值)。

---

## 10. 向后兼容

| 场景 | 兼容 SOP |
|---|---|
| 老 vault(v0.6.x 之前)无 audit log table | 启动时 migration 添加,空表起步 |
| 老 client(extension v0.6.0)无 privacy 字段 | extension 不读 `/privacy/status`,后端兼容旧路径;新字段属 additive |
| 老用户从未见过 PrivacyView | v1.0.x 首次启动跑 once-only "Privacy Tour" 弹窗,讲解 5 个出网点(可关) |
| Telemetry 默认值变更 | 永远默认 `false`,即使后续推 opt-in 推荐也不改默认 |
| DSAR schema bump | export JSON 含 `schema_version`,后续 import 必兼容 ≥3 个旧版本 |
| Redactor pattern 集合扩 | 旧 placeholder 永远兼容(`PERSON_1` 编号继续可还原) |

**禁止 breaking change**:任何降低用户隐私默认值的改动 = breaking change,必须经 user 评审 + RELEASE.md "Breaking" 节明示。

---

## 11. 风险登记

### R1 — 第三方 LLM provider 数据残留(高)

**风险**:

| Provider | 默认数据留存政策(2026-05 已查) | 训练用户数据 |
|---|---|---|
| OpenAI(API,非 ChatGPT)| 30 天 abuse-monitoring,enterprise/zero-retention 选项 | 默认 **不训练**(API)|
| Anthropic Claude API | 30 天后删除,默认不训练 | 默认不训练 |
| Google Gemini API | 取决于 tier;Gemini Free **可训练** | Free tier 默认训练,Paid 不训练 |
| DeepSeek | 中国法域,30 天 + 法定保留可能更长 | 默认不训练(2026-04 policy) |
| Attune Pro Gateway(gateway.engi-stack.com)| 透传到上游,不在 gateway 留存 | gateway 不训练 |
| 本地 Ollama | 完全本地,零外发 | 不可能 |

**缓解**:

1. **Settings → LLM Provider 选择页 UI** 显示每个 provider 当前隐私政策摘要 + 官方政策链接
2. **PII redactor 强制全开** 出网前 — placeholder 让 provider 即使留存也看不到真 PII
3. **wizard 默认推荐顺序**:Attune Pro Gateway → BYOK 用户已有付费 → 本地 Ollama;**不推荐 Gemini Free**(训练风险)
4. **RELEASE.md + PrivacyView 标注**:「使用云端 LLM 即代表你接受对应 provider 的数据政策。attune 本身**不保留**你的 prompt;但 provider 可能保留以做 abuse monitoring。」
5. **年度 audit**:`docs/PRIVACY-AUDIT-CHECKLIST.md` 月度核对每家 provider 隐私政策变化,有破坏性变更 → RELEASE.md 紧急 notice

### R2 — Telemetry 隐式开启(高)

**风险**:常见反模式是「first run 时弹窗『帮助改进产品』,默认勾选」。这与 1Password 式承诺冲突。

**缓解**:

- **永远默认 false**(per §3.3 表)
- 首次启动**不弹窗问 telemetry**,只在 PrivacyView 提供 opt-in 开关
- 任何 telemetry 代码引入 PR 必须 spec 评审

### R3 — Vault DB 被恶意 backup 软件吃掉(中)

**风险**:用户 Time Machine / Windows Backup 自动 backup vault DB → backup 存储不加密 → 物理盗窃可读密文(但是密文,需 master_key)

**缓解**:

- README 明示:vault DB 是加密的,backup 安全但 master_password 仍是唯一钥匙
- 推荐用户用 attune 自身的 `attune vault-export` (DSAR Art.20 端点)做 backup,而非依赖 OS 级 backup
- v1.1 加 `.exclude-from-backup` 标记(macOS / Windows Backup 都支持的属性)

### R4 — WebDAV 凭证 → 用户自托管 → 中间人(中)

**风险**:用户配 HTTP(非 HTTPS)WebDAV,attune 上传 vault 备份 → wire-tap

**缓解**:

- `sync/webdav.rs` 中:HTTP scheme 必须二次确认,UI 红色警告
- WebDAV 备份**仅上传 ciphertext**(vault DB 已加密),即使 wire-tap 也是密文
- 凭证存 vault 内 `cloud_session` table,vault 上锁即不可读

### R5 — Chrome extension 越权 / 浏览器漏洞(中)

**风险**:`<all_urls>` 权限广,extension 被恶意 patch 后能读所有页面;Manifest V3 service worker 漏洞

**缓解**:

- `manifest.json` 已设 `"incognito": "not_allowed"` — 隐身窗口硬阻断
- `browse_capture.js` HARD_BLACKLIST 双层(host + path)— 银行/政府/密码管理器 0 捕获
- 默认 opt-out — 只有 `chrome.storage.local["browseWhitelist"]` 含当前 hostname 才捕获
- extension code 走 Web Store 审核 + signed,本地 dev 模式不分发
- 详见 Spec 2 (web plugin as knowledge source)

### R6 — DSAR delete 不完整(中)

**风险**:用户 DSAR Art.17 删除,但 cloud 端 backup snapshot / 索引 cache / log 残留

**缓解**:

- DSAR delete endpoint **同时**调本地 + cloud 双删
- cloud 端 `accounts` 实施了 30 天 hard-delete(per v1.0.4 / #166)
- audit log 保留 deletion event 凭证(GDPR 反义务 — 证明你删了)
- 测试矩阵 §9 integration case 验证 export → delete → re-export 应为空

### R7 — Crash report 含敏感字段(中)

**风险**:panic backtrace 含 vault path / 用户名 / 文件名 → 默认 telemetry 关但 OS 级 crash dump 可能漏

**缓解**:

- 主二进制 `release` profile **strip 符号**(per CLAUDE.md Build profile 节)
- panic handler scrub:路径 normalize / 用户名 hash
- 默认**不**上传 crash dump,留本地 `~/.attune/crash/<ts>.log` 用户主动报障时 attach

### R8 — LLM gateway(Attune Pro)中间人风险(低-中)

**风险**:用户使用 Attune Pro Gateway → 我们作为中间人能看见 redacted prompt 与响应

**缓解**:

- gateway 不 log prompt / response body,仅 log meta(model / tokens / latency / user_id hash)→ 同 §5.3 audit log 规则
- gateway 代码开源在 `attune-cloud` 仓 — 用户可 self-host
- 用户始终可选 BYOK 直连或本地 Ollama 旁路 gateway
- TLS pin 上游 cert(rustls 默认)

---

## 实施 next steps(本 spec 之外)

1. **本 spec landed** → 立即 invoke `superpowers:writing-plans` 出 implementation plan(覆盖 PrivacyView.tsx + privacy.rs route + audit 脚本)
2. **v1.0.x roadmap 嵌入**:本 spec 大部分内容是**梳理 + 文档**,代码层仅小增量;放入 v1.0.x 现有 patch tag(不单独发版)
3. **PRIVACY.md user-facing 文档** → 律师 review 后 publish(v1.0.8 ICP 一起)
4. **scripts/privacy-audit.sh** → 进 CI hard gate

---

**Spec 完成**:11 节齐全。任何后续 attune 引入新出网 / 新 PII pattern / 新 DSAR 项 / 新隐私 UI 必先 amend 本 spec → user 评审 → implementation。
