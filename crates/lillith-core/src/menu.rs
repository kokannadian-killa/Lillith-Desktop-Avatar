//! The right-click menu (M6), as pure data: [`build_menu`] turns the current
//! [`MenuState`] into a [`Menu`] tree with check / radio state, and
//! [`on_menu_action`] turns a picked [`MenuAction`] into the existing brain
//! [`Action`]s plus a few overlay-side [`MenuEffect`]s.
//!
//! No toolkit and no I/O here: the overlay shows the tree with whatever
//! backend it has (see `assets/processed/patches/HANDOFF.md`: a small
//! tiny-skia popup in the overlay window, or the ksni tray menu, which takes
//! the same tree) and calls back with the leaf's [`MenuAction`].
//!
//! ```text
//! Mode            ▸ (•) Stay  ( ) Wander  ( ) Follow
//! Mood            ▸ (•) Playful ( ) Happy ( ) Mischievous ( ) Alert ( ) Sleepy
//! [ ] Mute voice
//! [ ] Pause system monitor
//! Size            ▸ ( ) Small  (•) Normal  ( ) Large
//! ─────────────
//! Open settings   (xdg-open config.toml)
//! Reset position
//! About Lillith
//! ─────────────
//! Quit
//! ```
//!
//! Hook (overlay `Controller`, on a right click on her):
//!
//! ```ignore
//! let state = MenuState::from_brain(&self.brain, monitor_paused, scale);
//! let menu = build_menu(&state);           // show it; on pick:
//! let out = on_menu_action(action, &mut self.brain);
//! self.apply(out.actions);                 // ModeChanged / MoodChanged / Muted / Quit …
//! for e in out.effects { /* resize, pause feed, xdg-open, reset position, about */ }
//! ```

use crate::config::{MoveMode, OverlayPos};
use crate::mood::Mood;
use crate::state::{Action, Brain};
use std::path::{Path, PathBuf};

/// Overlay size presets (multiplier on the 400 px cell).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Small,
    Normal,
    Large,
}

impl Size {
    pub const ALL: [Size; 3] = [Size::Small, Size::Normal, Size::Large];

    pub fn scale(self) -> f32 {
        match self {
            Size::Small => 0.45,
            Size::Normal => 0.55,
            Size::Large => 0.75,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Size::Small => "Small",
            Size::Normal => "Normal",
            Size::Large => "Large",
        }
    }

    /// The preset `scale` matches (±0.01), if any (a custom `--scale`
    /// checks no radio item).
    pub fn of_scale(scale: f32) -> Option<Size> {
        Size::ALL
            .into_iter()
            .find(|s| (s.scale() - scale).abs() <= 0.01)
    }
}

/// Every mood, in menu order (lillith-core's [`Mood`]).
pub const MOODS: [Mood; 5] = [
    Mood::Playful,
    Mood::Happy,
    Mood::Mischievous,
    Mood::Alert,
    Mood::Sleepy,
];

/// Every movement mode, in menu order.
pub const MODES: [MoveMode; 3] = [MoveMode::Stay, MoveMode::Wander, MoveMode::Follow];

pub fn mood_label(m: Mood) -> &'static str {
    match m {
        Mood::Playful => "Playful",
        Mood::Happy => "Happy",
        Mood::Mischievous => "Mischievous",
        Mood::Alert => "Alert",
        Mood::Sleepy => "Sleepy",
    }
}

pub fn mode_label(m: MoveMode) -> &'static str {
    match m {
        MoveMode::Stay => "Stay",
        MoveMode::Wander => "Wander",
        MoveMode::Follow => "Follow",
    }
}

/// What a leaf does when picked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MenuAction {
    SetMode(MoveMode),
    SetMood(Mood),
    /// Mute / unmute (voice and SFX).
    ToggleMute,
    /// Pause / resume the system-monitor event feed.
    ToggleMonitor,
    SetSize(Size),
    OpenSettings,
    ResetPosition,
    About,
    Quit,
}

