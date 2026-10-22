//! The non-UI half of the avatar importer and anchor editor: import
//! summaries, saving a fresh import or an anchor edit, re-guessing
//! anchors, hit-testing / moving anchor points, and the live animated
//! preview. Shared by the settings windows (`lillith-settings-gui`'s
//! Avatar tab and `lillith-panel::settings`) so they only draw widgets.

use crate::avatar::{
    alpha_bbox, draw_avatar, estimate_anchors, Avatar, AvatarError, AvatarMeta, AvatarMotion,
    AvatarStore, MotionInputs, CANVAS,
};
use crate::Canvas;
use image::RgbaImage;
use lillith_core::spring::Transform;
use std::path::PathBuf;

/// One line about what an import did, e.g. "imported cat: background
/// removed, object (jiggle off)".
pub fn import_summary(meta: &AvatarMeta) -> String {
    format!(
        "imported {}: {}{}",
        meta.name,
        if meta.already_transparent {
            "kept its transparency"
        } else {
            "background removed"
        },
        if meta.character {
            ", looks like a character"
        } else {
            ", object (jiggle off)"
        }
    )
}

/// Re-estimates anchors, the character flag and the silhouette box from
/// the fitted image (the editor's "Re-guess anchors").
pub fn reguess_anchors(img: &RgbaImage, meta: &mut AvatarMeta) {
    let (a, character) = estimate_anchors(img);
    meta.anchors = a;
    meta.character = character;
    if let Some(b) = alpha_bbox(img) {
        meta.bbox = b;
    }
}

/// Saves the editor's avatar: a fresh import (`*new`) gets an id and its
/// image written (then `*new` turns false and `meta.id` is set); an
/// existing one only has its `avatar.json` rewritten. Returns the id.
pub fn commit(
    store: &AvatarStore,
    img: &RgbaImage,
    meta: &mut AvatarMeta,
    new: &mut bool,
) -> Result<String, AvatarError> {
    if *new {
        let id = store.save(img, meta)?;
        meta.id = id.clone();
        *new = false;
        Ok(id)
    } else {
        store.save_meta(meta)?;
        Ok(meta.id.clone())
    }
}

/// The stored image of `meta` (for thumbnails).
pub fn image_path(store: &AvatarStore, meta: &AvatarMeta) -> PathBuf {
    store.dir.join("avatars").join(&meta.id).join(&meta.image)
}

/// Editor colours of the anchor dots, in [`crate::avatar::Anchors::named`]
/// order (head, mouth, chest, hip, hair left, hair right).
pub const ANCHOR_COLORS: [[u8; 3]; 6] = [
    [255, 105, 180],
    [255, 80, 80],
    [255, 196, 80],
    [124, 77, 255],
    [90, 230, 255],
    [90, 230, 255],
];

/// The anchor nearest to canvas point `p` within `radius` canvas px.
pub fn nearest_anchor(meta: &AvatarMeta, p: (f32, f32), radius: f32) -> Option<&'static str> {
    meta.anchors
        .named()
        .iter()
        .map(|(n, a)| (*n, ((a.0 - p.0).powi(2) + (a.1 - p.1).powi(2)).sqrt()))
        .filter(|(_, d)| *d <= radius)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(n, _)| n)
}

/// Moves anchor `name` to canvas point `p` (clamped to the canvas).
pub fn move_anchor(meta: &mut AvatarMeta, name: &str, p: (f32, f32)) {
    let c = CANVAS as f32;
    if let Some(a) = meta.anchors.get_mut(name) {
        *a = (p.0.clamp(0.0, c), p.1.clamp(0.0, c));
    }
}

/// Live animated preview of an avatar being edited: the overlay's motion
/// engine (hovered, idle) plus a slow synthetic chest / hair sway so the
/// warp is visible.
pub struct Preview {
    motion: AvatarMotion,
    avatar: Option<Avatar>,
}

impl Preview {
    pub fn new(img: &RgbaImage, meta: &AvatarMeta) -> Self {
        let avatar = crate::render::pixmap_from_rgba(img).map(|image| Avatar {
            meta: meta.clone(),
            image,
            path: PathBuf::new(),
        });
        Self {
            motion: AvatarMotion::default(),
            avatar,
        }
    }

    /// Renders one `side`×`side` frame (premultiplied RGBA), `dt` seconds
    /// after the last one, at time `t` seconds; `meta` carries the edits.
    pub fn render(&mut self, meta: &AvatarMeta, dt: f32, t: f32, side: u32) -> Option<Canvas> {
        let av = self.avatar.as_mut()?;
        av.meta.anchors = meta.anchors;
        av.meta.character = meta.character;
        av.meta.jiggle_strength = meta.jiggle_strength;
        av.meta.bbox = meta.bbox;
        let pose = self.motion.tick(
            dt.min(0.1),
            MotionInputs {
                hovered: true,
                ..Default::default()
            },
        );
        let phase = t * std::f32::consts::TAU * 0.45;
        let tr = Transform {
            chest_offset: 2.5 * (phase * 2.0).sin(),
            pigtail_offset: 4.0 * phase.sin(),
            ..Transform::IDENTITY
        };
        let s = side as f32;
        let mut c = tiny_skia::Pixmap::new(side, side)?;
        draw_avatar(
            &mut c,
            av,
            &pose,
            &tr,
            meta.jiggle_strength,
            (s * 0.5, s * 0.98),
            false,
            s / CANVAS as f32,
        );
        Some(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::avatar::{import_image, ImportOptions};

    fn sample(dir: &std::path::Path) -> PathBuf {
        let mut img = image::RgbaImage::from_pixel(120, 200, image::Rgba([30, 140, 130, 255]));
        for y in 20..190 {
            for x in 40..80 {
                img.put_pixel(x, y, image::Rgba([240, 120, 170, 255]));
            }
        }
        let p = dir.join("buddy.png");
        img.save(&p).unwrap();
        p
    }

    #[test]
    fn import_edit_commit_preview() {
        let dir = std::env::temp_dir().join(format!("lillith-avatar-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (img, mut meta) = import_image(&sample(&dir), ImportOptions::default()).unwrap();
        assert!(import_summary(&meta).starts_with("imported buddy: background removed"));
        // Hit-test and drag the head.
        let head = meta.anchors.head;
        assert_eq!(
            nearest_anchor(&meta, (head.0 + 2.0, head.1), 10.0),
            Some("head")
        );
        assert_eq!(nearest_anchor(&meta, (-500.0, -500.0), 10.0), None);
        move_anchor(&mut meta, "head", (head.0, -40.0));
        assert_eq!(meta.anchors.head, (head.0, 0.0));
        // Preview renders.
        let mut pv = Preview::new(&img, &meta);
        let frame = pv.render(&meta, 1.0 / 30.0, 0.3, 160).expect("frame");
        assert_eq!((frame.width(), frame.height()), (160, 160));
        assert!(frame.data().chunks(4).any(|p| p[3] > 0), "something drawn");
        // Commit: new → saved with an id, then meta-only.
        let store = AvatarStore::new(&dir);
        let mut new = true;
        let id = commit(&store, &img, &mut meta, &mut new).unwrap();
        assert_eq!((id.as_str(), new), ("buddy", false));
        assert!(image_path(&store, &meta).is_file());
        reguess_anchors(&img, &mut meta);
        assert_eq!(meta.anchors.head, head);
        assert_eq!(commit(&store, &img, &mut meta, &mut new).unwrap(), "buddy");
        assert_eq!(store.list().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
