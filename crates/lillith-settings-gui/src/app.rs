//! The window: iron frame, title bar, carved tabs, body, rail, footer,
//! claw resize grips, saving, screenshots.

use crate::chrome;
use crate::store::ConfigStore;
use crate::tabs::{Shared, TabRegistry};
use crate::{ActionSink, Options};
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, Id, LayerId, Order, Pos2, Rect, ResizeDirection,
    Sense, Stroke, UiBuilder, ViewportCommand,
};
use lillith_core::menu::{pick_line, MenuPick};
use lillith_setup::detect::{self, DetectHandle, DetectOptions};
use lillith_theme::egui_theme::{self as th, c32};
use lillith_theme::geom;
use lillith_theme::palette::{self as pal, Rgba};
use std::path::PathBuf;
use std::time::Instant;

const FRAME: f32 = 16.0;
const TITLE_H: f32 = 50.0;
const TABS_H: f32 = 46.0;
const FOOTER_H: f32 = 38.0;
const RAIL_W: f32 = 200.0;

struct Shot {
    path: PathBuf,
    requested: bool,
    min_secs: f32,
}

pub struct SettingsApp {
    shared: Shared,
    tabs: TabRegistry,
    current: usize,
    detect: Option<DetectHandle>,
    sink: Option<ActionSink>,
    print_actions: bool,
    decorated: bool,
    shot: Option<Shot>,
    start: Instant,
    confirm_close: bool,
    closing: bool,
}

/// `[chat] endpoint` from config text, but only when `[llm] server_url` is
/// not set (an explicit `server_url` always wins).
fn chat_endpoint_fallback(text: &str) -> Option<String> {
    let doc: toml_edit::DocumentMut = text.parse().ok()?;
    let has_llm_url = doc
        .get("llm")
        .and_then(|t| t.get("server_url"))
        .and_then(|v| v.as_str())
        .is_some();
    if has_llm_url {
        return None;
    }
    let ep = doc.get("chat")?.get("endpoint")?.as_str()?.trim();
    (!ep.is_empty()).then(|| ep.to_string())
}

fn detect_options(path: &std::path::Path) -> DetectOptions {
    let mut o = DetectOptions::default();
    if let Ok(c) = lillith_setup::config::load(path) {
        o.llama_server_url = c.server_url;
        // No `[llm] server_url` but a `[chat] endpoint`: that is the server
        // the overlay actually talks to, so probe (and show) that one.
        if let Some(ep) = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| chat_endpoint_fallback(&t))
        {
            o.llama_server_url = ep;
        }
        o.ollama_host = c.ollama_host;
        if !c.llama_server_bin.trim().is_empty() {
            if let Some(d) = lillith_setup::paths::expand_tilde(&c.llama_server_bin).parent() {
                o.extra_bin_dirs.push(d.to_path_buf());
            }
        }
    }
    o
}