/// Check-mark state of a leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    None,
    /// Independent toggle.
    Checkbox(bool),
    /// One of a group (exactly one checked, or none for a custom value).
    Radio(bool),
}

/// One entry of the menu tree.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuItem {
    Leaf {
        label: String,
        action: MenuAction,
        check: Check,
        enabled: bool,
    },
    Submenu {
        label: String,
        items: Vec<MenuItem>,
    },
    Separator,
}

impl MenuItem {
    fn leaf(label: impl Into<String>, action: MenuAction, check: Check) -> Self {
        MenuItem::Leaf {
            label: label.into(),
            action,
            check,
            enabled: true,
        }
    }

    pub fn label(&self) -> Option<&str> {
        match self {
            MenuItem::Leaf { label, .. } | MenuItem::Submenu { label, .. } => Some(label),
            MenuItem::Separator => None,
        }
    }
}

/// The whole menu.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub items: Vec<MenuItem>,
}

impl Menu {
    /// Every leaf with its submenu path (e.g. `["Mood"]`), depth-first in
    /// display order. Handy for backends that need flat ids.
    pub fn leaves(&self) -> Vec<(Vec<&str>, &MenuItem)> {
        fn walk<'a>(
            items: &'a [MenuItem],
            path: &mut Vec<&'a str>,
            out: &mut Vec<(Vec<&'a str>, &'a MenuItem)>,
        ) {
            for it in items {
                match it {
                    MenuItem::Leaf { .. } => out.push((path.clone(), it)),
                    MenuItem::Submenu { label, items } => {
                        path.push(label);
                        walk(items, path, out);
                        path.pop();
                    }
                    MenuItem::Separator => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut Vec::new(), &mut out);
        out
    }

    /// The leaf for `action`, if present.
    pub fn find(&self, action: MenuAction) -> Option<&MenuItem> {
        self.leaves()
            .into_iter()
            .map(|(_, it)| it)
            .find(|it| matches!(it, MenuItem::Leaf { action: a, .. } if *a == action))
    }

    /// The check state of `action`'s leaf.
    pub fn check_of(&self, action: MenuAction) -> Option<Check> {
        match self.find(action)? {
            MenuItem::Leaf { check, .. } => Some(*check),
            _ => None,
        }
    }

    /// The action of leaf number `leaf_index` ([`Menu::leaves`] order), for
    /// backends that key callbacks by number.
    pub fn action_at(&self, leaf_index: usize) -> Option<MenuAction> {
        match self.leaves().get(leaf_index)?.1 {
            MenuItem::Leaf { action, .. } => Some(*action),
            _ => None,
        }
    }
}

/// Everything the menu shows, read from the app right before it opens.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuState {
    pub mode: MoveMode,
    pub mood: Mood,
    /// Brain mute (voice follows `Action::Muted`).
    pub muted: bool,
    pub monitor_paused: bool,
    /// Current overlay scale.
    pub scale: f32,
}

impl MenuState {
    /// Mode, mood and mute from the brain; the overlay supplies the rest.
    pub fn from_brain(brain: &Brain, monitor_paused: bool, scale: f32) -> Self {
        Self {
            mode: brain.mode(),
            mood: brain.mood(),
            muted: brain.muted(),
            monitor_paused,
            scale,
        }
    }
}

/// "About" text.
pub fn about_text() -> String {
    format!(
        "Lillith {} — a desktop companion. MIT; art: see LICENSE-ASSETS.md.",
        env!("CARGO_PKG_VERSION")
    )
}

