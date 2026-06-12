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

/// 离线模式：`HF_HUB_OFFLINE` 置 `1` / `true` / `yes` 时，禁止任何 HuggingFace 网络
/// 下载——只允许命中本地缓存。air-gapped 部署（K3 一体机 / 企业内网）+ 测试套件
/// 用它阻断 `ensure_models` 的阻塞式 `ureq` 下载（一次 setup/unlock 会同步拉 330MB
/// reranker + embedding ONNX，无超时；测试里 9 个并发 server 各拉一份会把 CI 卡到超时）。
/// 沿用 hf-hub 生态既有的 `HF_HUB_OFFLINE` 约定，不另造 attune 专属变量。
pub(crate) fn hf_hub_offline() -> bool {
    std::env::var("HF_HUB_OFFLINE")
        .map(|v| {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

/// 下载客户端的连接超时。对连不上的 endpoint(CN 已死的 hf-mirror.com / 黑洞地址)
/// 用**显式 connect 超时**把"connect 阶段永久 hang"压成有界失败(呼应 9936dca 教训:
/// hf-hub 0.5 的零超时 ureq agent 对死源会阻塞到 TCP 内核超时,可达数分钟)。
const ENDPOINT_PROBE_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 单次模型文件下载的**整体**超时上限。gdb thread-dump(9936dca)实证 hang 发生在 TLS recv
/// **传输中途**(连上后 stall),connect 守卫覆盖不到。reqwest **blocking** ClientBuilder
/// 不暴露 `read_timeout`(仅 async 有),故用 total `.timeout()` 把 worst-case 从"永久"压成
/// 有界:本仓模型 ≤330MB,即便 ~500KB/s 慢网也 < 11min 完成,600s 足够不误杀合法慢下载,
/// 而纯 stall 的死源 600s 内必 `Err` → 有界失败 + graceful degrade,不再永久阻塞 unlock/启动。
const DOWNLOAD_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// 构造一个 HF-resolve 兼容 URL(`{endpoint}/{repo}/resolve/main/{filename}`)并**流式**
/// 下载到 `dst`。endpoint 由调用方**显式**给定 —— S8 selector(`model_source`)解析出的源,
/// 或 failover 链上的次优源。**唯一下载原语**(旧 `download_hf_file` env-默认包装 +
/// `probe_endpoint_reachable` 前哨已被 S8 failover 取代:source selector 的探测做选源,
/// 本函数的 connect+total 超时做单次下载的有界守卫)。
///
/// client 配 `connect_timeout`(死源 connect 守卫)+ total `timeout`(兜 stall-after-connect,
/// 把永久 hang 压成 ≤600s 有界失败;reqwest blocking 无 read_timeout,故用 total)。逐块 copy
/// 到 `.part` 后 atomic rename,半下载不留脏文件。与 hf-hub 同 URL 约定(`{endpoint}/{repo}/
/// resolve/{rev}/{file}`),CN ModelScope /models 路径已含在 endpoint 内。
pub(crate) fn download_hf_file_from(
    endpoint: &str,
    repo_id: &str,
    filename: &str,
    dst: &std::path::Path,
) -> Result<()> {
    let endpoint = endpoint.trim_end_matches('/');
    let url = format!("{endpoint}/{repo_id}/resolve/main/{filename}");
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(ENDPOINT_PROBE_CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_TOTAL_TIMEOUT)
        .build()
        .map_err(|e| VaultError::ModelLoad(format!("build download client: {e}")))?;
    let mut resp = client
        .get(&url)
        .send()
        .map_err(|e| VaultError::ModelLoad(format!("download GET {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(VaultError::ModelLoad(format!(
            "download {url} returned status {}",
            resp.status()
        )));
    }
    let tmp = dst.with_extension("part");
    let mut out = std::fs::File::create(&tmp)
        .map_err(|e| VaultError::ModelLoad(format!("create tmp {}: {e}", tmp.display())))?;
    // copy_to 在 total timeout(DOWNLOAD_TOTAL_TIMEOUT)触发时返回 Err(不会无限阻塞);失败清理 .part。
    resp.copy_to(&mut out).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        VaultError::ModelLoad(format!("stream download {url} → {}: {e}", tmp.display()))
    })?;
    drop(out);
    std::fs::rename(&tmp, dst).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        VaultError::ModelLoad(format!("rename {} → {}: {e}", tmp.display(), dst.display()))
    })?;
    Ok(())
}

