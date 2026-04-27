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

## W2 Batch 1: RAG Quality Hardening（2026-04-27）

设计稿：`docs/superpowers/specs/2026-04-27-w2-rag-quality-batch1-design.md`

### J1 Chunk 路径前缀
- [ ] 导入一份多级标题 markdown 文档（4 级以上）→ chunk 索引化后用 `sqlite3` 查 `items.content` 含 `> A > B > C` 面包屑前缀
- [ ] chat 引用某 chunk 时，prompt 里能看到完整 `[文档名 > 章节路径]`

### J3 召回阈值
- [ ] Settings 默认状态下 chat → 召回结果数量与 W2 之前相比"略降但精度上升"（吴师兄曲线）
- [ ] Chrome 扩展 `/api/v1/search/relevant` 行为完全保持（陌生 query 仍能召回模糊匹配）— **回归核心**
- [ ] **失败场景**：故意问"完全不相关问题" → 应返回 0 结果（陈旧版本会硬返回 top-5 噪音）

### J5 强约束 Prompt + 置信度
- [ ] chat 询问明确问题 → 答案中**不出现** "可能" "大概" "建议咨询" "或许" "应该"
- [ ] chat 答案末尾**用户看不到**【置信度: N/5】marker（被 strip）
- [ ] 故意问知识库无答案的问题 → 触发二次检索（日志看 `confidence < 3, triggering secondary retrieval`）；答案最终为"知识库中暂无相关信息"
- [ ] LLM 输出多个 marker 时（罕见）→ parse 取最后一个、strip 只删最后一个之后

### B1 backend
- [ ] chat API 响应 JSON 含 `confidence` + `secondary_retrieval_used` + `citations[].breadcrumb` + `citations[].chunk_offset_start/end` 字段（即使 breadcrumb=[] / offset=null）
- [ ] **Known limitation 验证**：当前 `breadcrumb` 总为空 array、`offset` 总为 null（W3 batch 2 才透传）— 前端不应假设有值

## W3 Batch C: K2 Parse Golden Set Baseline（2026-04-27）

### K2 baseline 验收
- [ ] `cargo test -p attune-core --test parse_golden_set_regression` 8 测试全绿
- [ ] 任意改 `chunker::extract_sections_with_path` 后跑此测试 — fixture fail 应清晰报告 expectation 名称
- [ ] 添加新 fixture：`tests/fixtures/parse_corpus/006-xxx.md` + manifest.yaml 加 entry → 测试自动覆盖（无需改 harness）
- [ ] regression gate 验证：临时把 manifest 中 `min_pass_rate: 1.0` 改 `2.0`（不可能阈值）→ `k2_baseline_corpus_passes_min_rate` 应失败提示

## W3 Batch B: G1 + G2 + G5 + F3（2026-04-27）

### 前置准备（必读）
- 后端：`cd rust && cargo run --bin attune-server` → 默认监听 `http://localhost:18900`
- Chrome 扩展构建：`cd extension && npm install && npm run build`（产出 `dist/`）
- Chrome 扩展加载：访问 `chrome://extensions/` → 开启"开发者模式" → "加载已解压扩展" → 选 `extension/` 目录
- 验证安装：扩展图标出现在工具栏；点击 popup 看到 "知识注入" toggle + "浏览隐私 →" 按钮
- 后端 vault 必须 unlocked（首次访问 backend `/api/v1/status` 应返回 `unlocked: true`）

### G1 浏览信号捕获 — 默认 opt-out 验证（核心隐私）
- [ ] 装好扩展后立即访问任何网站 → `attune --diag` 或 `GET /api/v1/browse_signals` 应显示 `count=0`（默认不捕获）
- [ ] 打开扩展 popup → Privacy tab → whitelist 列表为空
- [ ] **隐私模式硬阻断**：开 Chrome incognito 窗口 → 添加 example.com 到 whitelist → 访问 example.com 浏览 5 分钟 → count 仍 0（incognito 不捕获）
- [ ] **HARD_BLACKLIST 双层验证**：手动加 `github.com` 到 whitelist → 访问 `github.com/login` 5 分钟 + 滚动 → count 增加；访问 `github.com/some-page` 5 分钟 → 受 path 黑名单覆盖**不应**捕获 login

