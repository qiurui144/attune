//! bench→attune 模型选型 catalog —— 把 vlm-llm-bench 实测选型从硬编码迁到数据驱动。
//!
//! **数据流**(spec docs/superpowers/specs/2026-06-20-model-selection-from-bench-drivers.md):
//! ```text
//! vlm-llm-bench (models.yaml + reports) ──export──► model-catalog.yaml (signed)
//!   ──company-mirror──► S8 download_with_failover ──► attune catalog.rs ──► resolve(tier, role)
//! ```
//!
//! **三层兜底(graceful degradation,§7)**:
//! 1. 远程签名 catalog 校验通过 → 用它(优化覆盖层)。
//! 2. 远程拉取失败 / 签名失败 / schema 不支持 → 回退**内置 baseline**(`model-catalog.default.yaml`)。
//! 3. 内置 baseline = attune 现状 freeze(embedding/reranker/ocr/asr 当前选型字节级等价)。
//!
//! catalog **是覆盖层不是硬依赖**:无 catalog 文件 / 全失败 → 行为 = 当前硬编码,老部署零影响。
//!
//! **OCR EP 规则**(任务 1 的数据侧编码):Intel tier OCR `ep: openvino`(实测 Intel
//! DirectML OCR CER 202% 全废);AMD tier OCR `ep: directml`(快 CPU 3.4×)。catalog 与
//! `accel.rs::recommend_ep_chain_for_task(InferTask::Ocr)` 双重保险:前者声明意图,后者强制执行。

use crate::error::{Result, VaultError};
use crate::platform::AccelKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 本 attune 二进制支持的最高 catalog schema 版本。远程 catalog `schema_version` 高于此 → 拒绝
/// (不盲解析未知字段,§10 向后兼容);低于等于 → 接受。
pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// 内置 baseline catalog(编进二进制;离线/首发/远程失败时兜底)。
const DEFAULT_CATALOG_YAML: &str = include_str!("../../assets/model-catalog.default.yaml");

/// company-mirror 上签名 catalog 的 HF-layout repo / 文件约定(经 S8 `download_with_failover`)。
pub const CATALOG_REPO: &str = "attune-catalog";
pub const CATALOG_FILE: &str = "model-catalog.yaml";

/// 实测裁决等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// bench 实测通过(指标达标)。spec §5.1 写大写 `PASS`,兼容大小写。
    #[serde(alias = "PASS", alias = "Pass")]
    Pass,
    /// 测了但指标未全验(如 ASR RTF 测了 CER 未测)。
    #[serde(alias = "MEASURED", alias = "Measured")]
    Measured,
    /// 未实测 —— 可下发但 UI 须标"未校准"(§6.3 不冒充实证)。默认(最保守)。
    #[default]
    #[serde(alias = "PENDING-VERIFY", alias = "pending_verify", alias = "PENDING_VERIFY")]
    PendingVerify,
}

impl Verdict {
    pub fn id(&self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Measured => "measured",
            Verdict::PendingVerify => "pending-verify",
        }
    }
    /// 是否已实测校准(UI 用:false → 标"未校准")。
    pub fn is_calibrated(&self) -> bool {
        matches!(self, Verdict::Pass | Verdict::Measured)
    }
}

/// 模型角色(attune 本地底座 + K3/RK 本地 LLM)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Embedding,
    Rerank,
    Ocr,
    Asr,
    /// 本地 LLM(K3 / RK NPU;非云端网关 token,见 spec §2)。
    Llm,
}

impl Role {
    pub fn id(&self) -> &'static str {
        match self {
            Role::Embedding => "embedding",
            Role::Rerank => "rerank",
            Role::Ocr => "ocr",
            Role::Asr => "asr",
            Role::Llm => "llm",
        }
    }
}

