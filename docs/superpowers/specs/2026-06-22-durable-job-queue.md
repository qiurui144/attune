# G5 通用持久任务队列（Durable Job Queue）— spec

> 状态：实现已落地（核心由 R2 #69 提交，本 spec 为补档 + 自动退避增量）。
> SSOT：本文件。代码：`attune-core::store::job_queue` + `office_job_queue` + `job_handler`；
> server 接线 `attune-server::job_worker`。
> 关联：K3 调度集成 spec `docs/superpowers/specs/2026-06-22-k3-scheduler-integration.md`。
>
> 背景修正：本 spec 取代代码注释里引用但从未入库的
> `2026-06-10-k3-g5-durable-job-queue.md`（该文件在 R2 worktree 清理时丢失，代码引用成了悬空路径）。
> 旧引用路径的注释将在后续清理批次统一指向本文件。

---

## 1. 目标定位

K3 一体机是 24h 常开的夜间批处理盒子；attune 桌面端也有锁定态 / 重启场景。
原 `JobRegistry` 是**纯内存**状态机：进程重启 = 所有在飞任务丢失（"服务器重启，请重新提交"），
锁定态期间排队的任务无处落脚，3am 跑批失败后无人值守自动重试。

本能力提供一个**通用、落盘、并发安全、自动重试**的任务队列，使：
- ASR / OCR / Agent / 批量导入 / monitoring 扫描 / 锁定态解锁后补处理 都注册为 job type；
- 任务**重启不丢**（落 SQLite），未完成任务**自动恢复**；
- 失败任务**自动退避重试**到上限后进 dead-letter（failed 终态）——无需运维 3am 值守。

与产品定位对齐：**降低 token + 数据安全 + 离线韧性**——队列全本地，无外部依赖。

## 2. 范围边界

**做**：
- 通用 job_queue 表（kind + payload + 状态 + 时间戳 + 优先级 + 重试计数 + deadline + 退避时刻）。
- 状态机 queued→running→done/failed/cancelled，可重排（priority/FIFO），timeout 回收。
- 并发多 worker 用单条 `UPDATE...WHERE state='queued'...RETURNING` CAS 取任务，不重复执行。
- 重启 boot 恢复：at_least_once kind 的 Running 重新入队；at_most_once 标 failed。
- TTL purge 终态行（按 finished_ms 计）。
- **自动退避重试**（本 spec 增量）：failed 且 attempts<N 且错误码可重试 → 指数退避后回 queued；
  attempts≥N 或不可重试错误码 → 留 failed（dead-letter 终态）。

**不做（写死，后续 minor）**：
- 跨进程/分布式队列（单机单 DB，N worker = N 连接同一 WAL）。
- 真正的子进程 mid-run SIGTERM kill（whisper 单次调用不可中断，仅边界协作取消）。
- 队列优先级抢占（运行中任务不被抢占，只在 queued 阶段重排）。
- per-kind 独立并发池（当前 worker 串行 drain，防 whisper 资源踩踏）。

## 3. 架构数据流

```
enqueue_job(kind,payload,priority,deadline)
        │  INSERT state='queued'
        ▼
   ┌──────────────┐   每 tick(500ms):
   │  job_queue   │◀── sweep_timeouts(now)      Running 超 deadline → failed(job-timeout)
   │  (SQLite)    │◀── auto_retry_failed(now,N) failed&attempts<N&可重试 → queued(+next_attempt_ms)
   │   WAL        │◀── purge_terminal(now,ttl)  done/failed/cancelled 超 TTL → DELETE
   └──────────────┘
        │  claim_next_job(now)  ← CAS: UPDATE..WHERE state='queued'
        │                              AND (next_attempt_ms IS NULL OR <= now)
        │                         ORDER BY priority DESC, created_ms ASC, id ASC
        │                         RETURNING ...   (单语句原子，N worker 不重复取)
        ▼
   run_one_job → increment_attempts → 超 max? park(max-attempts)
        │  handler.run(payload, ctl)   ← 不持 store 锁；ctl 协作取消 + 进度
        ▼
   complete_job / fail_job  (WHERE state='running' guard：cancel/timeout 赢竞态)

重启：Store::open 不动 Running（unlock 也走 open，不能误碰）；
      AppState 进程级唯一一次调 recover_on_boot()：
        Running × at_least_once → queued (清 started_ms)
        Running × at_most_once  → failed (interrupted-no-retry)
```

