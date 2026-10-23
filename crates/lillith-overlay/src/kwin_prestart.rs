//! Pre-start KWin check: keep force-blur effects off her *before* her first
//! surface is mapped, so Ko never sees the dark blurred box, not even for
//! one frame.
//!
//! Called by the app before the overlay backend (Wayland or X11) creates
//! her window (`[overlay] auto_fix_kwin`, default on). On KDE it:
//!
//! 1. asks KWin which effects are loaded (`org.kde.kwin.Effects`
//!    `loadedEffects` over D-Bus); falls back to `[Plugins] …Enabled` in
//!    kwinrc when D-Bus doesn't answer;
//! 2. for every loaded force-blur effect (Better Blur DX `better_blur_dx`,
//!    Better Blur `forceblur`, `blurplus`) whose `WindowClasses` would blur
//!    her, edits the list like `lillith fix-kwin` (blur-all-except: add
//!    `lillith`; blur-only: drop the `lillith` line), records the change
//!    in the same state file `kwin-blur-exclude.sh revert` /
//!    `lillith fix-kwin --revert` undo, and reconfigures that effect
//!    (`reconfigureEffect`, synchronous inside KWin) plus KWin;
//! 3. when that's impossible (a `/regex/` in blur-only mode that covers her,
//!    an unknown effect whose config group has a blur-all `WindowClasses`
//!    list, no `kwriteconfig6`), unloads the effect **for this session
//!    only** (`unloadEffect`, kwinrc untouched) and leaves a one-time note
//!    for the chat saying what was done and how to make it permanent.
//!
//! Never blocks startup for long: every D-Bus/helper call shares one
//! deadline ([`BUDGET`], 1.4 s) and is killed when it runs out; without
//! D-Bus she simply starts (her dock window type still keeps Better Blur
//! off by default).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::kde_effects::{
    class_list_targets_her, exclude_her, kwinrc_path, Ini, WindowRole, BLUR_EFFECTS, CLASS,
};

/// Total time the pre-start check may take (all D-Bus + helper calls).
pub const BUDGET: Duration = Duration::from_millis(1400);
/// Settle time after a reconfigure, so KWin's own reconfigure timer has run.
pub const SETTLE: Duration = Duration::from_millis(250);
/// State file shared with `linux/kwin-blur-exclude.sh` (`group<TAB>added|removed`).
pub const STATE_FILE: &str = "kwin-blur-exclusion.state";
/// Effects already announced as session-unloaded (one per line).
pub const NOTIFIED_FILE: &str = "kwin-unload.notified";

/// Why a bus call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BusError {
    /// The call didn't answer before the deadline (killed).
    Timeout,
    /// No helper (`dbus-send`/`qdbus6`) or no session bus / KWin.
    Unavailable(String),
}

/// The few KWin D-Bus calls the check needs.
pub trait KwinBus {
    /// `org.kde.kwin.Effects.loadedEffects` on `/Effects`.
    fn loaded_effects(&mut self, deadline: Instant) -> Result<Vec<String>, BusError>;
    /// `org.kde.kwin.Effects.reconfigureEffect(id)`.
    fn reconfigure_effect(&mut self, id: &str, deadline: Instant) -> Result<(), BusError>;
    /// `org.kde.KWin.reconfigure` on `/KWin`.
    fn reconfigure(&mut self, deadline: Instant) -> Result<(), BusError>;
    /// `org.kde.kwin.Effects.unloadEffect(id)` (session only).
    fn unload_effect(&mut self, id: &str, deadline: Instant) -> Result<(), BusError>;
}

/// kwinrc access: read the merged text, write one key.
pub trait KwinRc {
    /// System (`/etc/xdg/kwinrc`, …) then user kwinrc text, so later keys win.
    fn text(&self) -> String;
    /// `kwriteconfig6 --file kwinrc --group G --key K V`.
    fn write(
        &mut self,
        group: &str,
        key: &str,
        value: &str,
        deadline: Instant,
    ) -> Result<(), String>;
}

/// One thing the check did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `WindowClasses` edited (persistent, revertible).
    Excluded {
        effect: String,
        group: String,
        how: &'static str,
    },
    /// Already leaves her alone.
    Fine { effect: String },
    /// Unloaded for this session (kwinrc untouched).
    Unloaded { effect: String, why: String },
    /// Needed a fix but couldn't (unload failed / timed out).
    Failed { effect: String, why: String },
}

