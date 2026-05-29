# Release Gap 评估 — attune 生态全量(2026-05-30)

> 方法:5 维度并行只读分析(Workflow fan-out)→ 综合。证据均实况(git/gh/测试数/文件:行)。
> 整体 release 就绪度 **88%**。**无 AI 能解的代码 P0** — 唯一 blocked 线(cloud)的 blocker 全是 ceremony/资质/用户动作。

## 4 线 verdict

| 产品线 | 就绪度 | verdict | 核心 |
|---|---|---|---|
| attune OSS v1.1.0 | 95% | 🟢 ship-ready | 已 GA tag(`e6b9b47` main + v1.1.0/desktop-v1.1.0),2105 tests/0fail,四门 PASS,chat-flow wiring 真证据验证 |
| attune-pro v1.1.0 | 92% | 🟡 minor-gaps | 配对 tag,law-pro 178/178@1.00;defamation F1=0.8683 标 Beta + VLM stub + 分支债 |
| cloud v3.0.0 | 90% | 🔴 blocked | 代码/测试/加固全 GA-ready(478 tests,整链 E2E 17p,四门 PASS,VERSIONS bumped);**blocked 仅因 ceremony/资质** |
| wiki/官网 | 80% | 🟡 minor-gaps | 四跳链通,Docusaurus 2 tab build 成熟 |

## P0 blockers(全 = 用户动作/外部资质,非代码)
1. **cloud GA tag 未打** — `git tag cloud-v3.0.0`(master `0335fbf`,四门已 PASS)。AI 不擅自打 GA tag(§7.1)。
2. **Stripe live mode 未切** — 需 `sk_live_*`(外部资质,AI 无 secret 权),GA 后小额灰度。
3. **cloud 真机部署验收** — GH artifact 生产环境走 register→checkout→email(§7.3 手工 checklist)。

## AI 收口项 — 本次已全部完成 ✅
- attune README v0.7/rc.2 旧文案 → v1.1.0 GA(`47d8aef`,治理对齐)
- wiki `/pluginhub/` 死链 → `pluginhub.engi-stack.com`(`bf9bb76`,SSOT 源头,sync 传 attune-docs)
- attune-pro RELEASE KL 补 defamation Beta(F1=0.8683)+ VLM stub + scaffold 声明(`06076d5`)
- 文档债清理(清债 agent):删 release-notes-drafts 目录 / 双 specs 消除 / ACP spec closeout(标已实施 GA)/ 旧 plan 删 / 根 RELEASE v0.7→v1.1.0
- 手动会员+授权码 feature(MVP):accounts `96dc037` feature 分支(246 tests),payment 暂不做时的过渡授权渠道

## 用户动作清单(release 真正卡点)
| # | 动作 | 类型 |
|---|---|---|
| 1 | 授权打 `cloud-v3.0.0` GA tag | release ceremony |
| 2 | Stripe live key 配置 + 小额灰度 | 外部资质 + secret |
| 3 | cloud 真机 install pkg 部署验收 | §7.3 手工 |
| 4 | attune-pro 分支决策(A:补 develop→main / B:认 develop-tag 政策) | 分支策略 |
| 5 | lawcontrol 9 处品牌残留意图澄清(历史对标 vs 兼容文档) | 语义确认 |
| 6 | 手动会员 feature merge(accounts feature→develop + cloud pin)+ review | 评审 |
| 7 | K3 端到端 / Step7 LLM live smoke | 等硬件/真 token |
| 8 | payment 多渠道实施决策(调研报告已出) | 产品方向 |

## 非阻塞 follow-up(已 surface 进 RELEASE KL)
- attune:Web UI 渲染 acp_flow(前端 minor)/ OSS real-LLM gate #[ignore](需 self-hosted runner)/ ACP-5 deterministic step binary dispatch(v1.2)/ self_evolving_skill legacy-overlap(v1.2)
- attune-pro:VLM 真接入(v1.1.x)/ defamation prompt iter 至 0.90+(v1.1.1)/ patent-pro & presales-pro 复活

## 结论
**三条产品线代码层全部 release-ready**。attune OSS 已实质 GA。attune-pro 配对就位。cloud 仅差用户 ceremony(tag + Stripe live + 真机验收)。wiki 链通。**剩余全是用户决策/资质/硬件门,无 AI 可解的代码缺陷。**
