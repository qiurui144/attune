# Attune Rust 商用线 — 功能 E2E + 首启硬化测试计划

> SSOT for sprint `attune-functional-e2e-and-onboarding-fixes` (2026-06-05/06).
> Spec(§9 矩阵来源): `docs/superpowers/specs/2026-06-05-attune-functional-e2e-and-onboarding-fixes.md`
> 执行证据: `reports/runs/<ts>/...`（§6.3 每 PASS 必引真实路径或 test-runner 输出）。
> 真服务定义: 运行中的 `attune-server-headless` :18900 + 云端 `43.130.26.91`（gateway/hub）+ Playwright Chrome（§6.4, channel=chrome）+ `test-pro@engi-stack.com` / `test-pro-not-real-0605`。

## 0. 测试目标（what + why）

本 sprint 修了 3 个首启/功能阻断 bug，必须证明修复在真服务路径生效，且 7 个功能域的 6 类场景有结论：

- **B1**（auth/credential，HIGH）: wizard Step3 「Attune Pro Membership」无凭据输入框 → 会员登录路径 dead-end。修复后 wizard 可输账号 + 登录 + 拿网关 token。
- **B2**（MED）: mount 时无条件连 `/ws/scan-progress` → 无 token 阶段 401 刷屏。修复后 wizard 期 console 零 401 storm。
- **B3**（schema/migration，HIGH）: `idx_skill_sig_kind` 建在无条件 SCHEMA_SQL → 老 pre-v0.7 库 `Store::open()` 崩 `no such column: kind`。修复后老库升级成功 + 零数据丢失。

## 1. 通过判据（可量化，不主观）

| 判据 | 阈值 |
|------|------|
| B1 wizard 会员登录 | Step3 选 attune-pro → 存在 email+password input → 填 test-pro → login 由 disabled→enabled → 登录成功拿 token（network 证据） |
| B2 WS 401 storm | wizard 期 console `/ws/scan-progress` 401/failed 行 = **0**（修前 ≥12） |
| B3 老库升级 | 修前红（`no such column: kind`）/ 修后绿；老行存活 count 不变 + kind='search_miss' |
| 域 happy | 全 PASS |
| 6 类下限 | 每域 6 类各 ≥1 用例有结论；evidence file 存在（R18：`ls reports/runs/<ts>/*` ≥6 域报告） |
| LLM agent | F1 ≥ 0.85（§Agent 验证铁律）；真 LLM 调用 ≤ ~20（批准 $8-15 内） |
| 锁序 | chat 并发：fulltext→vectors→vault，无死锁 |

## 2. 测试矩阵（7 域 × 6 类）

> 完整矩阵见 spec §9。本节为可执行映射 + 层归属。

| 域 | happy | edge | error | adversarial | concurrent | resource | 主层 |
|----|-------|------|-------|-------------|------------|----------|------|
| 1 folder-bind | API 绑 3-md 目录→watcher→3 项可搜 | 空目录/0字节 | 不存在路径 错误码 | `../`/symlink 逃逸拒绝 | 2 目录并发去重 | 500 文件队列化 | API/CLI（headless web 仅提示桌面版） |
| 2 KB-import | upload MD/PDF/DOCX/代码→可搜 | 空/空白/>50MB/unicode名 | 损坏 PDF lossy 容错 | 路径穿越/注入安全入库 | 并发 5 upload | 重复 content_hash=duplicate | 真服务 E2E + API |
| 3 chat+RAG | 配 LLM→问入库内容→RAG 命中+引用 | 空/超长/无相关 | LLM 不可达降级 | prompt injection 防护 | 2 会话并发不串 | demo 禁用 + token 估算 | 真 LLM E2E |
| 4 OCR | `attune ocr` 清晰图→抽文字 | 空白/极小/旋转 | 非图/缺模型 错误 | 超大分辨率不 OOM | 并发 2 OCR | PDF 多页内存稳 | CLI-unit |
| 5 ASR | `attune transcribe` 中文→WER<20% | 静音/极短/混语 | 非音频/缺模型 错误 | >1h 分段不 OOM | 并发 2 转写 | 大文件内存+临时清理 | CLI-unit |
| 6 membership | B1 后输 test-pro→登录→token+quota→切云端模型 | license_code 空 | 错密码 login_fail toast | 抓包密码不入 URL/log | 重复点登录幂等 | quota 显示+耗尽降级 | 真服务 E2E |
| 7 law-pro agents | 登录→plugin_sync 下 law-pro@1.0.5→验签→12 agent→派发1确定性→正确 | 空输入 graceful | 篡改包验签失败/LLM 不可达降级 | 篡改 entitlement/错 pubkey 拒绝 | 并发派发2隔离 | 12 agent 磁盘+subprocess LLM env | 真服务 E2E |

