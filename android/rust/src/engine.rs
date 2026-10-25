//! The per-frame engine the Android overlay drives: desktop brain +
//! animator, rendered at the desktop default scale × display density, with
//! the window position, touch gestures and gravity worked out here so the
//! Kotlin/Java side only blits pixels and moves the overlay window.

use crate::fall::{Fall, Step};
use crate::pages::{Pages, DEFAULT_BUDGET};
use lillith_anim::animator::{Animator, AnimatorConfig, Dir};
use lillith_anim::atlas::{AtlasError, Manifest};
use lillith_anim::render::{draw_cell, draw_cell_crossfade, draw_mouth};
use lillith_anim::Canvas;
use lillith_core::config::Config;
use lillith_core::interaction::Button;
use lillith_core::rules::RuleSet;
use lillith_core::state::{Action, BehaviorConfig, Brain, Input};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// Alpha (0..255) a pixel needs to count as "on her" for touches.
pub const HIT_ALPHA: u8 = 40;
/// Finger travel (dp) before a press becomes a drag.
pub const DRAG_SLOP_DP: f32 = 10.0;
/// Hold (s) without moving that counts as a long-press (the desktop's
/// right click: the menu).
pub const LONG_PRESS_S: f32 = 0.5;

/// Requests for the Java side, OR-ed into [`Engine::take_requests`].
pub const REQ_OPEN_CHAT: u32 = 1;
pub const REQ_MENU: u32 = 2;
pub const REQ_QUIT: u32 = 4;

/// Touch phases the overlay forwards (Android `MotionEvent` actions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touch {
    Down,
    Move,
    Up,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// `.../generated/manifest.toml` (pages relative to it).
    pub manifest: PathBuf,
    /// Display density (Android `DisplayMetrics.density`, 1.0 = 160 dpi).
    pub density: f32,
    /// User size multiplier on top of the default (1.0 = default).
    pub size: f32,
    pub seed: u64,
    /// Parsed `config.toml` (same schema as desktop); defaults if absent.
    pub config: Config,
    pub cache_budget: usize,
}

impl EngineOptions {
    pub fn new(manifest: impl Into<PathBuf>, density: f32) -> Self {
        Self {
            manifest: manifest.into(),
            density,
            size: 1.0,
            seed: 1,
            config: Config::default(),
            cache_budget: DEFAULT_BUDGET,
        }
    }

