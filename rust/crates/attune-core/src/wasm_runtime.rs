//! WASM capability runtime — 跨平台 agent 分发执行引擎。
//!
//! 把 `runtime: wasm` 的 skill/agent 编到 `wasm32-wasip1`,由内嵌 wasmtime 执行。
//! 一份 `.wasm` 即在所有目标平台运行(Windows P0 / Linux P1 / riscv64 K3 P2),
//! 与现有 subprocess 契约对齐:stdin JSON → stdout JSON → exit code 0/1/2/-1。
//!
//! ## 契约映射 (spec §5.2)
//! - stdin pipe   = `CapabilityInvocation.stdin`
//! - stdout/stderr= MemoryOutputPipe 捕获 → `CapabilityResult.{stdout,stderr}`
//! - exit code    = `_start` proc_exit(N) → N;trap → 1;epoch timeout → -1(timed_out)
//!
//! ## 边界硬约束 (spec §7)
//! - 每次调用 fresh `Store`(无跨调用状态泄漏)
//! - `StoreLimits` 内存上限 256 MB(失控插件不拖垮宿主)
//! - epoch deadline 超时:后台 std::thread ticker `increment_epoch`(不引 tokio)
//! - 默认无 fs / net;按 `wasi_caps` 显式授(read:<path> / env:<KEY> / clock / stdio)
//!
//! `Engine` 进程级复用(JIT 产物可摊销);每调用新建 `Store` + `WasiP1Ctx`。

use crate::capability_dispatch::{CapabilityInvocation, CapabilityResult};
use crate::error::{Result, VaultError};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use wasmtime::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::p2::pipe::{MemoryInputPipe, MemoryOutputPipe};
use wasmtime_wasi::p1::WasiP1Ctx;
use wasmtime_wasi::{DirPerms, FilePerms, I32Exit, WasiCtxBuilder};

/// wasm linear memory 上限(256 MB,per spec §7)。
const MAX_WASM_MEMORY_BYTES: usize = 256 * 1024 * 1024;
/// stdout / stderr 捕获缓冲上限(16 MB,防输出炸内存)。
const OUTPUT_PIPE_CAPACITY: usize = 16 * 1024 * 1024;
/// epoch ticker 间隔。timeout = ceil(invocation.timeout / TICK) 个 epoch。
const EPOCH_TICK: Duration = Duration::from_millis(50);

/// Store 携带的宿主状态:WASI ctx + 资源 limiter。
struct HostState {
    wasi: WasiP1Ctx,
    limits: StoreLimits,
}

/// wasm 执行引擎。`Engine` 进程级复用(cranelift JIT 产物缓存),
/// 每次 `run` 新建 `Store` 保证无状态泄漏。
pub struct WasmRunner {
    engine: Engine,
}