/// 确保 model_filename 和 tokenizer_filename 两个文件已缓存在本地
///
/// 若文件不存在则从 HuggingFace Hub 下载（支持 HF_ENDPOINT 环境变量镜像）。
/// `HF_HUB_OFFLINE=1` 时禁止下载，未命中缓存直接返回 `Err`（调用方 graceful degrade）。
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

    // 离线模式：缓存未命中（上面的 early-return 没触发）时，禁止任何网络下载。
    // 在发起任何下载请求之前拦截。
    if hf_hub_offline() {
        return Err(VaultError::ModelLoad(format!(
            "model {repo_id}/{model_filename} not cached and HF_HUB_OFFLINE is set; refusing network download"
        )));
    }

    // S8: 经动态源选择解析 failover 顺序(候选注册表 + 健康/吞吐探测),逐源下载,
    // 任一源失败(网络 / 非 2xx / sha)自动切次优。embedding repo 是 `Xenova/*`,
    // 全源覆盖 → 顺序通常 company-mirror → ModelScope(CN)→ HF。探测只在此显式
    // 下载路径(非请求路径,R3);显式 HF_ENDPOINT 仍由 download_with_failover 尊重。
    // 替代旧的"静态单源 probe + download_hf_file"(S1/C1 的超时守卫已下沉进
    // download_hf_file_from,failover 全程复用)。
    let sources = crate::infer::model_source::resolve_sources_for(repo_id);

    if !model_path.exists() {
        crate::infer::model_source::download_with_failover(
            &sources,
            repo_id,
            model_filename,
            &model_path,
        )?;
    }

    if !tokenizer_path.exists() {
        crate::infer::model_source::download_with_failover(
            &sources,
            repo_id,
            tokenizer_filename,
            &tokenizer_path,
        )?;
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
    fn hf_hub_offline_parses_truthy_values() {
        // Pure parser over an explicit string — no process env mutation (would race
        // with parallel unit tests). The env wiring itself is exercised by the
        // privacy_endpoints_test integration suite, which sets HF_HUB_OFFLINE=1 and
        // must complete in <1s instead of stalling on a 330MB blocking download.
        fn parse(v: &str) -> bool {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes")
        }
        for t in ["1", "true", "TRUE", " yes ", "Yes"] {
            assert!(parse(t), "{t:?} must be truthy");
        }
        for f in ["0", "false", "no", "", "off", "2"] {
            assert!(!parse(f), "{f:?} must be falsy");
        }
    }

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
    fn download_from_black_hole_fails_fast() {
        // 黑洞地址(127.0.0.1:1 — 普通环境无监听 → connect refused/timeout)。
        // 断言下载原语在 connect 超时上限内返回 Err 而非永久 hang(9936dca 守卫)。
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("model.bin");
        let start = std::time::Instant::now();
        let r = download_hf_file_from("http://127.0.0.1:1", "Xenova/bge-m3", "model.bin", &dst);
        let elapsed = start.elapsed();
        assert!(r.is_err(), "black-hole endpoint must return Err, not hang");
        assert!(!dst.exists(), "no file left on failed download");
        assert!(
            elapsed < ENDPOINT_PROBE_CONNECT_TIMEOUT + std::time::Duration::from_secs(5),
            "download must fail-fast within ~{}s, took {:?}",
            ENDPOINT_PROBE_CONNECT_TIMEOUT.as_secs(),
            elapsed
        );
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
