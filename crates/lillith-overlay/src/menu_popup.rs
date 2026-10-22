//! The right-click menu popup (M6), drawn by the overlay itself with
//! `tiny-skia` (+ `cosmic-text` labels, feature `text`) and shown by the
//! backend at the pointer: a small override-redirect window on X11, an
//! `xdg_popup` on her layer surface on Wayland.
//!
//! Layout is an accordion: the top level of [`lillith_core::menu::Menu`],
//! where a submenu row (`Mode ▸`) expands in place to show its leaves,
//! indented. Every leaf is reachable; radio and checkbox state are drawn.
//! The canvas is sized for the tallest expansion, so the popup never has to
//! resize; rows below the content are transparent and outside the input
//! shape.
//!
//! The popup is pure state: the backend feeds pointer motion / clicks /
//! keys in popup-local px and the [`crate::run::Controller`] applies picks.

use crate::input_region::{mask_rects, Rect};
use lillith_core::menu::{build_menu, Check, Menu, MenuAction, MenuItem, MenuState};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect as SkRect, Stroke, Transform};

/// Popup width in px.
pub const WIDTH: u32 = 232;
/// Height of a clickable row.
pub const ROW_H: u32 = 26;
/// Height of a separator row.
pub const SEP_H: u32 = 9;
/// Padding inside the frame.
pub const PAD: u32 = 5;
/// Label font size (px).
pub const FONT_PX: f32 = 14.0;

const BG: (u8, u8, u8, u8) = (28, 22, 38, 246);
const BORDER: (u8, u8, u8, u8) = (176, 108, 255, 255);
const HOVER: (u8, u8, u8, u8) = (122, 64, 196, 255);
const TEXT: (u8, u8, u8, u8) = (240, 232, 250, 255);
const DIM: (u8, u8, u8, u8) = (150, 140, 165, 255);
const SEP: (u8, u8, u8, u8) = (90, 74, 112, 255);

/// One visible row.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// A submenu header; `item` indexes `Menu::items`.
    Header {
        item: usize,
        label: String,
        open: bool,
    },
    /// A leaf; `leaf` is its [`Menu::leaves`] / [`Menu::action_at`] index.
    Leaf {
        leaf: usize,
        action: MenuAction,
        label: String,
        check: Check,
        enabled: bool,
        indent: bool,
    },
    Separator,
}

impl Row {
    fn height(&self) -> u32 {
        match self {
            Row::Separator => SEP_H,
            _ => ROW_H,
        }
    }
    fn selectable(&self) -> bool {
        match self {
            Row::Header { .. } => true,
            Row::Leaf { enabled, .. } => *enabled,
            Row::Separator => false,
        }
    }
}

/// Keyboard navigation inside the popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    Up,
    Down,
    /// Collapse the open submenu.
    Left,
    /// Expand a submenu / pick.
    Right,
    Enter,
    Escape,
}

/// What a click / key did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MenuHit {
    /// Outside the popup (or Esc): close it.
    Dismiss,
    /// A leaf was picked: apply it and close.
    Pick(MenuAction),
    /// Expanded/collapsed a submenu or moved the highlight: redraw.
    Changed,
    /// Nothing happened (separator, disabled row).
    None,
}

/// Label renderer. With feature `text`, `cosmic-text` over the system fonts
/// (loaded once on a background thread at startup, so the first right click
/// does not stall a frame); without it rows are drawn without labels.
pub struct Fonts {
    #[cfg(feature = "text")]
    loader: Option<std::thread::JoinHandle<cosmic_text::FontSystem>>,
    #[cfg(feature = "text")]
    ready: Option<(cosmic_text::FontSystem, cosmic_text::SwashCache)>,
}

impl std::fmt::Debug for Fonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Fonts")
    }
}

impl Fonts {
    /// Starts loading the system fonts in the background.
    pub fn preload() -> Self {
        #[cfg(feature = "text")]
        {
            let loader = std::thread::Builder::new()
                .name("lillith-fonts".into())
                .spawn(cosmic_text::FontSystem::new)
                .ok();
            Self {
                loader,
                ready: None,
            }
        }
        #[cfg(not(feature = "text"))]
        {
            Self {}
        }
    }