/// What the check found and did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// Set when the check didn't run (not KDE, switched off).
    pub skipped: Option<String>,
    /// `true` when `loadedEffects` answered (else kwinrc flags were used).
    pub bus_ok: bool,
    pub actions: Vec<Action>,
    /// Steps in order, for logs and the ordering test.
    pub trace: Vec<String>,
    /// One-time note for the chat (what was done, how to make it permanent).
    pub notice: Option<String>,
    /// Time spent.
    pub elapsed: Duration,
}

/// `true` on a KDE Plasma session (`XDG_CURRENT_DESKTOP`/`KDE_FULL_SESSION`).
pub fn is_kde(get: impl Fn(&str) -> Option<String>) -> bool {
    get("XDG_CURRENT_DESKTOP")
        .map(|v| v.split(':').any(|d| d.eq_ignore_ascii_case("KDE")))
        .unwrap_or(false)
        || get("KDE_FULL_SESSION").is_some_and(|v| v == "true")
        || get("KDE_SESSION_VERSION").is_some_and(|v| !v.is_empty())
}

/// Config groups an unknown effect id may keep its settings in.
fn candidate_groups(id: &str) -> Vec<String> {
    let mut g = vec![format!("Effect-{id}")];
    let dashed = format!("Effect-{}", id.replace('_', "-"));
    if !g.contains(&dashed) {
        g.push(dashed);
    }
    g
}

/// Effects shipped with KWin that never force blur (fast skip).
const STOCK: &[&str] = &[
    "blur",
    "contrast",
    "backgroundcontrast",
    "slide",
    "fade",
    "scale",
    "zoom",
    "dimscreen",
    "colorpicker",
    "screenshot",
    "screenedge",
    "startupfeedback",
    "kscreen",
    "overview",
    "windowview",
    "desktopgrid",
    "highlightwindow",
    "outputlocator",
    "tileseditor",
    "systembell",
    "shakecursor",
    "magiclamp",
    "squash",
    "maximize",
    "login",
    "logout",
    "fadingpopups",
    "slidingpopups",
    "frozenapp",
    "dialogparent",
    "blendchanges",
];

