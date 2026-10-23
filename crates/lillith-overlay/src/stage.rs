//! The backend-independent "stage": owns the M2 [`Animator`] and atlas
//! pages, renders the current frame into a reusable transparent canvas at
//! the overlay size, and keeps the click-through mask in sync.
//!
//! Backends call [`Stage::tick`] at `render_hz`; it returns what changed so
//! they only re-upload pixels / input region when needed (idle CPU).

use crate::input_region::{mask_rects, rects_hit, Rect};
use crate::speech::{draw_speech, SpeechLayout, SpeechView};
use lillith_anim::animator::{Animator, AnimatorConfig, Layer};
use lillith_anim::atlas::Manifest;
use lillith_anim::avatar::{draw_avatar, Avatar, AvatarMotion, AvatarPose, MotionInputs};
use lillith_anim::render::{draw_cell, draw_cell_mix, draw_mouth, frame_transform, PageCache};
use lillith_anim::talk::Mouth;
use lillith_anim::text::Fonts;
use lillith_anim::Canvas;
use std::path::Path;

/// Alpha at or above which a pixel catches clicks.
pub const HIT_ALPHA: u8 = 40;
/// How long one render tick waits for a pose-video frame that is still
/// decoding before it keeps the previous frame on screen.
pub const VIDEO_FRAME_WAIT: std::time::Duration = std::time::Duration::from_millis(30);
/// Horizontal snap for mask rectangles (fewer rects, ≤ 2 px wider hit area).
pub const MASK_GRID: u32 = 2;

/// Stage errors.
#[derive(Debug, thiserror::Error)]
pub enum StageError {
    #[error(transparent)]
    Atlas(#[from] lillith_anim::atlas::AtlasError),
    #[error(transparent)]
    Anim(#[from] lillith_anim::animator::AnimError),
    #[error("overlay size {0}x{1} is invalid")]
    Size(u32, u32),
}

/// What [`Stage::tick`] changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Changed {
    /// New pixels in [`Stage::canvas`].
    pub pixels: bool,
    /// New rectangles in [`Stage::mask`].
    pub mask: bool,
}

/// Quantised "what is on screen" key; equal keys render identical pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    clip: String,
    frame: usize,
    flip: bool,
    q: [i32; 6],
    mouth: Option<Mouth>,
    /// Smooth mode: next frame + quantised cross-fade weight.
    blend: Option<(usize, i32)>,
    /// Hash of the speech bubble's text + quantised opacity (0 = none).
    speech: u64,
    /// Custom avatar pose, quantised (all zero for Lillith's sprites).
    pose: [i32; 6],
}

/// A custom avatar on stage, animated per render frame.
struct AvatarRig {
    av: Avatar,
    motion: AvatarMotion,
    pose: AvatarPose,
    inputs: MotionInputs,
}

/// Overlay speech bubble state (see [`Stage::with_speech_area`]).
struct SpeechArea {
    fonts: Fonts,
    layout: SpeechLayout,
    view: Option<SpeechView>,
    /// Wrapped lines of `view`, cached by its text.
    lines: (String, Vec<String>),
}

pub struct Stage {
    anim: Animator,
    /// Last cropped cell, keyed by (clip, frame): crops only on frame change.
    cell: Option<((String, usize), Canvas)>,
    /// Second cached cell: the frame being cross-faded in (smooth mode).
    cell_next: Option<((String, usize), Canvas)>,
    /// Coarse key of the last mask: the mask is only rescanned when the
    /// frame changes or the body moved by ≥ 1 px / 0.3° / 0.5 %.
    mask_key: Option<Key>,
    render_time: std::time::Duration,
    pages: PageCache,
    canvas: Canvas,
    scale: f32,
    origin: (f32, f32),
    key: Option<Key>,
    mask: Vec<Rect>,
    renders: u64,
    speech: Option<SpeechArea>,
    /// Largest manifest cell (unscaled) and the idle feet anchor in it.
    cell_wh: (u32, u32),
    base_anchor: (i32, i32),
    /// Click-through mode: only her head catches clicks (a grab handle so
    /// the menu stays reachable); the rest passes to the desktop.
    click_through: bool,
    avatar: Option<AvatarRig>,
    /// Jiggle strength 0..1 of the custom avatar (Settings slider).
    avatar_strength: f32,
    /// Renders skipped because a video frame was still decoding.
    pending_frames: u64,
    /// Canvas held at a fixed size while her scale eases (tease).
    frame: Option<StageFrame>,
    /// The canvas was just (re)allocated and is still empty: the next
    /// render must draw something rather than wait for a video frame.
    fresh: bool,
}

/// A canvas size held independently of her drawing scale, so a scale
/// animation (the "come closer" tease) resizes the window once instead of
/// on every step: no per-step buffer reallocation, no not-yet-painted
/// window area, no flicker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageFrame {
    /// Canvas scale (canvas = largest cell × this).
    pub scale: f32,
    /// Canvas x (px) of her body centre line (the cell's horizontal
    /// centre), so she stays put when the window is clamped on screen.
    pub center_x: f32,
}

