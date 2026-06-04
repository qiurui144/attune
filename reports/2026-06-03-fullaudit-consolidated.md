# Full-Audit Consolidated — 12-Area SDLC Cross-Repo Roll-up

> Date: 2026-06-03 · Consolidation agent (SDLC lens) · Read-only synthesis
> Source: 11 area reports (attune ×5 / attune-pro ×2 / cloud ×4) + 1 cloud spec
> Scope: attune (OSS core/server/cli/accounts/python) · attune-pro (law-pro/tech-stubs) · cloud (CLI/accounts/admin/pluginhub/content-web)

---

## 0. Report inventory (all confirmed present)

| # | Area | Report path |
|---|------|-------------|
| 1 | cloud CLI reliability + consolidation | `/data/company/cloud/docs/superpowers/specs/2026-06-03-cloud-cli-reliability-consolidation.md` (spec, 11-节) |
| 2 | attune-core | `/data/company/project/attune/reports/2026-06-03-audit-attune-core.md` |
| 3 | attune-server | `/data/company/project/attune/reports/2026-06-03-audit-attune-server.md` |
| 4 | attune-cli | `/data/company/project/attune/reports/2026-06-03-audit-attune-cli.md` |
| 5 | attune-accounts + agent-sdk | `/data/company/project/attune/reports/2026-06-03-audit-attune-accounts-agentsdk.md` |
| 6 | attune-python-prototype | `/data/company/project/attune/reports/2026-06-03-audit-attune-python-prototype.md` |
| 7 | attune-pro/law-pro | `/data/company/project/attune-pro/reports/2026-06-03-audit-attune-pro-law-pro.md` |
| 8 | attune-pro/tech+stubs | `/data/company/project/attune-pro/reports/2026-06-03-audit-attune-pro-tech-stubs.md` |
| 9 | cloud-accounts | `/data/company/cloud/reports/2026-06-03-audit-cloud-accounts.md` |
| 10 | cloud-admin | `/data/company/cloud/reports/2026-06-03-audit-cloud-admin.md` |
| 11 | cloud-pluginhub | `/data/company/cloud/reports/2026-06-03-audit-cloud-pluginhub.md` |
| 12 | cloud-content-web | `/data/company/cloud/reports/2026-06-03-audit-cloud-content-web.md` |

---

## 1. Cross-repo scorecard

Dimensions 1–5 (5 = best). `overall` = unweighted mean rounded to 0.5.

| # | Area | LOC (1st-party) | code_quality | complexity | simplification_potential | doc_accuracy | overall |
|---|------|---:|:--:|:--:|:--:|:--:|:--:|
| 2 | attune-core | 63,790 (~½ tests) | 4 | 4 | 3 | 3 | **3.5** |
| 3 | attune-server | 15,468 | 3 | 2 | 2 | 3 | **2.5** |
| 4 | attune-cli | 1,838 | 4 | 3 | 3 | 3 | **3.5** |
| 5 | attune-accounts + agent-sdk | 1,334 + 342 | 4 | 5 | 4 | 3 | **4.0** |
| 6 | attune-python-prototype | ~6,400 | 4 | 4 | 2 | 2 | **3.0** |
| 7 | attune-pro/law-pro | ~17,400 | 4 | 3 | 2 | 3 | **3.0** |
| 8 | attune-pro/tech+stubs | ~1,450 | 4 | 5 | 3 | 2 | **3.5** |
| 9 | cloud-accounts | ~5,300 | 4 | 4 | 4 | 2 | **3.5** |
| 10 | cloud-admin | ~2,261 | 3 | 4 | 3 | 2 | **3.0** |
| 11 | cloud-pluginhub | ~3,500 | 3 | 4 | 3 | 4 | **3.5** |
| 12 | cloud-content-web | ~mixed | 4 | 3 | 3 | 2 | **3.0** |
| 1 | cloud CLI (spec) | 1,515 + 3,568 sh | — (spec, no score) | — | — | — | — |

