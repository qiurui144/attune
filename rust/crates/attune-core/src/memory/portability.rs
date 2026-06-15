//! memory/portability — 口令加密的全量记忆导出包(合并式跨设备迁移)。
//!
//! WHY portable key 而非 vault DEK:bundle 要跨设备搬运,目标设备的 DEK 不同;
//! 用用户口令经 Argon2id 派生一个与设备无关的 portable key 加密 payload,
//! 任一拿到 bundle + 口令的人都能在新设备解密合并 —— 这正是"可迁移"的代价,
//! 故 export 是全部记忆明文等价(已 DEK 解密 → portable key 重加密)。
//!
//! WHY 参数下限钉死为代码常量(不可由调用方降低):bundle 长期离线存放,
//! 攻击者可离线暴力破解口令;KDF 强度必须 >= vault 在线解锁档(那档为低延迟
//! 调优)。本刀只做 export 半部;import + 合并在 Task 6。
//!
//! 见 `docs/superpowers/plans/2026-06-15-memory-continuity-and-portability.md` Task 5。

use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::Store;

// 钉死的 Argon2id 下限:64 MiB / 3 pass / 1 lane。离线 bundle 比在线解锁更需抗暴破,
// 故高于 crypto.rs 的在线档(19 MiB/t2)。调用方无法降低。
const KDF_M_KIB: u32 = 65_536;
const KDF_T: u32 = 3;
const KDF_P: u32 = 1;
// bundle 内随机 salt 长度(口令派生用,与 vault salt 无关)。
const BUNDLE_SALT_LEN: usize = 16;
const BUNDLE_FORMAT_VERSION: u32 = 1;

/// bundle 头部明文清单:格式版本 + 记忆条数(import 时校验,detect 截断/损坏)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleManifest {
    pub format_version: u32,
    pub memories: usize,
}

/// 一条可移植记忆:summary 明文 + 幂等键(source_chunk_hashes)+ 重建所需的全部字段
/// + 关联向量(model/embedding)。import 用本机 DEK 重加密入库,向量按本机模型 reindex 对齐。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableMemory {
    pub kind: String,
    pub window_start: i64,
    pub window_end: i64,
    pub source_chunk_hashes: Vec<String>,
    pub summary: String,
    pub model: String,
    pub created_at: i64,
    pub topic_key: Option<String>,
    /// 关联 memory_vectors 行(若已 embed);(model, embedding)。
    pub vector: Option<PortableVector>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortableVector {
    pub model: String,
    pub embedding: Vec<f32>,
}

/// import 合并结果计数(Task 6 填充语义;此处定义供跨 task 类型一致)。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportResult {
    pub imported: usize,
    pub merged: usize,
    pub skipped: usize,
}

/// 从口令 + salt 确定性派生 32 字节 portable key。同口令同 salt → 同 key。
fn derive_portable_key(passphrase: &str, salt: &[u8]) -> Result<Key32> {
    let params = Params::new(KDF_M_KIB, KDF_T, KDF_P, Some(32))
        .map_err(|e| VaultError::Crypto(format!("portable kdf params: {e}")))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .map_err(|e| VaultError::Crypto(format!("portable kdf derive: {e}")))?;
    Ok(Key32::from_bytes(out))
}

/// 把当前 vault 的全部记忆收集为可移植结构(summary 已 DEK 解密 → 明文)。
fn collect_portable_memories(store: &Store, dek: &Key32) -> Result<Vec<PortableMemory>> {
    let rows = store.list_recent_memories(dek, usize::MAX)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let vector = store.get_memory_vector(&r.id)?.map(|v| PortableVector {
            model: v.model,
            embedding: v.embedding,
        });
        out.push(PortableMemory {
            kind: r.kind,
            window_start: r.window_start,
            window_end: r.window_end,
            source_chunk_hashes: r.source_chunk_hashes,
            summary: r.summary,
            model: r.model,
            created_at: r.created_at,
            topic_key: r.topic_key,
            vector,
        });
    }
    Ok(out)
}

/// 导出全量记忆为口令加密 bundle。
///
/// 物理格式:`[manifest-json]\n[salt(16B)][ciphertext]`。manifest 明文在头部
/// 供 import 在派生 key 前校验格式版本 + 条数;salt 随机(每次导出不同密文);
/// ciphertext = AES-256-GCM(portable_key, JSON(memories))。
pub fn export_memory_bundle(store: &Store, dek: &Key32, passphrase: &str) -> Result<Vec<u8>> {
    let salt = crypto::generate_salt();
    let salt = &salt[..BUNDLE_SALT_LEN];
    let pkey = derive_portable_key(passphrase, salt)?;

    let mems = collect_portable_memories(store, dek)?;
    let payload = serde_json::to_vec(&mems)?;
    let enc = crypto::encrypt(&pkey, &payload)?;

    let manifest = BundleManifest {
        format_version: BUNDLE_FORMAT_VERSION,
        memories: mems.len(),
    };
    let mut out = serde_json::to_vec(&manifest)?;
    out.push(b'\n');
    out.extend_from_slice(salt);
    out.extend_from_slice(&enc);
    Ok(out)
}