/// HOOK 1: the menu for the current state.
pub fn build_menu(s: &MenuState) -> Menu {
    let size = Size::of_scale(s.scale);
    let items = vec![
        MenuItem::Submenu {
            label: "Mode".into(),
            items: MODES
                .iter()
                .map(|&m| {
                    MenuItem::leaf(
                        mode_label(m),
                        MenuAction::SetMode(m),
                        Check::Radio(m == s.mode),
                    )
                })
                .collect(),
        },
        MenuItem::Submenu {
            label: "Mood".into(),
            items: MOODS
                .iter()
                .map(|&m| {
                    MenuItem::leaf(
                        mood_label(m),
                        MenuAction::SetMood(m),
                        Check::Radio(m == s.mood),
                    )
                })
                .collect(),
        },
        MenuItem::leaf(
            "Mute voice",
            MenuAction::ToggleMute,
            Check::Checkbox(s.muted),
        ),
        MenuItem::leaf(
            "Pause system monitor",
            MenuAction::ToggleMonitor,
            Check::Checkbox(s.monitor_paused),
        ),
        MenuItem::Submenu {
            label: "Size".into(),
            items: Size::ALL
                .iter()
                .map(|&z| {
                    MenuItem::leaf(
                        z.label(),
                        MenuAction::SetSize(z),
                        Check::Radio(Some(z) == size),
                    )
                })
                .collect(),
        },
        MenuItem::Separator,
        MenuItem::leaf("Open settings", MenuAction::OpenSettings, Check::None),
        MenuItem::leaf("Reset position", MenuAction::ResetPosition, Check::None),
        MenuItem::leaf("About Lillith", MenuAction::About, Check::None),
        MenuItem::Separator,
        MenuItem::leaf("Quit", MenuAction::Quit, Check::None),
    ];
    Menu { items }
}

/// Overlay-side work a menu pick needs (beyond brain actions).
#[derive(Debug, Clone, PartialEq)]
pub enum MenuEffect {
    /// Stop / resume applying system-monitor events (e.g. drop them in
    /// `Controller::drain_feed` while paused).
    SetMonitorPaused(bool),
    /// Re-create the stage at this scale and persist `scale` in config.toml.
    SetScale(f32),
    /// Open the config file (create it first if missing), e.g. with
    /// [`open_settings_command`].
    OpenSettings(Option<PathBuf>),
    /// Move her to this position and persist it ([`crate::config::save_overlay`]).
    ResetPosition(OverlayPos),
    /// Show this text (speech bubble or notification).
    ShowAbout(String),
}

/// What to do after a pick.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MenuOutcome {
    /// Feed to the overlay's existing `Controller::apply` (and to
    /// `lillith_voice::Voice::on_actions`).
    pub actions: Vec<Action>,
    pub effects: Vec<MenuEffect>,
}

/// HOOK 2: applies a pick. Mode, mood and mute change the brain right
/// away (through its own setters) and come back as the usual
/// `ModeChanged` / `MoodChanged` / `Muted` actions; `Quit` is `Action::Quit`.
/// `state` must be the [`MenuState`] the menu was built from (toggles flip it).
pub fn on_menu_action(action: MenuAction, brain: &mut Brain, state: &MenuState) -> MenuOutcome {
    let mut out = MenuOutcome::default();
    match action {
        MenuAction::SetMode(m) => out.actions = brain.set_mode(m),
        MenuAction::SetMood(m) => out.actions = brain.set_mood(m),
        MenuAction::ToggleMute => {
            let m = !brain.muted();
            brain.set_muted(m);
            out.actions.push(Action::Muted(m));
        }
        MenuAction::ToggleMonitor => out
            .effects
            .push(MenuEffect::SetMonitorPaused(!state.monitor_paused)),
        MenuAction::SetSize(z) => {
            if (z.scale() - state.scale).abs() > 1e-4 {
                out.effects.push(MenuEffect::SetScale(z.scale()));
            }
        }
        MenuAction::OpenSettings => out.effects.push(MenuEffect::OpenSettings(
            crate::config::Config::default_path(),
        )),
        MenuAction::ResetPosition => out
            .effects
            .push(MenuEffect::ResetPosition(OverlayPos::default())),
        MenuAction::About => out.effects.push(MenuEffect::ShowAbout(about_text())),
        MenuAction::Quit => out.actions.push(Action::Quit),
    }
    out
}