/// Runs the check. `role` is her window type (docks are skipped by Better
/// Blur unless "Blur docks" is on; exclusion is applied anyway, unloading
/// only when the effect would really draw behind her).
pub fn run_check(
    bus: &mut dyn KwinBus,
    rc: &mut dyn KwinRc,
    data_dir: Option<&Path>,
    role: WindowRole,
    budget: Duration,
) -> Report {
    let t0 = Instant::now();
    let deadline = t0 + budget;
    let mut r = Report::default();
    r.trace.push("check:start".into());
    let text = rc.text();
    let ini = Ini { text: &text };
    let loaded = match bus.loaded_effects(deadline) {
        Ok(l) => {
            r.bus_ok = true;
            l
        }
        Err(e) => {
            r.trace.push(format!("bus:{e:?}"));
            // Fall back to what kwinrc says is enabled (known effects only).
            BLUR_EFFECTS
                .iter()
                .filter(|e| ini.bool("Plugins", e.plugin_key) == Some(true))
                .map(|e| e.plugin_id.to_string())
                .collect()
        }
    };
    r.trace.push(format!("loaded:{}", loaded.join(",")));
    let mut reconfigure: Vec<String> = Vec::new();
    let mut unload: Vec<(String, String)> = Vec::new();
    let mut state_lines: Vec<String> = Vec::new();
    for id in &loaded {
        if let Some(e) = BLUR_EFFECTS.iter().find(|e| e.plugin_id == id) {
            let group = if e.plugin_id == "better_blur_dx"
                && !ini.has_group(e.group)
                && ini.has_group("Effect-blurplus")
            {
                "Effect-blurplus"
            } else {
                e.group
            };
            let classes = ini.get(group, "WindowClasses").unwrap_or_default();
            let blacklist = ini.bool(group, "BlurMatching") == Some(false);
            if !class_list_targets_her(&classes, blacklist) {
                r.actions.push(Action::Fine {
                    effect: e.name.into(),
                });
                continue;
            }
            let docks = ini.bool(group, "BlurDocks") == Some(true);
            let hits_her = role != WindowRole::Dock || docks;
            match exclude_her(&classes, blacklist) {
                Some(new) => match rc.write(group, "WindowClasses", &new, deadline) {
                    Ok(()) => {
                        let how = if blacklist { "added" } else { "removed" };
                        r.trace.push(format!("write:{group}"));
                        state_lines.push(format!("{group}\t{how}"));
                        r.actions.push(Action::Excluded {
                            effect: e.name.into(),
                            group: group.into(),
                            how,
                        });
                        reconfigure.push(id.clone());
                    }
                    Err(why) if hits_her => unload.push((id.clone(), why)),
                    Err(why) => {
                        // Her dock window type keeps this effect off her
                        // anyway: log it, don't bother Ko.
                        r.trace.push(format!("write-failed:{group}:{why}"));
                        r.actions.push(Action::Fine {
                            effect: e.name.into(),
                        });
                    }
                },
                None if hits_her => unload.push((
                    id.clone(),
                    format!(
                        "a /regex/ in [{group}] WindowClasses (blur only listed) covers `{CLASS}`"
                    ),
                )),
                None => r.actions.push(Action::Fine {
                    effect: e.name.into(),
                }),
            }
            continue;
        }
        if STOCK.contains(&id.as_str()) {
            continue;
        }
        // Unknown effect: a WindowClasses list in blur-all mode?
        for g in candidate_groups(id) {
            let Some(classes) = ini.get(&g, "WindowClasses") else {
                continue;
            };
            let blur_all = ini.bool(&g, "BlurMatching") == Some(false);
            if blur_all && class_list_targets_her(&classes, true) {
                unload.push((
                    id.clone(),
                    format!("unknown effect with a blur-all window list in [{g}]"),
                ));
            } else {
                r.actions.push(Action::Fine { effect: id.clone() });
            }
            break;
        }
    }
    if !state_lines.is_empty() {
        if let Some(d) = data_dir {
            let _ = append_state(&d.join(STATE_FILE), &state_lines);
        }
    }
    if !reconfigure.is_empty() && r.bus_ok {
        let mut all_ok = true;
        for id in &reconfigure {
            let res = bus.reconfigure_effect(id, deadline);
            r.trace
                .push(format!("reconfigureEffect:{id}:{}", res.is_ok()));
            all_ok &= res.is_ok();
        }
        let res = bus.reconfigure(deadline);
        r.trace.push(format!("reconfigure:{}", res.is_ok()));
        if !all_ok {
            // Fall back to KWin's own (timer-driven) reconfigure.
            settle(deadline, SETTLE);
        } else {
            settle(deadline, SETTLE / 2);
        }
        r.trace.push("settled".into());
    }
    let mut unloaded = Vec::new();
    for (id, why) in unload {
        let name = BLUR_EFFECTS
            .iter()
            .find(|e| e.plugin_id == id)
            .map(|e| e.name.to_string())
            .unwrap_or_else(|| id.clone());
        if !r.bus_ok {
            r.actions.push(Action::Failed {
                effect: name,
                why: format!("{why}; KWin's D-Bus didn't answer, so it couldn't be unloaded"),
            });
            continue;
        }
        match bus.unload_effect(&id, deadline) {
            Ok(()) => {
                r.trace.push(format!("unload:{id}"));
                unloaded.push((id.clone(), name.clone()));
                r.actions.push(Action::Unloaded { effect: name, why });
            }
            Err(e) => r.actions.push(Action::Failed {
                effect: name,
                why: format!("{why}; unloadEffect: {e:?}"),
            }),
        }
    }
    r.notice = notice(&r.actions, &unloaded, data_dir);
    r.elapsed = t0.elapsed();
    r.trace.push("check:done".into());
    r
}

fn settle(deadline: Instant, want: Duration) {
    let left = deadline.saturating_duration_since(Instant::now());
    std::thread::sleep(want.min(left));
}

fn append_state(path: &Path, lines: &[String]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    for l in lines {
        writeln!(f, "{l}")?;
    }
    Ok(())
}

