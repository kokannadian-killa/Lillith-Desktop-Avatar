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
use crate::relic::{self, Accent};
use lillith_core::menu::{build_menu, Check, Menu, MenuAction, MenuItem, MenuState};
use lillith_theme::palette as pal;
use lillith_theme::Rgba;
use tiny_skia::Pixmap;

/// Popup width in px.
pub const WIDTH: u32 = 244;
/// Height of a clickable row.
pub const ROW_H: u32 = 26;
/// Height of a separator row.
pub const SEP_H: u32 = 9;
/// Padding between the panel edge and the rows.
pub const PAD: u32 = 2;
/// Black-iron frame band (top and sides).
pub const FRAME: u32 = 7;
/// Bottom frame band (carries the carved runes).
pub const BOTTOM: u32 = 14;
/// Header plate (sigil + "LILLITH").
pub const HEADER_H: u32 = 32;
/// Top of the first row.
pub const TOP: u32 = FRAME + HEADER_H + PAD;
/// Label font size (px).
pub const FONT_PX: f32 = 14.0;
/// Header title size (px).
pub const TITLE_PX: f32 = 17.0;

/// Which bundled face a label is set in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Chakra Petch SemiBold (rows).
    Label,
    /// Cinzel Decorative Bold (the relic title).
    Title,
}

impl Face {
    #[cfg_attr(not(feature = "text"), allow(dead_code))]
    fn px(self) -> f32 {
        match self {
            Face::Label => FONT_PX,
            Face::Title => TITLE_PX,
        }
    }
}

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
/// plus the bundled relic faces from [`lillith_theme::fonts`] (loaded once
/// on a background thread at startup, so the first right click does not
/// stall a frame); without it rows are drawn without labels.
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

#[cfg(feature = "text")]
fn font_system() -> cosmic_text::FontSystem {
    let mut fs = cosmic_text::FontSystem::new();
    let db = fs.db_mut();
    for bytes in [
        lillith_theme::fonts::CHAKRA_PETCH_SEMIBOLD,
        lillith_theme::fonts::CINZEL_DECORATIVE_BOLD,
    ] {
        db.load_font_data(bytes.to_vec());
    }
    fs
}

impl Fonts {
    /// Starts loading the fonts in the background.
    pub fn preload() -> Self {
        #[cfg(feature = "text")]
        {
            let loader = std::thread::Builder::new()
                .name("lillith-fonts".into())
                .spawn(font_system)
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
                .unwrap_or_else(font_system);
            self.ready = Some((fs, cosmic_text::SwashCache::new()));
        }
        self.ready.as_mut().expect("fonts loaded")
    }

