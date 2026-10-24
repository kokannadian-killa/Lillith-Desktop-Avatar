//! Keeping third-party KWin effects off her window (the "black box").
//!
//! On KDE Plasma (e.g. Garuda's default desktop) two popular effects add a
//! dark, blurred, rounded panel behind *every normal window*, including
//! fully transparent ones like hers:
//!
//! - **Better Blur DX** (`better_blur_dx`, Garuda's default) and its parent
//!   **Better Blur** (`forceblur`):
//!   "force blur" blurs + tints whatever is behind a window chosen by
//!   window class ("blur all except matching" mode = every window), with
//!   rounded corners. A window-provided blur region cannot opt out: force
//!   blur overrides it. Docks are skipped unless "Blur docks" is on
//!   (default off).
//! - **Rounded Corners** (`kwin4_effect_shapecorners`): rounds, outlines and
//!   adds a soft shadow to every *normal* window (and dialogs); docks are
//!   never handled.
//!
//! KWin gives a layer-shell surface its window type from the layer
//! *namespace* (`dock`, `utility`, `notification`, …; anything else is
//! `Normal`), so the old namespace `lillith` made her a normal window and
//! both effects drew their panel behind her; on X11 the `UTILITY` type still
//! got force blur. Her window is therefore a **dock** by default (layer
//! namespace `dock`, `_NET_WM_WINDOW_TYPE_DOCK`), which both effects skip
//! out of the box. `LILLITH_WINDOW_TYPE=utility|normal` restores the old
//! types (debugging, or compositors whose rules key on the namespace).
//!
//! [`kwinrc_findings`] reads `~/.config/kwinrc` once at startup and logs
//! which of those effects are on and whether she is excluded, with the
//! exact settings to change; [`fix_kwin`] (`lillith fix-kwin`, also run by
//! `linux/install.sh`) adds her window class `lillith` to the force-blur
//! exclusion list. (The KDE blur protocol cannot opt out: force blur
//! overrides a window's own blur region.)

use std::path::PathBuf;

/// Environment variable overriding [`WindowRole`].
pub const ROLE_ENV: &str = "LILLITH_WINDOW_TYPE";

/// How her window presents itself to the window manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowRole {
    /// `dock`: skipped by Better Blur (force blur) and Rounded Corners.
    #[default]
    Dock,
    /// `utility`: the pre-fix X11 type (Wayland namespace `utility`).
    Utility,
    /// Normal window (Wayland namespace `lillith`, X11 `UTILITY`
    /// kept): the pre-fix Wayland behaviour.
    Normal,
}

impl WindowRole {
    /// Parses `dock` / `utility` / `normal` (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dock" | "" => Some(Self::Dock),
            "utility" => Some(Self::Utility),
            "normal" | "lillith" => Some(Self::Normal),
            _ => None,
        }
    }

    /// From [`ROLE_ENV`], default [`WindowRole::Dock`].
    pub fn from_env() -> Self {
        match std::env::var(ROLE_ENV) {
            Ok(v) => Self::parse(&v).unwrap_or_else(|| {
                tracing::warn!(value = %v, "{ROLE_ENV}: expected dock|utility|normal; using dock");
                Self::Dock
            }),
            Err(_) => Self::Dock,
        }
    }

    /// `zwlr_layer_shell_v1` namespace. KWin maps `dock`/`utility` to that
    /// window type and anything else to `Normal`.
    pub fn layer_namespace(self) -> &'static str {
        match self {
            Self::Dock => "dock",
            Self::Utility => "utility",
            Self::Normal => "lillith",
        }
    }

    /// `true` when KWin effects that only touch normal windows / skip docks
    /// leave her alone.
    pub fn skipped_by_effects(self) -> bool {
        self == Self::Dock
    }
}

/// A KWin effect that can draw a panel behind her.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Effect name as shown in System Settings.
    pub effect: &'static str,
    /// `true` when, with the current settings and `role`, it still targets her.
    pub affects_her: bool,
    /// What to change in System Settings (empty when nothing is needed).
    pub fix: String,
}

/// Minimal INI view: `(group, key) -> value` lookups on a kwinrc text.
pub(crate) struct Ini<'a> {
    pub(crate) text: &'a str,
}