impl std::fmt::Debug for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stage")
            .field("size", &self.size())
            .field("scale", &self.scale)
            .field("base", &self.anim.base())
            .finish()
    }
}

impl Stage {
    /// Loads `manifest_path` and builds the animator. The overlay is the
    /// manifest's cell size (largest frame) × `scale`, with every clip's
    /// feet anchor mapped to the same point.
    pub fn load(manifest_path: &Path, scale: f32, cfg: AnimatorConfig) -> Result<Self, StageError> {
        let manifest = Manifest::load(manifest_path)?;
        let dir = manifest_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        Self::new(manifest, PageCache::new(dir), scale, cfg)
    }

    pub fn new(
        manifest: Manifest,
        pages: PageCache,
        scale: f32,
        cfg: AnimatorConfig,
    ) -> Result<Self, StageError> {
        let scale = scale.clamp(0.1, 4.0);
        let (mut cw, mut ch) = (1u32, 1u32);
        for c in manifest.clips.values() {
            for f in &c.frames {
                cw = cw.max(f.w);
                ch = ch.max(f.h);
            }
        }
        let mut manifest = manifest;
        let mut pages = pages;
        drop_unloadable_mouth_patches(&mut manifest, &mut pages);
        let anim = Animator::new(manifest, cfg)?;
        // Feet anchor of the idle base: every clip shares the canvas/baseline.
        let a = anim.current().rect.anchor;
        let w = (cw as f32 * scale).round() as u32;
        let h = (ch as f32 * scale).round() as u32;
        let canvas = Canvas::new(w, h).ok_or(StageError::Size(w, h))?;
        Ok(Self {
            anim,
            cell: None,
            cell_next: None,
            mask_key: None,
            render_time: std::time::Duration::ZERO,
            pages,
            canvas,
            scale,
            origin: (a.0 as f32 * scale, a.1 as f32 * scale),
            key: None,
            mask: Vec::new(),
            renders: 0,
            speech: None,
            cell_wh: (cw, ch),
            base_anchor: a,
            click_through: false,
            avatar: None,
            avatar_strength: lillith_anim::avatar::DEFAULT_STRENGTH,
            pending_frames: 0,
            frame: None,
            fresh: true,
        })
    }

    /// Current scale (overlay size = cell × scale).
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Resizes the overlay live (Resize menu / Settings): canvas, feet
    /// anchor and speech area follow; the animator keeps playing.
    ///
    /// While a [`StageFrame`] is held (the "come closer" tease) the canvas
    /// keeps the frame's size and only the drawing scale changes, so the
    /// window is not resized on every easing step.
    pub fn set_scale(&mut self, scale: f32) -> Result<(), StageError> {
        let scale = scale.clamp(0.1, 4.0);
        let (cw, ch) = self.cell_wh;
        // Canvas scale: the held frame if it is (about) big enough for her.
        let held = self.frame.filter(|f| f.scale + 0.01 >= scale);
        let fs = held.map_or(scale, |f| f.scale.max(scale));
        let w = (cw as f32 * fs).round() as u32;
        let body_h = (ch as f32 * fs).round() as u32;
        let mut h = body_h;
        let resized = {
            let mut pad = 0;
            if let Some(s) = &self.speech {
                pad = SpeechLayout::new(&s.fonts, w, fs).pad;
            }
            h += pad;
            (w, h) != self.size()
        };
        let mut pad = 0;
        if let Some(s) = &mut self.speech {
            if resized || (self.scale - scale).abs() > f32::EPSILON {
                s.layout = SpeechLayout::new(&s.fonts, w, fs);
                s.lines = (String::new(), Vec::new());
            }
            pad = s.layout.pad;
        }
        if resized {
            // A new canvas is fully transparent (0x00000000); `fresh`
            // makes the next render draw her even if a video frame is
            // still decoding, so a resize never shows an empty window.
            self.canvas = Canvas::new(w, h).ok_or(StageError::Size(w, h))?;
            self.fresh = true;
        }
        self.scale = scale;
        let a = self.base_anchor;
        self.origin = match held {
            Some(f) => {
                // Feet on the frame's body centre line, on the same floor
                // line the window bottom would give at this scale.
                let x = f.center_x + (a.0 as f32 - cw as f32 / 2.0) * scale;
                let y = body_h as f32 - (ch as f32 - a.1 as f32) * scale;
                (x, y + pad as f32)
            }
            _ => (a.0 as f32 * scale, a.1 as f32 * scale + pad as f32),
        };
        self.key = None;
        self.mask_key = None;
        self.mask.clear();
        Ok(())
    }