/// 一个 (tier, role) 的选型条目。字段对所有 role 通用 —— 不适用的字段为 `None`/默认。
///
/// 反序列化容忍未知字段(`#[serde(default)]` 全字段),保证远程 catalog 加字段不炸老 client。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelChoice {
    /// HF repo id(embedding/rerank 用;OCR/ASR 引擎自带模型时为空)。
    #[serde(default)]
    pub repo: String,
    /// repo 内 ONNX 文件相对路径。
    #[serde(default)]
    pub file: String,
    /// embedding 维度(仅 embedding role 有意义;0 = 不适用)。
    #[serde(default)]
    pub dims: usize,
    /// 引擎名(OCR: rapidocr/ppocr;ASR: sensevoice/whisper/rk-asr)。
    #[serde(default)]
    pub engine: String,
    /// 具体模型档(ASR: sensevoice-small / whisper-small;LLM: qwen2.5-0.5b)。
    #[serde(default)]
    pub model: String,
    /// Execution Provider 提示(cpu/cuda/directml/openvino/rknn/...)。空 = 由 accel.rs 自动选。
    #[serde(default)]
    pub ep: String,
    /// 实测裁决。
    #[serde(default = "default_verdict")]
    pub verdict: Verdict,
    /// 实测指标摘要(诊断/UI 显示用)。
    #[serde(default)]
    pub metric: String,
    /// 数据来源(reports 文件:行号 或 代码:行号;§6.3 有源)。
    #[serde(default)]
    pub source: String,
}

fn default_verdict() -> Verdict {
    Verdict::PendingVerify
}

/// 单个 tier 的各 role 选型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TierEntry {
    #[serde(default)]
    pub embedding: Option<ModelChoice>,
    #[serde(default)]
    pub rerank: Option<ModelChoice>,
    #[serde(default)]
    pub ocr: Option<ModelChoice>,
    #[serde(default)]
    pub asr: Option<ModelChoice>,
    #[serde(default)]
    pub llm: Option<ModelChoice>,
}

impl TierEntry {
    /// 取某 role 的选型(若该 tier 该 role 缺则 `None`)。
    pub fn role(&self, role: Role) -> Option<&ModelChoice> {
        match role {
            Role::Embedding => self.embedding.as_ref(),
            Role::Rerank => self.rerank.as_ref(),
            Role::Ocr => self.ocr.as_ref(),
            Role::Asr => self.asr.as_ref(),
            Role::Llm => self.llm.as_ref(),
        }
    }
}

/// 完整 catalog(schema 版本 + 各 tier)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub schema_version: u32,
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub harness_version: String,
    #[serde(default)]
    pub source_repo: String,
    /// tier key → 选型;BTreeMap 保证序确定(测试/diff 稳定)。
    #[serde(default)]
    pub tiers: BTreeMap<String, TierEntry>,
}

/// cpu-fallback tier 的稳定 key(任何 tier 缺 role 时回退到这)。
pub const CPU_FALLBACK_TIER: &str = "cpu-fallback";

impl Catalog {
    /// 解析内置 baseline(离线兜底)。**永不 panic** —— baseline 编译期固定且测试守卫合法。
    pub fn builtin_default() -> Self {
        Self::parse(DEFAULT_CATALOG_YAML)
            .expect("built-in model-catalog.default.yaml must be valid (snapshot test guards this)")
    }

    /// 从 YAML 文本解析 + 校验 schema 版本。
    ///
    /// 错误码(kebab,§7):
    /// - `catalog-parse-failed` — YAML 不合法。
    /// - `catalog-schema-unsupported` — schema_version 高于本 attune 支持。
    pub fn parse(yaml: &str) -> Result<Self> {
        let cat: Catalog = serde_yaml::from_str(yaml).map_err(|e| {
            VaultError::ModelLoad(format!("catalog parse: {e}. error-code=catalog-parse-failed"))
        })?;
        if cat.schema_version > SUPPORTED_SCHEMA_VERSION {
            return Err(VaultError::ModelLoad(format!(
                "catalog schema_version {} > supported {}; rejecting (will fall back to built-in). error-code=catalog-schema-unsupported",
                cat.schema_version, SUPPORTED_SCHEMA_VERSION
            )));
        }
        Ok(cat)
    }

