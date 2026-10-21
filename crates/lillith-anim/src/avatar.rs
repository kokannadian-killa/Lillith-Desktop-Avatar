//! Custom avatars: any single image, imported locally and animated
//! procedurally by the 2.5D engine (Lillith keeps her sprite set).
//!
//! - **Import** ([`import_image`]): load PNG/JPG (WebP through a local
//!   `ffmpeg`/`dwebp` if present), remove a flat background when the image
//!   has no alpha (edge flood-fill with a soft, de-spilled edge; skipped
//!   for "already transparent" images), trim, scale and fit onto the shared
//!   400×400 canvas (baseline 392, anchor [200, 392]), and estimate anchors
//!   from the alpha silhouette ([`estimate_anchors`]).
//! - **Store** ([`AvatarStore`]): `<config dir>/avatars/<id>/{avatar.png,
//!   avatar.json}` plus `<config dir>/avatar.json` with the selection and
//!   recent avatars.
//! - **Motion** ([`AvatarMotion`]): continuous, frame-rate independent
//!   state (idle / walk / talk / react / hover) blended with smoothed
//!   weights; slow eased breathing and sway; hop/glide with lean; low-passed
//!   inputs and clamped velocities so nothing jitters.
//! - **Render** ([`draw_avatar`]): a fine warp mesh (24×24 cells, bilinear,
//!   sub-pixel) deformed by the pose and the engine springs (chest / hair
//!   jiggle at the estimated points, scaled by a strength slider), with a
//!   parallax tilt and a soft drop shadow.

use crate::Canvas;
use image::RgbaImage;
use lillith_core::spring::Transform;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Shared canvas size (px).
pub const CANVAS: u32 = 400;
/// Feet baseline (px from the top).
pub const BASELINE: u32 = 392;
/// Feet anchor on the canvas.
pub const ANCHOR: (i32, i32) = (200, 392);
/// Highest point of a fitted image (px from the top).
pub const TOP_MARGIN: u32 = 20;
/// Default jiggle strength for character images (gentle).
pub const DEFAULT_STRENGTH: f32 = 0.35;
/// Warp mesh resolution (cells per side over the silhouette box).
pub const MESH: usize = 24;

/// Avatar errors.
#[derive(Debug, thiserror::Error)]
pub enum AvatarError {
    #[error("avatar io: {0}")]
    Io(#[from] std::io::Error),
    #[error("avatar image {0}: {1}")]
    Image(String, String),
    #[error("avatar json: {0}")]
    Json(String),
    #[error("the image is empty after background removal")]
    Empty,
}

/// Points on the 400×400 canvas used by the warp.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Anchors {
    pub head: (f32, f32),
    pub mouth: (f32, f32),
    pub chest: (f32, f32),
    pub hip: (f32, f32),
    pub hair_left: (f32, f32),
    pub hair_right: (f32, f32),
}

impl Anchors {
    /// Names and values, in editor order.
    pub fn named(&self) -> [(&'static str, (f32, f32)); 6] {
        [
            ("head", self.head),
            ("mouth", self.mouth),
            ("chest", self.chest),
            ("hip", self.hip),
            ("hair_left", self.hair_left),
            ("hair_right", self.hair_right),
        ]
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut (f32, f32)> {
        Some(match name {
            "head" => &mut self.head,
            "mouth" => &mut self.mouth,
            "chest" => &mut self.chest,
            "hip" => &mut self.hip,
            "hair_left" => &mut self.hair_left,
            "hair_right" => &mut self.hair_right,
            _ => return None,
        })
    }
}

/// `avatar.json` next to each stored avatar image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvatarMeta {
    pub version: u32,
    pub id: String,
    pub name: String,
    /// Fitted 400×400 PNG, relative to the avatar's folder.
    pub image: String,
    /// Original file it was imported from (informational).
    #[serde(default)]
    pub source: String,
    pub anchors: Anchors,
    /// A character (jiggle and talk bob) or an object (gentle sway only).
    pub character: bool,
    /// Chest / hair jiggle strength 0..1 (0 = off).
    pub jiggle_strength: f32,
    /// The import kept the image's own alpha (no background removal).
    #[serde(default)]
    pub already_transparent: bool,
    /// Opaque silhouette box on the canvas (x0, y0, x1, y1).
    pub bbox: (u32, u32, u32, u32),
}

/// Import options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImportOptions {
    /// Keep the image's alpha as is (never flood-fill).
    pub already_transparent: bool,
    /// Colour distance (0..441) still counted as background.
    pub tolerance: f32,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            already_transparent: false,
            tolerance: 42.0,
        }
    }
}

/// Loads PNG/JPG with `image`; other formats (WebP) through a local
/// `dwebp` or `ffmpeg` converting to a temporary PNG (no network).
pub fn load_any(path: &Path) -> Result<RgbaImage, AvatarError> {
    let err = |e: String| AvatarError::Image(path.display().to_string(), e);
    match image::open(path) {
        Ok(img) => return Ok(img.to_rgba8()),
        Err(e) => {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext != "webp" {
                return Err(err(e.to_string()));
            }
        }
    }
    let tmp = std::env::temp_dir().join(format!("lillith-avatar-{}.png", std::process::id()));
    let tries: [(&str, Vec<String>); 2] = [
        (
            "dwebp",
            vec![
                path.display().to_string(),
                "-o".into(),
                tmp.display().to_string(),
            ],
        ),
        (
            "ffmpeg",
            vec![
                "-loglevel".into(),
                "error".into(),
                "-y".into(),
                "-i".into(),
                path.display().to_string(),
                tmp.display().to_string(),
            ],
        ),
    ];
    for (bin, args) in tries {
        let ok = std::process::Command::new(bin)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            let img = image::open(&tmp).map_err(|e| err(e.to_string()))?;
            let _ = std::fs::remove_file(&tmp);
            return Ok(img.to_rgba8());
        }
    }
    Err(err(
        "WebP needs `dwebp` or `ffmpeg` installed (convert it to PNG otherwise)".into(),
    ))
}