    /// Holds the canvas at `frame` (see [`StageFrame`]) or releases it
    /// (`None`: the canvas follows her scale again). Re-lays the stage out.
    pub fn set_frame(&mut self, frame: Option<StageFrame>) -> Result<(), StageError> {
        self.frame = frame.map(|f| StageFrame {
            scale: f.scale.clamp(0.1, 4.0),
            ..f
        });
        self.set_scale(self.scale)
    }

    /// The held canvas frame, if any.
    pub fn frame(&self) -> Option<StageFrame> {
        self.frame
    }

    /// Canvas size (px) for scale `scale` without a held frame and without
    /// the speech area.
    pub fn body_size_at(&self, scale: f32) -> (u32, u32) {
        let (cw, ch) = self.cell_wh;
        let s = scale.clamp(0.1, 4.0);
        (
            (cw as f32 * s).round() as u32,
            (ch as f32 * s).round() as u32,
        )
    }

    /// Turns the overlay speech area on/off live (resizes the canvas).
    pub fn set_speech_enabled(&mut self, on: bool) -> Result<(), StageError> {
        if on == self.speech.is_some() {
            return Ok(());
        }
        if on {
            self.speech = Some(SpeechArea {
                fonts: Fonts::new(),
                layout: SpeechLayout::new(&Fonts::bundled(), 1, self.scale),
                view: None,
                lines: (String::new(), Vec::new()),
            });
        } else {
            self.speech = None;
        }
        self.set_scale(self.scale)
    }

    /// Click-through mode (only her head stays clickable).
    pub fn set_click_through(&mut self, on: bool) {
        if self.click_through != on {
            self.click_through = on;
            self.mask_key = None;
            self.key = None;
        }
    }

    pub fn click_through(&self) -> bool {
        self.click_through
    }

    /// Shows a custom avatar instead of Lillith's sprites (`None` = her).
    pub fn set_avatar(&mut self, av: Option<Avatar>) {
        self.avatar = av.map(|av| {
            self.avatar_strength = av.meta.jiggle_strength;
            AvatarRig {
                av,
                motion: AvatarMotion::default(),
                pose: AvatarPose::default(),
                inputs: MotionInputs::default(),
            }
        });
        self.key = None;
        self.mask_key = None;
    }

    /// Id of the custom avatar on stage (`None` = Lillith).
    pub fn avatar_id(&self) -> Option<&str> {
        self.avatar.as_ref().map(|r| r.av.meta.id.as_str())
    }

    /// Custom avatar jiggle strength 0..1 (live).
    pub fn set_avatar_strength(&mut self, s: f32) {
        self.avatar_strength = s.clamp(0.0, 1.0);
        self.key = None;
    }

    /// Custom avatar motion pace (1 = default; lower = slower).
    pub fn set_avatar_pace(&mut self, pace: f32) {
        if let Some(r) = &mut self.avatar {
            r.motion.pace = pace.clamp(0.2, 3.0);
        }
    }

    /// Inputs for the custom avatar motion (talking mouth, hover, drag).
    pub fn set_avatar_inputs(&mut self, talk: Option<f32>, hovered: bool, dragged: Option<f32>) {
        if let Some(r) = &mut self.avatar {
            r.inputs.talk = talk;
            r.inputs.hovered = hovered;
            r.inputs.dragged = dragged;
        }
    }

    /// A click/poke: the custom avatar does a soft bounce.
    pub fn avatar_react(&mut self, seconds: f32) {
        if let Some(r) = &mut self.avatar {
            r.motion.react(seconds);
        }
    }

    /// Current custom-avatar pose (tests, logs).
    pub fn avatar_pose(&self) -> Option<AvatarPose> {
        self.avatar.as_ref().map(|r| r.pose)
    }

    /// Grows the canvas upward to make room for her overlay speech bubble
    /// (`[chat] overlay_speech`). The feet anchor moves down by the same
    /// amount, so with the window anchored at its bottom edge she stays put.
    pub fn with_speech_area(mut self) -> Result<Self, StageError> {
        if self.speech.is_some() {
            return Ok(self);
        }
        let fonts = Fonts::new();
        let layout = SpeechLayout::new(&fonts, self.canvas.width(), self.scale);
        let (w, h) = (self.canvas.width(), self.canvas.height() + layout.pad);
        self.canvas = Canvas::new(w, h).ok_or(StageError::Size(w, h))?;
        self.origin.1 += layout.pad as f32;
        self.fresh = true;
        self.key = None;
        self.mask_key = None;
        self.speech = Some(SpeechArea {
            fonts,
            layout,
            view: None,
            lines: (String::new(), Vec::new()),
        });
        Ok(self)
    }

    /// Px above the sprite cell reserved for the speech bubble (0 if none).
    pub fn speech_pad(&self) -> u32 {
        self.speech.as_ref().map_or(0, |s| s.layout.pad)
    }

