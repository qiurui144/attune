use crate::error::{Result, VaultError};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;

/// 给定 HuggingFace repo_id，返回本地缓存目录路径
/// repo_id 中的 '/' 替换为 '_'，避免目录层级问题
pub fn model_cache_dir(repo_id: &str) -> PathBuf {
    crate::platform::models_dir().join(repo_id.replace('/', "_"))
}

/// 计算文件的 SHA256 十六进制字符串
fn file_sha256(path: &std::path::Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| VaultError::ModelLoad(format!("open file for sha256: {e}")))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| VaultError::ModelLoad(format!("read file for sha256: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// 校验文件完整性：检查 .sha256 伴随文件是否匹配
/// - 无 .sha256 文件：首次，计算并写入，通过
/// - 有 .sha256 文件：比对，不匹配则删除两个文件并返回 Err
fn verify_or_record_sha256(file_path: &std::path::Path) -> Result<()> {
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let sha_path = file_path.with_extension(format!("{ext}.sha256"));
    let actual = file_sha256(file_path)?;
    if sha_path.exists() {
        let expected = std::fs::read_to_string(&sha_path)
            .map_err(|e| VaultError::ModelLoad(format!("read sha256 file: {e}")))?;
        let expected = expected.trim();
        if actual != expected {
            let _ = std::fs::remove_file(file_path);
            let _ = std::fs::remove_file(&sha_path);
            return Err(VaultError::ModelLoad(format!(
                "SHA256 mismatch for {}: expected {expected}, got {actual}; file deleted, re-download required",
                file_path.display()
            )));
        }
    } else {
        // 首次：记录哈希
        std::fs::write(&sha_path, &actual)
            .map_err(|e| VaultError::ModelLoad(format!("write sha256 file: {e}")))?;
    }
    Ok(())
}

/// 确保 model_filename 和 tokenizer_filename 两个文件已缓存在本地
///
/// 若文件不存在则从 HuggingFace Hub 下载（支持 HF_ENDPOINT 环境变量镜像）。
/// 返回 (model_path, tokenizer_path)。
pub fn ensure_models(
    repo_id: &str,
    model_filename: &str,
    tokenizer_filename: &str,
) -> Result<(PathBuf, PathBuf)> {
    let cache_dir = model_cache_dir(repo_id);
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| VaultError::ModelLoad(format!("create model dir: {e}")))?;

    // 取文件名末段（model_filename 可能含路径如 "onnx/model_quantized.onnx"）
    let model_basename = model_filename.rsplit('/').next().unwrap_or(model_filename);
    let tokenizer_basename = tokenizer_filename.rsplit('/').next().unwrap_or(tokenizer_filename);

    let model_path = cache_dir.join(model_basename);
    let tokenizer_path = cache_dir.join(tokenizer_basename);

    if model_path.exists() && tokenizer_path.exists() {
        // 独立校验两个文件，避免短路运算导致一个文件损坏时另一个被跳过
        let model_ok = verify_or_record_sha256(&model_path).is_ok();
        let tokenizer_ok = verify_or_record_sha256(&tokenizer_path).is_ok();
        if model_ok && tokenizer_ok {
            return Ok((model_path, tokenizer_path));
        }
        // 至少一个校验失败（损坏文件已被删除）：继续走下载流程
        log::warn!("model integrity check failed (model_ok={model_ok}, tokenizer_ok={tokenizer_ok}), re-downloading affected files");
    }

    let api = hf_hub::api::sync::Api::new()
        .map_err(|e| VaultError::ModelLoad(format!("hf-hub init: {e}")))?;
    let repo = api.model(repo_id.to_string());

    if !model_path.exists() {
        let src = repo.get(model_filename)
            .map_err(|e| VaultError::ModelLoad(format!("download {model_filename}: {e}")))?;
        std::fs::copy(&src, &model_path)
            .map_err(|e| VaultError::ModelLoad(format!("copy model file: {e}")))?;
    }

    if !tokenizer_path.exists() {
        let src = repo.get(tokenizer_filename)
            .map_err(|e| VaultError::ModelLoad(format!("download {tokenizer_filename}: {e}")))?;
        std::fs::copy(&src, &tokenizer_path)
            .map_err(|e| VaultError::ModelLoad(format!("copy tokenizer file: {e}")))?;
    }

    // 完整性校验（首次写入 .sha256；后续对比）
    verify_or_record_sha256(&model_path)?;
    verify_or_record_sha256(&tokenizer_path)?;

    Ok((model_path, tokenizer_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_cache_dir_for_repo() {
        let dir = model_cache_dir("Qwen/Qwen3-Embedding-0.6B");
        assert!(dir.to_str().unwrap().contains("Qwen_Qwen3-Embedding-0.6B"));
    }

    #[test]
    fn model_cache_dir_replaces_slash() {
        let dir = model_cache_dir("BAAI/bge-reranker-v2-m3");
        let s = dir.to_str().unwrap();
        assert!(!s.contains("BAAI/bge"), "slash should be replaced");
        assert!(s.contains("BAAI_bge-reranker-v2-m3"));
    }

    #[test]
    fn sha256_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.bin");
        std::fs::write(&file_path, b"hello world").unwrap();
        // 首次：写入 sha256
        assert!(verify_or_record_sha256(&file_path).is_ok());
        let sha_path = file_path.with_extension("bin.sha256");
        assert!(sha_path.exists());
        // 第二次：验证通过
        assert!(verify_or_record_sha256(&file_path).is_ok());
        // 篡改文件：验证失败，文件被删除
        std::fs::write(&file_path, b"tampered").unwrap();
        assert!(verify_or_record_sha256(&file_path).is_err());
        assert!(!file_path.exists());
    }
}