/// `true` if the image already has real transparency (>0.5 % of pixels
/// below alpha 250).
pub fn has_alpha(img: &RgbaImage) -> bool {
    let n = img.pixels().filter(|p| p[3] < 250).count();
    n * 200 > (img.width() * img.height()) as usize
}

fn dist(a: [u8; 3], b: [f32; 3]) -> f32 {
    let d = |i: usize| a[i] as f32 - b[i];
    (d(0) * d(0) + d(1) * d(1) + d(2) * d(2)).sqrt()
}

/// Removes a flat-ish background: flood-fills from every border pixel
/// close to the border's median colour, gives the edge a soft alpha ramp
/// and removes the background tint from it. Returns the pixels cleared.
pub fn remove_background(img: &mut RgbaImage, tolerance: f32) -> usize {
    let (w, h) = img.dimensions();
    if w < 3 || h < 3 {
        return 0;
    }
    let mut border = Vec::new();
    for x in 0..w {
        border.push(*img.get_pixel(x, 0));
        border.push(*img.get_pixel(x, h - 1));
    }
    for y in 0..h {
        border.push(*img.get_pixel(0, y));
        border.push(*img.get_pixel(w - 1, y));
    }
    let median = |i: usize| {
        let mut v: Vec<u8> = border.iter().map(|p| p[i]).collect();
        v.sort_unstable();
        v[v.len() / 2] as f32
    };
    let bg = [median(0), median(1), median(2)];
    let tol = tolerance.max(1.0);
    let idx = |x: u32, y: u32| (y * w + x) as usize;
    let mut seen = vec![false; (w * h) as usize];
    let mut stack = Vec::new();
    let px = |img: &RgbaImage, x: u32, y: u32| {
        let p = img.get_pixel(x, y);
        [p[0], p[1], p[2]]
    };
    for x in 0..w {
        for y in [0, h - 1] {
            if dist(px(img, x, y), bg) < tol {
                stack.push((x, y));
            }
        }
    }
    for y in 0..h {
        for x in [0, w - 1] {
            if dist(px(img, x, y), bg) < tol {
                stack.push((x, y));
            }
        }
    }
    let mut cleared = 0;
    while let Some((x, y)) = stack.pop() {
        let i = idx(x, y);
        if seen[i] || dist(px(img, x, y), bg) >= tol {
            continue;
        }
        seen[i] = true;
        cleared += 1;
        if x > 0 {
            stack.push((x - 1, y));
        }
        if x + 1 < w {
            stack.push((x + 1, y));
        }
        if y > 0 {
            stack.push((x, y - 1));
        }
        if y + 1 < h {
            stack.push((x, y + 1));
        }
    }
    // Enclosed holes (between an arm and the body, inside a tail loop):
    // background-coloured regions the border fill cannot reach. Clear
    // those that are big enough not to be a detail of the subject, with
    // a slightly stricter tolerance.
    let hole_tol = tol * 0.75;
    let min_hole = ((w * h) as f32 * 0.0001).max(16.0) as usize;
    let mut probe = vec![false; (w * h) as usize];
    for sy in 0..h {
        for sx in 0..w {
            let si = idx(sx, sy);
            if seen[si] || probe[si] || dist(px(img, sx, sy), bg) >= hole_tol {
                continue;
            }
            let mut comp = Vec::new();
            stack.push((sx, sy));
            while let Some((x, y)) = stack.pop() {
                let i = idx(x, y);
                if seen[i] || probe[i] || dist(px(img, x, y), bg) >= hole_tol {
                    continue;
                }
                probe[i] = true;
                comp.push(i);
                if x > 0 {
                    stack.push((x - 1, y));
                }
                if x + 1 < w {
                    stack.push((x + 1, y));
                }
                if y > 0 {
                    stack.push((x, y - 1));
                }
                if y + 1 < h {
                    stack.push((x, y + 1));
                }
            }
            if comp.len() >= min_hole {
                cleared += comp.len();
                for i in comp {
                    seen[i] = true;
                }
            }
        }
    }
    // Soft edge: pixels next to the cleared area get alpha from how far
    // their colour is from the background (tol..2·tol), tint removed.
    let mut out = img.clone();
    for y in 0..h {
        for x in 0..w {
            let i = idx(x, y);
            if seen[i] {
                out.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
                continue;
            }
            let near = [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)]
                .iter()
                .any(|(dx, dy)| {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    nx >= 0
                        && ny >= 0
                        && nx < w as i32
                        && ny < h as i32
                        && seen[idx(nx as u32, ny as u32)]
                });
            if !near {
                continue;
            }
            let p = *img.get_pixel(x, y);
            let d = dist([p[0], p[1], p[2]], bg);
            let a = ((d - tol) / tol).clamp(0.25, 1.0);
            let un =
                |c: u8, b: f32| ((c as f32 - (1.0 - a) * b) / a).round().clamp(0.0, 255.0) as u8;
            out.put_pixel(
                x,
                y,
                image::Rgba([
                    un(p[0], bg[0]),
                    un(p[1], bg[1]),
                    un(p[2], bg[2]),
                    (p[3] as f32 * a) as u8,
                ]),
            );
        }
    }
    *img = out;
    cleared
}