**Cross-repo dimension means** (11 scored areas):
- code_quality **3.7** · complexity **3.7** · simplification_potential **3.0** · doc_accuracy **2.7**

**Reading**: code quality and complexity are healthy; the systemic weakness is **doc_accuracy (2.7)** — every area carries doc-drift, and several drifts are functional (cloud-admin UI calls deleted routes; attune-server CLAUDE lock-order is inverted vs code; attune-pro lib.rs谎报"无 agent 代码"). simplification_potential 3.0 means there is real, low-risk dead-code/dup removal across the board but no single area is bloated.

**Worst scored area**: attune-server (2.5) — god-function `chat()` 1212行 + ABBA deadlock + half-migrated error handling. **Best**: attune-accounts/agent-sdk (4.0) — agent-sdk is a model wasm leaf crate.

---

## 2. Severity census (HIGH/P0 findings across all areas)

| Sev | Area | Finding | Type |
|-----|------|---------|------|
| **P0** | cloud CLI | install-wizard `while read` 无 `[ -t 0 ]`/EOF 守卫 → 非交互 stdin 死循环空转 (违反 §8.1 0-manual) | reliability |
| **P0/HIGH** | cloud-admin | `JWT_SECRET` 公开硬编码 default 无 startup guard → env 空时任何人伪造 super-admin JWT 过 /verify | **security** |
| **P0/HIGH** | cloud-admin | users.html 调用已删 `/{id}/upgrade\|suspend` (int id),后端改 email-keyed → 全部按钮 404 (backend-first 绿掩盖坏 UI) | correctness/doc |
| **HIGH** | attune-server | ABBA 锁序倒置死锁:search/chat `fulltext→vectors→vault` vs items.rs `vault→...` 反序可互等 | correctness |
| **HIGH** | attune-server | CLAUDE.md 声明锁序与代码真实热点序冲突 | doc-drift |
| **HIGH** | cloud-pluginhub | `/activate` import `LicenseMachine` model 不存在 → 任何调用 ImportError 500 (ships untested+broken) | correctness |
| **HIGH** | cloud-pluginhub | 全 pytest suite un-collectable (`Config` NameError module-level annotation) → 当前"178 pass"是假绿 | test-infra |
| **HIGH** | cloud-content-web | official-web RELEASE/WORDPRESS-SETUP 写旧栈(Astra/CF7/RankMath)与现行 custom 主题+YAML自动化矛盾 → 误导运维 | doc-drift |
| **MED→升** | attune-accounts | reference `activate_license`/`get_llm_endpoint` 从不调 `SignedLicense::verify()` → 本地伪造任意 tier/quota 激活 (reference 被照抄即漏洞) | security |
| **MED** | cloud-pluginhub | legacy `admin_token` super-fallback = Web UI Basic-Auth 密码 → 一个泄漏 env = 全控 (Phase-B before GA) | security |
| **MED** | cloud-admin | RBAC `require_role` 定义但零引用 + 逻辑本身有 bug → read-only 角色可升级/暂停/审批 | security |
| **MED** | cloud-accounts | expiry SSOT 不一致:`licenses_validate.py` 用 `user.plan_expires`,`internal.py` 用签名 payload `expires_at` → 同 license 两端结论可不同 | correctness |

**P1 (cloud CLI reliability, from spec §7)**: 单服务 deploy/upgrade 无 vhost regen (与 C11 gateway 500 同根因) · 全栈 deploy regen 是隐式副作用(靠 acme restart) · teardown 非交互无 `--force` 静默 exit 0 (CI 误判成功)。

---

## 3. Prioritized action items — 3 tracks

### T-A — cloud 可靠性 + 命令面收拢 (task #19)

Spec 已落档(11节齐),处于 G1-pre。以下为 spec 内已登记 + 跨 cloud-area 安全/正确性合并后的执行优先级。