    /// 解析远程 catalog,失败时返回内置 baseline(永不让选型层拿不到 catalog)。
    ///
    /// 这是消费侧主入口:remote 拿到字节 → 此函数 → 总有一个合法 Catalog。signature 校验
    /// 由 [`verify_catalog_signature`] 在调用本函数**之前**做(校验失败直接走 builtin,不解析正文)。
    pub fn parse_or_builtin(yaml: &str) -> Self {
        match Self::parse(yaml) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("remote catalog invalid ({e}); using built-in baseline");
                Self::builtin_default()
            }
        }
    }

    /// 选型核心:`resolve(tier, role)`。
    ///
    /// 规则(§7 边界):
    /// 1. tier 命中 ∧ 该 role 存在 → 返回它。
    /// 2. tier 命中但该 role 缺 → 回退 `cpu-fallback` tier 的该 role。
    /// 3. tier 未知 → 直接走 `cpu-fallback`。
    /// 4. 连 cpu-fallback 都没有该 role → `None`(调用方用各 provider 的硬编码默认,§10 兼容)。
    pub fn resolve(&self, tier: &str, role: Role) -> Option<&ModelChoice> {
        if let Some(entry) = self.tiers.get(tier) {
            if let Some(choice) = entry.role(role) {
                return Some(choice);
            }
        }
        // 回退 cpu-fallback。
        self.tiers.get(CPU_FALLBACK_TIER).and_then(|e| e.role(role))
    }

    /// resolve 并标注实际命中的 tier(诊断/REST `/ai-stack/catalog` 用)。
    pub fn resolve_with_tier(&self, tier: &str, role: Role) -> Option<(String, &ModelChoice)> {
        if let Some(entry) = self.tiers.get(tier) {
            if let Some(choice) = entry.role(role) {
                return Some((tier.to_string(), choice));
            }
        }
        self.tiers
            .get(CPU_FALLBACK_TIER)
            .and_then(|e| e.role(role))
            .map(|c| (CPU_FALLBACK_TIER.to_string(), c))
    }
}

/// 把检测到的硬件 + OS 映射到 catalog tier key。
///
/// 与 `accel.rs` 的 `AccelKind` 对齐。优先级:NPU/GPU 厂商 tier > cpu-fallback。
/// 注:这是**选型 tier**(决定模型 repo/engine),与 `accel.rs` 的 **EP tier**(决定 ORT EP)
/// 互补 —— catalog 给 repo+ep hint,accel.rs 把 ep hint 落成实际 EP 链(并施加 OCR per-task 规则)。
pub fn tier_for_hardware(os: &str, hardware: &[AccelKind]) -> String {
    let has = |k: AccelKind| hardware.contains(&k);
    let windows = os == "windows";

    // NVIDIA dGPU(任意 OS)。
    if has(AccelKind::NvidiaGpu) {
        return "nvidia-cuda".to_string();
    }
    // AMD Ryzen AI(Win):NPU 或 RDNA iGPU/GPU。
    if windows && (has(AccelKind::AmdNpu) || has(AccelKind::AmdGpu)) {
        return "amd-win".to_string();
    }
    // Intel Core Ultra(Win):NPU 或 Arc/Iris iGPU。
    if windows && (has(AccelKind::IntelNpu) || has(AccelKind::IntelIgpu)) {
        return "intel-win".to_string();
    }
    // 其余(Linux x86 无 dGPU / 纯 CPU / 未识别加速)→ cpu-fallback。
    // riscv-k3 / rk1820 / rk3588 tier 由专用部署路径显式指定(非通用硬件探测),不在此自动派生。
    CPU_FALLBACK_TIER.to_string()
}

/// 用当前进程探测的硬件派生 tier(便捷封装)。
pub fn current_tier() -> String {
    let sel = crate::infer::accel::cached_selection();
    tier_for_hardware(sel.os, &sel.hardware)
}

/// catalog 签名校验(R4 信任域)。
///
/// **R4 决策(spec §11 R4,PENDING 用户拍板)**:catalog 复用 attune 现有 ed25519 信任链
/// **机制**,但应使用**独立的 catalog 信任锚**(不并入 plugin/entitlement anchor —— 与
/// entitlement 快照纪律一致:不同信任域不共用 key)。当前**尚无 catalog 专用公钥下发**,
/// 故:
/// - `CATALOG_PUBLIC_KEYS` 为空 → 任何远程 catalog 都**校验失败 → 回退内置 baseline**
///   (= 现状 freeze + OCR 修)。这是安全默认:无锚则不信任远程,绝不静默接受未签名 catalog。
/// - 待用户拍板 catalog 信任锚后,填入此列表即激活远程覆盖(无需改其他代码)。
///
/// 错误码:`catalog-sig-invalid`。
///
/// TODO(用户拍板 R4):确定 catalog 信任锚 = 独立 key(推荐)还是复用 entitlement key;
/// 定后填 `CATALOG_PUBLIC_KEYS` + 在 cloud 侧 catalog 签名端点签发。
pub const CATALOG_PUBLIC_KEYS: &[&str] = &[];