    /// Sets what the overlay speech bubble shows (`None` hides it). No-op
    /// without [`Stage::with_speech_area`].
    pub fn set_speech(&mut self, view: Option<SpeechView>) {
        if let Some(s) = &mut self.speech {
            s.view = view.filter(|v| v.alpha > 0.01 && !v.text.trim().is_empty());
        }
    }

    /// Lines the speech bubble shows right now (for tests/logs).
    pub fn speech_lines(&mut self) -> Vec<String> {
        match &mut self.speech {
            Some(s) => match &s.view {
                Some(v) => {
                    if s.lines.0 != v.text {
                        s.lines = (v.text.clone(), s.layout.lines(&s.fonts, &v.text));
                    }
                    s.lines.1.clone()
                }
                None => Vec::new(),
            },
            None => Vec::new(),
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.canvas.width(), self.canvas.height())
    }

    /// Where her feet anchor is drawn in the window (px from the top-left).
    pub fn feet_anchor(&self) -> (f32, f32) {
        self.origin
    }

    pub fn animator(&self) -> &Animator {
        &self.anim
    }

    pub fn animator_mut(&mut self) -> &mut Animator {
        &mut self.anim
    }

    /// Premultiplied RGBA pixels of the current frame.
    pub fn canvas(&self) -> &Canvas {
        &self.canvas
    }

    /// Click-through mask (input region) of the current frame.
    pub fn mask(&self) -> &[Rect] {
        &self.mask
    }

    /// `true` if `(x, y)` (window px) is on her.
    pub fn hit(&self, x: f64, y: f64) -> bool {
        rects_hit(&self.mask, x.floor() as i32, y.floor() as i32)
    }

    /// Pose-video cache counters and the renders skipped while a video
    /// frame was still decoding.
    pub fn video_stats(&self) -> (lillith_anim::video::VideoStats, u64) {
        (self.pages.video().stats(), self.pending_frames)
    }

    /// Bytes of decoded PNG pages held (the PNG fallback / non-video clips).
    pub fn page_bytes(&self) -> usize {
        self.pages.page_bytes()
    }

    /// Frames actually rasterised so far (for CPU diagnostics).
    pub fn renders(&self) -> u64 {
        self.renders
    }

    /// Total time spent rasterising + mask scanning.
    pub fn render_time(&self) -> std::time::Duration {
        self.render_time
    }

    /// Ground the drawn walk frames covered since the last call, in screen
    /// px (animator root motion × overlay scale); see
    /// [`Animator::take_root_motion`].
    pub fn take_root_motion_px(&mut self) -> f32 {
        self.anim.take_root_motion() * self.scale
    }

    /// Current layer (for logs).
    pub fn layer(&self) -> Layer {
        self.anim.current().layer
    }

    /// Advances the animator by `dt` seconds and re-renders if what is on
    /// screen changed (clip/frame/flip, or the spring transform by more than
    /// ~¼ px / 0.05° / 0.1 %).
    pub fn tick(&mut self, dt: f32) -> Result<Changed, StageError> {
        self.anim.tick(dt);
        if let Some(r) = &mut self.avatar {
            let walking =
                self.anim.base().starts_with("walk") || self.anim.base().starts_with("run");
            let flip = self.anim.current().flip;
            r.inputs.walk = walking.then_some(if flip { -1.0 } else { 1.0 });
            r.pose = r.motion.tick(dt, r.inputs);
        }
        self.render()
    }

