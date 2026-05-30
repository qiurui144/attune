# Release gap P1 闭环：Web UI 渲染 chat 响应的 `acp_flow` 块

**日期**: 2026-05-30  **分支**: feature/acp-flow-ui（基于 origin/develop）  **范围**: OSS attune Rust 商用线嵌入式 Web UI（React + Preact signals + TS + i18n）

## 1. 背景 / gap

ACP-5 chat-flow wiring 已 ship（v1.1.0 GA）：`routes/chat.rs` 在 chat 响应附加 `acp_flow` 块（flow_id / status / 每步 StepTrace）。但 Web UI **不渲染**它 —— 用户看不到自主流转发生（RELEASE.md Known Limitation / release gap P1）。本次补齐渲染。

后端 `acp_flow` 真实结构（`acp_chat.rs::ChatFlowOutcome` + `docs/reports/acp5-verify/chat-response-evidence.json`）：
```
{ flow_id, status: complete|partial|degraded|aborted,
  steps: [{ agent_id, ran, degraded, note }], final_type, final_value }
```

## 2. 渲染组件设计

`ChatMessage.tsx` 新增 `AcpFlowPanel`（紧随现有 `CitationRow` 之后渲染，复用 chip 样式，不动 RAG/citations/cost 块）：

- **头行**：`⚙ Autonomous flow` 标签 + `flow_id` 等宽 chip + **状态 badge**（颜色编码）：
  - `complete` 绿 / `partial`·`degraded` 黄(amber) / `aborted` 红
- **可展开步骤 trace**：点 `Show step details (N)` 展开 `<ul role=…>`，每步：
  - 三态圆点 + 状态词：`degraded`(黄) / `ran`(绿) / `skipped`(灰，!ran)
  - `agent_id` + `note`（如 entitlement 拦截原因）
- 渲染门：`m.acp_flow && !streaming`（打字机完成后才显示，与 citations 一致）。
- 未知 status 兜底：badge 退回 `partial` 样式 + 显示 raw status（后端将来加 status 不炸）。

数据流接线：`signals.ts` 加 `AcpFlow`/`AcpFlowStep`/`AcpFlowStatus` 类型 + `Message.acp_flow?`；`useChat.ts::ChatResponse` 加 `acp_flow?`，`sendMessage` 把 `res.acp_flow` 挂到 assistantMsg。

### 调试发现的真 bug（user-first Playwright 暴露）

首轮真测 acp_flow 块**不渲染**。根因：首次发送（无 session）后 `sendMessage` 回填 `activeSessionId` → 触发 `ChatView` 的 `useEffect([currentSid])` → `loadSession()` 用服务端 history **覆盖**内存消息，而 history **不持久化 acp_flow**（live-only trace）→ 刚附的 acp_flow 被冲掉。
**修复**：`useChat` 加 `skipNextSessionLoad` guard，回填 session 那一次重载跳过，保留内存消息（含 acp_flow）。已知限制：history 不存 acp_flow，重新打开旧会话不显示 flow 块（仅 live 显示）—— 完整持久化需 store schema 变更，超本 P1 范围（另起 spec）。

## 3. i18n（CLAUDE.md 铁律）

`zh.ts` + `en.ts` 同步加 14 key（`chat.flow.label` / `chat.flow.status.{complete,partial,degraded,aborted}` / `chat.flow.step.{ran,degraded,skipped}` / `chat.flow.{expand,collapse,steps_aria}`）。
两条 grep 守卫均 0 输出（硬编码中文 UI 字面量 = 0；zh/en key 集合 diff = 0）。浏览器 locale=en，实测全英文外壳无中英混杂。

## 4. Playwright Chrome 真验（§2.2 user-first / §6.4）

环境：release 二进制 `attune-server-headless`（嵌入新 dist）+ 隔离 HOME + 1 agent flow/22 agents loaded + OpenAI 兼容 mock LLM stub（127.0.0.1:18988，不起 ollama）。

用户路径（全程 Chrome MCP）：navigate → vault setup（密码 `test-pass-not-real-123`）→ unlock（真 unlock 屏）→ MainApp → 发 defamation chat「对方公开诽谤侮辱损害我的名誉权，精神损害赔偿」（命中 `legal_defamation` route_keywords）。

后端响应实测 `acp_flow.status=degraded`，2 步 `ran=false`（fact_extractor / defamation_extractor 均 `tier=paid` 被 free 用户 entitlement 拦截）—— 与 evidence.json 一致。

UI 实测渲染：
- 折叠态：`⚙ Autonomous flow` + `legal_defamation` chip + 黄 `Degraded` badge + `Show step details (2)`。
- 展开态：两步 `· skipped` + note「blocked (entitlement): agent X requires a paid plan (tier=paid)」。
- **free 用户 degraded 清晰可见、非静默**。

截图（committed）：
- `docs/screenshots/v1.1.0-acp-flow-ui/acp-flow-01-collapsed-degraded.png`
- `docs/screenshots/v1.1.0-acp-flow-ui/acp-flow-02-expanded-step-trace.png`

## 5. build 验证

- `npm run build`（tsc --noEmit + vite singlefile）通过 → `dist/index.html` 320 KB；嵌入链 `routes/ui.rs include_str!("../../ui/dist/index.html")`。
- `cargo build --release -p attune-server --bin attune-server-headless` 通过；服务端实测 embed 含新组件（live bundle grep 命中 `acp_flow` / `Autonomous flow` / `自主流转`）。

## 6. 改动文件 + commit / push

改动：`ChatMessage.tsx`（AcpFlowPanel）/ `signals.ts`（类型）/ `useChat.ts`（接线 + skip guard）/ `ChatView.tsx`（effect guard）/ `i18n/{zh,en}.ts`（14 key）/ `dist/index.html`（rebuild）。

commit SHA / push 证据见本节末（提交后回填）。
