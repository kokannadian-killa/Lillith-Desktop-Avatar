//! Pre-start KWin check: keep force-blur effects off her *before* her first
//! surface is mapped, so Ko never sees the dark blurred box, not even for
//! one frame.
//!
//! Called by the app before the overlay backend (Wayland or X11) creates
//! her window (`[overlay] auto_fix_kwin`, default on). On KDE it:
//!
//! 1. asks KWin which effects are loaded (`org.kde.kwin.Effects`
//!    `loadedEffects` over D-Bus, a read-only query); falls back to
//!    `[Plugins] …Enabled` in kwinrc when D-Bus doesn't answer;
//! 2. plans the changes below **without running anything** ([`plan_check`])
//!    and, if there is anything to change, shows Ko a mouse-only Soul
//!    Contract card (`lillith contract-card`, Yes is `clicked_by(Primary)`
//!    only and not focusable) with the exact kwinrc diff (keys before and
//!    after) and every command, one argument per line. Audit lines
//!    `PROPOSED` / `CONFIRM` / `DENY` / `TIMEOUT` / `CANCELLED` / `NO_UI`
//!    carry `card=<id>`. Only Yes applies the plan ([`apply_plan`]); No, a
//!    timeout, a closed card or no card UI skip the tweak (nothing written,
//!    nothing reloaded) and the overlay still starts;
//! 3. on Yes, for every loaded force-blur effect (Better Blur DX `better_blur_dx`,
//!    Better Blur `forceblur`, `blurplus`) whose `WindowClasses` would blur
//!    her, edits the list like `lillith fix-kwin` (blur-all-except: add
//!    `lillith`; blur-only: drop the `lillith` line), records the change
//!    in the same state file `kwin-blur-exclude.sh revert` /
//!    `lillith fix-kwin --revert` undo, and reconfigures that effect
//!    (`reconfigureEffect`, synchronous inside KWin) plus KWin;
//! 4. when that's impossible (a `/regex/` in blur-only mode that covers her,
//!    an unknown effect whose config group has a blur-all `WindowClasses`
//!    list, no `kwriteconfig6`), unloads the effect **for this session
//!    only** (`unloadEffect`, kwinrc untouched) and leaves a one-time note
//!    for the chat saying what was done and how to make it permanent.
//!
//! Never blocks startup for long: every D-Bus/helper call shares one
//! deadline ([`BUDGET`], 1.4 s, fresh again after a Yes) and is killed when
//! it runs out (the card itself waits up to 60 s for Ko); without
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
    /// Needed a fix, but Ko didn't approve the card (No, timeout, no UI):
    /// nothing was changed.
    Skipped { effect: String, why: String },
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

/// A kwinrc `WindowClasses` edit the check wants to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedWrite {
    /// KWin plugin id (`better_blur_dx`, …).
    pub id: String,
    /// Display name (`Better Blur DX`).
    pub effect: String,
    pub group: String,
    /// Current value (`\n`-separated list).
    pub before: String,
    /// Value after the edit.
    pub after: String,
    /// `added` (blur-all list) or `removed` (blur-only list).
    pub how: &'static str,
    /// The effect would really draw behind her (a failed write then falls
    /// back to a session unload, as listed on the card).
    pub hits_her: bool,
}

/// What the check would change, worked out without changing anything:
/// the only command run to build it is the read-only `loadedEffects`
/// query. Nothing here is applied until [`apply_plan`] (after a Yes).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub bus_ok: bool,
    /// kwinrc edits (persistent, revertible).
    pub writes: Vec<PlannedWrite>,
    /// Session-only unloads `(id, why)` (only when the bus answered).
    pub unload: Vec<(String, String)>,
    /// Effects already fine, or unfixable without D-Bus (nothing to run).
    pub actions: Vec<Action>,
    pub trace: Vec<String>,
}

impl Plan {
    /// Nothing to change: no card is needed.
    pub fn is_empty(&self) -> bool {
        self.writes.is_empty() && self.unload.is_empty()
    }

    /// The card's one-line summary.
    pub fn summary(&self) -> String {
        let mut names: Vec<String> = self.writes.iter().map(|w| w.effect.clone()).collect();
        for (id, _) in &self.unload {
            names.push(effect_name(id));
        }
        names.dedup();
        format!(
            "keep {} off her window: edit your KDE kwinrc and reload KWin's effects",
            names.join(", ")
        )
    }

