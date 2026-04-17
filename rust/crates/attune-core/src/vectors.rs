// npu-vault/crates/vault-core/src/vectors.rs

use std::collections::HashMap;
use std::path::Path;
use usearch::ffi::{IndexOptions, MetricKind, ScalarKind};

use crate::error::{Result, VaultError};

/// 向量元数据
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VectorMeta {
    pub item_id: String,
    pub chunk_idx: usize,
    pub level: u8,        // 1=章节, 2=段落
    pub section_idx: usize,
}

/// usearch 向量索引封装
pub struct VectorIndex {
    index: usearch::Index,
    meta: HashMap<u64, VectorMeta>,
    next_key: u64,
    dims: usize,
}

impl VectorIndex {
    pub fn new(dims: usize) -> Result<Self> {
        let options = IndexOptions {
            dimensions: dims,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F16,
            ..Default::default()
        };
        let index = usearch::new_index(&options)
            .map_err(|e| VaultError::Crypto(format!("usearch init: {e}")))?;
        index.reserve(10000)
            .map_err(|e| VaultError::Crypto(format!("usearch reserve: {e}")))?;
        Ok(Self { index, meta: HashMap::new(), next_key: 0, dims })
    }

    /// 添加向量
    pub fn add(&mut self, vector: &[f32], meta: VectorMeta) -> Result<u64> {
        if vector.len() != self.dims {
            return Err(VaultError::Crypto(format!(
                "vector dims mismatch: expected {}, got {}", self.dims, vector.len()
            )));
        }
        let key = self.next_key;
        self.next_key += 1;
        self.index.add(key, vector)
            .map_err(|e| VaultError::Crypto(format!("usearch add: {e}")))?;
        self.meta.insert(key, meta);
        Ok(key)
    }

    /// 搜索最相似向量
    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<(VectorMeta, f32)>> {
        if self.index.size() == 0 {
            return Ok(vec![]);
        }
        let results = self.index.search(query, top_k)
            .map_err(|e| VaultError::Crypto(format!("usearch search: {e}")))?;

        let mut output = Vec::new();
        for i in 0..results.keys.len() {
            let key = results.keys[i];
            let distance = results.distances[i];
            if let Some(meta) = self.meta.get(&key) {
                // cosine distance → cosine similarity
                let score = 1.0 - distance;
                output.push((meta.clone(), score));
            }
        }
        Ok(output)
    }

    /// 按 item_id 删除所有向量
    pub fn delete_by_item_id(&mut self, item_id: &str) -> Result<usize> {
        let keys_to_remove: Vec<u64> = self.meta.iter()
            .filter(|(_, m)| m.item_id == item_id)
            .map(|(k, _)| *k)
            .collect();
        let count = keys_to_remove.len();
        for key in &keys_to_remove {
            self.index.remove(*key)
                .map_err(|e| VaultError::Crypto(format!("usearch remove: {e}")))?;
            self.meta.remove(key);
        }
        Ok(count)
    }

    pub fn len(&self) -> usize {
        self.index.size()
    }

    pub fn is_empty(&self) -> bool {
        self.index.size() == 0
    }

    /// 按 item_id 取出所有 chunk 向量，返回均值向量（用于 reranking）
    ///
    /// 若该 item 不存在任何向量或 usearch get() 失败则返回 None。
    pub fn get_vector(&self, item_id: &str) -> Option<Vec<f32>> {
        if item_id.is_empty() { return None; }
        let keys: Vec<u64> = self.meta.iter()
            .filter(|(_, m)| m.item_id == item_id)
            .map(|(k, _)| *k)
            .collect();

        if keys.is_empty() {
            return None;
        }

        let mut sum = vec![0.0f32; self.dims];
        let mut count = 0usize;

        for key in &keys {
            let mut buf = vec![0.0f32; self.dims];
            if let Ok(n) = self.index.get(*key, &mut buf) {
                if n > 0 {
                    for (s, v) in sum.iter_mut().zip(buf.iter()) {
                        *s += v;
                    }
                    count += 1;
                }
            }
        }

        if count == 0 {
            return None;
        }

        let inv = 1.0 / count as f32;
        Some(sum.into_iter().map(|v| v * inv).collect())
    }