### G1 信号上报 + 加密
- [ ] whitelist 加 `github.com` → 浏览 github 任意页面 5 分钟 → 30 秒后 attune 日志看到 POST /api/v1/browse_signals 200 → count 增加
- [ ] `sqlite3 vault.sqlite "SELECT hex(url_enc) FROM browse_signals LIMIT 1"` 输出非可读（DEK 加密）
- [ ] `sqlite3 ... "SELECT domain_hash FROM browse_signals LIMIT 1"` 输出 64 hex 字符（HMAC-SHA256 with pepper）

### G2 高 engagement 评分
- [ ] 浏览某页 1 分钟（不达 3 分钟）→ POST 响应 `high_engagement: 0`
- [ ] 浏览某页 4 分钟 + 滚动 80% + 复制一段文字 → POST 响应 `high_engagement: 1`
- [ ] G2 v1 仅计数不创建 item（W5-6 G3 才会真正抓内容）— 知识库不应出现"github.com"占位条目

### G5 隐私控制面板
- [ ] popup 显示已捕获信号数 + Pause toggle + whitelist 增删
- [ ] 全局 Pause 后浏览 whitelist 域名 → count 不增（content script 检查 browsePaused）
- [ ] "清除所有已捕获" 按钮 → DELETE /api/v1/browse_signals → count 归零
- [ ] per-domain "清除" 按钮 → 仅清该域名 signals

### F3 J5 secondary retrieval
- [ ] 自动化测试 `cargo test -p attune-core --test rag_w3_batch_b_integration` 全绿（5 测试）

## W3 Batch A: F2 + C1 + F1 + F4（2026-04-27）

### F2 Citation breadcrumb 透传
- [ ] 上传一份 4 级标题 markdown → chat 询问深节内容 → API 响应 `citations[0].breadcrumb` 数组非空
- [ ] **Known limitation v1 验证**：breadcrumb 是 item 顶层路径（item 第一个 chunk），不是具体命中段；offset 是 sidecar 累计 char 不严格对齐原文 — 前端 Reader 跳转 W3v1 仅顶层导航，精确高亮等 W5+
- [ ] WebDAV 同步进来的 item 也有 breadcrumb（验证 scanner_webdav 接入）
- [ ] 文件夹监听扫描的 item 也有 breadcrumb（验证 scanner.rs 接入）
- [ ] **软删除安全**：删除 item 后再问相同问题 → Citation 不应再引用已删 item 的 breadcrumb（reviewer R2 P0-1 验收）

### C1 Web search local cache
- [ ] 关闭网络 → chat 询问知识库无结果但需 web search 的问题 → 报错（无缓存）
- [ ] 联网 → 同问题 → web search 触发 → 答案显示 + 日志 `C1: web_search cache HIT` 缺失（首次 miss）
- [ ] 30 秒内重问同问题 → 日志显示 `C1: web_search cache HIT (saved network call)`，无网络请求
- [ ] **C1 闭环 (W4-002, 2026-04-27)**：`curl -X DELETE http://localhost:18900/api/v1/web_search_cache` 返回 `{"deleted": N}`；`curl http://localhost:18900/api/v1/web_search_cache` 返回 `{"count": 0}`；后续相同 query → 必须重新触发 web search（缓存已清）
- [ ] **C1 vault locked 防御**：lock vault 后 `curl -X DELETE /api/v1/web_search_cache` 返回 403，不泄露 cache count
- [ ] 加密验证：`sqlite3 vault.sqlite "SELECT hex(results_json_enc) FROM web_search_cache LIMIT 1"` 输出二进制非可读 JSON

### F1 二次检索可观测性
- [ ] chat 询问知识库无答案的问题 → 日志看到 `J5 F1: secondary retrieval result` 行，`local_was_empty=true broader_count=0` 或类似
- [ ] 询问含模糊词的本地问题（confidence 1-2/5）→ 日志 `J5 F1` 显示 `broader_count > pre_count`，触发二次 LLM 调用

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