impl SettingsApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        opts: Options,
        sink: Option<ActionSink>,
        customize: Option<crate::Customize>,
    ) -> Self {
        let config_path = opts
            .config_path
            .clone()
            .unwrap_or_else(crate::default_config_path);
        // The chrome accent follows the active avatar (Lillith's by default).
        if let Some(st) = lillith_anim::avatar::AvatarStore::for_config(Some(&config_path)) {
            th::use_accent(st.selected_accent());
        }
        th::install(&cc.egui_ctx);
        let shared = Shared {
            store: ConfigStore::load(&config_path),
            config_path: config_path.clone(),
            runtime: None,
            detecting: true,
            want_redetect: false,
            manifest: opts
                .manifest
                .clone()
                .or_else(lillith_anim::atlas::default_manifest_path),
            picks_out: Vec::new(),
            toasts: Vec::new(),
            time: 0.0,
            goto_tab: None,
            want_save: false,
            accent_changed: None,
        };
        let mut tabs = TabRegistry::with_defaults(&shared);
        if let Some(f) = customize {
            f(&mut tabs);
        }
        let mut current = 0;
        if let Some(t) = &opts.start_tab {
            current = tabs.index_of(t).unwrap_or(0);
        }
        if let Some(d) = &opts.deep_link {
            tabs.tabs[current].deep_link(d);
        }
        let mut app = Self {
            detect: Some(detect::spawn(detect_options(&config_path))),
            shared,
            tabs,
            current,
            sink,
            print_actions: opts.print_actions,
            decorated: opts.decorated,
            shot: opts.screenshot.map(|path| Shot {
                path,
                requested: false,
                min_secs: 2.5,
            }),
            start: Instant::now(),
            confirm_close: false,
            closing: false,
        };
        if let Some(e) = app.shared.store.load_error().map(str::to_string) {
            app.shared
                .toast(format!("config.toml unreadable: {e}"), pal::WARN);
        }
        app
    }

    fn pending(&self) -> usize {
        self.shared.store.pending() + self.tabs.tabs.iter().map(|t| t.pending()).sum::<usize>()
    }

    fn save_all(&mut self) {
        let mut errs = Vec::new();
        // Whether anything actually reached config.toml.
        let mut wrote = false;
        let staged = self.shared.store.pending() > 0;
        match self.shared.store.save() {
            Ok(()) => wrote |= staged,
            Err(e) => errs.push(e),
        }
        for t in &mut self.tabs.tabs {
            if t.pending() > 0 {
                match t.save(&mut self.shared) {
                    Ok(()) => wrote = true,
                    Err(e) => errs.push(format!("{}: {e}", t.title())),
                }
            }
        }
        self.shared.store.reload();
        if wrote {
            // The overlay reads config.toml at start; this tells it to
            // re-read it now.
            // Queued after any picks a tab's save() pushed (e.g. `avatar`).
            self.shared.picks_out.push(MenuPick::ReloadConfig);
        }
        if errs.is_empty() {
            self.shared
                .toast("sealed ✔ written to config.toml", pal::OK);
        } else {
            self.shared
                .toast(format!("⚠ {}", errs.join(" · ")), pal::BLOOD_SHEEN);
        }
    }

    fn revert_all(&mut self) {
        self.shared.store.revert();
        for t in &mut self.tabs.tabs {
            t.revert(&mut self.shared);
        }
        self.shared
            .toast("reverted to what's on disk", pal::TEXT_DIM);
    }

    fn emit(&mut self, p: &MenuPick) {
        if let Some(s) = &mut self.sink {
            s(p);
        }
        self.emit_line(&pick_line(p));
    }

    /// Prints one wire line on stdout (unless `--quiet`).
    fn emit_line(&mut self, line: &str) {
        if self.print_actions {
            // Not println!: if the overlay that launched us has quit, the
            // pipe is closed and println! would panic the window.
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
                self.print_actions = false;
            }
        }
    }

    // ------------------------------------------------------------------
    // chrome
    // ------------------------------------------------------------------

    fn title_bar(&mut self, ui: &mut egui::Ui, r: Rect) {
        let p = ui.painter().clone();
        th::gradient_rect(&p, r, c32(Rgba::hex(0x1d1519)), c32(Rgba::hex(0x0a0608)));
        p.line_segment(
            [r.left_bottom(), r.right_bottom()],
            Stroke::new(2.0, c32(Rgba::hex(0x000000))),
        );
        p.line_segment(
            [
                r.left_bottom() + vec2(0.0, 1.5),
                r.right_bottom() + vec2(0.0, 1.5),
            ],
            Stroke::new(1.0, c32(pal::IRON_HI)),
        );
        // heavy blood running down from the top of the frame
        th::blood_drips(
            &p,
            r.left() + 46.0,
            r.right() - 46.0,
            r.top(),
            61,
            28.0,
            21.0,
        );
        // drag to move, double click to maximize
        let drag = ui.interact(
            r.shrink2(vec2(60.0, 0.0)),
            Id::new("title-drag"),
            Sense::click_and_drag(),
        );
        if !self.decorated {
            if drag.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if drag.double_clicked() {
                let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!max));
            }
        }
        let narrow = r.width() < 760.0;
        let pulse = ((self.shared.time * 1.6).sin() * 0.5 + 0.5) as f32;
        // left: sigil + name
        let sx = r.left() + 66.0;
        th::sigil(&p, pos2(sx, r.center().y + 1.0), 36.0, pulse, false);
        let name = p.text(
            pos2(sx + 24.0, r.center().y - 1.0),
            Align2::LEFT_CENTER,
            "LILLITH",
            th::carved_font(22.0),
            c32(pal::TEXT),
        );
        if !narrow {
            p.text(
                pos2(name.right() + 10.0, r.center().y + 1.0),
                Align2::LEFT_CENTER,
                "· GRIMOIRE OF SETTINGS",
                th::display_font(10.0),
                c32(pal::TEXT_MUTE),
            );
        }
        // right: close + llama chip
        let close = Rect::from_center_size(pos2(r.right() - 66.0, r.center().y), vec2(26.0, 26.0));
        let cr = ui
            .interact(close, Id::new("close"), Sense::click())
            .on_hover_text("Close");
        let hot = ui.ctx().animate_bool(cr.id, cr.hovered());
        let poly = geom::chamfer_rect(
            [close.left(), close.top(), close.right(), close.bottom()],
            5.0,
        );
        th::fill_convex(&p, &poly, c32(pal::IRON_MID.lerp(pal::BLOOD, hot)));
        th::neon_line(
            &p,
            &poly,
            pal::IRON_EDGE.lerp(pal::BLOOD_NEON, hot),
            1.0,
            2.0 * hot,
            true,
        );
        let k = 6.0;
        for (a, b) in [(vec2(-k, -k), vec2(k, k)), (vec2(k, -k), vec2(-k, k))] {
            p.line_segment(
                [close.center() + a, close.center() + b],
                Stroke::new(2.0, c32(pal::TEXT.lerp(Rgba::hex(0xffffff), hot))),
            );
        }
        if cr.clicked() {
            self.request_close(ui.ctx());
        }
        // llama chip
        let (text, color) = match (&self.shared.runtime, self.shared.detecting) {
            (_, true) => ("llama-server · probing…".to_string(), pal::WARN),
            (Some(rt), _) if rt.llama_server_api.reachable => {
                let host = rt
                    .llama_server_api
                    .url
                    .trim_start_matches("http://")
                    .trim_end_matches('/')
                    .to_string();
                (format!("llama-server · online {host}"), pal::OK)
            }
            (Some(rt), _) => {
                let host = rt
                    .llama_server_api
                    .url
                    .trim_start_matches("http://")
                    .trim_end_matches('/')
                    .to_string();
                (format!("llama-server · offline {host}"), pal::BLOOD_NEON)
            }
            (None, _) => ("llama-server · unknown".to_string(), pal::TEXT_MUTE),
        };
        let text = if narrow {
            text.split(' ').take(3).collect::<Vec<_>>().join(" ")
        } else {
            text
        };
        let g = p.layout_no_wrap(text, th::display_font(10.0), c32(color));
        let w = g.size().x + 32.0;
        let chip = Rect::from_min_size(
            pos2(close.left() - 12.0 - w, r.center().y - 11.0),
            vec2(w, 22.0),
        );
        chrome::paint_chip(&p, chip, g, color, true);
        // center plaque
        let plaque_w = if narrow { 0.0 } else { 270.0 };
        let left_end = if narrow {
            name.right()
        } else {
            name.right() + 180.0
        };
        if plaque_w > 0.0 && chip.left() - left_end > plaque_w + 20.0 {
            let cx = (left_end + chip.left()) * 0.5;
            let pr = Rect::from_center_size(pos2(cx, r.center().y), vec2(plaque_w, 30.0));
            let poly = geom::chamfer_rect([pr.left(), pr.top(), pr.right(), pr.bottom()], 9.0);
            th::fill_convex(&p, &poly, c32(Rgba::hex(0x14090d)));
            th::neon_line(&p, &poly, pal::BLOOD_NEON.alpha(200), 1.0, 3.0, true);
            for (side, d) in [(pr.left() - 8.0, 1.0f32), (pr.right() + 8.0, -1.0)] {
                let tri = [
                    [side, pr.center().y],
                    [side + d * 7.0, pr.center().y - 5.0],
                    [side + d * 7.0, pr.center().y + 5.0],
                ];
                th::fill_convex(&p, &tri, c32(pal::BLOOD_NEON));
            }
            let cursor = if (self.shared.time * 2.0) as i64 % 2 == 0 {
                "_"
            } else {
                " "
            };
            th::glitch_text(
                &p,
                pr.center(),
                Align2::CENTER_CENTER,
                &format!("LillithOS · SYSTEM_ACTIVE{cursor}"),
                th::display_font(11.5),
                pal::BLOOD_SHEEN,
            );
        }
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui, r: Rect) {
        let p = ui.painter().clone();
        th::gradient_rect(&p, r, c32(Rgba::hex(0x0d0709)), c32(Rgba::hex(0x060304)));
        let pad = 10.0;
        let gap = 4.0;
        let n = self.tabs.tabs.len();
        let font = th::carved_font(13.5);
        let widths: Vec<f32> = self
            .tabs
            .tabs
            .iter()
            .map(|t| {
                let label = if t.sealed() {
                    roman_of(t.title())
                } else {
                    t.title().to_string()
                };
                p.layout_no_wrap(label, font.clone(), Color32::WHITE)
                    .size()
                    .x
                    + if t.sealed() { 40.0 } else { 46.0 }
            })
            .collect();
        let avail = r.width() - pad * 2.0 - gap * (n as f32 - 1.0);
        let total: f32 = widths.iter().sum();
        // compact: active tab keeps its label, others show their rune only
        let compact = total > avail;
        let rune_only_w = 34.0;
        let mut ws = widths.clone();
        if compact {
            let cur_w = widths[self.current];
            let others = (avail - cur_w) / (n as f32 - 1.0);
            for (i, w) in ws.iter_mut().enumerate() {
                if i != self.current {
                    *w = others.clamp(rune_only_w, widths[i]);
                }
            }
        } else {
            // stretch a little so the row reads as one carved band
            let extra = ((avail - total) / n as f32).min(14.0);
            for w in &mut ws {
                *w += extra;
            }
        }
        let mut x = r.left() + pad;
        let top = r.top() + 6.0;
        let bot = r.bottom();
        let mut clicked = None;
        for (i, t) in self.tabs.tabs.iter().enumerate() {
            let w = ws[i];
            let tr = Rect::from_min_max(pos2(x, top), pos2(x + w, bot));
            let resp = ui
                .interact(tr, Id::new(("tab", i)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            let resp = if t.sealed() {
                resp.on_hover_text("Sealed add-in slot — reserved for future rites")
            } else {
                resp.on_hover_text(t.title())
            };
            let active = i == self.current;
            let hover = ui.ctx().animate_bool(resp.id, resp.hovered());
            let outline =
                geom::tab_outline([tr.left(), tr.top(), tr.right(), tr.bottom() + 1.0], 9.0);
            // stone body
            let (c_top, c_bot) = if active {
                (Rgba::hex(0x3a1420), Rgba::hex(0x14060a))
            } else if t.sealed() {
                (Rgba::hex(0x17121a), Rgba::hex(0x0b080c))
            } else {
                (pal::STONE_HI.lerp(Rgba::hex(0x4a3a42), hover), pal::STONE)
            };
            let mut m = egui::epaint::Mesh::default();
            for v in &outline {
                let t = (v[1] - tr.top()) / tr.height();
                m.colored_vertex(pos2(v[0], v[1]), c32(c_top.lerp(c_bot, t)));
            }
            for k in 1..outline.len() as u32 - 1 {
                m.add_triangle(0, k, k + 1);
            }
            p.add(egui::Shape::mesh(m));
            if !active {
                // carved cracks for texture
                let seed = i as u32 * 31 + 5;
                th::ember_cracks(
                    &p,
                    &geom::cracks(
                        seed,
                        [
                            tr.left() + 2.0,
                            tr.top() + 4.0,
                            tr.right() - 2.0,
                            tr.bottom() - 2.0,
                        ],
                        1,
                    ),
                );
            }
            let edge: Vec<Pos2> = outline.iter().map(|q| pos2(q[0], q[1])).collect();
            if active {
                th::knife_highlight(
                    &p,
                    Rect::from_min_max(
                        pos2(tr.left() + 4.0, tr.bottom() - 9.0),
                        pos2(tr.right() - 2.0, tr.bottom() - 1.0),
                    ),
                    1.0,
                );
                th::neon_line(
                    &p,
                    &outline[..outline.len()],
                    th::accent().base,
                    1.3,
                    4.0,
                    false,
                );
                th::blood_drips(
                    &p,
                    tr.left() + 9.0,
                    tr.right() - 9.0,
                    tr.top() + 1.0,
                    40 + i as u32,
                    13.0,
                    16.0,
                );
            } else {
                p.add(egui::Shape::line(
                    edge.clone(),
                    Stroke::new(1.0, c32(pal::IRON_EDGE.lerp(pal::BLOOD_NEON, hover * 0.8))),
                ));
                p.line_segment(
                    [
                        pos2(tr.left() + 9.0, tr.top() + 1.5),
                        pos2(tr.right() - 9.0, tr.top() + 1.5),
                    ],
                    Stroke::new(1.0, c32(Rgba::hex(0x8a7a82).alpha(70))),
                );
            }
            // rune + label
            let show_label = !compact || active || w >= widths[i] - 0.5;
            let rh = 14.0;
            let label = if t.sealed() {
                roman_of(t.title())
            } else {
                t.title().to_string()
            };
            let lg = p.layout_no_wrap(label, font.clone(), Color32::WHITE);
            let rune_gap = if t.sealed() { 15.0 } else { 8.0 };
            let content_w = geom::rune_width(rh)
                + if show_label {
                    rune_gap + lg.size().x
                } else {
                    0.0
                };
            let cx0 = tr.center().x - content_w * 0.5;
            let ry = tr.center().y + 2.0 - rh * 0.5;
            if t.sealed() {
                // a wax seal behind the rune
                let sc = pos2(cx0 + geom::rune_width(rh) * 0.5, tr.center().y + 2.0);
                p.circle_filled(
                    sc,
                    10.0,
                    c32(if active {
                        pal::VIOLET_DEEP
                    } else {
                        Rgba::hex(0x2a0f3a)
                    }),
                );
                p.circle_stroke(
                    sc,
                    10.0,
                    Stroke::new(1.0, c32(pal::VIOLET.alpha(if active { 255 } else { 120 }))),
                );
                th::neon_rune(
                    &p,
                    t.rune(),
                    pos2(cx0, ry),
                    rh,
                    pal::VIOLET.lerp(th::accent().base, hover * 0.5),
                );
            } else if active {
                th::neon_rune(&p, t.rune(), pos2(cx0, ry), rh, pal::NEON_BLUE_CORE);
            } else {
                th::carved_rune(
                    &p,
                    t.rune(),
                    pos2(cx0, ry),
                    rh,
                    Some(pal::BLOOD.lerp(pal::BLOOD_NEON, hover)),
                );
            }
            if show_label {
                let tc = if active {
                    Rgba::hex(0xffffff)
                } else if t.sealed() {
                    pal::TEXT_MUTE.lerp(pal::VIOLET, 0.3 + hover * 0.4)
                } else {
                    pal::TEXT_DIM.lerp(pal::TEXT, hover)
                };
                let lp = pos2(
                    cx0 + geom::rune_width(rh) + rune_gap,
                    tr.center().y + 2.0 - lg.size().y * 0.5,
                );
                if active {
                    p.galley_with_override_text_color(
                        lp + vec2(1.0, 1.0),
                        lg.clone(),
                        c32(Rgba::hex(0x000000)),
                    );
                }
                p.galley_with_override_text_color(lp, lg, c32(tc));
            }
            if resp.clicked() {
                clicked = Some(i);
            }
            x += w + gap;
        }
        p.line_segment(
            [pos2(r.left(), r.bottom()), pos2(r.right(), r.bottom())],
            Stroke::new(1.0, c32(th::accent().base.alpha(140))),
        );
        if let Some(i) = clicked {
            self.current = i;
        }
    }

    fn rail(&mut self, ui: &mut egui::Ui, r: Rect) {
        let p = ui.painter().clone();
        let t = self.shared.time as f32;
        let h1 = (r.height() * 0.42).max(150.0);
        let a = Rect::from_min_size(r.min, vec2(r.width(), h1));
        let inner = th::terminal_panel(&p, a, "DATA_FEED", None);
        let tab = &self.tabs.tabs[self.current];
        let models = self
            .shared
            .runtime
            .as_ref()
            .map(|r| r.ollama_api.models.len())
            .unwrap_or(0);
        let lines = [
            ("SOUL_SYNTAX", tab.id().to_uppercase()),
            ("CORRUPTION_LEVEL", format!("{}", self.pending())),
            (
                "RUNE_KEY",
                format!(
                    "F{} · {}",
                    self.current + 1,
                    geom::RUNES[tab.rune() % geom::RUNES.len()]
                        .name
                        .to_uppercase()
                ),
            ),
            (
                "SERVER",
                if self
                    .shared
                    .runtime
                    .as_ref()
                    .is_some_and(|r| r.llama_server_api.reachable)
                {
                    "ONLINE".into()
                } else {
                    "OFFLINE".into()
                },
            ),
            ("OLLAMA_SOULS", models.to_string()),
        ];
        let mut y = inner.top();
        for (k, v) in lines {
            th::glitch_text(
                &p,
                pos2(inner.left(), y),
                Align2::LEFT_TOP,
                k,
                th::display_font(9.0),
                pal::BLOOD_SHEEN,
            );
            p.text(
                pos2(inner.right(), y + 11.0),
                Align2::RIGHT_TOP,
                v,
                egui::FontId::monospace(11.0),
                c32(pal::TERMINAL_TEXT),
            );
            y += 26.0;
            if y > inner.bottom() - 12.0 {
                break;
            }
        }
        // animated flux graph (decorative)
        let b = Rect::from_min_max(
            pos2(r.left(), a.bottom() + 8.0),
            pos2(r.right(), (a.bottom() + 8.0 + 120.0).min(r.bottom())),
        );
        if b.height() > 70.0 {
            let gi = th::terminal_panel(&p, b, "AETHER_FLUX", Some("hz"));
            let n = 24;
            let bw = gi.width() / n as f32;
            for i in 0..n {
                let v = ((i as f32 * 0.7 + t * 1.3).sin() * 0.5 + 0.5) * 0.6
                    + ((i as f32 * 1.9 - t * 0.7).sin() * 0.5 + 0.5) * 0.4;
                let h = gi.height() * v.clamp(0.08, 1.0);
                let br = Rect::from_min_max(
                    pos2(gi.left() + i as f32 * bw + 1.0, gi.bottom() - h),
                    pos2(gi.left() + (i + 1) as f32 * bw - 1.0, gi.bottom()),
                );
                th::gradient_rect(
                    &p,
                    br,
                    c32(pal::BLOOD_NEON.alpha(200)),
                    c32(pal::BLOOD_DEEP.alpha(120)),
                );
            }
        }
        // rune column + skull at the bottom
        let c = Rect::from_min_max(pos2(r.left(), b.bottom() + 8.0), r.max);
        if c.height() > 90.0 {
            let ci = th::terminal_panel(&p, c, "RUNE_KEY", Some("seal"));
            let rh = 13.0;
            let cols = ((ci.width()) / (geom::rune_width(rh) + 9.0)).floor() as usize;
            let rows = ((ci.height() - 50.0) / (rh + 9.0)).floor().max(0.0) as usize;
            for row in 0..rows {
                for col in 0..cols {
                    let idx = row * 7 + col * 3 + self.current;
                    let lit = ((t * 2.0) as usize + row * cols + col) % 11 == 0;
                    let o = pos2(
                        ci.left() + col as f32 * (geom::rune_width(rh) + 9.0),
                        ci.top() + row as f32 * (rh + 9.0),
                    );
                    if lit {
                        th::neon_rune(&p, idx, o, rh, pal::VIOLET);
                    } else {
                        th::neon_rune(&p, idx, o, rh, pal::BLOOD.alpha(150));
                    }
                }
            }
            if ci.height() > 70.0 {
                th::skull(
                    &p,
                    pos2(ci.center().x, ci.bottom() - 20.0),
                    28.0,
                    pal::VIOLET,
                );
            }
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui, r: Rect) {
        let p = ui.painter().clone();
        th::gradient_rect(&p, r, c32(Rgba::hex(0x0b0608)), c32(Rgba::hex(0x170e12)));
        p.line_segment(
            [r.left_top(), r.right_top()],
            Stroke::new(1.0, c32(pal::IRON_HI)),
        );
        let pending = self.pending();
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(r.shrink2(vec2(14.0, 2.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut child;
        let g = ui.painter().layout_no_wrap(
            "LillithOS · SYSTEM_ACTIVE".into(),
            th::display_font(10.0),
            c32(pal::OK),
        );
        let (cr, _) = ui.allocate_exact_size(vec2(g.size().x + 32.0, 22.0), Sense::hover());
        chrome::paint_chip(ui.painter(), cr, g, pal::OK, true);
        let wide = r.width() > 760.0;
        if wide {
            ui.label(
                egui::RichText::new(self.shared.config_path.display().to_string())
                    .monospace()
                    .size(11.0)
                    .color(c32(pal::TEXT_MUTE)),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if chrome::blade_button(ui, "SAVE", pending > 0)
                .on_hover_text("Ctrl+S")
                .clicked()
            {
                self.save_all();
            }
            if pending > 0 && chrome::stone_button(ui, "Revert").clicked() {
                self.revert_all();
            }
            let (t, c) = if pending > 0 {
                (format!("{pending} pending"), pal::WARN)
            } else {
                ("saved ✔".to_string(), pal::OK)
            };
            ui.label(
                egui::RichText::new(t)
                    .font(th::display_font(10.0))
                    .color(c32(c)),
            );
        });
    }

    fn resize_grips(&mut self, ctx: &egui::Context, full: Rect) {
        let fg = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("claws")));
        let size = 30.0;
        let grips = [
            (
                full.right_bottom(),
                [-1.0, -1.0],
                ResizeDirection::SouthEast,
            ),
            (full.left_bottom(), [1.0, -1.0], ResizeDirection::SouthWest),
        ];
        egui::Area::new(Id::new("grip-area"))
            .order(Order::Foreground)
            .fixed_pos(full.min)
            .show(ctx, |ui| {
                for (i, (corner, dir, rd)) in grips.iter().enumerate() {
                    let r =
                        Rect::from_two_pos(*corner, *corner + vec2(dir[0] * size, dir[1] * size));
                    let resp = ui.interact(r, Id::new(("claw", i)), Sense::drag());
                    let hot = resp.hovered() || resp.dragged();
                    let resp = resp.on_hover_cursor(if i == 0 {
                        egui::CursorIcon::ResizeSouthEast
                    } else {
                        egui::CursorIcon::ResizeSouthWest
                    });
                    if resp.drag_started() && !self.decorated {
                        ctx.send_viewport_cmd(ViewportCommand::BeginResize(*rd));
                    }
                    th::claw_grip(
                        &fg,
                        *corner + vec2(dir[0] * 3.0, dir[1] * 3.0),
                        size,
                        *dir,
                        hot,
                    );
                }
                if !self.decorated {
                    let e = 5.0;
                    let edges = [
                        (
                            Rect::from_min_max(
                                pos2(full.left() + size, full.bottom() - e),
                                pos2(full.right() - size, full.bottom()),
                            ),
                            ResizeDirection::South,
                            egui::CursorIcon::ResizeVertical,
                        ),
                        (
                            Rect::from_min_max(
                                pos2(full.left() + size, full.top()),
                                pos2(full.right() - size, full.top() + e),
                            ),
                            ResizeDirection::North,
                            egui::CursorIcon::ResizeVertical,
                        ),
                        (
                            Rect::from_min_max(
                                pos2(full.left(), full.top() + size),
                                pos2(full.left() + e, full.bottom() - size),
                            ),
                            ResizeDirection::West,
                            egui::CursorIcon::ResizeHorizontal,
                        ),
                        (
                            Rect::from_min_max(
                                pos2(full.right() - e, full.top() + size),
                                pos2(full.right(), full.bottom() - size),
                            ),
                            ResizeDirection::East,
                            egui::CursorIcon::ResizeHorizontal,
                        ),
                    ];
                    for (i, (r, d, cur)) in edges.into_iter().enumerate() {
                        let resp = ui
                            .interact(r, Id::new(("edge", i)), Sense::drag())
                            .on_hover_cursor(cur);
                        if resp.drag_started() {
                            ctx.send_viewport_cmd(ViewportCommand::BeginResize(d));
                        }
                    }
                }
            });
    }

    fn request_close(&mut self, ctx: &egui::Context) {
        if self.pending() > 0 {
            self.confirm_close = true;
        } else {
            self.closing = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_close {
            return;
        }
        let screen = ctx.screen_rect();
        let bg = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("modal-bg")));
        bg.rect_filled(
            screen,
            CornerRadius::ZERO,
            c32(Rgba::hex(0x000000).alpha(170)),
        );
        egui::Area::new(Id::new("confirm"))
            .order(Order::Tooltip)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let (r, _) = ui.allocate_exact_size(vec2(380.0, 150.0), Sense::hover());
                let inner = th::terminal_panel(ui.painter(), r, "UNSAVED_RITES", None);
                let mut c = ui.new_child(UiBuilder::new().max_rect(inner));
                c.label(
                    egui::RichText::new(format!("{} change(s) aren't sealed yet.", self.pending()))
                        .color(c32(pal::TEXT)),
                );
                c.add_space(10.0);
                c.horizontal(|ui| {
                    if chrome::blade_button(ui, "SAVE & CLOSE", true).clicked() {
                        self.save_all();
                        self.closing = true;
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    if chrome::stone_button(ui, "Discard").clicked() {
                        self.closing = true;
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    if chrome::stone_button(ui, "Cancel").clicked() {
                        self.confirm_close = false;
                    }
                });
            });
    }

    fn toasts(&mut self, ctx: &egui::Context, body: Rect) {
        let now = self.shared.time;
        self.shared.toasts.retain(|t| t.2 > now);
        let p = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("toasts")));
        let mut y = body.bottom() - 12.0;
        for (text, color, until) in self.shared.toasts.iter().rev().take(3) {
            let a = ((until - now) as f32).clamp(0.0, 1.0);
            let g = p.layout_no_wrap(text.clone(), th::bold_font(13.0), c32(pal::TEXT.fade(a)));
            let r = Rect::from_center_size(
                pos2(body.center().x, y - 14.0),
                vec2(g.size().x + 30.0, 28.0),
            );
            let poly = geom::chamfer_rect([r.left(), r.top(), r.right(), r.bottom()], 6.0);
            th::fill_convex(&p, &poly, c32(Rgba::hex(0x14060a).fade(a * 0.95)));
            th::neon_line(&p, &poly, color.fade(a), 1.0, 3.0, true);
            p.galley(
                pos2(r.left() + 15.0, r.center().y - g.size().y * 0.5),
                g,
                c32(pal::TEXT.fade(a)),
            );
            y -= 34.0;
        }
    }

    fn screenshot(&mut self, ctx: &egui::Context) {
        let busy = self.shared.detecting || self.tabs.tabs.iter().any(|t| t.busy());
        let elapsed = self.start.elapsed().as_secs_f32();
        let Some(shot) = &mut self.shot else { return };
        if !shot.requested && elapsed > shot.min_secs && (!busy || elapsed > 20.0) {
            shot.requested = true;
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let mut done = None;
        ctx.input(|i| {
            for e in &i.raw.events {
                if let egui::Event::Screenshot { image, .. } = e {
                    done = Some(image.clone());
                }
            }
        });
        if let Some(img) = done {
            let [w, h] = img.size;
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            match image::save_buffer(
                &shot.path,
                &bytes,
                w as u32,
                h as u32,
                image::ExtendedColorType::Rgba8,
            ) {
                Ok(()) => eprintln!(
                    "lillith-settings: screenshot {}x{} → {}",
                    w,
                    h,
                    shot.path.display()
                ),
                Err(e) => eprintln!("lillith-settings: screenshot failed: {e}"),
            }
            self.closing = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}

fn roman_of(title: &str) -> String {
    title.rsplit(' ').next().unwrap_or(title).to_string()
}

impl eframe::App for SettingsApp {
    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        [0.02, 0.0, 0.01, 1.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.shared.time = self.start.elapsed().as_secs_f64();
        // background jobs
        if let Some(h) = &mut self.detect {
            if let Some(r) = h.try_recv() {
                self.shared.runtime = Some(r);
                self.shared.detecting = false;
                self.detect = None;
            }
        }
        if self.shared.want_redetect {
            self.shared.want_redetect = false;
            if self.detect.is_none() {
                self.detect = Some(detect::spawn(detect_options(&self.shared.config_path)));
                self.shared.detecting = true;
            }
        }
        for t in &mut self.tabs.tabs {
            t.tick(ctx, &mut self.shared);
        }
        // keys
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
            self.save_all();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Tab)) {
            self.current = (self.current + 1) % self.tabs.tabs.len();
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.closing && self.pending() > 0 {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.confirm_close = true;
        }

        let mut body_rect = Rect::NOTHING;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(c32(pal::VOID)))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                let p = ui.painter().clone();
                th::stone_fill(&p, full, 3, 18);
                let inner = th::iron_frame(&p, full, FRAME, 5);
                let title = Rect::from_min_size(inner.min, vec2(inner.width(), TITLE_H));
                let tabs_r = Rect::from_min_size(
                    pos2(inner.left(), title.bottom()),
                    vec2(inner.width(), TABS_H),
                );
                let footer =
                    Rect::from_min_max(pos2(inner.left(), inner.bottom() - FOOTER_H), inner.max);
                let body = Rect::from_min_max(
                    pos2(inner.left(), tabs_r.bottom()),
                    pos2(inner.right(), footer.top()),
                );
                body_rect = body;
                // body: obsidian with a faint red grid
                th::gradient_rect(&p, body, c32(Rgba::hex(0x0c0508)), c32(Rgba::hex(0x070305)));
                let mut gx = body.left() + 24.0;
                while gx < body.right() {
                    p.line_segment(
                        [pos2(gx, body.top()), pos2(gx, body.bottom())],
                        Stroke::new(1.0, c32(pal::TERMINAL_GRID.alpha(3))),
                    );
                    gx += 24.0;
                }
                let mut gy = body.top() + 24.0;
                while gy < body.bottom() {
                    p.line_segment(
                        [pos2(body.left(), gy), pos2(body.right(), gy)],
                        Stroke::new(1.0, c32(pal::TERMINAL_GRID.alpha(3))),
                    );
                    gy += 24.0;
                }
                // ember-lit cracks creeping in from the stone at the edges
                let cracks: Vec<Vec<[f32; 2]>> = geom::cracks(
                    77,
                    [body.left(), body.top(), body.right(), body.bottom()],
                    14,
                )
                .into_iter()
                .map(|l| l.into_iter().take(6).collect())
                .collect();
                th::ember_cracks(&p, &cracks);
                th::vignette(&p, body, 60.0);
                self.title_bar(ui, title);
                // drips from the title bar, behind the tabs
                th::blood_drips(
                    &p,
                    tabs_r.left(),
                    tabs_r.right(),
                    tabs_r.top(),
                    17,
                    34.0,
                    30.0,
                );
                self.tab_bar(ui, tabs_r);
                // drips under the tab band into the body margin
                th::blood_drips(
                    &p,
                    body.left() + 4.0,
                    body.right() - 4.0,
                    body.top() + 1.0,
                    23,
                    16.0,
                    46.0,
                );
                self.footer(ui, footer);

                let show_rail = body.width() >= 940.0;
                let content = if show_rail {
                    let rail = Rect::from_min_max(
                        pos2(body.right() - RAIL_W - 12.0, body.top() + 14.0),
                        pos2(body.right() - 12.0, body.bottom() - 10.0),
                    );
                    self.rail(ui, rail);
                    Rect::from_min_max(
                        pos2(body.left() + 14.0, body.top() + 14.0),
                        pos2(rail.left() - 10.0, body.bottom() - 6.0),
                    )
                } else {
                    Rect::from_min_max(
                        pos2(body.left() + 12.0, body.top() + 12.0),
                        pos2(body.right() - 12.0, body.bottom() - 6.0),
                    )
                };
                let mut child = ui.new_child(
                    UiBuilder::new()
                        .max_rect(content)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                child.set_clip_rect(content.intersect(ui.clip_rect()));
                let cur = self.current;
                egui::ScrollArea::vertical()
                    .id_salt(("page", cur))
                    .auto_shrink([false, false])
                    .show(&mut child, |ui| {
                        ui.set_width(ui.available_width() - 10.0);
                        self.tabs.tabs[cur].ui(ui, &mut self.shared);
                        ui.add_space(10.0);
                    });

                // heavy blood along the very top of the frame (foreground)
                let fg = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("top-blood")));
                th::blood_drips(
                    &fg,
                    full.left() + 46.0,
                    full.right() - 46.0,
                    full.top(),
                    9,
                    11.0,
                    26.0,
                );
                // horned skulls on the top corners
                th::skull(
                    &fg,
                    pos2(full.left() + 30.0, full.top() + 34.0),
                    36.0,
                    pal::EMBER,
                );
                th::skull(
                    &fg,
                    pos2(full.right() - 30.0, full.top() + 34.0),
                    36.0,
                    pal::EMBER,
                );
                self.resize_grips(ctx, full);
            });
        self.toasts(ctx, body_rect);
        self.confirm_dialog(ctx);

        if let Some(a) = self.shared.accent_changed.take() {
            th::set_accent(ctx, a);
        }
        if let Some(t) = self.shared.goto_tab.take() {
            if let Some(i) = self.tabs.index_of(&t) {
                self.current = i;
            }
        }
        if std::mem::take(&mut self.shared.want_save) {
            self.save_all();
        }
        let picks = std::mem::take(&mut self.shared.picks_out);
        for p in &picks {
            self.emit(p);
        }
        self.screenshot(ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(80));
    }
}

#[cfg(test)]
mod detect_tests {
    use super::chat_endpoint_fallback;

    #[test]
    fn chat_endpoint_is_used_only_without_llm_server_url() {
        let chat = "[chat]\nendpoint = \"http://127.0.0.1:8089\"\n";
        assert_eq!(
            chat_endpoint_fallback(chat).as_deref(),
            Some("http://127.0.0.1:8089")
        );
        let both = format!("{chat}[llm]\nserver_url = \"http://127.0.0.1:8080\"\n");
        assert_eq!(chat_endpoint_fallback(&both), None);
        assert_eq!(chat_endpoint_fallback("[voice]\nenabled = true\n"), None);
    }
}
