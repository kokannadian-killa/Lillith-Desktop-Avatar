//! Home / XDG directory helpers (std only, no `directories` dependency).

use std::path::PathBuf;

/// `$HOME`, if set and non-empty (Windows: `%USERPROFILE%` when `HOME`
/// is unset).
pub fn home_dir() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|h| !h.is_empty());
    let h = var("HOME");
    #[cfg(windows)]
    let h = h.or_else(|| var("USERPROFILE"));
    h.map(PathBuf::from)
}

fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    base_dir_from(
        var,
        fallback,
        |k| std::env::var_os(k).map(PathBuf::from),
        home_dir(),
        cfg!(windows),
    )
}

/// The base-dir rule from explicit values (testable on every OS): an
/// absolute `$XDG_*` wins; on Windows config then uses `%APPDATA%` (same as
/// `lillith-core`'s config path) and data/cache use `%LOCALAPPDATA%`; else
/// `~/<fallback>`. Roaming `%APPDATA%` is never a data/cache fallback (it
/// can sync to a server; memory and history stay local-only).
fn base_dir_from(
    var: &str,
    fallback: &str,
    get: impl Fn(&str) -> Option<PathBuf>,
    home: Option<PathBuf>,
    windows: bool,
) -> Option<PathBuf> {
    let abs = |k: &str| get(k).filter(|v| !v.as_os_str().is_empty() && v.is_absolute());
    abs(var)
        .or_else(|| match (windows, var) {
            (false, _) => None,
            (true, "XDG_CONFIG_HOME") => abs("APPDATA"),
            (true, _) => abs("LOCALAPPDATA"),
        })
        .or_else(|| home.map(|h| h.join(fallback)))
}

/// `$XDG_CONFIG_HOME`, else `~/.config`.
pub fn config_home() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME", ".config")
}

/// `$XDG_DATA_HOME`, else `~/.local/share`.
pub fn data_home() -> Option<PathBuf> {
    xdg("XDG_DATA_HOME", ".local/share")
}

/// `$XDG_CACHE_HOME`, else `~/.cache`.
pub fn cache_home() -> Option<PathBuf> {
    xdg("XDG_CACHE_HOME", ".cache")
}

/// `~/.config/lillith/config.toml` (same rule as `lillith-core::config`).
pub fn default_config_path() -> Option<PathBuf> {
    config_home().map(|c| c.join("lillith/config.toml"))
}

/// Where the wizard writes Lily's Modelfile by default:
/// `~/.config/lillith/Modelfile.lily`.
pub fn default_modelfile_path() -> Option<PathBuf> {
    config_home().map(|c| c.join("lillith/Modelfile.lily"))
}

/// Expands a leading `~` / `~/`.
pub fn expand_tilde(p: &str) -> PathBuf {
    if p == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(p));
    }
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(h) = home_dir() {
            return h.join(rest);
        }
    }
    PathBuf::from(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs(p: &str) -> PathBuf {
        // Absolute on the OS running the tests.
        std::env::temp_dir().join(p)
    }

    #[test]
    fn data_and_cache_never_fall_back_to_roaming_appdata() {
        let env = |set: &'static [&'static str]| move |k: &str| set.contains(&k).then(|| abs(k));
        let home = Some(abs("home"));
        for var in ["XDG_DATA_HOME", "XDG_CACHE_HOME"] {
            // XDG wins.
            let all = env(&["XDG_DATA_HOME", "XDG_CACHE_HOME", "LOCALAPPDATA", "APPDATA"]);
            assert_eq!(
                base_dir_from(var, "f", all, home.clone(), true),
                Some(abs(var))
            );
            // Then %LOCALAPPDATA%.
            let la = env(&["LOCALAPPDATA", "APPDATA"]);
            assert_eq!(
                base_dir_from(var, "f", la, home.clone(), true),
                Some(abs("LOCALAPPDATA"))
            );
            // %APPDATA% alone is ignored: HOME, else nothing.
            let roam = env(&["APPDATA"]);
            assert_eq!(
                base_dir_from(var, "f", roam, home.clone(), true),
                Some(abs("home").join("f"))
            );
            assert_eq!(base_dir_from(var, "f", env(&["APPDATA"]), None, true), None);
        }
        // Config keeps %APPDATA% (same file lillith-core reads).
        assert_eq!(
            base_dir_from("XDG_CONFIG_HOME", "f", env(&["APPDATA"]), None, true),
            Some(abs("APPDATA"))
        );
        // Non-Windows: XDG, else ~/fallback; the Known Folder vars mean nothing.
        assert_eq!(
            base_dir_from(
                "XDG_DATA_HOME",
                ".local/share",
                env(&["LOCALAPPDATA"]),
                home,
                false
            ),
            Some(abs("home").join(".local/share"))
        );
        // A relative XDG value is ignored.
        let rel = |k: &str| (k == "XDG_DATA_HOME").then(|| PathBuf::from("rel"));
        assert_eq!(base_dir_from("XDG_DATA_HOME", "f", rel, None, false), None);
    }
}
