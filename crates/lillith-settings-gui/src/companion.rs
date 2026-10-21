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
use lillith_anim::atlas::Manifest;
use lillith_anim::avatar::AvatarStore;
use lillith_core::config::{MoveMode, OverlayPos};
use lillith_core::menu::{MenuAction, MenuExtras, MenuState, PoseGroup};
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
        MenuAction::Avatar(0) => state.extras.avatar = None,
        MenuAction::Avatar(n) => {
            if let Some((id, _)) = state.extras.avatars.get(n as usize - 1) {
                state.extras.avatar = Some(id.clone());
            }
        }
        MenuAction::PosePage(p) => state.extras.page = Some(p),
        MenuAction::MenuBack => state.extras.page = None,
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

// The wire format lives in the core (`lillith_core::menu::wire`) so the
// overlay (which must not pull in egui) parses exactly what this window
// prints.
pub use lillith_core::menu::{
    action_line, action_line_in, mode_key, parse_action_line, parse_pick, pick_line, wire, MenuPick,
};

/// Poses ▸ categories from the animation set, exactly as the overlay builds
/// them (`lillith_anim::catalog::grouped` + `label`).
pub fn poses_of(m: &Manifest) -> Vec<PoseGroup> {
    lillith_anim::catalog::grouped(m)
        .into_iter()
        .map(|(label, clips)| PoseGroup {
            label: label.to_string(),
            clips: clips
                .into_iter()
                .map(|c| {
                    let l = lillith_anim::catalog::label(&c);
                    (c, l)
                })
                .collect(),
        })
        .collect()
}

/// Avatar ▸ recents `(id, name)` and the selection, from the avatar store
/// (`<config dir>/avatar.json`), as the overlay lists them.
pub fn avatars_of(store: &AvatarStore) -> (Vec<(String, String)>, Option<String>) {
    let sel = store.selection();
    let recent = sel
        .recent
        .iter()
        .filter_map(|id| {
            let name = store
                .list()
                .into_iter()
                .find(|m| &m.id == id)
                .map(|m| m.name)?;
            Some((id.clone(), name))
        })
        .collect();
    (recent, sel.selected)
}

/// The menu extras this window can know on its own: poses from `manifest`,
/// avatars from the store next to the config. Chat is offered (the overlay
/// ignores it when it has no chat).
pub fn extras_for(poses: Vec<PoseGroup>, avatars: Option<&AvatarStore>) -> MenuExtras {
    let (avatars, avatar) = avatars.map(avatars_of).unwrap_or_default();
    MenuExtras {
        poses,
        page: None,
        avatars,
        avatar,
        chat: true,
    }
}

/// What goes on the wire for a pick: poses and recent avatars **by name**
/// (the overlay's indices are its own), everything else as the action.
/// `pick_line(&to_pick(a, x)) == action_line_in(a, x)`.
pub fn to_pick(a: MenuAction, extras: &MenuExtras) -> MenuPick {
    match a {
        MenuAction::Pose(n) => match extras.pose_at(n as usize) {
            Some(clip) => MenuPick::Pose(clip.to_string()),
            None => MenuPick::Action(a),
        },
        MenuAction::Avatar(n) if n > 0 => match extras.avatars.get(n as usize - 1) {
            Some((id, _)) => MenuPick::Avatar(Some(id.clone())),
            None => MenuPick::Action(a),
        },
        _ => MenuPick::Action(a),
    }
}