/// Opaque (alpha ≥ 32) bounding box `(x0, y0, x1, y1)` (exclusive end).
pub fn alpha_bbox(img: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = img.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p[3] >= 32 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

/// Trims to the silhouette, scales it to fit the canvas (≤ 372 px tall,
/// ≤ 388 px wide) and places it bottom-centre on the baseline.
pub fn fit_to_canvas(img: &RgbaImage) -> Result<RgbaImage, AvatarError> {
    let (x0, y0, x1, y1) = alpha_bbox(img).ok_or(AvatarError::Empty)?;
    let crop = image::imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image();
    let (cw, ch) = crop.dimensions();
    let max_h = (BASELINE - TOP_MARGIN) as f32;
    let max_w = (CANVAS - 12) as f32;
    let k = (max_h / ch as f32).min(max_w / cw as f32);
    let (nw, nh) = (
        ((cw as f32 * k).round() as u32).max(1),
        ((ch as f32 * k).round() as u32).max(1),
    );
    let scaled = image::imageops::resize(&crop, nw, nh, image::imageops::FilterType::Lanczos3);
    let mut out = RgbaImage::new(CANVAS, CANVAS);
    let ox = (CANVAS as i64 - nw as i64) / 2;
    let oy = BASELINE as i64 - nh as i64;
    image::imageops::overlay(&mut out, &scaled, ox, oy);
    Ok(out)
}

/// Estimates anchors from the alpha silhouette of a fitted image (head,
/// mouth, chest, hip from height bands; hair at the head band's outer
/// edges), and whether it looks like a character (tall, narrower head).
pub fn estimate_anchors(img: &RgbaImage) -> (Anchors, bool) {
    let (x0, y0, x1, y1) = alpha_bbox(img).unwrap_or((150, 20, 250, BASELINE));
    let hgt = (y1 - y0).max(1) as f32;
    let band = |a: f32, b: f32| {
        let (ya, yb) = (
            y0 + (a * hgt) as u32,
            (y0 + (b * hgt) as u32).max(y0 + (a * hgt) as u32 + 1),
        );
        let (mut sx, mut n, mut lo, mut hi) = (0f64, 0u64, u32::MAX, 0u32);
        for y in ya..yb.min(img.height()) {
            for x in x0..x1 {
                if img.get_pixel(x, y)[3] >= 128 {
                    sx += x as f64;
                    n += 1;
                    lo = lo.min(x);
                    hi = hi.max(x);
                }
            }
        }
        let cx = if n > 0 {
            (sx / n as f64) as f32
        } else {
            (x0 + x1) as f32 / 2.0
        };
        (cx, lo.min(hi) as f32, hi as f32, n)
    };
    let at = |f: f32| y0 as f32 + f * hgt;
    let (hx, hl, hr, _) = band(0.02, 0.16);
    let (cx, _, _, _) = band(0.32, 0.40);
    let (px, _, _, _) = band(0.49, 0.57);
    let (_, ml, mr, _) = band(0.0, 1.0);
    let width = (x1 - x0) as f32;
    let head_w = (hr - hl).max(1.0);
    let character = hgt / width.max(1.0) >= 1.3 && head_w < 0.85 * (mr - ml).max(1.0);
    let a = Anchors {
        // Anime proportions (big head, often horns/ears/hair on top of
        // the bbox): tuned on full-body character art.
        head: (hx, at(0.12)),
        mouth: (hx, at(0.185)),
        chest: (cx, at(0.36)),
        hip: (px, at(0.53)),
        hair_left: (hl + 0.15 * head_w, at(0.20)),
        hair_right: (hr - 0.15 * head_w, at(0.20)),
    };
    (a, character)
}

/// Full local import: load, (maybe) remove background, fit, estimate.
pub fn import_image(
    path: &Path,
    opts: ImportOptions,
) -> Result<(RgbaImage, AvatarMeta), AvatarError> {
    let mut img = load_any(path)?;
    let transparent = opts.already_transparent || has_alpha(&img);
    if !transparent {
        remove_background(&mut img, opts.tolerance);
    }
    let fitted = fit_to_canvas(&img)?;
    let (anchors, character) = estimate_anchors(&fitted);
    let bbox = alpha_bbox(&fitted).ok_or(AvatarError::Empty)?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("avatar")
        .to_string();
    let meta = AvatarMeta {
        version: 1,
        id: String::new(),
        name,
        image: "avatar.png".into(),
        source: path.display().to_string(),
        anchors,
        character,
        jiggle_strength: if character { DEFAULT_STRENGTH } else { 0.0 },
        already_transparent: transparent,
        bbox,
    };
    Ok((fitted, meta))
}

/// Which avatar is selected, and recent custom ones (`avatar.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AvatarSelection {
    /// `None` = Lillith.
    pub selected: Option<String>,
    #[serde(default)]
    pub recent: Vec<String>,
}

/// Custom avatars under `<dir>` (the user config dir).
#[derive(Debug, Clone)]
pub struct AvatarStore {
    pub dir: PathBuf,
}

/// A loaded custom avatar.
#[derive(Debug, Clone)]
pub struct Avatar {
    pub meta: AvatarMeta,
    /// Premultiplied 400×400 image.
    pub image: Canvas,
    pub path: PathBuf,
}