/// `xdg-open <path>` (not spawned here; the overlay spawns it off the
/// render path, e.g. `cmd.spawn()` and forget).
pub fn open_settings_command(path: &Path) -> std::process::Command {
    let mut c = std::process::Command::new("xdg-open");
    c.arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    c
}

/// Text for a fresh config.toml (written before "Open settings" if the
/// file does not exist yet).
pub fn settings_template() -> String {
    let mut s = String::from(
        "# Lillith settings. Restart Lillith after editing.\n# Missing keys use the defaults below.\n\n",
    );
    s.push_str(&toml::to_string_pretty(&crate::config::Config::default()).unwrap_or_default());
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::RuleSet;
    use crate::state::BehaviorConfig;

    fn brain() -> Brain {
        let clips = ["idle", "walk_right", "knife_tap"].map(String::from);
        Brain::new(
            BehaviorConfig::default(),
            MoveMode::Stay,
            RuleSet::bundled(),
            clips,
            1,
        )
    }

    fn state(b: &Brain) -> MenuState {
        MenuState::from_brain(b, false, 0.55)
    }

    fn radios(menu: &Menu, sub: &str) -> Vec<(String, bool)> {
        menu.leaves()
            .into_iter()
            .filter(|(p, _)| p.first() == Some(&sub))
            .map(|(_, it)| match it {
                MenuItem::Leaf {
                    label,
                    check: Check::Radio(on),
                    ..
                } => (label.clone(), *on),
                other => panic!("not a radio: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn menu_layout() {
        let b = brain();
        let m = build_menu(&state(&b));
        let top: Vec<Option<&str>> = m.items.iter().map(MenuItem::label).collect();
        assert_eq!(
            top,
            [
                Some("Mode"),
                Some("Mood"),
                Some("Mute voice"),
                Some("Pause system monitor"),
                Some("Size"),
                None,
                Some("Open settings"),
                Some("Reset position"),
                Some("About Lillith"),
                None,
                Some("Quit"),
            ]
        );
        assert_eq!(m.leaves().len(), 3 + 5 + 2 + 3 + 4);
        let moods: Vec<String> = radios(&m, "Mood").into_iter().map(|r| r.0).collect();
        assert_eq!(
            moods,
            ["Playful", "Happy", "Mischievous", "Alert", "Sleepy"]
        );
        assert_eq!(m.action_at(0), Some(MenuAction::SetMode(MoveMode::Stay)));
        assert_eq!(m.action_at(m.leaves().len() - 1), Some(MenuAction::Quit));
    }

    #[test]
    fn radio_and_check_state_follow_the_app() {
        let b = brain();
        let m = build_menu(&state(&b));
        assert_eq!(
            radios(&m, "Mode"),
            [
                ("Stay".into(), true),
                ("Wander".into(), false),
                ("Follow".into(), false)
            ]
        );
        assert_eq!(radios(&m, "Mood").iter().filter(|r| r.1).count(), 1);
        assert_eq!(
            m.check_of(MenuAction::SetMood(Mood::Playful)),
            Some(Check::Radio(true))
        );
        assert_eq!(
            m.check_of(MenuAction::SetSize(Size::Normal)),
            Some(Check::Radio(true))
        );
        assert_eq!(
            m.check_of(MenuAction::ToggleMute),
            Some(Check::Checkbox(false))
        );
        let s = MenuState {
            mode: MoveMode::Follow,
            mood: Mood::Sleepy,
            muted: true,
            monitor_paused: true,
            scale: 0.75,
        };
        let m = build_menu(&s);
        assert_eq!(
            m.check_of(MenuAction::SetMode(MoveMode::Follow)),
            Some(Check::Radio(true))
        );
        assert_eq!(
            m.check_of(MenuAction::SetMood(Mood::Sleepy)),
            Some(Check::Radio(true))
        );
        assert_eq!(
            m.check_of(MenuAction::ToggleMute),
            Some(Check::Checkbox(true))
        );
        assert_eq!(
            m.check_of(MenuAction::ToggleMonitor),
            Some(Check::Checkbox(true))
        );
        assert_eq!(
            radios(&m, "Size"),
            [
                ("Small".into(), false),
                ("Normal".into(), false),
                ("Large".into(), true)
            ]
        );
        // A custom scale checks no size.
        let m = build_menu(&MenuState { scale: 0.6, ..s });
        assert!(radios(&m, "Size").iter().all(|r| !r.1));
    }

    #[test]
    fn actions_map_to_brain_and_effects() {
        let mut b = brain();
        let s = state(&b);
        let o = on_menu_action(MenuAction::SetMode(MoveMode::Wander), &mut b, &s);
        assert_eq!(o.actions, [Action::ModeChanged(MoveMode::Wander)]);
        assert_eq!(b.mode(), MoveMode::Wander);
        // Same mode again: nothing.
        assert!(
            on_menu_action(MenuAction::SetMode(MoveMode::Wander), &mut b, &s)
                .actions
                .is_empty()
        );

        let o = on_menu_action(MenuAction::SetMood(Mood::Alert), &mut b, &s);
        assert_eq!(o.actions, [Action::MoodChanged(Mood::Alert)]);
        assert_eq!(b.mood(), Mood::Alert);
        // The rebuilt menu shows it.
        let m = build_menu(&MenuState::from_brain(&b, false, 0.55));
        assert_eq!(
            m.check_of(MenuAction::SetMood(Mood::Alert)),
            Some(Check::Radio(true))
        );
        assert_eq!(
            m.check_of(MenuAction::SetMode(MoveMode::Wander)),
            Some(Check::Radio(true))
        );

        let o = on_menu_action(MenuAction::ToggleMute, &mut b, &s);
        assert_eq!(o.actions, [Action::Muted(true)]);
        assert!(b.muted());
        let o = on_menu_action(MenuAction::ToggleMute, &mut b, &s);
        assert_eq!(o.actions, [Action::Muted(false)]);

        let o = on_menu_action(MenuAction::ToggleMonitor, &mut b, &s);
        assert_eq!(o.effects, [MenuEffect::SetMonitorPaused(true)]);
        let paused = MenuState {
            monitor_paused: true,
            ..s.clone()
        };
        let o = on_menu_action(MenuAction::ToggleMonitor, &mut b, &paused);
        assert_eq!(o.effects, [MenuEffect::SetMonitorPaused(false)]);

        let o = on_menu_action(MenuAction::SetSize(Size::Large), &mut b, &s);
        assert_eq!(o.effects, [MenuEffect::SetScale(0.75)]);
        assert!(
            on_menu_action(MenuAction::SetSize(Size::Normal), &mut b, &s)
                .effects
                .is_empty()
        );
        assert_eq!(Size::Small.scale(), 0.45);

        let o = on_menu_action(MenuAction::ResetPosition, &mut b, &s);
        assert_eq!(
            o.effects,
            [MenuEffect::ResetPosition(OverlayPos::default())]
        );
        let o = on_menu_action(MenuAction::OpenSettings, &mut b, &s);
        assert!(matches!(&o.effects[..], [MenuEffect::OpenSettings(_)]));
        let o = on_menu_action(MenuAction::About, &mut b, &s);
        assert!(matches!(&o.effects[..], [MenuEffect::ShowAbout(t)] if t.contains("Lillith")));
        let o = on_menu_action(MenuAction::Quit, &mut b, &s);
        assert_eq!(o.actions, [Action::Quit]);
        assert!(o.effects.is_empty());
    }

    #[test]
    fn settings_helpers() {
        let c = open_settings_command(Path::new("/tmp/x/config.toml"));
        assert_eq!(c.get_program(), "xdg-open");
        assert_eq!(c.get_args().collect::<Vec<_>>(), ["/tmp/x/config.toml"]);
        let t = settings_template();
        let parsed = crate::config::Config::from_toml_str(&t).expect("template parses");
        assert_eq!(parsed, crate::config::Config::default());
    }
}
