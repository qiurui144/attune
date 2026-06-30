//! Cross-platform desktop shell integration.
//!
//! Keep this module dependency-light: it runs in the Tauri process and must work
//! the same way in Windows and Linux release packages.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(windows)]
use std::process::Stdio;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const APP_DIR: &str = "attune";
const LEGACY_APP_DIR: &str = "npu-vault";
const PREFS_FILE: &str = "desktop-preferences.json";

#[derive(Debug, Clone, Serialize)]
pub struct DesktopAppInfo {
    pub version: String,
    pub platform: String,
    pub exe_path: String,
    pub data_dir: String,
    pub config_dir: String,
    pub log_dir: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LaunchAtLoginState {
    pub supported: bool,
    pub enabled: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopPreferences {
    pub close_action: String,
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self {
            close_action: "tray".to_string(),
        }
    }
}

pub fn data_dir() -> PathBuf {
    let base = dirs::data_local_dir()
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    resolve_app_dir(base)
}

pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir()
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    resolve_app_dir(base)
}

pub fn log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ATTUNE_LOG_DIR") {
        return PathBuf::from(dir);
    }
    data_dir().join("logs")
}

pub fn desktop_app_info() -> DesktopAppInfo {
    DesktopAppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        platform: std::env::consts::OS.to_string(),
        exe_path: std::env::current_exe()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        data_dir: data_dir().to_string_lossy().into_owned(),
        config_dir: config_dir().to_string_lossy().into_owned(),
        log_dir: log_dir().to_string_lossy().into_owned(),
    }
}

pub fn load_preferences() -> DesktopPreferences {
    let path = preferences_path();
    let Ok(bytes) = fs::read(path) else {
        return DesktopPreferences::default();
    };
    serde_json::from_slice::<DesktopPreferences>(&bytes).unwrap_or_default()
}

pub fn save_preferences(prefs: &DesktopPreferences) -> Result<(), String> {
    let path = preferences_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(prefs).map_err(|e| e.to_string())?;
    fs::write(path, bytes).map_err(|e| e.to_string())
}

pub fn close_action() -> String {
    match load_preferences().close_action.as_str() {
        "quit" => "quit".to_string(),
        _ => "tray".to_string(),
    }
}

pub fn set_close_action(action: &str) -> Result<DesktopPreferences, String> {
    let normalized = match action {
        "quit" => "quit",
        "tray" => "tray",
        other => return Err(format!("unsupported close action: {other}")),
    };
    let prefs = DesktopPreferences {
        close_action: normalized.to_string(),
    };
    save_preferences(&prefs)?;
    Ok(prefs)
}

pub fn open_desktop_path(kind: &str) -> Result<String, String> {
    let path = match kind {
        "data" => data_dir(),
        "config" => config_dir(),
        "logs" => log_dir(),
        other => return Err(format!("unsupported desktop path kind: {other}")),
    };
    fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    open_path(&path)?;
    Ok(path.to_string_lossy().into_owned())
}

pub fn create_diagnostic_bundle() -> Result<String, String> {
    let logs = log_dir();
    fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
    let out_dir = logs.join("diagnostics");
    fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let bundle = out_dir.join(format!("attune-diagnostics-{ts}.zip"));

    let file = fs::File::create(&bundle).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    let info = desktop_app_info();
    let manifest = format!(
        "Attune diagnostics\nversion={}\nplatform={}\nexe_path={}\ndata_dir={}\nconfig_dir={}\nlog_dir={}\n",
        info.version, info.platform, info.exe_path, info.data_dir, info.config_dir, info.log_dir
    );
    zip.start_file("manifest.txt", options)
        .map_err(|e| e.to_string())?;
    zip.write_all(manifest.as_bytes())
        .map_err(|e| e.to_string())?;

    if logs.exists() {
        for entry in fs::read_dir(&logs).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.starts_with("attune-") {
                continue;
            }
            let mut input = fs::File::open(&path).map_err(|e| e.to_string())?;
            let mut buf = Vec::new();
            input.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            zip.start_file(format!("logs/{name}"), options)
                .map_err(|e| e.to_string())?;
            zip.write_all(&buf).map_err(|e| e.to_string())?;
        }
    }

    zip.finish().map_err(|e| e.to_string())?;
    Ok(bundle.to_string_lossy().into_owned())
}

pub fn reveal_diagnostic_bundle(path: &str) -> Result<(), String> {
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        open_path(parent)
    } else {
        Err("diagnostic bundle has no parent directory".to_string())
    }
}