/// Chat note: exclusions are announced once (they're permanent); session
/// unloads only the first time a given effect gets unloaded.
fn notice(
    actions: &[Action],
    unloaded: &[(String, String)],
    data_dir: Option<&Path>,
) -> Option<String> {
    let mut parts = Vec::new();
    for a in actions {
        if let Action::Excluded { effect, group, .. } = a {
            parts.push(format!(
                "{effect} wanted to put a dark blurred box behind me, so I added `{CLASS}` to its \
                 excluded windows ([{group}] in kwinrc). Undo: `lillith fix-kwin --revert`."
            ));
        }
    }
    let notified_path = data_dir.map(|d| d.join(NOTIFIED_FILE));
    let notified = notified_path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let mut newly = Vec::new();
    for (id, name) in unloaded {
        if notified.lines().any(|l| l.trim() == id) {
            continue;
        }
        newly.push(id.clone());
        parts.push(format!(
            "{name} would have blurred a dark box behind me and I couldn't exclude myself, so I \
             switched it off for this session only (nothing saved). To make it permanent: System \
             Settings > Window Management > Desktop Effects > {name} > gear > window classes, make \
             sure `{CLASS}` is not blurred (or untick {name})."
        ));
    }
    if let (Some(p), false) = (&notified_path, newly.is_empty()) {
        let _ = append_state(p, &newly);
    }
    for a in actions {
        if let Action::Failed { effect, why } = a {
            // Once per effect, like the unload note.
            let key = format!("failed:{effect}");
            if notified.lines().any(|l| l.trim() == key) {
                continue;
            }
            if let Some(p) = &notified_path {
                let _ = append_state(p, std::slice::from_ref(&key));
            }
            parts.push(format!(
                "{effect} may draw a dark box behind me and I couldn't fix it ({why}). Run \
                 `lillith fix-kwin`, or untick {effect} in System Settings > Desktop Effects."
            ));
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

// ---------------------------------------------------------------- real I/O

/// Runs `cmd` until it exits or `deadline` passes (then kills it).
fn run_until(cmd: &mut Command, deadline: Instant) -> Result<String, BusError> {
    if Instant::now() >= deadline {
        return Err(BusError::Timeout);
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| BusError::Unavailable(e.to_string()))?;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                let mut out = String::new();
                if let Some(mut o) = child.stdout.take() {
                    let _ = o.read_to_string(&mut out);
                }
                return if st.success() {
                    Ok(out)
                } else {
                    Err(BusError::Unavailable(format!("exit {st}")))
                };
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BusError::Timeout);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(BusError::Unavailable(e.to_string())),
        }
    }
}

/// Every `string "…"` in `dbus-send --print-reply` output (or, for
/// `qdbus6`, one name per line).
pub fn parse_string_list(out: &str) -> Vec<String> {
    if out.contains("string \"") {
        return out
            .split("string \"")
            .skip(1)
            .filter_map(|s| s.split('"').next())
            .map(str::to_string)
            .collect();
    }
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.contains(' '))
        .map(str::to_string)
        .collect()
}

/// The real session bus via `dbus-send` (or `qdbus6` when it's missing).
#[derive(Debug, Default)]
pub struct SessionBus;

impl SessionBus {
    fn ms_left(deadline: Instant) -> u128 {
        deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .max(1)
    }

    fn call(
        &self,
        path: &str,
        method: &str,
        args: &[String],
        deadline: Instant,
    ) -> Result<String, BusError> {
        let mut c = Command::new("dbus-send");
        c.arg("--session")
            .arg("--print-reply")
            .arg(format!("--reply-timeout={}", Self::ms_left(deadline)))
            .arg("--dest=org.kde.KWin")
            .arg(path)
            .arg(method)
            .args(args);
        match run_until(&mut c, deadline) {
            Err(BusError::Unavailable(e))
                if e.contains("No such file") || e.contains("not found") =>
            {
                // No dbus-send: qdbus6 takes plain args.
                let short = method.rsplit('.').next().unwrap_or(method);
                let iface = method.rsplit_once('.').map(|x| x.0).unwrap_or("");
                let mut q = Command::new("qdbus6");
                q.arg("org.kde.KWin").arg(path);
                if method == "org.freedesktop.DBus.Properties.Get" {
                    q.arg("org.freedesktop.DBus.Properties.Get");
                } else {
                    q.arg(format!("{iface}.{short}"));
                }
                q.args(
                    args.iter()
                        .map(|a| a.split_once(':').map(|x| x.1).unwrap_or(a)),
                );
                run_until(&mut q, deadline)
            }
            r => r,
        }
    }
}

impl KwinBus for SessionBus {
    fn loaded_effects(&mut self, deadline: Instant) -> Result<Vec<String>, BusError> {
        let out = self.call(
            "/Effects",
            "org.freedesktop.DBus.Properties.Get",
            &[
                "string:org.kde.kwin.Effects".into(),
                "string:loadedEffects".into(),
            ],
            deadline,
        )?;
        Ok(parse_string_list(&out))
    }
    fn reconfigure_effect(&mut self, id: &str, deadline: Instant) -> Result<(), BusError> {
        self.call(
            "/Effects",
            "org.kde.kwin.Effects.reconfigureEffect",
            &[format!("string:{id}")],
            deadline,
        )
        .map(drop)
    }
    fn reconfigure(&mut self, deadline: Instant) -> Result<(), BusError> {
        self.call("/KWin", "org.kde.KWin.reconfigure", &[], deadline)
            .map(drop)
    }
    fn unload_effect(&mut self, id: &str, deadline: Instant) -> Result<(), BusError> {
        self.call(
            "/Effects",
            "org.kde.kwin.Effects.unloadEffect",
            &[format!("string:{id}")],
            deadline,
        )
        .map(drop)
    }
}

