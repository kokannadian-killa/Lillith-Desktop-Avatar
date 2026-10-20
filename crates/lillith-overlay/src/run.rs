//! Backend dispatch plus the [`Controller`] both backends drive: stage
//! (animator + renderer + mask), drag, placement, position persistence and
//! the M3 behaviour [`Brain`] (clicks, hover, keys, drag, moods, sleep/wake,
//! Wander/Stay, pose cycle).

use crate::backend::Backend;
use crate::drag::{Drag, Placement, Release};
use crate::stage::{Changed, Stage, StageError};
use lillith_anim::animator::{AnimatorConfig, Dir};
use lillith_core::config::{save_overlay, MoveMode, OverlayPos};
use lillith_core::interaction::Button;
use lillith_core::rules::RuleSet;
use lillith_core::state::{Action, BehaviorConfig, Brain, Input, Key};
use lillith_core::Event;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Events from the system monitor (`lillith-sense`) and other async
/// sources, drained into the brain once per frame. Shared so the Wayland →
/// X11 fallback keeps the same feed.
pub type EventFeed = Arc<Mutex<Receiver<Event>>>;

/// Most feed events applied per frame (a burst cannot stall rendering).
pub const MAX_EVENTS_PER_TICK: usize = 16;

/// Px of her window kept on screen when dragging off an edge.
pub const KEEP_ON_SCREEN: i32 = 64;

