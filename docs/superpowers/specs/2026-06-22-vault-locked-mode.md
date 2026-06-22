# Spec: Vault Locked-Mode 降级运转 + 可选 Auto-Unlock (K3 G3)

> Status: DRAFT (G3 impl in-progress) · 2026-06-22 · 关联 K3 gap spec `2026-06-22-k3-scheduler-integration.md` §对齐 G1-G8 / 任务 #141
> 范围:G3① locked-mode 语义(主体,可独立 ship)+ G3② 可选 auto-unlock(框架,密钥封装 PENDING-安全评审)

## 1. 目标定位

K3 一体机 24h 常驻。凌晨断电重启后,attune-server 启动时 vault 是 **LOCKED**(无内存密钥),
此时:

- **后台 agents 不启动**:所有 `start_*_worker` 仅在 unlock 路径里启动 → reboot 后到 owner 早上手动
  解锁前,scanner / embed / reindex / monitoring / sync 全停。
- **ingest 入口被拒**:`upload` / `ingest_*` 需要 `vault.dek_db()` → LOCKED 时返回 403,
  文件在这段窗口直接丢失(用户/自动化推进来的文档不入库、不排队)。

痛点:**夜间窗口的所有外部输入(文件夹落档 / 邮件 / RSS / API 推送)都被静默丢弃,直到人手解锁**。
G3 让 attune 在 LOCKED 态**优雅降级运转**:输入安全暂存(加密、不触碰 vault 明文),agents 安静暂停(不报错刷屏),
解锁后**自动补处理**暂存输入并恢复 agents。

与产品定位对齐:1Password 式"私密优先" + K3"无人值守一体机"形态刚需。LOCKED 态绝不牺牲隐私
(暂存区无明文),也绝不静默丢数据(暂存 + 解锁补跑)。

## 2. 范围边界

**做(G3①,本轮主体)**:
1. LOCKED 态 ingest 进**加密暂存区**(staging),不解密入库、不触碰 vault DEK。
2. LOCKED 态后台 agents **暂停**:reboot 后启动一个**轻量 supervisor**,周期检查 vault state;
   LOCKED 时 worker 不跑业务、不刷错误日志;解锁后 worker 正常启动。
3. **解锁后自动补处理**:unlock 时 drain staging → 解密 → 走正常 ingest pipeline 入库 → 删除暂存项。
   幂等:中途 crash / 重启不丢不重。

**做(G3②,本轮框架,不做密钥封装实体)**:
4. settings 加 `auto_unlock_enabled` 开关(**默认 false**)。
5. 开启时**显式返回威胁模型变化提示**(密钥本机封装 = 物理接触者可读库)。
6. auto-unlock 的接口框架(状态字段 + 启动钩子点),**真封装机制标 PENDING-安全评审**,
   本轮**不落任何密钥到磁盘**(避免半成品的不安全密钥存储)。

**不做(写死,后续切片)**:
- ❌ auto-unlock 的真实密钥封装(TPM / secure enclave / OS keyring / passphrase-in-env) → 安全评审切片。
- ❌ staging 区的去重 / content_hash 短路(解锁补跑时由现有 pipeline 的 content_hash 短路覆盖)。
- ❌ staging quota / 老化清理策略(本轮设硬上限 + 拒绝,精细策略留 v.next)。
- ❌ LOCKED 态对 search / chat 的降级(无 DEK 无法读库,保持现状 401)。

## 3. 架构数据流