/// 校验 catalog 字节 + detached 签名(base64 ed25519,对 sha256(catalog_bytes))。
///
/// 返回 `Ok(())` 仅当某个 `CATALOG_PUBLIC_KEYS` 验签通过。空锚列表 → 永远 `Err`(安全默认)。
pub fn verify_catalog_signature(catalog_bytes: &[u8], signature_b64: &str) -> Result<()> {
    use base64::Engine as _;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    use sha2::{Digest, Sha256};

    if CATALOG_PUBLIC_KEYS.is_empty() {
        return Err(VaultError::ModelLoad(
            "no catalog trust anchor configured; remote catalog not trusted (using built-in). error-code=catalog-sig-invalid".to_string(),
        ));
    }

    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(signature_b64.trim())
        .map_err(|e| {
            VaultError::ModelLoad(format!("catalog sig base64 decode: {e}. error-code=catalog-sig-invalid"))
        })?;
    let sig_arr: [u8; 64] = sig_bytes.as_slice().try_into().map_err(|_| {
        VaultError::ModelLoad("catalog sig not 64 bytes. error-code=catalog-sig-invalid".to_string())
    })?;
    let signature = Signature::from_bytes(&sig_arr);

    let digest = Sha256::digest(catalog_bytes);

    for key_hex in CATALOG_PUBLIC_KEYS {
        let Ok(key_bytes) = hex::decode(key_hex) else { continue };
        let Ok(key_arr): std::result::Result<[u8; 32], _> = key_bytes.as_slice().try_into() else {
            continue;
        };
        let Ok(vk) = VerifyingKey::from_bytes(&key_arr) else { continue };
        if vk.verify(&digest, &signature).is_ok() {
            return Ok(());
        }
    }
    Err(VaultError::ModelLoad(
        "catalog signature did not match any trust anchor. error-code=catalog-sig-invalid".to_string(),
    ))
}

