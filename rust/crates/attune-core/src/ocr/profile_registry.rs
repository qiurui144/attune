//! OCR Profile 注册表 — 内置 4 个 builtin + 用户自定义 profile 的持久化 CRUD.
//!
//! 持久化路径: `<data_dir>/ocr_profiles.json`
//! - 文件不存在时 `load` 自动写入 4 个 builtin
//! - builtin profile 拒绝 delete / update (返回 Err)
//! - 用户自定义 profile (`builtin = false`) 可任意增删改

use crate::error::{Result, VaultError};
use crate::ocr::profile::OcrProfile;
use std::path::{Path, PathBuf};

/// Profile 文件名 (在 data_dir 下)
const PROFILES_FILE: &str = "ocr_profiles.json";

/// 注册表 — 用 in-memory Vec 持有当前 profile 列表, 每次写操作都 flush 到磁盘.
pub struct ProfileRegistry {
    path: PathBuf,
    profiles: Vec<OcrProfile>,
}

impl ProfileRegistry {
    /// 默认路径 = `<data_dir>/ocr_profiles.json`
    pub fn default_path() -> PathBuf {
        crate::platform::data_dir().join(PROFILES_FILE)
    }

    /// 用默认路径加载注册表 (空 → 写 4 builtin).
    pub fn load_default() -> Result<Self> {
        Self::load_from(Self::default_path())
    }

    /// 用指定路径加载 (测试用 / 自定义部署).
    pub fn load_from<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let profiles = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            serde_json::from_str::<Vec<OcrProfile>>(&raw)?
        } else {
            let bs = OcrProfile::builtins();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let body = serde_json::to_string_pretty(&bs)?;
            std::fs::write(&path, body)?;
            bs
        };
        Ok(Self { path, profiles })
    }

    /// 内存中所有 profile (含 builtin + 自定义).
    pub fn list(&self) -> &[OcrProfile] {
        &self.profiles
    }

    /// 按 id 取
    pub fn get(&self, id: &str) -> Option<&OcrProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// upsert: 已存在则覆盖, 不存在则新增.
    /// 若覆盖一个 builtin (即 id 命中且原记录 builtin=true), 返回 Err — 不允许修改 builtin.
    pub fn upsert(&mut self, mut p: OcrProfile) -> Result<()> {
        p.validate().map_err(VaultError::InvalidInput)?;
        if let Some(existing) = self.profiles.iter().find(|x| x.id == p.id) {
            if existing.builtin {
                return Err(VaultError::InvalidInput(format!(
                    "profile {} 是 builtin, 不可修改",
                    p.id
                )));
            }
        }
        // 强制 builtin=false (用户通过 API 写入的永远不是 builtin)
        p.builtin = false;
        if let Some(slot) = self.profiles.iter_mut().find(|x| x.id == p.id) {
            *slot = p;
        } else {
            self.profiles.push(p);
        }
        self.flush()
    }

    /// 按 id 删除. builtin 拒绝.
    pub fn delete(&mut self, id: &str) -> Result<()> {
        let pos = self
            .profiles
            .iter()
            .position(|p| p.id == id)
            .ok_or_else(|| VaultError::NotFound(format!("profile {}", id)))?;
        if self.profiles[pos].builtin {
            return Err(VaultError::InvalidInput(format!(
                "profile {} 是 builtin, 不可删除",
                id
            )));
        }
        self.profiles.remove(pos);
        self.flush()
    }

    fn flush(&self) -> Result<()> {
        let body = serde_json::to_string_pretty(&self.profiles)?;
        std::fs::write(&self.path, body)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_registry() -> (TempDir, ProfileRegistry) {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("ocr_profiles.json");
        let reg = ProfileRegistry::load_from(&path).expect("load");
        (tmp, reg)
    }

    #[test]
    fn first_load_writes_four_builtins() {
        let (tmp, reg) = fresh_registry();
        assert_eq!(reg.list().len(), 4);
        assert!(tmp.path().join("ocr_profiles.json").exists());
    }

    #[test]
    fn second_load_reads_existing() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("ocr_profiles.json");
        let _first = ProfileRegistry::load_from(&path).expect("load1");
        // 写入一个用户自定义
        let mut reg = ProfileRegistry::load_from(&path).expect("load2");
        reg.upsert(OcrProfile {
            id: "mine".to_string(),
            name: "我的".to_string(),
            description: "x".to_string(),
            languages: "chi_sim".to_string(),
            dpi: 300,
            tags: vec![],
            builtin: false,
        })
        .expect("upsert");
        // 再 load 一次, 应该看到 5 条
        let reg3 = ProfileRegistry::load_from(&path).expect("load3");
        assert_eq!(reg3.list().len(), 5);
        assert!(reg3.get("mine").is_some());
    }

    #[test]
    fn delete_builtin_rejected() {
        let (_tmp, mut reg) = fresh_registry();
        let err = reg.delete("contract").expect_err("must reject");
        assert!(format!("{err}").contains("builtin"));
    }

    #[test]
    fn delete_custom_works() {
        let (_tmp, mut reg) = fresh_registry();
        reg.upsert(OcrProfile {
            id: "custom".to_string(),
            name: "X".to_string(),
            description: "x".to_string(),
            languages: "eng".to_string(),
            dpi: 200,
            tags: vec![],
            builtin: false,
        })
        .expect("upsert");
        assert!(reg.get("custom").is_some());
        reg.delete("custom").expect("delete");
        assert!(reg.get("custom").is_none());
    }

    #[test]
    fn upsert_over_builtin_rejected() {
        let (_tmp, mut reg) = fresh_registry();
        let err = reg
            .upsert(OcrProfile {
                id: "contract".to_string(),
                name: "替换".to_string(),
                description: "x".to_string(),
                languages: "eng".to_string(),
                dpi: 100,
                tags: vec![],
                builtin: false,
            })
            .expect_err("must reject");
        assert!(format!("{err}").contains("builtin"));
    }

    #[test]
    fn upsert_forces_builtin_false() {
        let (_tmp, mut reg) = fresh_registry();
        reg.upsert(OcrProfile {
            id: "fake_builtin".to_string(),
            name: "X".to_string(),
            description: "x".to_string(),
            languages: "eng".to_string(),
            dpi: 200,
            tags: vec![],
            builtin: true, // 攻击者尝试声明 builtin
        })
        .expect("upsert");
        assert!(!reg.get("fake_builtin").unwrap().builtin);
    }

    #[test]
    fn upsert_validates_dpi() {
        let (_tmp, mut reg) = fresh_registry();
        let bad = OcrProfile {
            id: "bad".to_string(),
            name: "X".to_string(),
            description: "x".to_string(),
            languages: "eng".to_string(),
            dpi: 10,
            tags: vec![],
            builtin: false,
        };
        assert!(reg.upsert(bad).is_err());
    }

    #[test]
    fn delete_unknown_returns_err() {
        let (_tmp, mut reg) = fresh_registry();
        assert!(reg.delete("does-not-exist").is_err());
    }
}