| # | Item | Sev | Area | SDLC next step |
|---|------|-----|------|----------------|
| A1 | **install-wizard 非交互守卫**:头部 `[ -t 0 ] \|\| error`(exit 4),`./cloud up </dev/null` 同链中招 | P0 | cloud CLI | **G2 plan → quick-fix PR**(spec 已设计 §I3) |
| A2 | **cloud-admin JWT_SECRET startup guard**:fail-fast if == public default or < N bytes(DB_URL 同);先于一切因为是 SSO 全栈准入 | P0 sec | cloud-admin | **quick-fix**(独立 hotfix,不等收拢 spec) |
| A3 | **cloud-admin users.html → email-keyed PATCH API** + 加 Playwright 真 UI check(backend-first 陷阱) | P0 corr | cloud-admin | **quick-fix + §2.2 UI 验证** |
| A4 | **cloud-pluginhub 修 `/activate`**:restore `LicenseMachine` model+migration 或删 endpoint(产品决策)+ 修 `Config` annotation 让 suite collectable | HIGH | pluginhub | **spec/decision gate**(删 vs 恢复)→ 然后 quick-fix collectable |
| A5 | **I5 vhost 一致性**:单服务 deploy/upgrade 后统一调 `cloud_proxy_regen`(P1-1/P1-2,C11 同根因)+ teardown 非交互 `--force` 退非 0(P1-3) | P1 | cloud CLI | **G2 plan**(并入收拢 impl PR) |

**SDLC 判定**: A1+A5 走 spec→G2 plan→impl(已设计,收拢映射 + alias 一并落地)。A2/A3 是独立 security/correctness **quick-fix hotfix**,不应被收拢 spec 阻塞。A4 需**产品决策 gate**(paywall activation 是否真需要)再 impl。
**伴随 MED**: cloud-pluginhub Phase-B 退役 legacy admin_token(before GA) · cloud-admin 接线 RBAC + 修 `require_role` 逻辑 + TOTP-lockout + ui.py cookie domain · cloud-accounts 统一 expiry SSOT(签名 payload 为准)。

### T-B — attune + attune-pro 简化压缩 (task #20)