impl AvatarStore {
    /// Store in `config_dir` (e.g. `~/.config/lillith`).
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: config_dir.into(),
        }
    }

    /// Store next to the config file `config_path`, else the default one.
    pub fn for_config(config_path: Option<&Path>) -> Option<Self> {
        let dir = match config_path.and_then(Path::parent) {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => lillith_core::config::Config::default_path()?
                .parent()?
                .to_path_buf(),
        };
        Some(Self::new(dir))
    }

    fn avatars(&self) -> PathBuf {
        self.dir.join("avatars")
    }

    fn selection_path(&self) -> PathBuf {
        self.dir.join("avatar.json")
    }

    pub fn selection(&self) -> AvatarSelection {
        std::fs::read_to_string(self.selection_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Selects `id` (`None` = Lillith) and moves it to the front of recents.
    pub fn select(&self, id: Option<&str>) -> Result<AvatarSelection, AvatarError> {
        let mut s = self.selection();
        s.selected = id.map(str::to_string);
        if let Some(id) = id {
            s.recent.retain(|r| r != id);
            s.recent.insert(0, id.to_string());
            s.recent.truncate(8);
        }
        s.recent
            .retain(|r| self.avatars().join(r).join("avatar.json").is_file());
        std::fs::create_dir_all(&self.dir)?;
        let json =
            serde_json::to_string_pretty(&s).map_err(|e| AvatarError::Json(e.to_string()))?;
        std::fs::write(self.selection_path(), json)?;
        Ok(s)
    }

    /// Saves an imported avatar (new id from its name) and returns the id.
    pub fn save(&self, img: &RgbaImage, meta: &AvatarMeta) -> Result<String, AvatarError> {
        let mut slug: String = meta
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        slug.truncate(32);
        let slug = slug.trim_matches('-').to_string();
        let base = if slug.is_empty() {
            "avatar".to_string()
        } else {
            slug
        };
        let id = if meta.id.is_empty() {
            let mut n = 1;
            let mut id = base.clone();
            while self.avatars().join(&id).exists() {
                n += 1;
                id = format!("{base}-{n}");
            }
            id
        } else {
            meta.id.clone()
        };
        let dir = self.avatars().join(&id);
        std::fs::create_dir_all(&dir)?;
        img.save(dir.join("avatar.png"))
            .map_err(|e| AvatarError::Image("avatar.png".into(), e.to_string()))?;
        let mut m = meta.clone();
        m.id = id.clone();
        m.image = "avatar.png".into();
        self.save_meta(&m)?;
        Ok(id)
    }

    /// Rewrites an avatar's `avatar.json` (anchor edits).
    pub fn save_meta(&self, m: &AvatarMeta) -> Result<(), AvatarError> {
        let json = serde_json::to_string_pretty(m).map_err(|e| AvatarError::Json(e.to_string()))?;
        std::fs::write(self.avatars().join(&m.id).join("avatar.json"), json)?;
        Ok(())
    }

    /// Stored avatars (id order).
    pub fn list(&self) -> Vec<AvatarMeta> {
        let mut v: Vec<AvatarMeta> = std::fs::read_dir(self.avatars())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path().join("avatar.json")).ok())
            .filter_map(|s| serde_json::from_str(&s).ok())
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub fn load(&self, id: &str) -> Result<Avatar, AvatarError> {
        let dir = self.avatars().join(id);
        let s = std::fs::read_to_string(dir.join("avatar.json"))?;
        let meta: AvatarMeta =
            serde_json::from_str(&s).map_err(|e| AvatarError::Json(e.to_string()))?;
        let path = dir.join(&meta.image);
        let img = image::open(&path)
            .map_err(|e| AvatarError::Image(path.display().to_string(), e.to_string()))?
            .to_rgba8();
        let image = crate::render::pixmap_from_rgba(&img).ok_or(AvatarError::Empty)?;
        Ok(Avatar { meta, image, path })
    }
}

// ---------------------------------------------------------------- motion

/// Smoothly approaches a target: critically damped (`zeta` = 1) or a touch
/// under-damped for soft settles; frame-rate independent sub-stepping and
/// a velocity clamp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Smooth {
    pub value: f32,
    pub vel: f32,
    /// Natural frequency (rad/s): lower = slower, unhurried.
    pub omega: f32,
    pub zeta: f32,
    pub max_vel: f32,
}

impl Smooth {
    pub fn new(value: f32, omega: f32, zeta: f32, max_vel: f32) -> Self {
        Self {
            value,
            vel: 0.0,
            omega,
            zeta,
            max_vel,
        }
    }

    pub fn step(&mut self, target: f32, dt: f32) -> f32 {
        let n = (dt / (1.0 / 240.0)).ceil().clamp(1.0, 64.0) as usize;
        let h = dt / n as f32;
        for _ in 0..n {
            let acc = self.omega * self.omega * (target - self.value)
                - 2.0 * self.zeta * self.omega * self.vel;
            self.vel = (self.vel + acc * h).clamp(-self.max_vel, self.max_vel);
            self.value += self.vel * h;
        }
        self.value
    }
}

fn ease_in_out(x: f32) -> f32 {
    // Smooth cosine ease: zero slope at both ends.
    0.5 - 0.5 * (std::f32::consts::PI * x.clamp(0.0, 1.0)).cos()
}

/// Inputs to [`AvatarMotion::tick`] (raw; the motion low-passes them).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MotionInputs {
    /// Walking direction (-1 left, +1 right), `None` = standing.
    pub walk: Option<f32>,
    /// Mouth openness while talking (0..1), `None` = silent.
    pub talk: Option<f32>,
    pub hovered: bool,
    /// Being dragged (horizontal velocity, px/s).
    pub dragged: Option<f32>,
}

/// Blended state weights (each 0..1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Weights {
    pub walk: f32,
    pub talk: f32,
    pub hover: f32,
    pub react: f32,
}

/// Continuous procedural motion for a custom avatar.
#[derive(Debug, Clone)]
pub struct AvatarMotion {
    t: f32,
    walk_w: Smooth,
    talk_w: Smooth,
    hover_w: Smooth,
    react_w: Smooth,
    react_left: f32,
    dir: Smooth,
    mouth: Smooth,
    drag: Smooth,
    hop_phase: f32,
    /// Overall pace (1 = default unhurried; Ko likes it slow).
    pub pace: f32,
}

impl Default for AvatarMotion {
    fn default() -> Self {
        Self::new(1.0)
    }
}

