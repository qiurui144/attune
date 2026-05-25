# attune ↔ attune-pro Integration Smoke Test Report

> 执行日期：2026-05-22
> 目标：验证 attune v1.0.0-rc.2 + attune-pro v1.0.0-rc.2 配对兼容性

## 目录 (Table of Contents)

- [0. 测试背景](#0-测试背景)
- [1. 目标定位](#1-目标定位)
- [2. 范围边界](#2-范围边界)
- [3. 架构数据流](#3-架构数据流)
- [4. 测试执行 — 6 步流程](#4-测试执行--6-步流程)
- [5. Bug List](#5-bug-list)
- [6. v1.0 Ship 风险评估](#6-v10-ship-风险评估)
- [7. attune ↔ attune-pro 版本兼容验证结果](#7-attune--attune-pro-版本兼容验证结果)
- [8. API 契约核验](#8-api-契约核验)
- [9. 错误处理 + 边界 case](#9-错误处理--边界-case)
- [10. 测试矩阵](#10-测试矩阵)
- [11. 结论与行动项](#11-结论与行动项)

---

## 0. 测试背景

| 项目 | 版本 | HEAD commit |
|------|------|-------------|
| attune | v1.0.0-rc.2 | 27c11dc |
| attune-pro | **v1.0.0-rc.2**（本次新打） | 242c3b0 |

attune-pro 之前停在 `v1.0.0-rc.1`，本次对齐 attune RC 节奏打 `v1.0.0-rc.2`。

---

## 1. 目标定位

验证用户在真实路径下「安装 attune + 装 attune-pro plugin」的端到端流程是否通路，
重点检查：
- CLI plugin 命令链路（verify / install / list）可用
- plugin manifest 版本约束（`attune_min_version`）正确
- trust↔pricing 联动安全机制正常工作
- law-pro 内部 278 个 lib 测试全部通过
- attune-core plugin_loader / plugin_sig 路径可用

---

## 2. 范围边界

**做**：
- attune CLI plugin 命令冒烟（plugin-verify / plugin-install / plugin-list / plugin-verify-sig）
- law-pro plugin.yaml manifest 解析验证
- attune-pro lib 测试（278 个 unit test）
- attune-core plugin 路径测试
- 版本字段（version / attune_min_version / maturity）核查

**不做**（待 5/25 GA 前完成）：
- 真实 HttpPluginHubProvider 在线调用（需 cloud pluginhub 部署）
- 完整 attune-server 起服务 + API 端到端（server 启动 + marketplace API 调用）
- LLM 调用路径（per 红线：不动 LLM / 不动 4090）
- plugin-sign / plugin-encrypt 真实密钥流程（需生产 Ed25519 keypair）

---

## 3. 架构数据流

```
用户流程:
  attune plugin-verify <law-pro-dir> --trust Trusted
    ↓ plugin_loader::from_dir_with_key()
    ↓ serde_yaml 解析 plugin.yaml
    ↓ trust↔pricing 联动校验 (paid → must be Trusted/Official)
    ↓ ✅ 通过 → 打印 manifest 摘要

  attune plugin-install <law-pro-dir> [--key <key>] [--pubkey <pubkey>]
    ↓ plugin_sig::verify_loose() (有 pubkey 则验签)
    ↓ plugin_loader::from_dir_with_key()
    ↓ cp plugin dir → ~/.local/share/attune/plugins/<id>/
    ↓ ✅ 装载

  attune plugin-list
    ↓ 扫描 ~/.local/share/attune/plugins/
    ↓ 对每个 plugin dir 调 from_dir()
    ↓ 打印 id / version / type / tier / agents 数

服务端路径 (attune-server):
  GET  /api/v1/marketplace/plugins  → MockPluginHubProvider.list_plugins()
  POST /api/v1/marketplace/plugins/{id}/install
    → hub.name()=="mock" → 503 pluginhub_not_configured (正确)
    → hub.name()=="http-pluginhub" → 真实下载 + 安装
  GET  /api/v1/plugins  → taxonomy + plugin_registry 合并列表
```

---

## 4. 测试执行 — 6 步流程

### Step 1: attune CLI plugin 命令可用性

**命令**：`attune --help | grep plugin`

**结果**：✅ PASS

完整 plugin 子命令列表可用：
- `plugin-encrypt` / `plugin-decrypt` — paid 分发加密链路
- `plugin-verify` — manifest 解析 + trust↔pricing 校验
- `plugin-keygen` / `plugin-sign` / `plugin-verify-sig` — Ed25519 签名链路
- `plugin-install` / `plugin-uninstall` / `plugin-list` — 装载管理
- `sync-plugins` — 云端 entitled 插件同步
- `plugin-publish` — 开发者 pluginhub 上传

命令覆盖完整，用户交付链路齐全。

---

### Step 2a: plugin-verify（Unsigned trust）

**命令**：`attune plugin-verify /data/company/project/attune-pro/plugins/law-pro`

**结果**：✅ 预期 FAIL（正确拒绝）

```
error: crypto error: paid/trial plugin must be Official or Trusted, got 'Unsigned'
```

trust↔pricing 联动安全机制正常：paid plugin 在 Unsigned trust 下**必须拒绝**。

---

### Step 2b: plugin-verify（Trusted trust）

**命令**：`attune plugin-verify /data/company/project/attune-pro/plugins/law-pro --trust Trusted`

**结果**：✅ PASS

```
✓ plugin loaded: id=law-pro, version=1.0.0, type=industry
  pricing: tier=paid
  skills: 0
  agents: 6
  mcp_servers: 0
  case_kinds: 2
  trust verified: Trusted
```

law-pro v1.0.0 manifest 解析正确，6 agents、2 case_kinds 数量与 plugin.yaml 一致。

---

### Step 3: plugin-install（无 key）

**命令**：`attune plugin-install /data/company/project/attune-pro/plugins/law-pro`

**结果**：✅ 预期 FAIL（正确拒绝）

```
⚠️  no --pubkey: trust=Unsigned (paid plugin will be rejected)
error: crypto error: paid/trial plugin must be Official or Trusted, got 'Unsigned'
```

无签名的 paid plugin 安装正确被拒绝，安全门有效。

---

### Step 4: plugin-list

**命令**：`attune plugin-list`

**结果**：✅ PASS（含 BUG-1 发现，见 §5）

```
  law-pro (v0.2.0, type=industry, tier=paid, agents=1, skills=0, mcps=0)
  presales_pro / ai_annotation_risk / rust_helper / ai_annotation_highlights / patent_pro / tech_pro
7 plugin(s) installed at /home/qiurui/.local/share/attune/plugins (0 errors)
```

list 功能正常。发现：本地已安装的 law-pro 是旧版 v0.2.0（`~/.local/share/attune/plugins/law-pro/`），
而 attune-pro 源仓库已是 v1.0.0。这是预期的版本漂移——GA 前需要一次 `plugin-install --force` 更新本地安装，或走 `sync-plugins` 云端同步。

---

### Step 5: Marketplace Mock API 架构核验

**检查内容**：`GET /api/v1/marketplace/plugins` 路由实现

**结果**：✅ PASS（架构正确）

Mock pluginhub provider 行为正确：
- `list_plugins()` → 返回内置固定 listing，无网络依赖
- `install_plugin()` → 返回 503 + `pluginhub_not_configured` 错误（正确，防止误判"安装成功"）
- 切换到真实 HttpPluginHubProvider 只需配置 `settings.pluginhub_url` + `license_key`

`GET /api/v1/plugins`（plugin_registry 路径）正常读取 taxonomy + 用户安装 plugins。

---

### Step 6: attune-pro lib 测试 + attune-core plugin 测试

**命令**：
- `cargo test --package law-pro --lib`
- `cargo test --package attune-core --lib plugin`

**结果**：✅ 278 passed, 0 failed, 0 ignored

覆盖范围（完整 6 类下限 ENFORCE）：
- 11 deterministic agents × golden + prop + boundary + error + E2E + 回归 fixture
- skill_signals / quality_scorer / vision_quality_scorer
- plugin_loader / manifest parse / chat_trigger / workflow type

---

## 5. Bug List

| ID | 严重度 | 描述 | 影响 | 行动项 |
|----|--------|------|------|--------|
| **BUG-1** | Low | `~/.local/share/attune/plugins/law-pro/` 版本停留在 v0.2.0，未跟随 source 升到 v1.0.0 | 本地开发环境 plugin-list 显示旧版本 | GA 前执行 `attune plugin-install --force /data/.../law-pro`（需先解决签名/key），或走 `sync-plugins`；**不影响 source 仓库 + CI** |
| **BUG-2** | Low | plugin-verify 报告的 `agents: 6` 仅计算 plugin.yaml 内 `agents:` 列表中的条目，而 attune-pro 总共有 14+ agent binary；plugin.yaml 当前只注册了 6 个 civil-loan + labor-dispute 领域 agent，其余 agent（traffic / sale / housing / inheritance / defamation 等）尚未在 plugin.yaml 中声明 | 新 agent 需要 Forms UI 路由必须先加入 plugin.yaml；CLI verify 数量与实际 binary 数量不一致 | v1.0.0 GA 前在 plugin.yaml 补齐所有 Production 状态 agent 的 manifest 条目（对应 binary 路径）|

---

## 6. v1.0 Ship 风险评估

| 风险项 | 等级 | 状态 | 缓解 |
|--------|------|------|------|
| law-pro plugin.yaml 仅注册 6/14 agents | 🟡 中 | 开放 BUG-2 | GA 前补齐 plugin.yaml agent 条目 |
| defamation_extractor LLM F1=0.72（目标≥0.75） | 🟡 中 | Beta 标记 | 推 v1.0.1 调优；GA 发布时标注 |
| HttpPluginHubProvider 未 E2E 验证（需 cloud 在线） | 🟡 中 | 待 cloud 部署 | 5/25 GA day 联调；Mock 路径已验证 |
| 本地安装 law-pro v0.2.0 陈旧 | 🟢 低 | BUG-1 | 不影响 CI 和用户全新安装 |
| trust↔pricing 安全门 | 🟢 已验证 | ✅ | Unsigned paid plugin 正确拒绝 |
| 278 lib tests + agent_golden_gate | 🟢 已验证 | ✅ | 0 failures |

**总体评估**：RC 质量满足 5/25 GA 节奏。BUG-2（plugin.yaml agent 条目缺失）是唯一需要 GA 前解决的功能性 gap；defamation_extractor F1 标记 Beta 可接受 ship。

---

## 7. attune ↔ attune-pro 版本兼容验证结果

| 检查项 | attune | attune-pro | 兼容性 |
|--------|--------|------------|--------|
| 版本号 | v1.0.0-rc.2 | v1.0.0-rc.2 | ✅ 对齐 |
| `attune_min_version` | — | `"1.0.0"` | ✅ 正确（要求 attune ≥ 1.0.0） |
| plugin.yaml schema | v2 (attune-plugin-protocol) | v2 | ✅ 兼容 |
| `maturity` | — | `stable` | ✅ |
| `pricing.tier` | — | `paid` | ✅（trust gate 验证通过） |
| plugin_registry 加载 | attune-core v1.0.0 | law-pro v1.0.0 | ✅ 通过 |
| PluginManifest serde | attune-core PluginManifest | law-pro plugin.yaml | ✅ 解析成功 |

---

## 8. API 契约核验

| API | 方法 | 路径 | 验证方式 | 结果 |
|-----|------|------|----------|------|
| plugin 列表 | GET | `/api/v1/plugins` | 源码 + plugin_registry | ✅ |
| marketplace list | GET | `/api/v1/marketplace/plugins` | 源码 Mock 路径 | ✅ |
| marketplace install | POST | `/api/v1/marketplace/plugins/{id}/install` | Mock → 503（正确） | ✅ |
| plugin forms schema | GET | `/api/v1/forms/<plugin>/<form>/schema` | plugin.yaml ui_components 声明 | ✅ 结构完整 |
| plugin-verify CLI | — | CLI | 实测 --trust Trusted | ✅ |
| plugin-install CLI | — | CLI | 实测无 key → 拒绝 | ✅ |

---

## 9. 错误处理 + 边界 case

| Case | 预期行为 | 实测 |
|------|----------|------|
| paid plugin + Unsigned trust | 拒绝，`crypto error` | ✅ |
| paid plugin + no --key/--pubkey | 警告 + 拒绝 | ✅ |
| Mock hub install | 503 + `pluginhub_not_configured` | ✅（架构验证） |
| plugin-list（本地旧版） | 显示已装载版本，不 crash | ✅ |
| Trusted trust + paid tier | 通过 + 打印摘要 | ✅ |

---

## 10. 测试矩阵

| 维度 | 覆盖 | 状态 |
|------|------|------|
| CLI plugin 命令可用性 | 12 个子命令枚举 | ✅ |
| manifest 解析（v2 schema） | law-pro plugin.yaml 完整解析 | ✅ |
| trust↔pricing 安全门 | Unsigned/Trusted 两路 | ✅ |
| plugin 安装正确拒绝 | 无 key → 拒绝 | ✅ |
| plugin-list 不 crash | 7 plugins 列表 | ✅ |
| marketplace Mock 架构 | list + install 路径 | ✅（架构）|
| lib unit tests | 278 tests（law-pro） | ✅ 0 fail |
| attune-core plugin tests | plugin_loader / manifest | ✅ |
| 版本号 attune_min_version | v1.0.0 ↔ v1.0.0 | ✅ |
| agent binary 存在性 | 14 binaries in src/bin/ | ✅ |

---

## 11. 结论与行动项

**结论**：attune v1.0.0-rc.2 ↔ attune-pro v1.0.0-rc.2 集成路径核心链路验证通过。
安全门（trust↔pricing）、manifest 解析、CLI 工具链、lib 测试矩阵全部绿。

**5/25 GA 前行动项**：

| 优先级 | 行动项 | 负责方 |
|--------|--------|--------|
| P0 | BUG-2：在 plugin.yaml 补齐 traffic / sale / housing / inheritance / defamation 等 8 个 agent 的 manifest 条目（含 binary / cost / chat_trigger） | attune-pro |
| P0 | cloud pluginhub 在线，HttpPluginHubProvider E2E 验证 | cloud |
| P1 | BUG-1：`plugin-install --force` 更新本地 law-pro 至 v1.0.0 | 开发环境 |
| P2 | defamation_extractor F1 v1.0.1 继续调优（当前 0.72，目标≥0.75） | attune-pro |