impl WasmRunner {
    /// 构造一个 WasmRunner。开启 epoch_interruption(超时控制必需)。
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        let engine = Engine::new(&config)
            .map_err(|e| VaultError::InvalidInput(format!("wasm engine init failed: {e}")))?;
        Ok(Self { engine })
    }

    /// 进程级共享单例(JIT/Engine 复用)。失败时每次 run 退化为新建(返回 Err)。
    pub fn shared() -> &'static WasmRunner {
        static SHARED: OnceLock<WasmRunner> = OnceLock::new();
        SHARED.get_or_init(|| {
            WasmRunner::new().unwrap_or_else(|e| panic!("wasm runner init: {e}"))
        })
    }

    /// 执行一个 wasm capability。`invocation.binary` = .wasm 模块路径(per 决策 D-a)。
    pub fn run(&self, invocation: &CapabilityInvocation) -> Result<CapabilityResult> {
        self.run_with_caps(invocation, &[])
    }

    /// 带 wasi_caps 白名单的执行(read:<path> / env:<KEY> / clock / stdio)。
    pub fn run_with_caps(
        &self,
        invocation: &CapabilityInvocation,
        wasi_caps: &[String],
    ) -> Result<CapabilityResult> {
        let wasm_path = &invocation.binary;
        if !wasm_path.exists() {
            return Err(VaultError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("wasm module not found: {}", wasm_path.display()),
            )));
        }

        // wasm-module-invalid: 非法 module 在编译期暴露。
        let module = Module::from_file(&self.engine, wasm_path)
            .map_err(|e| VaultError::InvalidInput(format!("wasm-module-invalid: {e}")))?;

        // ── WASI ctx:默认无 fs/net,按 wasi_caps 显式授 ──
        let stdin = MemoryInputPipe::new(invocation.stdin.clone().unwrap_or_default());
        let stdout = MemoryOutputPipe::new(OUTPUT_PIPE_CAPACITY);
        let stderr = MemoryOutputPipe::new(OUTPUT_PIPE_CAPACITY);

        let mut builder = WasiCtxBuilder::new();
        builder.stdin(stdin).stdout(stdout.clone()).stderr(stderr.clone());

        // env:<KEY> 白名单:只注入声明了的 env(从 invocation.env 取值)。
        for cap in wasi_caps {
            if let Some(key) = cap.strip_prefix("env:") {
                if let Some((_, v)) = invocation.env.iter().find(|(k, _)| k == key) {
                    builder.env(key, v);
                }
            } else if let Some(path) = cap.strip_prefix("read:") {
                // read:<host_path> → 只读 preopen(guest 同名路径)。
                builder
                    .preopened_dir(path, path, DirPerms::READ, FilePerms::READ)
                    .map_err(|e| {
                        VaultError::InvalidInput(format!("wasi-cap-denied: read:{path}: {e}"))
                    })?;
            }
            // "stdio" / "clock" 默认隐含,无需额外操作(clock 由 WasiCtx 默认提供)。
        }

        let wasi = builder.build_p1();
        let limits = StoreLimitsBuilder::new()
            .memory_size(MAX_WASM_MEMORY_BYTES)
            .build();

        let mut store = Store::new(&self.engine, HostState { wasi, limits });
        store.limiter(|s| &mut s.limits);

        // ── 超时:epoch deadline + 后台 ticker(不引 tokio,与 dispatch 同 std::thread 模式)──
        // deadline = N ticks;后台线程每 EPOCH_TICK increment_epoch 一次,超 N 次 → trap。
        let ticks: u64 = {
            let t = invocation.timeout.as_millis() as u64;
            let per = EPOCH_TICK.as_millis() as u64;
            (t / per).max(1)
        };
        store.set_epoch_deadline(ticks);

        let engine = self.engine.clone();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_t = stop.clone();
        let ticker = std::thread::spawn(move || {
            while !stop_t.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(EPOCH_TICK);
                engine.increment_epoch();
            }
        });

        // ── linker:挂 WASI preview1 ──
        let mut linker: Linker<HostState> = Linker::new(&self.engine);
        let link_res = wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |s: &mut HostState| {
            &mut s.wasi
        });
        if let Err(e) = link_res {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = ticker.join();
            return Err(VaultError::InvalidInput(format!("wasm linker setup: {e}")));
        }

        // ── 实例化 + 调 _start ──
        let call_start = std::time::Instant::now();
        let outcome = (|| -> std::result::Result<(), wasmtime::Error> {
            let instance = linker.instantiate(&mut store, &module)?;
            let start = instance.get_typed_func::<(), ()>(&mut store, "_start")?;
            start.call(&mut store, ())
        })();
        let elapsed = call_start.elapsed();

        // 停 ticker
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = ticker.join();

        let stdout_bytes = stdout.contents();
        let stderr_bytes = stderr.contents();
        let stdout_str = String::from_utf8_lossy(&stdout_bytes).to_string();
        let mut stderr_str = String::from_utf8_lossy(&stderr_bytes).to_string();

        match outcome {
            // 正常返回(无 proc_exit)→ exit_code 0
            Ok(()) => Ok(CapabilityResult {
                exit_code: 0,
                stdout: stdout_str,
                stderr: stderr_str,
                timed_out: false,
            }),
            Err(e) => {
                // proc_exit(N) 走 I32Exit trap → 透传 N(0/1/2 业务语义)
                if let Some(exit) = e.downcast_ref::<I32Exit>() {
                    return Ok(CapabilityResult {
                        exit_code: exit.0,
                        stdout: stdout_str,
                        stderr: stderr_str,
                        timed_out: false,
                    });
                }
                // epoch 超时 → timed_out / exit_code -1 (wasm-timeout)。
                // 判据(鲁棒,不依赖 wall-clock 时序,避免并行负载下 flaky):
                // epoch ticker 触发的中断在 wasmtime 表现为 `Trap::Interrupt`,
                // 直接 downcast trap code 判定;wall-clock 仅作兜底。
                let msg = e.to_string();
                let is_interrupt_trap = e
                    .downcast_ref::<wasmtime::Trap>()
                    .map(|t| matches!(t, wasmtime::Trap::Interrupt))
                    .unwrap_or(false);
                let is_timeout = is_interrupt_trap
                    || elapsed >= invocation.timeout
                    || msg.contains("interrupt");
                if is_timeout {
                    return Ok(CapabilityResult {
                        exit_code: -1,
                        stdout: stdout_str,
                        stderr: format!("wasm-timeout: {msg}"),
                        timed_out: true,
                    });
                }
                // 其他 trap(unreachable / OOB / StoreLimits OOM)→ exit_code 1 (wasm-trap)
                if stderr_str.is_empty() {
                    stderr_str = format!("wasm-trap: {msg}");
                } else {
                    stderr_str.push_str(&format!("\nwasm-trap: {msg}"));
                }
                Ok(CapabilityResult {
                    exit_code: 1,
                    stdout: stdout_str,
                    stderr: stderr_str,
                    timed_out: false,
                })
            }
        }
    }
}
