//! MCP (Model Context Protocol) client — stdio 协议最小实装.
//!
//! 用于 attune 调用外部数据源 plugin (如 lpr_history_mcp / court_judgment_mcp).
//!
//! 协议: JSON-RPC 2.0 over stdio (LSP-like Content-Length framed messages).
//! 不实装: http transport / 资源订阅 / 长任务进度 — 留待 v2 扩展.
//!
//! 生命周期 (eager): spawn 后常驻, 每 30s 调 ping, 失败 N 次重启.

use crate::error::{Result, VaultError};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// MCP server 配置
#[derive(Debug, Clone)]
pub struct McpConfig {
    pub id: String,
    pub command: PathBuf,
    pub args: Vec<String>,
    pub heartbeat_interval: Duration,
    pub restart_on_failure: u32,
}

impl McpConfig {
    pub fn new<P: Into<PathBuf>>(id: &str, command: P) -> Self {
        Self {
            id: id.to_string(),
            command: command.into(),
            args: Vec::new(),
            heartbeat_interval: Duration::from_secs(30),
            restart_on_failure: 3,
        }
    }
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }
}

/// MCP JSON-RPC 请求
#[derive(Debug, Clone, Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// MCP JSON-RPC 响应
#[derive(Debug, Clone, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: u64,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

/// 长生命 MCP 进程句柄
pub struct McpServer {
    config: McpConfig,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    stdout: Mutex<Option<BufReader<ChildStdout>>>,
    next_id: AtomicU64,
    last_heartbeat: Mutex<Instant>,
    failure_count: Mutex<u32>,
}

impl McpServer {
    /// spawn 进程 + 初始化
    pub fn spawn(config: McpConfig) -> Result<Self> {
        let server = Self {
            config,
            child: Mutex::new(None),
            stdin: Mutex::new(None),
            stdout: Mutex::new(None),
            next_id: AtomicU64::new(1),
            last_heartbeat: Mutex::new(Instant::now()),
            failure_count: Mutex::new(0),
        };
        server.start_process()?;
        Ok(server)
    }

    fn start_process(&self) -> Result<()> {
        let mut cmd = Command::new(&self.config.command);
        cmd.args(&self.config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(VaultError::Io)?;
        let stdin = child.stdin.take().ok_or_else(|| {
            VaultError::Io(std::io::Error::other("mcp child stdin unavailable"))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            VaultError::Io(std::io::Error::other("mcp child stdout unavailable"))
        })?;

        *self.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
        *self.stdin.lock().unwrap_or_else(|e| e.into_inner()) = Some(stdin);
        *self.stdout.lock().unwrap_or_else(|e| e.into_inner()) = Some(BufReader::new(stdout));
        Ok(())
    }

    /// 调用一个 MCP tool
    pub fn call_tool(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method,
            params: Some(params),
        };
        let body = serde_json::to_string(&req)
            .map_err(|e| VaultError::Io(std::io::Error::other(format!("serialize: {e}"))))?;
        let framed = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        // 写入 stdin
        {
            let mut stdin_guard = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
            let stdin = stdin_guard
                .as_mut()
                .ok_or_else(|| VaultError::Io(std::io::Error::other("mcp stdin not connected")))?;
            stdin.write_all(framed.as_bytes()).map_err(VaultError::Io)?;
            stdin.flush().map_err(VaultError::Io)?;
        }

        // 读取响应
        let response_body = self.read_framed_message()?;
        let resp: JsonRpcResponse = serde_json::from_str(&response_body)
            .map_err(|e| VaultError::Io(std::io::Error::other(format!("parse mcp resp: {e}"))))?;

        if let Some(err) = resp.error {
            return Err(VaultError::Io(std::io::Error::other(format!(
                "mcp error code={}: {}", err.code, err.message
            ))));
        }
        Ok(resp.result.unwrap_or(serde_json::Value::Null))
    }

