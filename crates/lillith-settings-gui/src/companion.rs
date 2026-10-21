//! The Companion tab's model: the right-click menu (`lillith_core::menu`)
//! mirrored 1:1, plus how each pick is persisted.
//!
//! The tab is drawn from [`lillith_core::menu::build_menu`] itself
//! (submenus → radio groups, checkbox leaves → toggles, plain leaves →
//! buttons), so new `MenuAction`s added to the core menu show up here
//! without changes. Picks are:
//!
//! * applied to the local [`MenuState`] (so the radios/checks update),
//! * persisted to config.toml where a key exists ([`persist`]) — the
//!   overlay runs in another process and reads them on (re)start,
//! * forwarded to the optional in-process sink, and printed on stdout by
//!   the `lillith-settings` binary as `{"menu_action": ...}` lines
//!   ([`action_line`]) for a host that wants them live.

use crate::store::ConfigStore;
use lillith_core::config::{MoveMode, OverlayPos};
use lillith_core::menu::{MenuAction, MenuState, Size, MODES, MOODS};
use lillith_core::mood::Mood;

/// Menu state as stored in config.toml (mood isn't persisted by the core:
/// it starts at its default).
pub fn state_from_store(s: &ConfigStore) -> MenuState {
    let mode = match s.get_str(&["mode"]).as_deref() {
        Some("wander") => MoveMode::Wander,
        Some("follow") => MoveMode::Follow,
        _ => MoveMode::Stay,
    };
    let muted = s.get_bool(&["voice", "mute"]).or_else(|| s.get_bool(&["muted"])).unwrap_or(false);
    MenuState {
        mode,
        mood: Mood::default(),
        muted,
        monitor_paused: !s.get_bool(&["monitor", "enabled"]).unwrap_or(true),
        scale: s.get_f32(&["scale"]).unwrap_or(1.0),
    }
}

/// Applies a pick to the local state (like the overlay does with
/// `on_menu_action`, minus the brain).
pub fn apply(state: &mut MenuState, a: MenuAction) {
    match a {
        MenuAction::SetMode(m) => state.mode = m,
        MenuAction::SetMood(m) => state.mood = m,
        MenuAction::ToggleMute => state.muted = !state.muted,
        MenuAction::ToggleMonitor => state.monitor_paused = !state.monitor_paused,
        MenuAction::SetSize(z) => state.scale = z.scale(),
        _ => {}
    }
}

/// What persisting a pick did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Persisted {
    /// Staged into config.toml.
    Staged,
    /// Nothing to store (runtime-only, e.g. mood, about, quit).
    RuntimeOnly,
}

/// Stages the config.toml keys for a pick (after [`apply`]).
pub fn persist(store: &mut ConfigStore, state: &MenuState, a: MenuAction) -> Persisted {
    match a {
        MenuAction::SetMode(m) => {
            store.set_str(&["mode"], mode_key(m));
            Persisted::Staged
        }
        MenuAction::ToggleMute => {
            store.set_bool(&["muted"], state.muted);
            // [voice] mute wins over the legacy key when present
            if store.get_bool(&["voice", "mute"]).is_some() {
                store.set_bool(&["voice", "mute"], state.muted);
            }
            Persisted::Staged
        }
        MenuAction::ToggleMonitor => {
            // TODO(LillithBot): the overlay has no persisted "paused" flag; the
            // nearest key is [monitor] enabled (read at start).
            store.set_bool(&["monitor", "enabled"], !state.monitor_paused);
            Persisted::Staged
        }
        MenuAction::SetSize(_) => {
            store.set_f32(&["scale"], state.scale, 2);
            Persisted::Staged
        }
        MenuAction::ResetPosition => {
            let d = OverlayPos::default();
            store.set_i64(&["overlay", "margin_left"], d.margin_left as i64);
            store.set_i64(&["overlay", "margin_bottom"], d.margin_bottom as i64);
            Persisted::Staged
        }
        _ => Persisted::RuntimeOnly,
    }
}

pub fn mode_key(m: MoveMode) -> &'static str {
    match m {
        MoveMode::Stay => "stay",
        MoveMode::Wander => "wander",
        MoveMode::Follow => "follow",
    }
}

fn mood_key(m: Mood) -> &'static str {
    match m {
        Mood::Playful => "playful",
        Mood::Happy => "happy",
        Mood::Mischievous => "mischievous",
        Mood::Alert => "alert",
        Mood::Sleepy => "sleepy",
    }
}