impl Ini<'_> {
    pub(crate) fn get(&self, group: &str, key: &str) -> Option<String> {
        let mut in_group = false;
        let mut out: Option<String> = None;
        for line in self.text.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                in_group = t == format!("[{group}]");
                continue;
            }
            if !in_group {
                continue;
            }
            let Some((k, v)) = t.split_once('=') else {
                continue;
            };
            // `Key[$e]=…` style modifiers are ignored.
            let k = k.split('[').next().unwrap_or(k).trim();
            if k == key {
                out = Some(unescape(v.trim()));
            }
        }
        out
    }

    pub(crate) fn has_group(&self, group: &str) -> bool {
        let g = format!("[{group}]");
        self.text.lines().any(|l| l.trim() == g)
    }

    pub(crate) fn bool(&self, group: &str, key: &str) -> Option<bool> {
        self.get(group, key)
            .map(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
    }
}

/// KConfig escapes: `\n`, `\t`, `\\`, `\s`.
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('s') => out.push(' '),
            Some(o) => out.push(o),
            None => out.push('\\'),
        }
    }
    out
}

/// Her window class on KDE (Wayland: executable name; X11: `WM_CLASS`).
pub const CLASS: &str = "lillith";

/// How one `WindowClasses` line relates to her window class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMatch {
    /// Exact name or a `/regex/` that matches `lillith`.
    Yes,
    /// Doesn't match.
    No,
    /// A `/regex/` we can't compile (Qt syntax we don't speak): could be her.
    Unknown,
}

/// Does a Better Blur window-class list line match [`CLASS`]? Plain lines
/// compare exactly (KWin lowercases resource classes); `/re/` lines are
/// unanchored regex searches, like `QRegularExpression::match().hasMatch()`.
pub fn line_match(line: &str) -> LineMatch {
    let l = line.trim();
    if l.is_empty() {
        return LineMatch::No;
    }
    if let Some(re) = l
        .strip_prefix('/')
        .and_then(|r| r.strip_suffix('/'))
        .filter(|r| !r.is_empty())
    {
        return match regex::Regex::new(re) {
            Ok(r) if r.is_match(CLASS) => LineMatch::Yes,
            Ok(_) => LineMatch::No,
            Err(_) => LineMatch::Unknown,
        };
    }
    if l == CLASS {
        LineMatch::Yes
    } else {
        LineMatch::No
    }
}

fn blur_class_matches(line: &str) -> bool {
    line_match(line) == LineMatch::Yes
}

/// A force-blur KWin effect and where its settings live in `kwinrc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlurEffect {
    /// Name in System Settings.
    pub name: &'static str,
    /// KWin plugin id (`[Plugins] <id>Enabled`, `unloadEffect <id>`).
    pub plugin_id: &'static str,
    /// `<plugin_id>Enabled`.
    pub plugin_key: &'static str,
    /// Settings group (current releases).
    pub group: &'static str,
    /// Settings group of older releases, used when only it exists.
    pub legacy_group: Option<&'static str>,
}