    /// Renders the current animator frame if it differs from the last one.
    pub fn render(&mut self) -> Result<Changed, StageError> {
        let f = self.anim.current();
        let t = f.transform;
        // Quantisation steps: fine for pixels, coarse for the mask.
        let speech_hash = self
            .speech
            .as_ref()
            .and_then(|s| s.view.as_ref())
            .map_or(0, |v| {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                v.text.hash(&mut h);
                ((v.alpha * 50.0).round() as i32).hash(&mut h);
                h.finish() | 1
            });
        let quant = |px: f32, ang: f32, sc: f32| Key {
            clip: f.clip_name.to_string(),
            frame: f.frame,
            flip: f.flip,
            mouth: f.mouth.map(|m| m.shape),
            blend: f.blend.map(|(n, w)| (n, (w * 24.0).round() as i32)),
            speech: 0,
            pose: [0; 6],
            q: [
                (t.x / px).round() as i32,
                (t.y / px).round() as i32,
                (t.angle / ang).round() as i32,
                (t.scale / sc).round() as i32,
                (t.chest_offset / (2.0 * px)).round() as i32,
                (t.pigtail_offset / (2.0 * px)).round() as i32,
            ],
        };
        let pose_q = self.avatar.as_ref().map_or([0; 6], |r| {
            let p = &r.pose;
            let q = |v: f32, step: f32| (v / step).round() as i32;
            [
                q(p.breath, 0.01),
                q(p.sway, 0.0002),
                q(p.hop, 0.05),
                q(p.squash, 0.0005),
                q(p.talk, 0.05),
                q(p.tilt, 0.005),
            ]
        });
        let key = Key {
            speech: speech_hash,
            pose: pose_q,
            ..quant(0.25, 0.001, 0.002)
        };
        if self.key.as_ref() == Some(&key) {
            return Ok(Changed::default());
        }
        let started = std::time::Instant::now();
        let head = if let Some(r) = &self.avatar {
            self.canvas.fill(tiny_skia::Color::TRANSPARENT);
            draw_avatar(
                &mut self.canvas,
                &r.av,
                &r.pose,
                &t,
                self.avatar_strength,
                self.origin,
                f.flip,
                self.scale,
            );
            let tf = frame_transform(
                lillith_anim::avatar::ANCHOR,
                self.origin,
                f.flip,
                &t,
                self.scale,
            );
            let mut pt =
                tiny_skia::Point::from_xy(r.av.meta.anchors.head.0, r.av.meta.anchors.head.1);
            tf.map_points(std::slice::from_mut(&mut pt));
            pt.x
        } else {
            // A blend helper is followed by its target clip: start
            // decoding that clip's video now so it is ready on time.
            if let Some(next) = f.clip.blend.as_ref().map(|b| b.to.clone()) {
                if let Some(c) = self.anim.manifest().clips.get(&next) {
                    self.pages.prefetch(c);
                }
            }
            match Self::render_sprite(
                &mut self.pages,
                &mut self.cell,
                &mut self.cell_next,
                &mut self.canvas,
                self.origin,
                self.scale,
                &f,
                &t,
                self.fresh,
            )? {
                Some((head, exact)) => {
                    if !exact {
                        // Stand-in drawn: render the real frame next tick.
                        self.fresh = false;
                        self.render_time += started.elapsed();
                        return Ok(Changed {
                            pixels: true,
                            mask: false,
                        });
                    }
                    head
                }
                None => {
                    // Video frame still decoding: keep the previous
                    // picture on screen and retry next tick.
                    self.pending_frames += 1;
                    self.render_time += started.elapsed();
                    return Ok(Changed::default());
                }
            }
        };
        self.key = Some(key);
        self.fresh = false;
        self.renders += 1;
        let mut mkey = quant(1.0 / self.scale.max(0.1), 0.005, 0.005);
        if self.avatar.is_some() {
            mkey.pose = pose_q.map(|v| v / 8);
        }
        let mut mask_changed = false;
        if self.mask_key.as_ref() != Some(&mkey) {
            let mut mask = mask_rects(
                self.canvas.data(),
                self.canvas.width(),
                self.canvas.height(),
                HIT_ALPHA,
                MASK_GRID,
            );
            if self.click_through {
                mask = head_only(
                    &mask,
                    self.origin.1 - self.cell_wh.1 as f32 * self.scale * 0.72,
                );
            }
            mask_changed = mask != self.mask;
            self.mask = mask;
            self.mask_key = Some(mkey);
        }
        self.draw_speech_bubble(head);
        self.render_time += started.elapsed();
        Ok(Changed {
            pixels: true,
            mask: mask_changed,
        })
    }