/// Overlay errors.
#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    #[error(transparent)]
    Stage(#[from] StageError),
    #[error("this build has no {0} backend; rebuild with `--features {0}`")]
    NotBuilt(&'static str),
    /// The compositor lacks `zwlr_layer_shell_v1` (e.g. GNOME) or there is
    /// no Wayland socket. Auto-detection falls back to X11.
    #[error("wayland unavailable: {0}")]
    WaylandUnavailable(String),
    #[error("wayland: {0}")]
    Wayland(String),
    #[error("x11: {0}")]
    X11(String),
}

/// Everything the overlay needs from the app.
#[derive(Debug, Clone)]
pub struct OverlayOptions {
    pub manifest: PathBuf,
    /// Overlay size = manifest cell × `scale`.
    pub scale: f32,
    /// Whole-sprite opacity 0..=1.
    pub opacity: f32,
    /// Redraw rate cap (Hz). Frames only re-render when something moved.
    pub render_hz: f32,
    pub animator: AnimatorConfig,
    /// Saved position (`[overlay]` in config.toml).
    pub position: OverlayPos,
    /// Where to persist the position after a drag (`None` = don't).
    pub config_path: Option<PathBuf>,
    /// `true` if the user forced the backend (no automatic fallback).
    pub forced: bool,
    /// Quit after this long (smoke tests / CPU measurement).
    pub exit_after: Option<Duration>,
    /// M3 behaviour timings (`[behavior]`).
    pub behavior: BehaviorConfig,
    /// Wander or Stay (Follow behaves like Stay until M5).
    pub mode: MoveMode,
    /// Start muted (no SFX cues).
    pub muted: bool,
    /// System-monitor events (`None` = `--no-monitor` / tests).
    pub events: Option<EventFeed>,
}

/// Shared overlay logic, independent of the windowing system.
#[derive(Debug)]
pub struct Controller {
    pub stage: Stage,
    pub drag: Drag,
    pub brain: Brain,
    /// Where the window is (last value handed to the compositor/WM).
    pub placement: Placement,
    output: Option<String>,
    config_path: Option<PathBuf>,
    pub opacity: u8,
    pending_move: Option<Placement>,
    screen: Option<(u32, u32)>,
    quit: bool,
    feed: Option<EventFeed>,
}

impl Controller {
    pub fn new(opts: &OverlayOptions) -> Result<Self, OverlayError> {
        let stage = Stage::load(&opts.manifest, opts.scale, opts.animator.clone())?;
        Ok(Self::with_stage(stage, opts))
    }

    pub fn with_stage(stage: Stage, opts: &OverlayOptions) -> Self {
        let clips: Vec<String> = stage.animator().clip_names().map(str::to_string).collect();
        let mut brain = Brain::new(
            opts.behavior.clone(),
            opts.mode,
            RuleSet::bundled(),
            clips,
            opts.animator.seed ^ 0xB4A1_17B4,
        );
        brain.set_muted(opts.muted);
        let placement = Placement::from_config(&opts.position);
        brain.set_position(placement.left as f32);
        Self {
            stage,
            drag: Drag::default(),
            brain,
            placement,
            output: opts.position.output.clone(),
            config_path: opts.config_path.clone(),
            opacity: crate::pixels::opacity_byte(opts.opacity),
            pending_move: None,
            screen: None,
            quit: false,
            feed: opts.events.clone(),
        }
    }

    /// Output size in px; sets the range she wanders over.
    pub fn set_screen(&mut self, output: (u32, u32)) {
        self.screen = Some(output);
        let w = self.stage.size().0 as f32;
        self.brain.set_area(0.0, (output.0 as f32 - w).max(0.0));
    }

    pub fn has_screen(&self) -> bool {
        self.screen.is_some()
    }

    /// A wander step the backend should apply (bottom unchanged).
    pub fn take_move(&mut self) -> Option<Placement> {
        self.pending_move.take()
    }

    /// `true` once Q/Esc asked to quit.
    pub fn quit_requested(&self) -> bool {
        self.quit
    }

    /// Advances the brain and the stage by `dt` seconds.
    pub fn tick(&mut self, dt: f32) -> Result<Changed, OverlayError> {
        self.drain_feed();
        let anim = self.stage.animator();
        let busy = anim.overlay_clip().is_some() || anim.queued() > 0;
        let actions = self.brain.tick(dt, busy);
        self.apply(actions);
        let changed = self.stage.tick(dt)?;
        // Walks with root motion move the window by exactly what the drawn
        // frames stepped, so the planted foot stays put on the desktop.
        let dx = self.stage.take_root_motion_px();
        let actions = self.brain.root_motion(dx);
        self.apply(actions);
        let anim = self.stage.animator();
        self.brain
            .set_root_motion(anim.base_has_root_motion() || anim.stopping());
        Ok(changed)
    }

    /// Applies brain actions to the animator / window.
    pub fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            let anim = self.stage.animator_mut();
            match a {
                Action::React { clip, priority } => {
                    tracing::debug!(clip, priority, "react");
                    if anim.has_clip(&clip) {
                        let _ = anim.react(&clip, priority);
                    }
                }
                Action::SetBase { clip } => {
                    if anim.has_clip(&clip) {
                        let _ = anim.set_base(&clip, false);
                    }
                }
                Action::Stand => anim.stand(),
                Action::Walk { right } => {
                    tracing::debug!(right, "wander walk");
                    let dir = if right { Dir::Right } else { Dir::Left };
                    if anim.walk(dir, false).is_err() {
                        tracing::debug!("no walk clip; gliding");
                    }
                }
                Action::MoveTo { left } => {
                    self.pending_move = Some(Placement {
                        left,
                        bottom: self.placement.bottom,
                    });
                }
                Action::Say(line) => tracing::info!("💬 {line}"),
                Action::Sfx(cue) => tracing::debug!(cue, "sfx (audio lands with lillith-voice)"),
                Action::MoodChanged(m) => tracing::info!(mood = ?m, "mood"),
                Action::ContextMenu => tracing::info!("right-click: pose menu (stub until M6)"),
                Action::OpenChat => tracing::info!("chat requested (panel chat lands in M7)"),
                Action::Muted(m) => tracing::info!(muted = m, "sound"),
                Action::ModeChanged(m) => tracing::info!(mode = ?m, "movement mode"),
                Action::Quit => {
                    tracing::info!("quit requested");
                    self.quit = true;
                }
            }
        }
    }

    /// Left button down at `at`. Ignored unless it is on her (X11 may
    /// still deliver presses on transparent pixels while the input shape
    /// catches up).
    pub fn press(&mut self, at: (f64, f64), hit_at: (f64, f64), local: bool) -> bool {
        if !self.stage.hit(hit_at.0, hit_at.1) {
            return false;
        }
        self.drag.press(at, self.placement, local);
        true
    }

    /// Right/middle button pressed at window-local `hit_at`.
    pub fn button(&mut self, button: Button, hit_at: (f64, f64)) -> bool {
        if !self.stage.hit(hit_at.0, hit_at.1) {
            return false;
        }
        tracing::debug!(?button, "press");
        let a = self.brain.input(Input::Click(button));
        self.apply(a);
        true
    }

    /// Pointer entered/left her silhouette.
    pub fn hover(&mut self, inside: bool) {
        if inside && self.drag.active() {
            return;
        }
        let a = self.brain.input(Input::Hover(inside));
        self.apply(a);
    }

    /// A key while she has keyboard focus.
    pub fn key(&mut self, key: Key) {
        let a = self.brain.input(Input::Key(key));
        self.apply(a);
    }

    /// A bus event (system monitor feed, tests, the injector).
    pub fn event(&mut self, ev: &Event) {
        tracing::debug!(event = %ev.name, detail = ?ev.detail, "event");
        let a = self.brain.event(ev);
        self.apply(a);
    }

    /// Applies pending [`EventFeed`] events (at most [`MAX_EVENTS_PER_TICK`]).
    fn drain_feed(&mut self) {
        let Some(feed) = &self.feed else { return };
        let mut batch = Vec::new();
        if let Ok(rx) = feed.lock() {
            while batch.len() < MAX_EVENTS_PER_TICK {
                match rx.try_recv() {
                    Ok(ev) => batch.push(ev),
                    Err(_) => break,
                }
            }
        }
        for ev in batch {
            self.event(&ev);
        }
    }

    /// Pointer moved; returns the new placement while dragging.
    pub fn motion(&mut self, at: (f64, f64)) -> Option<Placement> {
        let was = self.drag.dragging();
        let p = self.drag.motion(at, self.placement)?;
        if !was {
            let a = self.brain.input(Input::DragStart);
            self.apply(a);
            self.pending_move = None;
        }
        Some(p)
    }

    /// Button released; persists the position after a drag.
    pub fn release(&mut self) -> Release {
        let r = self.drag.release(self.placement);
        match r {
            Release::Dropped(p) => {
                let a = self.brain.input(Input::DragEnd { left: p.left });
                self.apply(a);
                self.persist(p);
            }
            Release::Click => {
                tracing::debug!("left click");
                let a = self.brain.input(Input::Click(Button::Left));
                self.apply(a);
            }
            Release::None => {}
        }
        r
    }

    /// Logs how much rasterising the session needed (CPU budget check).
    pub fn log_stats(&self) {
        let n = self.stage.renders().max(1);
        tracing::info!(
            renders = self.stage.renders(),
            avg_render_us = (self.stage.render_time().as_micros() as u64) / n,
            "overlay stats"
        );
    }

    /// Saves `p` to `[overlay]` in config.toml (logs, never fails the app).
    pub fn persist(&self, p: Placement) {
        let Some(path) = &self.config_path else {
            return;
        };
        let pos = OverlayPos {
            margin_left: p.left,
            margin_bottom: p.bottom,
            output: self.output.clone(),
        };
        match save_overlay(path, &pos) {
            Ok(()) => {
                tracing::info!(left = p.left, bottom = p.bottom, path = %path.display(), "position saved")
            }
            Err(e) => tracing::warn!("could not save position to {}: {e}", path.display()),
        }
    }
}