impl BlurEffect {
    fn group_in(&self, ini: &Ini<'_>) -> &'static str {
        match self.legacy_group {
            Some(old) if !ini.has_group(self.group) && ini.has_group(old) => old,
            _ => self.group,
        }
    }
}

/// Force-blur effects (source of truth: `src/blur.kcfg` + `metadata.json`
/// of xarblu/kwin-effects-better-blur-dx and taj-ny/kwin-effects-forceblur).
/// Both: `WindowClasses` (one class per line, `/regex/` allowed, matched
/// against resourceClass and resourceName), `BlurMatching` (default true =
/// blur only the listed classes; false = blur all *except* them),
/// `BlurDocks` (default false: docks never force-blurred).
pub const BLUR_EFFECTS: [BlurEffect; 3] = [
    BlurEffect {
        name: "Better Blur DX",
        plugin_id: "better_blur_dx",
        plugin_key: "better_blur_dxEnabled",
        group: "Effect-better-blur-dx",
        // Renamed from the forceblur group in Nov 2025.
        legacy_group: Some("Effect-blurplus"),
    },
    BlurEffect {
        name: "Better Blur",
        plugin_id: "forceblur",
        plugin_key: "forceblurEnabled",
        group: "Effect-blurplus",
        legacy_group: None,
    },
    BlurEffect {
        name: "Blur Plus",
        plugin_id: "blurplus",
        plugin_key: "blurplusEnabled",
        group: "Effect-blurplus",
        legacy_group: None,
    },
];

/// Does `WindowClasses` + mode force blur onto her?
pub fn class_list_targets_her(classes: &str, blacklist: bool) -> bool {
    let listed = classes.lines().any(blur_class_matches);
    if !blacklist && classes.lines().any(|l| line_match(l) == LineMatch::Unknown) {
        // "Blur only listed" with a regex we can't read: assume it's her.
        return true;
    }
    listed != blacklist
}

/// New `WindowClasses` that keeps force blur off her, or `None` when the
/// list already does: blacklist mode appends `lillith`; whitelist mode
/// drops the exact `lillith` line (a regex that matches her is reported
/// as unfixable by returning `None` from a list that still targets her).
pub fn exclude_her(classes: &str, blacklist: bool) -> Option<String> {
    if !class_list_targets_her(classes, blacklist) {
        return None;
    }
    if blacklist {
        let mut out = classes.trim_end_matches('\n').to_string();
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(CLASS);
        Some(out)
    } else {
        let kept: Vec<&str> = classes.lines().filter(|l| l.trim() != CLASS).collect();
        let out = kept.join("\n");
        (!class_list_targets_her(&out, false)).then_some(out)
    }
}

/// Which installed/enabled effects still put a panel behind her, from the
/// text of `kwinrc`, for her window `role`.
pub fn kwinrc_findings(kwinrc: &str, role: WindowRole) -> Vec<Finding> {
    let ini = Ini { text: kwinrc };
    let mut out = Vec::new();
    for e in BLUR_EFFECTS {
        if ini.bool("Plugins", e.plugin_key) != Some(true) {
            continue;
        }
        let group = e.group_in(&ini);
        let classes = ini.get(group, "WindowClasses").unwrap_or_default();
        let blacklist = ini.bool(group, "BlurMatching") == Some(false);
        let force = class_list_targets_her(&classes, blacklist);
        let docks = ini.bool(group, "BlurDocks") == Some(true);
        let affects_her = force && (role != WindowRole::Dock || docks);
        let fix = if affects_her {
            format!(
                "run `lillith fix-kwin` (adds `{CLASS}` to {name}'s excluded window classes), or \
                 System Settings > Window Management > Desktop Effects > {name} (gear) > {}",
                if blacklist {
                    format!("add the line `{CLASS}` to the window classes")
                } else {
                    format!("remove `{CLASS}` from the window classes")
                },
                name = e.name
            )
        } else {
            String::new()
        };
        out.push(Finding {
            effect: e.name,
            affects_her,
            fix,
        });
    }
    // Rounded Corners: enabled by default once installed; the group exists
    // as soon as its settings were opened.
    let rc_enabled = ini.bool("Plugins", "kwin4_effect_shapecornersEnabled");
    if rc_enabled == Some(true) || (rc_enabled.is_none() && ini.has_group("Round-Corners")) {
        let excluded = ini
            .get("Round-Corners", "Exclusions")
            .unwrap_or_default()
            .split(',')
            .any(|e| !e.trim().is_empty() && CLASS.contains(&e.trim().to_ascii_lowercase()));
        let normal_included = ini
            .bool("Round-Corners", "IncludeNormalWindows")
            .unwrap_or(true);
        let affects_her = role == WindowRole::Normal && normal_included && !excluded;
        out.push(Finding {
            effect: "Rounded Corners",
            affects_her,
            fix: if affects_her {
                format!(
                    "System Settings > Window Management > Desktop Effects > Rounded Corners \
                     (gear) > Inclusions & Exclusions > select `{CLASS}` > Exclude > Apply"
                )
            } else {
                String::new()
            },
        });
    }
    out
}

/// What [`fix_kwin`] did for one effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixOutcome {
    /// `WindowClasses` rewritten (old value kept for the report).
    Excluded { effect: &'static str, old: String },
    /// Already leaves her alone.
    AlreadyFine { effect: &'static str },
    /// A whitelist regex matches her: change it by hand.
    Manual {
        effect: &'static str,
        classes: String,
    },
}

/// Excludes her from every enabled force-blur effect by editing its
/// `WindowClasses` with `kreadconfig6` / `kwriteconfig6`, then asks KWin
/// to reconfigure (`qdbus6 org.kde.KWin /KWin reconfigure`). With
/// `dry_run` nothing is written. Returns one outcome per enabled effect.
pub fn fix_kwin(dry_run: bool) -> std::io::Result<Vec<FixOutcome>> {
    use std::process::Command;
    let read = |group: &str, key: &str| -> std::io::Result<String> {
        let mut c = Command::new("kreadconfig6");
        c.args(["--file", "kwinrc", "--group", group, "--key", key]);
        lillith_core::launch_audit::record_command("fix_kwin", &c);
        let out = c.output()?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .trim_end_matches('\n')
            .to_string())
    };
    let text = kwinrc_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let ini = Ini { text: &text };
    let mut outcomes = Vec::new();
    let mut changed = false;
    for e in BLUR_EFFECTS {
        if read("Plugins", e.plugin_key)?.trim() != "true" {
            continue;
        }
        let group = e.group_in(&ini);
        let classes = read(group, "WindowClasses")?;
        let blacklist = read(group, "BlurMatching")?.trim() == "false";
        match exclude_her(&classes, blacklist) {
            Some(new) => {
                if !dry_run {
                    let mut c = Command::new("kwriteconfig6");
                    c.args([
                        "--file",
                        "kwinrc",
                        "--group",
                        group,
                        "--key",
                        "WindowClasses",
                    ])
                    .arg(&new);
                    lillith_core::launch_audit::record_command("fix_kwin", &c);
                    let st = c.status()?;
                    if !st.success() {
                        return Err(std::io::Error::other(format!("kwriteconfig6 failed: {st}")));
                    }
                    changed = true;
                }
                outcomes.push(FixOutcome::Excluded {
                    effect: e.name,
                    old: classes,
                });
            }
            None if class_list_targets_her(&classes, blacklist) => {
                outcomes.push(FixOutcome::Manual {
                    effect: e.name,
                    classes,
                });
            }
            None => outcomes.push(FixOutcome::AlreadyFine { effect: e.name }),
        }
    }
    if changed {
        let mut c = Command::new("qdbus6");
        c.args(["org.kde.KWin", "/KWin", "reconfigure"]);
        lillith_core::launch_audit::record_command("fix_kwin", &c);
        let ok = c.status().is_ok_and(|s| s.success());
        if !ok {
            let mut c = Command::new("dbus-send");
            c.args([
                "--session",
                "--type=method_call",
                "--dest=org.kde.KWin",
                "/KWin",
                "org.kde.KWin.reconfigure",
            ]);
            lillith_core::launch_audit::record_command("fix_kwin", &c);
            let _ = c.status();
        }
    }
    Ok(outcomes)
}

/// `$XDG_CONFIG_HOME/kwinrc` or `~/.config/kwinrc`.
pub(crate) fn kwinrc_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kwinrc"))
}

