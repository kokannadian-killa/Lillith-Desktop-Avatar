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
//! Poses           ▸ Show off ✨ · <category> (n) …  (opens a Poses page)
//! Chat
//! Avatar          ▸ (•) Lillith ( ) <recent> … · Custom image…
//! Resize          ▸ ( ) Small (•) Normal ( ) Large ( ) XL ( ) XXL · Bigger + · Smaller −
//! Minimize to peek
//! ─────────────
//! Mode            ▸ (•) Stay  ( ) Wander  ( ) Follow
//! Mood            ▸ (•) Playful ( ) Happy ( ) Mischievous ( ) Alert ( ) Sleepy
//! [ ] Mute voice
//! [ ] Pause system monitor
//! ─────────────
//! Settings…       (lillith-settings window; xdg-open config.toml fallback)
//! Reset position
//! About Lillith
//! ─────────────
//! Close
//! ```
//!
//! Poses / avatars / chat come from [`MenuExtras`] (the overlay fills them
//! from the animation set and the avatar store); without them the Poses
//! submenu is left out and Chat is disabled.
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
    XLarge,
    Huge,
}

/// Smallest / largest scale the Resize steps reach.
pub const SCALE_RANGE: (f32, f32) = (0.3, 1.6);
/// One "Bigger" / "Smaller" step.
pub const SCALE_STEP: f32 = 0.05;
/// Poses per page of the Poses ▸ category pages.
pub const POSE_PAGE: usize = 14;

impl Size {
    pub const ALL: [Size; 5] = [
        Size::Small,
        Size::Normal,
        Size::Large,
        Size::XLarge,
        Size::Huge,
    ];

    pub fn scale(self) -> f32 {
        match self {
            Size::Small => 0.45,
            Size::Normal => 0.55,
            Size::Large => 0.75,
            // Not 1.0: `scale = 1.0` in config.toml means "the default".
            Size::XLarge => 0.95,
            Size::Huge => 1.25,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Size::Small => "Small",
            Size::Normal => "Normal",
            Size::Large => "Large",
            Size::XLarge => "XL",
            Size::Huge => "XXL",
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
    /// Resize by `n` × [`SCALE_STEP`] (Bigger / Smaller).
    ScaleStep(i8),
    /// Play pose number `n` ([`MenuExtras::pose_at`] order).
    Pose(u16),
    /// Open a Poses category page.
    PosePage(MenuPage),
    /// Back from a Poses page to the main menu.
    MenuBack,
    /// 4–6 random allure poses in a row.
    ShowOff,
    /// Open the chat bubble.
    Chat,
    /// Shrink to the peek icon (a click on it restores her).
    Minimize,
    /// Avatar `0` = Lillith, `n` = [`MenuExtras::avatars`]`[n - 1]`.
    Avatar(u8),
    /// Import a custom image (opens the `lillith-settings` window).
    ImportAvatar,
    /// The `lillith-settings` window (xdg-open config.toml fallback).
    OpenSettings,
    ResetPosition,
    About,
    /// "Close": quit Lillith.
    Quit,
}

/// One page of a Poses category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MenuPage {
    pub group: u8,
    pub page: u8,
}

/// A Poses category with its clips (`(clip name, label)`), filled by the
/// overlay from the animation set.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PoseGroup {
    pub label: String,
    pub clips: Vec<(String, String)>,
}

/// Overlay-supplied menu content beyond the brain state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MenuExtras {
    pub poses: Vec<PoseGroup>,
    /// Showing a Poses page instead of the main menu.
    pub page: Option<MenuPage>,
    /// Recent custom avatars `(id, label)`.
    pub avatars: Vec<(String, String)>,
    /// Avatar on stage (`None` = Lillith).
    pub avatar: Option<String>,
    /// The chat bubble is available.
    pub chat: bool,
}

impl MenuExtras {
    /// Clip of pose number `n` (groups flattened in order).
    pub fn pose_at(&self, n: usize) -> Option<&str> {
        self.poses
            .iter()
            .flat_map(|g| g.clips.iter())
            .nth(n)
            .map(|(c, _)| c.as_str())
    }

    fn pose_offset(&self, group: usize) -> usize {
        self.poses.iter().take(group).map(|g| g.clips.len()).sum()
    }
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
    /// Poses, avatars, chat (empty = those items are left out / disabled).
    pub extras: MenuExtras,
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
            extras: MenuExtras::default(),
        }
    }

    pub fn with_extras(mut self, extras: MenuExtras) -> Self {
        self.extras = extras;
        self
    }
}

/// "About" text.
pub fn about_text() -> String {
    format!(
        "Lillith {} — a desktop companion. MIT; art: see LICENSE-ASSETS.md.",
        env!("CARGO_PKG_VERSION")
    )
}

