//! Plugin subprocess dispatcher.
//!
//! 给上层 (chat handler / Web UI) 提供调用 plugin 二进制的统一 API.
//!
//! 设计:
//! - subprocess 隔离 (attune-core 不 link plugin lib, 保持 OSS-pro 边界)
//! - JSON I/O 协议 (stdin/stdout) 跨语言友好
//! - exit code 透传: 0 success / 2 业务红线 / 其他业务定义
//! - 超时控制 (默认 60s, 大容量 OCR 等场景调高)
//!
//! 不做:
//! - 进程池 / 缓存 (调用方按需)
//! - 业务输出解析 (调用方按 plugin schema)
//! - sandbox (信任已装载, sandbox 是签名验证 plugin_sig.rs 的事)

use crate::error::{Result, VaultError};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Capability binary 调用结果
#[derive(Debug, Clone)]
pub struct CapabilityResult {
    /// 进程 exit code (0 成功 / 1 错误 / 2 业务红线 / 其他业务定义)
    pub exit_code: i32,
    /// stdout (通常 JSON 业务输出)
    pub stdout: String,
    /// stderr (人类可读 audit_trail / progress / 错误)
    pub stderr: String,
    /// 是否超时
    pub timed_out: bool,
}

impl CapabilityResult {
    /// exit code = 0
    pub fn is_success(&self) -> bool { self.exit_code == 0 && !self.timed_out }
    /// exit code = 2 (业务红线)
    pub fn is_red_line(&self) -> bool { self.exit_code == 2 }
}

/// 调用规格
#[derive(Debug, Clone)]
pub struct CapabilityInvocation {
    /// 二进制绝对路径 (调用方负责: PATH 查找 / plugin dir 解析)
    pub binary: PathBuf,
    /// 命令行参数
    pub args: Vec<String>,
    /// 喂给 stdin 的内容 (常见: JSON 事实)
    pub stdin: Option<String>,
    /// 环境变量 (常见: LLM_ENDPOINT / LLM_API_KEY)
    pub env: Vec<(String, String)>,
    /// 超时 (默认 60s)
    pub timeout: Duration,
}

impl CapabilityInvocation {
    pub fn new<P: Into<PathBuf>>(binary: P) -> Self {
        Self {
            binary: binary.into(),
            args: Vec::new(),
            stdin: None,
            env: Vec::new(),
            timeout: Duration::from_secs(60),
        }
    }
    pub fn arg<S: Into<String>>(mut self, a: S) -> Self {
        self.args.push(a.into());
        self
    }
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }
    pub fn stdin<S: Into<String>>(mut self, s: S) -> Self {
        self.stdin = Some(s.into());
        self
    }
    pub fn env<K: Into<String>, V: Into<String>>(mut self, k: K, v: V) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = d;
        self
    }
}