    /// 保存索引到文件
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let path_str = path.to_str()
            .ok_or_else(|| VaultError::Crypto("non-UTF8 path in save".into()))?;
        self.index.save(path_str)
            .map_err(|e| VaultError::Crypto(format!("usearch save: {e}")))?;
        // 保存 meta
        let meta_path = path.with_extension("meta.json");
        let meta_data = serde_json::to_vec(&self.meta)?;
        std::fs::write(&meta_path, meta_data)?;
        // 保存 next_key
        let key_path = path.with_extension("nextkey");
        std::fs::write(&key_path, self.next_key.to_le_bytes())?;
        Ok(())
    }

    /// 从文件加载索引
    pub fn load(path: &Path, dims: usize) -> Result<Self> {
        let options = IndexOptions {
            dimensions: dims,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F16,
            ..Default::default()
        };
        let index = usearch::new_index(&options)
            .map_err(|e| VaultError::Crypto(format!("usearch init: {e}")))?;
        let path_str = path.to_str()
            .ok_or_else(|| VaultError::Crypto("non-UTF8 path in load".into()))?;
        index.load(path_str)
            .map_err(|e| VaultError::Crypto(format!("usearch load: {e}")))?;

        let meta_path = path.with_extension("meta.json");
        let meta: HashMap<u64, VectorMeta> = if meta_path.exists() {
            let data = std::fs::read(&meta_path)?;
            serde_json::from_slice(&data)?
        } else {
            HashMap::new()
        };

        let key_path = path.with_extension("nextkey");
        let next_key = if key_path.exists() {
            let bytes = std::fs::read(&key_path)?;
            if bytes.len() == 8 {
                u64::from_le_bytes(bytes.try_into().unwrap())
            } else { meta.len() as u64 }
        } else { meta.len() as u64 };

        Ok(Self { index, meta, next_key, dims })
    }

    /// 保存到加密文件：save 到临时目录 → 打包 main/meta/nextkey → 加密写入目标路径
    pub fn save_encrypted(&self, key: &crate::crypto::Key32, target: &Path) -> Result<()> {
        if self.is_empty() {
            // 空索引：仅写标记
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            crate::crypto::save_encrypted_file(key, target, b"empty")?;
            return Ok(());
        }

        let tmp = tempfile::TempDir::new()?;
        let tmp_path = tmp.path().join("vectors.tmp");
        self.save(&tmp_path)?;

        let main_bytes = std::fs::read(&tmp_path)?;
        let meta_path = tmp_path.with_extension("meta.json");
        let meta_bytes = std::fs::read(&meta_path).unwrap_or_default();
        let key_path = tmp_path.with_extension("nextkey");
        let key_bytes = std::fs::read(&key_path).unwrap_or_default();

        let mut packed = Vec::new();
        packed.extend_from_slice(&(main_bytes.len() as u64).to_le_bytes());
        packed.extend_from_slice(&main_bytes);
        packed.extend_from_slice(&(meta_bytes.len() as u64).to_le_bytes());
        packed.extend_from_slice(&meta_bytes);
        packed.extend_from_slice(&(key_bytes.len() as u64).to_le_bytes());
        packed.extend_from_slice(&key_bytes);

        crate::crypto::save_encrypted_file(key, target, &packed)?;
        Ok(())
    }

    /// 从加密文件加载: 解密 → 解包 → 分发到临时文件 → 调用 load
    pub fn load_encrypted(key: &crate::crypto::Key32, target: &Path, dims: usize) -> Result<Self> {
        let bytes = match crate::crypto::load_encrypted_file(key, target)? {
            Some(b) => b,
            None => return Self::new(dims),
        };

        if bytes == b"empty" {
            return Self::new(dims);
        }

        if bytes.len() < 24 {
            return Err(VaultError::Crypto("vectors file too short".into()));
        }
        let main_len = u64::from_le_bytes(bytes[0..8].try_into().unwrap()) as usize;
        let mut offset = 8;
        if bytes.len() < offset + main_len + 8 {
            return Err(VaultError::Crypto("vectors file truncated".into()));
        }
        let main_bytes = &bytes[offset..offset + main_len];
        offset += main_len;

        let meta_len = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()) as usize;
        offset += 8;
        if bytes.len() < offset + meta_len + 8 {
            return Err(VaultError::Crypto("vectors file truncated".into()));
        }
        let meta_bytes = &bytes[offset..offset + meta_len];
        offset += meta_len;

        let key_len = u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()) as usize;
        offset += 8;
        if bytes.len() < offset + key_len {
            return Err(VaultError::Crypto("vectors file truncated".into()));
        }
        let key_bytes = &bytes[offset..offset + key_len];

        let tmp = tempfile::TempDir::new()?;
        let tmp_main = tmp.path().join("vectors.tmp");
        std::fs::write(&tmp_main, main_bytes)?;
        if !meta_bytes.is_empty() {
            std::fs::write(tmp_main.with_extension("meta.json"), meta_bytes)?;
        }
        if !key_bytes.is_empty() {
            std::fs::write(tmp_main.with_extension("nextkey"), key_bytes)?;
        }

        Self::load(&tmp_main, dims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_vector(dims: usize) -> Vec<f32> {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        (0..dims).map(|_| rng.gen::<f32>()).collect()
    }

    #[test]
    fn create_index() {
        let idx = VectorIndex::new(1024).unwrap();
        assert_eq!(idx.len(), 0);
        assert!(idx.is_empty());
    }

    #[test]
    fn add_and_search() {
        let mut idx = VectorIndex::new(4).unwrap();
        let v1 = vec![1.0, 0.0, 0.0, 0.0];
        let v2 = vec![0.0, 1.0, 0.0, 0.0];

        idx.add(&v1, VectorMeta { item_id: "a".into(), chunk_idx: 0, level: 2, section_idx: 0 }).unwrap();
        idx.add(&v2, VectorMeta { item_id: "b".into(), chunk_idx: 0, level: 2, section_idx: 0 }).unwrap();

        let results = idx.search(&v1, 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0.item_id, "a", "Closest should be identical vector");
    }

    #[test]
    fn delete_by_item_id() {
        let mut idx = VectorIndex::new(4).unwrap();
        let v = vec![1.0, 0.0, 0.0, 0.0];
        idx.add(&v, VectorMeta { item_id: "x".into(), chunk_idx: 0, level: 1, section_idx: 0 }).unwrap();
        idx.add(&v, VectorMeta { item_id: "x".into(), chunk_idx: 1, level: 2, section_idx: 0 }).unwrap();
        idx.add(&v, VectorMeta { item_id: "y".into(), chunk_idx: 0, level: 2, section_idx: 0 }).unwrap();
        assert_eq!(idx.len(), 3);

        let removed = idx.delete_by_item_id("x").unwrap();
        assert_eq!(removed, 2);
        assert_eq!(idx.len(), 1);
    }

    #[test]
    fn save_and_load() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("vectors.usearch");

        let mut idx = VectorIndex::new(4).unwrap();
        idx.add(&[1.0, 0.0, 0.0, 0.0], VectorMeta {
            item_id: "id1".into(), chunk_idx: 0, level: 2, section_idx: 0
        }).unwrap();
        idx.save(&path).unwrap();

        let loaded = VectorIndex::load(&path, 4).unwrap();
        assert_eq!(loaded.len(), 1);
        let results = loaded.search(&[1.0, 0.0, 0.0, 0.0], 1).unwrap();
        assert_eq!(results[0].0.item_id, "id1");
    }

    #[test]
    fn save_load_encrypted_roundtrip() {
        use crate::crypto::Key32;
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("vectors.enc");
        let key = Key32::generate();

        let mut idx = VectorIndex::new(4).unwrap();
        idx.add(&[1.0, 0.0, 0.0, 0.0], VectorMeta {
            item_id: "a".into(), chunk_idx: 0, level: 2, section_idx: 0,
        }).unwrap();

        idx.save_encrypted(&key, &path).unwrap();
        assert!(path.exists());

        let loaded = VectorIndex::load_encrypted(&key, &path, 4).unwrap();
        assert_eq!(loaded.len(), 1);
    }

    #[test]
    fn dimension_mismatch_error() {
        let mut idx = VectorIndex::new(4).unwrap();
        let result = idx.add(&[1.0, 0.0], VectorMeta {
            item_id: "x".into(), chunk_idx: 0, level: 2, section_idx: 0
        });
        assert!(result.is_err());
    }

    #[test]
    fn get_vector_returns_mean() {
        let mut idx = VectorIndex::new(4).unwrap();
        idx.add(&[1.0, 0.0, 0.0, 0.0], VectorMeta {
            item_id: "a".into(), chunk_idx: 0, level: 2, section_idx: 0
        }).unwrap();
        idx.add(&[0.0, 1.0, 0.0, 0.0], VectorMeta {
            item_id: "a".into(), chunk_idx: 1, level: 2, section_idx: 0
        }).unwrap();
        idx.add(&[0.0, 0.0, 1.0, 0.0], VectorMeta {
            item_id: "b".into(), chunk_idx: 0, level: 2, section_idx: 0
        }).unwrap();

        let v = idx.get_vector("a").unwrap();
        assert_eq!(v.len(), 4);
        assert!((v[0] - 0.5).abs() < 1e-5, "expected 0.5 got {}", v[0]);
        assert!((v[1] - 0.5).abs() < 1e-5, "expected 0.5 got {}", v[1]);
        assert!(idx.get_vector("nonexistent").is_none());
    }

    #[test]
    fn get_vector_missing_item_returns_none() {
        let idx = VectorIndex::new(4).unwrap();
        assert!(idx.get_vector("ghost").is_none());
    }

    #[test]
    fn get_vector_empty_item_id_returns_none() {
        let idx = VectorIndex::new(4).unwrap();
        assert!(idx.get_vector("").is_none());
    }

    #[test]
    fn get_vector_single_chunk_equals_original() {
        let mut idx = VectorIndex::new(3).unwrap();
        idx.add(&[1.0, 2.0, 3.0], VectorMeta { item_id: "x".into(), chunk_idx: 0, level: 2, section_idx: 0 }).unwrap();
        let v = idx.get_vector("x").unwrap();
        assert_eq!(v.len(), 3);
        assert!((v[0] - 1.0).abs() < 1e-5);
        assert!((v[1] - 2.0).abs() < 1e-5);
        assert!((v[2] - 3.0).abs() < 1e-5);
    }
}