    /// Draws Lillith's sprite frame (cross-faded in smooth mode) plus the
    /// mouth patch; returns the x of her head for the bubble tail.
    #[allow(clippy::too_many_arguments)]
    fn render_sprite(
        pages: &mut PageCache,
        cell_cache: &mut Option<((String, usize), Canvas)>,
        cell_next: &mut Option<((String, usize), Canvas)>,
        canvas: &mut Canvas,
        origin: (f32, f32),
        scale: f32,
        f: &lillith_anim::animator::FrameRef<'_>,
        t: &lillith_core::spring::Transform,
        must_draw: bool,
    ) -> Result<Option<(f32, bool)>, StageError> {
        let t = *t;
        let ck = (f.clip_name.to_string(), f.frame);
        // `false` while a stand-in (the previous cell) is drawn.
        let mut exact = true;
        if cell_cache.as_ref().map(|(k, _)| k) != Some(&ck) {
            // MP4 clips stream in; with no picture yet (start-up) block,
            // otherwise wait a moment and keep the old frame if needed.
            if cell_cache.is_none() {
                *cell_cache = Some((ck, pages.cell(f.clip, f.frame)?));
            } else {
                match pages.poll_cell(f.clip, f.frame, VIDEO_FRAME_WAIT)? {
                    Some(c) => *cell_cache = Some((ck, c)),
                    // The canvas was just reallocated (resize) and is still
                    // empty: draw the previous cell as a stand-in rather
                    // than present an empty window while this one decodes.
                    None if must_draw => exact = false,
                    None => return Ok(None),
                }
            }
        }
        canvas.fill(tiny_skia::Color::TRANSPARENT);
        let mut blend = f.blend.filter(|&(_, w)| exact && w > 1.0 / 48.0);
        if let Some((next, _)) = blend {
            let nk = (f.clip_name.to_string(), next);
            if cell_next.as_ref().map(|(k, _)| k) != Some(&nk) {
                // Usually the previous "next" becomes the current cell.
                match pages.poll_cell(f.clip, next, VIDEO_FRAME_WAIT)? {
                    Some(cell) => *cell_next = Some((nk, cell)),
                    // Not decoded yet: draw this frame without the fade.
                    None => blend = None,
                }
            }
        }
        if let Some((next, w)) = blend {
            let a = &cell_cache.as_ref().expect("cell cached").1;
            let b = &cell_next.as_ref().expect("cell cached").1;
            let anchor_b = f.clip.frames[next].anchor;
            draw_cell_mix(
                canvas,
                a,
                f.rect.anchor,
                origin,
                f.flip,
                &t,
                scale,
                1.0 - w,
                tiny_skia::BlendMode::SourceOver,
            );
            draw_cell_mix(
                canvas,
                b,
                anchor_b,
                origin,
                f.flip,
                &t,
                scale,
                w,
                tiny_skia::BlendMode::Plus,
            );
        } else {
            let cell = &cell_cache.as_ref().expect("cell cached").1;
            draw_cell(canvas, cell, f.rect.anchor, origin, f.flip, &t, scale);
        }
        if let Some(m) = &f.mouth {
            match pages.image(&m.patch.page) {
                Ok(patch) => draw_mouth(canvas, patch, m, f.rect.anchor, origin, f.flip, &t, scale),
                Err(e) => tracing::debug!(error = %e, "mouth patch"),
            }
        }
        // Where the bubble tail points: her head (mouth point, else the
        // cell's top centre), in canvas px.
        let head = {
            let tf = frame_transform(f.rect.anchor, origin, f.flip, &t, scale);
            let p = f
                .clip
                .frame_points
                .get("mouth_point")
                .and_then(|v| v.get(f.frame))
                .or_else(|| f.clip.points.get("mouth_point"))
                .map(|&(x, y)| (x as f32, y as f32))
                .unwrap_or((f.rect.anchor.0 as f32, 80.0));
            let mut pt = tiny_skia::Point::from_xy(p.0, p.1);
            tf.map_points(std::slice::from_mut(&mut pt));
            pt.x
        };
        Ok(Some((head, exact)))
    }

    /// The bubble goes on after the mask scan: it never catches clicks.
    fn draw_speech_bubble(&mut self, head: f32) {
        if let Some(s) = &mut self.speech {
            if let Some(v) = &s.view {
                if s.lines.0 != v.text {
                    s.lines = (v.text.clone(), s.layout.lines(&s.fonts, &v.text));
                }
                let tip = s.layout.pad as f32 + 5.0 * self.scale / 0.75;
                draw_speech(
                    &mut self.canvas,
                    &s.fonts,
                    &s.layout,
                    &s.lines.1,
                    head,
                    tip,
                    v.alpha,
                );
            }
        }
    }
}

/// Keeps only the mask above `y_max` (her head): click-through mode.
fn head_only(mask: &[Rect], y_max: f32) -> Vec<Rect> {
    let y_max = y_max.max(0.0) as i32;
    mask.iter()
        .filter(|r| r.y < y_max)
        .map(|r| Rect {
            h: r.h.min(y_max - r.y),
            ..*r
        })
        .collect()
}

