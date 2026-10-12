use std::{collections::HashSet, path::PathBuf};

fn browser_id(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

pub fn discovered_browser_paths() -> Vec<String> {
    let mut candidates = Vec::<PathBuf>::new();
    #[cfg(target_os = "windows")]
    {
        for root in [std::env::var_os("PROGRAMFILES"), std::env::var_os("PROGRAMFILES(X86)"), std::env::var_os("LOCALAPPDATA")].into_iter().flatten() {
            let root = PathBuf::from(root);
            for relative in ["Microsoft/Edge/Application/msedge.exe", "Google/Chrome/Application/chrome.exe", "BraveSoftware/Brave-Browser/Application/brave.exe", "Chromium/Application/chrome.exe"] {
                candidates.push(root.join(relative));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        for root in ["/Applications", "/System/Applications"] {
            for relative in ["Microsoft Edge.app/Contents/MacOS/Microsoft Edge", "Google Chrome.app/Contents/MacOS/Google Chrome", "Brave Browser.app/Contents/MacOS/Brave Browser", "Chromium.app/Contents/MacOS/Chromium"] {
                candidates.push(PathBuf::from(root).join(relative));
            }
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        for name in ["microsoft-edge", "google-chrome", "google-chrome-stable", "brave-browser", "chromium", "chromium-browser"] {
            if let Some(paths) = std::env::var_os("PATH") {
                for directory in std::env::split_paths(&paths) {
                    candidates.push(directory.join(name));
                }
            }
        }
    }
    let mut seen = HashSet::new();
    candidates.into_iter().filter(|path| path.is_file()).filter_map(|path| std::fs::canonicalize(path).ok()).map(|path| path.to_string_lossy().into_owned()).filter(|path| seen.insert(browser_id(path))).collect()
}

pub fn bundled_chromium_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(root) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|parent| parent.to_path_buf())) {
        let browsers = root.join("browsers");
        #[cfg(target_os = "windows")]
        let relative = PathBuf::from("chrome-win").join("chrome.exe");
        #[cfg(target_os = "macos")]
        let relative = PathBuf::from("chrome-mac").join("Chromium.app").join("Contents").join("MacOS").join("Chromium");
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        let relative = PathBuf::from("chrome-linux").join("chrome");
        if let Ok(entries) = std::fs::read_dir(&browsers) {
            for entry in entries.flatten() {
                out.push(entry.path().join(&relative));
            }
        }
        out.push(browsers.join(&relative));
    }
    out
}

pub fn resolve_browser_executable(configured: &str) -> Option<String> {
    let configured = configured.trim();
    if !configured.is_empty() {
        return Some(configured.to_string());
    }
    for candidate in bundled_chromium_candidates() {
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    discovered_browser_paths().into_iter().next()
}

pub fn resolve_adt_browser_executable() -> Option<String> {
    let configured = std::env::var("MDEV_BROWSER_EXECUTABLE").unwrap_or_default();
    resolve_browser_executable(&configured)
}
