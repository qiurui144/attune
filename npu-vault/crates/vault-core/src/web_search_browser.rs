// npu-vault/crates/vault-core/src/web_search_browser.rs
//
// BrowserSearchProvider：chromiumoxide 驱动系统已装的 Chrome/Edge 完成网络搜索。
// 本文件前半部分是跨平台浏览器检测，后半部分是 Provider 实现（Task 4 追加）。

use std::path::{Path, PathBuf};

/// 在常见安装路径中查找一个 Chromium 内核浏览器。
///
/// 查找顺序（首个存在的即返回）：
///   Linux:   google-chrome → chromium → microsoft-edge
///   macOS:   Google Chrome.app → Microsoft Edge.app
///   Windows: Chrome → Edge（ProgramFiles + ProgramFiles(x86) + LocalAppData）
///
/// 返回 None 表示系统无 Chromium 内核浏览器，网络搜索将禁用。
pub fn detect_system_browser() -> Option<PathBuf> {
    detect_with(|p: &Path| p.exists())
}

/// 可测试版本：注入 `exists` 判断函数
fn detect_with<F: Fn(&Path) -> bool>(exists: F) -> Option<PathBuf> {
    for path in candidate_paths() {
        if exists(&path) {
            return Some(path);
        }
    }
    None
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "linux")]
    {
        paths.push(PathBuf::from("/usr/bin/google-chrome"));
        paths.push(PathBuf::from("/usr/bin/google-chrome-stable"));
        paths.push(PathBuf::from("/usr/bin/chromium"));
        paths.push(PathBuf::from("/usr/bin/chromium-browser"));
        paths.push(PathBuf::from("/snap/bin/chromium"));
        paths.push(PathBuf::from("/usr/bin/microsoft-edge"));
    }

    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ));
        paths.push(PathBuf::from(
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ));
        paths.push(PathBuf::from(
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ));
    }

    #[cfg(target_os = "windows")]
    {
        let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
        let pf86 = std::env::var("ProgramFiles(x86)")
            .unwrap_or_else(|_| "C:\\Program Files (x86)".into());
        let local = std::env::var("LOCALAPPDATA").unwrap_or_default();

        paths.push(PathBuf::from(format!(
            "{pf}\\Google\\Chrome\\Application\\chrome.exe"
        )));
        paths.push(PathBuf::from(format!(
            "{pf86}\\Google\\Chrome\\Application\\chrome.exe"
        )));
        if !local.is_empty() {
            paths.push(PathBuf::from(format!(
                "{local}\\Google\\Chrome\\Application\\chrome.exe"
            )));
        }
        paths.push(PathBuf::from(format!(
            "{pf}\\Microsoft\\Edge\\Application\\msedge.exe"
        )));
        paths.push(PathBuf::from(format!(
            "{pf86}\\Microsoft\\Edge\\Application\\msedge.exe"
        )));
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_with_returns_first_existing_path() {
        // 注入的 closure 只对特定路径返回 true，其他全部 false
        // 断言：返回的路径确实是我们"允许"的那条
        let target_name = "chromium"; // linux 里有 /usr/bin/chromium 候选
        let result = detect_with(|p: &Path| {
            p.to_string_lossy().ends_with(target_name)
        });
        assert!(result.is_some(), "should find a matching candidate");
        assert!(result.unwrap().to_string_lossy().ends_with(target_name));
    }

    #[test]
    fn detect_with_returns_none_when_nothing_exists() {
        let result = detect_with(|_p: &Path| false);
        assert!(result.is_none());
    }

    #[test]
    fn candidate_paths_not_empty_on_current_os() {
        let paths = candidate_paths();
        assert!(!paths.is_empty(), "at least one candidate path on this OS");
    }
}
