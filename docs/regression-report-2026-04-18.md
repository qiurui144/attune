# Attune 完整回归报告 — 2026-04-18

**环境**: Ryzen 7 8845H + Radeon 780M + XDNA NPU (192.168.100.201) · Ollama qwen2.5:3b 本地

**执行方式**: Playwright MCP E2E（真实 UI 交互）+ Ollama 真实 LLM 调用 + SQLite 直连验证状态

| Phase | 场景 | 断言数 | 通过 | 结果 |
|-------|------|-------|------|------|
| P1 | 首次启动 → 3 步 setup wizard (角色/密码/AI 后端) → sealed→unlocked | 4 | 4 | ✅ |
| P2 | 顶栏菜单 + 模型 chip 下拉互斥 + Settings 模态 + 4 provider × 3 strategy 保存 roundtrip + ESC | 19 | 19 | ✅ |
| P3 | 真实 2KB markdown 上传（MySQL 索引原理）→ 12 chunk 入队 → 3s 抽干 | — | — | ✅ |
| P4 | Reader 打开条目 + 3 色批注（⭐/🤔/🗑）+ 右栏卡片渲染 + 删除 过时 | 11 | 11 | ✅ |
| P5 | AI 4 角度分析（highlights/risk/outdated/questions）→ 13 条 AI 批注 | — | — | ✅ |
| P6 | Chat RAG cache miss → hit（10s → 3s，3.3× 加速）+ 批注 boost 生效 | — | — | ✅ |
| P7 | local→openai 切换：chip 🟢→🔵 + token chip 成本 免费→$0.00003 琥珀色 | 5 | 5 | ✅ |
| P8 | 中断场景：Settings ESC / AI 分析期间关 reader 无 toast / popup 先于 reader ESC | 10 | 10 | ✅ |
| P9 | Lock → 403 API + chip 降级 → 密码 unlock → 数据全部持久（1 item + 29 批注） | 8 | 8 | ✅ |
| P10 | 软删除 item → annotations 29→0 / chunk_summaries 1→0 级联 + JOIN filter 兜底 | — | — | ✅ |

**量化总计**
- **57 项显式断言，57 通过（100%）**
- 0 失败 · 0 需重测

**核心功能闭环验证**
- 成本/触发契约：🆓 批注加权零成本 · 💰 LLM 调用仅在用户显式触发（chat/AI 分析）
- 加密持久：批注 content / chunk_summary 都加密 BLOB · lock/unlock 跨周期数据不丢
- 级联正确：item 软删 → annotations + chunk_summaries 硬删（"忘记知识"语义）
- UI 成本透明：本地绿 · 云端琥珀 + $金额 · token chip popover 显示"检索候选/最终注入/boost/剔除/缓存/策略"
- 异常路径：AI 分析期间用户关 reader → 无错误 toast · 服务端批注仍落库 · 锁定期间 API 403 而非 500
