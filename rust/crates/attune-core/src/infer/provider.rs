use crate::error::{Result, VaultError};
use crate::platform::NpuKind;
use ort::{
    execution_providers::{CPUExecutionProvider, CUDAExecutionProvider},
    session::Session,
};
use std::path::Path;

/// 根据平台检测结果，构建带最优 Execution Provider 的 ort Session
///
/// EP 优先级：CUDA > CPU（其余 EP 通过 ort feature flags 和 NPU_VAULT_EP 环境变量启用）
pub fn build_session(model_path: &Path) -> Result<Session> {
    let npu = crate::platform::detect_npu();

    let session_builder = Session::builder()
        .map_err(|e| VaultError::Crypto(format!("ort Session::builder: {e}")))?;

    let session = match npu {
        NpuKind::Cuda => session_builder
            .with_execution_providers([
                CUDAExecutionProvider::default().build(),
                CPUExecutionProvider::default().build(),
            ])
            .map_err(|e| VaultError::Crypto(format!("ort with_execution_providers: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| VaultError::Crypto(format!("ort commit_from_file: {e}")))?,
        // IntelNpu / IntelIgpu → OpenVINO EP；AmdNpu → DirectML EP
        // 待 ort features 添加 "openvino" / "directml" 后激活
        _ => session_builder
            .with_execution_providers([CPUExecutionProvider::default().build()])
            .map_err(|e| VaultError::Crypto(format!("ort with_execution_providers: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| VaultError::Crypto(format!("ort commit_from_file: {e}")))?
    };

    Ok(session)
}