/// Runs the overlay until she is closed. With an auto-detected Wayland
/// backend, a compositor without layer-shell falls back to X11/XWayland.
pub fn run(backend: Backend, opts: OverlayOptions) -> Result<(), OverlayError> {
    tracing::info!(
        ?backend,
        forced = opts.forced,
        session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default(),
        wayland_display = std::env::var("WAYLAND_DISPLAY").unwrap_or_default(),
        display = std::env::var("DISPLAY").unwrap_or_default(),
        "overlay backend selected"
    );
    match backend {
        Backend::Wayland => match run_wayland(&opts) {
            Err(OverlayError::WaylandUnavailable(why)) if !opts.forced => {
                tracing::warn!("{why}; falling back to the X11 backend");
                run_x11(&opts)
            }
            Err(OverlayError::NotBuilt(_)) if !opts.forced => run_x11(&opts),
            r => r,
        },
        Backend::X11 => run_x11(&opts),
    }
}

fn run_wayland(opts: &OverlayOptions) -> Result<(), OverlayError> {
    #[cfg(feature = "wayland")]
    {
        crate::wayland::run(opts)
    }
    #[cfg(not(feature = "wayland"))]
    {
        let _ = opts;
        Err(OverlayError::NotBuilt("wayland"))
    }
}

fn run_x11(opts: &OverlayOptions) -> Result<(), OverlayError> {
    #[cfg(feature = "x11")]
    {
        crate::x11::run(opts)
    }
    #[cfg(not(feature = "x11"))]
    {
        let _ = opts;
        Err(OverlayError::NotBuilt("x11"))
    }
}