/// The pose [`draw_avatar`] renders.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AvatarPose {
    /// Breathing -1..1 (eased).
    pub breath: f32,
    /// Lean/sway angle (rad, + = clockwise) about the feet.
    pub sway: f32,
    /// Lift off the ground (px, up).
    pub hop: f32,
    /// Vertical squash (+) / stretch (-) about the feet.
    pub squash: f32,
    /// Head bob from talking (px, up).
    pub talk: f32,
    /// Parallax tilt -1..1 (top shifts sideways, slight depth squash).
    pub tilt: f32,
    pub weights: Weights,
}

impl AvatarMotion {
    pub fn new(pace: f32) -> Self {
        let s = |omega, zeta| Smooth::new(0.0, omega, zeta, 8.0);
        Self {
            t: 0.0,
            walk_w: s(3.0, 1.0),
            talk_w: s(4.0, 1.0),
            hover_w: s(2.5, 1.0),
            react_w: s(5.0, 0.8),
            react_left: 0.0,
            dir: Smooth::new(0.0, 2.5, 1.0, 4.0),
            mouth: Smooth::new(0.0, 14.0, 0.9, 20.0),
            drag: Smooth::new(0.0, 4.0, 1.0, 3000.0),
            hop_phase: 0.0,
            pace: pace.clamp(0.2, 3.0),
        }
    }

    /// A click/poke/reaction: a soft bounce that eases out (s).
    pub fn react(&mut self, seconds: f32) {
        self.react_left = self.react_left.max(seconds);
    }

    pub fn weights(&self) -> Weights {
        Weights {
            walk: self.walk_w.value.clamp(0.0, 1.0),
            talk: self.talk_w.value.clamp(0.0, 1.0),
            hover: self.hover_w.value.clamp(0.0, 1.0),
            react: self.react_w.value.clamp(0.0, 1.0),
        }
    }

    /// Advances by `dt` (real seconds) and returns the pose.
    pub fn tick(&mut self, dt: f32, i: MotionInputs) -> AvatarPose {
        let dt = dt.clamp(0.0, 0.1);
        let pace = self.pace;
        self.t += dt * pace;
        let t = self.t;
        self.walk_w
            .step(if i.walk.is_some() { 1.0 } else { 0.0 }, dt);
        self.talk_w
            .step(if i.talk.is_some() { 1.0 } else { 0.0 }, dt);
        self.hover_w.step(if i.hovered { 1.0 } else { 0.0 }, dt);
        self.react_left = (self.react_left - dt).max(0.0);
        self.react_w
            .step(if self.react_left > 0.0 { 1.0 } else { 0.0 }, dt);
        let dir = self.dir.step(i.walk.unwrap_or(0.0), dt);
        let mouth = self.mouth.step(i.talk.unwrap_or(0.0), dt);
        let drag = self.drag.step(i.dragged.unwrap_or(0.0), dt);
        let w = self.weights();

        // Idle: slow eased breathing (~4.6 s) and a lazy sway (~7 s).
        let breath_phase = (t / 4.6).fract();
        let breath = ease_in_out(if breath_phase < 0.45 {
            breath_phase / 0.45
        } else {
            1.0 - (breath_phase - 0.45) / 0.55
        }) * 2.0
            - 1.0;
        let idle_sway = 0.012 * (t * std::f32::consts::TAU / 7.0).sin()
            + 0.005 * (t * std::f32::consts::TAU / 3.1 + 1.3).sin();

        // Walk: unhurried hop/glide (~1.7 hops/s at pace 1) with a lean.
        self.hop_phase = (self.hop_phase + dt * pace * 1.7 * w.walk.max(0.05)).fract();
        let hop_s = (self.hop_phase * std::f32::consts::PI).sin();
        let hop = w.walk * 7.0 * hop_s * hop_s;
        let land = w.walk * 0.03 * (1.0 - hop_s).powi(4);
        let lean = 0.07 * dir * w.walk;

        // Talk: gentle head bob following the low-passed mouth.
        let talk = w.talk * (1.2 + 1.8 * mouth);
        // Hover: a slow float.
        let hover = w.hover * 2.5 * (0.5 - 0.5 * (t * std::f32::consts::TAU / 2.4).cos());
        // React: a soft bounce that settles.
        let react = w.react * 0.035 * (t * 9.0).sin() * (1.0 - w.react * 0.3);
        // Drag: lean against the motion (low-passed, clamped).
        let drag_lean = (-drag / 2500.0).clamp(-0.12, 0.12);

        AvatarPose {
            breath,
            sway: idle_sway * (1.0 - 0.6 * w.walk) + lean + drag_lean,
            hop: hop + hover,
            squash: land + react + 0.01 * breath * (1.0 - w.walk),
            talk,
            tilt: (0.6 * dir * w.walk + idle_sway * 8.0).clamp(-1.0, 1.0),
            weights: w,
        }
    }
}

// ---------------------------------------------------------------- render

fn gauss(d2: f32, r: f32) -> f32 {
    (-d2 / (2.0 * r * r)).exp()
}

