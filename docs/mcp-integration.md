# MCP 集成 — 让任何 MCP 客户端把 Attune 当作知识源

> 状态：v0.6 stable（W4 deliverable，2026-04-27）。
> 路线：薄 stdio shim 包装 Attune 现有 REST API。按 strategy plan 决策不自研 MCP server crate，
> 用最小可能的适配器让所有 MCP 兼容客户端（Claude Desktop / Cursor / Continue / Cherry Studio /
> LobeChat / open-webui，或你自己写的脚本）都能调 Attune。

## 你能得到什么

配置完成后，MCP 客户端可以：

- `attune_search(query: str, top_k: int = 10)` — Attune vault 上的混合（BM25 + 向量）搜索。
  返回排序后的 items + title + snippet + 引用。
- `attune_get_item(id: str)` — 拿一个 item 的完整内容。
- `attune_chat(prompt: str)` — 基于 vault 的 RAG-grounded chat 答案，带引用。

所有调用打到你**本地**的 Attune server（默认 `http://localhost:18900`）。Vault 留在你的机器上，
MCP 客户端只看到搜索/chat 结果，不接触原始 vault。

## 快速开始（Claude Desktop）

1. 确保 Attune server 跑起来 + vault 已解锁：
   ```bash
   cd rust && cargo run --bin attune-server
   curl http://localhost:18900/api/v1/status   # 应返回 unlocked: true
   ```

2. 把 `tools/attune_mcp_shim.py`（仓库内提供）放到稳定位置 — 比如
   `~/.local/share/attune/attune_mcp_shim.py`。

3. 编辑 `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS) 或
   `%APPDATA%\Claude\claude_desktop_config.json` (Windows)：

   ```json
   {
     "mcpServers": {
       "attune": {
         "command": "python3",
         "args": ["/绝对路径/到/attune_mcp_shim.py"],
         "env": {
           "ATTUNE_BASE_URL": "http://localhost:18900",
           "ATTUNE_API_TOKEN": "你的-token"
         }
       }
     }
   }
   ```

4. 重启 Claude Desktop。新对话里输入 `@attune search "rust ownership"` —
   应该看到 vault 命中流入答案。

## 快速开始（Cursor）

Cursor 读 MCP 配置 `~/.cursor/mcp.json`：

```json
{
  "mcpServers": {
    "attune": {
      "command": "python3",
      "args": ["/绝对路径/到/attune_mcp_shim.py"]
    }
  }
}
```

然后在 Cursor: `@attune` 会触发工具列表 → 选 `attune_search`。

## 快速开始（Continue / Cherry Studio / LobeChat / open-webui）

这些客户端都支持 stdio 传输的 MCP，JSON 格式相同 — 只要把 `command + args` 指向
`attune_mcp_shim.py`。具体配置文件位置见各客户端文档。

## Shim 工作原理

`tools/attune_mcp_shim.py` 约 120 行 Python。在 stdio (`stdin/stdout`) 上说 MCP JSON-RPC 2.0，
把每个工具调用翻译成 `requests.get/post` 打到本地 Attune REST API。无持久化、无缓存、无业务逻辑，
真正干活的是 Attune。

意思是：
- ✅ 零 Rust 依赖增加 — shim 只需 Python ≥3.9
- ✅ MCP 协议版本变化只改一个文件
- ✅ 加新工具（如 `attune_create_item`）每个 ~10 行
- ⚠️ 每个 MCP 客户端连接起一个 Python 进程（个人用毫无影响）

## 配置参考

| 环境变量 | 默认 | 用途 |
|---------|------|------|
| `ATTUNE_BASE_URL` | `http://localhost:18900` | server 监听地址 |
| `ATTUNE_API_TOKEN` | _(空)_ | 配了 server token 就填这里 |
| `ATTUNE_TIMEOUT_SEC` | `30` | 单次 HTTP 超时 |
| `ATTUNE_DEBUG` | _(未设置)_ | 设 `1` 开 stderr 详细日志 |

## 鉴权

shim 每次请求带 `Authorization: Bearer $ATTUNE_API_TOKEN`。如果 server 跑无 auth 模式（开发期），
环境变量留空即可。

## 隐私边界

按 Attune "私有 AI 知识伙伴" 承诺：
- Vault 永不离开你的机器 — 走 MCP 时 MCP 客户端只看到搜索/chat **结果**，不接触原始 vault
- shim 不 log 查询（设 `ATTUNE_DEBUG=1` 才会写 stderr，生产环境千万别开）
- 所有 Attune 端加密（DEK + AES-256-GCM）继续生效；shim 只是翻译层

## 故障排查

| 现象 | 原因 | 修复 |
|------|------|------|
| `Connection refused` | server 没跑 | `cd rust && cargo run --bin attune-server` |
| `403 vault locked or unavailable` | vault 没解锁 | 打开 Attune 桌面或调 `/api/v1/vault/unlock` |
| MCP 客户端不显示 "attune" | 配置路径错 | 用绝对路径 + 检查 JSON 语法 |
| 结果空 | vault 空 / 查询不对 | 直接调 `/api/v1/items` 验证内容 |

## 版本策略

- v0.6: search / get_item / chat 三个工具（本文档）
- v0.7: 加 `attune_list_recent`、`attune_create_note` 写操作
- shim 暴露的工具名跨小版本稳定；改名会与大版本一起放进 `RELEASE.md`

## 为什么这么设计（而不是 Rust MCP crate）

按 strategy plan v4: "C2 用 gpt-researcher 现成 MCP server，不要自研" — Attune 的价值
在 vault + RAG 质量，不在重新实现 MCP 协议。120 行 Python shim：
- 让 MCP 生态自由演进，不必每次重 build Attune
- 保持 Attune 二进制精简（不加 Rust crate，不加传递依赖）
- 证明任何 HTTP 后端都能在一小时内变 MCP 可调

如果你需要 Rust 原生 MCP server（比如嵌入式部署 Python 不可用），提 issue —
`attune-mcp-server` 在 v0.8 路线图。

## 参考

- Strategy plan v4 (W4 C2 决策): 见 `docs/v0.6-release-readiness.md`
- MCP 协议 spec: <https://modelcontextprotocol.io/>
- gpt-researcher MCP 示例: <https://github.com/assafelovic/gpt-researcher>
- Attune REST API: 见 `rust/RELEASE.md` endpoint 清单
