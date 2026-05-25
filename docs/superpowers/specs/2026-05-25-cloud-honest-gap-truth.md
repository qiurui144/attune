# Cloud 5 属性 honest gap truth — 5/26 真就绪度评估

> user 原话「规划一下现有 cloud 的缺口,安全、稳定、易用是否真的达到了」(2026-05-25 15:30)。
> 综合 #176 cloud 固化 audit + #180 cloud 20 轮 audit + 现 in-flight + 实施未完成 spec。
> **honest 评估,不 sugarcoat** — 哪些真实 ship-ready,哪些只是 spec / runbook,哪些是 5/26 → 6/26 真实风险。

## 0. 总判

**5/26 上架 Go ✅(0 P0 blocker)**,但 5 属性达成率不均:

| 属性 | 真达成 % | 评级 | 说明 |
|------|---------|------|------|
| **稳定** | 80% | 🟢 良好 | restart + healthcheck + gatus 基础 ✅;Prometheus / Loki 缺;真实负载只测 100 并发 |
| **可靠** | 60% | 🟡 中等 | backup ✅;**真 restore 演练 0** / off-site backup 0 / DR drill 0 |
| **安全** | 70% | 🟡 中等 | SOPS + ACME + 0 hardcoded ✅;**pen test 0** / fuzz 0 / secrets rotation 仅 runbook 无自动化 |
| **易用** | 75% | 🟢 良好 | cloud.sh 一键 ✅ + install-wizard 中 ✅;**web admin UI 0** / 错误 actionable 度未校验 |
| **好维护** | 85% | 🟢 良好 | 8 runbook 齐 ✅;docker image 累 160 GB 待 cleanup / cloud.sh shellcheck minor |

**综合达成 = 74%** — 上架可行,**6/12-7/15 必须补到 90%+**(v1.0.3 / v1.0.4 / v1.0.6 sprint)。

---

## 1. 稳定 — 80% 🟢

### ✅ 已真实达成