/// Canvas → output mapping of a mesh vertex (cell coords → canvas px).
#[allow(clippy::too_many_arguments)]
fn warp_point(
    m: &AvatarMeta,
    p: &AvatarPose,
    t: &Transform,
    strength: f32,
    x: f32,
    y: f32,
    origin: (f32, f32),
    flip: bool,
    scale: f32,
) -> (f32, f32) {
    let a = &m.anchors;
    let (bx0, by0, _bx1, _) = m.bbox;
    let top = by0 as f32;
    let base = BASELINE as f32;
    let hgt = (base - top).max(1.0);
    let hf = ((base - y) / hgt).clamp(0.0, 1.2); // 0 at feet, 1 at top
    let (mut dx, mut dy) = (0.0f32, 0.0f32);
    let r = hgt * 0.11;
    let s = strength.clamp(0.0, 1.0);
    if m.character && s > 0.0 {
        // Chest jiggle (engine chest spring) and breathing swell.
        let gc = gauss((x - a.chest.0).powi(2) + (y - a.chest.1).powi(2), r);
        dy += gc * (t.chest_offset * 0.9 * s - 0.8 * p.breath * s);
        dx += gc * (x - a.chest.0) * 0.02 * p.breath * s;
        // Hair sway (pigtail spring), hanging below the hair points.
        for hp in [a.hair_left, a.hair_right] {
            let below = ((y - hp.1) / r + 1.0).clamp(0.0, 3.0);
            let gh = gauss((x - hp.0).powi(2) + (y - hp.1 - r).powi(2), r * 1.3);
            dx += gh * below * t.pigtail_offset * 0.6 * s;
        }
    }
    if m.character {
        // Talk: the head bobs, the neck stretches a little.
        let gh = gauss((x - a.head.0).powi(2) + (y - a.head.1).powi(2), r * 1.4);
        dy -= gh * p.talk;
    }
    // Breathing: tiny vertical scale about the feet (all images).
    let sy = 1.0 + p.squash * -1.0 + 0.006 * p.breath;
    let sx = 1.0 + p.squash * 0.6;
    let mut cx = ANCHOR.0 as f32 + (x - ANCHOR.0 as f32) * sx + dx;
    let mut cy = base - (base - y) * sy + dy;
    // Sway bends more towards the top (a soft curve, not a rigid tilt).
    let bend = p.sway * (base - cy) * (0.4 + 0.6 * hf);
    cx += bend;
    // Parallax: the top shifts sideways, slight depth squash on the far side.
    cx += p.tilt * 5.0 * hf;
    cx = ANCHOR.0 as f32 + (cx - ANCHOR.0 as f32) * (1.0 - 0.02 * p.tilt.abs());
    cy -= p.hop;
    let _ = bx0;
    // Spring transform (no global squash/skew: the mesh does it locally).
    let tt = Transform {
        chest_offset: 0.0,
        pigtail_offset: 0.0,
        ..*t
    };
    let tf = crate::render::frame_transform(ANCHOR, origin, flip, &tt, scale);
    let mut pt = tiny_skia::Point::from_xy(cx, cy);
    tf.map_points(std::slice::from_mut(&mut pt));
    (pt.x, pt.y)
}

/// Draws `av` posed by `pose` and the springs `t` into `canvas` with its
/// feet at `origin`: drop shadow first, then the warped image.
#[allow(clippy::too_many_arguments)]
pub fn draw_avatar(
    canvas: &mut Canvas,
    av: &Avatar,
    pose: &AvatarPose,
    t: &Transform,
    strength: f32,
    origin: (f32, f32),
    flip: bool,
    scale: f32,
) {
    let m = &av.meta;
    // Shadow: soft ellipse under the feet, smaller/lighter while lifted.
    let (bx0, by0, bx1, by1) = m.bbox;
    let width = (bx1 - bx0) as f32 * scale;
    let lift = (pose.hop / 10.0).clamp(0.0, 1.0);
    let sw = (width * 0.42 * (1.0 - 0.25 * lift)).max(6.0);
    let sh = (sw * 0.16).max(2.0);
    let cx = origin.0 + t.x * scale + pose.tilt * 3.0 * scale;
    let cy = origin.1 - 1.5 * scale;
    if let Some(rect) = tiny_skia::Rect::from_xywh(cx - sw, cy - sh, 2.0 * sw, 2.0 * sh) {
        let path = tiny_skia::PathBuilder::from_oval(rect);
        if let Some(path) = path {
            for (k, a) in [(1.0f32, 10u8), (0.7, 12), (0.45, 14)] {
                let mut paint = tiny_skia::Paint::default();
                paint.set_color_rgba8(30, 6, 30, (a as f32 * (1.0 - 0.5 * lift)) as u8);
                paint.anti_alias = true;
                let tf = tiny_skia::Transform::from_translate(cx, cy)
                    .pre_scale(k, k)
                    .pre_translate(-cx, -cy);
                canvas.fill_path(&path, &paint, tiny_skia::FillRule::Winding, tf, None);
            }
        }
    }
    // Mesh over the silhouette box (+ margin for the hair swing).
    let pad = 6.0;
    let (x0, y0) = ((bx0 as f32 - pad).max(0.0), (by0 as f32 - pad).max(0.0));
    let (x1, y1) = (
        (bx1 as f32 + pad).min(CANVAS as f32),
        (by1 as f32 + 1.0).min(CANVAS as f32),
    );
    let n = MESH;
    let mut src = Vec::with_capacity((n + 1) * (n + 1));
    let mut dst = Vec::with_capacity((n + 1) * (n + 1));
    for j in 0..=n {
        for i in 0..=n {
            let x = x0 + (x1 - x0) * i as f32 / n as f32;
            let y = y0 + (y1 - y0) * j as f32 / n as f32;
            src.push((x, y));
            dst.push(warp_point(m, pose, t, strength, x, y, origin, flip, scale));
        }
    }
    let at = |i: usize, j: usize| j * (n + 1) + i;
    for j in 0..n {
        for i in 0..n {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            fill_textured(
                canvas,
                &av.image,
                [dst[a], dst[b], dst[c]],
                [src[a], src[b], src[c]],
            );
            fill_textured(
                canvas,
                &av.image,
                [dst[a], dst[c], dst[d]],
                [src[a], src[c], src[d]],
            );
        }
    }
}

