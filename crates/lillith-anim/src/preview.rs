//! Headless clip preview (M2 "every manifest clip is previewable"): renders
//! a clip's frames, raw or played through the [`Animator`] with springs, into
//! contact sheets or per-frame PNGs. Needs no window or compositor, so it
//! works on any box; the egui pose browser in `lillith-panel` uses the same
//! [`render`](crate::render) path.

use crate::animator::{AnimError, Animator, AnimatorConfig, Layer};
use crate::atlas::{AtlasError, Manifest};
use crate::render::{draw_cell, render_frame, PageCache};
use crate::Canvas;
use lillith_core::spring::Transform;
use tiny_skia::{Color, PixmapPaint, Transform as SkTransform};

/// Preview errors.
#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error(transparent)]
    Anim(#[from] AnimError),
    #[error(transparent)]
    Atlas(#[from] AtlasError),
    #[error("png: {0}")]
    Png(String),
}

/// Preview options.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewOpts {
    /// Output cell size in px (cells are square; art is scaled to fit).
    pub cell: u32,
    /// Play through the animator with spring physics instead of raw frames.
    pub physics: bool,
    /// Sampling rate for physics previews.
    pub sample_fps: f32,
    /// Max seconds to simulate for physics previews.
    pub max_s: f32,
}

impl Default for PreviewOpts {
    fn default() -> Self {
        Self {
            cell: 200,
            physics: false,
            sample_fps: 12.0,
            max_s: 6.0,
        }
    }
}

/// One rendered preview frame.
#[derive(Debug, Clone)]
pub struct PreviewFrame {
    pub canvas: Canvas,
    pub clip: String,
    pub frame: usize,
    pub layer: Layer,
    pub transform: Transform,
}

fn cell_scale(manifest: &Manifest, name: &str, cell: u32) -> f32 {
    let c = &manifest.clips[name];
    let side = c
        .frames
        .iter()
        .map(|f| f.w.max(f.h))
        .max()
        .unwrap_or(1)
        .max(1);
    cell as f32 / side as f32
}

/// Renders every frame of `name`. Raw mode: one canvas per frame, no
/// springs. Physics mode: the clip is played through an [`Animator`]
/// (locomotion clips as the base for one cycle, everything else as a
/// reaction over idle) and sampled at `sample_fps`, springs included.
pub fn clip_frames(
    manifest: &Manifest,
    pages: &mut PageCache,
    name: &str,
    opts: &PreviewOpts,
) -> Result<Vec<PreviewFrame>, PreviewError> {
    let clip = manifest
        .clips
        .get(name)
        .ok_or_else(|| AnimError::UnknownClip(name.into()))?;
    let s = cell_scale(manifest, name, opts.cell);
    let mut out = Vec::new();
    if !opts.physics {
        for (i, r) in clip.frames.iter().enumerate() {
            let cell = pages.cell(clip, i)?;
            let mut canvas = Canvas::new(opts.cell, opts.cell).expect("cell > 0");
            let origin = (r.anchor.0 as f32 * s, r.anchor.1 as f32 * s);
            draw_cell(
                &mut canvas,
                &cell,
                r.anchor,
                origin,
                false,
                &Transform::IDENTITY,
                s,
            );
            out.push(PreviewFrame {
                canvas,
                clip: name.into(),
                frame: i,
                layer: Layer::Base,
                transform: Transform::IDENTITY,
            });
        }
        return Ok(out);
    }

    let mut anim = Animator::new(
        manifest.clone(),
        AnimatorConfig {
            idle_enabled: false,
            ..AnimatorConfig::default()
        },
    )?;
    // Let breathing settle into its rhythm before sampling.
    for _ in 0..30 {
        anim.tick(1.0 / 30.0);
    }
    let locomotion = crate::animator::footfall_for(name).is_some() && name != "dance";
    let duration = clip.duration_s().min(opts.max_s);
    if locomotion || name == anim.base() {
        anim.set_base(name, false)?;
    } else {
        anim.react(name, u8::MAX)?;
    }
    let dt = 1.0 / opts.sample_fps.max(1.0);
    let steps = ((duration / dt).ceil() as usize).max(1);
    for _ in 0..steps {
        let f = anim.current();
        let r = f.rect;
        let origin = (r.anchor.0 as f32 * s, r.anchor.1 as f32 * s);
        let canvas = render_frame(pages, &f, opts.cell, opts.cell, origin, s)?;
        out.push(PreviewFrame {
            canvas,
            clip: f.clip_name.to_string(),
            frame: f.frame,
            layer: f.layer,
            transform: f.transform,
        });
        // Sub-tick so physics sees the frame changes (footfalls).
        for _ in 0..4 {
            anim.tick(dt / 4.0);
        }
    }
    Ok(out)
}

