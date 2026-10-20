//! The backend-independent "stage": owns the M2 [`Animator`] and atlas
//! pages, renders the current frame into a reusable transparent canvas at
//! the overlay size, and keeps the click-through mask in sync.
//!
//! Backends call [`Stage::tick`] at `render_hz`; it returns what changed so
//! they only re-upload pixels / input region when needed (idle CPU).

use crate::input_region::{mask_rects, rects_hit, Rect};
use lillith_anim::animator::{Animator, AnimatorConfig, Layer};
use lillith_anim::atlas::Manifest;
use lillith_anim::render::{draw_cell, PageCache};
use lillith_anim::Canvas;
use std::path::Path;

/// Alpha at or above which a pixel catches clicks.
pub const HIT_ALPHA: u8 = 40;
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
}

pub struct Stage {
    anim: Animator,
    /// Last cropped cell, keyed by (clip, frame): crops only on frame change.
    cell: Option<((String, usize), Canvas)>,
    /// Coarse key of the last mask: the mask is only rescanned when the
    /// frame changes or the body moved by ≥ 1 px / 0.3° / 0.5 %.
    mask_key: Option<Key>,
    render_time: std::time::Duration,
    pages: PageCache,
    canvas: Canvas,
    scale: f32,
    /// Manifest cell (largest frame) and idle feet anchor, unscaled.
    cell_size: (u32, u32),
    anchor: (f32, f32),
    origin: (f32, f32),
    key: Option<Key>,
    mask: Vec<Rect>,
    renders: u64,
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
        let anim = Animator::new(manifest, cfg)?;
        // Feet anchor of the idle base: every clip shares the canvas/baseline.
        let a = anim.current().rect.anchor;
        let w = (cw as f32 * scale).round() as u32;
        let h = (ch as f32 * scale).round() as u32;
        let canvas = Canvas::new(w, h).ok_or(StageError::Size(w, h))?;
        Ok(Self {
            anim,
            cell: None,
            mask_key: None,
            render_time: std::time::Duration::ZERO,
            pages,
            canvas,
            scale,
            cell_size: (cw, ch),
            anchor: (a.0 as f32, a.1 as f32),
            origin: (a.0 as f32 * scale, a.1 as f32 * scale),
            key: None,
            mask: Vec::new(),
            renders: 0,
        })
    }

    /// Current scale (overlay size = cell × scale).
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Re-sizes the stage to `scale` (menu Size ▸), keeping the animator,
    /// its springs and the decoded pages. The next [`Stage::render`] draws
    /// the new size; the backend must resize its window to [`Stage::size`].
    pub fn set_scale(&mut self, scale: f32) -> Result<(), StageError> {
        let scale = scale.clamp(0.1, 4.0);
        let w = (self.cell_size.0 as f32 * scale).round() as u32;
        let h = (self.cell_size.1 as f32 * scale).round() as u32;
        self.canvas = Canvas::new(w, h).ok_or(StageError::Size(w, h))?;
        self.scale = scale;
        self.origin = (self.anchor.0 * scale, self.anchor.1 * scale);
        self.key = None;
        self.mask_key = None;
        self.mask.clear();
        Ok(())
    }

    pub fn size(&self) -> (u32, u32) {
        (self.canvas.width(), self.canvas.height())
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
        self.render()
    }

    /// Renders the current animator frame if it differs from the last one.
    pub fn render(&mut self) -> Result<Changed, StageError> {
        let f = self.anim.current();
        let t = f.transform;
        // Quantisation steps: fine for pixels, coarse for the mask.
        let quant = |px: f32, ang: f32, sc: f32| Key {
            clip: f.clip_name.to_string(),
            frame: f.frame,
            flip: f.flip,
            q: [
                (t.x / px).round() as i32,
                (t.y / px).round() as i32,
                (t.angle / ang).round() as i32,
                (t.scale / sc).round() as i32,
                (t.chest_offset / (2.0 * px)).round() as i32,
                (t.pigtail_offset / (2.0 * px)).round() as i32,
            ],
        };
        let key = quant(0.25, 0.001, 0.002);
        if self.key.as_ref() == Some(&key) {
            return Ok(Changed::default());
        }
        let started = std::time::Instant::now();
        let ck = (f.clip_name.to_string(), f.frame);
        if self.cell.as_ref().map(|(k, _)| k) != Some(&ck) {
            let cell = self.pages.cell(f.clip, f.frame)?;
            self.cell = Some((ck, cell));
        }
        let cell = &self.cell.as_ref().expect("cell cached").1;
        self.canvas.fill(tiny_skia::Color::TRANSPARENT);
        draw_cell(
            &mut self.canvas,
            cell,
            f.rect.anchor,
            self.origin,
            f.flip,
            &t,
            self.scale,
        );
        self.key = Some(key);
        self.renders += 1;
        let mkey = quant(1.0 / self.scale.max(0.1), 0.005, 0.005);
        let mut mask_changed = false;
        if self.mask_key.as_ref() != Some(&mkey) {
            let mask = mask_rects(
                self.canvas.data(),
                self.canvas.width(),
                self.canvas.height(),
                HIT_ALPHA,
                MASK_GRID,
            );
            mask_changed = mask != self.mask;
            self.mask = mask;
            self.mask_key = Some(mkey);
        }
        self.render_time += started.elapsed();
        Ok(Changed {
            pixels: true,
            mask: mask_changed,
        })
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
}