/// kwinrc on disk (`$XDG_CONFIG_DIRS/kwinrc` under `~/.config/kwinrc`),
/// written with `kwriteconfig6`.
#[derive(Debug, Default)]
pub struct DiskRc;

impl KwinRc for DiskRc {
    fn text(&self) -> String {
        let mut out = String::new();
        let dirs = std::env::var("XDG_CONFIG_DIRS")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "/etc/xdg".into());
        // Lowest priority first (the list is most-important-first).
        for d in dirs.split(':').rev().filter(|d| !d.is_empty()) {
            if let Ok(t) = std::fs::read_to_string(PathBuf::from(d).join("kwinrc")) {
                out.push_str(&t);
                out.push('\n');
            }
        }
        if let Some(t) = kwinrc_path().and_then(|p| std::fs::read_to_string(p).ok()) {
            out.push_str(&t);
        }
        out
    }
    fn write(
        &mut self,
        group: &str,
        key: &str,
        value: &str,
        deadline: Instant,
    ) -> Result<(), String> {
        let mut c = Command::new("kwriteconfig6");
        c.args(["--file", "kwinrc", "--group", group, "--key", key, "--"])
            .arg(value);
        run_until(&mut c, deadline)
            .map(drop)
            .map_err(|e| format!("kwriteconfig6: {e:?}"))
    }
}

/// The app's entry point: no-op (logged) off KDE or when switched off;
/// otherwise [`run_check`] on the real bus/kwinrc, logging every action.
pub fn before_first_frame(enabled: bool, data_dir: Option<&Path>) -> Report {
    let role = WindowRole::from_env();
    if !enabled {
        tracing::info!("[overlay] auto_fix_kwin = false: KWin pre-start check skipped");
        return Report {
            skipped: Some("switched off".into()),
            ..Report::default()
        };
    }
    if !is_kde(|k| std::env::var(k).ok()) {
        tracing::debug!("not a KDE session: KWin pre-start check skipped");
        return Report {
            skipped: Some("not KDE".into()),
            ..Report::default()
        };
    }
    let r = run_check(&mut SessionBus, &mut DiskRc, data_dir, role, BUDGET);
    log_report(&r);
    r
}

/// One log line per action (and the D-Bus status).
pub fn log_report(r: &Report) {
    if !r.bus_ok {
        tracing::warn!(trace = ?r.trace, "KWin pre-start check: KWin D-Bus didn't answer; used kwinrc only");
    }
    for a in &r.actions {
        match a {
            Action::Excluded { effect, group, how } => tracing::warn!(
                effect,
                group,
                how,
                "KWin pre-start: excluded her from force blur (revert: lillith fix-kwin --revert)"
            ),
            Action::Fine { effect } => {
                tracing::info!(effect, "KWin pre-start: effect leaves her alone")
            }
            Action::Unloaded { effect, why } => tracing::warn!(
                effect,
                why,
                "KWin pre-start: unloaded for this session only (kwinrc unchanged)"
            ),
            Action::Failed { effect, why } => {
                tracing::warn!(
                    effect,
                    why,
                    "KWin pre-start: could not keep this effect off her"
                )
            }
        }
    }
    tracing::info!(
        elapsed_ms = r.elapsed.as_millis() as u64,
        steps = ?r.trace,
        "KWin pre-start check done (before her first surface)"
    );
}