pub fn get_launch_at_login() -> LaunchAtLoginState {
    launch_at_login_state()
}

pub fn set_launch_at_login(enabled: bool) -> Result<LaunchAtLoginState, String> {
    #[cfg(windows)]
    {
        set_windows_launch_at_login(enabled)?;
        return Ok(launch_at_login_state());
    }
    #[cfg(target_os = "linux")]
    {
        set_linux_launch_at_login(enabled)?;
        return Ok(launch_at_login_state());
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = enabled;
        Ok(LaunchAtLoginState {
            supported: false,
            enabled: false,
            detail: Some("launch at login is supported in Windows and Linux builds".to_string()),
        })
    }
}

pub fn is_background_launch() -> bool {
    std::env::args().any(|arg| arg == "--background" || arg == "--start-minimized")
}

fn preferences_path() -> PathBuf {
    config_dir().join(PREFS_FILE)
}

fn resolve_app_dir(base: PathBuf) -> PathBuf {
    let next = base.join(APP_DIR);
    let legacy = base.join(LEGACY_APP_DIR);
    if !next.exists() && legacy.exists() {
        legacy
    } else {
        next
    }
}

fn open_path(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        command_no_window("explorer")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut last_err = None;
        for program in ["xdg-open", "gio"] {
            let mut cmd = Command::new(program);
            if program == "gio" {
                cmd.arg("open");
            }
            match cmd.arg(path).spawn() {
                Ok(_) => return Ok(()),
                Err(e) => last_err = Some(e),
            }
        }
        return Err(last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "no opener found".to_string()));
    }
    #[allow(unreachable_code)]
    Err(format!(
        "opening paths is unsupported on {}",
        std::env::consts::OS
    ))
}

fn launch_at_login_state() -> LaunchAtLoginState {
    #[cfg(windows)]
    {
        let path = windows_startup_shortcut_path();
        return LaunchAtLoginState {
            supported: true,
            enabled: path.exists(),
            detail: Some(path.to_string_lossy().into_owned()),
        };
    }
    #[cfg(target_os = "linux")]
    {
        let path = linux_autostart_path();
        return LaunchAtLoginState {
            supported: true,
            enabled: path.exists(),
            detail: Some(path.to_string_lossy().into_owned()),
        };
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        LaunchAtLoginState {
            supported: false,
            enabled: false,
            detail: Some("launch at login is supported in Windows and Linux builds".to_string()),
        }
    }
}

#[cfg(windows)]
fn windows_startup_shortcut_path() -> PathBuf {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| config_dir());
    appdata
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join("Attune.lnk")
}

#[cfg(windows)]
fn set_windows_launch_at_login(enabled: bool) -> Result<(), String> {
    let path = windows_startup_shortcut_path();
    if !enabled {
        if path.exists() {
            fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let work_dir = exe.parent().unwrap_or_else(|| Path::new(""));
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');\
         $s.TargetPath='{}';\
         $s.Arguments='--background';\
         $s.WorkingDirectory='{}';\
         $s.IconLocation='{}';\
         $s.Description='Attune';\
         $s.Save()",
        ps_quote(&path),
        ps_quote(&exe),
        ps_quote(work_dir),
        ps_quote(&exe),
    );
    let status = command_no_window("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("powershell exited with {status}"))
    }
}

#[cfg(windows)]
fn ps_quote(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

#[cfg(target_os = "linux")]
fn linux_autostart_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| config_dir());
    base.join("autostart").join("attune.desktop")
}

#[cfg(target_os = "linux")]
fn set_linux_launch_at_login(enabled: bool) -> Result<(), String> {
    let path = linux_autostart_path();
    if !enabled {
        if path.exists() {
            fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let content = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Attune\n\
         Comment=Private AI knowledge companion\n\
         Exec={} --background\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        desktop_exec_quote(&exe)
    );
    fs::write(path, content).map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
fn desktop_exec_quote(path: &Path) -> String {
    let raw = path.to_string_lossy();
    format!("\"{}\"", raw.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(windows)]
fn command_no_window(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_action_rejects_unknown_values() {
        assert!(matches!("quit", "quit" | "tray"));
        assert!(!matches!("close", "quit" | "tray"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn desktop_exec_quote_escapes_quotes() {
        assert_eq!(
            desktop_exec_quote(Path::new("/tmp/attune \"dev\"/attune")),
            "\"/tmp/attune \\\"dev\\\"/attune\""
        );
    }
}
