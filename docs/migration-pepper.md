# Pepper 升级路径手册（browse_signals.domain_hash）

> 状态：v0.7 计划项 — 本文档固化升级契约，让 v0.6 用户提前知道会变什么、
> v0.7 实施者有可参照的实现规格。

## 当前状态（v0.6）

`browse_signals.domain_hash` 计算方式：
`HMAC-SHA256(DOMAIN_HASH_PEPPER, domain)`，其中
`DOMAIN_HASH_PEPPER = b"attune.browse_signals.v1.2026"` 是**编译期常量**，
所有 Attune 二进制内置同一份。

v0.6 接受的取舍：

- ✅ 同一 attune 二进制 → 同 domain 的 hash 相同 → "清空 github.com 的浏览信号"
  跨重装仍正确
- ✅ 任何 pepper 的 HMAC-SHA256 都比裸 SHA-256 强很多（防 google.com / mail.qq.com
  这类常见域名彩虹表反推）
- ❌ 两台机器的两个 vault 对同一 domain 算出同一 hash — 攻击者若同时拿到两份
  vault.sqlite 且知道 pepper，可跨机关联浏览
- ❌ pepper 不是用户私密的（在二进制里）

## 目标状态（v0.7）

`DOMAIN_HASH_PEPPER` 改为**vault salt 派生**：

```rust
let vault_pepper = hkdf_expand(vault_salt, b"browse_signals.domain_hash.v2", 32);
let domain_hash = HMAC-SHA256(vault_pepper, domain);
```

新增特性：
- 每个 vault 独立 pepper → 同时被偷的两份 vault 无法按 domain 关联
- 改密时 pepper 自动轮换

## 升级挑战

`browse_signals` 现有数据的 `domain_hash` 是用旧 pepper 算的。升级后用
`WHERE domain_hash = HMAC(new_pepper, domain)` 查不到任何东西 — 按域名删除
按钮 + 历史筛选会静默失效。

## 升级算法（v0.7 计划）

1. **Schema 版本追踪**
   - `vault_meta` 加一行：`key = 'pepper_version'`，老 vault 默认 `'v1'`
   - Store::open 时检测 `pepper_version` ≠ 当前代码版本 → 触发升级
   - 单向：`v1 → v2`（不支持降级）

2. **Re-hash 后台扫描**（H1 governor Conservative 档限速）
   - 100 行一批迭代 `browse_signals`
   - 每行：
     - 用 DEK 解密 `url_enc` → `host_of()` 取 domain
     - 算新 hash：`HMAC(new_pepper, domain)`
     - `UPDATE browse_signals SET domain_hash = ? WHERE rowid = ?`
   - 每 100 行一个事务（中断安全）
   - 解密失败的行（外来 vault 残留）：跳过 + log 警告
     — `list_recent_browse_signals` 已经 silent-skip 这种（per R15 P1）

3. **完成标记**
   - `UPDATE vault_meta SET value = 'v2' WHERE key = 'pepper_version'`
   - 后续 open 跳过 re-hash

4. **用户可见窗口**
   - 升级在 v0.7 首次启动 vault unlock 时跑
   - 期间按域名操作可能短暂返回混合/空结果（10K 行的典型 vault ~秒到分钟级）
   - UI 显示 "正在升级浏览信号..." toast
   - "全清浏览信号" 不受影响（与 domain_hash 无关）

## 回滚

若 v0.7 回退到 v0.6：
- v2 pepper 写的新行 v0.6 用 v1 pepper 查不到
- "按域名删除" 静默跳过
- "全清" 仍 work
- **建议**：跨 pepper 版本不要降级。v0.7 release notes 会标 Breaking。

## 测试

升级测试在 `rust/crates/attune-core/tests/migration_roundtrip_test.rs`，
沿用 W3 batch A `migrate_breadcrumbs_encrypt` 模式（per R07 P0）：

- `migration_drops_old_plaintext_breadcrumb_column` — 老列消失
- `migration_is_idempotent_on_second_open` — 重跑无副作用
- `encrypted_breadcrumb_survives_close_and_reopen` — 加密数据 round-trip

`migration_pepper_v1_to_v2_rehashes_all_rows` 测试在 v0.7 落地时加。

## 为什么推到 v0.7

W3 batch B 优先发 G1 捕获 + G5 隐私面板 + sidecar 加密（R04 P0-1，明文落盘的紧急
风险）。Pepper 版本化是 defense-in-depth 加固，升级路径已写清，适合 v0.7 与
计划中的密钥轮换（K5 Items Keys per Standard Notes 004 spec）一起做。

## 参考

- W3 batch B 设计稿：`docs/superpowers/specs/2026-04-27-w3-batch-b-design.zh.md`
- R04 P0-1 review（推动这次更广 audit 的 sidecar 加密缺口）：`tmp/w3-final-review-tracker.md`
- v0.7 K5 设计（Standard Notes 004 items keys）：计划中，见 strategy plan
- HKDF (RFC 5869) — pepper 派生原语