## 3. 视角划分

| 视角 | 域应用 |
|------|--------|
| 白盒 | B3 migration（pragma_table_info / sqlite_master 断言）；锁序静态核 |
| 灰盒 | API 层（folder-bind / KB-import contract）；CLI（OCR/ASR exit code） |
| 黑盒 | wizard B1/B2（Playwright 真 Chrome 点 UI）；membership→law-pro 真服务链 |

## 4. 数据集来源

- KB 语料: 真实 .md（`docs/` 子集）+ 1 个真 PDF/DOCX（非 synthetic）。
- OCR/ASR: 真图/真音频 fixture（非生成）。
- 凭据: `test-pro@engi-stack.com` / `test-pro-not-real-0605`（已 entitle law-pro；fixture，非生产）。
- 绝不用随机测试数据（§6.1 反模式 + `docs/TESTING.md` golden 策略）。

## 5. 执行环境前置

```
TMPDIR=/data/tmp-sdlc
cargo build -p attune-server --bin attune-server-headless   # 嵌入 341KB dist（B1/B2）
# 停旧 :18900 实例 → 重起 headless 127.0.0.1:18900
# vault 已建: master password TestSetup0605!（当前 locked/sealed）
# 云端: gateway.engi-stack.com / hub.engi-stack.com（→ 43.130.26.91）
# 真 LLM 上限 ≤~20: chat happy ≤3（N=3 评分）+ agents happy ≤2；其余优先确定性断言
```

## 6. 成本契约（§8 spec）

- 真 LLM 调用 ≤ ~20 次，落在批准 $8-15。
- edge/error/adversarial 优先确定性断言（不烧 token）。
- 截图 → `docs/screenshots/<topic>/`（§6.4，不落仓库根）。

## 7. v 历史 trace

| 日期 | 变更 | 新发现 bug |
|------|------|-----------|
| 2026-06-05 | B1/B2/B3 TDD 实施 + 单测绿（attune-core 1559 pass / clippy clean） | B3 红→绿回归测试 3 个入库 |
| 2026-06-06 | T6 真服务 E2E：B1/B2/B3 真服务证明通过 | **B4（新）**：CloudClient/OCR-bootstrap 阻塞调用直接在 async handler → `Cannot drop a runtime` panic，membership login 100% 挂；3 处（member/dsar/headless bootstrap）spawn_blocking 修复 + 回归测试。证据 `reports/runs/2026-06-06_111857_t6-e2e/` |

## 8. T6 执行结论（2026-06-06）

完整 7 域矩阵报告：`reports/runs/2026-06-06_111857_t6-e2e/T6-matrix-report.md`。

- **B1/B2/B3 真服务通过**；**B4 新 blocker 发现并修复**（spawn_blocking ×3 + 回归测试）。
- **PASS**：域2 KB-import（upload→search）、域4 OCR error+bootstrap-fixed、域5 ASR pipeline、域6 membership（test-pro HTTP 200 + 错密码 401 + 密码不入 URL）。
- **BLOCKED-fixture**（非代码缺陷，需云端 provision）：域3 chat（test-pro 无 gateway token）、域7 law-pro（无 entitlement + pluginhub 未配）。
- 回归：attune-core 1559 / attune-server 109 全绿；clippy 干净；真 LLM 调用 0 次（< $8-15 cap）。
