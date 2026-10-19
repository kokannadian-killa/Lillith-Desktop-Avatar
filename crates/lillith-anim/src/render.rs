//! CPU compositing of an [`Animator`](crate::animator::Animator) frame with
//! `tiny-skia`: crop the cell, mirror, apply the spring transform about the
//! feet anchor, draw into a [`Canvas`].
//!
//! Without layered body parts, the chest channel becomes a subtle vertical
//! squash/stretch and the pigtail channel a slight skew of the upper body
//! (PLAN.md §3.1 design note). Both are tiny by design.

use crate::animator::FrameRef;
use crate::atlas::{AtlasError, Clip};
use crate::Canvas;
use lillith_core::spring::Transform;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tiny_skia::{IntRect, IntSize, PixmapPaint, Transform as SkTransform};

/// Fraction of vertical squash per px of chest offset.
pub const CHEST_SQUASH_PER_PX: f32 = 0.004;
/// Horizontal skew per px of pigtail offset.
pub const PIGTAIL_SKEW_PER_PX: f32 = 0.003;

/// Converts straight RGBA8 into a premultiplied pixmap.
pub fn pixmap_from_rgba(img: &image::RgbaImage) -> Option<Canvas> {
    let mut data = img.as_raw().clone();
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u16;
        for c in &mut px[..3] {
            *c = ((*c as u16 * a + 127) / 255) as u8;
        }
    }
    Canvas::from_vec(data, IntSize::from_wh(img.width(), img.height())?)
}

/// Converts a premultiplied pixmap back to straight RGBA8.
pub fn rgba_from_pixmap(p: &Canvas) -> image::RgbaImage {
    let mut out = image::RgbaImage::new(p.width(), p.height());
    for (dst, src) in out.pixels_mut().zip(p.pixels()) {
        let c = src.demultiply();
        *dst = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    out
}

/// Loads and caches atlas pages (as premultiplied pixmaps) relative to the
/// manifest's directory.
#[derive(Debug, Default)]
pub struct PageCache {
    dir: PathBuf,
    pages: HashMap<String, Canvas>,
}

impl PageCache {
    pub fn new(manifest_dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: manifest_dir.into(),
            pages: HashMap::new(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Page for `clip`, loading it on first use.
    pub fn page(&mut self, clip: &Clip) -> Result<&Canvas, AtlasError> {
        if !self.pages.contains_key(&clip.page) {
            let img = crate::atlas::load_page(&clip.page_path(&self.dir))?;
            let pm = pixmap_from_rgba(&img).ok_or_else(|| AtlasError::InvalidClip {
                clip: clip.page.clone(),
                reason: "empty page".into(),
            })?;
            self.pages.insert(clip.page.clone(), pm);
        }
        Ok(&self.pages[&clip.page])
    }

    /// Crops one frame cell out of its page.
    pub fn cell(&mut self, clip: &Clip, frame: usize) -> Result<Canvas, AtlasError> {
        let r = clip.frames[frame.min(clip.frames.len() - 1)];
        let page = self.page(clip)?;
        IntRect::from_xywh(r.x as i32, r.y as i32, r.w, r.h)
            .and_then(|rect| page.clone_rect(rect))
            .ok_or_else(|| AtlasError::InvalidClip {
                clip: clip.page.clone(),
                reason: format!("frame {frame} rect {r:?} outside page"),
            })
    }
}

/// The affine transform that draws a cell whose feet anchor is `anchor`
/// so the anchor lands at `origin`, mirrored if `flip`, with the spring
/// transform applied about the feet.
pub fn frame_transform(
    anchor: (i32, i32),
    origin: (f32, f32),
    flip: bool,
    t: &Transform,
    scale: f32,
) -> SkTransform {
    let squash = (-t.chest_offset * CHEST_SQUASH_PER_PX).clamp(-0.08, 0.08);
    let sx = t.scale * scale * (1.0 - 0.5 * squash);
    let sy = t.scale * scale * (1.0 + squash);
    let skew = (t.pigtail_offset * PIGTAIL_SKEW_PER_PX).clamp(-0.08, 0.08);
    SkTransform::from_translate(origin.0 + t.x * scale, origin.1 + t.y * scale)
        .pre_rotate(t.angle.to_degrees())
        .pre_scale(sx, sy)
        .pre_concat(SkTransform::from_skew(skew, 0.0))
        .pre_scale(if flip { -1.0 } else { 1.0 }, 1.0)
        .pre_translate(-anchor.0 as f32, -anchor.1 as f32)
}

/// Draws `cell` into `canvas` per [`frame_transform`].
pub fn draw_cell(
    canvas: &mut Canvas,
    cell: &Canvas,
    anchor: (i32, i32),
    origin: (f32, f32),
    flip: bool,
    t: &Transform,
    scale: f32,
) {
    let tf = frame_transform(anchor, origin, flip, t, scale);
    canvas.draw_pixmap(
        0,
        0,
        cell.as_ref(),
        &PixmapPaint {
            quality: tiny_skia::FilterQuality::Bilinear,
            ..PixmapPaint::default()
        },
        tf,
        None,
    );
}

/// Renders the animator's current frame into a fresh `w`×`h` canvas with
/// the feet anchor at `origin`.
pub fn render_frame(
    pages: &mut PageCache,
    f: &FrameRef<'_>,
    w: u32,
    h: u32,
    origin: (f32, f32),
    scale: f32,
) -> Result<Canvas, AtlasError> {
    let mut canvas = Canvas::new(w.max(1), h.max(1)).expect("non-zero canvas");
    let cell = pages.cell(f.clip, f.frame)?;
    draw_cell(
        &mut canvas,
        &cell,
        f.rect.anchor,
        origin,
        f.flip,
        &f.transform,
        scale,
    );
    Ok(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::Point;

    fn map(tf: SkTransform, x: f32, y: f32) -> (f32, f32) {
        let mut p = [Point::from_xy(x, y)];
        tf.map_points(&mut p);
        (p[0].x, p[0].y)
    }

    #[test]
    fn anchor_lands_on_origin() {
        let tf = frame_transform((200, 392), (100.0, 300.0), false, &Transform::IDENTITY, 1.0);
        let (x, y) = map(tf, 200.0, 392.0);
        assert!((x - 100.0).abs() < 1e-3 && (y - 300.0).abs() < 1e-3);
        // Flip mirrors around the anchor.
        let tf = frame_transform((200, 392), (100.0, 300.0), true, &Transform::IDENTITY, 1.0);
        let (x, _) = map(tf, 250.0, 392.0);
        assert!((x - 50.0).abs() < 1e-3, "{x}");
    }

    #[test]
    fn springs_offset_and_squash() {
        let t = Transform {
            y: -10.0,
            chest_offset: 10.0,
            ..Transform::IDENTITY
        };
        let tf = frame_transform((200, 392), (100.0, 300.0), false, &t, 1.0);
        let (_, y) = map(tf, 200.0, 392.0);
        assert!((y - 290.0).abs() < 1e-3);
        // Head (100 px above feet) moves less than 100 px up: squashed.
        let (_, yh) = map(tf, 200.0, 292.0);
        assert!(290.0 - yh < 100.0);
    }

    #[test]
    fn premultiply_round_trip() {
        let mut img = image::RgbaImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgba([200, 100, 50, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 0, 0]));
        let pm = pixmap_from_rgba(&img).unwrap();
        assert_eq!(rgba_from_pixmap(&pm), img);
    }
}