/// HOOK 1: the menu for the current state (a Poses page when
/// `extras.page` is set).
pub fn build_menu(s: &MenuState) -> Menu {
    if let Some(page) = s.extras.page {
        return pose_page(s, page);
    }
    let size = Size::of_scale(s.scale);
    let x = &s.extras;
    let mut items = Vec::new();
    if !x.poses.is_empty() {
        let mut poses = vec![MenuItem::leaf(
            "Show off ✨",
            MenuAction::ShowOff,
            Check::None,
        )];
        for (g, group) in x.poses.iter().enumerate() {
            poses.push(MenuItem::leaf(
                format!("{} ({}) …", group.label, group.clips.len()),
                MenuAction::PosePage(MenuPage {
                    group: g as u8,
                    page: 0,
                }),
                Check::None,
            ));
        }
        items.push(MenuItem::Submenu {
            label: "Poses".into(),
            items: poses,
        });
    }
    items.push(MenuItem::Leaf {
        label: "Chat".into(),
        action: MenuAction::Chat,
        check: Check::None,
        enabled: x.chat,
    });
    let mut avatars = vec![MenuItem::leaf(
        "Lillith",
        MenuAction::Avatar(0),
        Check::Radio(x.avatar.is_none()),
    )];
    for (i, (id, label)) in x.avatars.iter().enumerate().take(6) {
        avatars.push(MenuItem::leaf(
            label.clone(),
            MenuAction::Avatar(i as u8 + 1),
            Check::Radio(x.avatar.as_deref() == Some(id)),
        ));
    }
    avatars.push(MenuItem::leaf(
        "Custom image…",
        MenuAction::ImportAvatar,
        Check::None,
    ));
    items.push(MenuItem::Submenu {
        label: "Avatar".into(),
        items: avatars,
    });
    let mut sizes: Vec<MenuItem> = Size::ALL
        .iter()
        .map(|&z| {
            MenuItem::leaf(
                z.label(),
                MenuAction::SetSize(z),
                Check::Radio(Some(z) == size),
            )
        })
        .collect();
    sizes.push(MenuItem::Leaf {
        label: "Bigger +".into(),
        action: MenuAction::ScaleStep(1),
        check: Check::None,
        enabled: s.scale < SCALE_RANGE.1 - 1e-3,
    });
    sizes.push(MenuItem::Leaf {
        label: "Smaller −".into(),
        action: MenuAction::ScaleStep(-1),
        check: Check::None,
        enabled: s.scale > SCALE_RANGE.0 + 1e-3,
    });
    items.push(MenuItem::Submenu {
        label: "Resize".into(),
        items: sizes,
    });
    items.push(MenuItem::leaf(
        "Minimize to peek",
        MenuAction::Minimize,
        Check::None,
    ));
    items.push(MenuItem::Separator);
    items.push(MenuItem::Submenu {
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
    });
    items.push(MenuItem::Submenu {
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
    });
    items.push(MenuItem::leaf(
        "Mute voice",
        MenuAction::ToggleMute,
        Check::Checkbox(s.muted),
    ));
    items.push(MenuItem::leaf(
        "Pause system monitor",
        MenuAction::ToggleMonitor,
        Check::Checkbox(s.monitor_paused),
    ));
    items.push(MenuItem::Separator);
    items.push(MenuItem::leaf(
        "Settings…",
        MenuAction::OpenSettings,
        Check::None,
    ));
    items.push(MenuItem::leaf(
        "Reset position",
        MenuAction::ResetPosition,
        Check::None,
    ));
    items.push(MenuItem::leaf(
        "About Lillith",
        MenuAction::About,
        Check::None,
    ));
    items.push(MenuItem::Separator);
    items.push(MenuItem::leaf("Close", MenuAction::Quit, Check::None));
    Menu { items }
}

