use crate::error::{Result, VaultError};
use crate::infer::accel::{cached_selection, EpChoice, EpSelectionTelemetry, InferTask, OpenVinoDevice};
use ort::execution_providers::{CPUExecutionProvider, ExecutionProviderDispatch};
use ort::session::Session;
use std::path::Path;

/// 根据本机硬件检测 + 当前 artifact 编入的 EP,构建带最优 Execution Provider 链的
/// ort Session。
///
/// EP 链由 `infer::accel::recommend_ep_chain()` 给出(首选 → … → **末位永远 CPU**)。
/// 关键不变量:**EP 运行时不可用 → graceful 降级下一个 / CPU,绝不 panic、绝不 error**。
/// ORT `with_execution_providers` 默认即「注册失败静默跳过 → 最终落 CPU」;本函数
/// **明确不用** `.error_on_failure()`(底座推理永远要能跑,加速是 best-effort,§7)。
///
/// 兑现历史 TODO:从 CUDA→CPU 单分支升级为跨厂商 EP 自动选型(CUDA/DirectML/OpenVINO/
/// ROCm/VitisAI/TensorRT,按硬件 × 编入 feature × env override 选)。
pub fn build_session(model_path: &Path) -> Result<Session> {
    build_session_for_task(model_path, InferTask::Generic)
}

/// 任务感知版 `build_session` —— OCR session 必须经此传 `InferTask::Ocr`,触发 per-task
/// EP 规则(Intel 禁 DirectML,实测 CER 202%)。其余底座推理传 `InferTask::Generic`。
///
/// 不变量与 `build_session` 一致:EP 注册失败 graceful 降级,绝不 panic、绝不 error,末位 CPU。
pub fn build_session_for_task(model_path: &Path, task: InferTask) -> Result<Session> {
    init_ort_dylib();
    let sel = cached_selection();
    // 电源感知(L_hw 电源层,session 构造时杠杆):能效约束态(电池/saver/热)把 NPU
    // 重排到链首(bench 能效最优)。AC/Unknown → 性能档不变。注意:EP 在 session 构造时
    // 锁定,运行时电源切换不重建(切 embedding EP 会全库重嵌);运行时由 resource_governor
    // 节流/暂停应对。OCR/ASR 短 session 按需重建即走当时电源态。
    let power = crate::platform::power::probe();
    let chain = sel.recommend_ep_chain_for_power(task, &power);
    let telemetry = EpSelectionTelemetry::from_chain(&chain);
    log::debug!(
        "ort EP selection for {}: requested={:?} active~={} (approx) power={:?}",
        model_path.display(),
        telemetry.requested,
        telemetry.active,
        power.source
    );

    let dispatches = build_ep_dispatches(&chain);

    Session::builder()
        .map_err(|e| VaultError::Crypto(format!("ort Session::builder: {e}")))?
        // 不用 .error_on_failure():加速 EP 注册失败 → ORT 静默跳到下一个 → CPU。
        .with_execution_providers(dispatches)
        .map_err(|e| VaultError::Crypto(format!("ort with_execution_providers: {e}")))?
        .commit_from_file(model_path)
        .map_err(|e| VaultError::Crypto(format!("ort commit_from_file: {e}")))
}

