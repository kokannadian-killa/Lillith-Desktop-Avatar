//! Home / XDG directory helpers (std only, no `directories` dependency).

use std::path::{Path, PathBuf};

/// `$HOME`, if set and non-empty (Windows: `%USERPROFILE%` when `HOME`
/// is unset).
pub fn home_dir() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|h| !h.is_empty());
    let h = var("HOME");
    #[cfg(windows)]
    let h = h.or_else(|| var("USERPROFILE"));
    h.map(PathBuf::from)
}

#[cfg(not(windows))]
fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() && Path::new(&v).is_absolute() => Some(PathBuf::from(v)),
        _ => home_dir().map(|h| h.join(fallback)),
    }
}

/// Windows: the XDG variables still win when set (absolute), else the
/// Known Folder environment variables (`%APPDATA%` for config,
/// `%LOCALAPPDATA%` for data and cache).
#[cfg(windows)]
fn xdg(var: &str, _fallback: &str) -> Option<PathBuf> {
    let abs = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty() && Path::new(v).is_absolute())
            .map(PathBuf::from)
    };
    abs(var).or_else(|| match var {
        "XDG_CONFIG_HOME" => abs("APPDATA"),
        _ => abs("LOCALAPPDATA").or_else(|| abs("APPDATA")),
    })
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