/// A Poses category page: Back, the category's clips ([`POSE_PAGE`] per
/// page) and "More…" when it continues.
fn pose_page(s: &MenuState, page: MenuPage) -> Menu {
    let x = &s.extras;
    let g = (page.group as usize).min(x.poses.len().saturating_sub(1));
    let mut items = vec![MenuItem::leaf("‹ Back", MenuAction::MenuBack, Check::None)];
    let Some(group) = x.poses.get(g) else {
        return Menu { items };
    };
    items.push(MenuItem::Leaf {
        label: group.label.clone(),
        action: MenuAction::MenuBack,
        check: Check::None,
        enabled: false,
    });
    items.push(MenuItem::Separator);
    let start = page.page as usize * POSE_PAGE;
    let base = x.pose_offset(g);
    for (i, (_, label)) in group.clips.iter().enumerate().skip(start).take(POSE_PAGE) {
        items.push(MenuItem::leaf(
            label.clone(),
            MenuAction::Pose((base + i) as u16),
            Check::None,
        ));
    }
    let pages = group.clips.len().div_ceil(POSE_PAGE);
    if pages > 1 {
        items.push(MenuItem::Separator);
        let next = (page.page as usize + 1) % pages;
        items.push(MenuItem::leaf(
            format!("More… ({}/{})", page.page as usize + 1, pages),
            MenuAction::PosePage(MenuPage {
                group: g as u8,
                page: next as u8,
            }),
            Check::None,
        ));
    }
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
    /// Play this pose clip now.
    PlayPose(String),
    /// Play a few random allure poses in a row.
    ShowOff,
    /// Open the chat bubble.
    OpenChat,
    /// Shrink to the peek icon.
    Minimize,
    /// Show this custom avatar (`None` = Lillith) and remember the choice.
    SetAvatar(Option<String>),
    /// Start a custom-image import (Settings ▸ Avatar).
    ImportAvatar,
    /// Re-open the menu at this Poses page (`None` = the main menu).
    OpenPage(Option<MenuPage>),
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
        MenuAction::ScaleStep(n) => {
            let v = state.scale + n as f32 * SCALE_STEP;
            let mut v = ((v / SCALE_STEP).round() * SCALE_STEP).clamp(SCALE_RANGE.0, SCALE_RANGE.1);
            // Skip exactly 1.0 (it would read back as the 0.55 default).
            if (v - 1.0).abs() < 1e-3 {
                v += if n > 0 { SCALE_STEP } else { -SCALE_STEP };
            }
            if (v - state.scale).abs() > 1e-4 {
                out.effects.push(MenuEffect::SetScale(v));
            }
        }
        MenuAction::Pose(n) => {
            if let Some(c) = state.extras.pose_at(n as usize) {
                out.effects.push(MenuEffect::PlayPose(c.to_string()));
            }
        }
        MenuAction::PosePage(p) => out.effects.push(MenuEffect::OpenPage(Some(p))),
        MenuAction::MenuBack => out.effects.push(MenuEffect::OpenPage(None)),
        MenuAction::ShowOff => out.effects.push(MenuEffect::ShowOff),
        MenuAction::Chat => out.effects.push(MenuEffect::OpenChat),
        MenuAction::Minimize => out.effects.push(MenuEffect::Minimize),
        MenuAction::Avatar(0) => out.effects.push(MenuEffect::SetAvatar(None)),
        MenuAction::Avatar(n) => {
            if let Some((id, _)) = state.extras.avatars.get(n as usize - 1) {
                out.effects.push(MenuEffect::SetAvatar(Some(id.clone())));
            }
        }
        MenuAction::ImportAvatar => out.effects.push(MenuEffect::ImportAvatar),
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

/// Wire name of a mode (`config.toml` `mode` value and the settings
/// window's JSON lines).
pub fn mode_key(m: MoveMode) -> &'static str {
    match m {
        MoveMode::Stay => "stay",
        MoveMode::Wander => "wander",
        MoveMode::Follow => "follow",
    }
}

/// Wire name of a mood.
pub fn mood_key(m: Mood) -> &'static str {
    match m {
        Mood::Playful => "playful",
        Mood::Happy => "happy",
        Mood::Mischievous => "mischievous",
        Mood::Alert => "alert",
        Mood::Sleepy => "sleepy",
    }
}

/// Wire name of a size preset.
pub fn size_key(s: Size) -> &'static str {
    match s {
        Size::Small => "small",
        Size::Normal => "normal",
        Size::Large => "large",
        Size::XLarge => "xl",
        Size::Huge => "xxl",
    }
}