/// Lays `frames` out in a grid of `cols` columns over a flat background.
pub fn contact_sheet(frames: &[Canvas], cols: usize, bg: [u8; 4]) -> Canvas {
    let cols = cols.clamp(1, frames.len().max(1));
    let rows = frames.len().div_ceil(cols).max(1);
    let (cw, ch) = frames
        .first()
        .map(|f| (f.width(), f.height()))
        .unwrap_or((1, 1));
    let mut sheet = Canvas::new(cw * cols as u32, ch * rows as u32).expect("non-zero sheet");
    sheet.fill(Color::from_rgba8(bg[0], bg[1], bg[2], bg[3]));
    for (i, f) in frames.iter().enumerate() {
        let (x, y) = ((i % cols) as u32 * cw, (i / cols) as u32 * ch);
        sheet.draw_pixmap(
            x as i32,
            y as i32,
            f.as_ref(),
            &PixmapPaint::default(),
            SkTransform::identity(),
            None,
        );
    }
    sheet
}

/// First frame of every clip, in manifest (alphabetical) order, as one grid.
pub fn overview(
    manifest: &Manifest,
    pages: &mut PageCache,
    cell: u32,
    cols: usize,
) -> Result<(Canvas, Vec<String>), PreviewError> {
    let opts = PreviewOpts {
        cell,
        ..PreviewOpts::default()
    };
    let mut cells = Vec::new();
    let mut names = Vec::new();
    for name in manifest.clips.keys() {
        let mut f = clip_frames(manifest, pages, name, &opts)?;
        let mid = f.len() / 2;
        cells.push(f.swap_remove(mid).canvas);
        names.push(name.clone());
    }
    Ok((contact_sheet(&cells, cols, [72, 72, 80, 255]), names))
}

/// Encodes a canvas as PNG (straight alpha).
pub fn save_png(canvas: &Canvas, path: &std::path::Path) -> Result<(), PreviewError> {
    canvas
        .save_png(path)
        .map_err(|e| PreviewError::Png(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Writes a tiny 2-clip manifest with real PNG pages into a temp dir.
    fn fixture() -> (PathBuf, Manifest) {
        let dir = std::env::temp_dir().join(format!("lillith-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, n) in [("idle", 2u32), ("knife_tap", 3u32)] {
            let mut img = image::RgbaImage::new(40 * n, 40);
            for (x, y, p) in img.enumerate_pixels_mut() {
                if (10..30).contains(&(x % 40)) && (5..38).contains(&y) {
                    *p = image::Rgba([220, 60, 140, 255]);
                }
            }
            img.save(dir.join(format!("{name}.png"))).unwrap();
        }
        let toml = r#"
            [clips.idle]
            page = "idle.png"
            fps = 8.0
            frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 40, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
            [clips.knife_tap]
            page = "knife_tap.png"
            fps = 10.0
            frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 40, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 80, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
        "#;
        (dir, Manifest::from_toml_str(toml).unwrap())
    }

    #[test]
    fn raw_and_physics_previews() {
        let (dir, m) = fixture();
        let mut pages = PageCache::new(&dir);
        let opts = PreviewOpts {
            cell: 40,
            ..PreviewOpts::default()
        };
        let raw = clip_frames(&m, &mut pages, "knife_tap", &opts).unwrap();
        assert_eq!(raw.len(), 3);
        assert!(raw[0].canvas.pixels().iter().any(|p| p.alpha() > 0));
        let phys = clip_frames(
            &m,
            &mut pages,
            "knife_tap",
            &PreviewOpts {
                physics: true,
                ..opts.clone()
            },
        )
        .unwrap();
        assert!(phys.len() >= 3);
        assert!(phys.iter().all(|f| f.clip == "knife_tap"));
        assert!(
            phys.iter().any(|f| f.transform.y < -0.5),
            "springs moved her"
        );
        let sheet = contact_sheet(
            &raw.iter().map(|f| f.canvas.clone()).collect::<Vec<_>>(),
            2,
            [0, 0, 0, 255],
        );
        assert_eq!((sheet.width(), sheet.height()), (80, 80));
        let (ov, names) = overview(&m, &mut pages, 40, 8).unwrap();
        assert_eq!(names, ["idle", "knife_tap"]);
        assert_eq!(ov.width(), 80);
        assert!(clip_frames(&m, &mut pages, "nope", &opts).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