    /// Render scale: the desktop scale (default
    /// [`lillith_core::config::DEFAULT_OVERLAY_SCALE`] = 0.55 of the 400 px
    /// cells, ~220 px tall) in dp, so she has the same apparent size.
    pub fn scale(&self) -> f32 {
        let s = self.config.overlay_scale();
        (s * self.density.max(0.5) * self.size.clamp(0.25, 3.0)).clamp(0.1, 4.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RenderKey {
    clip: String,
    frame: usize,
    flip: bool,
    blend: Option<(usize, i32)>,
    mouth: bool,
    q: [i32; 6],
}

#[derive(Debug, Clone, Copy)]
struct Press {
    /// Finger minus window top-left at the press (screen px).
    grab: (f32, f32),
    start: (f32, f32),
    held_s: f32,
    dragging: bool,
    long_fired: bool,
}

/// See the module docs.
pub struct Engine {
    anim: Animator,
    brain: Brain,
    behavior: BehaviorConfig,
    pages: Pages,
    canvas: Canvas,
    scale: f32,
    density: f32,
    origin: (f32, f32),
    screen: (f32, f32),
    left: f32,
    top: f32,
    fall: Option<Fall>,
    press: Option<Press>,
    key: Option<RenderKey>,
    says: VecDeque<String>,
    requests: u32,
    dirty: bool,
    placed: bool,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("base", &self.anim.base())
            .field("scale", &self.scale)
            .field("pos", &(self.left, self.top))
            .finish()
    }
}

/// Removes clips whose page `installed` says is missing, then (until
/// nothing changes) clips whose `base_clip`, `jiggle_of` or blend
/// `from`/`to` were removed. A dangling `jiggle_variant` is cleared.
/// Returns the number of clips removed.
pub fn drop_uninstalled_clips(manifest: &mut Manifest, installed: impl Fn(&str) -> bool) -> usize {
    let before = manifest.clips.len();
    manifest.clips.retain(|_, c| installed(&c.page));
    loop {
        let names: std::collections::BTreeSet<String> = manifest.clips.keys().cloned().collect();
        let n = manifest.clips.len();
        manifest.clips.retain(|_, c| {
            let ok = |o: &Option<String>| o.as_ref().is_none_or(|x| names.contains(x));
            ok(&c.base_clip)
                && ok(&c.jiggle_of)
                && c.blend
                    .as_ref()
                    .is_none_or(|b| names.contains(&b.from) && names.contains(&b.to))
        });
        if manifest.clips.len() == n {
            break;
        }
    }
    let names: std::collections::BTreeSet<String> = manifest.clips.keys().cloned().collect();
    for c in manifest.clips.values_mut() {
        if c.jiggle_variant
            .as_ref()
            .is_some_and(|v| !names.contains(v))
        {
            c.jiggle_variant = None;
        }
    }
    before - manifest.clips.len()
}

impl Engine {
    pub fn load(opts: EngineOptions) -> Result<Self, String> {
        let manifest = Manifest::load(&opts.manifest).map_err(|e| e.to_string())?;
        let dir = opts
            .manifest
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        Self::new(manifest, Pages::new(dir, opts.cache_budget), &opts)
    }

    pub fn new(manifest: Manifest, pages: Pages, opts: &EngineOptions) -> Result<Self, String> {
        let cfg = &opts.config;
        let scale = opts.scale();
        let (mut cw, mut ch) = (1u32, 1u32);
        for c in manifest.clips.values() {
            for f in &c.frames {
                cw = cw.max(f.w);
                ch = ch.max(f.h);
            }
        }
        // Clips whose sheet isn't installed yet (the Android clip pack is
        // downloaded after first run) are left out until it is, along with
        // anything that points at them, so she never picks an invisible clip.
        let mut manifest = manifest;
        drop_uninstalled_clips(&mut manifest, |rel| pages.exists(rel));
        // Mouth patches are optional art: drop any the APK doesn't carry.
        for c in manifest.clips.values_mut() {
            c.mouth_patches.retain(|_, p| pages.exists(&p.page));
        }
        let behavior = cfg.behavior.clone();
        let anim_cfg = AnimatorConfig {
            springs: cfg.springs.clone(),
            physics_hz: cfg.physics_hz,
            idle: behavior.idle.clone(),
            seed: opts.seed,
            physics_driver: true,
            playback_speed: cfg.anim.playback_speed,
            talk_speed: cfg.anim.talk_speed,
            smooth_frames: cfg.anim.smooth_frames,
            crossfade: cfg.anim.crossfade,
            ..AnimatorConfig::default()
        };
        let anim = Animator::new(manifest, anim_cfg).map_err(|e| e.to_string())?;
        let a = anim.current().rect.anchor;
        let w = ((cw as f32 * scale).round() as u32).max(1);
        let h = ((ch as f32 * scale).round() as u32).max(1);
        let canvas = Canvas::new(w, h).ok_or_else(|| format!("bad canvas {w}x{h}"))?;
        let clips: Vec<String> = anim.clip_names().map(str::to_string).collect();
        let mut brain = Brain::new(
            behavior.clone(),
            cfg.mode,
            RuleSet::bundled(),
            clips,
            opts.seed ^ 0xB4A1_17B4,
        );
        brain.set_muted(true);
        let mut e = Self {
            anim,
            brain,
            behavior,
            pages,
            canvas,
            scale,
            density: opts.density.max(0.5),
            origin: (a.0 as f32 * scale, a.1 as f32 * scale),
            screen: (w as f32 * 3.0, h as f32 * 3.0),
            left: 0.0,
            top: 0.0,
            fall: None,
            press: None,
            key: None,
            says: VecDeque::new(),
            requests: 0,
            dirty: true,
            placed: false,
        };
        e.sync_walk_speed();
        Ok(e)
    }

    fn sync_walk_speed(&mut self) {
        let Some(v) = ["walk_right", "walk_left", "walk"]
            .iter()
            .find_map(|n| self.anim.manifest().clips.get(*n))
            .and_then(|c| c.move_speed_px_s())
        else {
            return;
        };
        let px_s = v * self.scale * self.anim.playback_speed();
        self.brain.set_walk_speed(px_s);
    }

    // ── Geometry ─────────────────────────────────────────────────────────

    /// Overlay window size (px).
    pub fn size(&self) -> (u32, u32) {
        (self.canvas.width(), self.canvas.height())
    }

    /// Overlay window top-left on screen (px).
    pub fn position(&self) -> (i32, i32) {
        (self.left.round() as i32, self.top.round() as i32)
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The window top that puts her feet on the bottom of the screen.
    pub fn floor_top(&self) -> f32 {
        self.screen.1 - self.origin.1
    }

    fn max_left(&self) -> f32 {
        (self.screen.0 - self.canvas.width() as f32).max(0.0)
    }

    /// The usable screen (px). The first call stands her on the floor
    /// near the right edge; later calls (rotation) keep her on screen and
    /// let her fall to the new floor.
    pub fn set_screen(&mut self, w: u32, h: u32) {
        let first = !std::mem::replace(&mut self.placed, true);
        self.screen = (w.max(1) as f32, h.max(1) as f32);
        self.brain.set_area(0.0, self.max_left());
        if first {
            self.left = (self.max_left() * 0.85).round();
            self.top = self.floor_top();
        } else {
            self.left = self.left.clamp(0.0, self.max_left());
            if self.press.is_none() && (self.top - self.floor_top()).abs() > 0.5 {
                self.top = self.top.min(self.floor_top());
                self.start_fall();
            }
        }
        self.brain.set_position(self.left);
    }

    // ── Frame loop ───────────────────────────────────────────────────────

    /// Advances everything by `dt` seconds. Returns `true` if the pixels
    /// changed (call [`Engine::pixels`] and redraw).
    pub fn tick(&mut self, dt: f32) -> bool {
        let dt = dt.clamp(0.0, 0.25);
        if let Some(p) = &mut self.press {
            p.held_s += dt;
            if !p.dragging && !p.long_fired && p.held_s >= LONG_PRESS_S {
                p.long_fired = true;
                let a = self.brain.input(Input::Click(Button::Right));
                self.apply(a);
            }
        }
        let busy =
            self.anim.overlay_clip().is_some() || self.anim.queued() > 0 || self.fall.is_some();
        let a = self.brain.tick(dt, busy);
        self.apply(a);
        self.anim.tick(dt);
        let dx = self.anim.take_root_motion() * self.scale;
        let a = self.brain.root_motion(dx);
        self.apply(a);
        let rm = self.anim.base_has_root_motion() || self.anim.stopping();
        self.brain.set_root_motion(rm);
        if let Some(mut f) = self.fall {
            let floor = self.floor_top();
            match f.step(&self.behavior.gravity, floor, dt) {
                Step::Falling => {
                    self.top = f.top;
                    self.fall = Some(f);
                }
                Step::Landed => {
                    self.top = f.top;
                    self.fall = None;
                    let a = self.brain.input(Input::DragEnd {
                        left: self.left.round() as i32,
                    });
                    self.apply(a);
                }
            }
        }
        self.render()
    }

    fn start_fall(&mut self) {
        if self.fall.is_none() {
            let a = self.brain.begin_fall();
            self.apply(a);
        }
        self.fall = Some(Fall::new(self.top));
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::React { clip, priority } => {
                    if self.anim.has_clip(&clip) {
                        let _ = self.anim.react(&clip, priority);
                    }
                }
                Action::SetBase { clip } => {
                    if self.anim.has_clip(&clip) {
                        let _ = self.anim.set_base(&clip, false);
                    }
                }
                Action::Stand => self.anim.stand(),
                Action::Walk { right } => {
                    let dir = if right { Dir::Right } else { Dir::Left };
                    let _ = self.anim.walk(dir, false);
                }
                Action::MoveTo { left } => {
                    if self.press.is_none() {
                        self.left = (left as f32).clamp(0.0, self.max_left());
                    }
                }
                Action::Say(line) => {
                    if self.says.len() < 8 {
                        self.says.push_back(line);
                    }
                }
                Action::ContextMenu => self.requests |= REQ_MENU,
                Action::OpenChat => self.requests |= REQ_OPEN_CHAT,
                Action::Quit => self.requests |= REQ_QUIT,
                Action::Sfx(_)
                | Action::MoodChanged(_)
                | Action::Muted(_)
                | Action::ModeChanged(_) => {}
            }
        }
    }

    /// Renders the current frame if anything visible changed.
    fn render(&mut self) -> bool {
        let f = self.anim.current();
        let t = f.transform;
        let blend = f.blend.filter(|&(_, w)| w > 1.0 / 48.0);
        let key = RenderKey {
            clip: f.clip_name.to_string(),
            frame: f.frame,
            flip: f.flip,
            blend: blend.map(|(n, w)| (n, (w * 24.0).round() as i32)),
            mouth: f.mouth.is_some(),
            q: [
                (t.x / 0.25).round() as i32,
                (t.y / 0.25).round() as i32,
                (t.angle / 0.001).round() as i32,
                (t.scale / 0.002).round() as i32,
                (t.chest_offset / 0.5).round() as i32,
                (t.pigtail_offset / 0.5).round() as i32,
            ],
        };
        if !self.dirty && self.key.as_ref() == Some(&key) {
            return false;
        }
        self.canvas.fill(tiny_skia::Color::TRANSPARENT);
        let drawn: Result<(), AtlasError> = (|| {
            let cell = self.pages.cell(f.clip, f.frame)?;
            if let Some((next, w)) = blend {
                let nc = self.pages.cell(f.clip, next)?;
                let na = f.clip.frames[next.min(f.clip.frames.len() - 1)].anchor;
                draw_cell_crossfade(
                    &mut self.canvas,
                    (&cell, f.rect.anchor),
                    (&nc, na),
                    self.origin,
                    f.flip,
                    &t,
                    self.scale,
                    w,
                );
            } else {
                draw_cell(
                    &mut self.canvas,
                    &cell,
                    f.rect.anchor,
                    self.origin,
                    f.flip,
                    &t,
                    self.scale,
                );
            }
            if let Some(m) = &f.mouth {
                let patch = self.pages.get(&m.patch.page)?.clone();
                draw_mouth(
                    &mut self.canvas,
                    &patch,
                    m,
                    f.rect.anchor,
                    self.origin,
                    f.flip,
                    &t,
                    self.scale,
                );
            }
            Ok(())
        })();
        // A missing page must not kill the overlay; keeping the key means
        // it isn't retried every frame.
        let _ = drawn;
        self.key = Some(key);
        self.dirty = false;
        true
    }

    /// Premultiplied RGBA8 pixels, `size()` big, row-major (what an Android
    /// `ARGB_8888` bitmap's `copyPixelsFromBuffer` expects).
    pub fn pixels(&self) -> &[u8] {
        self.canvas.data()
    }

    /// `true` if window-local `(x, y)` is on her (not a transparent pixel).
    pub fn hit(&self, x: f32, y: f32) -> bool {
        let (w, h) = self.size();
        if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
            return false;
        }
        let i = ((y as u32 * w + x as u32) * 4 + 3) as usize;
        self.canvas.data().get(i).is_some_and(|&a| a >= HIT_ALPHA)
    }