    #[cfg(feature = "text")]
    fn get(&mut self) -> &mut (cosmic_text::FontSystem, cosmic_text::SwashCache) {
        if self.ready.is_none() {
            let fs = self
                .loader
                .take()
                .and_then(|h| h.join().ok())
                .unwrap_or_else(cosmic_text::FontSystem::new);
            self.ready = Some((fs, cosmic_text::SwashCache::new()));
        }
        self.ready.as_mut().expect("fonts loaded")
    }

    /// Draws `text` with its top-left at (x, y), clipped to `max_w`.
    #[allow(unused_variables)]
    fn draw(
        &mut self,
        pm: &mut Pixmap,
        text: &str,
        x: i32,
        y: i32,
        max_w: f32,
        rgba: (u8, u8, u8, u8),
    ) {
        #[cfg(feature = "text")]
        {
            use cosmic_text::{Attrs, Buffer, Color as TColor, Family, Metrics, Shaping};
            let (fs, cache) = self.get();
            let mut buf = Buffer::new(fs, Metrics::new(FONT_PX, FONT_PX * 1.3));
            buf.set_size(fs, Some(max_w), Some(FONT_PX * 1.6));
            buf.set_text(
                fs,
                text,
                Attrs::new().family(Family::SansSerif),
                Shaping::Advanced,
            );
            buf.shape_until_scroll(fs, false);
            let (w, h) = (pm.width() as i32, pm.height() as i32);
            let data = pm.data_mut();
            buf.draw(
                fs,
                cache,
                TColor::rgba(rgba.0, rgba.1, rgba.2, rgba.3),
                |gx, gy, _, _, c| {
                    let (px, py) = (x + gx, y + gy);
                    if px < 0 || py < 0 || px >= w || py >= h || gx as f32 > max_w {
                        return;
                    }
                    blend(
                        data,
                        ((py * w + px) * 4) as usize,
                        c.r(),
                        c.g(),
                        c.b(),
                        c.a(),
                    );
                },
            );
        }
    }
}

/// Source-over of a straight-alpha colour onto a premultiplied RGBA pixel.
#[cfg_attr(not(feature = "text"), allow(dead_code))]
fn blend(data: &mut [u8], i: usize, r: u8, g: u8, b: u8, a: u8) {
    if a == 0 {
        return;
    }
    let a32 = a as u32;
    let inv = 255 - a32;
    let mix = |src: u8, dst: u8| ((src as u32 * a32 + dst as u32 * inv + 127) / 255) as u8;
    data[i] = mix(r, data[i]);
    data[i + 1] = mix(g, data[i + 1]);
    data[i + 2] = mix(b, data[i + 2]);
    data[i + 3] = ((a32 * 255 + data[i + 3] as u32 * inv + 127) / 255) as u8;
}

fn paint(c: (u8, u8, u8, u8)) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c.0, c.1, c.2, c.3));
    p.anti_alias = true;
    p
}

/// An open menu: the tree it was built from, the visible rows and the
/// highlight.
#[derive(Debug)]
pub struct MenuPopup {
    pub state: MenuState,
    pub menu: Menu,
    expanded: Option<usize>,
    rows: Vec<Row>,
    hover: Option<usize>,
    size: (u32, u32),
    pixmap: Option<Pixmap>,
    dirty: bool,
}

impl MenuPopup {
    /// The menu for `state` (all submenus collapsed).
    pub fn new(state: MenuState) -> Self {
        let menu = build_menu(&state);
        let size = (WIDTH, max_height(&menu));
        let mut p = Self {
            state,
            menu,
            expanded: None,
            rows: Vec::new(),
            hover: None,
            size,
            pixmap: None,
            dirty: true,
        };
        p.relayout();
        p
    }

    /// Canvas size (fixed while open).
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn expanded(&self) -> Option<usize> {
        self.expanded
    }

    /// Height actually covered by rows (the rest is transparent).
    pub fn content_height(&self) -> u32 {
        content_height(&self.rows)
    }

    /// Input / bounding region (the frame).
    pub fn region(&self) -> Vec<Rect> {
        vec![Rect {
            x: 0,
            y: 0,
            w: self.size.0 as i32,
            h: self.content_height() as i32,
        }]
    }