/// load-dynamic 变体:首个 ORT Session 前把 `ORT_DYLIB_PATH` 指向 ep-stacks 里**含 provider**
/// 的 onnxruntime 动态库(openvino 栈的 libonnxruntime),令 OpenVINO/ROCm EP 经 dlopen 生效
/// (EP 随注入的 dylib 走,不随 ort crate feature 走)。幂等(Once)。
///
/// **ort-bundled(默认)= no-op** —— 用 ort build 期 copy 的自包含 onnxruntime,行为零变化。
/// 找不到栈库 → **不 set env** → ort 走默认查找 → 最终 graceful 落 CPU(绝不 panic,§7 不变量)。
/// 已显式设了 `ORT_DYLIB_PATH`(高级用户/测试)→ 尊重不覆盖。
#[cfg(feature = "ort-dynamic")]
fn init_ort_dylib() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        if std::env::var_os("ORT_DYLIB_PATH").is_some() {
            return; // 显式指定优先
        }
        let libnames: &[&str] = {
            #[cfg(target_os = "windows")]
            {
                &["onnxruntime.dll"]
            }
            #[cfg(target_os = "macos")]
            {
                &["libonnxruntime.dylib"]
            }
            #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
            {
                &["libonnxruntime.so"]
            }
        };
        // 已就绪的 EP 栈里找 onnxruntime 动态库(openvino 优先;后续 rocm 同理)。
        for stack in ["openvino", "rocm"] {
            if !crate::infer::stack_installer::probe_stack(stack) {
                continue;
            }
            let dir = crate::infer::stack_installer::stack_dir(stack);
            for lib in libnames {
                let p = dir.join(lib);
                if p.exists() {
                    // ⭐ onnxruntime 的兄弟依赖(OV provider DLL + OV runtime libs)与它**同目录**,
                    // 但 Windows 加载被 dlopen 的 dll 时**不搜索该 dll 自身的目录** → onnxruntime.dll
                    // 找不到 onnxruntime_providers_openvino.dll / openvino*.dll → native crash
                    // (155H 实证:不加此路径 app 启动即崩;加了存活)。故先把栈目录加进 DLL/so 搜索路径。
                    // Linux 的 wheel .so 多带 $ORIGIN rpath 自解析,但加 LD_LIBRARY_PATH 兜底无害。
                    prepend_lib_search_path(&dir);
                    std::env::set_var("ORT_DYLIB_PATH", &p);
                    log::info!("ort-dynamic: ORT_DYLIB_PATH → {} (+lib search path, stack={stack})", p.display());
                    return;
                }
            }
        }
        log::warn!(
            "ort-dynamic: no ep-stack onnxruntime dylib found (openvino/rocm not .ready) → ORT default lookup → likely CPU"
        );
    });
}

/// 把 `dir` 加进当前进程的动态库搜索路径(Windows=PATH / Linux=LD_LIBRARY_PATH),让被
/// dlopen 的 onnxruntime 能解析与它同目录的兄弟依赖(OV provider + OV runtime libs)。幂等。
#[cfg(feature = "ort-dynamic")]
fn prepend_lib_search_path(dir: &std::path::Path) {
    let key = if cfg!(target_os = "windows") { "PATH" } else { "LD_LIBRARY_PATH" };
    let sep = if cfg!(target_os = "windows") { ';' } else { ':' };
    let d = dir.to_string_lossy().to_string();
    let cur = std::env::var(key).unwrap_or_default();
    if cur.split(sep).any(|p| p == d) {
        return; // 已在路径里
    }
    let next = if cur.is_empty() { d } else { format!("{d}{sep}{cur}") };
    std::env::set_var(key, next);
}

/// ort-bundled(默认):no-op —— onnxruntime 由 ort download-binaries 自包含,无需注入。
#[cfg(not(feature = "ort-dynamic"))]
#[inline]
fn init_ort_dylib() {}

/// OpenVINO 编译缓存目录(`models/ov-cache/`)。首次编译量化图的产物落盘,后续 session
/// 加载跳过重编译(155H 实测 iGPU 36s → 缓存命中后 ~1-2s)。建不了目录 → None(不设
/// cache_dir,OV 退化为每次重编译,功能仍可用,只是慢)。
#[cfg(feature = "openvino")]
fn ov_cache_dir() -> Option<String> {
    let dir = crate::platform::models_dir().join("ov-cache");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.to_string_lossy().into_owned())
}