    // ── Touch ────────────────────────────────────────────────────────────

    /// A touch at screen `(sx, sy)`. Returns `false` if a `Down` missed her
    /// (transparent pixel): the rest of that gesture is ignored.
    pub fn touch(&mut self, phase: Touch, sx: f32, sy: f32) -> bool {
        match phase {
            Touch::Down => {
                if !self.hit(sx - self.left, sy - self.top) {
                    self.press = None;
                    return false;
                }
                self.press = Some(Press {
                    grab: (sx - self.left, sy - self.top),
                    start: (sx, sy),
                    held_s: 0.0,
                    dragging: false,
                    long_fired: false,
                });
                true
            }
            Touch::Move => {
                let Some(mut p) = self.press else {
                    return false;
                };
                let slop = DRAG_SLOP_DP * self.density;
                if !p.dragging && !p.long_fired && dist(p.start, (sx, sy)) > slop {
                    p.dragging = true;
                    self.fall = None;
                    let a = self.brain.input(Input::DragStart);
                    self.apply(a);
                }
                if p.dragging {
                    let nl = (sx - p.grab.0).clamp(0.0, self.max_left());
                    let nt = (sy - p.grab.1).clamp(-self.origin.1 * 0.5, self.floor_top());
                    let vx = (nl - self.left) * 60.0;
                    self.anim.set_dragged(Some(vx.clamp(-3000.0, 3000.0)));
                    self.left = nl;
                    self.top = nt;
                }
                self.press = Some(p);
                true
            }
            Touch::Up | Touch::Cancel => {
                let Some(p) = self.press.take() else {
                    return false;
                };
                if p.dragging {
                    self.anim.set_dragged(None);
                    self.brain.set_position(self.left);
                    if self.behavior.gravity.g_px_s2 > 0.0 && self.top < self.floor_top() - 0.5 {
                        // Keeps the drag pose while falling; landing ends the drag.
                        self.fall = Some(Fall::new(self.top));
                    } else {
                        self.top = self.top.min(self.floor_top());
                        let a = self.brain.input(Input::DragEnd {
                            left: self.left.round() as i32,
                        });
                        self.apply(a);
                    }
                } else if phase == Touch::Up && !p.long_fired {
                    let a = self.brain.input(Input::Click(Button::Left));
                    self.apply(a);
                }
                true
            }
        }
    }

    // ── Outputs for the Java side ────────────────────────────────────────

    /// Speech lines since the last call (shown as a bubble/toast).
    pub fn take_say(&mut self) -> Option<String> {
        self.says.pop_front()
    }

    /// `REQ_*` bits since the last call.
    pub fn take_requests(&mut self) -> u32 {
        std::mem::take(&mut self.requests)
    }

    /// Plays a reaction clip by name (menu picks, chat replies).
    pub fn react(&mut self, clip: &str) -> bool {
        self.anim.has_clip(clip) && self.anim.react(clip, 60).is_ok()
    }

    pub fn base(&self) -> &str {
        self.anim.base()
    }

    pub fn falling(&self) -> bool {
        self.fall.is_some()
    }

    pub fn dragging(&self) -> bool {
        self.press.is_some_and(|p| p.dragging)
    }

    /// Bytes of decoded atlas pages held.
    pub fn cache_bytes(&self) -> usize {
        self.pages.bytes()
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}