/// Logs the window role and any KWin effect that would still draw a panel
/// behind her (one line each, with the setting to change). Read-only.
pub fn log_startup(role: WindowRole) {
    tracing::info!(
        role = ?role,
        namespace = role.layer_namespace(),
        "window type for KWin effects (dock = skipped by Better Blur / Rounded Corners; \
         override with {ROLE_ENV}=utility|normal)"
    );
    let Some(text) = kwinrc_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
        return;
    };
    for f in kwinrc_findings(&text, role) {
        if f.affects_her {
            tracing::warn!(
                effect = f.effect,
                "this KWin effect draws a dark blurred panel behind her (the 'black box'). Fix: {}",
                f.fix
            );
        } else {
            tracing::info!(effect = f.effect, "KWin effect enabled; she is excluded");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_parsing_and_namespaces() {
        assert_eq!(WindowRole::default(), WindowRole::Dock);
        assert_eq!(WindowRole::parse(" Dock "), Some(WindowRole::Dock));
        assert_eq!(WindowRole::parse("utility"), Some(WindowRole::Utility));
        assert_eq!(WindowRole::parse("normal"), Some(WindowRole::Normal));
        assert_eq!(WindowRole::parse("bogus"), None);
        // KWin's scopeToType: exact lowercase "dock" / "utility"; else Normal.
        assert_eq!(WindowRole::Dock.layer_namespace(), "dock");
        assert_eq!(WindowRole::Utility.layer_namespace(), "utility");
        assert!(WindowRole::Dock.skipped_by_effects());
        assert!(!WindowRole::Normal.skipped_by_effects());
    }

    const GARUDA_LIKE: &str = "\
[Plugins]
forceblurEnabled=true
kwin4_effect_shapecornersEnabled=true

[Effect-blurplus]
BlurMatching=false
BlurNonMatching=true
WindowClasses=firefox\\nvlc

[Round-Corners]
Size=12
";

    #[test]
    fn garuda_like_effects_hit_a_normal_window_but_not_a_dock() {
        let normal = kwinrc_findings(GARUDA_LIKE, WindowRole::Normal);
        assert_eq!(normal.len(), 2);
        assert!(normal.iter().all(|f| f.affects_her), "{normal:?}");
        assert!(
            normal[0].fix.contains("add the line `lillith`"),
            "{}",
            normal[0].fix
        );
        assert!(normal[1].fix.contains("Exclude"));
        let utility = kwinrc_findings(GARUDA_LIKE, WindowRole::Utility);
        assert!(utility[0].affects_her, "force blur ignores the window type");
        assert!(
            !utility[1].affects_her,
            "rounded corners: normal windows only"
        );
        let dock = kwinrc_findings(GARUDA_LIKE, WindowRole::Dock);
        assert!(dock.iter().all(|f| !f.affects_her), "{dock:?}");
    }

    #[test]
    fn better_blur_dx_current_and_legacy_groups() {
        let dx = "[Plugins]\nbetter_blur_dxEnabled=true\n[Effect-better-blur-dx]\nBlurMatching=false\nWindowClasses=vlc\n";
        let f = kwinrc_findings(dx, WindowRole::Normal);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].effect, "Better Blur DX");
        assert!(f[0].affects_her && f[0].fix.contains("lillith fix-kwin"));
        let legacy = dx.replace("Effect-better-blur-dx", "Effect-blurplus");
        assert!(kwinrc_findings(&legacy, WindowRole::Normal)[0].affects_her);
        assert!(!kwinrc_findings(dx, WindowRole::Dock)[0].affects_her);
    }

    #[test]
    fn exclude_her_edits_the_class_list() {
        // Blacklist: append once.
        assert_eq!(
            exclude_her("vlc\nfirefox", true).as_deref(),
            Some("vlc\nfirefox\nlillith")
        );
        assert_eq!(exclude_her("", true).as_deref(), Some("lillith"));
        assert_eq!(exclude_her("vlc\nlillith", true), None);
        // Whitelist: drop her line; nothing to do when absent.
        assert_eq!(exclude_her("kate\nlillith", false).as_deref(), Some("kate"));
        assert_eq!(exclude_her("kate", false), None);
        // Whitelist regex that matches her cannot be fixed by a line edit.
        assert_eq!(exclude_her("/lillith|kate/", false), None);
        assert!(class_list_targets_her("/lillith|kate/", false));
    }

    #[test]
    fn exclusions_and_whitelist_mode() {
        let excluded = GARUDA_LIKE.replace("firefox\\nvlc", "firefox\\nlillith")
            + "Exclusions=konsole,lillith\n";
        let f = kwinrc_findings(&excluded, WindowRole::Normal);
        assert!(f.iter().all(|f| !f.affects_her), "{f:?}");
        let regex = GARUDA_LIKE.replace("firefox\\nvlc", "/^(lillith)$/");
        assert!(!kwinrc_findings(&regex, WindowRole::Normal)[0].affects_her);
        // BlurMatching=true wins over BlurNonMatching (whitelist).
        let both = GARUDA_LIKE.replace("BlurMatching=false", "BlurMatching=true");
        assert!(!kwinrc_findings(&both, WindowRole::Normal)[0].affects_her);
        // Default whitelist mode with her not listed: no force blur.
        let wl = "[Plugins]\nforceblurEnabled=true\n[Effect-blurplus]\nWindowClasses=kate\n";
        assert!(!kwinrc_findings(wl, WindowRole::Normal)[0].affects_her);
        // Docks blurred only when the user ticks "Blur docks".
        let docks = GARUDA_LIKE.replace("BlurMatching=false", "BlurMatching=false\nBlurDocks=true");
        assert!(kwinrc_findings(&docks, WindowRole::Dock)[0].affects_her);
        // Disabled / absent effects are not reported.
        assert!(
            kwinrc_findings("[Plugins]\nforceblurEnabled=false\n", WindowRole::Normal).is_empty()
        );
    }
}