/// Frame period for `hz` (clamped 1..=120).
pub fn period(hz: f32) -> Duration {
    Duration::from_secs_f32(1.0 / hz.clamp(1.0, 120.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::tests::fixture;
    use lillith_anim::render::PageCache;
    use lillith_core::config::Config;

    #[test]
    fn drag_persists_and_click_is_not_a_drag() {
        let (dir, m) = fixture("ctl");
        let cfg_path = dir.join("cfg/config.toml");
        let opts = OverlayOptions {
            manifest: dir.join("manifest.toml"),
            scale: 2.0,
            opacity: 1.0,
            render_hz: 30.0,
            animator: AnimatorConfig {
                idle_enabled: false,
                ..AnimatorConfig::default()
            },
            position: OverlayPos {
                margin_left: 100,
                margin_bottom: 50,
                output: None,
            },
            config_path: Some(cfg_path.clone()),
            forced: false,
            exit_after: None,
            behavior: BehaviorConfig::default(),
            mode: MoveMode::Stay,
            muted: false,
            events: None,
        };
        let stage = Stage::new(m, PageCache::new(&dir), 2.0, opts.animator.clone()).unwrap();
        let mut c = Controller::with_stage(stage, &opts);
        c.stage.render().unwrap();
        // Press on a transparent pixel is ignored (it would not even reach
        // us through the input region).
        assert!(!c.press((1.0, 1.0), (1.0, 1.0), true));
        // Click on her: no movement, nothing saved.
        assert!(c.press((40.0, 40.0), (40.0, 40.0), true));
        assert_eq!(c.release(), Release::Click);
        assert!(!cfg_path.exists());
        // Drag (surface-local, Wayland style) by +30, -20.
        assert!(c.press((40.0, 40.0), (40.0, 40.0), true));
        let p = c.motion((70.0, 20.0)).unwrap();
        assert_eq!(
            p,
            Placement {
                left: 130,
                bottom: 70
            }
        );
        c.placement = p;
        assert_eq!(c.release(), Release::Dropped(p));
        let saved = Config::load(&cfg_path).unwrap().overlay;
        assert_eq!((saved.margin_left, saved.margin_bottom), (130, 70));
        let _ = std::fs::remove_dir_all(dir);
    }

    fn m3_manifest(tag: &str) -> (std::path::PathBuf, lillith_anim::atlas::Manifest) {
        let (dir, _) = fixture(tag);
        let frames = r#"page = "idle.png"
fps = 8.0
frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 40, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
"#;
        let mut toml = String::new();
        for c in [
            "idle",
            "knife_tap",
            "wave",
            "drag_held",
            "walk_right",
            "sleep",
            "wake",
            "event_input_click_right",
        ] {
            toml.push_str(&format!("[clips.{c}]\n{frames}\n"));
        }
        (
            dir,
            lillith_anim::atlas::Manifest::from_toml_str(&toml).unwrap(),
        )
    }

    fn m3_controller(tag: &str, mode: MoveMode) -> (std::path::PathBuf, Controller) {
        let (dir, m) = m3_manifest(tag);
        let opts = OverlayOptions {
            manifest: dir.join("manifest.toml"),
            scale: 2.0,
            opacity: 1.0,
            render_hz: 30.0,
            animator: AnimatorConfig {
                idle_enabled: false,
                ..AnimatorConfig::default()
            },
            position: OverlayPos {
                margin_left: 100,
                margin_bottom: 50,
                output: None,
            },
            config_path: None,
            forced: false,
            exit_after: None,
            behavior: BehaviorConfig {
                pose_cycle_s: 1e6,
                ..BehaviorConfig::default()
            },
            mode,
            muted: false,
            events: None,
        };
        let stage = Stage::new(m, PageCache::new(&dir), 2.0, opts.animator.clone()).unwrap();
        let mut c = Controller::with_stage(stage, &opts);
        c.stage.render().unwrap();
        (dir, c)
    }

    #[test]
    fn brain_drives_the_animator() {
        let (dir, mut c) = m3_controller("m3", MoveMode::Stay);
        // Left click on her -> knife_tap reaction.
        assert!(c.press((40.0, 40.0), (40.0, 40.0), true));
        assert_eq!(c.release(), Release::Click);
        assert_eq!(c.stage.animator().overlay_clip(), Some("knife_tap"));
        // Right click on her: cleaning reaction, never quits.
        assert!(c.button(Button::Right, (40.0, 40.0)));
        assert!(!c.quit_requested());
        assert!(!c.button(Button::Middle, (1.0, 1.0)), "transparent pixel");
        // Drag -> drag_held base; drop -> idle.
        assert!(c.press((40.0, 40.0), (40.0, 40.0), true));
        c.motion((70.0, 40.0)).unwrap();
        assert_eq!(c.stage.animator().base(), "drag_held");
        c.release();
        assert_eq!(c.stage.animator().base(), "idle");
        // Sleep, then proximity wakes her.
        c.event(&Event::new("idle.asleep"));
        assert_eq!(c.stage.animator().base(), "sleep");
        c.hover(true);
        assert_eq!(c.stage.animator().base(), "idle");
        assert_eq!(c.stage.animator().overlay_clip(), Some("wake"));
        // Q quits.
        c.key(Key::Char('q'));
        assert!(c.quit_requested());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn monitor_feed_reaches_the_brain() {
        let (dir, mut c) = m3_controller("m4feed", MoveMode::Stay);
        let (tx, rx) = std::sync::mpsc::channel();
        c.feed = Some(Arc::new(Mutex::new(rx)));
        tx.send(Event::new("idle.asleep")).unwrap();
        c.tick(0.05).unwrap();
        assert_eq!(c.stage.animator().base(), "sleep");
        // Desktop input after a long break (presence monitor) wakes her.
        tx.send(Event::with_detail("user.returned", "away 6m"))
            .unwrap();
        c.tick(0.05).unwrap();
        assert_eq!(c.stage.animator().base(), "idle");
        // A burst is spread over frames.
        for _ in 0..(MAX_EVENTS_PER_TICK + 4) {
            tx.send(Event::new("user.activity")).unwrap();
        }
        c.tick(0.05).unwrap();
        assert_eq!(
            c.feed.as_ref().unwrap().lock().unwrap().try_iter().count(),
            4
        );
        drop(tx);
        c.tick(0.05).unwrap(); // disconnected feed is harmless
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn wander_moves_the_window() {
        let (dir, mut c) = m3_controller("m3w", MoveMode::Wander);
        c.set_screen((1200, 800));
        let mut moved = Vec::new();
        let mut walked = false;
        for _ in 0..1200 {
            c.tick(0.1).unwrap();
            // walk_left is walk_right mirrored in this manifest.
            walked |= c.stage.animator().base() == "walk_right";
            if let Some(p) = c.take_move() {
                assert_eq!(p.bottom, 50, "only x changes");
                c.placement = p;
                moved.push(p.left);
            }
        }
        assert!(!moved.is_empty(), "she wandered");
        assert!(walked, "walk clip played");
        assert!(moved.iter().all(|&x| (0..=1200 - 80).contains(&x)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_backend_feature_is_reported() {
        let opts = OverlayOptions {
            manifest: "/nonexistent/manifest.toml".into(),
            scale: 1.0,
            opacity: 1.0,
            render_hz: 30.0,
            animator: AnimatorConfig::default(),
            position: OverlayPos::default(),
            config_path: None,
            forced: true,
            exit_after: Some(Duration::ZERO),
            behavior: BehaviorConfig::default(),
            mode: MoveMode::Stay,
            muted: false,
            events: None,
        };
        // Either the backend is not compiled in, or the manifest is missing:
        // both are clean errors, never a panic or a hang.
        assert!(run(Backend::X11, opts).is_err());
        assert_eq!(period(30.0), Duration::from_secs_f32(1.0 / 30.0));
        assert_eq!(period(0.0), Duration::from_secs(1));
    }
}
