//! Backend dispatch plus the [`Controller`] both backends drive: stage
//! (animator + renderer + mask), drag, placement and position persistence.

use crate::backend::Backend;
use crate::drag::{Drag, Placement, Release};
use crate::stage::{Changed, Stage, StageError};
use lillith_anim::animator::AnimatorConfig;
use lillith_core::config::{save_overlay, OverlayPos};
use std::path::PathBuf;
use std::time::Duration;

/// Clip played while she is held (if the manifest has it).
pub const DRAG_CLIP: &str = "drag_held";
/// Reaction to a plain left click on her (M3 owns the full click table).
pub const CLICK_CLIP: &str = "wave";
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
    /// Right-click on her quits (until the M6 tray menu exists).
    pub right_click_quits: bool,
}

/// Shared overlay logic, independent of the windowing system.
#[derive(Debug)]
pub struct Controller {
    pub stage: Stage,
    pub drag: Drag,
    /// Where the window is (last value handed to the compositor/WM).
    pub placement: Placement,
    output: Option<String>,
    config_path: Option<PathBuf>,
    pub opacity: u8,
    pub right_click_quits: bool,
}

impl Controller {
    pub fn new(opts: &OverlayOptions) -> Result<Self, OverlayError> {
        let stage = Stage::load(&opts.manifest, opts.scale, opts.animator.clone())?;
        Ok(Self::with_stage(stage, opts))
    }

    pub fn with_stage(stage: Stage, opts: &OverlayOptions) -> Self {
        Self {
            stage,
            drag: Drag::default(),
            placement: Placement::from_config(&opts.position),
            output: opts.position.output.clone(),
            config_path: opts.config_path.clone(),
            opacity: crate::pixels::opacity_byte(opts.opacity),
            right_click_quits: opts.right_click_quits,
        }
    }

    pub fn tick(&mut self, dt: f32) -> Result<Changed, OverlayError> {
        Ok(self.stage.tick(dt)?)
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

    /// Pointer moved; returns the new placement while dragging.
    pub fn motion(&mut self, at: (f64, f64)) -> Option<Placement> {
        let was = self.drag.dragging();
        let p = self.drag.motion(at, self.placement)?;
        if !was {
            let anim = self.stage.animator_mut();
            if anim.has_clip(DRAG_CLIP) {
                let _ = anim.set_base(DRAG_CLIP, false);
            }
        }
        Some(p)
    }

    /// Button released; persists the position after a drag.
    pub fn release(&mut self) -> Release {
        let r = self.drag.release(self.placement);
        match r {
            Release::Dropped(p) => {
                self.stage.animator_mut().stand();
                self.persist(p);
            }
            Release::Click => {
                let anim = self.stage.animator_mut();
                if anim.has_clip(CLICK_CLIP) {
                    let _ = anim.react(CLICK_CLIP, 1);
                }
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
            right_click_quits: true,
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
            right_click_quits: false,
        };
        // Either the backend is not compiled in, or the manifest is missing:
        // both are clean errors, never a panic or a hang.
        assert!(run(Backend::X11, opts).is_err());
        assert_eq!(period(30.0), Duration::from_secs_f32(1.0 / 30.0));
        assert_eq!(period(0.0), Duration::from_secs(1));
    }
}