fn size_key(s: Size) -> &'static str {
    match s {
        Size::Small => "small",
        Size::Normal => "normal",
        Size::Large => "large",
    }
}

/// One-line JSON for a pick: `{"menu_action":"set_mode","value":"wander"}`.
pub fn action_line(a: MenuAction) -> String {
    let (name, value) = match a {
        MenuAction::SetMode(m) => ("set_mode", Some(mode_key(m))),
        MenuAction::SetMood(m) => ("set_mood", Some(mood_key(m))),
        MenuAction::ToggleMute => ("toggle_mute", None),
        MenuAction::ToggleMonitor => ("toggle_monitor", None),
        MenuAction::SetSize(z) => ("set_size", Some(size_key(z))),
        MenuAction::OpenSettings => ("open_settings", None),
        MenuAction::ResetPosition => ("reset_position", None),
        MenuAction::About => ("about", None),
        MenuAction::Quit => ("quit", None),
        #[allow(unreachable_patterns)]
        _ => ("unknown", None),
    };
    match value {
        Some(v) => format!("{{\"menu_action\":\"{name}\",\"value\":\"{v}\"}}"),
        None => format!("{{\"menu_action\":\"{name}\"}}"),
    }
}

/// Parses an [`action_line`] back (for a host reading our stdout).
pub fn parse_action_line(line: &str) -> Option<MenuAction> {
    let field = |k: &str| -> Option<String> {
        let pat = format!("\"{k}\":\"");
        let i = line.find(&pat)? + pat.len();
        let j = line[i..].find('"')? + i;
        Some(line[i..j].to_string())
    };
    let name = field("menu_action")?;
    let value = field("value");
    let v = value.as_deref();
    Some(match name.as_str() {
        "set_mode" => MenuAction::SetMode(*MODES.iter().find(|m| Some(mode_key(**m)) == v)?),
        "set_mood" => MenuAction::SetMood(*MOODS.iter().find(|m| Some(mood_key(**m)) == v)?),
        "toggle_mute" => MenuAction::ToggleMute,
        "toggle_monitor" => MenuAction::ToggleMonitor,
        "set_size" => MenuAction::SetSize(*Size::ALL.iter().find(|s| Some(size_key(**s)) == v)?),
        "open_settings" => MenuAction::OpenSettings,
        "reset_position" => MenuAction::ResetPosition,
        "about" => MenuAction::About,
        "quit" => MenuAction::Quit,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lillith_core::menu::{build_menu, MenuItem};

    #[test]
    fn every_menu_leaf_round_trips_as_a_line() {
        let st = MenuState { mode: MoveMode::Stay, mood: Mood::Playful, muted: false, monitor_paused: false, scale: 0.55 };
        let menu = build_menu(&st);
        let leaves = menu.leaves();
        assert!(leaves.len() >= 16);
        for (_, it) in leaves {
            if let MenuItem::Leaf { action, .. } = it {
                let l = action_line(*action);
                assert_eq!(parse_action_line(&l), Some(*action), "{l}");
            }
        }
    }

    #[test]
    fn picks_persist_to_config() {
        let p = std::env::temp_dir().join(format!("lillith-settings-comp-{}.toml", std::process::id()));
        std::fs::write(&p, "scale = 0.55\n[voice]\nmute = false\n[llm]\nbackend = \"ollama\"\n").unwrap();
        let mut store = ConfigStore::load(&p);
        let mut st = state_from_store(&store);
        assert_eq!(st.scale, 0.55);
        for a in [
            MenuAction::SetMode(MoveMode::Follow),
            MenuAction::ToggleMute,
            MenuAction::SetSize(Size::Large),
            MenuAction::ToggleMonitor,
            MenuAction::ResetPosition,
            MenuAction::SetMood(Mood::Sleepy),
        ] {
            apply(&mut st, a);
            persist(&mut store, &st, a);
        }
        assert_eq!(st.mood, Mood::Sleepy);
        store.save().unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        let core = lillith_core::config::Config::from_toml_str(&text).unwrap();
        assert_eq!(core.mode, MoveMode::Follow);
        assert!((core.scale - 0.75).abs() < 1e-6);
        assert!(core.muted);
        assert!(!core.monitor.enabled);
        assert_eq!(core.overlay, OverlayPos::default());
        assert!(text.contains("[voice]\nmute = true"), "{text}");
        assert!(text.contains("backend = \"ollama\""));
        let back = state_from_store(&ConfigStore::load(&p));
        assert_eq!(back.mode, MoveMode::Follow);
        assert!(back.muted && back.monitor_paused);
    }
}
