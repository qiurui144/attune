# Attune 人工验收清单

> 自动化测试无法覆盖的人工 UX 验证项。每条都是勾选式可执行步骤。
> 自动化测试见 `docs/TESTING.md`。

## H1 资源治理框架（2026-04-27）

设计稿：`docs/superpowers/specs/2026-04-27-resource-governor-design.md`
用户面文档：`docs/system-impact.md`

### Linux 桌面验证

- [ ] **基线**：Settings → System Impact 默认显示 "Balanced" 档
- [ ] **三档切换**：切到 "Conservative" → 后台 embedding 速度肉眼变慢；切到 "Aggressive" → 速度回升
- [ ] **顶栏 Pause**：开始 100 文件批量 embedding → 点顶栏 Pause → 1 秒内 embedding 队列停止处理（pending count 不再下降）
- [ ] **Resume**：再点 Resume → 处理立刻恢复（pending count 继续下降）
- [ ] **CPU 阈值**：在 Balanced 档跑 100 文件批量 embedding → 同时 `top -p $(pgrep attune)` 观察 → 进程 CPU% 不会持续打满（≤ 50% 大致符合 25% 全局阈值在多核机的反映）
- [ ] **diag 命令**：`attune --diag`（H5 实现后）显示所有已注册 governor 的当前 profile / paused / 最近 sample

### Windows 验证

- [ ] 同上 6 条在 Windows MSI 安装的 Attune 上重跑
- [ ] 资源管理器 → 进程 → attune.exe 的 CPU 列与 diag 输出一致（差距 ≤ 5%）

### 跨场景验证

- [ ] **演示场景**：开 zoom 全屏共享屏幕 → 顶栏 Pause → 演示期间 attune 后台零打扰
- [ ] **全屏游戏场景**（H4 实现后）：启动全屏游戏 → governor 自动降到 Conservative → 游戏 FPS 不受 attune 影响
- [ ] **电池场景**（H4 实现后）：拔电源切电池 → governor 自动切 Conservative → 续航不显著缩短

## A1 Memory Consolidation（2026-04-27）

设计稿：`docs/superpowers/specs/2026-04-27-memory-consolidation-design.md`

### 基本流程验证

- [ ] **数据准备**：导入约 30 个文档跨过去 3 天（每天 ~10 chunks 进入 chunk_summaries 表）
- [ ] **首次 consolidate**：手动触发或等待 6h 周期 → 服务日志看到 `Memory consolidator: N new episodic memories`（应 N=3，每天 1 条）
- [ ] **数据可读**：用 `sqlite3 vault.sqlite "SELECT id, kind, window_start, source_chunk_count FROM memories"` 看 3 行 episodic 记录
- [ ] **解密验证**：通过 chat 或 list_recent_memories API 取出 summary 文本 → 应是中文 ~200 字、第三人称口吻、无前缀"总结："
- [ ] **幂等重跑**：重启 attune → 6h 后再跑 → 日志应显示 0 new memories（已 consolidated）

### 边界场景

- [ ] **当前窗口排除**：今天的 chunks（window 还未结束）不应被 consolidate（避免半天数据被早提交）
- [ ] **少量数据跳过**：单天少于 5 个 chunks 的窗口应被静默跳过（无 LLM 调用）
- [ ] **LLM 配额限速**：Conservative 档位下，跨 10 天积压 → 每周期最多生成 4 条（受 MAX_BUNDLES_PER_CYCLE）+ 配额按 bundle 消耗
- [ ] **vault lock 中途**：触发 consolidation 后立即 lock vault → 服务日志看到 `Vault locked during consolidation, discarding ... bundle result(s)`（不应崩溃 / 丢数据）
- [ ] **改密码后**：用旧密码触发 consolidation → 等 phase 2 LLM 调用期间用新密码 unlock → phase 3 应用新 dek 加密写入 → 后续 list_recent_memories 解密成功

### Worker 接入 H1 治理

- [ ] **Pause 顶栏**：consolidation 周期跑到一半时点顶栏 Pause → 当前 bundle 完成后停止，剩余 bundle 留下次（无超额 LLM 调用）
- [ ] **Conservative 档**：切到 Conservative → MemoryConsolidation governor LLM 配额降为 5/h → 多 bundle 周期会触发 deferred 日志

## 注意事项

- 任何一项失败 → 提 issue + 附 `attune --diag` 输出 + 本机 CPU/核数信息
- "演示场景"是核心，必须每次发版前手动验
- A1 的 LLM 速率限制依赖 H1 的 governor，验证 A1 前先确认 H1 已工作