/// Bilinear premultiplied sample at `(u, v)` (pixel-centre convention).
fn sample(img: &Canvas, u: f32, v: f32) -> [f32; 4] {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let (fx, fy) = (u - 0.5, v - 0.5);
    let (x0, y0) = (fx.floor() as i32, fy.floor() as i32);
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let data = img.data();
    let get = |x: i32, y: i32| -> [f32; 4] {
        if x < 0 || y < 0 || x >= w || y >= h {
            return [0.0; 4];
        }
        let k = ((y * w + x) * 4) as usize;
        [
            data[k] as f32,
            data[k + 1] as f32,
            data[k + 2] as f32,
            data[k + 3] as f32,
        ]
    };
    let (a, b, c, d) = (
        get(x0, y0),
        get(x0 + 1, y0),
        get(x0, y0 + 1),
        get(x0 + 1, y0 + 1),
    );
    let mut o = [0.0; 4];
    for k in 0..4 {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bot = c[k] + (d[k] - c[k]) * tx;
        o[k] = top + (bot - top) * ty;
    }
    o
}

/// Fills a destination triangle with the source triangle's texels
/// (affine, bilinear, premultiplied source-over, top-left fill rule).
fn fill_textured(canvas: &mut Canvas, img: &Canvas, d: [(f32, f32); 3], s: [(f32, f32); 3]) {
    let area = (d[1].0 - d[0].0) * (d[2].1 - d[0].1) - (d[2].0 - d[0].0) * (d[1].1 - d[0].1);
    if area.abs() < 1e-4 {
        return;
    }
    let (cw, ch) = (canvas.width() as i32, canvas.height() as i32);
    let minx = d
        .iter()
        .map(|p| p.0)
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as i32;
    let maxx = (d.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil() as i32).min(cw - 1);
    let miny = d
        .iter()
        .map(|p| p.1)
        .fold(f32::MAX, f32::min)
        .floor()
        .max(0.0) as i32;
    let maxy = (d.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil() as i32).min(ch - 1);
    if minx > maxx || miny > maxy {
        return;
    }
    let sign = area.signum();
    // Edge function of edge (a -> b) at p, oriented so inside is >= 0.
    let edge = |a: (f32, f32), b: (f32, f32), p: (f32, f32)| {
        sign * ((b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0))
    };
    // Top-left rule: edges that are "top" or "left" include their pixels.
    let tl = |a: (f32, f32), b: (f32, f32)| {
        let (ex, ey) = ((b.0 - a.0) * sign, (b.1 - a.1) * sign);
        (ey == 0.0 && ex < 0.0) || ey > 0.0
    };
    let edges = [(d[1], d[2]), (d[2], d[0]), (d[0], d[1])];
    let incl = [tl(d[1], d[2]), tl(d[2], d[0]), tl(d[0], d[1])];
    let inv = 1.0 / area.abs();
    let data = canvas.data_mut();
    for y in miny..=maxy {
        for x in minx..=maxx {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let mut w = [0.0f32; 3];
            let mut inside = true;
            for k in 0..3 {
                let e = edge(edges[k].0, edges[k].1, p);
                if e < 0.0 || (e == 0.0 && !incl[k]) {
                    inside = false;
                    break;
                }
                w[k] = e * inv;
            }
            if !inside {
                continue;
            }
            let u = w[0] * s[0].0 + w[1] * s[1].0 + w[2] * s[2].0;
            let v = w[0] * s[0].1 + w[1] * s[1].1 + w[2] * s[2].1;
            let c = sample(img, u, v);
            let a = c[3] / 255.0;
            if a <= 0.0 {
                continue;
            }
            let k = ((y * cw + x) * 4) as usize;
            for j in 0..4 {
                let v = c[j] + data[k + j] as f32 * (1.0 - a);
                data[k + j] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pink "character" (head circle + body) on a flat teal background.
    pub(crate) fn sample_character(w: u32, h: u32) -> RgbaImage {
        let mut img = RgbaImage::from_pixel(w, h, image::Rgba([40, 160, 150, 255]));
        let cx = w as f32 / 2.0;
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32, y as f32);
                let head =
                    (fx - cx).powi(2) + (fy - h as f32 * 0.2).powi(2) < (w as f32 * 0.12).powi(2);
                let body =
                    (fx - cx).abs() < w as f32 * 0.2 && fy > h as f32 * 0.3 && fy < h as f32 * 0.92;
                if head || body {
                    img.put_pixel(x, y, image::Rgba([236, 120, 170, 255]));
                }
            }
        }
        img
    }

    #[test]
    fn background_removal_fit_and_anchors() {
        let mut img = sample_character(200, 300);
        assert!(!has_alpha(&img));
        let n = remove_background(&mut img, 42.0);
        assert!(n > 30_000, "cleared {n}");
        assert!(has_alpha(&img));
        assert_eq!(img.get_pixel(2, 2)[3], 0);
        assert_eq!(img.get_pixel(100, 200)[3], 255);
        let fit = fit_to_canvas(&img).unwrap();
        assert_eq!(fit.dimensions(), (CANVAS, CANVAS));
        let (x0, y0, x1, y1) = alpha_bbox(&fit).unwrap();
        assert_eq!(y1, BASELINE, "feet on the baseline");
        assert!(y0 >= TOP_MARGIN - 1);
        assert!(((x0 + x1) as i32 / 2 - 200).abs() <= 2, "centred");
        let (a, character) = estimate_anchors(&fit);
        assert!(character);
        assert!(a.head.1 < a.mouth.1 && a.mouth.1 < a.chest.1 && a.chest.1 < a.hip.1);
        assert!((a.head.0 - 200.0).abs() < 4.0);
        assert!(a.hair_left.0 < a.head.0 && a.hair_right.0 > a.head.0);
    }

    #[test]
    fn enclosed_background_holes_are_cleared() {
        // A ring (like a tail loop) on a flat background: the inside hole
        // is not reachable from the border but must still go; a tiny
        // background-coloured dot inside the subject stays.
        let bg = image::Rgba([46, 158, 142, 255]);
        let mut img = RgbaImage::from_pixel(120, 120, bg);
        for y in 20..100 {
            for x in 20..100 {
                img.put_pixel(x, y, image::Rgba([200, 40, 140, 255]));
            }
        }
        for y in 40..80 {
            for x in 40..80 {
                img.put_pixel(x, y, bg);
            }
        }
        img.put_pixel(25, 25, bg);
        remove_background(&mut img, 42.0);
        assert_eq!(img.get_pixel(60, 60)[3], 0, "hole cleared");
        assert_eq!(img.get_pixel(5, 5)[3], 0, "outside cleared");
        assert!(img.get_pixel(25, 25)[3] > 0, "tiny detail kept");
        assert_eq!(img.get_pixel(30, 30)[3], 255, "subject kept");
    }

    #[test]
    fn wide_object_is_not_a_character() {
        let mut img = RgbaImage::new(300, 120);
        for y in 10..110 {
            for x in 10..290 {
                img.put_pixel(x, y, image::Rgba([250, 200, 40, 255]));
            }
        }
        let fit = fit_to_canvas(&img).unwrap();
        let (_, character) = estimate_anchors(&fit);
        assert!(!character);
    }

    #[test]
    fn store_saves_selects_and_loads() {
        let dir = std::env::temp_dir().join(format!("lillith-avatars-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("in/Cute Girl.png");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        sample_character(200, 300).save(&src).unwrap();
        let (img, meta) = import_image(&src, ImportOptions::default()).unwrap();
        assert!(!meta.already_transparent && meta.character);
        assert!(meta.jiggle_strength > 0.0);
        let store = AvatarStore::new(&dir);
        let id = store.save(&img, &meta).unwrap();
        assert_eq!(id, "cute-girl");
        assert_eq!(store.save(&img, &meta).unwrap(), "cute-girl-2");
        store.select(Some(&id)).unwrap();
        let s = store.selection();
        assert_eq!(s.selected.as_deref(), Some("cute-girl"));
        assert_eq!(s.recent[0], "cute-girl");
        let av = store.load(&id).unwrap();
        assert_eq!(av.image.width(), CANVAS);
        assert_eq!(store.list().len(), 2);
        store.select(None).unwrap();
        assert_eq!(store.selection().selected, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn motion_is_smooth_and_frame_rate_independent() {
        let walk = MotionInputs {
            walk: Some(1.0),
            ..Default::default()
        };
        // Same 3 s at 30 and 144 Hz end in (nearly) the same pose.
        let run = |hz: f32| {
            let mut m = AvatarMotion::default();
            let mut last = m.tick(0.0, MotionInputs::default());
            let mut max_step = 0.0f32;
            let n = (3.0 * hz) as usize;
            for k in 0..n {
                let i = if k < n / 2 {
                    walk
                } else {
                    MotionInputs::default()
                };
                let p = m.tick(1.0 / hz, i);
                max_step = max_step.max((p.hop - last.hop).abs() * hz / 60.0);
                last = p;
            }
            (last, max_step)
        };
        let (a, sa) = run(30.0);
        let (b, sb) = run(144.0);
        assert!(
            (a.weights.walk - b.weights.walk).abs() < 0.03,
            "{a:?} {b:?}"
        );
        assert!((a.sway - b.sway).abs() < 0.01);
        // No jumps: per-60Hz-frame change of the hop stays small.
        assert!(sa < 2.0 && sb < 2.0, "{sa} {sb}");
        // Weights cross-fade: never a hard 0 -> 1 switch in one frame.
        let mut m = AvatarMotion::default();
        let p = m.tick(1.0 / 60.0, walk);
        assert!(p.weights.walk > 0.0 && p.weights.walk < 0.1);
    }

    #[test]
    fn warped_render_draws_and_moves_smoothly() {
        let mut img = sample_character(200, 300);
        remove_background(&mut img, 42.0);
        let fit = fit_to_canvas(&img).unwrap();
        let (anchors, character) = estimate_anchors(&fit);
        let av = Avatar {
            meta: AvatarMeta {
                version: 1,
                id: "t".into(),
                name: "t".into(),
                image: "avatar.png".into(),
                source: String::new(),
                anchors,
                character,
                jiggle_strength: 0.5,
                already_transparent: false,
                bbox: alpha_bbox(&fit).unwrap(),
            },
            image: crate::render::pixmap_from_rgba(&fit).unwrap(),
            path: PathBuf::new(),
        };
        let render = |pose: &AvatarPose, t: &Transform| {
            let mut c = Canvas::new(300, 300).unwrap();
            draw_avatar(&mut c, &av, pose, t, 0.5, (150.0, 294.0), false, 0.75);
            c
        };
        let rest = render(&AvatarPose::default(), &Transform::IDENTITY);
        let ink = rest.data().chunks_exact(4).filter(|p| p[3] > 200).count();
        // Fitted silhouette area x 0.75^2, roughly.
        assert!(ink > 8_000, "ink {ink}");
        // No seams: the inside of the body is fully opaque.
        let body = rest.pixel(150, 200).unwrap();
        assert_eq!(body.alpha(), 255);
        // A hop lifts it smoothly (sub-pixel: 0.5 px moves the edge a bit).
        let lifted = render(
            &AvatarPose {
                hop: 0.5,
                ..Default::default()
            },
            &Transform::IDENTITY,
        );
        assert_ne!(lifted.data(), rest.data());
        // Chest spring moves pixels near the chest only.
        let jig = render(
            &AvatarPose::default(),
            &Transform {
                chest_offset: 6.0,
                ..Transform::IDENTITY
            },
        );
        let diff_rows: Vec<u32> = (0..300u32)
            .filter(|&y| (0..300u32).any(|x| jig.pixel(x, y) != rest.pixel(x, y)))
            .collect();
        assert!(!diff_rows.is_empty());
        assert!(*diff_rows.last().unwrap() < 270, "feet stay put");
    }
}