DB 表：`job_queue`（见 §5）。索引 `idx_job_queue_state_prio(state, priority DESC, created_ms)`
匹配 claim/list 的 ORDER BY；`idx_job_queue_kind(kind)` 给 list 过滤。

## 4. 模块边界

| 层 | 文件 | 职责 |
|----|------|------|
| 类型 | `attune-core/src/office_job_queue.rs` | JobKind / JobState / JobError / JobRecord / DeliveryContract |
| 存储 | `attune-core/src/store/job_queue.rs` | enqueue/claim/complete/fail/requeue/recover/sweep/purge/auto_retry + Store impl |
| schema | `attune-core/src/store/mod.rs` | CREATE TABLE job_queue + ALTER（next_attempt_ms 迁移）|
| worker | `attune-core/src/job_handler.rs` | JobHandler trait + JobControl + run_one_job（无锁跑 handler）|
| 接线 | `attune-server/src/job_worker.rs` | AsrJobHandler + build_registry + start_job_worker tick loop |
| 恢复 | `attune-server/src/state.rs` | install_job_store → recover_on_boot（进程级一次）|

后台 worker **走 store API**，不直调 vectors/fulltext（per CLAUDE.md lock ordering 约定）。

## 5. API 契约

**Store 方法**（attune-core）：
- `enqueue_job(kind, payload_json, priority, deadline_ms) -> id`
- `claim_next_job() -> Option<JobRecord>` / `claim_next_job_at(now) -> Option<JobRecord>`（退避感知）
- `get_job(id)` / `list_jobs(kind?, state?, limit)` / `job_queue_position(id)` / `in_flight_job_count()`
- `update_job_progress(id, stage_json?, progress)`
- `complete_job(id, result_json)->bool` / `fail_job(id, code, msg)->bool`（state='running' guard）
- `cancel_job(id)->bool` / `is_job_cancelled(id)->bool` / `requeue_job(id)->bool`
- `set_job_priority(id, priority)->bool`（仅 queued）/ `increment_job_attempts(id)->i64`
- `recover_on_boot()->RecoverSummary` / `sweep_timeouts(now)->usize` / `purge_terminal_jobs(now,ttl_days)->usize`
- **新增** `auto_retry_failed_jobs(now, max_attempts, base_backoff_ms)->usize`

**job_queue 表 schema**（epoch-ms i64 时间）：
```sql
CREATE TABLE job_queue (
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'queued',
  stage_json TEXT, progress REAL NOT NULL DEFAULT 0, priority INTEGER NOT NULL DEFAULT 0,
  payload_json TEXT NOT NULL, result_json TEXT, error_code TEXT, error_message TEXT,
  warnings_json TEXT, attempts INTEGER NOT NULL DEFAULT 0,
  created_ms INTEGER NOT NULL, started_ms INTEGER, finished_ms INTEGER,
  deadline_ms INTEGER, next_attempt_ms INTEGER   -- 新增：退避后最早可 claim 时刻
);
```
迁移：`ALTER TABLE job_queue ADD COLUMN next_attempt_ms INTEGER`（幂等，忽略 duplicate-column）。

**REST**（后续 minor，本 spec 仅契约占位）：`GET /jobs`（list 面板）、`POST /jobs/{id}/cancel`、`POST /jobs/{id}/retry`。

## 6. 扩展点 / 插件接口

新 job type = 加 `JobKind` 枚举项 + `as_str`/`from_str_kind`/`default_delivery` + 实现 `JobHandler`
+ `build_registry()` 注册。`monitoring` 扫描 / `锁定态补处理` 各注册为一个 kind（payload 带 cursor）。
多阶段 handler 在阶段间轮询 `ctl.is_cancelled()` 实现协作取消 + `ctl.report()` 推进度。

