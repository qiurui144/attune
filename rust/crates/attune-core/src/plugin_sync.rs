//! Plugin sync — 会员登录后, 按账号 license entitled_plugins 自动下载 + 安装 pro 插件.
//!
//! 流程:
//! 1. cloud_client::list_licenses() 拿用户所有 license + entitled_plugins
//! 2. 比对本地已装 (plugin_registry::default_plugins_dir() 内目录列表)
//! 3. 差异: 缺的 → 下载 .attunepkg + verify sig + 解密 + install
//!    多余的 → 留着 (用户手动卸载, 防误删自装插件)

use crate::cloud_client::{CloudClient, EntitledPlugin};
use crate::error::{Result, VaultError};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SyncReport {
    pub installed: Vec<String>,
    pub skipped_already_installed: Vec<String>,
    pub failed: Vec<(String, String)>, // (plugin_id, reason)
}

/// 拉云端 entitled 清单, 自动装缺的 pro 插件
pub fn sync_plugins(cloud: &CloudClient) -> Result<SyncReport> {
    let licenses = cloud.list_licenses()?;
    let plugins_dir = crate::plugin_registry::PluginRegistry::default_plugins_dir()?;
    std::fs::create_dir_all(&plugins_dir).map_err(VaultError::Io)?;
    let installed_ids: std::collections::HashSet<String> = list_installed_plugin_ids(&plugins_dir)?;

    let mut report = SyncReport {
        installed: Vec::new(),
        skipped_already_installed: Vec::new(),
        failed: Vec::new(),
    };

    for lic in &licenses {
        for ep in &lic.entitled_plugins {
            if installed_ids.contains(&ep.plugin_id) {
                report.skipped_already_installed.push(ep.plugin_id.clone());
                continue;
            }
            match install_one_plugin(ep, &plugins_dir) {
                Ok(()) => report.installed.push(ep.plugin_id.clone()),
                Err(e) => report.failed.push((ep.plugin_id.clone(), format!("{e}"))),
            }
        }
    }
    Ok(report)
}

fn list_installed_plugin_ids(plugins_dir: &std::path::Path) -> Result<std::collections::HashSet<String>> {
    let mut out = std::collections::HashSet::new();
    if !plugins_dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(plugins_dir).map_err(VaultError::Io)? {
        let entry = entry.map_err(VaultError::Io)?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // 用 Trusted 装载 (绕开 paid/Unsigned 联动 — 用户已装)
        if let Ok(plugin) =
            crate::plugin_loader::LoadedPlugin::from_dir_with_key(&path, None, Some("Trusted"))
        {
            out.insert(plugin.manifest.id);
        }
    }
    Ok(out)
}

fn install_one_plugin(ep: &EntitledPlugin, plugins_dir: &std::path::Path) -> Result<()> {
    // 1. 下载 .attunepkg
    let tmp = tempfile::tempdir().map_err(VaultError::Io)?;
    let pkg_path = tmp.path().join(format!("{}.attunepkg", ep.plugin_id));
    download_to_file(&ep.download_url, &pkg_path)?;

    // 2. 解压到临时目录 (假定 .attunepkg 是 tar.gz)
    let extract_dir = tmp.path().join("extracted");
    std::fs::create_dir_all(&extract_dir).map_err(VaultError::Io)?;
    extract_tarball(&pkg_path, &extract_dir)?;

    // 3. 找解压后 plugin 实际目录 (通常是 extract_dir/<plugin_id>/ 或 extract_dir/)
    let plugin_src = locate_plugin_dir(&extract_dir)?;

    // 4. 签名校验
    let sig_ok = crate::plugin_sig::verify_with_key(&plugin_src, &ep.signing_pubkey_hex)?;
    if !sig_ok {
        return Err(VaultError::Crypto(format!(
            "plugin {} signature verification FAILED",
            ep.plugin_id
        )));
    }

    // 5. 装载校验 (含解密)
    let key_bytes = ep.decrypt_key.as_ref().map(|k| k.as_bytes().to_vec());
    let _ = crate::plugin_loader::LoadedPlugin::from_dir_with_key(
        &plugin_src,
        key_bytes.as_deref(),
        Some("Trusted"),
    )?;

    // 6. 复制到目标
    let dst = plugins_dir.join(&ep.plugin_id);
    if dst.exists() {
        std::fs::remove_dir_all(&dst).map_err(VaultError::Io)?;
    }
    copy_dir_recursive(&plugin_src, &dst)?;
    Ok(())
}