/// 调 capability binary (subprocess), 同步等待结果
///
/// 不 spawn 子进程在 caller 线程跑 — 调用方如需异步用 tokio::task::spawn_blocking 包装.
pub fn dispatch(invocation: &CapabilityInvocation) -> Result<CapabilityResult> {
    if !invocation.binary.exists() {
        return Err(VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("capability binary not found: {}", invocation.binary.display()),
        )));
    }

    let mut cmd = Command::new(&invocation.binary);
    cmd.args(&invocation.args);
    for (k, v) in &invocation.env {
        cmd.env(k, v);
    }
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(VaultError::Io)?;

    // 喂 stdin 后立即关闭 (避免子进程在 stdin 上 hang)
    if let Some(stdin_str) = &invocation.stdin {
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(stdin_str.as_bytes()).map_err(VaultError::Io)?;
            // drop(stdin) 隐式 close
        }
    } else {
        drop(child.stdin.take());
    }

    // 超时 wait (用 std::thread + flag 实现, 避免引入 tokio 在 lib 层)
    let started = std::time::Instant::now();
    loop {
        match child.try_wait().map_err(VaultError::Io)? {
            Some(status) => {
                let exit_code = status.code().unwrap_or(-1);
                let stdout = child
                    .stdout
                    .take()
                    .map(|mut s| {
                        use std::io::Read;
                        let mut buf = String::new();
                        let _ = s.read_to_string(&mut buf);
                        buf
                    })
                    .unwrap_or_default();
                let stderr = child
                    .stderr
                    .take()
                    .map(|mut s| {
                        use std::io::Read;
                        let mut buf = String::new();
                        let _ = s.read_to_string(&mut buf);
                        buf
                    })
                    .unwrap_or_default();
                return Ok(CapabilityResult {
                    exit_code,
                    stdout,
                    stderr,
                    timed_out: false,
                });
            }
            None => {
                if started.elapsed() > invocation.timeout {
                    let _ = child.kill();
                    return Ok(CapabilityResult {
                        exit_code: -1,
                        stdout: String::new(),
                        stderr: format!("capability timed out after {:?}", invocation.timeout),
                        timed_out: true,
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Capability 执行运行时分流类型 (spec §5.3).
///
/// - `RustBinary`: 现有 subprocess(平台相关二进制)
/// - `Wasm`: wasm32-wasip1 模块,wasmtime 执行(一包通吃所有平台)
/// - `DataOnly`: 无执行体(纯 prompt + JSON schema,宿主侧组合)
///
/// `python_subprocess` 不入 enum — parse 时遇到返回 `unsupported-runtime` Err
/// (声明未实现,不 silent NotFound)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityRuntime {
    RustBinary,
    Wasm,
    DataOnly,
}

/// 解析 manifest runtime 字符串到 `CapabilityRuntime`。
///
/// `python_subprocess` / 任何未知值 → `unsupported-runtime` Err。
pub fn parse_runtime(s: &str) -> Result<CapabilityRuntime> {
    match s {
        "rust_binary" => Ok(CapabilityRuntime::RustBinary),
        "wasm" => Ok(CapabilityRuntime::Wasm),
        "data_only" => Ok(CapabilityRuntime::DataOnly),
        "python_subprocess" => Err(VaultError::InvalidInput(
            "unsupported-runtime: python_subprocess is declared but not implemented".into(),
        )),
        other => Err(VaultError::InvalidInput(format!(
            "unsupported-runtime: unknown runtime '{other}'"
        ))),
    }
}

/// 在 plugin dir 下解析 wasm 模块路径 (runtime=wasm)。
pub fn resolve_wasm(plugin_dir: &Path, rel: &str) -> Option<PathBuf> {
    let p = plugin_dir.join(rel);
    if p.exists() {
        Some(p)
    } else {
        None
    }
}

/// 统一 capability 分流入口 (spec §3.2):调用方按 `runtime` 透明分流,
/// 不感知 RustBinary / Wasm / DataOnly 差异,产物统一 `CapabilityResult`。
///
/// - `RustBinary`: `entry` = 已解析的 binary 绝对路径,走现有 `dispatch`。
/// - `Wasm`: `entry` = .wasm 模块绝对路径;`wasm-runtime` feature 开 → WasmRunner;
///   关 → `unsupported-runtime` Err(不 silent 成功)。
/// - `DataOnly`: 无执行体,返回明确 Err(宿主侧 LLM lane 处理,不该走 dispatch)。
pub fn dispatch_capability(
    runtime: CapabilityRuntime,
    invocation: &CapabilityInvocation,
) -> Result<CapabilityResult> {
    match runtime {
        CapabilityRuntime::RustBinary => dispatch(invocation),
        CapabilityRuntime::Wasm => {
            #[cfg(feature = "wasm-runtime")]
            {
                crate::wasm_runtime::WasmRunner::shared().run(invocation)
            }
            #[cfg(not(feature = "wasm-runtime"))]
            {
                Err(VaultError::InvalidInput(
                    "unsupported-runtime: wasm capability requires the 'wasm-runtime' \
                     feature (disabled in this build)"
                        .into(),
                ))
            }
        }
        CapabilityRuntime::DataOnly => Err(VaultError::InvalidInput(
            "data_only capability has no executable; handle via host LLM lane".into(),
        )),
    }
}

/// 在 plugin dir 下解析 binary 路径 (per plugin.yaml `capability_binary` 字段或默认 `bin/run_<id>`)
///
/// 约定:
/// - `<plugin_dir>/bin/run_<capability_id>` 或
/// - `<plugin_dir>/target/release/run_<capability_id>` (开发期 cargo build --release)
/// - `which run_<capability_id>` (PATH 已安装)
pub fn resolve_binary(plugin_dir: &Path, capability_id: &str) -> Option<PathBuf> {
    let bin_name = format!("run_{capability_id}");
    let candidates = [
        plugin_dir.join("bin").join(&bin_name),
        plugin_dir.join("target").join("release").join(&bin_name),
        plugin_dir
            .parent()
            .and_then(|p| p.parent())
            .map(|w| w.join("target").join("release").join(&bin_name))
            .unwrap_or_else(|| PathBuf::from(&bin_name)),
    ];
    for c in &candidates {
        if c.exists() {
            return Some(c.clone());
        }
    }
    // PATH 兜底
    which::which(&bin_name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_returns_success_for_echo() {
        // 用 /bin/echo 验证 dispatch 链路 (跨平台: macOS/Linux 都有)
        let echo = which::which("echo").unwrap_or_else(|_| PathBuf::from("/bin/echo"));
        if !echo.exists() {
            eprintln!("skip: echo not found");
            return;
        }
        let inv = CapabilityInvocation::new(&echo).arg("hello");
        let r = dispatch(&inv).expect("dispatch");
        assert_eq!(r.exit_code, 0);
        assert!(r.is_success());
        assert!(r.stdout.contains("hello"));
        assert!(!r.timed_out);
    }

    #[test]
    fn dispatch_propagates_exit_code_2_red_line() {
        // 用 sh -c 'exit 2' 模拟业务红线退出码
        let sh = which::which("sh").unwrap_or_else(|_| PathBuf::from("/bin/sh"));
        if !sh.exists() {
            eprintln!("skip: sh not found");
            return;
        }
        let inv = CapabilityInvocation::new(&sh).args(["-c", "exit 2"]);
        let r = dispatch(&inv).expect("dispatch");
        assert_eq!(r.exit_code, 2);
        assert!(r.is_red_line());
        assert!(!r.is_success());
    }

    #[test]
    fn dispatch_pipes_stdin_to_subprocess() {
        let cat = which::which("cat").unwrap_or_else(|_| PathBuf::from("/bin/cat"));
        if !cat.exists() {
            eprintln!("skip: cat not found");
            return;
        }
        let inv = CapabilityInvocation::new(&cat).stdin("test_stdin_payload");
        let r = dispatch(&inv).expect("dispatch");
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("test_stdin_payload"));
    }

    #[test]
    fn dispatch_timeout_kills_long_running() {
        let sleep = which::which("sleep").unwrap_or_else(|_| PathBuf::from("/bin/sleep"));
        if !sleep.exists() {
            eprintln!("skip: sleep not found");
            return;
        }
        let inv = CapabilityInvocation::new(&sleep)
            .arg("10")
            .timeout(Duration::from_millis(200));
        let r = dispatch(&inv).expect("dispatch");
        assert!(r.timed_out);
        assert_eq!(r.exit_code, -1);
    }

    #[test]
    fn dispatch_propagates_env_vars() {
        let sh = which::which("sh").unwrap_or_else(|_| PathBuf::from("/bin/sh"));
        if !sh.exists() {
            eprintln!("skip: sh not found");
            return;
        }
        let inv = CapabilityInvocation::new(&sh)
            .args(["-c", "echo $MY_TEST_ENV"])
            .env("MY_TEST_ENV", "from_dispatch");
        let r = dispatch(&inv).expect("dispatch");
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("from_dispatch"), "stdout: {}", r.stdout);
    }

    #[test]
    fn dispatch_returns_io_error_for_missing_binary() {
        let inv = CapabilityInvocation::new("/nonexistent/binary/path/xyz123");
        let err = dispatch(&inv).unwrap_err();
        assert!(matches!(err, VaultError::Io(_)));
    }

    // ── 跨平台 runtime 分流 (spec §5.3) ──

    #[test]
    fn parse_runtime_known_values() {
        assert_eq!(parse_runtime("rust_binary").unwrap(), CapabilityRuntime::RustBinary);
        assert_eq!(parse_runtime("wasm").unwrap(), CapabilityRuntime::Wasm);
        assert_eq!(parse_runtime("data_only").unwrap(), CapabilityRuntime::DataOnly);
    }

    #[test]
    fn parse_runtime_python_subprocess_unsupported() {
        let err = parse_runtime("python_subprocess").unwrap_err();
        assert!(err.to_string().contains("unsupported-runtime"), "got {err}");
    }

    #[test]
    fn parse_runtime_unknown_value_unsupported() {
        let err = parse_runtime("brainfuck").unwrap_err();
        assert!(err.to_string().contains("unsupported-runtime"), "got {err}");
    }

    #[test]
    fn dispatch_capability_rust_binary_still_works() {
        // RustBinary 分支 == 现有 dispatch,行为不变
        let echo = which::which("echo").unwrap_or_else(|_| PathBuf::from("/bin/echo"));
        if !echo.exists() {
            eprintln!("skip: echo not found");
            return;
        }
        let inv = CapabilityInvocation::new(&echo).arg("router_ok");
        let r = dispatch_capability(CapabilityRuntime::RustBinary, &inv).expect("dispatch");
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("router_ok"));
    }

    #[test]
    fn dispatch_capability_data_only_is_error() {
        let inv = CapabilityInvocation::new("/unused");
        let err = dispatch_capability(CapabilityRuntime::DataOnly, &inv).unwrap_err();
        assert!(err.to_string().contains("data_only"), "got {err}");
    }

    #[cfg(not(feature = "wasm-runtime"))]
    #[test]
    fn dispatch_capability_wasm_unsupported_without_feature() {
        let inv = CapabilityInvocation::new("/unused.wasm");
        let err = dispatch_capability(CapabilityRuntime::Wasm, &inv).unwrap_err();
        assert!(err.to_string().contains("unsupported-runtime"), "got {err}");
    }

    #[test]
    fn resolve_wasm_finds_existing_file() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let wasm_dir = tmp.path().join("wasm");
        std::fs::create_dir_all(&wasm_dir).expect("mkdir");
        std::fs::write(wasm_dir.join("a.wasm"), b"\0asm").expect("write");
        assert!(resolve_wasm(tmp.path(), "wasm/a.wasm").is_some());
        assert!(resolve_wasm(tmp.path(), "wasm/missing.wasm").is_none());
    }

    #[test]
    fn resolve_binary_returns_none_when_not_found() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        // 假设 PATH 里也没有 run_unique_xyz_capability_zzz
        let r = resolve_binary(tmp.path(), "unique_xyz_capability_zzz_does_not_exist");
        assert!(r.is_none());
    }

    #[test]
    fn resolve_binary_finds_in_plugin_bin_dir() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).expect("mkdir");
        let bin_path = bin_dir.join("run_test_cap");
        std::fs::write(&bin_path, "#!/bin/sh\necho ok\n").expect("write");
        // 跨平台: Unix 才支持 chmod +x. Windows 上的可执行文件靠 .exe 后缀.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&bin_path).expect("meta").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&bin_path, perms).expect("chmod");
        }
        let r = resolve_binary(tmp.path(), "test_cap");
        assert!(r.is_some(), "should find binary at {bin_path:?}");
    }
}
