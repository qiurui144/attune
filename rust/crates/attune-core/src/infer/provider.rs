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
    let sel = cached_selection();
    let chain = sel.recommend_ep_chain_for(task);
    let telemetry = EpSelectionTelemetry::from_chain(&chain);
    log::debug!(
        "ort EP selection for {}: requested={:?} active~={} (approx)",
        model_path.display(),
        telemetry.requested,
        telemetry.active
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
                out.push(ort::ep::OpenVINO::default().with_device_type(dev.device_type()).build())
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
