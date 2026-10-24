//! CPU compositing of an [`Animator`](crate::animator::Animator) frame with
//! `tiny-skia`: crop the cell, mirror, apply the spring transform about the
//! feet anchor, draw into a [`Canvas`].
//!
//! Without layered body parts, the chest channel becomes a subtle vertical
//! squash/stretch and the pigtail channel a slight skew of the upper body
//! (PLAN.md §3.1 design note). Both are tiny by design.

use crate::animator::{FrameRef, MouthDraw};
use crate::atlas::{AtlasError, Clip};
use crate::video::{Poll, VideoCache, VideoConfig};
use crate::Canvas;
use lillith_core::spring::Transform;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
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
/// manifest's directory, and plays clips that have a [`Clip::video`] from
/// their MP4 ([`crate::video::VideoCache`]), falling back to the page when
/// the video is off, ffmpeg is missing or the file is bad.
#[derive(Debug, Default)]
pub struct PageCache {
    dir: PathBuf,
    pages: HashMap<String, Canvas>,
    video: VideoCache,
}

impl PageCache {
    pub fn new(manifest_dir: impl Into<PathBuf>) -> Self {
        Self::with_video(manifest_dir, VideoConfig::from_env())
    }

    /// [`PageCache::new`] with explicit video settings.
    pub fn with_video(manifest_dir: impl Into<PathBuf>, cfg: VideoConfig) -> Self {
        Self {
            dir: manifest_dir.into(),
            pages: HashMap::new(),
            video: VideoCache::new(cfg),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The MP4 decoder cache (stats, config).
    pub fn video(&self) -> &VideoCache {
        &self.video
    }

    /// Bytes held by decoded PNG pages (video frames: [`VideoCache::stats`]).
    pub fn page_bytes(&self) -> usize {
        self.pages.values().map(|p| p.data().len()).sum()
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

    /// Any image under the manifest directory (e.g. a mouth patch), cached
    /// by its relative path.
    pub fn image(&mut self, rel: &str) -> Result<&Canvas, AtlasError> {
        if !self.pages.contains_key(rel) {
            let img = crate::atlas::load_page(&self.dir.join(rel))?;
            let pm = pixmap_from_rgba(&img).ok_or_else(|| AtlasError::InvalidClip {
                clip: rel.to_string(),
                reason: "empty image".into(),
            })?;
            self.pages.insert(rel.to_string(), pm);
        }
        Ok(&self.pages[rel])
    }

    /// `true` if `clip` is (or will be) drawn from its MP4.
    pub fn plays_video(&self, clip: &Clip) -> bool {
        clip.video.is_some()
            && self.video.enabled()
            && clip
                .video_path(&self.dir)
                .is_some_and(|p| !self.video.has_failed(&p))
    }

    /// Starts decoding `clip`'s video in the background (no-op without one).
    pub fn prefetch(&mut self, clip: &Clip) {
        if let Some(p) = clip.video_path(&self.dir) {
            let (w, h) = clip.cell_size();
            self.video.prefetch(&p, w, h, clip.frames.len());
        }
    }

    /// Crops one frame cell out of its page (PNG only, ignores the video).
    pub fn page_cell(&mut self, clip: &Clip, frame: usize) -> Result<Canvas, AtlasError> {
        let r = clip.frames[frame.min(clip.frames.len() - 1)];
        let page = self.page(clip)?;
        IntRect::from_xywh(r.x as i32, r.y as i32, r.w, r.h)
            .and_then(|rect| page.clone_rect(rect))
            .ok_or_else(|| AtlasError::InvalidClip {
                clip: clip.page.clone(),
                reason: format!("frame {frame} rect {r:?} outside page"),
            })
    }

    /// One frame cell: from the clip's MP4 when it has one (blocking until
    /// that frame is decoded), else (or on any video failure) its page.
    pub fn cell(&mut self, clip: &Clip, frame: usize) -> Result<Canvas, AtlasError> {
        if let Some(p) = clip.video_path(&self.dir) {
            let (w, h) = clip.cell_size();
            let i = frame.min(clip.frames.len() - 1);
            if let Some(c) = self.video.wait_frame(&p, w, h, clip.frames.len(), i) {
                return Ok(c);
            }
        }
        self.page_cell(clip, frame)
    }

    /// Non-blocking [`PageCache::cell`] for the render loop: waits at most
    /// `wait` for a video frame still decoding and returns `Ok(None)` if it
    /// isn't there yet (keep showing the previous frame, retry next tick).
    pub fn poll_cell(
        &mut self,
        clip: &Clip,
        frame: usize,
        wait: Duration,
    ) -> Result<Option<Canvas>, AtlasError> {
        if let Some(p) = clip.video_path(&self.dir) {
            let (w, h) = clip.cell_size();
            let i = frame.min(clip.frames.len() - 1);
            match self.video.poll(&p, w, h, clip.frames.len(), i, wait) {
                Poll::Ready(c) => return Ok(Some(c)),
                Poll::Pending => return Ok(None),
                Poll::Unavailable => {}
            }
        }
        self.page_cell(clip, frame).map(Some)
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

/// [`draw_cell`] at `opacity` with `blend` (cross-fades: the outgoing
/// frame at `1 - w` source-over, the incoming one at `w` with
/// [`tiny_skia::BlendMode::Plus`] gives an exact linear mix).
#[allow(clippy::too_many_arguments)]
pub fn draw_cell_mix(
    canvas: &mut Canvas,
    cell: &Canvas,
    anchor: (i32, i32),
    origin: (f32, f32),
    flip: bool,
    t: &Transform,
    scale: f32,
    opacity: f32,
    blend: tiny_skia::BlendMode,
) {
    let tf = frame_transform(anchor, origin, flip, t, scale);
    canvas.draw_pixmap(
        0,
        0,
        cell.as_ref(),
        &PixmapPaint {
            quality: tiny_skia::FilterQuality::Bilinear,
            opacity: opacity.clamp(0.0, 1.0),
            blend_mode: blend,
        },
        tf,
        None,
    );
}

/// Cross-fades frame `a` into frame `b` (`w` = 0 → all `a`, 1 → all `b`)
/// without making her see-through: where both frames cover a pixel the
/// colours mix linearly, but each frame's *coverage* only fades over its own
/// half of the fade ([`coverage_mix`]). A plain linear mix leaves every limb
/// that moved between the two frames at ~50 % alpha mid-fade, so the desktop
/// shows through it (a white ghost/halo around her while she walks or is
/// dragged on a light wallpaper).
#[allow(clippy::too_many_arguments)]
pub fn draw_cell_crossfade(
    canvas: &mut Canvas,
    a: (&Canvas, (i32, i32)),
    b: (&Canvas, (i32, i32)),
    origin: (f32, f32),
    flip: bool,
    t: &Transform,
    scale: f32,
    w: f32,
) {
    let size = (canvas.width(), canvas.height());
    let (Some(mut ca), Some(mut cb)) = (Canvas::new(size.0, size.1), Canvas::new(size.0, size.1))
    else {
        return;
    };
    draw_cell(&mut ca, a.0, a.1, origin, flip, t, scale);
    draw_cell(&mut cb, b.0, b.1, origin, flip, t, scale);
    coverage_mix(canvas, &ca, &cb, w);
}

/// Per-pixel cross-fade of premultiplied `a` and `b` into `dst` (all the
/// same size): colour = the linear mix of the two (straight) colours
/// weighted by coverage, alpha = the union of `a`'s coverage scaled by
/// `min(1, 2(1 - w))` and `b`'s scaled by `min(1, 2w)`. So `a` stays fully
/// opaque while `b` fades in over the first half, then `a` fades out over the
/// second: never both half-transparent at once, and no pop at either end.
pub fn coverage_mix(dst: &mut Canvas, a: &Canvas, b: &Canvas, w: f32) {
    let w = w.clamp(0.0, 1.0);
    let (ka, kb) = ((2.0 * (1.0 - w)).min(1.0), (2.0 * w).min(1.0));
    let (pa, pb) = (a.data(), b.data());
    for (i, d) in dst.data_mut().chunks_exact_mut(4).enumerate() {
        let (sa, sb) = (&pa[i * 4..i * 4 + 4], &pb[i * 4..i * 4 + 4]);
        let (aa, ab) = (sa[3] as f32 / 255.0, sb[3] as f32 / 255.0);
        let den = (1.0 - w) * aa + w * ab;
        if den <= 0.0 {
            d.copy_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let (ca, cb) = (aa * ka, ab * kb);
        let alpha = ca + cb - ca * cb;
        let a8 = (alpha * 255.0).round().min(255.0) as u8;
        for c in 0..3 {
            let straight = ((1.0 - w) * sa[c] as f32 + w * sb[c] as f32) / den;
            d[c] = ((straight * alpha).round() as u8).min(a8);
        }
        d[3] = a8;
    }
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

/// Pastes a mouth patch (`patch` = the [`MouthDraw`]'s image) over a cell
/// drawn with the same `anchor`/`origin`/`flip`/`t`/`scale`: the patch
/// centre lands on the frame's mouth point, rotated by its head tilt and
/// scaled/mirrored with the body.
#[allow(clippy::too_many_arguments)]
pub fn draw_mouth(
    canvas: &mut Canvas,
    patch: &Canvas,
    m: &MouthDraw<'_>,
    anchor: (i32, i32),
    origin: (f32, f32),
    flip: bool,
    t: &Transform,
    scale: f32,
) {
    let tf = mouth_transform(m, anchor, origin, flip, t, scale);
    canvas.draw_pixmap(
        0,
        0,
        patch.as_ref(),
        &PixmapPaint {
            quality: tiny_skia::FilterQuality::Bilinear,
            ..PixmapPaint::default()
        },
        tf,
        None,
    );
}

/// Patch-space → canvas transform used by [`draw_mouth`].
pub fn mouth_transform(
    m: &MouthDraw<'_>,
    anchor: (i32, i32),
    origin: (f32, f32),
    flip: bool,
    t: &Transform,
    scale: f32,
) -> SkTransform {
    frame_transform(anchor, origin, flip, t, scale)
        .pre_translate(m.point.0, m.point.1)
        .pre_rotate(m.angle_deg)
        .pre_translate(-m.patch.centre.0, -m.patch.centre.1)
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
    if let Some(m) = &f.mouth {
        let patch = pages.image(&m.patch.page)?.clone();
        draw_mouth(
            &mut canvas,
            &patch,
            m,
            f.rect.anchor,
            origin,
            f.flip,
            &f.transform,
            scale,
        );
    }
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
    fn crossfade_keeps_coverage_where_a_limb_moved() {
        let px = |c: [u8; 4]| {
            let mut p = Canvas::new(2, 1).unwrap();
            p.data_mut()[..4].copy_from_slice(&c);
            p
        };
        // Pixel 0: only frame a covers it; pixel 1 (left transparent in both).
        let a = px([200, 100, 50, 255]);
        let b = px([0, 0, 0, 0]);
        let mut out = Canvas::new(2, 1).unwrap();
        coverage_mix(&mut out, &a, &b, 0.5);
        assert_eq!(&out.data()[..4], &[200, 100, 50, 255], "opaque mid-fade");
        assert_eq!(&out.data()[4..], &[0, 0, 0, 0]);
        coverage_mix(&mut out, &a, &b, 0.75);
        assert_eq!(out.data()[3], 128, "fades out over the second half");
        assert!(out.data()[0] <= out.data()[3], "stays premultiplied");
        // Only frame b: fades in over the first half.
        coverage_mix(&mut out, &b, &a, 0.25);
        assert_eq!(out.data()[3], 128);
        coverage_mix(&mut out, &b, &a, 0.5);
        assert_eq!(out.data()[3], 255);
        // Both opaque: linear colour mix, fully opaque.
        let c = px([0, 0, 200, 255]);
        coverage_mix(&mut out, &a, &c, 0.5);
        assert_eq!(&out.data()[..4], &[100, 50, 125, 255]);
        // Ends are exactly a and b.
        coverage_mix(&mut out, &a, &c, 0.0);
        assert_eq!(&out.data()[..4], &[200, 100, 50, 255]);
        coverage_mix(&mut out, &a, &c, 1.0);
        assert_eq!(&out.data()[..4], &[0, 0, 200, 255]);
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
