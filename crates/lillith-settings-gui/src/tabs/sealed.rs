//! Empty add-in slots ("Sealed I/II/III"): a grimoire page with a rune seal.

use super::{SettingsTab, Shared};
use crate::chrome;
use egui::{pos2, vec2, Align2, Sense};
use lillith_theme::egui_theme::{self as th, c32};
use lillith_theme::geom;
use lillith_theme::palette::{self as pal, Rgba};

pub struct SealedTab {
    n: usize,
    id: String,
    title: String,
}

impl SealedTab {
    pub fn new(n: usize) -> Self {
        let roman = ["I", "II", "III", "IV", "V", "VI"][(n - 1).min(5)];
        Self {
            n,
            id: format!("sealed-{n}"),
            title: format!("Sealed {roman}"),
        }
    }
}

impl SettingsTab for SealedTab {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn rune(&self) -> usize {
        [20, 18, 21][(self.n - 1) % 3]
    }
    fn sealed(&self) -> bool {
        true
    }
    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let h = ui.available_height().max(300.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        let p = ui.painter();
        // grimoire page: dark vellum inside an iron border
        let page = r.shrink2(vec2((r.width() * 0.08).max(12.0), 10.0));
        th::gradient_rect(p, page, c32(Rgba::hex(0x1a0d10)), c32(Rgba::hex(0x0d0507)));
        th::vignette(p, page, 40.0);
        p.rect_stroke(
            page,
            egui::CornerRadius::ZERO,
            egui::Stroke::new(1.0, c32(pal::IRON_HI)),
            egui::StrokeKind::Inside,
        );
        p.rect_stroke(
            page.shrink(6.0),
            egui::CornerRadius::ZERO,
            egui::Stroke::new(1.0, c32(pal::BLOOD_DRIED)),
            egui::StrokeKind::Inside,
        );
        // seal: double ring of runes around a wax disc
        let sz = (page.height() * 0.62).min(page.width() * 0.5).max(120.0);
        let c = pos2(page.center().x, page.top() + 26.0 + sz * 0.5);
        let pulse = (shared.time * 1.3).sin() as f32 * 0.5 + 0.5;
        th::glow_blob(p, c, sz * 0.62, pal::VIOLET, 0.5 + 0.5 * pulse);
        p.circle_stroke(
            c,
            sz * 0.5,
            egui::Stroke::new(2.0, c32(pal::VIOLET.alpha(200))),
        );
        p.circle_stroke(
            c,
            sz * 0.5,
            egui::Stroke::new(7.0, th::additive(pal::VIOLET, 0.12)),
        );
        p.circle_stroke(c, sz * 0.36, egui::Stroke::new(1.0, c32(pal::VIOLET_DEEP)));
        let n = 16;
        for i in 0..n {
            let a = i as f32 / n as f32 * std::f32::consts::TAU + shared.time as f32 * 0.05;
            let rad = sz * 0.43;
            let rh = sz * 0.075;
            let o = pos2(
                c.x + a.cos() * rad - geom::rune_width(rh) * 0.5,
                c.y + a.sin() * rad - rh * 0.5,
            );
            th::neon_rune(
                p,
                i * 3 + self.n,
                o,
                rh,
                pal::VIOLET.lerp(th::accent().base, 0.25),
            );
        }
        // wax seal
        p.circle_filled(
            c + vec2(2.0, 3.0),
            sz * 0.27,
            c32(Rgba::hex(0x000000).alpha(160)),
        );
        p.circle_filled(c, sz * 0.27, c32(pal::BLOOD_DEEP));
        p.circle_filled(c + vec2(-sz * 0.03, -sz * 0.03), sz * 0.23, c32(pal::BLOOD));
        th::sigil(p, c + vec2(0.0, sz * 0.02), sz * 0.36, pulse * 0.4, false);
        // skulls guard the page corners, runes carved along the margins
        for x in [page.left() + 26.0, page.right() - 26.0] {
            th::skull(p, pos2(x, page.top() + 30.0), 26.0, pal::VIOLET);
            let mut y = page.top() + 64.0;
            let mut k = self.n * 5;
            while y < page.bottom() - 30.0 {
                th::carved_rune(p, k, pos2(x - 4.0, y), 13.0, Some(pal::BLOOD));
                y += 26.0;
                k += 3;
            }
        }
        let ty = c.y + sz * 0.5 + 26.0;
        th::glitch_text(
            p,
            pos2(c.x, ty),
            Align2::CENTER_TOP,
            &self.title.to_uppercase(),
            th::carved_font(22.0),
            pal::TEXT,
        );
        p.text(
            pos2(c.x, ty + 34.0),
            Align2::CENTER_TOP,
            "reserved for future rites · add-in slot",
            th::bold_font(15.0),
            c32(pal::TEXT_DIM),
        );
        p.text(
            pos2(c.x, ty + 56.0),
            Align2::CENTER_TOP,
            format!(
                "ADD-IN SLOT {} · RUNE_KEY {} · awaiting a SettingsTab",
                self.n,
                geom::RUNES[self.rune()].name.to_uppercase()
            ),
            th::display_font(10.5),
            c32(pal::TEXT_MUTE),
        );
        let _ = chrome::center(r);
    }
}