/// Clears `mouth_patches` of clips whose patch images don't load, so the
/// talk code falls back to picking frames by their baked mouths.
fn drop_unloadable_mouth_patches(manifest: &mut Manifest, pages: &mut PageCache) {
    for (name, clip) in manifest.clips.iter_mut() {
        if clip.mouth_patches.is_empty() {
            continue;
        }
        let bad: Vec<String> = clip
            .mouth_patches
            .values()
            .filter(|p| pages.image(&p.page).is_err())
            .map(|p| p.page.clone())
            .collect();
        if !bad.is_empty() {
            tracing::warn!(clip = %name, missing = ?bad, "mouth patches missing; using baked mouths");
            clip.mouth_patches.clear();
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 2-clip manifest with real PNG pages: `idle` (2 frames) and `drag_held`.
    pub(crate) fn fixture(tag: &str) -> (std::path::PathBuf, Manifest) {
        let dir = std::env::temp_dir().join(format!("lillith-stage-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut img = image_png(80, 40);
        for y in 5..38u32 {
            for x in 10..30u32 {
                for off in [0u32, 40] {
                    let i = ((y * 80 + x + off) * 4) as usize;
                    img[i..i + 4].copy_from_slice(&[220, 60, 140, 255]);
                }
            }
        }
        write_png(&dir.join("idle.png"), 80, 40, &img);
        let toml = r#"
            [clips.idle]
            page = "idle.png"
            fps = 8.0
            frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 40, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
        "#;
        (dir, Manifest::from_toml_str(toml).unwrap())
    }

    fn image_png(w: u32, h: u32) -> Vec<u8> {
        vec![0u8; (w * h * 4) as usize]
    }

    fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
        let pm =
            tiny_skia::Pixmap::from_vec(rgba.to_vec(), tiny_skia::IntSize::from_wh(w, h).unwrap())
                .unwrap();
        pm.save_png(path).unwrap();
    }

    /// The real art at the default 0.55 scale (with the speech area, as the
    /// app runs it): the Wayland buffer is exactly the surface size, its
    /// border is fully transparent (0x00000000), there is no alpha-0 pixel
    /// with colour, and the input mask only covers her.
    #[test]
    fn bundled_art_buffer_is_transparent_and_surface_sized() {
        let manifest =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/generated/manifest.toml");
        let cfg = AnimatorConfig {
            idle_enabled: false,
            ..AnimatorConfig::default()
        };
        let mut s = Stage::load(&manifest, 0.55, cfg)
            .unwrap()
            .with_speech_area()
            .unwrap();
        s.render().unwrap();
        let (w, h) = s.size();
        assert_eq!((w, h), (s.canvas().width(), s.canvas().height()));
        let mut buf = vec![0xAAu8; (w * h * 4) as usize];
        crate::pixels::rgba_to_argb8888_le(s.canvas().data(), &mut buf, 255);
        let st = crate::wl_geom::alpha_stats(&buf, w, h);
        assert_eq!(st.corners, [0; 4], "{st:?}");
        assert_eq!(st.bad_premul, 0, "{st:?}");
        assert!(st.clear > st.opaque, "mostly transparent: {st:?}");
        let border_alpha: u32 = (0..w)
            .flat_map(|x| [(x, 0), (x, h - 1)])
            .chain((0..h).flat_map(|y| [(0, y), (w - 1, y)]))
            .map(|(x, y)| buf[((y * w + x) * 4 + 3) as usize] as u32)
            .sum();
        assert_eq!(border_alpha, 0, "transparent border");
        let covered: i64 = s.mask().iter().map(|r| (r.w * r.h) as i64).sum();
        assert!(
            covered > 0 && covered < (w * h) as i64 / 2,
            "mask {covered}"
        );
    }

    #[test]
    fn renders_mask_and_skips_identical_frames() {
        let (dir, m) = fixture("stage");
        let cfg = AnimatorConfig {
            idle_enabled: false,
            breath_scale: 0.0,
            breath_chest: 0.0,
            ..AnimatorConfig::default()
        };
        let mut s = Stage::new(m, PageCache::new(&dir), 2.0, cfg).unwrap();
        assert_eq!(s.size(), (80, 80));
        let c = s.render().unwrap();
        assert!(c.pixels && c.mask);
        assert!(!s.mask().is_empty());
        // Body spans x 10..30, y 5..38 in cell px -> ×2 on the canvas.
        assert!(s.hit(40.0, 40.0), "centre of her body");
        assert!(!s.hit(2.0, 2.0), "corner is click-through");
        assert!(!s.hit(75.0, 40.0));
        // Same frame, settled springs: no re-render.
        let n = s.renders();
        assert_eq!(s.render().unwrap(), Changed::default());
        assert_eq!(s.renders(), n);
        // Ticking long enough reaches frame 2 -> re-render.
        let mut any = false;
        for _ in 0..30 {
            any |= s.tick(1.0 / 30.0).unwrap().pixels;
        }
        assert!(any);
        // Re-scale keeps the animator and re-renders at the new size.
        s.set_scale(1.0).unwrap();
        assert_eq!(s.size(), (40, 40));
        assert!(s.render().unwrap().pixels);
        assert!(s.hit(20.0, 20.0));
        assert!(!s.hit(45.0, 20.0));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn smooth_mode_cross_fades_between_frames() {
        let dir = std::env::temp_dir().join(format!("lillith-stage-smooth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Frame 0: block at x 5..15; frame 1: block at x 25..35.
        let mut img = image_png(80, 40);
        for y in 5..38u32 {
            for (x0, off) in [(5u32, 0u32), (25, 40)] {
                for x in x0..x0 + 10 {
                    let i = ((y * 80 + x + off) * 4) as usize;
                    img[i..i + 4].copy_from_slice(&[220, 60, 140, 255]);
                }
            }
        }
        write_png(&dir.join("idle.png"), 80, 40, &img);
        let m = Manifest::from_toml_str(
            r#"
            [clips.idle]
            page = "idle.png"
            fps = 2.0
            frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 40, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
        "#,
        )
        .unwrap();
        let cfg = AnimatorConfig {
            idle_enabled: false,
            smooth_frames: true,
            crossfade: 0.5,
            playback_speed: 1.0,
            ..AnimatorConfig::default()
        };
        let mut st = Stage::new(m, PageCache::new(&dir), 1.0, cfg).unwrap();
        let alpha_at = |st: &Stage, x: u32| st.canvas().pixel(x, 20).unwrap().alpha();
        st.tick(0.05).unwrap();
        // Early in frame 0: hard frame 0, nothing of frame 1 yet.
        assert_eq!(alpha_at(&st, 10), 255);
        assert_eq!(alpha_at(&st, 30), 0);
        // 3/4 through frame 0 (0.5 s frames, fade over the last half):
        // halfway mixed, both blocks semi-transparent.
        let mut steps = Vec::new();
        for _ in 0..9 {
            st.tick(0.05).unwrap();
            steps.push((alpha_at(&st, 10), alpha_at(&st, 30)));
        }
        let mid = steps[5];
        assert!(
            mid.0 > 60 && mid.0 < 200 && mid.1 > 60 && mid.1 < 200,
            "{steps:?}"
        );
        // Monotonic fade: frame 0 goes out, frame 1 comes in.
        for w in steps.windows(2) {
            assert!(w[1].0 <= w[0].0 && w[1].1 >= w[0].1, "{steps:?}");
        }
        // Smooth mode off: hard steps again.
        st.animator_mut().set_smooth_frames(false, 0.5);
        st.tick(0.0).unwrap();
        let a = (alpha_at(&st, 10), alpha_at(&st, 30));
        assert!(a == (255, 0) || a == (0, 255), "{a:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn live_resize_keeps_feet_and_speech_area() {
        let (dir, m) = fixture("resize");
        let mut st = Stage::new(m, PageCache::new(&dir), 2.0, AnimatorConfig::default())
            .unwrap()
            .with_speech_area()
            .unwrap();
        let pad2 = st.speech_pad();
        st.set_scale(4.0).unwrap();
        assert_eq!(st.size().0, 160);
        assert!(st.speech_pad() > pad2, "bubble grows with her");
        assert_eq!(st.size().1, 160 + st.speech_pad());
        st.set_speech_enabled(false).unwrap();
        assert_eq!(st.size(), (160, 160));
        st.render().unwrap();
        assert!(st.hit(40.0, 120.0));
        let _ = std::fs::remove_dir_all(dir);
    }

    fn corners_clear(st: &Stage) -> bool {
        let (w, h) = st.size();
        let px = st.canvas().data();
        [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)]
            .iter()
            .all(|&(x, y)| px[((y * w + x) * 4 + 3) as usize] == 0)
    }

    fn opaque_px(st: &Stage) -> usize {
        st.canvas()
            .data()
            .chunks_exact(4)
            .filter(|p| p[3] == 255)
            .count()
    }

    #[test]
    fn resize_draws_her_into_a_fresh_transparent_canvas() {
        let (dir, m) = fixture("fresh");
        let mut st = Stage::new(m, PageCache::new(&dir), 2.0, AnimatorConfig::default()).unwrap();
        st.render().unwrap();
        assert!(corners_clear(&st) && opaque_px(&st) > 0);
        st.set_scale(3.0).unwrap();
        // The new buffer starts fully transparent (0x00000000), never black.
        assert_eq!(st.size(), (120, 120));
        assert!(st.canvas().data().iter().all(|&b| b == 0), "cleared to 0");
        let c = st.render().unwrap();
        assert!(c.pixels, "first frame after a resize is drawn");
        assert!(corners_clear(&st), "transparent around her after resize");
        assert!(opaque_px(&st) > 0, "she is in the resized buffer");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn held_frame_keeps_the_canvas_while_she_scales() {
        let (dir, m) = fixture("frame");
        let mut st = Stage::new(m, PageCache::new(&dir), 2.0, AnimatorConfig::default()).unwrap();
        st.set_frame(Some(StageFrame {
            scale: 4.0,
            center_x: 70.0,
        }))
        .unwrap();
        assert_eq!(st.size(), (160, 160), "window grows once, to the peak");
        // Still drawn at her normal size, feet on the centre line / floor.
        assert_eq!(st.scale(), 2.0);
        assert_eq!(st.feet_anchor(), (70.0, 160.0 - 2.0 * 2.0));
        st.render().unwrap();
        assert!(corners_clear(&st));
        let small = opaque_px(&st);
        for s in [2.5, 3.0, 3.5, 4.0] {
            st.set_scale(s).unwrap();
            assert_eq!(st.size(), (160, 160), "no resize at {s}");
            assert_eq!(st.feet_anchor(), (70.0, 160.0 - 2.0 * s));
            st.render().unwrap();
            assert!(corners_clear(&st), "transparent at {s}");
        }
        assert!(opaque_px(&st) > small * 3, "she grew inside the frame");
        st.set_scale(2.0).unwrap();
        st.set_frame(None).unwrap();
        assert_eq!(st.size(), (80, 80), "back to her own size");
        assert_eq!(st.feet_anchor(), (40.0, 76.0));
        let _ = std::fs::remove_dir_all(dir);
    }
}