/// 把 `EpChoice` 链 build 成 ORT `ExecutionProviderDispatch` 列表。
///
/// 每个非默认 EP 走 `#[cfg(feature=...)]` 门:未编入该 feature 时不会引用对应 `ort::ep`
/// 类型(default workspace build 不触碰未编 EP 的符号),该 EP 在链里被静默跳过(它本来
/// 也进不了 `compiled_eps()`,这里是双保险)。CPU 永远可 build(兜底不变量)。
fn build_ep_dispatches(chain: &[EpChoice]) -> Vec<ExecutionProviderDispatch> {
    let mut out: Vec<ExecutionProviderDispatch> = Vec::with_capacity(chain.len());
    for ep in chain {
        match ep {
            EpChoice::Cpu => out.push(CPUExecutionProvider::default().build()),

            #[cfg(feature = "cuda")]
            EpChoice::Cuda => out.push(ort::ep::CUDA::default().build()),
            #[cfg(feature = "tensorrt")]
            EpChoice::TensorRt => out.push(ort::ep::TensorRT::default().build()),
            #[cfg(feature = "directml")]
            EpChoice::DirectMl => out.push(ort::ep::DirectML::default().build()),
            #[cfg(feature = "coreml")]
            EpChoice::CoreMl => out.push(ort::ep::CoreML::default().build()),
            #[cfg(feature = "openvino")]
            EpChoice::OpenVino(dev) => {
                // OV 首次编译量化图(bge-m3 INT8 QDQ)昂贵 —— 155H 实测 iGPU 36s / NPU 首推 75s /
                // CPU 10s。无缓存时每次 unlock 都重编译(embedding+reranker 双 session 串行 ~72s),
                // CPU 饱和拖垮 tokio → unlock 超时(崩因 #158-#2 根因:不是崩/挂,是未缓存的慢编译)。
                // cache_dir 把编译产物落盘 → 后续加载秒级。建不了缓存目录则退化为每次编译(仍可用)。
                let mut b = ort::ep::OpenVINO::default().with_device_type(dev.device_type());
                if let Some(cache) = ov_cache_dir() {
                    b = b.with_cache_dir(cache);
                }
                out.push(b.build())
            }
            #[cfg(feature = "rocm")]
            EpChoice::Rocm => out.push(ort::ep::ROCm::default().build()),
            #[cfg(feature = "vitis")]
            EpChoice::VitisAi => out.push(ort::ep::Vitis::default().build()),

            // 未编入对应 feature 的 EP:静默跳过(它不该出现在 chain 里 —— compiled_eps()
            // 已过滤;此分支只是让非默认 build 编得过)。绑定 dev 以消未用警告。
            #[allow(unreachable_patterns)]
            other => {
                let _ = other;
                let _: Option<OpenVinoDevice> = None;
            }
        }
    }
    // 极端兜底:若链 build 后为空(理论不可能,CPU 永远在),补 CPU。
    if out.is_empty() {
        out.push(CPUExecutionProvider::default().build());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infer::accel::EpChoice;

    #[test]
    fn build_dispatches_cpu_only_yields_one() {
        let d = build_ep_dispatches(&[EpChoice::Cpu]);
        assert_eq!(d.len(), 1, "CPU-only chain → 1 dispatch");
    }

    #[test]
    fn build_dispatches_never_empty() {
        // 即便给空链(防御),也至少回一个 CPU dispatch(兜底不变量)。
        let d = build_ep_dispatches(&[]);
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn build_dispatches_real_chain_from_detect() {
        // 真机检测链 → build 不 panic,长度 == 链长(默认 build 下未编 EP 不在链里)。
        let sel = cached_selection();
        let chain = sel.recommend_ep_chain();
        let d = build_ep_dispatches(&chain);
        // CPU 在链末位 → dispatch 至少 1。
        assert!(!d.is_empty());
    }

    #[test]
    fn ocr_task_chain_builds_and_is_directml_free_on_intel_via_provider() {
        use crate::infer::accel::{recommend_ep_chain_for_task, EpChoice, OpenVinoDevice, InferTask};
        use crate::platform::AccelKind;
        // 经 provider 层语义验证:OCR 任务在 Intel 机不会喂 DirectML dispatch。
        let chain = recommend_ep_chain_for_task(
            "windows",
            &[AccelKind::IntelIgpu],
            &[EpChoice::Cpu, EpChoice::DirectMl, EpChoice::OpenVino(OpenVinoDevice::Auto)],
            None,
            InferTask::Ocr,
        );
        assert!(!chain.contains(&EpChoice::DirectMl));
        let d = build_ep_dispatches(&chain);
        assert!(!d.is_empty(), "OCR chain must still build a non-empty dispatch list");
    }

    #[test]
    fn build_session_for_task_generic_equals_default() {
        // build_session 委托 generic 任务;cached chain 一致(纯回归保护,不起真 session)。
        let sel = cached_selection();
        let generic = sel.recommend_ep_chain_for(InferTask::Generic);
        let default = sel.recommend_ep_chain();
        assert_eq!(generic, default);
    }
}