/// 从口令加密 bundle 合并式导入记忆到当前 vault(用本机 DEK 重加密)。
///
/// WHY 合并而非覆盖:跨设备搬运时目标设备可能已有记忆;按 `(kind, source_chunk_hashes)`
/// 幂等去重(底层 `INSERT OR IGNORE` + `uq_memories_source` 唯一索引),已存在的记忆跳过,
/// 重复导入同 bundle 是安全的 no-op。
///
/// WHY 解密在写库之前:口令错 → GCM 认证失败(`InvalidPassword`)在派生 key 解出 payload
/// 阶段就返回 Err,此时尚未触碰任何表 → 零写入(原子拒绝整包)。
/// manifest 条数与解出条数不符同样在写库前拒绝(detect 截断/损坏)。
///
/// 向量按 bundle 内 model 原样写入;后续 reindex(Task 3)会把它对齐到本机 embedding 模型。
pub fn import_memory_bundle(
    store: &Store,
    dek: &Key32,
    bundle: &[u8],
    passphrase: &str,
) -> Result<ImportResult> {
    // 切分 [manifest-json]\n[salt(16B)][ciphertext],严格对称 export_memory_bundle。
    let nl = bundle
        .iter()
        .position(|&b| b == b'\n')
        .ok_or_else(|| VaultError::InvalidInput("corrupt-bundle: no manifest delimiter".into()))?;
    let manifest: BundleManifest = serde_json::from_slice(&bundle[..nl])
        .map_err(|_| VaultError::InvalidInput("unsupported-bundle-version".into()))?;
    if manifest.format_version != BUNDLE_FORMAT_VERSION {
        return Err(VaultError::InvalidInput("unsupported-bundle-version".into()));
    }
    let rest = &bundle[nl + 1..];
    if rest.len() < BUNDLE_SALT_LEN {
        return Err(VaultError::InvalidInput("corrupt-bundle: truncated salt".into()));
    }
    let (salt, ct) = rest.split_at(BUNDLE_SALT_LEN);

    let pkey = derive_portable_key(passphrase, salt)?;
    // 口令错 → crypto::decrypt 返回 VaultError::InvalidPassword(GCM tag 失配),
    // 在任何写库之前 → 零写入。route 层据此映射 400 bad-passphrase。
    let plain = crypto::decrypt(&pkey, ct)?;
    let mems: Vec<PortableMemory> = serde_json::from_slice(&plain)
        .map_err(|_| VaultError::InvalidInput("corrupt-bundle: payload".into()))?;

    // manifest 校验:解出条数须与头部声明一致,否则拒绝整包(写库前)。
    if mems.len() != manifest.memories {
        return Err(VaultError::InvalidInput("corrupt-bundle: count mismatch".into()));
    }

    let now = chrono::Utc::now().timestamp();
    let mut res = ImportResult::default();
    for m in mems {
        // insert_memory 走 INSERT OR IGNORE + uq_memories_source 唯一索引:
        // 返回 1=新增、0=已存在(幂等 skip),无需先 SELECT 检查。
        let affected = store.insert_memory(
            dek,
            &m.kind,
            m.window_start,
            m.window_end,
            &m.source_chunk_hashes,
            &m.summary,
            &m.model,
            m.created_at,
        )?;
        if affected == 0 {
            res.skipped += 1;
            continue;
        }
        res.imported += 1;
        // 若 bundle 带向量,按其 model 写入(后续 reindex 对齐本机模型)。
        // 向量按本机记忆 id 关联:insert_memory 内部新生成 id,故需经 source_chunk_hashes
        // 回查刚插入行的 id。
        if let Some(v) = m.vector {
            if let Some(id) = store.find_memory_id_by_source(&m.kind, &m.source_chunk_hashes)? {
                store.put_memory_vector(&id, &v.embedding, &v.model, now)?;
            }
        }
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unlocked_store_with_n_memories(n: usize) -> (Store, Key32) {
        let store = Store::open_memory().unwrap();
        let dek = Key32::generate();
        for i in 0..n {
            let h = format!("h{i}");
            store
                .insert_memory(
                    &dek,
                    "episodic",
                    1,
                    2,
                    &[h],
                    &format!("summary {i}"),
                    "embed-dim4",
                    i as i64,
                )
                .unwrap();
        }
        (store, dek)
    }

    #[test]
    fn export_produces_nonempty_bundle_with_correct_manifest_count() {
        let (store, dek) = unlocked_store_with_n_memories(5);
        let bundle = export_memory_bundle(&store, &dek, "correct horse battery").unwrap();
        assert!(!bundle.is_empty());

        // 头部 manifest 明文可解析,条数与导出记忆数一致。
        let nl = bundle.iter().position(|&b| b == b'\n').unwrap();
        let manifest: BundleManifest = serde_json::from_slice(&bundle[..nl]).unwrap();
        assert_eq!(manifest.format_version, BUNDLE_FORMAT_VERSION);
        assert_eq!(manifest.memories, 5);
    }

    #[test]
    fn export_layout_is_manifest_nl_salt_ciphertext() {
        let (store, dek) = unlocked_store_with_n_memories(2);
        let bundle = export_memory_bundle(&store, &dek, "pw").unwrap();
        let nl = bundle.iter().position(|&b| b == b'\n').unwrap();
        // \n 之后至少有 salt(16B)+ 非空密文(nonce 12B + GCM tag 16B 起步)。
        let rest = &bundle[nl + 1..];
        assert!(rest.len() > BUNDLE_SALT_LEN + 12 + 16);
    }

    #[test]
    fn derive_portable_key_is_deterministic_for_same_pass_and_salt() {
        let salt = [7u8; BUNDLE_SALT_LEN];
        let k1 = derive_portable_key("same-pass", &salt).unwrap();
        let k2 = derive_portable_key("same-pass", &salt).unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());

        // 口令不同 → key 不同(同 salt)。
        let k3 = derive_portable_key("other-pass", &salt).unwrap();
        assert_ne!(k1.as_bytes(), k3.as_bytes());
    }

    #[test]
    fn export_roundtrips_through_portable_key_decrypt() {
        // export 自洽:用同口令 + bundle 内 salt 派生 key 能解出原 memories
        // (import 完整链在 Task 6;此处只验 export 产物可被对应 key 解密)。
        let (store, dek) = unlocked_store_with_n_memories(3);
        let bundle = export_memory_bundle(&store, &dek, "pw123").unwrap();
        let nl = bundle.iter().position(|&b| b == b'\n').unwrap();
        let rest = &bundle[nl + 1..];
        let (salt, ct) = rest.split_at(BUNDLE_SALT_LEN);
        let pkey = derive_portable_key("pw123", salt).unwrap();
        let plain = crypto::decrypt(&pkey, ct).unwrap();
        let mems: Vec<PortableMemory> = serde_json::from_slice(&plain).unwrap();
        assert_eq!(mems.len(), 3);
        assert!(mems.iter().all(|m| m.summary.starts_with("summary")));
    }

    fn empty_unlocked_store() -> (Store, Key32) {
        (Store::open_memory().unwrap(), Key32::generate())
    }

    #[test]
    fn export_then_import_roundtrip_equivalent() {
        let (store, dek) = unlocked_store_with_n_memories(5);
        let bundle = export_memory_bundle(&store, &dek, "correct horse battery").unwrap();

        // 全新设备(不同 DEK)首次导入:5 条全部落库。
        let (store2, dek2) = empty_unlocked_store();
        let r = import_memory_bundle(&store2, &dek2, &bundle, "correct horse battery").unwrap();
        assert_eq!(r.imported, 5);
        assert_eq!(r.skipped, 0);
        assert_eq!(store2.list_recent_memories(&dek2, 100).unwrap().len(), 5);

        // 二次导入同 bundle:source_chunk_hashes 幂等去重 → 全部 skip,零新增。
        let r2 = import_memory_bundle(&store2, &dek2, &bundle, "correct horse battery").unwrap();
        assert_eq!(r2.imported, 0);
        assert_eq!(r2.skipped, 5);
        assert_eq!(store2.list_recent_memories(&dek2, 100).unwrap().len(), 5);
    }

    #[test]
    fn import_wrong_passphrase_fails_no_write() {
        let (store, dek) = unlocked_store_with_n_memories(2);
        let bundle = export_memory_bundle(&store, &dek, "right").unwrap();

        let (store2, dek2) = empty_unlocked_store();
        // 口令错 → GCM 解密在写库之前就失败,整包拒绝。
        assert!(import_memory_bundle(&store2, &dek2, &bundle, "wrong").is_err());
        assert_eq!(store2.list_recent_memories(&dek2, 100).unwrap().len(), 0);
    }
}