    /// `true` if (x, y) is on the visible frame.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= 0.0 && y >= 0.0 && x < self.size.0 as f64 && y < self.content_height() as f64
    }

    fn relayout(&mut self) {
        self.rows = rows_of(&self.menu, self.expanded);
        self.dirty = true;
    }

    /// Row under popup-local (x, y).
    pub fn row_at(&self, x: f64, y: f64) -> Option<usize> {
        if !self.contains(x, y) {
            return None;
        }
        let mut top = PAD as f64;
        for (i, r) in self.rows.iter().enumerate() {
            let h = r.height() as f64;
            if y >= top && y < top + h {
                return Some(i);
            }
            top += h;
        }
        None
    }

    /// Pointer moved over the popup (`None` = left it). Returns `true` if
    /// the highlight changed.
    pub fn pointer(&mut self, at: Option<(f64, f64)>) -> bool {
        let h = at
            .and_then(|(x, y)| self.row_at(x, y))
            .filter(|&i| self.rows[i].selectable());
        if h != self.hover {
            self.hover = h;
            self.dirty = true;
            return true;
        }
        false
    }

    /// Left click at popup-local (x, y).
    pub fn click(&mut self, x: f64, y: f64) -> MenuHit {
        if !self.contains(x, y) {
            return MenuHit::Dismiss;
        }
        match self.row_at(x, y) {
            Some(i) => self.activate(i),
            None => MenuHit::None,
        }
    }

    fn activate(&mut self, i: usize) -> MenuHit {
        match self.rows[i].clone() {
            Row::Header { item, .. } => {
                self.expanded = if self.expanded == Some(item) {
                    None
                } else {
                    Some(item)
                };
                self.relayout();
                // Keep the header highlighted.
                self.hover = self
                    .rows
                    .iter()
                    .position(|r| matches!(r, Row::Header { item: it, .. } if *it == item));
                MenuHit::Changed
            }
            Row::Leaf {
                action,
                enabled: true,
                ..
            } => MenuHit::Pick(action),
            _ => MenuHit::None,
        }
    }

    /// Keyboard navigation.
    pub fn key(&mut self, key: MenuKey) -> MenuHit {
        let sel: Vec<usize> = (0..self.rows.len())
            .filter(|&i| self.rows[i].selectable())
            .collect();
        if sel.is_empty() {
            return MenuHit::Dismiss;
        }
        let pos = self.hover.and_then(|h| sel.iter().position(|&i| i == h));
        match key {
            MenuKey::Escape => MenuHit::Dismiss,
            MenuKey::Down | MenuKey::Up => {
                let n = sel.len();
                let next = match (pos, key) {
                    (None, MenuKey::Down) => 0,
                    (None, _) => n - 1,
                    (Some(p), MenuKey::Down) => (p + 1) % n,
                    (Some(p), _) => (p + n - 1) % n,
                };
                self.hover = Some(sel[next]);
                self.dirty = true;
                MenuHit::Changed
            }
            MenuKey::Left => match self.expanded {
                Some(item) => {
                    self.expanded = None;
                    self.relayout();
                    self.hover = self
                        .rows
                        .iter()
                        .position(|r| matches!(r, Row::Header { item: it, .. } if *it == item));
                    MenuHit::Changed
                }
                None => MenuHit::None,
            },
            MenuKey::Right | MenuKey::Enter => match self.hover {
                Some(h) => {
                    if key == MenuKey::Right
                        && matches!(self.rows[h], Row::Header { open: true, .. })
                    {
                        return MenuHit::None;
                    }
                    self.activate(h)
                }
                None => MenuHit::None,
            },
        }
    }

    /// Marks the pixels stale (e.g. the window was exposed).
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// Redraws if something changed; returns the pixels when they did.
    pub fn render(&mut self, fonts: &mut Fonts) -> Option<&Pixmap> {
        if !self.dirty && self.pixmap.is_some() {
            return None;
        }
        let (w, h) = self.size;
        let mut pm = match self.pixmap.take() {
            Some(p) => p,
            None => Pixmap::new(w, h)?,
        };
        pm.fill(Color::TRANSPARENT);
        let ch = self.content_height() as f32;
        // Frame.
        if let Some(path) = rounded(0.5, 0.5, w as f32 - 1.0, ch - 1.0, 6.0) {
            pm.fill_path(
                &path,
                &paint(BG),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
            let stroke = Stroke {
                width: 1.0,
                ..Stroke::default()
            };
            pm.stroke_path(&path, &paint(BORDER), &stroke, Transform::identity(), None);
        }
        let mut top = PAD as f32;
        for (i, row) in self.rows.iter().enumerate() {
            let rh = row.height() as f32;
            if self.hover == Some(i) {
                if let Some(r) = rounded(3.0, top, w as f32 - 6.0, rh, 4.0) {
                    pm.fill_path(
                        &r,
                        &paint(HOVER),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }
            match row {
                Row::Separator => {
                    if let Some(r) =
                        SkRect::from_xywh(10.0, top + (SEP_H / 2) as f32, w as f32 - 20.0, 1.0)
                    {
                        pm.fill_rect(r, &paint(SEP), Transform::identity(), None);
                    }
                }
                Row::Header { label, open, .. } => {
                    fonts.draw(&mut pm, label, 14, label_y(top), w as f32 - 50.0, TEXT);
                    arrow(&mut pm, w as f32 - 18.0, top + rh / 2.0, *open);
                }
                Row::Leaf {
                    label,
                    check,
                    enabled,
                    indent,
                    ..
                } => {
                    let x0 = if *indent { 26.0 } else { 10.0 };
                    let col = if *enabled { TEXT } else { DIM };
                    let tx = match check {
                        Check::None => x0 + 4.0,
                        Check::Checkbox(on) => {
                            checkbox(&mut pm, x0, top + rh / 2.0, *on);
                            x0 + 22.0
                        }
                        Check::Radio(on) => {
                            radio(&mut pm, x0, top + rh / 2.0, *on);
                            x0 + 22.0
                        }
                    };
                    fonts.draw(
                        &mut pm,
                        label,
                        tx as i32,
                        label_y(top),
                        w as f32 - tx - 8.0,
                        col,
                    );
                }
            }
            top += rh;
        }
        self.pixmap = Some(pm);
        self.dirty = false;
        self.pixmap.as_ref()
    }

    /// The last rendered pixels.
    pub fn pixmap(&self) -> Option<&Pixmap> {
        self.pixmap.as_ref()
    }

    /// Bounding/input rectangles of the rendered frame (alpha ≥ `threshold`).
    pub fn mask(&self, threshold: u8) -> Vec<Rect> {
        match &self.pixmap {
            Some(p) => mask_rects(p.data(), p.width(), p.height(), threshold, 1),
            None => self.region(),
        }
    }
}

fn label_y(top: f32) -> i32 {
    (top + (ROW_H as f32 - FONT_PX * 1.3) / 2.0).round() as i32
}

fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// ▸ (closed) / ▾ (open) centred at (cx, cy).
fn arrow(pm: &mut Pixmap, cx: f32, cy: f32, open: bool) {
    let mut pb = PathBuilder::new();
    if open {
        pb.move_to(cx - 5.0, cy - 3.0);
        pb.line_to(cx + 5.0, cy - 3.0);
        pb.line_to(cx, cy + 4.0);
    } else {
        pb.move_to(cx - 3.0, cy - 5.0);
        pb.line_to(cx + 4.0, cy);
        pb.line_to(cx - 3.0, cy + 5.0);
    }
    pb.close();
    if let Some(p) = pb.finish() {
        pm.fill_path(
            &p,
            &paint(TEXT),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn checkbox(pm: &mut Pixmap, x: f32, cy: f32, on: bool) {
    let stroke = Stroke {
        width: 1.5,
        ..Stroke::default()
    };
    if let Some(r) = rounded(x, cy - 7.0, 14.0, 14.0, 3.0) {
        if on {
            pm.fill_path(
                &r,
                &paint(BORDER),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        pm.stroke_path(&r, &paint(TEXT), &stroke, Transform::identity(), None);
    }
    if on {
        let mut pb = PathBuilder::new();
        pb.move_to(x + 3.0, cy);
        pb.line_to(x + 6.0, cy + 3.5);
        pb.line_to(x + 11.0, cy - 3.5);
        if let Some(p) = pb.finish() {
            let s = Stroke {
                width: 2.0,
                ..Stroke::default()
            };
            pm.stroke_path(&p, &paint(TEXT), &s, Transform::identity(), None);
        }
    }
}

fn radio(pm: &mut Pixmap, x: f32, cy: f32, on: bool) {
    let stroke = Stroke {
        width: 1.5,
        ..Stroke::default()
    };
    if let Some(c) = PathBuilder::from_circle(x + 7.0, cy, 6.5) {
        pm.stroke_path(&c, &paint(TEXT), &stroke, Transform::identity(), None);
    }
    if on {
        if let Some(c) = PathBuilder::from_circle(x + 7.0, cy, 3.5) {
            pm.fill_path(
                &c,
                &paint(BORDER),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

/// The visible rows with submenu `expanded` (an index into `menu.items`) open.
pub fn rows_of(menu: &Menu, expanded: Option<usize>) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut leaf = 0usize;
    for (i, it) in menu.items.iter().enumerate() {
        match it {
            MenuItem::Leaf {
                label,
                action,
                check,
                enabled,
            } => {
                rows.push(Row::Leaf {
                    leaf,
                    action: *action,
                    label: label.clone(),
                    check: *check,
                    enabled: *enabled,
                    indent: false,
                });
                leaf += 1;
            }
            MenuItem::Submenu { label, items } => {
                let open = expanded == Some(i);
                rows.push(Row::Header {
                    item: i,
                    label: label.clone(),
                    open,
                });
                // Leaves only (the model nests one level deep).
                for sub in items {
                    if let MenuItem::Leaf {
                        label,
                        action,
                        check,
                        enabled,
                    } = sub
                    {
                        if open {
                            rows.push(Row::Leaf {
                                leaf,
                                action: *action,
                                label: label.clone(),
                                check: *check,
                                enabled: *enabled,
                                indent: true,
                            });
                        }
                        leaf += 1;
                    }
                }
            }
            MenuItem::Separator => rows.push(Row::Separator),
        }
    }
    rows
}

fn content_height(rows: &[Row]) -> u32 {
    rows.iter().map(Row::height).sum::<u32>() + 2 * PAD
}

/// Tallest layout over every possible expansion.
fn max_height(menu: &Menu) -> u32 {
    let mut h = content_height(&rows_of(menu, None));
    for i in 0..menu.items.len() {
        h = h.max(content_height(&rows_of(menu, Some(i))));
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use lillith_core::config::MoveMode;
    use lillith_core::menu::Size;
    use lillith_core::mood::Mood;

    fn state() -> MenuState {
        MenuState {
            mode: MoveMode::Stay,
            mood: Mood::Playful,
            muted: false,
            monitor_paused: false,
            scale: 0.55,
            extras: Default::default(),
        }
    }

    fn row_y(p: &MenuPopup, i: usize) -> f64 {
        let top: u32 = PAD + p.rows()[..i].iter().map(Row::height).sum::<u32>();
        top as f64 + ROW_H as f64 / 2.0
    }

    fn find(p: &MenuPopup, f: impl Fn(&Row) -> bool) -> usize {
        p.rows().iter().position(f).expect("row")
    }

    #[test]
    fn every_leaf_is_reachable_with_its_index() {
        let p = MenuPopup::new(state());
        let n = p.menu.leaves().len();
        let mut seen = vec![false; n];
        for e in 0..p.menu.items.len() {
            for r in rows_of(&p.menu, Some(e)) {
                if let Row::Leaf { leaf, action, .. } = r {
                    assert_eq!(p.menu.action_at(leaf), Some(action));
                    seen[leaf] = true;
                }
            }
        }
        assert!(seen.iter().all(|&s| s), "{seen:?}");
        // Collapsed (no poses): 4 headers (Avatar, Resize, Mode, Mood) +
        // 8 top leaves (Chat, Minimize, Mute, Pause, Settings, Reset,
        // About, Close) + 3 separators.
        assert_eq!(rows_of(&p.menu, None).len(), 15);
        assert!(p.size().1 >= p.content_height());
    }

    #[test]
    fn click_expands_then_picks_and_outside_dismisses() {
        let mut p = MenuPopup::new(state());
        let mood = find(
            &p,
            |r| matches!(r, Row::Header { label, .. } if label == "Mood"),
        );
        assert_eq!(p.click(20.0, row_y(&p, mood)), MenuHit::Changed);
        assert!(matches!(p.rows()[mood], Row::Header { open: true, .. }));
        let alert = find(
            &p,
            |r| matches!(r, Row::Leaf { label, .. } if label == "Alert"),
        );
        assert_eq!(
            p.click(40.0, row_y(&p, alert)),
            MenuHit::Pick(MenuAction::SetMood(Mood::Alert))
        );
        // Radio state is shown on the rows.
        let playful = find(
            &p,
            |r| matches!(r, Row::Leaf { label, .. } if label == "Playful"),
        );
        assert!(matches!(
            p.rows()[playful],
            Row::Leaf {
                check: Check::Radio(true),
                ..
            }
        ));
        assert_eq!(p.click(-5.0, 10.0), MenuHit::Dismiss);
        assert_eq!(p.click(10.0, p.size().1 as f64 + 50.0), MenuHit::Dismiss);
        // Separator does nothing.
        let sep = find(&p, |r| matches!(r, Row::Separator));
        let top: u32 = PAD + p.rows()[..sep].iter().map(Row::height).sum::<u32>();
        assert_eq!(p.click(20.0, top as f64 + 4.0), MenuHit::None);
    }

    #[test]
    fn keyboard_navigation() {
        let mut p = MenuPopup::new(state());
        let mode = find(
            &p,
            |r| matches!(r, Row::Header { label, .. } if label == "Mode"),
        );
        // Chat (disabled without a chat hook) is skipped; Down walks to Mode.
        assert_eq!(p.key(MenuKey::Down), MenuHit::Changed);
        assert_ne!(p.hover, Some(0), "disabled Chat is skipped");
        while p.hover != Some(mode) {
            p.key(MenuKey::Down);
        }
        assert_eq!(p.key(MenuKey::Right), MenuHit::Changed); // expand
        assert!(p.expanded().is_some());
        assert!(matches!(p.rows()[mode], Row::Header { open: true, .. }));
        p.key(MenuKey::Down); // Stay
        p.key(MenuKey::Down); // Wander
        assert_eq!(
            p.key(MenuKey::Enter),
            MenuHit::Pick(MenuAction::SetMode(MoveMode::Wander))
        );
        assert_eq!(p.key(MenuKey::Left), MenuHit::Changed);
        assert_eq!(p.expanded(), None);
        p.key(MenuKey::Up); // the leaf above Mode
        assert_eq!(p.key(MenuKey::Enter), MenuHit::Pick(MenuAction::Minimize));
        assert_eq!(p.key(MenuKey::Escape), MenuHit::Dismiss);
        // From the top, Up wraps to Close.
        let mut p = MenuPopup::new(state());
        p.key(MenuKey::Up);
        assert_eq!(p.key(MenuKey::Enter), MenuHit::Pick(MenuAction::Quit));
        let size = MenuPopup::new(MenuState {
            scale: 0.75,
            ..state()
        });
        assert_eq!(
            size.menu.check_of(MenuAction::SetSize(Size::Large)),
            Some(Check::Radio(true))
        );
    }

    #[test]
    fn renders_frame_and_hover() {
        let mut fonts = Fonts::preload();
        let mut p = MenuPopup::new(state());
        let px = p.render(&mut fonts).expect("first render").clone();
        assert_eq!((px.width(), px.height()), p.size());
        // Inside the frame is opaque-ish, below the content transparent.
        let a = |pm: &Pixmap, x: u32, y: u32| pm.data()[((y * pm.width() + x) * 4 + 3) as usize];
        assert!(a(&px, 100, 8) > 200);
        if p.size().1 > p.content_height() + 2 {
            assert_eq!(a(&px, 100, p.size().1 - 1), 0);
        }
        assert!(p.render(&mut fonts).is_none(), "nothing changed");
        let avatar = find(
            &p,
            |r| matches!(r, Row::Header { label, .. } if label == "Avatar"),
        );
        assert!(p.pointer(Some((20.0, row_y(&p, avatar)))));
        assert!(p.render(&mut fonts).is_some());
        assert!(!p.mask(128).is_empty());
    }
}