    /// Draws `text` in `face` with its top-left at (x, y), clipped to `max_w`.
    #[allow(unused_variables, clippy::too_many_arguments)]
    fn draw(
        &mut self,
        pm: &mut Pixmap,
        text: &str,
        x: i32,
        y: i32,
        max_w: f32,
        rgba: Rgba,
        face: Face,
    ) {
        #[cfg(feature = "text")]
        {
            use cosmic_text::{Attrs, Buffer, Color as TColor, Family, Metrics, Shaping, Weight};
            let px = face.px();
            let attrs = match face {
                Face::Label => Attrs::new()
                    .family(Family::Name("Chakra Petch"))
                    .weight(Weight::SEMIBOLD),
                Face::Title => Attrs::new()
                    .family(Family::Name("Cinzel Decorative"))
                    .weight(Weight::BOLD),
            };
            let (fs, cache) = self.get();
            let mut buf = Buffer::new(fs, Metrics::new(px, px * 1.3));
            buf.set_size(fs, Some(max_w), Some(px * 1.6));
            buf.set_text(fs, text, attrs, Shaping::Advanced);
            buf.shape_until_scroll(fs, false);
            let (w, h) = (pm.width() as i32, pm.height() as i32);
            let data = pm.data_mut();
            buf.draw(
                fs,
                cache,
                TColor::rgba(rgba.r, rgba.g, rgba.b, rgba.a),
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
    accent: Accent,
    /// Static layers (frame, header, rows without hover) per expansion:
    /// a hover change only copies one and draws the blade + one label.
    base: Vec<(Option<usize>, Pixmap)>,
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
            accent: Accent::LILLITH,
            base: Vec::new(),
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

    /// Top edge (popup-local px) of row `i`.
    pub fn row_top(&self, i: usize) -> u32 {
        TOP + self.rows[..i.min(self.rows.len())]
            .iter()
            .map(Row::height)
            .sum::<u32>()
    }

    /// Row under popup-local (x, y).
    pub fn row_at(&self, x: f64, y: f64) -> Option<usize> {
        if !self.contains(x, y) {
            return None;
        }
        let mut top = TOP as f64;
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

    /// The avatar-matched accent (default: Lillith's magenta).
    pub fn accent(&self) -> Accent {
        self.accent
    }

    /// Re-tints the popup for the active avatar (drops the cached layers).
    pub fn set_accent(&mut self, accent: Accent) {
        if accent != self.accent {
            self.accent = accent;
            self.base.clear();
            self.dirty = true;
        }
    }

    /// Marks the pixels stale (e.g. the window was exposed).
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// Redraws if something changed; returns the pixels when they did.
    /// The frame and the un-hovered rows come from a cached static layer;
    /// only the knife highlight and its label are drawn per change.
    pub fn render(&mut self, fonts: &mut Fonts) -> Option<&Pixmap> {
        if !self.dirty && self.pixmap.is_some() {
            return None;
        }
        let key = self.expanded;
        if !self.base.iter().any(|(k, _)| *k == key) {
            let layer = self.paint_base(fonts)?;
            if self.base.len() >= 8 {
                self.base.remove(0);
            }
            self.base.push((key, layer));
        }
        let base = &self.base.iter().find(|(k, _)| *k == key)?.1;
        let mut pm = match self.pixmap.take() {
            Some(p) if p.width() == base.width() && p.height() == base.height() => p,
            _ => base.clone(),
        };
        pm.data_mut().copy_from_slice(base.data());
        if let Some(i) = self.hover.filter(|&i| i < self.rows.len()) {
            let top = self.row_top(i) as f32;
            draw_row(&mut pm, fonts, &self.rows, i, top, true, &self.accent);
        }
        self.pixmap = Some(pm);
        self.dirty = false;
        self.pixmap.as_ref()
    }

    /// The static layer for the current expansion.
    fn paint_base(&self, fonts: &mut Fonts) -> Option<Pixmap> {
        let (w, h) = self.size;
        let mut pm = Pixmap::new(w, h)?;
        paint_chrome(&mut pm, fonts, self.content_height() as f32, &self.accent);
        let mut top = TOP as f32;
        for i in 0..self.rows.len() {
            draw_row(&mut pm, fonts, &self.rows, i, top, false, &self.accent);
            top += self.rows[i].height() as f32;
        }
        Some(pm)
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

/// `true` for the Quit/Close leaf.
fn is_quit(row: &Row) -> bool {
    matches!(
        row,
        Row::Leaf {
            action: MenuAction::Quit,
            ..
        }
    )
}

/// The iron frame, red terminal panel, blood drips, skulls and the header
/// (sigil + "LILLITH"), for a frame `ch` px tall.
fn paint_chrome(pm: &mut Pixmap, fonts: &mut Fonts, ch: f32, accent: &Accent) {
    let w = pm.width() as f32;
    let (f, b) = (FRAME as f32, BOTTOM as f32);
    relic::iron_frame(pm, [0.0, 0.0, w, ch], f, b, 3);
    let panel = [f, f, w - f, ch - b];
    relic::terminal_panel(pm, panel);
    relic::brackets(pm, panel, 6.0, pal::BLOOD_NEON, false, true);
    // header plate: a darker blood wash fading down, closed by a blade
    let hb = f + HEADER_H as f32;
    relic::fill_gradient(
        pm,
        &relic::rect_pts([f, f, w - f, hb]),
        [0.0, f],
        pal::BLOOD_DEEP.alpha(150),
        [0.0, hb],
        pal::BLOOD_DEEP.alpha(20),
    );
    relic::serrated_blade(pm, 16.0, w - 14.0, hb - 1.0, false);
    // blood running over the top edge
    relic::blood_drips(pm, 5.0, w - 5.0, 0.0, 0x11, 16.0, 15.0);
    // skull corner ornaments
    for (x, y, s) in [
        (13.0, 15.0, 14.0),
        (w - 13.0, 15.0, 14.0),
        (12.5, ch - 7.0, 11.0),
        (w - 12.5, ch - 7.0, 11.0),
    ] {
        relic::skull(pm, [x, y], s, pal::BLOOD_NEON);
    }
    // sigil + title
    let cy = f + HEADER_H as f32 * 0.5;
    relic::sigil(pm, [38.0, cy + 1.0], 27.0, accent.base, accent.hot);
    let ty = (cy - TITLE_PX * 0.65).round() as i32;
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        fonts.draw(
            pm,
            "LILLITH",
            58 + dx,
            ty + dy,
            140.0,
            pal::BLOOD_NEON.alpha(90),
            Face::Title,
        );
    }
    fonts.draw(pm, "LILLITH", 58, ty, 140.0, pal::TEXT, Face::Title);
    // three neon runes on the right of the plate
    let rh = 10.0;
    let mut x = w - 26.0 - 3.0 * (lillith_theme::geom::rune_width(rh) + 4.0);
    for idx in [2usize, 15, 21] {
        relic::neon_rune(pm, idx, [x, cy - rh * 0.5 + 1.0], rh, pal::BLOOD_NEON);
        x += lillith_theme::geom::rune_width(rh) + 4.0;
    }
}

/// Draws row `i` (its top at `top`); `hot` = hovered (knife highlight).
fn draw_row(
    pm: &mut Pixmap,
    fonts: &mut Fonts,
    rows: &[Row],
    i: usize,
    top: f32,
    hot: bool,
    accent: &Accent,
) {
    let w = pm.width() as f32;
    let row = &rows[i];
    let rh = row.height() as f32;
    let cy = top + rh / 2.0;
    let quit = is_quit(row);
    if hot && !matches!(row, Row::Separator) {
        let r = [FRAME as f32 + 2.0, top, w - FRAME as f32 - 4.0, top + rh];
        if quit {
            relic::knife_hover(pm, r, pal::BLOOD_DEEP, pal::BLOOD_NEON, pal::BLOOD_SHEEN);
        } else {
            relic::knife_hover(pm, r, accent.deep, accent.base, accent.hot);
        }
    }
    let text = if hot { Rgba::hex(0xffffff) } else { pal::TEXT };
    match row {
        Row::Separator => {
            // The divider right above Quit is a glowing red blade.
            let fence = rows.get(i + 1).is_some_and(is_quit);
            relic::serrated_blade(pm, 16.0, w - 14.0, cy, fence);
        }
        Row::Header { label, open, .. } => {
            fonts.draw(pm, label, 24, label_y(top), w - 60.0, text, Face::Label);
            relic::arrow(pm, w - 24.0, cy, *open, accent.base, accent.hot);
        }
        Row::Leaf {
            label,
            check,
            enabled,
            indent,
            ..
        } => {
            if *indent && !hot {
                // carved guide down the open submenu
                relic::rule(
                    pm,
                    [26.5, top],
                    [26.5, top + rh],
                    accent.base.alpha(70),
                    1.0,
                );
                relic::rule(pm, [26.5, cy], [31.0, cy], accent.base.alpha(110), 1.0);
            }
            let x0 = if *indent { 36.0 } else { 20.0 };
            let col = if quit {
                if hot {
                    text
                } else {
                    Rgba::hex(0xff5466)
                }
            } else if !*enabled {
                pal::TEXT_MUTE
            } else {
                text
            };
            let tx = match check {
                Check::None => x0 + 4.0,
                Check::Checkbox(on) => {
                    relic::checkbox(pm, x0, cy, *on, accent.base, accent.hot);
                    x0 + 22.0
                }
                Check::Radio(on) => {
                    relic::radio(pm, x0, cy, *on, accent.base, accent.hot);
                    x0 + 22.0
                }
            };
            fonts.draw(
                pm,
                label,
                tx as i32,
                label_y(top),
                w - tx - 16.0,
                col,
                Face::Label,
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
    TOP + rows.iter().map(Row::height).sum::<u32>() + PAD + BOTTOM
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
        let top: u32 = TOP + p.rows()[..i].iter().map(Row::height).sum::<u32>();
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
        let top: u32 = TOP + p.rows()[..sep].iter().map(Row::height).sum::<u32>();
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

    #[test]
    fn hit_test_matches_row_geometry() {
        let mut p = MenuPopup::new(state());
        let expansions: Vec<Option<usize>> = std::iter::once(None)
            .chain((0..p.menu.items.len()).map(Some))
            .collect();
        for e in expansions {
            p.expanded = e;
            p.relayout();
            let mut top = TOP;
            for i in 0..p.rows().len() {
                assert_eq!(p.row_top(i), top);
                let h = p.rows()[i].height();
                for x in [0.5, 120.0, WIDTH as f64 - 0.5] {
                    assert_eq!(p.row_at(x, top as f64), Some(i));
                    assert_eq!(p.row_at(x, (top + h) as f64 - 0.01), Some(i));
                }
                top += h;
            }
            // The header plate and the bottom band are on the popup but
            // hit no row (clicks there do nothing, they don't dismiss).
            assert_eq!(p.row_at(100.0, (TOP - 1) as f64), None);
            assert_eq!(p.click(100.0, 3.0), MenuHit::None);
            assert_eq!(p.row_at(100.0, p.content_height() as f64 - 1.0), None);
            assert_eq!(p.content_height(), top + PAD + BOTTOM);
            assert!(p.content_height() <= p.size().1);
        }
    }

    #[test]
    fn hover_redraw_only_touches_the_hovered_row() {
        let mut fonts = Fonts::preload();
        let mut p = MenuPopup::new(state());
        let before = p.render(&mut fonts).expect("first").clone();
        let quit = p.rows().iter().position(is_quit).expect("quit row");
        assert!(p.pointer(Some((60.0, row_y(&p, quit)))));
        let after = p.render(&mut fonts).expect("hover").clone();
        assert_eq!(p.base.len(), 1, "hover reuses the static layer");
        let top = p.row_top(quit);
        let w = before.width();
        let mut changed = 0;
        for y in 0..before.height() {
            let a = &before.data()[(y * w * 4) as usize..((y + 1) * w * 4) as usize];
            let b = &after.data()[(y * w * 4) as usize..((y + 1) * w * 4) as usize];
            if a != b {
                assert!(y >= top && y < top + ROW_H, "row {y} changed");
                changed += 1;
            }
        }
        assert!(changed > 10);
        // Leaving restores the static layer exactly.
        assert!(p.pointer(None));
        assert_eq!(p.render(&mut fonts).expect("leave").data(), before.data());
        // Expanding builds (and keeps) a second layer.
        let mood = find(
            &p,
            |r| matches!(r, Row::Header { label, .. } if label == "Mood"),
        );
        p.click(20.0, row_y(&p, mood));
        p.render(&mut fonts);
        assert_eq!(p.base.len(), 2);
    }
}