/// Menu-navigation picks this window handles itself (it shows every Poses
/// page inline instead of re-opening a popup): never sent.
pub fn is_local(a: MenuAction) -> bool {
    matches!(a, MenuAction::PosePage(_) | MenuAction::MenuBack)
}

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

    fn rich_extras() -> MenuExtras {
        MenuExtras {
            poses: vec![
                PoseGroup {
                    label: "Flirty / Allure".into(),
                    clips: (0..16)
                        .map(|i| (format!("tease_{i}"), format!("Tease {i}")))
                        .collect(),
                },
                PoseGroup {
                    label: "Gestures".into(),
                    clips: vec![("wave".into(), "Wave".into())],
                },
            ],
            page: None,
            avatars: vec![("mug".into(), "Mug".into())],
            avatar: None,
            chat: true,
        }
    }

    #[test]
    fn folded_in_items_go_out_as_core_wire_lines() {
        use lillith_core::menu::MenuPage;
        let x = rich_extras();
        let mut st = MenuState {
            mode: MoveMode::Stay,
            mood: Mood::Playful,
            muted: false,
            monitor_paused: false,
            scale: 0.55,
            extras: x.clone(),
        };
        // Main menu and a Poses page: every leaf → the core's line.
        for page in [None, Some(MenuPage { group: 0, page: 1 })] {
            st.extras.page = page;
            for (_, it) in build_menu(&st).leaves() {
                if let MenuItem::Leaf { action, .. } = it {
                    let line = pick_line(&to_pick(*action, &x));
                    assert_eq!(line, action_line_in(*action, &x));
                    assert!(parse_pick(&line).is_some(), "{line}");
                }
            }
        }
        let line = |a| pick_line(&to_pick(a, &x));
        let named = |n: &str, v: Option<&str>| match v {
            Some(v) => format!(r#"{{"menu_action":"{n}","value":"{v}"}}"#),
            None => format!(r#"{{"menu_action":"{n}"}}"#),
        };
        assert_eq!(line(MenuAction::ShowOff), named(wire::SHOW_OFF, None));
        assert_eq!(line(MenuAction::Chat), named(wire::CHAT, None));
        assert_eq!(line(MenuAction::Minimize), named(wire::MINIMIZE, None));
        assert_eq!(line(MenuAction::Pose(16)), named(wire::POSE, Some("wave")));
        assert_eq!(
            line(MenuAction::Avatar(1)),
            named(wire::AVATAR, Some("mug"))
        );
        assert_eq!(
            line(MenuAction::Avatar(0)),
            named(wire::AVATAR, Some(wire::LILLITH))
        );
        assert_eq!(
            line(MenuAction::ScaleStep(-1)),
            named(wire::SCALE_STEP, Some("-1"))
        );
        assert_eq!(
            pick_line(&MenuPick::ReloadConfig),
            named(wire::RELOAD_CONFIG, None)
        );
        // Unknown indices fall back to the index lines.
        assert_eq!(
            line(MenuAction::Pose(99)),
            named(wire::POSE_INDEX, Some("99"))
        );
        // Paging is handled here, never sent.
        let p = MenuPage { group: 0, page: 1 };
        assert!(is_local(MenuAction::PosePage(p)) && is_local(MenuAction::MenuBack));
        assert!(!is_local(MenuAction::ShowOff));
        let mut st2 = st.clone();
        st2.extras.page = None;
        apply(&mut st2, MenuAction::PosePage(p));
        assert_eq!(st2.extras.page, Some(p));
        apply(&mut st2, MenuAction::MenuBack);
        assert_eq!(st2.extras.page, None);
        apply(&mut st2, MenuAction::Avatar(1));
        assert_eq!(st2.extras.avatar.as_deref(), Some("mug"));
    }

    #[test]
    fn poses_and_avatars_come_from_the_same_sources_as_the_overlay() {
        let clip = |n: &str| {
            format!(
                "[clips.{n}]\npage = \"{n}.png\"\nfps = 8.0\n[[clips.{n}.frames]]\nx = 0\ny = 0\nw = 400\nh = 400\nanchor = [200, 392]\n"
            )
        };
        let text: String = ["idle", "blow_kiss", "wave"].map(clip).concat();
        let m = Manifest::from_toml_str(&text).expect("manifest");
        let g = poses_of(&m);
        let all: Vec<&str> = g
            .iter()
            .flat_map(|g| g.clips.iter().map(|c| c.0.as_str()))
            .collect();
        assert!(all.contains(&"blow_kiss") && all.contains(&"wave"), "{g:?}");
        assert_eq!(g, {
            let mut want = Vec::new();
            for (label, clips) in lillith_anim::catalog::grouped(&m) {
                want.push(PoseGroup {
                    label: label.into(),
                    clips: clips
                        .iter()
                        .map(|c| (c.clone(), lillith_anim::catalog::label(c)))
                        .collect(),
                });
            }
            want
        });
        let dir =
            std::env::temp_dir().join(format!("lillith-settings-av-x-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let st = AvatarStore::new(&dir);
        assert_eq!(extras_for(Vec::new(), Some(&st)).avatars, []);
        let x = extras_for(Vec::new(), None);
        assert!(x.chat && x.avatar.is_none());
        let _ = std::fs::remove_dir_all(dir);
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