/// Undoes what the state file recorded (`lillith fix-kwin --revert`).
/// Returns the groups restored.
pub fn revert(data_dir: &Path, rc: &mut dyn KwinRc) -> Vec<String> {
    let path = data_dir.join(STATE_FILE);
    let Ok(state) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut done = Vec::new();
    for line in state.lines() {
        let Some((group, what)) = line.split_once('\t') else {
            continue;
        };
        let text = rc.text();
        let ini = Ini { text: &text };
        let cur = ini.get(group, "WindowClasses").unwrap_or_default();
        let has = cur.lines().any(|l| l.trim() == CLASS);
        let new = match what.trim() {
            "added" if has => Some(
                cur.lines()
                    .filter(|l| l.trim() != CLASS)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            "removed" if !has => Some(if cur.is_empty() {
                CLASS.to_string()
            } else {
                format!("{}\n{CLASS}", cur.trim_end_matches('\n'))
            }),
            _ => None,
        };
        if let Some(n) = new {
            if rc.write(group, "WindowClasses", &n, deadline).is_ok() {
                done.push(group.to_string());
            }
        }
    }
    let _ = std::fs::remove_file(&path);
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Fake KWin: canned `loadedEffects`, optional hang, records calls.
    struct FakeBus {
        loaded: Result<Vec<String>, BusError>,
        hang: bool,
        calls: Vec<String>,
    }
    impl FakeBus {
        fn with(ids: &[&str]) -> Self {
            Self {
                loaded: Ok(ids.iter().map(|s| s.to_string()).collect()),
                hang: false,
                calls: Vec::new(),
            }
        }
        fn wait(&self, deadline: Instant) -> Result<(), BusError> {
            if self.hang {
                // Like run_until: give up at the deadline.
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                return Err(BusError::Timeout);
            }
            Ok(())
        }
    }
    impl KwinBus for FakeBus {
        fn loaded_effects(&mut self, d: Instant) -> Result<Vec<String>, BusError> {
            self.calls.push("loadedEffects".into());
            self.wait(d)?;
            self.loaded.clone()
        }
        fn reconfigure_effect(&mut self, id: &str, d: Instant) -> Result<(), BusError> {
            self.calls.push(format!("reconfigureEffect {id}"));
            self.wait(d)
        }
        fn reconfigure(&mut self, d: Instant) -> Result<(), BusError> {
            self.calls.push("reconfigure".into());
            self.wait(d)
        }
        fn unload_effect(&mut self, id: &str, d: Instant) -> Result<(), BusError> {
            self.calls.push(format!("unloadEffect {id}"));
            self.wait(d)
        }
    }

    /// Fake kwinrc: base text + key overrides written by the check.
    struct FakeRc {
        base: String,
        writes: BTreeMap<(String, String), String>,
        writable: bool,
    }
    impl FakeRc {
        fn new(base: &str) -> Self {
            Self {
                base: base.into(),
                writes: BTreeMap::new(),
                writable: true,
            }
        }
    }
    impl KwinRc for FakeRc {
        fn text(&self) -> String {
            let mut t = self.base.clone();
            for ((g, k), v) in &self.writes {
                t.push_str(&format!("\n[{g}]\n{k}={}\n", v.replace('\n', "\\n")));
            }
            t
        }
        fn write(&mut self, g: &str, k: &str, v: &str, _: Instant) -> Result<(), String> {
            if !self.writable {
                return Err("kwriteconfig6: not found".into());
            }
            self.writes.insert((g.into(), k.into()), v.into());
            Ok(())
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lillith-kwin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const DX_BLUR_ALL: &str = "[Plugins]\nbetter_blur_dxEnabled=true\n\n[Effect-better-blur-dx]\nBlurMatching=false\nWindowClasses=plasmashell\\nkrunner\n";

    #[test]
    fn blur_all_mode_adds_her_and_reconfigures_before_returning() {
        let dir = tmp("all");
        let mut bus = FakeBus::with(&["blur", "better_blur_dx", "slide"]);
        let mut rc = FakeRc::new(DX_BLUR_ALL);
        let r = run_check(&mut bus, &mut rc, Some(&dir), WindowRole::Dock, BUDGET);
        assert!(r.bus_ok);
        assert_eq!(
            rc.writes[&("Effect-better-blur-dx".into(), "WindowClasses".into())],
            "plasmashell\nkrunner\nlillith"
        );
        assert!(
            matches!(&r.actions[0], Action::Excluded { how: "added", .. }),
            "{:?}",
            r.actions
        );
        assert_eq!(
            bus.calls,
            [
                "loadedEffects",
                "reconfigureEffect better_blur_dx",
                "reconfigure"
            ]
        );
        // Order: write → reconfigure → settle → done (then the window maps).
        let pos = |s: &str| r.trace.iter().position(|t| t.starts_with(s)).unwrap();
        assert!(pos("write:") < pos("reconfigureEffect:") && pos("settled") < pos("check:done"));
        assert_eq!(
            std::fs::read_to_string(dir.join(STATE_FILE)).unwrap(),
            "Effect-better-blur-dx\tadded\n"
        );
        assert!(r.notice.as_deref().unwrap().contains("fix-kwin --revert"));
        // Second start: already excluded → nothing written, no notice.
        let mut bus2 = FakeBus::with(&["better_blur_dx"]);
        let r2 = run_check(&mut bus2, &mut rc, Some(&dir), WindowRole::Dock, BUDGET);
        assert_eq!(
            r2.actions,
            [Action::Fine {
                effect: "Better Blur DX".into()
            }]
        );
        assert_eq!(bus2.calls, ["loadedEffects"]);
        assert!(r2.notice.is_none());
        // Revert puts the list back exactly.
        assert_eq!(revert(&dir, &mut rc), ["Effect-better-blur-dx"]);
        let t = rc.text();
        assert_eq!(
            Ini { text: &t }
                .get("Effect-better-blur-dx", "WindowClasses")
                .unwrap(),
            "plasmashell\nkrunner"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn blur_only_mode_drops_her_line_or_leaves_it_alone() {
        let listed = "[Plugins]\nforceblurEnabled=true\n[Effect-blurplus]\nBlurMatching=true\nWindowClasses=kate\\nlillith\n";
        let mut bus = FakeBus::with(&["forceblur"]);
        let mut rc = FakeRc::new(listed);
        let r = run_check(&mut bus, &mut rc, None, WindowRole::Normal, BUDGET);
        assert_eq!(
            rc.writes[&("Effect-blurplus".into(), "WindowClasses".into())],
            "kate"
        );
        assert!(matches!(
            &r.actions[0],
            Action::Excluded { how: "removed", .. }
        ));
        // Not listed in blur-only mode: nothing to do, no D-Bus besides the query.
        let mut bus = FakeBus::with(&["forceblur"]);
        let mut rc = FakeRc::new(&listed.replace("\\nlillith", ""));
        let r = run_check(&mut bus, &mut rc, None, WindowRole::Normal, BUDGET);
        assert!(rc.writes.is_empty() && r.notice.is_none());
        assert_eq!(bus.calls, ["loadedEffects"]);
    }

    #[test]
    fn regex_covering_her_unloads_for_the_session_and_tells_ko_once() {
        let dir = tmp("regex");
        let rx = "[Plugins]\nforceblurEnabled=true\n[Effect-blurplus]\nBlurMatching=true\nWindowClasses=/.*/\n";
        let mut bus = FakeBus::with(&["forceblur"]);
        let mut rc = FakeRc::new(rx);
        let r = run_check(&mut bus, &mut rc, Some(&dir), WindowRole::Normal, BUDGET);
        assert!(rc.writes.is_empty(), "no persistent change");
        assert_eq!(bus.calls, ["loadedEffects", "unloadEffect forceblur"]);
        assert!(matches!(&r.actions[0], Action::Unloaded { .. }));
        let n = r.notice.unwrap();
        assert!(
            n.contains("this session only") && n.contains("System Settings"),
            "{n}"
        );
        // Next start: unloaded again, but no second chat note.
        let mut bus = FakeBus::with(&["forceblur"]);
        let r = run_check(&mut bus, &mut rc, Some(&dir), WindowRole::Normal, BUDGET);
        assert_eq!(bus.calls, ["loadedEffects", "unloadEffect forceblur"]);
        assert!(r.notice.is_none());
        // Blur-all mode with a regex that covers her = she's excluded.
        let ok = rx
            .replace("BlurMatching=true", "BlurMatching=false")
            .replace("/.*/", "/^lil+ith$/");
        let mut bus = FakeBus::with(&["forceblur"]);
        let r = run_check(
            &mut bus,
            &mut FakeRc::new(&ok),
            None,
            WindowRole::Normal,
            BUDGET,
        );
        assert_eq!(
            r.actions,
            [Action::Fine {
                effect: "Better Blur".into()
            }]
        );
        // An unknown effect with a blur-all list is unloaded too.
        let unk = "[Effect-glassy-blur]\nBlurMatching=false\nWindowClasses=vlc\n";
        let mut bus = FakeBus::with(&["blur", "glassy_blur"]);
        let r = run_check(
            &mut bus,
            &mut FakeRc::new(unk),
            None,
            WindowRole::Dock,
            BUDGET,
        );
        assert_eq!(bus.calls, ["loadedEffects", "unloadEffect glassy_blur"]);
        assert!(matches!(&r.actions[0], Action::Unloaded { .. }));
        // No kwriteconfig6 → can't exclude → session unload.
        let mut bus = FakeBus::with(&["better_blur_dx"]);
        let mut rc = FakeRc::new(DX_BLUR_ALL);
        rc.writable = false;
        let r = run_check(&mut bus, &mut rc, None, WindowRole::Normal, BUDGET);
        assert_eq!(bus.calls, ["loadedEffects", "unloadEffect better_blur_dx"]);
        assert!(matches!(&r.actions[0], Action::Unloaded { .. }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn no_kde_means_no_check() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|p| p.0 == k).map(|p| p.1.to_string())
        };
        assert!(!is_kde(env(&[("XDG_CURRENT_DESKTOP", "sway")])));
        assert!(!is_kde(env(&[])));
        assert!(is_kde(env(&[("XDG_CURRENT_DESKTOP", "KDE")])));
        assert!(is_kde(env(&[("XDG_CURRENT_DESKTOP", "garuda:KDE")])));
        assert!(is_kde(env(&[("KDE_FULL_SESSION", "true")])));
        // Switched off: nothing runs (and nothing touches the bus).
        let r = before_first_frame(false, None);
        assert_eq!(r.skipped.as_deref(), Some("switched off"));
        assert!(r.trace.is_empty());
    }

    #[test]
    fn dbus_timeout_never_hangs_startup() {
        let dir = tmp("timeout");
        let mut bus = FakeBus::with(&["better_blur_dx"]);
        bus.hang = true;
        let mut rc = FakeRc::new(DX_BLUR_ALL);
        let t0 = Instant::now();
        let r = run_check(&mut bus, &mut rc, Some(&dir), WindowRole::Dock, BUDGET);
        assert!(
            t0.elapsed() < Duration::from_millis(1500),
            "{:?}",
            t0.elapsed()
        );
        assert!(!r.bus_ok);
        // kwinrc still fixed (from [Plugins]) so the next KWin start is clean;
        // no further D-Bus calls once the bus timed out.
        assert_eq!(bus.calls, ["loadedEffects"]);
        assert_eq!(
            rc.writes[&("Effect-better-blur-dx".into(), "WindowClasses".into())],
            "plasmashell\nkrunner\nlillith"
        );
        // The real helper also gives up at the deadline.
        let t0 = Instant::now();
        let mut c = Command::new("sleep");
        c.arg("5");
        let e = run_until(&mut c, Instant::now() + Duration::from_millis(200));
        assert_eq!(e, Err(BusError::Timeout));
        assert!(t0.elapsed() < Duration::from_millis(1000));
        // Missing helper = unavailable, immediately.
        let mut c = Command::new("lillith-no-such-dbus-helper");
        assert!(matches!(
            run_until(&mut c, Instant::now() + BUDGET),
            Err(BusError::Unavailable(_))
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn unfixable_without_dbus_tells_ko_once_and_never_for_a_dock() {
        let dir = tmp("unfixable");
        // Dock: the effect skips her anyway, so a failed write is no news.
        let mut rc = FakeRc::new(DX_BLUR_ALL);
        rc.writable = false;
        let mut bus = FakeBus::with(&[]);
        bus.loaded = Err(BusError::Unavailable("no bus".into()));
        let r = run_check(&mut bus, &mut rc, Some(&dir), WindowRole::Dock, BUDGET);
        assert!(r.notice.is_none(), "{:?}", r.notice);
        assert!(matches!(&r.actions[..], [Action::Fine { .. }]), "{:?}", r.actions);
        // Normal window, no D-Bus to unload: told once, not every start.
        let r = run_check(&mut bus, &mut rc, Some(&dir), WindowRole::Normal, BUDGET);
        assert!(r.notice.as_deref().unwrap().contains("Better Blur DX"));
        let r = run_check(&mut bus, &mut rc, Some(&dir), WindowRole::Normal, BUDGET);
        assert!(r.notice.is_none(), "only once");
        assert!(matches!(&r.actions[..], [Action::Failed { .. }]), "still logged");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn parses_dbus_send_and_qdbus_output() {
        let ds = "method return time=1 sender=:1.5 -> destination=:1.9 serial=7 reply_serial=2\n   variant       array [\n         string \"blur\"\n         string \"better_blur_dx\"\n      ]\n";
        assert_eq!(parse_string_list(ds), ["blur", "better_blur_dx"]);
        assert_eq!(
            parse_string_list("blur\nbetter_blur_dx\n"),
            ["blur", "better_blur_dx"]
        );
        assert!(parse_string_list("").is_empty());
    }
}
