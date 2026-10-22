//! Home / XDG directory helpers (std only, no `directories` dependency).

use std::path::{Path, PathBuf};

/// `$HOME`, if set and non-empty.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() && Path::new(&v).is_absolute() => Some(PathBuf::from(v)),
        _ => home_dir().map(|h| h.join(fallback)),
    }
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