| # | Item | Sev | Area | SDLC next step |
|---|------|-----|------|----------------|
| B1 | **attune-server ABBA 锁序死锁**:选定唯一序(建议沿用热点 `fulltext→vectors→vault`,改 items.rs)+ 同步修 CLAUDE.md 锁序节 | HIGH | server | **systematic-debugging → quick-fix**(真死锁风险,优先于压缩) |
| B2 | **attune-pro agent bin 泛型化**:8 个 deterministic agent bin 95% 逐行相同 → `run_agent_main::<A>()`,删 ~550-620 LOC | 简化(最大单点) | law-pro | **spec-lite + impl**(跨 8 bin 重构需测试守) |
| B3 | **attune-pro stub 降级**:patent-pro/presales-pro 占位 src 降纯 assets(presales 619行 prompt 保留为 asset)+ 删 tech-pro 未用 attune-core dep → **解除 wasm 迁移阻塞**(关联 task #2) | doc+简化 | tech-stubs | **quick-fix**(unblocks S2/#2) |
| B4 | **attune-server AppError 迁移收尾**:26 文件 + 77 inline error JSON + 4 份 err_500 重复 → 净删 ~250-400 LOC + 统一错误契约 | 简化 | server | **plan + 分批 impl**(渐进,非阻塞) |
| B5 | **attune-pro defamation 双抽取收敛**:`extractor_v3.rs`(405行)仅测试引用,生产走 v2 → 删或明确 v3 状态;+ `score_*_extractor_case` 复制 ~180 LOC 合并 | dead/简化 | law-pro | **decision + impl** |

**SDLC 判定**: B1 是 HIGH correctness → 优先 quick-fix(配 systematic-debugging 复现死锁场景)。B2/B4/B5 是纯压缩 → spec-lite 或 plan 后分批 impl,**走 Agent 验证铁律**(law-pro agent bin 重构后 agent_golden_gate.rs 必须 1.00)。B3 是高杠杆——既清债又解 wasm 迁移阻塞(#2),应优先。
**伴随 MED/LOW**: attune-core llm.rs retry-validate 中心化(−150~250) + migration helper(−80~120) + ocr scene_* 共享(−100~150);attune-core `cloud_client.logout()` FIXME(v1.1) 委托 `wipe_session`;attune-cli `min_core_version=0.4.0` 硬编码(crate 已 1.2.0)+ 28 子命令零文档 + run_ocr/run_transcribe 抽函数(−60~80)。

### T-C — 关联库 (task #21) — cloud 子服务 + python 原型 + agent-sdk

| # | Item | Sev | Area | SDLC next step |
|---|------|-----|------|----------------|
| C1 | **cloud-pluginhub suite un-collectable**:`Config` annotation 加引号 → 恢复真绿(当前 178 pass 是假绿,CI gate 失效) | HIGH | pluginhub | **quick-fix**(test-infra,先于任何 cloud release gate) |
| C2 | **cloud-content-web doc 旧栈纠正**:official-web RELEASE+WORDPRESS-SETUP 重写为 custom 主题+YAML自动化契约(误导运维做错配置) | HIGH doc | content-web | **quick-fix**(docs,§3.2) |
| C3 | **attune-accounts reference 签名校验**:activate/endpoint 路径加 `SignedLicense::verify()`(reference 被照抄即伪造漏洞)+ 删 ~75 LOC test-only fingerprint scaffolding(顺带移除 sha2 dep) | MED sec + 简化 | accounts | **quick-fix(sec) + 简化 impl** |
| C4 | **attune-python-prototype 战略收敛**:doc 宣称的两层层级索引/extract_sections/两阶段检索全不存在(grep 0);Rust 商用线已功能等价 → 删死代码(OpenVINO/process_immediate/skills+setup stub,~96 LOC) + 冻结为纯算法沙盒(README 去毕业模块) | doc+战略 | python | **decision gate(用户拍板)**→ 然后删死代码 quick-fix |
| C5 | **cloud-content-web update_content.py 拆分**:897 LOC god-orchestrator + 手写 markdown→WP-block 解析器(~180行正则顺序敏感)→ 拆 `markdown_blocks.py` + 删 2 DEPRECATED shim(~44 LOC) + 归档 7 份 2026-03 stale docs/plans | 复杂度+简化 | content-web | **plan + impl** |

**SDLC 判定**: C1 是 test-infra HIGH——**任何 cloud release gate 前必修**(否则 §7.2 Gate 2 形同虚设)。C2 docs quick-fix。C3 security quick-fix。C4 需用户战略拍板(双产品线去重是 §2.3 北极星级决策,不能 agent 自决)。C5 plan 后 impl。
**伴随**: cloud-accounts issue_license_for() helper 收敛 3 处签发重复(~70-90 LOC,兼修 payload/expiry 漂移) + 版本号三处不一致(pyproject/main/RELEASE);cloud-admin authenticate() helper(~50-60,一处修 cookie-domain+TOTP-lockout);agent-sdk **零动作**(模范 leaf crate,仅 NaN proptest 吹毛求疵)。

---

## 4. 压缩简化总览 (cross-repo dead-code / 重复 / 可删 LOC)

| 区 | 最大简化项 | 估 LOC | 风险/前置 |
|----|-----------|--------:|-----------|
| attune-pro/law-pro | agent bin 泛型化 `run_agent_main::<A>()` | -550~620 | agent_golden_gate 1.00 守 |
| attune-server | AppError 迁移收尾 + helper 去重 | -250~400 | 渐进,非阻塞 |
| attune-pro/law-pro | score_*_extractor_case 合并 + extractor_v3 删 | -180~360 | v3 状态决策 |
| attune-pro/tech-stubs | patent+presales stub 降纯 assets | -174 | 解锁 wasm 迁移(#2) |
| cloud-admin | authenticate() + proxy boilerplate 合并 | -100~130 | 一处修 cookie/TOTP 漂移 |
| attune-core | llm retry-validate + migration + scene_* | -330~520 | 行为不变重构 |
| cloud-pluginhub | 删 dead `/activate` + manual JWT | -115 | 产品决策(删 vs 恢复) |
| attune-python | 死代码 S1-S4(OpenVINO/process_immediate/stub) | -96 | 无风险 |
| cloud-accounts | issue_license_for() + best-effort-httpx 合并 | -140~165 | 兼修 expiry 漂移 |
| attune-accounts | test-only fingerprint scaffolding | -89 (+sha2 dep) | reference 决策 |
| cloud-content-web | DEPRECATED shim + 双语 helper + dead export | -80~90 (+150KB stale docs 归档) | 无风险 |
| attune-cli | run_ocr/run_transcribe/copy_vault helper | -60~80 | 无风险 |

**跨仓可删/压缩合计估**: **~2,200 – 2,900 LOC** 行为不变压缩(其中纯无风险删除 ~640-870 LOC:python死代码 + content-web shim + cli helper + accounts scaffolding + tech-stubs 降级)。**最大单点**: attune-pro agent bin 泛型化 (-550~620)。**最高杠杆(非 LOC)**: tech-stubs 降级解除 attune-pro wasm 迁移阻塞(关联 #2/S2)。

**战略级去重(需用户拍板,不计入上表)**: attune-python-prototype 与 Rust 商用线功能性等价重叠(SQLite/FTS/RRF/向量/embedding/平台检测 Rust 全已实装),冻结已毕业模块可省数百 LOC——这是 §2.3 北极星级决策。

---

## 5. SDLC top picks

**Top spec 候选**(走完整 §3.1 spec→plan→impl):
1. **cloud CLI 可靠性+收拢**(已落档,A1+A5)——多条件矩阵 + alias 兼容 + 收拢映射,跨 17 sh 重构。
2. **attune-pro agent bin 泛型化**(B2)——跨 8 bin,需 Agent 验证铁律守(spec-lite)。
3. **attune-python 战略收敛/冻结**(C4)——双产品线去重是北极星级,需 spec + 用户拍板。

**Top quick-fix 候选**(独立 hotfix,不进 spec):
1. **cloud-admin JWT_SECRET startup guard**(A2)——SSO 全栈准入,公开 default 可伪造 super-admin,最高安全紧迫度。
2. **cloud-pluginhub suite collectable**(C1)——当前"178 pass"假绿,CI gate 失效,任何 cloud release 前必修。
3. **attune-server ABBA 锁序死锁**(B1)——真死锁风险 + 修 CLAUDE 文档冲突。
4. **cloud-admin users.html → email API**(A3)——用户管理界面全部按钮 404。

---

## 6. 跨切面观察 (SDLC)

- **doc_accuracy 是系统性最弱维(2.7)**,且多为**功能性 doc-drift 而非美化**:cloud-admin UI↔API drift / attune-server CLAUDE 锁序倒置 / attune-pro lib.rs 谎报无 agent 代码 / cloud-content-web 旧栈 / 三处版本号不一致(cloud-admin/accounts/pluginhub/tech-pro 均中招)。**根因**:slice 重构(后端改路由/改栈)未同步前端+文档,§5.3 强制流程第 3 步(全量文档更新)未走完。
- **backend-first 绿掩盖坏 UI 复现**(§2.2):cloud-admin users.html 是教科书案例——后端测试直打新路由全绿,UI 按钮全 404。建议 cloud release 前补 Playwright 真 UI check。
- **假绿 CI**:cloud-pluginhub suite un-collectable → 178 pass 不可信。§7.2 Gate 2(cargo/pytest 全过)在 collectable 失效时形同虚设。
- **reference-as-vulnerability**:attune-accounts(签名不校验)+ cloud-pluginhub(legacy admin_token super-fallback)——reference/MVP 实现被照抄进生产即漏洞,GA 前必硬化。
- **agent-sdk 是跨仓质量标杆**(4.0,零冗余零 native dep,JSON wire byte-exact 锁契约),可作为其余 leaf crate 模板。