/// 经 S8 `download_with_failover` 拉取签名 catalog,校验通过 → 解析覆盖;否则回退内置 baseline。
///
/// **不进请求路径**(§7 R3):仅 pre-flight / 显式 `/ai-stack/catalog/refresh` 触发。
/// `dst` / `sig_dst` 是缓存落盘路径(`~/.local/share/attune/catalog/`)。
///
/// 流程:① 拉 catalog.yaml + catalog.yaml.sig(S8 failover)② verify_catalog_signature
/// ③ 通过 → `Catalog::parse_or_builtin` ④ 任一步失败 → `Catalog::builtin_default`。
pub fn fetch_remote_or_builtin(
    sources: &[crate::infer::model_source::ModelSource],
    dst: &std::path::Path,
    sig_dst: &std::path::Path,
) -> Catalog {
    // ① catalog 正文。
    if let Err(e) = crate::infer::model_source::download_with_failover(sources, CATALOG_REPO, CATALOG_FILE, dst) {
        log::warn!("catalog fetch failed ({e}); using built-in baseline. error-code=catalog-fetch-failed");
        return Catalog::builtin_default();
    }
    // ② 签名文件。
    let sig_file = format!("{CATALOG_FILE}.sig");
    if let Err(e) = crate::infer::model_source::download_with_failover(sources, CATALOG_REPO, &sig_file, sig_dst) {
        log::warn!("catalog signature fetch failed ({e}); using built-in baseline. error-code=catalog-fetch-failed");
        return Catalog::builtin_default();
    }
    // ③ 读 + 验签。
    let (Ok(body), Ok(sig)) = (std::fs::read(dst), std::fs::read_to_string(sig_dst)) else {
        log::warn!("catalog cache read failed; using built-in baseline");
        return Catalog::builtin_default();
    };
    if let Err(e) = verify_catalog_signature(&body, &sig) {
        log::warn!("catalog signature rejected ({e}); using built-in baseline");
        return Catalog::builtin_default();
    }
    // ④ 验签通过 → 解析(解析失败仍回退 builtin)。
    match std::str::from_utf8(&body) {
        Ok(text) => Catalog::parse_or_builtin(text),
        Err(_) => {
            log::warn!("catalog body not utf-8; using built-in baseline");
            Catalog::builtin_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── golden:内置 baseline resolve 每 tier × role 返回期望 ─────────────

    #[test]
    fn builtin_default_parses() {
        let c = Catalog::builtin_default();
        assert_eq!(c.schema_version, 1);
        assert!(c.tiers.contains_key("amd-win"));
        assert!(c.tiers.contains_key("intel-win"));
        assert!(c.tiers.contains_key(CPU_FALLBACK_TIER));
    }

    #[test]
    fn amd_ocr_uses_directml() {
        let c = Catalog::builtin_default();
        let ocr = c.resolve("amd-win", Role::Ocr).unwrap();
        assert_eq!(ocr.ep, "directml", "AMD OCR uses DirectML (3.4x faster, bench:32)");
        assert_eq!(ocr.verdict, Verdict::Pass);
        assert!(!ocr.source.is_empty(), "every entry must cite a source (§6.3)");
    }

    #[test]
    fn intel_ocr_uses_openvino_not_directml() {
        let c = Catalog::builtin_default();
        let ocr = c.resolve("intel-win", Role::Ocr).unwrap();
        assert_eq!(ocr.ep, "openvino", "Intel OCR MUST be OpenVINO (DirectML CER 202%, bench:33-34)");
        assert_ne!(ocr.ep, "directml");
        assert!(ocr.source.contains("33-34"));
    }

    #[test]
    fn amd_intel_embedding_qwen3() {
        let c = Catalog::builtin_default();
        assert_eq!(c.resolve("amd-win", Role::Embedding).unwrap().repo, "Xenova/qwen3-embedding-0.6b");
        assert_eq!(c.resolve("intel-win", Role::Embedding).unwrap().repo, "Xenova/qwen3-embedding-0.6b");
    }

    #[test]
    fn cpu_fallback_freezes_current_attune_defaults() {
        let c = Catalog::builtin_default();
        // §10 migration:cpu-fallback = 现状 freeze。
        assert_eq!(c.resolve(CPU_FALLBACK_TIER, Role::Embedding).unwrap().repo, "Xenova/bge-m3");
        assert_eq!(c.resolve(CPU_FALLBACK_TIER, Role::Rerank).unwrap().repo, "Xenova/bge-reranker-base");
        assert_eq!(c.resolve(CPU_FALLBACK_TIER, Role::Asr).unwrap().engine, "whisper");
    }

    #[test]
    fn amd_intel_asr_sensevoice_cpu_whisper() {
        let c = Catalog::builtin_default();
        assert_eq!(c.resolve("amd-win", Role::Asr).unwrap().engine, "sensevoice");
        assert_eq!(c.resolve("intel-win", Role::Asr).unwrap().engine, "sensevoice");
        // CPU tier 保留 whisper(sensevoice CPU FAIL CER 23%)。
        assert_eq!(c.resolve(CPU_FALLBACK_TIER, Role::Asr).unwrap().engine, "whisper");
    }

    #[test]
    fn npu_tiers_present_with_verdicts() {
        let c = Catalog::builtin_default();
        assert_eq!(c.resolve("riscv-k3", Role::Llm).unwrap().model, "qwen2.5-0.5b");
        assert_eq!(c.resolve("rk1820-npu", Role::Llm).unwrap().verdict, Verdict::Pass);
        assert_eq!(c.resolve("rk3588-rknpu", Role::Embedding).unwrap().repo, "minicpm-embed-rk3588");
    }

    // ── K3 调度层集成 (2026-06-22):riscv-k3 本地能力经 k3-scheduler :8090 收口 ──

    /// K3 一体机:embedding/rerank/ocr/asr 全部 resolve 到 ep=k3-scheduler 哨兵
    /// (经 :8090 服务,预置不下载)。这是「:8090 统一收口」的 catalog 兑现。
    #[test]
    fn k3_local_capabilities_route_to_scheduler() {
        let c = Catalog::builtin_default();
        for role in [Role::Embedding, Role::Rerank, Role::Ocr, Role::Asr] {
            let (hit_tier, choice) = c
                .resolve_with_tier("riscv-k3", role)
                .unwrap_or_else(|| panic!("riscv-k3 must have {} entry", role.id()));
            assert_eq!(hit_tier, "riscv-k3", "{} must hit riscv-k3 (not cpu-fallback)", role.id());
            assert_eq!(
                choice.ep, "k3-scheduler",
                "{} on K3 must route to k3-scheduler :8090 sentinel EP",
                role.id()
            );
        }
        // LLM 仍在(本地慢 LLM 可选);云端默认由 settings 决定,不在 catalog。
        assert_eq!(c.resolve("riscv-k3", Role::Llm).unwrap().model, "qwen2.5-0.5b");
    }

    /// k3-scheduler 哨兵条目无 HF 下载(repo/file 空)—— 预置模型,wizard 跳过下载步。
    #[test]
    fn k3_scheduler_entries_have_no_hf_download() {
        let c = Catalog::builtin_default();
        let emb = c.resolve("riscv-k3", Role::Embedding).unwrap();
        assert!(emb.repo.is_empty(), "k3-scheduler embedding has no HF repo (served via :8090)");
        assert!(emb.file.is_empty(), "k3-scheduler embedding has no HF file");
    }

    // ── 边界:缺 tier / 缺 role / 回退 cpu-fallback ─────────────────────

    #[test]
    fn unknown_tier_falls_back_to_cpu() {
        let c = Catalog::builtin_default();
        // 未知 tier → cpu-fallback。
        let e = c.resolve("does-not-exist", Role::Embedding).unwrap();
        assert_eq!(e.repo, "Xenova/bge-m3");
    }

    #[test]
    fn tier_missing_role_falls_back_to_cpu() {
        let c = Catalog::builtin_default();
        // rk3588-rknpu 只有 embedding,无 llm → 回退 cpu-fallback。
        // (cpu-fallback 也没有 llm → resolve(llm)=None,验在 missing_role_everywhere_returns_none。
        //  此处验"缺 role 回退到 cpu-fallback 的 role"用 rk1820-npu 缺 embedding 的场景。)
        let e = c.resolve("rk1820-npu", Role::Embedding).unwrap();
        assert_eq!(e.repo, "Xenova/bge-m3", "rk1820-npu 无 embedding → cpu-fallback bge-m3");
    }

    #[test]
    fn resolve_with_tier_reports_fallback() {
        let c = Catalog::builtin_default();
        // rk3588-rknpu 无 OCR → 回退 cpu-fallback(riscv-k3 现已有自己的 OCR,改用 rk3588)。
        let (hit_tier, _) = c.resolve_with_tier("rk3588-rknpu", Role::Ocr).unwrap();
        assert_eq!(hit_tier, CPU_FALLBACK_TIER, "rk3588 has no OCR → reports cpu-fallback");
        let (hit_tier2, _) = c.resolve_with_tier("amd-win", Role::Ocr).unwrap();
        assert_eq!(hit_tier2, "amd-win");
    }

    #[test]
    fn missing_role_everywhere_returns_none() {
        // 一个最小 catalog,cpu-fallback 也没有 llm → resolve(llm) = None。
        let yaml = "schema_version: 1\ntiers:\n  cpu-fallback:\n    embedding: {repo: x, ep: cpu}\n";
        let c = Catalog::parse(yaml).unwrap();
        assert!(c.resolve("cpu-fallback", Role::Llm).is_none());
        assert!(c.resolve("whatever", Role::Llm).is_none());
    }

    // ── 异常 / 错误:畸形 YAML / 不支持 schema ──────────────────────────

    #[test]
    fn malformed_yaml_errors() {
        let r = Catalog::parse("this: is: not: valid: yaml: : :");
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("catalog-parse-failed"));
    }

    #[test]
    fn unsupported_schema_version_rejected() {
        let yaml = "schema_version: 999\ntiers: {}\n";
        let r = Catalog::parse(yaml);
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("catalog-schema-unsupported"));
    }

    #[test]
    fn parse_or_builtin_recovers_from_garbage() {
        // 畸形远程 catalog → 回退内置 baseline(不 panic,不丢选型)。
        let c = Catalog::parse_or_builtin("{{{garbage");
        assert_eq!(c.resolve("amd-win", Role::Ocr).unwrap().ep, "directml");
    }

    #[test]
    fn parse_or_builtin_recovers_from_future_schema() {
        let c = Catalog::parse_or_builtin("schema_version: 42\ntiers: {}\n");
        // 回退 builtin → 仍有 intel-win OCR openvino。
        assert_eq!(c.resolve("intel-win", Role::Ocr).unwrap().ep, "openvino");
    }

    // ── 签名失败回退(R4 信任域) ────────────────────────────────────────

    #[test]
    fn empty_anchor_rejects_any_signature() {
        // R4 安全默认:无 catalog 信任锚 → 任何远程 catalog 验签失败。
        assert!(CATALOG_PUBLIC_KEYS.is_empty(), "no catalog anchor configured yet (R4 PENDING)");
        let r = verify_catalog_signature(b"any catalog body", "AAAA");
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("catalog-sig-invalid"));
    }

    #[test]
    fn bad_signature_format_rejected() {
        let r = verify_catalog_signature(b"body", "not-base64-!!!");
        assert!(r.is_err());
    }

    // ── 兼容:lenient parse(远程加未知字段不炸) ───────────────────────

    #[test]
    fn unknown_fields_tolerated() {
        let yaml = "schema_version: 1\nfuture_top_field: 1\ntiers:\n  amd-win:\n    ocr: {engine: rapidocr, ep: directml, future_field: hi, verdict: pass, source: x}\n";
        let c = Catalog::parse(yaml).unwrap();
        assert_eq!(c.resolve("amd-win", Role::Ocr).unwrap().ep, "directml");
    }

    #[test]
    fn missing_verdict_defaults_pending_verify() {
        let yaml = "schema_version: 1\ntiers:\n  amd-win:\n    ocr: {engine: rapidocr, ep: directml}\n";
        let c = Catalog::parse(yaml).unwrap();
        assert_eq!(c.resolve("amd-win", Role::Ocr).unwrap().verdict, Verdict::PendingVerify);
    }

    // ── tier 派生 ───────────────────────────────────────────────────────

    #[test]
    fn tier_derivation_maps_hardware() {
        assert_eq!(tier_for_hardware("windows", &[AccelKind::AmdNpu]), "amd-win");
        assert_eq!(tier_for_hardware("windows", &[AccelKind::AmdGpu]), "amd-win");
        assert_eq!(tier_for_hardware("windows", &[AccelKind::IntelIgpu]), "intel-win");
        assert_eq!(tier_for_hardware("windows", &[AccelKind::IntelNpu]), "intel-win");
        assert_eq!(tier_for_hardware("linux", &[AccelKind::NvidiaGpu]), "nvidia-cuda");
        assert_eq!(tier_for_hardware("windows", &[AccelKind::NvidiaGpu]), "nvidia-cuda");
        // 无加速 / Linux Intel iGPU(无专用 Linux tier)→ cpu-fallback。
        assert_eq!(tier_for_hardware("linux", &[]), CPU_FALLBACK_TIER);
        assert_eq!(tier_for_hardware("linux", &[AccelKind::Cpu]), CPU_FALLBACK_TIER);
    }

    #[test]
    fn nvidia_wins_over_other_accel() {
        // 同机 NVIDIA + Intel iGPU → NVIDIA tier(优先级)。
        assert_eq!(
            tier_for_hardware("windows", &[AccelKind::NvidiaGpu, AccelKind::IntelIgpu]),
            "nvidia-cuda"
        );
    }

    // ── 回退一致性:每个 entry 有 source(§6.3 数据有源) ──────────────

    #[test]
    fn every_entry_has_source_and_verdict() {
        let c = Catalog::builtin_default();
        for (tier, entry) in &c.tiers {
            for role in [Role::Embedding, Role::Rerank, Role::Ocr, Role::Asr, Role::Llm] {
                if let Some(choice) = entry.role(role) {
                    assert!(
                        !choice.source.is_empty(),
                        "tier={tier} role={} must cite a source (§6.3)",
                        role.id()
                    );
                }
            }
        }
    }
}
