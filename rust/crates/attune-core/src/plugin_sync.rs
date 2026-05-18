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
            match install_one_plugin(ep, &lic.license_key, &plugins_dir) {
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

fn install_one_plugin(ep: &EntitledPlugin, license_key: &str, plugins_dir: &std::path::Path) -> Result<()> {
    // 1. 下载 .attunepkg (PluginHub 要求 Bearer license_key 鉴权)
    let tmp = tempfile::tempdir().map_err(VaultError::Io)?;
    let pkg_path = tmp.path().join(format!("{}.attunepkg", ep.plugin_id));
    download_to_file(&ep.download_url, license_key, &pkg_path)?;

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

/// 从 `.attunepkg` 字节流安装一个插件到 plugins 目录 —— marketplace 下载安装路径用。
///
/// 与 `sync_plugins` 的 entitlement 路径不同：marketplace 不下发 Ed25519 pubkey，
/// 故以"解压后能被 plugin_loader 以 Trusted source 装载"作为包结构合法性判据。
/// 新装插件经一次 attune-server 重启后由 plugin_registry 装载生效。
pub fn install_plugin_package(
    plugin_id: &str,
    pkg_bytes: &[u8],
    plugins_dir: &std::path::Path,
) -> Result<PathBuf> {
    // plugin_id 直接落成目录名 —— 白名单校验，杜绝路径穿越 / NUL / 异常字符
    if plugin_id.is_empty()
        || plugin_id.starts_with('.')
        || !plugin_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
    {
        return Err(VaultError::InvalidInput(format!(
            "unsafe plugin id: {plugin_id}"
        )));
    }

    let tmp = tempfile::tempdir().map_err(VaultError::Io)?;
    let pkg_path = tmp.path().join(format!("{plugin_id}.attunepkg"));
    std::fs::write(&pkg_path, pkg_bytes).map_err(VaultError::Io)?;

    let extract_dir = tmp.path().join("extracted");
    std::fs::create_dir_all(&extract_dir).map_err(VaultError::Io)?;
    extract_tarball(&pkg_path, &extract_dir)?;

    let plugin_src = locate_plugin_dir(&extract_dir)?;

    // 装载校验：能以 Trusted source 装载即视为结构合法（paid tier 也放行）
    let loaded = crate::plugin_loader::LoadedPlugin::from_dir_with_key(
        &plugin_src,
        None,
        Some("Trusted"),
    )?;
    if loaded.manifest.id != plugin_id {
        return Err(VaultError::InvalidInput(format!(
            "package plugin id '{}' mismatches expected '{plugin_id}'",
            loaded.manifest.id
        )));
    }

    std::fs::create_dir_all(plugins_dir).map_err(VaultError::Io)?;
    let dst = plugins_dir.join(plugin_id);
    // 先拷到同目录 staging，再原子 rename 替换 —— 避免"先删后写"被中断留下半成品
    let staging = plugins_dir.join(format!("{plugin_id}.installing"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(VaultError::Io)?;
    }
    copy_dir_recursive(&plugin_src, &staging)?;
    if dst.exists() {
        std::fs::remove_dir_all(&dst).map_err(VaultError::Io)?;
    }
    std::fs::rename(&staging, &dst).map_err(|e| {
        let _ = std::fs::remove_dir_all(&staging); // rename 失败（如跨设备）不留 staging 残目录
        VaultError::Io(e)
    })?;
    Ok(dst)
}

fn download_to_file(url: &str, license_key: &str, dest: &std::path::Path) -> Result<()> {
    // PluginHub requires Bearer authorization; reqwest::blocking::get() is a bare fn with
    // no header support, so we build a one-shot Client here.
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| VaultError::Io(std::io::Error::other(format!("build client: {e}"))))?;
    let resp = client
        .get(url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {license_key}"))
        .send()
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
    // gzip (magic 1f 8b) → 纯 Rust 解压：跨平台含 Windows P0，不依赖系统 tar。
    // tar crate 的 unpack 默认净化 `..` / 绝对路径成员，防解压穿越。
    // 其余格式 (bz2/xz) 回退系统 tar。
    let mut magic = [0u8; 2];
    {
        use std::io::Read;
        let mut f = std::fs::File::open(pkg).map_err(VaultError::Io)?;
        let n = f.read(&mut magic).map_err(VaultError::Io)?;
        if n < 2 {
            return Err(VaultError::InvalidInput(
                "package too small or not a valid archive".into(),
            ));
        }
    }
    if magic == [0x1f, 0x8b] {
        let f = std::fs::File::open(pkg).map_err(VaultError::Io)?;
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(f));
        archive.unpack(dest).map_err(VaultError::Io)?;
        return Ok(());
    }
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

    /// 把一个最小插件目录打成 tar.gz 字节流
    fn make_pkg(parent: &std::path::Path, dir_name: &str, plugin_id: &str) -> Vec<u8> {
        let plugin_dir = parent.join(dir_name);
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.yaml"),
            format!("id: {plugin_id}\nname: Demo\ntype: skill\nversion: 1.0.0\n"),
        )
        .unwrap();
        // 纯 Rust 打包 —— 与 extract_tarball 一致，测试不依赖系统 tar（Windows P0 CI）
        let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(enc);
        builder.append_dir_all(dir_name, &plugin_dir).unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn install_plugin_package_lands_plugin() {
        let tmp = TempDir::new().expect("tmp");
        let bytes = make_pkg(tmp.path(), "demo-plugin", "demo-plugin");
        let plugins_dir = tmp.path().join("plugins");
        let dst = install_plugin_package("demo-plugin", &bytes, &plugins_dir).expect("install");
        assert_eq!(dst, plugins_dir.join("demo-plugin"));
        assert!(dst.join("plugin.yaml").exists());
    }

    #[test]
    fn install_plugin_package_rejects_unsafe_id() {
        let tmp = TempDir::new().expect("tmp");
        let err = install_plugin_package("../evil", b"x", tmp.path()).unwrap_err();
        assert!(format!("{err}").contains("unsafe plugin id"));
    }

    #[test]
    fn install_plugin_package_rejects_id_mismatch() {
        let tmp = TempDir::new().expect("tmp");
        let bytes = make_pkg(tmp.path(), "realname", "realname");
        let err = install_plugin_package("expected-other", &bytes, &tmp.path().join("plugins"))
            .unwrap_err();
        assert!(format!("{err}").contains("mismatch"));
    }

    #[test]
    fn install_plugin_package_overwrites_existing() {
        let tmp = TempDir::new().expect("tmp");
        let plugins_dir = tmp.path().join("plugins");
        // 预置一个旧版本目录
        let stale = plugins_dir.join("demo-plugin");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("stale.txt"), "old").unwrap();
        let bytes = make_pkg(tmp.path(), "demo-plugin", "demo-plugin");
        let dst = install_plugin_package("demo-plugin", &bytes, &plugins_dir).expect("install");
        assert!(dst.join("plugin.yaml").exists());
        assert!(!dst.join("stale.txt").exists(), "旧内容应被覆盖清除");
    }
}
