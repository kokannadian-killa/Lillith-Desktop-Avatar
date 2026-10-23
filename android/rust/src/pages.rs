//! LRU atlas page cache with a byte budget. A decoded 25-frame strip of
//! 400×400 cells is ~16 MB; the desktop keeps every page it ever loaded,
//! which a phone can't afford across 130+ clips.

use lillith_anim::atlas::{load_page, AtlasError, Clip};
use lillith_anim::render::pixmap_from_rgba;
use lillith_anim::Canvas;
use std::collections::HashMap;
use std::path::PathBuf;
use tiny_skia::IntRect;

/// Default budget for decoded pages (bytes).
pub const DEFAULT_BUDGET: usize = 160 * 1024 * 1024;

#[derive(Debug)]
pub struct Pages {
    dir: PathBuf,
    budget: usize,
    clock: u64,
    pages: HashMap<String, (Canvas, u64)>,
}

impl Pages {
    pub fn new(dir: impl Into<PathBuf>, budget: usize) -> Self {
        Self {
            dir: dir.into(),
            budget,
            clock: 0,
            pages: HashMap::new(),
        }
    }

    /// Bytes of decoded pages held now.
    pub fn bytes(&self) -> usize {
        self.pages.values().map(|(c, _)| c.data().len()).sum()
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// `true` if `rel` exists under the manifest dir.
    pub fn exists(&self, rel: &str) -> bool {
        self.dir.join(rel).is_file()
    }

    /// The page at `rel` (relative to the manifest dir), loading it and
    /// evicting least-recently-used pages beyond the budget. The page just
    /// requested is never evicted.
    pub fn get(&mut self, rel: &str) -> Result<&Canvas, AtlasError> {
        self.clock += 1;
        let now = self.clock;
        if let Some(e) = self.pages.get_mut(rel) {
            e.1 = now;
        } else {
            let img = load_page(&self.dir.join(rel))?;
            let pm = pixmap_from_rgba(&img).ok_or_else(|| AtlasError::InvalidClip {
                clip: rel.to_string(),
                reason: "empty page".into(),
            })?;
            self.pages.insert(rel.to_string(), (pm, now));
            self.evict(rel);
        }
        Ok(&self.pages[rel].0)
    }

    fn evict(&mut self, keep: &str) {
        while self.bytes() > self.budget && self.pages.len() > 1 {
            let Some(oldest) = self
                .pages
                .iter()
                .filter(|(k, _)| k.as_str() != keep)
                .min_by_key(|(_, (_, t))| *t)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.pages.remove(&oldest);
        }
    }

    /// One frame cell of `clip`.
    pub fn cell(&mut self, clip: &Clip, frame: usize) -> Result<Canvas, AtlasError> {
        let r = clip.frames[frame.min(clip.frames.len().saturating_sub(1))];
        let page = self.get(&clip.page)?;
        IntRect::from_xywh(r.x as i32, r.y as i32, r.w, r.h)
            .and_then(|rect| page.clone_rect(rect))
            .ok_or_else(|| AtlasError::InvalidClip {
                clip: clip.page.clone(),
                reason: format!("frame {frame} rect {r:?} outside page"),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_page(dir: &std::path::Path, name: &str, w: u32, h: u32) {
        let mut img = image_rgba(w, h);
        img[3] = 255;
        let buf = image::RgbaImage::from_raw(w, h, img).unwrap();
        buf.save(dir.join(name)).unwrap();
    }

    fn image_rgba(w: u32, h: u32) -> Vec<u8> {
        vec![0u8; (w * h * 4) as usize]
    }

    #[test]
    fn evicts_least_recently_used_beyond_budget() {
        let dir = std::env::temp_dir().join(format!("lillith-pages-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for n in ["a.png", "b.png", "c.png"] {
            write_page(&dir, n, 64, 64); // 16 KiB decoded each
        }
        let mut p = Pages::new(&dir, 40 * 1024);
        p.get("a.png").unwrap();
        p.get("b.png").unwrap();
        p.get("a.png").unwrap(); // a is now newer than b
        p.get("c.png").unwrap(); // over budget: b goes
        assert_eq!(p.len(), 2);
        assert!(p.pages.contains_key("a.png") && p.pages.contains_key("c.png"));
        assert!(p.bytes() <= 40 * 1024);
        // A single page bigger than the budget is still served.
        let mut tiny = Pages::new(&dir, 1);
        assert!(tiny.get("a.png").is_ok());
        assert_eq!(tiny.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
