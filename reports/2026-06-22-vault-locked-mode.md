# Report: Vault Locked-Mode 降级运转 + Auto-Unlock 框架 (local scheduler G3)

**Date**: 2026-06-22 · **Task**: #141 · **Branch tip**: a513806 (3 commits on develop, NOT pushed)
**Spec**: `docs/superpowers/specs/2026-06-22-vault-locked-mode.md` (§3.1 11 节)

## 背景

local scheduler 一体机 24h 常驻,凌晨断电重启 → server 启动时 vault LOCKED(无内存密钥)。旧行为:
后台 agents 仅在 unlock 路径启动(reboot 后全停),inbound upload 因取不到 DEK 直接 403 丢失。
G3 补 locked-mode 降级:输入安全暂存(加密、无明文)+ agents 安静暂停 + 解锁自动补处理。

## G3① locked-mode 语义 — DONE

### 数据流(暂存 / 暂停 / 补处理)

- **暂存**:`attune-core/src/staging.rs::IngestStaging`。LOCKED 时 upload 进 `<data_dir>/staging/`,
  内容 AES-256-GCM 密文 `<uuid>.stg` + 明文路由元数据 `<uuid>.meta.json`(只含 uri/mime/源/时间,**不含内容**)。
  upload route LOCKED 分支返回 `202 {status:"staged", staging_id}`(取代旧 403)。
- **暂停**:既有后台 worker(scanner/embed/reindex/monitoring/sync 等)**仅在 unlock 路径启动**,
  fresh reboot LOCKED 态根本不启动 → 天然"暂停且不报错刷屏"(无需改 worker)。worker loop 顶部已有
  `if !Unlocked { break }` 守卫,运行中被 re-lock 也安静退出。
- **补处理**:`state.rs::start_staging_drain_worker`,unlock/setup/reset 三路径均挂。drain 按
  created_at 稳定排序 → 每项短临界区取 vault DEK → `ingest_document` 走正常 pipeline → 成功删暂存(commit point)。

### 暂存区无明文证据

- 派生键 `staging_key = HMAC_SHA256(device_secret, "attune-staging-key-v1")`(device.key 0600,
  与 vault 锁状态无关 → 无需 vault 密码即可加密)。
- 单测 `staged_blob_contains_no_plaintext`:stage 明文 marker 后,读 `.stg` 字节窗口扫描**不含** marker;
  `.meta.json` 同样不含内容。`wrong_device_secret_cannot_decrypt`:换 device.key → 解密失败。
- LOCKED 态本就无 vault key,暂存绝不触碰 vault 明文;暂存仅短时中转(解锁即清),
  信任边界 = device.key 包装(物理接触者风险与 vault DEK 包装同档),已在 spec §11 + 代码注释明示。

### 补处理幂等

- 文件存在 == pending;删除 == done。drain 成功才 remove → 中途 crash 剩余项下次 unlock 重试。
- 已 ingest-未删除的项:pipeline `content_hash` 短路兜底,重跑不重复入库。
- 单测 `mid_drain_crash_does_not_lose_or_double`:drain 前 2 项后"崩溃",re-list 恰好剩 2 项、顺序不变、不重处理。
- 损坏密文/meta:`load` 返 Err → drain **跳过且保留**(`corrupt_ciphertext_load_fails_and_retains`),不删(防静默丢)、不 crash loop。

## G3② auto-unlock 框架 — 框架 DONE,真封装 PENDING-安全评审

- `GET/PUT /api/v1/vault/auto-unlock`:开关**默认 false**;PUT 返回 `implemented:false` +
  `code:auto-unlock-pending-security-review` + 威胁模型文案(密钥本机封装 = 物理接触者可读库)。
- **诚实边界**:本轮**不落任何密钥到磁盘**——开关只持久化一个非密 1 字节 intent flag
  (`config_dir/auto_unlock.flag`),真 TPM/keyring/enclave 封装是独立安全评审切片。**没有半做的不安全密钥存储。**

## 六类测试(§6.1)

| 类型 | 覆盖 | 证据 |
|---|---|---|
| happy | stage→load roundtrip / E2E locked→drain→searchable | `stage_then_load_roundtrip` / E2E PASS |
| edge | 空 drain / 0 字节 / unicode uri / 排序 | `list_pending_empty` `zero_byte` `unicode_uri` `list_pending_sorted` |
| error | 无 device.key / 损坏密文 / staging 满 | `stage_without_device_key_fails` `corrupt_ciphertext...` `staging_full_rejects` |
| adversarial | 暂存区无明文 / 错 device secret 不可解 | `staged_blob_contains_no_plaintext` `wrong_device_secret_cannot_decrypt` |
| concurrent | 并发 stage 无 uuid 撞 | `concurrent_stage_unique_ids_no_collision` |
| 幂等(回归) | mid-drain crash 不丢不重 | `mid_drain_crash_does_not_lose_or_double` |

- 核心单测:`cargo test -p attune-core --lib staging::` → **13 passed**。全 core lib → **2505 passed, 0 failed**(无回退)。
- E2E:`vault_locked_mode_staging_test.rs`(`--ignored`)→ **1 passed**:
  setup→lock→locked upload(200 staged)→staging-status pending=1→unlock→drain→searchable→pending=0。
  日志实证:`Staging drain: 1 pending` → `Staging drain: drained 1 item(s)`。

## clippy

`cargo clippy -p attune-core -p attune-server --all-targets -- -D warnings` → **RC=0**(干净)。

## 锁序 / 跨平台

- drain 仅短临界区取 vault 锁(dek clone + ingest),**不嵌套 vectors/fulltext** → 不与 search/chat 热点 ABBA。
- staging 用 PathBuf;device.key 0600 已有 cfg(unix) 保护;serde_json + AES-GCM 纯 Rust 跨平台。

## 改动文件

- 新:`attune-core/src/staging.rs`、`attune-server/tests/vault_locked_mode_staging_test.rs`、spec
- 改:`attune-core/src/{lib.rs,error.rs}`(StagingFull 变体)、
  `attune-server/src/{lib.rs(2 route),middleware.rs(upload bypass),routes/upload.rs(locked 分支),routes/vault.rs(drain+auto-unlock+staging-status),state.rs(drain worker+flag)}`

## 残留 / 后续切片

- G3② 真密钥封装(TPM/keyring/enclave)= 安全评审切片(本轮只框架)。
- staging quota 精细老化策略、folder-watcher LOCKED 态暂存(当前 watcher 本就 unlock-gated)留 v.next。
- 未 push(遵 hold);未碰 GA。