fn download_to_file(url: &str, dest: &std::path::Path) -> Result<()> {
    let resp = reqwest::blocking::get(url)
        .map_err(|e| VaultError::Io(std::io::Error::other(format!("download: {e}"))))?;
    if !resp.status().is_success() {
        return Err(VaultError::Io(std::io::Error::other(format!(
            "download status {}",
            resp.status()
        ))));
    }
    let bytes = resp
        .bytes()
        .map_err(|e| VaultError::Io(std::io::Error::other(format!("read body: {e}"))))?;
    std::fs::write(dest, &bytes).map_err(VaultError::Io)?;
    Ok(())
}

fn extract_tarball(pkg: &std::path::Path, dest: &std::path::Path) -> Result<()> {
    // 简化: shell out 到 tar (现在: gz / bz2 / xz 都支持). 不引入新 Rust dep.
    let status = std::process::Command::new("tar")
        .args(["xf"])
        .arg(pkg)
        .arg("-C")
        .arg(dest)
        .status()
        .map_err(VaultError::Io)?;
    if !status.success() {
        return Err(VaultError::Io(std::io::Error::other(format!(
            "tar exit {:?}", status.code()
        ))));
    }
    Ok(())
}

fn locate_plugin_dir(extract_dir: &std::path::Path) -> Result<PathBuf> {
    // 优先: extract_dir 本身有 plugin.yaml / plugin.yaml.enc
    if extract_dir.join("plugin.yaml").exists() || extract_dir.join("plugin.yaml.enc").exists() {
        return Ok(extract_dir.to_path_buf());
    }
    // 次: extract_dir/<single_subdir> 有 plugin.yaml
    for entry in std::fs::read_dir(extract_dir).map_err(VaultError::Io)? {
        let entry = entry.map_err(VaultError::Io)?;
        let path = entry.path();
        if path.is_dir()
            && (path.join("plugin.yaml").exists() || path.join("plugin.yaml.enc").exists())
        {
            return Ok(path);
        }
    }
    Err(VaultError::InvalidInput(
        "no plugin.yaml found in extracted .attunepkg".into(),
    ))
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dst).map_err(VaultError::Io)?;
    for entry in std::fs::read_dir(src).map_err(VaultError::Io)? {
        let entry = entry.map_err(VaultError::Io)?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            std::fs::copy(&path, &target).map_err(VaultError::Io)?;
            // 保留 binary 权限 (Unix)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&path) {
                    let mode = meta.permissions().mode();
                    let _ = std::fs::set_permissions(
                        &target,
                        std::fs::Permissions::from_mode(mode),
                    );
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn list_installed_empty_dir_returns_empty() {
        let tmp = TempDir::new().expect("tmp");
        let ids = list_installed_plugin_ids(tmp.path()).expect("list");
        assert!(ids.is_empty());
    }

    #[test]
    fn list_installed_nonexistent_dir_returns_empty() {
        let ids = list_installed_plugin_ids(std::path::Path::new("/nonexistent-xyz")).expect("list");
        assert!(ids.is_empty());
    }

    #[test]
    fn list_installed_finds_valid_plugin() {
        let tmp = TempDir::new().expect("tmp");
        let plugin_dir = tmp.path().join("test-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.yaml"),
            "id: test-plugin\nname: test\ntype: skill\nversion: 1.0.0\n",
        )
        .unwrap();
        let ids = list_installed_plugin_ids(tmp.path()).expect("list");
        assert_eq!(ids.len(), 1);
        assert!(ids.contains("test-plugin"));
    }

    #[test]
    fn locate_plugin_dir_finds_in_root() {
        let tmp = TempDir::new().expect("tmp");
        std::fs::write(tmp.path().join("plugin.yaml"), "id: x").unwrap();
        let found = locate_plugin_dir(tmp.path()).expect("locate");
        assert_eq!(found, tmp.path());
    }

    #[test]
    fn locate_plugin_dir_finds_in_subdir() {
        let tmp = TempDir::new().expect("tmp");
        let sub = tmp.path().join("law-pro");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("plugin.yaml"), "id: law-pro").unwrap();
        let found = locate_plugin_dir(tmp.path()).expect("locate");
        assert_eq!(found, sub);
    }

    #[test]
    fn locate_plugin_dir_missing_yaml_errors() {
        let tmp = TempDir::new().expect("tmp");
        std::fs::create_dir_all(tmp.path().join("empty")).unwrap();
        let err = locate_plugin_dir(tmp.path()).unwrap_err();
        assert!(format!("{err}").contains("no plugin.yaml"));
    }

    #[test]
    fn sync_report_default_empty() {
        let r = SyncReport {
            installed: vec![],
            skipped_already_installed: vec![],
            failed: vec![],
        };
        assert!(r.installed.is_empty());
        assert!(r.failed.is_empty());
    }
}
