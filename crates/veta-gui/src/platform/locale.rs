//! The operating system's language, for the first start (DECISIONS 51).

/// The system locale (e.g. `es_ES.UTF-8`, `es-ES`), if it can be found.
pub fn system_locale() -> Option<String> {
    let env = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX");
    env.or_else(os_locale)
}

/// Apps started from Finder get no `LANG`: ask the user defaults.
#[cfg(target_os = "macos")]
fn os_locale() -> Option<String> {
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_owned()).filter(|l| !l.is_empty())
}

/// The user's locale name from the registry, e.g. `es-ES`.
#[cfg(windows)]
fn os_locale() -> Option<String> {
    use std::os::windows::process::CommandExt;
    // Don't flash a console window.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Control Panel\International",
            "/v",
            "LocaleName",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    // "    LocaleName    REG_SZ    es-ES"
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.contains("LocaleName"))
        .and_then(|l| l.split_whitespace().last())
        .map(str::to_owned)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn os_locale() -> Option<String> {
    None
}
