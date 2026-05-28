# Performance Baseline — v1.0.5

> 本文档是 attune v1.0.5 性能 SSOT。所有 SLO 数值化承诺、capacity planning、压测 framework
> 入口均在此。**本 sprint(v1.0.5)仅准备 framework + SLO 数值,不真跑** — 真跑等 user
> 启 cloud 真服务器 + 准备 200GB+ disk 主机后 dispatch。

## 目录

- [1. 范围与约束](#1-范围与约束)
- [2. SLO 数值化承诺](#2-slo-数值化承诺)
- [3. Capacity planning](#3-capacity-planning)
- [4. 压测 framework 入口](#4-压测-framework-入口)
- [5. 真跑前置条件](#5-真跑前置条件)
- [6. Baseline 数据记录格式](#6-baseline-数据记录格式)
- [7. 退化告警阈值](#7-退化告警阈值)
- [8. 与已有 stress baseline 关系](#8-与已有-stress-baseline-关系)

---

## 1. 范围与约束

**覆盖**:
- cloud + attune-server 联合(end-to-end user path)
- attune-server 单独(本地 vault + FTS + vector + reranker)
- 桌面客户端打包后 install pkg 真跑(per § Release 验证)

**不覆盖**:
- 单 crate micro benchmark(走 criterion,见 `crates/*/benches/`)
- attune-pro plugin 内部 agent latency(由 attune-pro 自己 baseline)

**硬约束**(per CLAUDE.md § Baseline 不轻易下结论 SOP):
- 任何"P99 = N ms"必须引 `reports/stress/<ts>/<file>`
- 任何"capacity = N user"必须引真服务器 hardware spec + 实测日志
- 缺源数据 → 标 PENDING-VERIFY,**不允许**写"已实测"

## 2. SLO 数值化承诺

> 这些数字是**目标承诺**,真跑数据填回后用 ✅ / △ / ❌ 标注实测命中。

### 2.1 LLM Chat path(走 cloud llm-gateway)

| 指标 | SLO | 实测(v1.0.5 真跑后填) | 状态 |
|-----|-----|--------------------|-----|
| P50 chat latency | < 2000 ms | PENDING-VERIFY | △ |
| P95 chat latency | < 5000 ms | PENDING-VERIFY | △ |
| P99 chat latency | < 10000 ms | PENDING-VERIFY | △ |
| chat error rate | < 2% | PENDING-VERIFY | △ |
| time-to-first-token (TTFT) | < 800 ms | PENDING-VERIFY | △ |

**SLO 依据**:
- DeepSeek-v4-flash / GPT-4o-mini 通常 TTFT 300-600ms,P50 1.5-2s
- failover 触发(主源 503 → 备源 retry)额外 +1.5s,纳入 P95 budget
- P99 10s 容忍 = 双源都慢 + 网络抖动

### 2.2 Search path(attune-server 本地)

| 指标 | SLO | 实测 | 状态 |
|-----|-----|-----|-----|
| P50 search latency | < 100 ms | PENDING-VERIFY | △ |
| P95 search latency | < 500 ms | PENDING-VERIFY | △ |
| P99 search latency | < 1000 ms | PENDING-VERIFY | △ |
| search error rate | < 1% | PENDING-VERIFY | △ |

**SLO 依据**:
- tantivy FTS p99 < 200ms(10k items;per `tests/docs/v1.0-stress-baseline.md`)
- usearch HNSW 单向量 query < 50ms(100k vectors)
- 100 GB vault 估算 ~1M chunks,大概率仍在 1s 内,需 100GB-vault stress 实测确认

### 2.3 Ingest path

| 指标 | SLO | 实测 | 状态 |
|-----|-----|-----|-----|
| ingest throughput | > 50 items/sec | PENDING-VERIFY | △ |
| chunk throughput | > 200 chunks/sec | PENDING-VERIFY | △ |
| 100 GB cold ingest | < 12 h | PENDING-VERIFY | △ |
| embed queue lag | < 60 s | PENDING-VERIFY | △ |

### 2.4 Auth / Session(走 cloud accounts)

| 指标 | SLO | 实测 | 状态 |
|-----|-----|-----|-----|
| signup latency P99 | < 1500 ms | PENDING-VERIFY | △ |
| login latency P99 | < 800 ms | PENDING-VERIFY | △ |
| token refresh P99 | < 300 ms | PENDING-VERIFY | △ |

## 3. Capacity planning

### 3.1 单 server capacity(LLM gateway + accounts 同 box)

| 资源 spec | 推荐并发 user | 100 GB vault 能否托管 |
|---|---|---|
| 4 vCPU / 8 GB RAM / SSD 100 GB | 50 user | ❌ vault disk 不够 |
| 8 vCPU / 16 GB RAM / SSD 500 GB | 200 user | ✅ 1 个用户 |
| 16 vCPU / 32 GB RAM / NVMe 2 TB | 1000 user | ✅ ≤ 20 用户共享(每人 ≤ 100 GB) |
| 32 vCPU / 64 GB RAM / NVMe 4 TB | 5000 user(估) | ✅ ≤ 40 用户共享 |

**caveat**:以上是估算,真跑数据填回前不能写"已验证"。

### 3.2 分层架构(v1.0.5+ 推荐)

```
┌──────────────────────────────────────────────────────┐
│ LB / WAF(Caddy / Cloudflare)                         │
└────────────────┬─────────────────────────────────────┘
                 │
        ┌────────┼──────────────┐
        │        │              │
    ┌───▼──┐ ┌──▼───┐      ┌────▼─────┐
    │ acct │ │ llm- │      │ attune-  │  N 个 server,
    │      │ │ gw   │      │ server   │  每人独立 vault
    └──────┘ └──────┘      └──────────┘
       │       │                │
       └───────┴────────────────┘
                  │
            ┌─────▼────┐
            │ Postgres │  cloud accounts DB
            └──────────┘
```

**横向扩展点**:
- llm-gateway 无状态,水平拷贝即可
- accounts 走 Postgres 主从
- attune-server 每用户独立进程(每 vault 一个 process),N user = N attune-server

## 4. 压测 framework 入口

### 4.1 1000 user / 4 path round-robin

文件:`tests/stress/k6-1000-user.js`

```bash
CLOUD_URL=https://gateway.engi-stack.com \
ATTUNE_SERVER_URL=https://attune-server.local:18900 \
AUTH_TOKEN=<bearer> \
k6 run tests/stress/k6-1000-user.js --out json=reports/stress/k6-1000-$(date +%s).json
```

4 路径:
1. **signup-light** — `/accounts/api/v1/auth/check`(heartbeat)
2. **login** — `/accounts/api/v1/auth/refresh`(token)
3. **chat** — `/llm-gateway/v1/chat/completions`(核心成本)
4. **search** — `/api/v1/search`(本地 vault)

stages:2m@100 → 5m@500 → 5m@1000 → 2m@0(共 14 min)。

### 4.2 100 GB vault cold-start + search latency

文件:`tests/stress/100gb-vault.sh`

```bash
ATTUNE_VAULT_DIR=/mnt/big/stress-vault \
ATTUNE_VAULT_PASSWORD=<test-pass-not-real> \
./tests/stress/100gb-vault.sh
```

5 phase:
1. preflight(disk space / server binary 检查)
2. 生成 100 GB synthetic md corpus
3. cold-start vault + 全量 ingest(走 `/api/v1/index/watch`)
4. 1000 query search latency(P50 / P95 / P99 → CSV)
5. memory ceiling 快照(`/proc/<pid>/status`)

报告写 `reports/stress/100gb-<timestamp>/`。

## 5. 真跑前置条件

| 条件 | 说明 |
|-----|------|
| cloud 真服务器 ready | accounts + llm-gateway + monitoring stack 都 healthcheck 绿 |
| attune-server release binary | `cargo build --release` 完毕,不用 dev build |
| 200 GB+ free disk | 100 GB corpus + 60 GB 索引 + buffer |
| 16+ GB RAM 物理机 | 笔电跑不动,需服务器或 NUC |
| k6 v0.50+ 安装 | `apt install k6` 或下载 release binary |
| AUTH_TOKEN env 准备 | `attune login` 拿 bearer,env 注入(不入 git) |

## 6. Baseline 数据记录格式

每次真跑后归档到 `reports/stress/<topic>-<ts>/`:

```
reports/stress/k6-1000-user-20260601-1430/
├── k6-summary.json         # k6 原始 output
├── k6-stdout.txt           # textSummary output
├── server.log              # attune-server log
├── memory.txt              # /proc/<pid>/status 快照
└── NOTES.md                # 人工 review notes(硬件 spec / 异常 / 结论)

reports/stress/100gb-20260601-1500/
├── 01-generate.log
├── 02-vault-init.json
├── 03-ingest-start.json
├── 04-ingest-progress.jsonl
├── 05-ingest-duration.txt
├── 06-search-latency.csv
├── 07-search-percentiles.txt
└── 08-memory.txt
```

数据填回本文档 §2 实测列时,引用 `reports/stress/<topic>-<ts>/<file>:<line>`。

## 7. 退化告警阈值

| 路径 | 告警阈值(相对 baseline) | 触发动作 |
|-----|----------------------|---------|
| chat P99 | > 1.5× baseline | failover 主备 swap |
| search P99 | > 2× baseline | 重建 tantivy 索引 |
| ingest throughput | < 0.5× baseline | 排查 disk IOPS / embed queue 死锁 |
| memory RSS | > 2× baseline | 排查 leak / arc-swap cache 增长 |

阈值由 v1.0.5 真跑数据填回后 lock,GA 前不允许下调(per § ratchet rule)。

## 8. 与已有 stress baseline 关系

- `tests/docs/v1.0-stress-baseline.md` — 单 crate 单元 stress(crash recovery / OOM / 并发 race)。**保留不动**。
- 本文档(v1.0.5) — end-to-end + capacity + SLO 层。**新增**。
- 两文档互不替代:单元 stress 防 crash,本文档防 SLO 漂移。

---

**版本**:v1.0.5-rc(framework only,真跑数据待 v1.0.5 GA 前补全)
**作者**:C4 sprint(performance stress framework + VLM provider)
**Review**:RC 阶段 user 真跑后归档