## 7. 错误 + 边界 case

| 场景 | 行为 |
|------|------|
| 无 handler 的 kind | fail_job(`no-handler`) |
| handler 返回 Err | fail_job(code, msg)；可重试码 → 自动退避；不可重试 → dead-letter |
| 超 max_attempts | park `max-attempts`（worker guard，运维反复 requeue 也挡）|
| Running 超 deadline | sweep → failed(`job-timeout`)|
| 源文件 enqueue 后被删 | handler 返回 `source-missing`（不可重试码）|
| cancel/complete 竞态 | complete/fail 带 state='running' guard，cancel/timeout 赢，晚到结果丢弃 |
| 重启中 Running | recover_on_boot：at_least_once→queued / at_most_once→failed(`interrupted-no-retry`)|
| 空队列 | claim 返回 None（不阻塞）|

**可重试错误码集**（自动退避）：`asr-engine-failed` / `ocr-engine-failed` / `job-timeout` /
瞬态 IO。**不可重试**（直接 dead-letter）：`bad-payload` / `source-missing` / `no-handler` /
`max-attempts` / `interrupted-no-retry` / `cancelled`。

退避公式：`next_attempt_ms = now + min(base * 2^(attempts-1), cap)`，cap=1h（K3 夜间批合理）。

## 8. 成本契约

零成本层：队列 CRUD = SQLite 毫秒级。任务执行成本归属各 handler（ASR=本地算力、Agent=LLM token）。
TTL purge 防 24h 盒无限增长（done/failed/cancelled 行按 finished_ms 计 30 天）。

## 9. 测试矩阵（六类下限）

| 类 | case |
|----|------|
| happy | enqueue→claim→complete；priority/FIFO 顺序；list 过滤 |
| 落盘持久 | file-backed DB 重开后 get_job 仍在；claim 已 commit |
| 重启恢复 | Running at_least_once→queued / at_most_once→failed；done/queued 不动 |
| 重排 | set_job_priority 仅 queued；queue_position 跟 claim 序 |
| timeout | sweep 过期 Running→failed→requeue 往返 |
| 失败重试退避 | auto_retry：failed&attempts<N→queued+next_attempt_ms；退避前不被 claim |
| 超限 dead-letter | attempts≥N 留 failed；不可重试码不退避；poison job park |
| 并发 | 8 worker × 200 job 无 double-claim、全 claim 一次 |
| 空队列 | claim None 不阻塞 |
| 资源耗尽 | 1000 job 入队 + 全 drain |

代码：`store/job_queue.rs` 单测 + `job_handler.rs` 单测 + `tests/job_queue_durable.rs` 集成。

## 10. 向后兼容

`next_attempt_ms` 经 `ALTER TABLE ADD COLUMN` 幂等迁移，老 DB（无此列）打开即补；
NULL = 立即可 claim（等价旧行为）。`claim_next_job()` 保留（内部走 now），
新增 `claim_next_job_at(now)` 供测试注入时间。reindex_queue 不变，作为独立兼容队列并行存在
（未来可作为一种 job kind 收编，本 minor 不动）。

## 11. 风险登记

| 风险 | 缓解 |
|------|------|
| 退避重试无限循环消耗算力 | max_attempts 硬上限 + dead-letter 终态 + 不可重试码集 |
| at_most_once 任务崩溃后误重跑（LLM 花费/外部副作用）| recover_on_boot 标 failed 不重跑；Agent kind 默认 at_most_once |
| 时间用 epoch-ms 与 reindex_queue rfc3339 不一致 | 队列独立，timeout 需整数比较，刻意分歧已注释 |
| 并发 CAS 在 SQLITE_BUSY 下竞争 | busy_timeout=5000 + worker 重试；race 集成测试 8×200 验证 |
| next_attempt_ms 索引缺失致退避 scan 慢 | claim 走 state 索引；24h 盒 job 量小，可接受 |