/// `menu_action` names of the wire format (the `lillith-settings` window's
/// stdout, read by the overlay). `value` is a string where noted.
pub mod wire {
    /// `value`: `stay` | `wander` | `follow`.
    pub const SET_MODE: &str = "set_mode";
    /// `value`: `playful` | `happy` | `mischievous` | `alert` | `sleepy`.
    pub const SET_MOOD: &str = "set_mood";
    pub const TOGGLE_MUTE: &str = "toggle_mute";
    pub const TOGGLE_MONITOR: &str = "toggle_monitor";
    /// Resize preset. `value`: `small` | `normal` | `large` | `xl` | `xxl`.
    pub const SET_SIZE: &str = "set_size";
    /// Resize ▸ Bigger / Smaller. `value`: steps of 0.05, e.g. `+1`, `-1`.
    pub const SCALE_STEP: &str = "scale_step";
    /// Poses ▸ <clip>. `value`: clip name, e.g. `blow_kiss`.
    pub const POSE: &str = "pose";
    /// Pose by index into the overlay's own pose list (`value`: `n`); only
    /// meaningful against the same list, prefer [`POSE`].
    pub const POSE_INDEX: &str = "pose_index";
    /// Open a Poses category page. `value`: `group/page`, e.g. `0/1`.
    pub const POSE_PAGE: &str = "pose_page";
    /// Back from a Poses page.
    pub const MENU_BACK: &str = "menu_back";
    /// Poses ▸ Show off.
    pub const SHOW_OFF: &str = "show_off";
    /// Open the chat bubble.
    pub const CHAT: &str = "chat";
    /// Minimize to peek.
    pub const MINIMIZE: &str = "minimize";
    /// Avatar switch. `value`: avatar id, or `lillith` for her own sprites.
    pub const AVATAR: &str = "avatar";
    /// Avatar by index into the overlay's recent list (`value`: `n` ≥ 1);
    /// prefer [`AVATAR`].
    pub const AVATAR_INDEX: &str = "avatar_index";
    /// Avatar ▸ Custom image… (import a new avatar).
    pub const AVATAR_IMPORT: &str = "avatar_import";
    pub const OPEN_SETTINGS: &str = "open_settings";
    pub const RESET_POSITION: &str = "reset_position";
    pub const ABOUT: &str = "about";
    /// "Close": quit Lillith.
    pub const QUIT: &str = "quit";
    /// config.toml changed on disk (Motion / Avatar / Chat knobs): the
    /// overlay re-reads it and applies it live.
    pub const RELOAD_CONFIG: &str = "reload_config";
    /// `value` of [`AVATAR`] meaning Lillith's own sprites.
    pub const LILLITH: &str = "lillith";
}

/// A pick on the wire: a [`MenuAction`], or a pose / avatar **by name**
/// ([`MenuAction::Pose`] / [`MenuAction::Avatar`] index the overlay's own
/// lists, which another process does not share), or a config reload.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuPick {
    Action(MenuAction),
    /// [`wire::POSE`]: play this clip.
    Pose(String),
    /// [`wire::AVATAR`]: switch to this avatar (`None` = Lillith).
    Avatar(Option<String>),
    /// [`wire::RELOAD_CONFIG`].
    ReloadConfig,
}

fn line_of(name: &str, value: Option<&str>) -> String {
    match value {
        Some(v) => {
            let v = v.replace('\\', "\\\\").replace('"', "\\\"");
            format!("{{\"menu_action\":\"{name}\",\"value\":\"{v}\"}}")
        }
        None => format!("{{\"menu_action\":\"{name}\"}}"),
    }
}

/// One-line JSON for a pick, as the `lillith-settings` window prints it on
/// stdout: `{"menu_action":"set_mode","value":"wander"}`. Total over
/// [`MenuAction`]; poses and recent avatars go out by index here (use
/// [`action_line_in`] to send them by name).
pub fn action_line(a: MenuAction) -> String {
    use wire::*;
    let owned;
    let (name, value): (&str, Option<&str>) = match a {
        MenuAction::SetMode(m) => (SET_MODE, Some(mode_key(m))),
        MenuAction::SetMood(m) => (SET_MOOD, Some(mood_key(m))),
        MenuAction::ToggleMute => (TOGGLE_MUTE, None),
        MenuAction::ToggleMonitor => (TOGGLE_MONITOR, None),
        MenuAction::SetSize(z) => (SET_SIZE, Some(size_key(z))),
        MenuAction::ScaleStep(n) => {
            owned = format!("{n:+}");
            (SCALE_STEP, Some(&owned))
        }
        MenuAction::Pose(n) => {
            owned = n.to_string();
            (POSE_INDEX, Some(&owned))
        }
        MenuAction::PosePage(p) => {
            owned = format!("{}/{}", p.group, p.page);
            (POSE_PAGE, Some(&owned))
        }
        MenuAction::MenuBack => (MENU_BACK, None),
        MenuAction::ShowOff => (SHOW_OFF, None),
        MenuAction::Chat => (CHAT, None),
        MenuAction::Minimize => (MINIMIZE, None),
        MenuAction::Avatar(0) => (AVATAR, Some(LILLITH)),
        MenuAction::Avatar(n) => {
            owned = n.to_string();
            (AVATAR_INDEX, Some(&owned))
        }
        MenuAction::ImportAvatar => (AVATAR_IMPORT, None),
        MenuAction::OpenSettings => (OPEN_SETTINGS, None),
        MenuAction::ResetPosition => (RESET_POSITION, None),
        MenuAction::About => (ABOUT, None),
        MenuAction::Quit => (QUIT, None),
    };
    line_of(name, value)
}