    /// 读取一个 LSP-style framed message
    fn read_framed_message(&self) -> Result<String> {
        let mut stdout_guard = self.stdout.lock().unwrap_or_else(|e| e.into_inner());
        let stdout = stdout_guard
            .as_mut()
            .ok_or_else(|| VaultError::Io(std::io::Error::other("mcp stdout not connected")))?;

        let mut content_length: Option<usize> = None;
        loop {
            let mut header_line = String::new();
            let n = stdout.read_line(&mut header_line).map_err(VaultError::Io)?;
            if n == 0 {
                return Err(VaultError::Io(std::io::Error::other("mcp eof")));
            }
            let trimmed = header_line.trim_end();
            if trimmed.is_empty() {
                break; // header 段结束
            }
            if let Some(len_str) = trimmed.strip_prefix("Content-Length:") {
                content_length = Some(
                    len_str
                        .trim()
                        .parse()
                        .map_err(|e| VaultError::Io(std::io::Error::other(format!("bad cl: {e}"))))?,
                );
            }
        }
        let len = content_length.ok_or_else(|| {
            VaultError::Io(std::io::Error::other("mcp resp missing Content-Length"))
        })?;
        let mut body = vec![0u8; len];
        std::io::Read::read_exact(stdout.get_mut(), &mut body).map_err(VaultError::Io)?;
        String::from_utf8(body).map_err(|e| VaultError::Io(std::io::Error::other(e.to_string())))
    }

    /// 心跳 ping (基于 mcp ping method, 失败累计触发重启)
    pub fn ping(&self) -> Result<()> {
        match self.call_tool("ping", serde_json::json!({})) {
            Ok(_) => {
                *self.failure_count.lock().unwrap_or_else(|e| e.into_inner()) = 0;
                *self.last_heartbeat.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
                Ok(())
            }
            Err(e) => {
                let mut fc = self.failure_count.lock().unwrap_or_else(|e| e.into_inner());
                *fc += 1;
                if *fc >= self.config.restart_on_failure {
                    drop(fc);
                    self.restart()?;
                    return Ok(());
                }
                Err(e)
            }
        }
    }

    /// 重启进程 (心跳失败 N 次后)
    pub fn restart(&self) -> Result<()> {
        if let Some(mut child) = self.child.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        *self.stdin.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.stdout.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.failure_count.lock().unwrap_or_else(|e| e.into_inner()) = 0;
        self.start_process()
    }

    pub fn id(&self) -> &str {
        &self.config.id
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用 sh 启动 echo 服务模拟最小 MCP 行为
    /// (实际 MCP server 需实现完整 JSON-RPC, 此测试只验证 spawn + drop 不 panic)
    #[test]
    fn spawn_and_drop_does_not_leak() {
        let sh = which::which("sh").unwrap_or_else(|_| PathBuf::from("/bin/sh"));
        if !sh.exists() {
            eprintln!("skip: sh not found");
            return;
        }
        let config = McpConfig::new("test-mcp", &sh).args(["-c", "sleep 60"]);
        let server = McpServer::spawn(config).expect("spawn");
        assert_eq!(server.id(), "test-mcp");
        // drop 应 kill child
        drop(server);
    }

    #[test]
    fn missing_binary_returns_io_error() {
        let config = McpConfig::new("missing", "/nonexistent/binary/path/zzz");
        match McpServer::spawn(config) {
            Ok(_) => panic!("expected IO error"),
            Err(VaultError::Io(_)) => {}
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn config_builder_chains() {
        let cfg = McpConfig::new("x", "/bin/echo").args(["a", "b", "c"]);
        assert_eq!(cfg.args.len(), 3);
        assert_eq!(cfg.heartbeat_interval, Duration::from_secs(30));
        assert_eq!(cfg.restart_on_failure, 3);
    }

    #[test]
    fn json_rpc_request_serializes() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 42,
            method: "ping",
            params: Some(serde_json::json!({})),
        };
        let s = serde_json::to_string(&req).expect("ser");
        assert!(s.contains("\"jsonrpc\":\"2.0\""));
        assert!(s.contains("\"id\":42"));
        assert!(s.contains("\"method\":\"ping\""));
    }

    #[test]
    fn json_rpc_error_response_parses() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(raw).expect("parse");
        let err = resp.error.expect("has error");
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }
}
