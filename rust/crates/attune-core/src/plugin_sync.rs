//! Plugin sync — 会员登录后, 按账号 license entitled_plugins 自动下载 + 安装 pro 插件.
//!
//! 流程:
//! 1. cloud_client::list_licenses() 拿用户所有 license + entitled_plugins
//! 2. 比对本地已装 (plugin_registry::default_plugins_dir() 内目录列表)
//! 3. 差异: 缺的 → 下载 .attunepkg + verify sig + 解密 + install
//!    多余的 → 留着 (用户手动卸载, 防误删自装插件)
//!
//! ## ACP-6 boundary invariant: plugin-shipped ⊥ user-accumulated
//!
//! The D audit (`2026-05-29-self-iteration-preservation-audit.md`) found that
//! learned state surviving a plugin upgrade was a **lucky accident** of file
//! layout, not an enforced contract. ACP-6 Task 4 makes the boundary explicit:
//!
//! | Class | Examples | Lives in | On upgrade |
//! |-------|----------|----------|-----------|
//! | **plugin-shipped** (code) | agent binaries, `plugin.yaml`, prompts, JSON schemas, golden YAML, ratchet thresholds | `plugins/<id>/` | **replaced wholesale** (`remove_dir_all` + recopy) |
//! | **user-accumulated** (learned/user state) | `agent_state` (skill_expansion / preference / ratchet_watermark), `skill_expansions`, signals, memory | vault DB (`data_dir`) | **untouched** |
//!
//! Enforcement (not just doc): every install path operates **only** on
//! `plugins_dir`; it never opens, reads, or writes the vault DB. In the real
//! layout the vault DB (`data_dir/vault.db`) is a *sibling* of `plugins/`
//! (`data_dir/plugins/`), so a `remove_dir_all(plugins/<id>)` on upgrade can
//! never touch it. The guard [`assert_vault_db_outside_plugins_dir`] refuses an
//! install whose `plugins_dir` would contain the vault DB, so a misconfiguration
//! can never let a plugin-dir wipe clobber learned state. The
//! `plugin_upgrade_preserves_user_agent_state` test turns the guarantee into a
//! tested one (audit rec #4).

use crate::cloud_client::{CloudClient, EntitledPlugin};
use crate::error::{Result, VaultError};
use std::path::{Path, PathBuf};

/// Boundary guard (ACP-6 Task 4): refuse any plugin install whose `plugins_dir`
/// would **contain** the vault DB. Plugin installs `remove_dir_all` + recopy a
/// plugin dir under `plugins_dir`; if the vault DB lived inside `plugins_dir`,
/// an upgrade could wipe user-accumulated learned state.
///
/// The real layout passes: `data_dir/vault.db` is a sibling of
/// `data_dir/plugins/`, not inside it. Only a misconfiguration that nests the
/// vault DB under the plugins dir is rejected.
///
/// Best-effort path normalization: canonicalize when the paths exist (resolves
/// symlinks / `..`), else compare lexically.
pub fn assert_vault_db_outside_plugins_dir(plugins_dir: &Path, vault_db: &Path) -> Result<()> {
    let norm = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let plugins = norm(plugins_dir);
    // The vault DB may not exist yet; canonicalize its parent then re-join.
    let vault = match (vault_db.parent(), vault_db.file_name()) {
        (Some(parent), Some(name)) => norm(parent).join(name),
        _ => norm(vault_db),
    };
    if vault == plugins || vault.starts_with(&plugins) {
        return Err(VaultError::InvalidInput(format!(
            "vault DB {vault:?} must not be inside the plugins dir {plugins:?} \
             (ACP-6 boundary: plugin-shipped code ⊥ user-accumulated learned state)"
        )));
    }
    Ok(())
}

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

    // W1-B trust-anchor cross-check (cloud slice8 §5.6) — MUST run before
    // verify_with_key. The signing key is the *root* we verify against; if the
    // server (compromised or MITM'd) hands us an off-allowlist key, verifying the
    // package against that key proves nothing. Pin the trust root to the
    // compile-time OFFICIAL_PLUGIN_ANCHORS allowlist; a miss = refuse install
    // (fail-closed), surfaced as `anchor-not-pinned` in SyncReport.failed.
    verify_plugin_anchor(ep)?;

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

    // ACP-6 boundary: never let a plugin-dir wipe touch the vault DB.
    guard_install_target(plugins_dir)?;

    // 6. 复制到目标
    let dst = plugins_dir.join(&ep.plugin_id);
    if dst.exists() {
        std::fs::remove_dir_all(&dst).map_err(VaultError::Io)?;
    }
    copy_dir_recursive(&plugin_src, &dst)?;
    Ok(())
}