/// [`action_line`], but poses and recent avatars by name (resolved against
/// `extras`, the lists the menu was built from).
pub fn action_line_in(a: MenuAction, extras: &MenuExtras) -> String {
    match a {
        MenuAction::Pose(n) => match extras.pose_at(n as usize) {
            Some(clip) => pick_line(&MenuPick::Pose(clip.to_string())),
            None => action_line(a),
        },
        MenuAction::Avatar(n) if n > 0 => match extras.avatars.get(n as usize - 1) {
            Some((id, _)) => pick_line(&MenuPick::Avatar(Some(id.clone()))),
            None => action_line(a),
        },
        _ => action_line(a),
    }
}

/// One-line JSON for a [`MenuPick`].
pub fn pick_line(p: &MenuPick) -> String {
    match p {
        MenuPick::Action(a) => action_line(*a),
        MenuPick::Pose(clip) => line_of(wire::POSE, Some(clip)),
        MenuPick::Avatar(id) => line_of(wire::AVATAR, Some(id.as_deref().unwrap_or(wire::LILLITH))),
        MenuPick::ReloadConfig => line_of(wire::RELOAD_CONFIG, None),
    }
}

/// Reads string field `k` of a one-line JSON object (tolerates whitespace
/// around `:`; `\"` and `\\` escapes in the value).
fn json_field(line: &str, k: &str) -> Option<String> {
    let key = format!("\"{k}\"");
    let rest = &line[line.find(&key)? + key.len()..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => out.push(chars.next()?),
            c => out.push(c),
        }
    }
}

/// Parses an [`action_line`] back into a [`MenuAction`]. Poses / avatars
/// sent by name ([`wire::POSE`], [`wire::AVATAR`] with an id) need
/// [`parse_pick`]; here they are `None` (except `avatar` = `lillith`).
/// Anything else (unknown action, bad value, not a line of ours) is `None`.
pub fn parse_action_line(line: &str) -> Option<MenuAction> {
    use wire::*;
    let v = json_field(line, "value");
    let v = v.as_deref();
    let num = |v: Option<&str>| v?.trim().parse::<i64>().ok();
    Some(match json_field(line, "menu_action")?.as_str() {
        SET_MODE => MenuAction::SetMode(*MODES.iter().find(|m| Some(mode_key(**m)) == v)?),
        SET_MOOD => MenuAction::SetMood(*MOODS.iter().find(|m| Some(mood_key(**m)) == v)?),
        TOGGLE_MUTE => MenuAction::ToggleMute,
        TOGGLE_MONITOR => MenuAction::ToggleMonitor,
        SET_SIZE => MenuAction::SetSize(*Size::ALL.iter().find(|s| Some(size_key(**s)) == v)?),
        SCALE_STEP => MenuAction::ScaleStep(i8::try_from(num(v)?).ok().filter(|n| *n != 0)?),
        POSE_INDEX => MenuAction::Pose(u16::try_from(num(v)?).ok()?),
        POSE_PAGE => {
            let (g, p) = v?.split_once('/')?;
            MenuAction::PosePage(MenuPage {
                group: g.trim().parse().ok()?,
                page: p.trim().parse().ok()?,
            })
        }
        MENU_BACK => MenuAction::MenuBack,
        SHOW_OFF => MenuAction::ShowOff,
        CHAT => MenuAction::Chat,
        MINIMIZE => MenuAction::Minimize,
        AVATAR if v == Some(LILLITH) || v == Some("") => MenuAction::Avatar(0),
        AVATAR_INDEX => MenuAction::Avatar(u8::try_from(num(v)?).ok().filter(|n| *n > 0)?),
        AVATAR_IMPORT => MenuAction::ImportAvatar,
        OPEN_SETTINGS => MenuAction::OpenSettings,
        RESET_POSITION => MenuAction::ResetPosition,
        ABOUT => MenuAction::About,
        QUIT => MenuAction::Quit,
        _ => return None,
    })
}

/// Parses any wire line: named poses / avatars and config reloads, else
/// whatever [`parse_action_line`] reads.
pub fn parse_pick(line: &str) -> Option<MenuPick> {
    let name = json_field(line, "menu_action")?;
    let v = json_field(line, "value");
    match name.as_str() {
        wire::POSE => {
            let clip = v?.trim().to_string();
            (!clip.is_empty()).then_some(MenuPick::Pose(clip))
        }
        wire::AVATAR => match v.as_deref().map(str::trim) {
            None | Some("") | Some(wire::LILLITH) => Some(MenuPick::Action(MenuAction::Avatar(0))),
            Some(id) => Some(MenuPick::Avatar(Some(id.to_string()))),
        },
        wire::RELOAD_CONFIG => Some(MenuPick::ReloadConfig),
        _ => parse_action_line(line).map(MenuPick::Action),
    }
}

