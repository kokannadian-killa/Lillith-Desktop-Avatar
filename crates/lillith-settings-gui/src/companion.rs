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
//!   overlay runs in another process and reads them on (re)start or on a
//!   `reload_config` line; size is the exception (the overlay owns it),
//! * forwarded to the optional in-process sink, and printed on stdout by
//!   the `lillith-settings` binary as `{"menu_action": ...}` lines
//!   ([`action_line`]) for a host that wants them live.

use crate::store::ConfigStore;
use lillith_core::config::{MoveMode, OverlayPos};
use lillith_core::menu::{MenuAction, MenuState};
use lillith_core::mood::Mood;

/// Menu state as stored in config.toml (mood isn't persisted by the core:
/// it starts at its default).
pub fn state_from_store(s: &ConfigStore) -> MenuState {
    let mode = match s.get_str(&["mode"]).as_deref() {
        Some("wander") => MoveMode::Wander,
        Some("follow") => MoveMode::Follow,
        _ => MoveMode::Stay,
    };
    let muted = s
        .get_bool(&["voice", "mute"])
        .or_else(|| s.get_bool(&["muted"]))
        .unwrap_or(false);
    MenuState {
        mode,
        mood: Mood::default(),
        muted,
        monitor_paused: !s.get_bool(&["monitor", "enabled"]).unwrap_or(true),
        // `scale = 1.0` (the default) means the 0.55 default size.
        scale: lillith_core::config::Config {
            scale: s.get_f32(&["scale"]).unwrap_or(1.0),
            ..Default::default()
        }
        .overlay_scale(),
        extras: Default::default(),
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
        // Local mirror only (radio state): the overlay owns size and
        // persists it itself.
        MenuAction::SetSize(z) => state.scale = z.scale(),
        MenuAction::ScaleStep(n) => state.scale = stepped(state.scale, n),
        _ => {}
    }
}

/// The scale a Bigger / Smaller step lands on (the overlay's own rule in
/// `lillith_core::menu::on_menu_action`: 0.05 grid, clamped, never exactly
/// 1.0). Only mirrors the radio state; nothing is written.
fn stepped(scale: f32, n: i8) -> f32 {
    use lillith_core::menu::{SCALE_RANGE, SCALE_STEP};
    let v = scale + n as f32 * SCALE_STEP;
    let mut v = ((v / SCALE_STEP).round() * SCALE_STEP).clamp(SCALE_RANGE.0, SCALE_RANGE.1);
    if (v - 1.0).abs() < 1e-3 {
        v += if n > 0 { SCALE_STEP } else { -SCALE_STEP };
    }
    v
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
///
/// Size is **not** written: the overlay owns `scale` (it applies
/// `set_size` / `scale_step` from the wire and saves it itself).
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
        MenuAction::ResetPosition => {
            let d = OverlayPos::default();
            store.set_i64(&["overlay", "margin_left"], d.margin_left as i64);
            store.set_i64(&["overlay", "margin_bottom"], d.margin_bottom as i64);
            Persisted::Staged
        }
        _ => Persisted::RuntimeOnly,
    }
}

// The wire format lives in the core so the overlay (which must not pull
// in egui) parses exactly what this window prints.
pub use lillith_core::menu::{action_line, mode_key, parse_action_line};

#[cfg(test)]
mod tests {
    use super::*;
    use lillith_core::menu::{build_menu, MenuItem, Size};

    #[test]
    fn every_menu_leaf_round_trips_as_a_line() {
        let st = MenuState {
            mode: MoveMode::Stay,
            mood: Mood::Playful,
            muted: false,
            monitor_paused: false,
            scale: 0.55,
            extras: Default::default(),
        };
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
    fn resize_shows_all_five_sizes_and_steps() {
        let st = MenuState {
            mode: MoveMode::Stay,
            mood: Mood::Playful,
            muted: false,
            monitor_paused: false,
            scale: 0.55,
            extras: Default::default(),
        };
        let menu = build_menu(&st);
        let resize: Vec<MenuAction> = menu
            .leaves()
            .into_iter()
            .filter(|(p, _)| p.first() == Some(&"Resize"))
            .filter_map(|(_, it)| match it {
                MenuItem::Leaf { action, .. } => Some(*action),
                _ => None,
            })
            .collect();
        let sizes: Vec<MenuAction> = Size::ALL.into_iter().map(MenuAction::SetSize).collect();
        assert_eq!(&resize[..5], &sizes[..]);
        assert_eq!(
            &resize[5..],
            [MenuAction::ScaleStep(1), MenuAction::ScaleStep(-1)]
        );
        assert_eq!(
            action_line(MenuAction::SetSize(Size::Huge)),
            r#"{"menu_action":"set_size","value":"xxl"}"#
        );
        // Steps skip exactly 1.0 like the overlay.
        assert!((stepped(0.95, 1) - 1.05).abs() < 1e-4);
        assert!((stepped(lillith_core::menu::SCALE_RANGE.0, -1) - 0.3).abs() < 1e-4);
    }

    #[test]
    fn picks_persist_to_config() {
        let p =
            std::env::temp_dir().join(format!("lillith-settings-comp-{}.toml", std::process::id()));
        std::fs::write(
            &p,
            "scale = 0.55\n[voice]\nmute = false\n[llm]\nbackend = \"ollama\"\n",
        )
        .unwrap();
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
        // Size is mirrored locally but never staged.
        assert_eq!(st.scale, 0.75);
        assert!(!store.is_pending(&["scale"]));
        apply(&mut st, MenuAction::ScaleStep(1));
        assert_eq!(
            persist(&mut store, &st, MenuAction::ScaleStep(1)),
            Persisted::RuntimeOnly
        );
        assert!((st.scale - 0.8).abs() < 1e-4);
        store.save().unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        let core = lillith_core::config::Config::from_toml_str(&text).unwrap();
        assert_eq!(core.mode, MoveMode::Follow);
        assert!((core.scale - 0.55).abs() < 1e-6, "scale untouched: {text}");
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
