# Attune 系统影响

Attune 的设计原则之一是做你电脑上的**好公民**。所有后台任务 — embedding 生成、文件扫描、LLM 自动分类、技能进化 — 都跑在每任务级的资源治理之下，系统忙时自动让出。

## 三档系统影响

按你使用机器的方式选档位。Settings → System Impact 随时切换。

| 档位 | 适用场景 | 行为 |
|------|---------|------|
| **Conservative**（保守） | 电池供电 / 老笔记本 / 视频会议 | 系统 CPU < 5–30%（按任务）才跑后台；LLM 进化限 5 次/小时 |
| **Balanced**（均衡，默认） | 插电笔记本 / 普通桌面 | 系统 CPU < 15–50%；LLM 进化 10 次/小时 |
| **Aggressive**（激进） | 闲置桌面 / 专用 NAS | 系统 CPU < 30–80%；LLM 进化 30 次/小时；最低退让 |

## 数字含义

`cpu_pct_max` 是**系统全局 CPU 阈值**，不是单任务占用上限。"EmbeddingQueue Balanced 25%" 意为：*当系统总 CPU 占用超过 25% 时 embedding worker 暂缓*。所有 worker 共享一个全局视图，多个 Attune worker 同时跑也不会意外打满你的机器。

## 顶栏 Pause 按钮

顶栏一键**暂停所有**后台 worker — embedding 队列、文件扫描、LLM 分类、技能进化、浏览器自动化网络搜索、浏览状态摄取。恢复也是一键。演示 / 游戏 / 跑 benchmark 前用它。

## 受治理的任务

| Worker | 默认 (Balanced) CPU 阈值 | 内存上限 |
|--------|-------------------------|---------|
| Embedding 队列 | 25% | 1 GB |
| 技能进化（含 LLM）| 20% | 512 MB（LLM 10 次/小时）|
| 文件扫描 | 20% | 512 MB |
| WebDAV 同步 | 15% | 256 MB |
| 浏览器自动化网络搜索 | 50% | 1.5 GB |
| AI 批注 | 20% | 512 MB |
| 浏览状态摄取（G1）| 10% | 128 MB |
| 自动 bookmark（G2）| 20% | 512 MB |
| Memory consolidation（A1）| 25% | 1 GB（LLM 10 次/小时）|

完整预设表：[`rust/crates/attune-core/src/resource_governor/profiles.rs`](../rust/crates/attune-core/src/resource_governor/profiles.rs)。

## 不受治理的部分

- **用户主动触发的操作**（chat 提问、搜索、手动上传）不走 governor — 你的活跃交互永远响应优先
- **请求级 HTTP handler**（Axum routes）不是后台 worker，是 tokio 短任务
- **GPU / NPU 使用**委托给 Ollama，由 Ollama 自身的资源控制

## 隐私：本地 Telemetry

Governor 在进程内记录 CPU / RAM 采样，仅用于诊断（`attune --diag` H5、未来 H6 图表）。这些数据**永远不出本机**，重启后不持久化。配合无遥测开关（D1，规划中），你可验证零出站网络调用。

## 验证生效

```bash
# 启动 100 文件批量 embedding，另一个终端：
attune --diag

# 输出（示例）：
# embedding_queue       profile=Balanced  paused=false  cpu=18.3%  rss=421MB  budget=25%/1024MB
# file_scanner          profile=Balanced  paused=false  cpu=2.1%   rss=89MB   budget=20%/512MB
# skill_evolution       profile=Balanced  paused=false  cpu=0.0%   rss=12MB   budget=20%/512MB (10 LLM/h)
```

如果你观察到 CPU 持续超 budget，请提 issue 附 diag 输出 — 这是 bug，不是预期行为。