```
┌──────────────── LOCKED 态(无 vault DEK)────────────────┐
│                                                          │
│  inbound (upload / api ingest)                           │
│      │                                                   │
│      ▼                                                   │
│  vault.state()==Locked ?                                 │
│      │ yes                                               │
│      ▼                                                   │
│  IngestStaging::stage(raw_bytes, meta)                   │
│      │  encrypt with staging_key = HMAC(device_secret,   │
│      │     "attune-staging-key-v1")  ← 不需 vault 密码    │
│      ▼                                                   │
│  <data_dir>/staging/<uuid>.stg  (AES-256-GCM 密文)        │
│   + <data_dir>/staging/<uuid>.meta.json (uri/mime/源)    │
│                                                          │
│  WorkerSupervisor (周期 tick):                            │
│    vault LOCKED → 不启动业务 worker,安静 idle            │
└──────────────────────────────────────────────────────────┘
                       │ owner unlock (或 auto-unlock)
                       ▼
┌──────────────── UNLOCKED 态(有 vault DEK)──────────────┐
│  unlock handler:                                         │
│    1. 正常 init (search engines / llm / entitlement)     │
│    2. start_*_worker  (agents 恢复)                       │
│    3. start_staging_drain_worker:                        │
│         for each <uuid>.stg in staging/ (排序稳定):       │
│           decrypt(staging_key) → RawDocument             │
│           ingest_document(dek_db, …) 走正常 pipeline      │
│           成功 → 删除 .stg + .meta.json (幂等点)          │
│           失败 → 保留,下次 drain 重试(不重复入库:        │
│                  pipeline content_hash 短路兜底)          │
└──────────────────────────────────────────────────────────┘
```

**暂存键派生(no-plaintext 核心)**:
`staging_key = Key32::from_bytes(HMAC_SHA256(device_secret, b"attune-staging-key-v1"))`。
device_secret 在 `<config_dir>/device.key`(0600)已存在,**与 vault 锁状态无关**。
故 LOCKED 态可加密暂存而**不需 vault 密码 / 不触碰 vault DEK**。

**目录**:`<data_dir>/staging/`(每项两文件:`<uuid>.stg` 密文 + `<uuid>.meta.json` 明文元数据)。
meta.json 只含非敏感路由信息(uri / mime_hint / source_kind / created_at),**不含文档内容**。

**DB tables**:无新表(staging 走文件系统,因为 LOCKED 态 sqlite 业务表是加密字段,
存内容进 DB 也得加密 → 文件方案更简单且天然幂等:文件存在=待处理,删除=已处理)。

**cache layers**:无。staging 是一次性中转,解锁即 drain。

## 4. 模块边界

| crate / module | 改动 |
|---|---|
| `attune-core/src/staging.rs` (新) | `IngestStaging`:`staging_key_from_device_secret` / `stage` / `list_pending` / `load` / `remove` / `count` / 硬上限 |
| `attune-core/src/lib.rs` | `pub mod staging;` |
| `attune-core/src/vault.rs` | 无改动(staging 不依赖 vault 内部;通过 config_dir 读 device.key) |
| `attune-server/src/routes/upload.rs` | LOCKED 分支 → `stage()` 返回 `202 staged` 而非 403 |
| `attune-server/src/state.rs` | `start_staging_drain_worker` + auto_unlock 字段框架 |
| `attune-server/src/routes/vault.rs` | unlock 后 `start_staging_drain_worker`;auto_unlock 状态暴露 |
| `attune-server/src/routes/settings.rs` (or 新) | `auto_unlock` 开关 GET/PUT + 威胁提示 |

## 5. API 契约

**Upload (LOCKED 降级)**:
- `POST /api/v1/upload`,LOCKED 时:`202 Accepted` `{ "status":"staged", "staging_id":"<uuid>", "note":"vault locked, queued for ingest on unlock" }`
- staging 满(硬上限):`503` `{ "error":"staging full", "code":"staging-full", "retry_after_seconds":… }`

**Auto-unlock 设置**:
- `GET /api/v1/settings/auto-unlock` → `{ "enabled":false, "implemented":false, "threat_model":"<提示文案>" }`
- `PUT /api/v1/settings/auto-unlock` `{ "enabled":true }` → `200` `{ "enabled":true, "implemented":false, "warning":"<威胁提示>", "code":"auto-unlock-pending-security-review" }`
  - 真封装未实装,故 `enabled=true` 只记录意图 + 返回威胁提示;**不在磁盘落任何密钥**。

**Staging 状态(可选,便于验收)**:
- `GET /api/v1/vault/staging-status` → `{ "pending": N }`(无需 DEK,可在 LOCKED 态调)

## 6. 扩展点 / 插件接口

- `IngestStaging` 与来源无关:任何 inbound(upload / 未来 api-push / webdav 落地)LOCKED 时都可
  `stage(RawDocument-equivalent bytes + meta)`。drain worker 统一走 `ingest_document`。
- auto-unlock 框架:`AutoUnlockProvider` trait 占位(本轮只 `NoopAutoUnlock`),安全评审切片实装
  `TpmAutoUnlock` / `KeyringAutoUnlock` 等,不改 drain / staging。