/// W1-B trust-anchor cross-check (cloud slice8 §5.6.1). Refuse to install any
/// entitlement whose `signing_pubkey_hex` is not in the compile-time
/// [`crate::plugin_anchor::OFFICIAL_PLUGIN_ANCHORS`] allowlist.
///
/// This is the desktop-side trust decision: cert-pinning protects the wire, but
/// only this allowlist protects against a *compromised server* substituting an
/// attacker pubkey (the legitimate TLS endpoint still matches the pin). A miss
/// is **rejection** (fail-closed), returned as [`VaultError::AnchorNotPinned`]
/// carrying the off-allowlist key so it lands in `SyncReport.failed` as the
/// `anchor-not-pinned` reason for the UI / telemetry.
fn verify_plugin_anchor(ep: &EntitledPlugin) -> Result<()> {
    if !crate::plugin_anchor::is_official_anchor(&ep.signing_pubkey_hex) {
        return Err(VaultError::AnchorNotPinned(ep.signing_pubkey_hex.clone()));
    }
    Ok(())
}

/// ACP-6 Task 4 enforcement: before any install mutates `plugins_dir`, assert
/// the live vault DB is not nested inside it (so `remove_dir_all` of a plugin
/// dir can never clobber user-accumulated learned state). Uses the real
/// platform vault path; in unit tests `plugins_dir` is a temp dir disjoint from
/// the real `data_dir`, so this is a harmless pass.
fn guard_install_target(plugins_dir: &Path) -> Result<()> {
    let vault_db = crate::platform::data_dir().join("vault.db");
    assert_vault_db_outside_plugins_dir(plugins_dir, &vault_db)
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

    // ACP-6 boundary: never let a plugin-dir wipe touch the vault DB.
    guard_install_target(plugins_dir)?;

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

    // ── ACP-6 Task 4: plugin-shipped ⊥ user-accumulated boundary enforcement ──

    #[test]
    fn install_refuses_when_vault_db_inside_plugins_dir() {
        // §2.3 / boundary invariant: a plugin install must NEVER be able to wipe
        // the vault DB (where user-accumulated agent_state lives). If
        // misconfigured so the vault DB sits inside the plugins dir, the guard
        // must refuse rather than risk a remove_dir_all clobbering learned state.
        let tmp = TempDir::new().expect("tmp");
        let plugins_dir = tmp.path().join("plugins");
        std::fs::create_dir_all(&plugins_dir).unwrap();

        // Misconfigured: vault DB nested INSIDE the plugins dir.
        let bad_vault = plugins_dir.join("vault.db");
        let err = assert_vault_db_outside_plugins_dir(&plugins_dir, &bad_vault).unwrap_err();
        assert!(format!("{err}").contains("must not be inside the plugins dir"));

        // The REAL production layout passes: data_dir/vault.db is a sibling of
        // data_dir/plugins/ (plugins is INSIDE data_dir, but the DB is not
        // inside plugins).
        let data_dir = tmp.path().join("attune");
        let real_plugins = data_dir.join("plugins");
        let real_vault = data_dir.join("vault.db");
        std::fs::create_dir_all(&real_plugins).unwrap();
        assert!(assert_vault_db_outside_plugins_dir(&real_plugins, &real_vault).is_ok());
    }

    #[test]
    fn plugin_upgrade_preserves_user_agent_state() {
        // The audit's recommended upgrade-preservation E2E (rec #4): real vault
        // accumulates agent_state, a plugin is upgraded v1.0.5 -> v1.0.6, assert
        // the user's learned state survives. Turns the incidental guarantee into
        // a tested one — plugin install only mutates the plugins dir.
        use crate::crypto::Key32;
        use crate::store::{AgentStateKind, Store};

        let tmp = TempDir::new().expect("tmp");
        // Vault DB lives in the data dir; plugins dir is a SIBLING (real layout).
        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let vault_db = data_dir.join("vault.db");
        let plugins_dir = tmp.path().join("plugins");

        // User accumulates learned state for an installed plugin.
        let dek = Key32::generate();
        {
            let store = Store::open(&vault_db).unwrap();
            store
                .upsert_agent_state(
                    &dek,
                    "defamation_extractor",
                    "law-pro",
                    AgentStateKind::SkillExpansion,
                    1,
                    b"user-learned-terms",
                )
                .unwrap();
            store
                .upsert_agent_state(
                    &dek,
                    "law-pro-router",
                    "law-pro",
                    AgentStateKind::Preference,
                    1,
                    b"verbosity:terse",
                )
                .unwrap();
            assert_eq!(store.count_agent_state().unwrap(), 2);
        }

        // Install law-pro v1.0.5, then "upgrade" to v1.0.6 (overwrite plugin dir).
        let v105 = make_pkg(tmp.path(), "law-pro-105", "law-pro");
        install_plugin_package("law-pro", &v105, &plugins_dir).expect("install v1.0.5");
        let v106 = make_pkg(tmp.path(), "law-pro-106", "law-pro");
        install_plugin_package("law-pro", &v106, &plugins_dir).expect("upgrade v1.0.6");
        assert!(plugins_dir.join("law-pro").join("plugin.yaml").exists());

        // The boundary invariant holds for this layout (vault DB not under plugins).
        assert!(assert_vault_db_outside_plugins_dir(&plugins_dir, &vault_db).is_ok());

        // CRITICAL: the user's accumulated agent_state must be fully intact after
        // the plugin upgrade — the vault DB was never touched by the install.
        let store = Store::open(&vault_db).unwrap();
        assert_eq!(
            store.count_agent_state().unwrap(),
            2,
            "plugin upgrade must NOT drop user-accumulated agent_state"
        );
        let row = store
            .get_agent_state(&dek, "defamation_extractor", "law-pro", AgentStateKind::SkillExpansion)
            .unwrap()
            .unwrap();
        assert_eq!(row.payload, b"user-learned-terms");
    }

    // ---- W1-B trust-anchor cross-check (cloud slice8 §5.6) ----

    fn ep_with_pubkey(signing_pubkey_hex: &str) -> EntitledPlugin {
        EntitledPlugin {
            plugin_id: "law-pro".into(),
            version: "1.0.5".into(),
            download_url: "https://hub.engi-stack.com/p/law-pro-1.0.5.attunepkg".into(),
            signing_pubkey_hex: signing_pubkey_hex.into(),
            decrypt_key: None,
        }
    }

    #[test]
    fn anchor_check_allows_official_publisher_key() {
        // The law-pro publisher anchor (SSOT mirror of cloud config) must pass.
        let ep = ep_with_pubkey(crate::plugin_anchor::OFFICIAL_PLUGIN_ANCHORS[0]);
        assert!(
            verify_plugin_anchor(&ep).is_ok(),
            "official baked anchor must pass the W1-B cross-check"
        );
    }

    #[test]
    fn anchor_check_rejects_off_allowlist_key_with_typed_error() {
        // Compromised-server / MITM threat: server hands an attacker pubkey over a
        // valid TLS endpoint (cert-pin can't catch this). W1 must reject it.
        let attacker = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let ep = ep_with_pubkey(attacker);
        let err = verify_plugin_anchor(&ep).unwrap_err();
        match err {
            VaultError::AnchorNotPinned(key) => {
                assert_eq!(key, attacker, "error must carry the off-allowlist key for telemetry");
            }
            other => panic!("expected AnchorNotPinned, got {other:?}"),
        }
    }

    #[test]
    fn anchor_check_rejects_empty_key_fail_closed() {
        // A missing signing key must never resolve to "trusted".
        let err = verify_plugin_anchor(&ep_with_pubkey("")).unwrap_err();
        assert!(matches!(err, VaultError::AnchorNotPinned(_)));
    }

    #[test]
    fn anchor_error_surfaces_as_anchor_not_pinned_reason() {
        // The Display string is the reason captured into SyncReport.failed →
        // stable kebab-ish tag the UI/telemetry keys on.
        let err = verify_plugin_anchor(&ep_with_pubkey("deadbeef")).unwrap_err();
        assert!(
            err.to_string().starts_with("anchor not pinned:"),
            "reason must be the anchor-not-pinned message, got: {err}"
        );
    }

    #[test]
    fn anchor_check_runs_before_signature_verification() {
        // Ordering invariant: install_one_plugin calls verify_plugin_anchor BEFORE
        // verify_with_key. We assert the gate alone rejects an off-allowlist key
        // (so a forged package signed by an attacker key never reaches sig verify,
        // which would otherwise "succeed" against the attacker's own key).
        let attacker_signed = ep_with_pubkey(
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        );
        assert!(
            verify_plugin_anchor(&attacker_signed).is_err(),
            "trust-root gate must reject before any signature math against the attacker key"
        );
    }
}