    /// The card's detail: the exact kwinrc change (each key before and
    /// after) and every command that would run, one argument per line.
    pub fn card_detail(&self) -> String {
        let mut out = String::new();
        let show = |v: &str| -> String {
            if v.is_empty() {
                "    (empty)".to_string()
            } else {
                v.lines()
                    .map(|l| format!("    {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        };
        for w in &self.writes {
            out.push_str(&format!(
                "kwinrc [{}] WindowClasses ({})\n- before:\n{}\n+ after:\n{}\n\n",
                w.group,
                w.effect,
                show(&w.before),
                show(&w.after)
            ));
        }
        let mut cmds: Vec<Vec<String>> = Vec::new();
        for w in &self.writes {
            cmds.push(kwrite_argv(&w.group, "WindowClasses", &w.after));
        }
        if self.bus_ok {
            for w in &self.writes {
                cmds.push(dbus_argv(
                    "/Effects",
                    "org.kde.kwin.Effects.reconfigureEffect",
                    &[format!("string:{}", w.id)],
                    None,
                ));
            }
            if !self.writes.is_empty() {
                cmds.push(dbus_argv("/KWin", "org.kde.KWin.reconfigure", &[], None));
            }
            for (id, _) in &self.unload {
                cmds.push(unload_argv(id));
            }
        }
        out.push_str("Commands (in order):\n");
        for c in &cmds {
            out.push_str(&argv_lines(c));
            out.push('\n');
        }
        let fallback: Vec<&PlannedWrite> = self
            .writes
            .iter()
            .filter(|w| w.hits_her && self.bus_ok)
            .collect();
        if !fallback.is_empty() {
            out.push_str("\nIf writing kwinrc fails, instead (this session only):\n");
            for w in fallback {
                out.push_str(&argv_lines(&unload_argv(&w.id)));
                out.push('\n');
            }
        }
        for (id, why) in &self.unload {
            out.push_str(&format!("\nUnload {id}: {why}"));
        }
        out.push_str(
            "\n(qdbus6 is used with the same arguments if dbus-send is missing.)\n\
             No: nothing is changed and she starts anyway. \
             Set [overlay] auto_fix_kwin = false to stop this check.",
        );
        out
    }
}

/// `$ <program>` then one argument per line (newlines in a value shown
/// as `\n`), never shortened.
pub fn argv_lines(argv: &[String]) -> String {
    let mut it = argv.iter();
    let mut s = format!("$ {}", it.next().map(String::as_str).unwrap_or(""));
    for a in it {
        s.push_str("\n    ");
        s.push_str(&a.replace('\n', "\\n"));
    }
    s
}

fn effect_name(id: &str) -> String {
    BLUR_EFFECTS
        .iter()
        .find(|e| e.plugin_id == id)
        .map(|e| e.name.to_string())
        .unwrap_or_else(|| id.to_string())
}

/// `kwriteconfig6 --file kwinrc --group G --key K -- V`.
pub fn kwrite_argv(group: &str, key: &str, value: &str) -> Vec<String> {
    [
        "kwriteconfig6",
        "--file",
        "kwinrc",
        "--group",
        group,
        "--key",
        key,
        "--",
        value,
    ]
    .map(str::to_string)
    .to_vec()
}

/// `dbus-send --session --print-reply --reply-timeout=MS --dest=org.kde.KWin PATH METHOD ARGS…`
/// (`ms` `None`: shown as `<ms left>` on the card).
pub fn dbus_argv(path: &str, method: &str, args: &[String], ms: Option<u128>) -> Vec<String> {
    let mut v = vec![
        "dbus-send".to_string(),
        "--session".into(),
        "--print-reply".into(),
        match ms {
            Some(ms) => format!("--reply-timeout={ms}"),
            None => "--reply-timeout=<ms left of the 1.4 s budget>".into(),
        },
        "--dest=org.kde.KWin".into(),
        path.into(),
        method.into(),
    ];
    v.extend(args.iter().cloned());
    v
}

fn unload_argv(id: &str) -> Vec<String> {
    dbus_argv(
        "/Effects",
        "org.kde.kwin.Effects.unloadEffect",
        &[format!("string:{id}")],
        None,
    )
}

/// Works out what the check would change (see [`Plan`]); writes nothing.
pub fn plan_check(bus: &mut dyn KwinBus, text: &str, role: WindowRole, deadline: Instant) -> Plan {
    let mut p = Plan::default();
    p.trace.push("check:start".into());
    let ini = Ini { text };
    let loaded = match bus.loaded_effects(deadline) {
        Ok(l) => {
            p.bus_ok = true;
            l
        }
        Err(e) => {
            p.trace.push(format!("bus:{e:?}"));
            // Fall back to what kwinrc says is enabled (known effects only).
            BLUR_EFFECTS
                .iter()
                .filter(|e| ini.bool("Plugins", e.plugin_key) == Some(true))
                .map(|e| e.plugin_id.to_string())
                .collect()
        }
    };
    p.trace.push(format!("loaded:{}", loaded.join(",")));
    let mut unload: Vec<(String, String)> = Vec::new();
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
                p.actions.push(Action::Fine {
                    effect: e.name.into(),
                });
                continue;
            }
            let docks = ini.bool(group, "BlurDocks") == Some(true);
            let hits_her = role != WindowRole::Dock || docks;
            match exclude_her(&classes, blacklist) {
                Some(new) => p.writes.push(PlannedWrite {
                    id: id.clone(),
                    effect: e.name.into(),
                    group: group.into(),
                    before: classes.clone(),
                    after: new,
                    how: if blacklist { "added" } else { "removed" },
                    hits_her,
                }),
                None if hits_her => unload.push((
                    id.clone(),
                    format!(
                        "a /regex/ in [{group}] WindowClasses (blur only listed) covers `{CLASS}`"
                    ),
                )),
                None => p.actions.push(Action::Fine {
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
                p.actions.push(Action::Fine { effect: id.clone() });
            }
            break;
        }
    }
    for (id, why) in unload {
        if p.bus_ok {
            p.unload.push((id, why));
        } else {
            // Nothing could run: logged / told once, no card needed.
            p.actions.push(Action::Failed {
                effect: effect_name(&id),
                why: format!("{why}; KWin's D-Bus didn't answer, so it couldn't be unloaded"),
            });
        }
    }
    p
}

/// Applies a [`Plan`] Ko said Yes to: the kwinrc writes (recorded in the
/// state file for `--revert`), the effect reloads and the session unloads.
pub fn apply_plan(
    plan: Plan,
    bus: &mut dyn KwinBus,
    rc: &mut dyn KwinRc,
    data_dir: Option<&Path>,
    deadline: Instant,
) -> Report {
    let mut r = Report {
        bus_ok: plan.bus_ok,
        actions: plan.actions,
        trace: plan.trace,
        ..Report::default()
    };
    let mut reconfigure: Vec<String> = Vec::new();
    let mut unload = plan.unload;
    let mut state_lines: Vec<String> = Vec::new();
    for w in plan.writes {
        match rc.write(&w.group, "WindowClasses", &w.after, deadline) {
            Ok(()) => {
                r.trace.push(format!("write:{}", w.group));
                state_lines.push(format!("{}\t{}", w.group, w.how));
                r.actions.push(Action::Excluded {
                    effect: w.effect,
                    group: w.group,
                    how: w.how,
                });
                reconfigure.push(w.id);
            }
            Err(why) if w.hits_her => unload.push((w.id, why)),
            Err(why) => {
                // Her dock window type keeps this effect off her
                // anyway: log it, don't bother Ko.
                r.trace.push(format!("write-failed:{}:{why}", w.group));
                r.actions.push(Action::Fine { effect: w.effect });
            }
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
        let name = effect_name(&id);
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
    r.trace.push("check:done".into());
    r
}

/// Ko's answer to the KWin card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consent {
    /// A mouse click on the card's Yes.
    Yes,
    /// No / Esc.
    No,
    /// Not answered in time.
    TimedOut,
    /// The card window was closed.
    Cancelled,
    /// No card could be shown (no display, no card helper).
    NoUi,
}

impl Consent {
    /// The audit tag of the decision.
    pub fn audit_tag(self) -> &'static str {
        match self {
            Consent::Yes => "CONFIRM",
            Consent::No => "DENY",
            Consent::TimedOut => "TIMEOUT",
            Consent::Cancelled => "CANCELLED",
            Consent::NoUi => "NO_UI",
        }
    }
}

/// The `kind` column of the KWin card's audit lines.
pub const CARD_KIND: &str = "kwin_fix";

/// The gated check: plans, and when anything would change, audits
/// `PROPOSED` (card=`card`), asks `ask` (the mouse-only Soul Contract
/// card), audits the decision and applies the plan only on [`Consent::Yes`].
/// No, timeout, a closed card or no UI change nothing (she starts anyway).
/// `audit_log` `None` skips the audit lines (tests without a log).
#[allow(clippy::too_many_arguments)]
pub fn run_check_gated(
    bus: &mut dyn KwinBus,
    rc: &mut dyn KwinRc,
    data_dir: Option<&Path>,
    role: WindowRole,
    budget: Duration,
    audit_log: Option<&Path>,
    card: &str,
    ask: &mut dyn FnMut(&Plan) -> Consent,
) -> Report {
    let t0 = Instant::now();
    let text = rc.text();
    let plan = plan_check(bus, &text, role, t0 + budget);
    let audit = |tag: &str, plan: &Plan| {
        if let Some(log) = audit_log {
            let what: Vec<String> = std::iter::once("kwriteconfig6".to_string())
                .chain(plan.writes.iter().map(|w| format!("[{}]", w.group)))
                .chain(plan.unload.iter().map(|(id, _)| format!("unload:{id}")))
                .collect();
            lillith_core::launch_audit::record_to(log, tag, CARD_KIND, &what, Some(card));
        }
    };
    if plan.is_empty() {
        let mut r = apply_plan(plan, bus, rc, data_dir, t0 + budget);
        r.elapsed = t0.elapsed();
        return r;
    }
    audit("PROPOSED", &plan);
    let consent = ask(&plan);
    audit(consent.audit_tag(), &plan);
    if consent != Consent::Yes {
        let mut r = Report {
            bus_ok: plan.bus_ok,
            actions: plan.actions,
            trace: plan.trace,
            ..Report::default()
        };
        r.trace.push(format!("card:{}", consent.audit_tag()));
        for w in plan.writes {
            r.actions.push(Action::Skipped {
                effect: w.effect,
                why: format!("not approved ({})", consent.audit_tag()),
            });
        }
        for (id, _) in plan.unload {
            r.actions.push(Action::Skipped {
                effect: effect_name(&id),
                why: format!("not approved ({})", consent.audit_tag()),
            });
        }
        r.trace.push("check:done".into());
        r.elapsed = t0.elapsed();
        return r;
    }
    // A fresh budget for the approved changes (the card took its time).
    let mut r = CARD.with(|c| {
        *c.borrow_mut() = Some(card.to_string());
        let r = apply_plan(plan, bus, rc, data_dir, Instant::now() + budget);
        *c.borrow_mut() = None;
        r
    });
    r.trace.insert(r.trace.len() - 1, "card:CONFIRM".into());
    r.elapsed = t0.elapsed();
    r
}

/// The ungated check (tests only: the app always goes through
/// [`run_check_gated`]).
#[cfg(test)]
pub fn run_check(
    bus: &mut dyn KwinBus,
    rc: &mut dyn KwinRc,
    data_dir: Option<&Path>,
    role: WindowRole,
    budget: Duration,
) -> Report {
    let t0 = Instant::now();
    let deadline = t0 + budget;
    let text = rc.text();
    let plan = plan_check(bus, &text, role, deadline);
    let mut r = apply_plan(plan, bus, rc, data_dir, deadline);
    r.elapsed = t0.elapsed();
    r
}

thread_local! {
    /// The approved card while its plan is applied: launches carry
    /// `card=<id>` in the audit log.
    static CARD: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
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

/// Runs `cmd` until it exits or `deadline` passes (then kills it). Each
/// launch writes one audit line (`card=<id>` while an approved KWin card's
/// plan is applied, else `no-card`: the read-only `loadedEffects` query
/// and `fix-kwin --revert`).
fn run_until(cmd: &mut Command, deadline: Instant) -> Result<String, BusError> {
    if Instant::now() >= deadline {
        return Err(BusError::Timeout);
    }
    let argv = lillith_core::launch_audit::argv_of_command(cmd);
    if let Some(log) = lillith_core::config::audit_log_path() {
        CARD.with(|c| {
            lillith_core::launch_audit::record_to(
                &log,
                lillith_core::launch_audit::LAUNCH_TAG,
                "kwin_prestart",
                &argv,
                c.borrow().as_deref(),
            )
        });
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
        let argv = dbus_argv(path, method, args, Some(Self::ms_left(deadline)));
        let mut c = Command::new(&argv[0]);
        c.args(&argv[1..]);
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
        let argv = kwrite_argv(group, key, value);
        let mut c = Command::new(&argv[0]);
        c.args(&argv[1..]);
        run_until(&mut c, deadline)
            .map(drop)
            .map_err(|e| format!("kwriteconfig6: {e:?}"))
    }
}

/// The app's entry point: no-op (logged) off KDE or when switched off;
/// otherwise [`run_check_gated`] on the real bus/kwinrc with the
/// stand-alone mouse-only Soul Contract card (Ko's decision: the kwinrc
/// edit and effect reloads need his Yes; No, a timeout or no UI skip the
/// tweak and she starts anyway), logging every action.
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
    let card = lillith_core::card_process::new_card_id("kwin");
    let log = lillith_core::config::audit_log_path();
    let r = run_check_gated(
        &mut SessionBus,
        &mut DiskRc,
        data_dir,
        role,
        BUDGET,
        log.as_deref(),
        &card,
        &mut |plan| ask_card(plan, &card),
    );
    log_report(&r);
    r
}

/// How long the KWin card waits for Ko before it counts as a timeout.
pub const CARD_TIMEOUT: Duration = Duration::from_secs(60);

/// Shows the KWin card (`lillith contract-card`, this same binary).
fn ask_card(plan: &Plan, card: &str) -> Consent {
    use lillith_core::card_process::{ask, CardAnswer, CardText};
    let Ok(exe) = std::env::current_exe() else {
        return Consent::NoUi;
    };
    let text = CardText {
        title: "Change KDE's KWin settings?".into(),
        summary: plan.summary(),
        target: format!("~/.config/kwinrc (card {card})"),
        detail: plan.card_detail(),
    };
    match ask(&exe, &text, CARD_TIMEOUT) {
        CardAnswer::Yes => Consent::Yes,
        CardAnswer::No => Consent::No,
        CardAnswer::TimedOut => Consent::TimedOut,
        CardAnswer::Cancelled => Consent::Cancelled,
        CardAnswer::NoUi => Consent::NoUi,
    }
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
            Action::Skipped { effect, why } => tracing::info!(
                effect,
                why,
                "KWin pre-start: change not approved on the Soul Contract card; nothing changed"
            ),
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
    fn kwin_card_no_timeout_or_no_ui_writes_nothing() {
        for c in [
            Consent::No,
            Consent::TimedOut,
            Consent::Cancelled,
            Consent::NoUi,
        ] {
            let dir = tmp("card-no");
            let log = dir.join("audit.log");
            let mut bus = FakeBus::with(&["better_blur_dx"]);
            let mut rc = FakeRc::new(DX_BLUR_ALL);
            let mut asked = 0;
            let r = run_check_gated(
                &mut bus,
                &mut rc,
                Some(&dir),
                WindowRole::Normal,
                BUDGET,
                Some(&log),
                "kwin-t1",
                &mut |p| {
                    asked += 1;
                    assert_eq!(p.writes.len(), 1);
                    c
                },
            );
            assert_eq!(asked, 1);
            assert!(rc.writes.is_empty(), "{c:?}: no kwinrc write");
            assert_eq!(bus.calls, ["loadedEffects"], "{c:?}: no reload");
            assert!(!dir.join(STATE_FILE).exists());
            assert!(
                matches!(&r.actions[..], [Action::Skipped { .. }]),
                "{:?}",
                r.actions
            );
            assert!(r.notice.is_none());
            let audit = std::fs::read_to_string(&log).unwrap();
            let lines: Vec<&str> = audit.lines().collect();
            assert_eq!(lines.len(), 2, "{audit}");
            let cols: Vec<&str> = lines[0].split('\t').collect();
            assert_eq!(
                &cols[1..5],
                ["PROPOSED", CARD_KIND, "kwriteconfig6", "card=kwin-t1"]
            );
            let cols: Vec<&str> = lines[1].split('\t').collect();
            assert_eq!(cols[1], c.audit_tag());
            assert_eq!(cols[4], "card=kwin-t1");
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn kwin_card_yes_applies_and_nothing_to_change_never_asks() {
        let dir = tmp("card-yes");
        let log = dir.join("audit.log");
        let mut bus = FakeBus::with(&["better_blur_dx"]);
        let mut rc = FakeRc::new(DX_BLUR_ALL);
        let r = run_check_gated(
            &mut bus,
            &mut rc,
            Some(&dir),
            WindowRole::Normal,
            BUDGET,
            Some(&log),
            "kwin-t2",
            &mut |_| Consent::Yes,
        );
        assert_eq!(
            rc.writes[&("Effect-better-blur-dx".into(), "WindowClasses".into())],
            "plasmashell\nkrunner\nlillith"
        );
        assert_eq!(
            bus.calls,
            [
                "loadedEffects",
                "reconfigureEffect better_blur_dx",
                "reconfigure"
            ]
        );
        assert!(matches!(&r.actions[..], [Action::Excluded { .. }]));
        let audit = std::fs::read_to_string(&log).unwrap();
        assert!(audit.lines().nth(1).unwrap().contains("\tCONFIRM\t"));
        // Already fine: no card, no audit line from the gate.
        let log2 = dir.join("audit2.log");
        let mut bus = FakeBus::with(&["better_blur_dx"]);
        let r = run_check_gated(
            &mut bus,
            &mut rc,
            Some(&dir),
            WindowRole::Normal,
            BUDGET,
            Some(&log2),
            "kwin-t3",
            &mut |_| panic!("nothing to change: no card"),
        );
        assert!(matches!(&r.actions[..], [Action::Fine { .. }]));
        assert!(!log2.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn kwin_card_shows_the_exact_diff_and_one_arg_per_line() {
        let mut bus = FakeBus::with(&["better_blur_dx"]);
        let p = plan_check(
            &mut bus,
            DX_BLUR_ALL,
            WindowRole::Normal,
            Instant::now() + BUDGET,
        );
        let d = p.card_detail();
        assert!(
            d.contains("kwinrc [Effect-better-blur-dx] WindowClasses (Better Blur DX)\n- before:\n    plasmashell\n    krunner\n+ after:\n    plasmashell\n    krunner\n    lillith\n"),
            "{d}"
        );
        assert!(
            d.contains("$ kwriteconfig6\n    --file\n    kwinrc\n    --group\n    Effect-better-blur-dx\n    --key\n    WindowClasses\n    --\n    plasmashell\\nkrunner\\nlillith\n"),
            "{d}"
        );
        assert!(d.contains("$ dbus-send\n    --session\n"), "{d}");
        assert!(d.contains("    org.kde.kwin.Effects.reconfigureEffect\n    string:better_blur_dx"));
        assert!(d.contains("    org.kde.KWin.reconfigure"));
        assert!(
            d.contains("unloadEffect"),
            "write-failure fallback is listed"
        );
        assert!(p.summary().contains("Better Blur DX"));
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
        assert!(
            matches!(&r.actions[..], [Action::Fine { .. }]),
            "{:?}",
            r.actions
        );
        // Normal window, no D-Bus to unload: told once, not every start.
        let r = run_check(&mut bus, &mut rc, Some(&dir), WindowRole::Normal, BUDGET);
        assert!(r.notice.as_deref().unwrap().contains("Better Blur DX"));
        let r = run_check(&mut bus, &mut rc, Some(&dir), WindowRole::Normal, BUDGET);
        assert!(r.notice.is_none(), "only once");
        assert!(
            matches!(&r.actions[..], [Action::Failed { .. }]),
            "still logged"
        );
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