## 7. 错误 + 边界 case

| case | 行为 | code |
|---|---|---|
| LOCKED upload | 加密暂存,202 | `staged` |
| staging 达硬上限 | 拒绝,503 | `staging-full` |
| device.key 缺失(SEALED 全新机) | 拒绝暂存(无派生密钥),503 | `staging-unavailable` |
| 暂存密文损坏 / 解密失败 | drain 跳过该项 + 记 warn,**不删**(留人工排查),继续后续 | — |
| meta.json 损坏 | 同上,跳过保留 | — |
| drain 中途 crash | 已删的不重跑;未删的下次重试;pipeline content_hash 短路防重复入库 | — |
| auto_unlock PUT enabled | 200 + 威胁提示,**不实际解锁、不存密钥** | `auto-unlock-pending-security-review` |

graceful degradation:LOCKED 态 worker 安静 idle(每个 worker 顶部 `if vault locked { sleep; continue }`,
**不 error! 日志刷屏**,只在状态切换时 info! 一次)。

## 8. 成本契约

- 暂存 = 🆓 零成本(AES-GCM 加密毫秒级 + 文件写)。LOCKED 态**绝不**触发本地算力 / LLM(无 DEK 也无法)。
- 解锁补处理 = ⚡ 本地算力(embedding)+ 建库阶段语义,与正常 upload 同档,**不升级到 LLM 分析层**
  (per 成本契约:建库阶段永不升第三层)。
- UI:LOCKED 态 upload 返回明确"已暂存,解锁后入库"提示;auto-unlock 开关旁常驻威胁文案。

## 9. 测试矩阵(§6.1 六类)

| 类型 | case |
|---|---|
| happy | LOCKED stage → unlock drain → item 入库可搜;agents 解锁后恢复 |
| edge | 空 staging drain (no-op);staging 单项 / 多项;unicode 文件名;0 字节内容 |
| error | staging 满拒绝;device.key 缺失拒绝;损坏密文跳过保留 |
| adversarial | **暂存区无明文**:stage 后读 `.stg` 字节不含原文 substring;path traversal in uri/filename 不逃逸 staging 目录 |
| concurrent | 多 inbound 并发 stage 不撞 uuid;drain 与新 stage 并发安全 |
| resource | staging 硬上限触发 backpressure;大文件暂存不 OOM(尺寸沿用 upload 100MB cap) |
| 幂等(回归) | drain 删除后重跑 no-op;drain 中途"模拟 crash"(drain 一半)再跑 → 不重复入库、剩余补齐 |
| auto-unlock | 默认 false;PUT enabled 返回威胁提示 + `implemented:false` + 磁盘无新密钥文件 |

## 10. 向后兼容

- 新增 staging 目录 + 新 routes,不改 vault schema / 现有 unlock 行为。
- 老 client:upload LOCKED 之前是 403,现在是 202 staged —— 行为更好(不丢数据),无破坏。
  老 client 收到 202 仍按 2xx 成功处理。
- 全部既有 vault / ingest 测试不回退。

## 11. 风险登记

| 风险 | 缓解 |
|---|---|
| staging_key 派生绑 device.key:物理接触者读 device.key 即可解暂存密文 | 与 vault DEK 包装同一信任边界(device.key 本就是 vault 派生输入);暂存是**短时中转**(解锁即清),敞口远小于长期库;文档明示 |
| drain 持锁阻塞 unlock | drain 在独立 worker 异步跑,不在 unlock handler 内同步 drain(unlock 立即返回) |
| lock ordering | drain worker 取 vault DEK 走短临界区(`dek_db()` clone 后即放),不嵌套 vectors/fulltext;遵守 `fulltext→vectors→vault` 序 |
| 暂存堆积撑爆盘 | 硬上限 count + 沿用 upload size cap;超限 503 backpressure |
| auto-unlock 半实现的不安全密钥存储 | 本轮**不落任何密钥**;PUT 仅记意图 + 返威胁提示;真封装留安全评审切片 |
| 跨平台 | staging 用 PathBuf + tempfile 语义;device.key 0600 已 cfg(unix) 保护;Windows 走 NTFS ACL |