| 项 | 证据 |
|----|------|
| Service restart policy 10/10 | `docker-compose.yml` 全 `restart: unless-stopped`(#176 audit) |
| DB/cache healthcheck 5/5 | postgres/redis/mailpit etc. `healthcheck:` + `depends_on: service_healthy` |
| Service uptime monitoring | gatus 7 endpoint health check |
| Container 自愈 | crash → 自动 restart;7 service 历史 uptime 数据(若有 7+ 天 baseline) |
| 100 并发 p99=17ms | #180 R18 实测 `/health` 端点(low-load 基线) |

### ⚠️ 已 spec / runbook 但**未实施**

| 项 | 现状 | 真风险 | 推 |
|----|------|------|------|
| Prometheus / Grafana metrics | runbook 提到,真不存在 | 应用层指标(req/s / latency / error rate)**不可见** | v1.0.3 |
| Loki / log aggregation | runbook 提到,真不存在 | 各容器 docker logs 散落,故障定位需逐个 `docker logs` | v1.0.3 |
| 1000 user 并发压测 | #180 R18 仅测 100 并发 | 真负载下 p99 表现 unknown | v1.0.5 |
| Cold start 性能 baseline | 0 数据 | 部署后第 1 分钟用户体验 unknown | v1.0.5 |
| SLO 数值化承诺(P99 chat < 5s) | 文档无数字 | 用户期望管理无依据 | v1.0.5 |

### ❌ **5/26-6/26 真实风险**

| 风险 | 严重度 | 缓解 |
|------|--------|------|
| **某 service silent OOM**(无 Prometheus 看不到内存增长趋势) | P1 | gatus 仍能发现 down,但**触发前 24h 预警 0** |
| **DB connection pool 耗尽** | P1 | 同上,看不到 pool 增长 |
| **磁盘满 disk full** | P0 if 真发生 | `OPS.md` weekly check `docker stats` + `df -h`,user 真要每周看 |

---

## 2. 可靠 — 60% 🟡

### ✅ 已真实达成

| 项 | 证据 |
|----|------|
| backup.sh 自动 30d retention | `cloud.sh backup`(#176 改 7d→30d) |
| 4 DB 备份覆盖(accounts/pluginhub/llm-gateway/wp) | scripts/backup.sh enumerate |
| ROLLBACK.md 6 场景 | docker fail / DB migration fail / cert expire / secrets rotate fail / cloud upgrade fail / submodule pointer 异常 |
| cloud.sh auto-rollback 6 步 | upgrade 失败自动 restore previous compose state |

### ⚠️ 已 spec 但**未演练**

| 项 | 现状 | 真风险 |
|----|------|------|
| **真 restore 演练 0** | backup 跑过没,但**从 backup 真恢复 0 次** | 备份**可能损坏**而 user 不知;5/26 上架后某天真灾难,backup 反而无用 |
| Off-site backup 0 | 备份只在本机 | 服务器整盘丢失 → backup **同时丢** |
| DR drill 0 | 灾难恢复手册有,**没演练** | 第一次真需要时,user 步骤不熟,RTO 不可控 |

### 5/26-6/26 真实风险

| 风险 | 严重度 | 缓解 |
|------|--------|------|
| **某次 DB schema migration 失败 → restore 时发现 backup 损坏** | **P0**(若发生数据丢失) | v1.0.4 必跑 1 次 restore drill |
| **服务器整盘丢失** | P2(rare 但 catastrophic) | v1.0.4 加 off-site backup(S3-compatible) |
| **首次需要回滚时,user 看 ROLLBACK.md 步骤陌生** | P1 | v1.0.6 真做 quarterly drill |

### accounts pytest 6/104 FAIL(20 轮 audit P1)

- `_check_internal_token` test fixture 漂移,production 行为正确
- **non-blocking** 但 test reliability 噪音 → 后续真有 regression 时容易误判
- v1.0.2 修

---

## 3. 安全 — 70% 🟡

### ✅ 已真实达成

| 项 | 证据 |
|----|------|
| SOPS + age secrets 加密 | `secrets/cloud.enc.yaml` 全栈 secret encrypted |
| 0 hardcoded secrets | grep verify 在 cloud + accounts + pluginhub 全 0 |
| TLS auto(ACME / Let's Encrypt) | proxy + acme-companion |
| TLS strict mode(ENV=production) | Stripe 强签名 + HSTS |
| JWT verify chain | accounts ↔ pluginhub ↔ attune-server LLM gateway 鉴权链生效(#180 R7) |
| cargo-audit + deny CI | attune 仓内 ✅(#162 fix),cloud 子仓暂未配 |

### ⚠️ runbook **但无自动化**

| 项 | 现状 | 真风险 |
|----|------|------|
| Secrets rotation | `docs/SECRETS-ROTATION-RUNBOOK.md` 13 类有手册 | **无定期 cron 提醒** user 轮换;6 个月后 user 必忘 |
| trivy CVE scan | `trivy-scan.yml` stub(#176) | **email alert 未配置**;实际跑了 user 看不到 |
| Container vuln scan | trivy 仅 stub | 6 ghcr.io image 有 known CVE 时不知 |
| pen test | **从未做过** | 黑盒测试只是手动 curl,真攻击面 unknown |
| fuzz testing | **从未做过** | REST API surface 无 fuzz |
| DSAR API(GDPR / PIPL §38) | v1.0.1 spec(#163)有,**未实施** | 法律强约束 — user 索取 / 删除数据 endpoint 缺 |
| Secrets leak detection in commits | 仅 grep 手工 | pre-commit hook trufflehog 0 |
| Bug bounty | **未启** | 外部研究员无激励披露 |

### 5/26-6/26 真实风险

| 风险 | 严重度 | 缓解 |
|------|--------|------|
| **某 transitive dep 新 CVE(如 lru / rand 等历史 RUSTSEC)** | P1 | trivy nightly scan + email alert 真配(v1.0.4) |
| **prompt injection 突破**(虽 #140 30/30 测过) | P1 | 持续 monitor;v1.0.4 加 fuzz |
| **用户索取数据 → 法律风险** | P0(法律强约束) | v1.0.1 必交 DSAR endpoint(#163) |
| **管理员账号 credential stuffing** | P1 | accounts 已有 rate limit?未校验 — v1.0.4 audit |

---

## 4. 易用 — 75% 🟢

### ✅ 已真实达成

| 项 | 证据 |
|----|------|
| `cloud.sh` 一键部署 | check / upgrade / verify / backup 5 子命令 |
| `detect-domain.sh` 域名自适应 | ROOT_DOMAIN 1 env 起,4 source detect |
| `cloud.sh --check` 14/15 OK | #180 R5 实测 |
| README 4 步首装 + 时间表 | 88be5f7 强化 |
| install-wizard 模式(中) | #182 a08ebd534 跑中 — L1 必填 + L2 自动生成 + 凭证报告 |
| 9 入口 ≤3 click 自检 | a34d92b0 官网 hub |

### ⚠️ 真实**未达成**

| 项 | 现状 | 真风险 |
|----|------|------|
| **Web admin UI** | 全 CLI / Makefile / sops edit | user 非工程师无法日常运维;6 个月后失联 |
| **错误信息 actionable 度** | cloud.sh 错误返回 exit code,无清晰 next-step 指引 | user 失败时无导引 |
| **首装失败 rollback path** | `cloud.sh` 中途 fail 时 partial state 不可恢复? | 实测未验 |
| **手机 / 平板访问 cloud 管理** | 0 适配 | 出差时 user 无法干预 |
| **公开 status.engi-stack.com** | gatus 内部有,**未对外公开** | user 客户看不到服务状态 |

### 5/26-6/26 真实风险

| 风险 | 严重度 | 缓解 |
|------|--------|------|
| **user 一周不在,服务异常 user 不知** | P1 | gatus 已有 SMTP alert,但 user 真订阅了吗?5/26 必校验 |
| **WP admin / 管理员密码 user 忘了** | P2 | 凭证报告(#182)落档 ~/cloud-credentials-*.txt,user 真存了吗? |
| **某 secret 需 rotate user 不会** | P1 | v1.0.4 自动化 cron 提醒 + 引导 |

---

## 5. 好维护 — 85% 🟢

### ✅ 已真实达成

| 项 | 证据 |
|----|------|
| 8 runbook 文档齐 | OPS / TROUBLESHOOT(12 issue)/ MONITORING / SECRETS-ROTATION(13 类)/ BACKUP-RUNBOOK / ROLLBACK / PRODUCTION_DEPLOY / SUBDOMAIN-PLAN |
| README 一键部署节强化 | 88be5f7,prereq + 4 步 + 时间表 |
| 文档导航重组 | 部署 / 运维 / 故障 / 审计 4 分组 |
| cloud.sh 5 子命令 + Makefile | help 命令面板 |
| ARCHITECTURE.md SSOT | 服务清单 / ADR / 缺口治理路线 |
| 5/26 readiness doc | `docs/v1.0-526-deploy-readiness-final.md` |

### ⚠️ minor

| 项 | 现状 | 推 |
|----|------|------|
| Docker image 累 160.7 GB(92% reclaimable) | `/data` 85% 已用 | v1.0.4 加 prune cron |
| cloud.sh 8 shellcheck minor | SC2015 / SC2034 / SC2155 | v1.0.4 cleanup |
| 6 容器 restart=no 残留 | compose 声明 `unless-stopped` 一致,5/26 走 cloud.sh 即恢复 | 5/26 自然修 |
| LLM gateway 仅 2 channels | 满足最小冗余 | v1.0.4 扩 4-5 |

### 5/26-6/26 真实风险

| 风险 | 严重度 | 缓解 |
|------|--------|------|
| **磁盘满 → 服务全停** | P1 | OPS.md weekly check;v1.0.4 加 prune + 告警 |
| **某个 script 在新 OS 版本下 bash 语法 incompatible** | P3 | 测试矩阵 v1.0.5 |

---

## 6. 综合 — 5/26 → 6/26 真实风险等级

| Risk Tier | 数 | 例子 |
|-----------|------|------|
| **P0**(必须 v1.0.1-v1.0.2 修) | 2 | DSAR API 法律强约束 / DB restore 演练 0 |
| **P1**(必须 v1.0.3-v1.0.4 修) | 8 | Prometheus / Loki / pen test / trivy email / off-site backup / secrets rotation 自动 / 应用层 metrics / Web admin UI 缺失 |
| **P2** | 7 | 1000 user 并发未测 / fuzz / quarterly DR drill / image cleanup / shellcheck / accounts pytest fixture 漂移 / status page 公开 |
| **P3** | 5 | Bug bounty / 手机适配 / OS 兼容矩阵 / LLM gateway 扩 channel / WP admin GUI 加固 |

**5/26 上架可行,但运营至 v1.0.4(6/12)前**:
- 必看 `OPS.md` weekly checklist(避免磁盘满 / SMTP alert 未订阅 / DB pool 增长)
- 必看 `MONITORING.md` 告警范围(知道**还没**全覆盖)
- 任何 user 数据请求 → v1.0.1 完成后才能正确处理(在此之前手动)

## 7. 与 v1.0.x roadmap 对齐(per `2026-05-25-v1-0-ga-and-v1-0-x-gap-closure-roadmap.md`)

| Roadmap minor | 解决本 audit gap |
|---------------|----------------|
| v1.0.1(5/28) | DSAR API(P0)+ GH templates + 升级策略 |
| v1.0.2(5/31) | accounts pytest fixture + DB rename + SLA 分级 + i18n 债 |
| **v1.0.3**(6/05) | **Prometheus + Loki + alerts** — 解决 8 P1 中 4 个 |
| **v1.0.4**(6/12) | **pen test + secrets rotation 自动 + DSAR 完整 + off-site backup + trivy email + image prune** — 解决 8 P1 余下 + 2 P2 |
| v1.0.5(6/18) | 1000 user + 100 GB vault + SLA 数值 |
| **v1.0.6**(6/25) | **真 restore 演练 + DR drill + status.engi-stack.com 公开** — 解决可靠 60% → 90% |

## 8. 真实评估("user 三大属性是否达到"问题答案)

| user 关注 | 真实评估 | 5/26 后多久达到 ≥ 90% |
|-----------|---------|-------------------|
| **安全** | 70% 🟡 — SOPS + ACME 基础 ✅,但 pen test / fuzz / DSAR / secrets rotation 自动化全缺 | **6/12 v1.0.4 后** |
| **稳定** | 80% 🟢 — restart + healthcheck + gatus 基础 ✅,但应用层 metrics / 真负载未测 | **6/05 v1.0.3 后** |
| **易用** | 75% 🟢 — CLI 一键 + install-wizard ✅,但 Web admin UI / status 公开 0 | **6/25 v1.0.6 后** |

**短期可行**(5/26 上架成立),**长期需 v1.0.3-v1.0.6 持续 sprint 才达"真实生产级"**。

## 9. 用户决策点

1. **DSAR API 是否 v1.0.1 必交付?**(法律强约束,推荐 yes)
2. **pen test 外包预算**(~$5-10k,user 拍板时机)
3. **off-site backup 方案**(S3-compatible 选 AWS / B2 / R2,user 选)
4. **Web admin UI 是否要做?**(若 user 长期纯 CLI 运维,可推后;若上规模有客户需要,v1.0.6 必做)
5. **Bug bounty 启动时机**(v1.0.5 用户量起来后再开)

## 10. 接下来 7 天行动

5/26 上架:**Go**
5/27-28:v1.0.1 spec 已 ready(#163)→ implementation
5/29-31:v1.0.2 sprint(DB rename + SLA + i18n)
6/01-05:v1.0.3 Observability(Prometheus + Loki + alerts)— **应用层 metrics 落地**
6/05+:5 属性达成率从 74% → 85%

## 11. 不动 confirm

- ❌ 不修代码(本文是 audit 性质)
- ❌ 不动 git tag(cloud-v2.2.0 已 GA)
- ❌ 不动 4090 / Ollama / key