/// `xdg-open <path>` (not spawned here; the overlay spawns it off the
/// render path, e.g. `cmd.spawn()` and forget).
/// On Windows it is Notepad (always present; config.toml has no default
/// handler there).
pub fn open_settings_command(path: &Path) -> std::process::Command {
    #[cfg(not(windows))]
    let mut c = std::process::Command::new("xdg-open");
    #[cfg(windows)]
    let mut c = std::process::Command::new("notepad.exe");
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
    s.push_str(VOICE_EXAMPLE);
    s
}

/// Commented `[voice]` example (read by `lillith-voice`; core's `Config`
/// ignores the table). Appended to [`settings_template`].
pub const VOICE_EXAMPLE: &str = r#"
# Voice (text-to-speech, offline). Needs piper, espeak-ng or spd-say on PATH;
# with none installed she stays silent. Uncomment to change the defaults.
# [voice]
# enabled = true
# engine = "auto"        # auto | piper | espeak-ng | spd-say | none
# model = "~/.local/share/lillith/piper/en_US-amy-medium.onnx"
# voice = "female1"      # optional (spd-say -t / espeak-ng -v)
# rate = 1.0             # 0.5..2.0
# volume = 0.8           # 0..1
# mute = false
# cooldown_s = 8.0
# repeat_cooldown_s = 90.0
# queue = 3
# max_chars = 240
# player = "auto"        # auto | aplay | paplay  (piper only)
"#;

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

    fn size_radios(menu: &Menu) -> Vec<(String, bool)> {
        menu.leaves()
            .into_iter()
            .filter(|(p, _)| p.first() == Some(&"Resize"))
            .filter_map(|(_, it)| match it {
                MenuItem::Leaf {
                    label,
                    check: Check::Radio(on),
                    ..
                } => Some((label.clone(), *on)),
                _ => None,
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
                Some("Chat"),
                Some("Avatar"),
                Some("Resize"),
                Some("Minimize to peek"),
                None,
                Some("Mode"),
                Some("Mood"),
                Some("Mute voice"),
                Some("Pause system monitor"),
                None,
                Some("Settings…"),
                Some("Reset position"),
                Some("About Lillith"),
                None,
                Some("Close"),
            ]
        );
        // Chat, avatar (Lillith + Custom), resize (5 + 2), minimize, mode,
        // mood, mute/monitor, settings/reset/about, close.
        assert_eq!(m.leaves().len(), 1 + 2 + 7 + 1 + 3 + 5 + 2 + 3 + 1);
        let moods: Vec<String> = radios(&m, "Mood").into_iter().map(|r| r.0).collect();
        assert_eq!(
            moods,
            ["Playful", "Happy", "Mischievous", "Alert", "Sleepy"]
        );
        assert_eq!(m.action_at(0), Some(MenuAction::Chat));
        assert_eq!(m.action_at(m.leaves().len() - 1), Some(MenuAction::Quit));
        // No chat hook: Chat is disabled.
        assert!(matches!(
            m.find(MenuAction::Chat),
            Some(MenuItem::Leaf { enabled: false, .. })
        ));
    }

    fn extras() -> MenuExtras {
        MenuExtras {
            poses: vec![
                PoseGroup {
                    label: "Flirty".into(),
                    clips: (0..20)
                        .map(|i| (format!("flirt_{i}"), format!("Flirt {i}")))
                        .collect(),
                },
                PoseGroup {
                    label: "Gestures".into(),
                    clips: vec![("wave".into(), "Wave".into())],
                },
            ],
            page: None,
            avatars: vec![("mug".into(), "mug".into())],
            avatar: Some("mug".into()),
            chat: true,
        }
    }

    #[test]
    fn poses_avatar_resize_chat_minimize_items() {
        let mut b = brain();
        let s = state(&b).with_extras(extras());
        let m = build_menu(&s);
        assert_eq!(m.items[0].label(), Some("Poses"));
        assert_eq!(m.check_of(MenuAction::Avatar(1)), Some(Check::Radio(true)));
        assert_eq!(m.check_of(MenuAction::Avatar(0)), Some(Check::Radio(false)));
        let fx = |a, s: &MenuState, b: &mut Brain| on_menu_action(a, b, s).effects;
        assert_eq!(fx(MenuAction::ShowOff, &s, &mut b), [MenuEffect::ShowOff]);
        assert_eq!(fx(MenuAction::Chat, &s, &mut b), [MenuEffect::OpenChat]);
        assert_eq!(fx(MenuAction::Minimize, &s, &mut b), [MenuEffect::Minimize]);
        assert_eq!(
            fx(MenuAction::Avatar(1), &s, &mut b),
            [MenuEffect::SetAvatar(Some("mug".into()))]
        );
        assert_eq!(
            fx(MenuAction::Avatar(0), &s, &mut b),
            [MenuEffect::SetAvatar(None)]
        );
        assert_eq!(
            fx(MenuAction::ImportAvatar, &s, &mut b),
            [MenuEffect::ImportAvatar]
        );
        // Resize steps go past 0.75 and stay in range.
        assert_eq!(
            fx(MenuAction::SetSize(Size::Huge), &s, &mut b),
            [MenuEffect::SetScale(1.25)]
        );
        assert_eq!(
            fx(MenuAction::ScaleStep(1), &s, &mut b),
            [MenuEffect::SetScale(0.6)]
        );
        let near = MenuState {
            scale: 0.95,
            ..s.clone()
        };
        let up = fx(MenuAction::ScaleStep(1), &near, &mut b);
        assert!(matches!(up[..], [MenuEffect::SetScale(v)] if (v - 1.05).abs() < 1e-3));
        let big = MenuState {
            scale: SCALE_RANGE.1,
            ..s.clone()
        };
        assert!(fx(MenuAction::ScaleStep(1), &big, &mut b).is_empty());
        // Poses: category page, paging, picking, back.
        let p0 = MenuPage { group: 0, page: 0 };
        assert_eq!(
            fx(MenuAction::PosePage(p0), &s, &mut b),
            [MenuEffect::OpenPage(Some(p0))]
        );
        let paged = s.clone().with_extras(MenuExtras {
            page: Some(p0),
            ..extras()
        });
        let m = build_menu(&paged);
        let picks: Vec<_> = m
            .leaves()
            .into_iter()
            .filter_map(|(_, it)| match it {
                MenuItem::Leaf {
                    action: MenuAction::Pose(n),
                    ..
                } => Some(*n),
                _ => None,
            })
            .collect();
        assert_eq!(picks.len(), POSE_PAGE);
        assert!(m
            .find(MenuAction::PosePage(MenuPage { group: 0, page: 1 }))
            .is_some());
        assert_eq!(
            fx(MenuAction::Pose(3), &paged, &mut b),
            [MenuEffect::PlayPose("flirt_3".into())]
        );
        // Second group's first clip comes after the 20 flirty ones.
        assert_eq!(
            fx(MenuAction::Pose(20), &paged, &mut b),
            [MenuEffect::PlayPose("wave".into())]
        );
        assert_eq!(
            fx(MenuAction::MenuBack, &paged, &mut b),
            [MenuEffect::OpenPage(None)]
        );
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
            extras: MenuExtras::default(),
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
            size_radios(&m),
            [
                ("Small".into(), false),
                ("Normal".into(), false),
                ("Large".into(), true),
                ("XL".into(), false),
                ("XXL".into(), false)
            ]
        );
        // A custom scale checks no size.
        let m = build_menu(&MenuState { scale: 0.6, ..s });
        assert!(size_radios(&m).iter().all(|r| !r.1));
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
    fn new_items_on_the_wire() {
        let b = brain();
        let ex = extras();
        // Every leaf of the full menu (poses page included) round-trips,
        // by index and by name.
        for page in [None, Some(MenuPage { group: 0, page: 1 })] {
            let s = state(&b).with_extras(MenuExtras { page, ..extras() });
            let m = build_menu(&s);
            for (_, it) in m.leaves() {
                if let MenuItem::Leaf { action, .. } = it {
                    let l = action_line(*action);
                    assert_eq!(parse_action_line(&l), Some(*action), "{l}");
                    let named = action_line_in(*action, &ex);
                    let back = match parse_pick(&named).expect(&named) {
                        MenuPick::Action(a) => Some(a),
                        MenuPick::Pose(c) => (0..64)
                            .find(|&i| ex.pose_at(i) == Some(c.as_str()))
                            .map(|i| MenuAction::Pose(i as u16)),
                        MenuPick::Avatar(Some(id)) => ex
                            .avatars
                            .iter()
                            .position(|a| a.0 == id)
                            .map(|i| MenuAction::Avatar(i as u8 + 1)),
                        p => panic!("{p:?}"),
                    };
                    assert_eq!(back, Some(*action), "{named}");
                }
            }
        }
        let cases: [(MenuAction, &str); 14] = [
            (MenuAction::ShowOff, r#"{"menu_action":"show_off"}"#),
            (MenuAction::Chat, r#"{"menu_action":"chat"}"#),
            (MenuAction::Minimize, r#"{"menu_action":"minimize"}"#),
            (
                MenuAction::SetSize(Size::Small),
                r#"{"menu_action":"set_size","value":"small"}"#,
            ),
            (
                MenuAction::SetSize(Size::Normal),
                r#"{"menu_action":"set_size","value":"normal"}"#,
            ),
            (
                MenuAction::SetSize(Size::Large),
                r#"{"menu_action":"set_size","value":"large"}"#,
            ),
            (
                MenuAction::SetSize(Size::XLarge),
                r#"{"menu_action":"set_size","value":"xl"}"#,
            ),
            (
                MenuAction::SetSize(Size::Huge),
                r#"{"menu_action":"set_size","value":"xxl"}"#,
            ),
            (
                MenuAction::ScaleStep(1),
                r#"{"menu_action":"scale_step","value":"+1"}"#,
            ),
            (
                MenuAction::ScaleStep(-1),
                r#"{"menu_action":"scale_step","value":"-1"}"#,
            ),
            (
                MenuAction::Avatar(0),
                r#"{"menu_action":"avatar","value":"lillith"}"#,
            ),
            (
                MenuAction::ImportAvatar,
                r#"{"menu_action":"avatar_import"}"#,
            ),
            (
                MenuAction::OpenSettings,
                r#"{"menu_action":"open_settings"}"#,
            ),
            (MenuAction::Quit, r#"{"menu_action":"quit"}"#),
        ];
        for (a, l) in cases {
            assert_eq!(action_line(a), l);
            assert_eq!(parse_pick(l), Some(MenuPick::Action(a)));
        }
        // By name.
        assert_eq!(
            action_line_in(MenuAction::Pose(20), &ex),
            r#"{"menu_action":"pose","value":"wave"}"#
        );
        assert_eq!(
            parse_pick(r#"{"menu_action":"pose","value":"blow_kiss"}"#),
            Some(MenuPick::Pose("blow_kiss".into()))
        );
        assert_eq!(
            action_line_in(MenuAction::Avatar(1), &ex),
            r#"{"menu_action":"avatar","value":"mug"}"#
        );
        assert_eq!(
            parse_pick(r#"{ "menu_action" : "avatar", "value" : "cute-girl" }"#),
            Some(MenuPick::Avatar(Some("cute-girl".into())))
        );
        assert_eq!(
            parse_pick(r#"{"menu_action":"avatar","value":"lillith"}"#),
            Some(MenuPick::Action(MenuAction::Avatar(0)))
        );
        assert_eq!(
            parse_pick(r#"{"menu_action":"reload_config"}"#),
            Some(MenuPick::ReloadConfig)
        );
        assert_eq!(
            pick_line(&MenuPick::ReloadConfig),
            r#"{"menu_action":"reload_config"}"#
        );
        // Bad values.
        assert_eq!(parse_pick(r#"{"menu_action":"pose","value":""}"#), None);
        assert_eq!(parse_pick(r#"{"menu_action":"pose"}"#), None);
        assert_eq!(
            parse_pick(r#"{"menu_action":"scale_step","value":"0"}"#),
            None
        );
        assert_eq!(
            parse_pick(r#"{"menu_action":"avatar_index","value":"0"}"#),
            None
        );
        assert_eq!(
            parse_pick(r#"{"menu_action":"set_size","value":"giant"}"#),
            None
        );
        // Escapes survive.
        let odd = MenuPick::Pose("a\"b\\c".into());
        assert_eq!(parse_pick(&pick_line(&odd)), Some(odd));
    }

    #[test]
    fn action_lines_round_trip() {
        let m = build_menu(&MenuState::from_brain(&brain(), false, 0.55));
        for (_, it) in m.leaves() {
            if let MenuItem::Leaf { action, .. } = it {
                let l = action_line(*action);
                assert_eq!(parse_action_line(&l), Some(*action), "{l}");
            }
        }
        assert_eq!(
            action_line(MenuAction::SetMode(MoveMode::Wander)),
            r#"{"menu_action":"set_mode","value":"wander"}"#
        );
        assert_eq!(
            parse_action_line(r#"{ "menu_action" : "set_size", "value": "large" }"#),
            Some(MenuAction::SetSize(Size::Large))
        );
        assert_eq!(
            parse_action_line(r#"{"menu_action":"set_mode","value":"fly"}"#),
            None
        );
        assert_eq!(parse_action_line(r#"{"menu_action":"nope"}"#), None);
        assert_eq!(parse_action_line("lillith-settings: hello"), None);
        assert_eq!(parse_action_line(""), None);
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
