//! The **deletion** Soul Contract (feature `egui`).
//!
//! Deleting is never a plain Yes/No. This modal shows EVERY file and folder
//! that would go (full paths, sizes, folder contents, totals) in a
//! scrollable crimson terminal slab, a **Trash** (default) / **Permanent**
//! choice, and a field where the human types the exact phrase (e.g.
//! `DELETE 12 ITEMS`). **SIGN IN BLOOD** only lights up when the phrase
//! matches; **REFUSE** and `Esc` cancel. There is no keyboard shortcut for
//! signing: SIGN is mouse-only (never focusable, so Tab skips it, and only
//! a primary-pointer click counts, never Enter/Space). The caller still verifies the phrase against the list
//! (`DeletionSignature::sign`): this widget only reports the click.

use crate::egui_theme::{additive, c32, skull};
use crate::palette::{self as pal, Rgba};
use egui::{Color32, Context, CornerRadius, FontFamily, FontId, Id, RichText, Stroke, Vec2};

/// What the popup shows (plain strings; mirrors `DeletionManifest`).
#[derive(Debug, Clone, Copy, Default)]
pub struct DeletionView<'a> {
    /// The full listing, one entry per line (never elided by the caller).
    pub listing: &'a str,
    /// e.g. `3 files, 1 folder, 4.2 MiB`.
    pub totals: &'a str,
    /// The phrase to type, e.g. `DELETE 4 ITEMS`.
    pub phrase: &'a str,
    /// A note above the list (e.g. "the files changed — new list").
    pub note: Option<&'a str>,
}

/// The human's in-progress answer (kept by the caller across frames).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeletionState {
    pub typed: String,
    pub permanent: bool,
}

/// What happened this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeletionChoice {
    /// SIGN was clicked with the phrase typed.
    Sign { typed: String, permanent: bool },
    /// REFUSE or `Esc`.
    Cancel,
}

/// The phrase check the button uses (case and extra spaces ignored), the
/// same rule as `DeletionSignature::sign`.
pub fn phrase_matches(typed: &str, phrase: &str) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_uppercase()
    };
    !phrase.trim().is_empty() && norm(typed) == norm(phrase)
}

/// Raises the deletion contract for one frame. Call every frame while a
/// deletion is pending.
pub fn deletion_contract_popup(
    ctx: &Context,
    id: Id,
    view: &DeletionView<'_>,
    state: &mut DeletionState,
) -> Option<DeletionChoice> {
    let screen = ctx.screen_rect();
    let w = crate::egui_contract::plate_width(screen.width());
    let list_h = (screen.height() * 0.38).clamp(80.0, 320.0);
    let t = ctx.input(|i| i.time) as f32;
    let flick = 0.5 + 0.5 * (t * 2.3).sin() * (t * 1.3).cos();
    let frame = egui::Frame::new()
        .fill(c32(Rgba::hex(0x0d0306)))
        .stroke(Stroke::new(2.0, c32(pal::CRIMSON)))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(egui::Margin::same(12));
    let modal = egui::Modal::new(id)
        .frame(frame)
        .backdrop_color(Color32::from_rgba_premultiplied(4, 0, 2, 210));
    let resp = modal.show(ctx, |ui| {
        ui.set_width(w - 24.0);
        let mut out = None;
        // forge-lit skull + heading
        let (r, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), egui::Sense::hover());
        let p = ui.painter_at(r.expand(8.0));
        p.circle_filled(r.center(), 22.0, additive(pal::EMBER, 0.12 + 0.08 * flick));
        skull(
            &p,
            r.center(),
            34.0,
            pal::EMBER.lerp(pal::EMBER_CORE, flick * 0.6),
        );
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("SOUL CONTRACT — DELETION")
                    .font(FontId::new(17.0, FontFamily::Monospace))
                    .color(c32(pal::BLOOD_NEON)),
            );
            ui.label(RichText::new("She wants to delete everything below.").color(c32(pal::TEXT)));
        });
        if let Some(n) = view.note.filter(|n| !n.is_empty()) {
            ui.label(RichText::new(n).color(c32(pal::WARN)));
        }
        ui.add_space(4.0);
        egui::Frame::new()
            .fill(c32(pal::TERMINAL_FILL))
            .stroke(Stroke::new(1.0, c32(pal::BLOOD_NEON.alpha(150))))
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .id_salt(id.with("list"))
                    .max_height(list_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for line in view.listing.lines() {
                            ui.label(
                                RichText::new(line)
                                    .font(FontId::new(11.0, FontFamily::Monospace))
                                    .color(c32(pal::TERMINAL_TEXT)),
                            );
                        }
                    });
            });
        ui.label(
            RichText::new(format!("Total: {}", view.totals))
                .strong()
                .color(c32(pal::EMBER_CORE)),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.radio_value(
                &mut state.permanent,
                false,
                RichText::new("Trash (can be restored)").color(c32(pal::TEXT)),
            );
            ui.radio_value(
                &mut state.permanent,
                true,
                RichText::new("Permanent").color(c32(pal::BLOOD_NEON)),
            );
        });
        if state.permanent {
            ui.label(
                RichText::new("Permanent: these can NOT be brought back.")
                    .color(c32(pal::BLOOD_NEON)),
            );
        }
        ui.label(
            RichText::new(format!("To sign, type: {}", view.phrase)).color(c32(pal::TEXT_DIM)),
        );
        let field = ui.add(
            egui::TextEdit::singleline(&mut state.typed)
                .id(id.with("phrase"))
                .hint_text(view.phrase)
                .desired_width(f32::INFINITY),
        );
        let ok = phrase_matches(&state.typed, view.phrase);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let half = (ui.available_width() - 8.0) / 2.0;
            let no = ui.add_sized(
                [half, 30.0],
                egui::Button::new(RichText::new("REFUSE").color(c32(pal::BONE)))
                    .fill(c32(pal::IRON)),
            );
            let label = if state.permanent {
                "SIGN — ERASE FOREVER"
            } else {
                "SIGN IN BLOOD — TRASH"
            };
            // SIGN is mouse-only, like the Yes plate: it senses clicks but
            // is never focusable (Tab skips it), and only a real primary
            // pointer click counts (`clicked()` also fires for Enter/Space
            // on a focused widget).
            let yes = ui.add_enabled(
                ok,
                egui::Button::new(RichText::new(label).strong().color(c32(pal::TEXT)))
                    .fill(c32(pal::BLOOD_DEEP))
                    .stroke(Stroke::new(1.5, c32(pal::BLOOD_NEON)))
                    .sense(egui::Sense::CLICK),
            );
            ui.ctx().data_mut(|d| {
                d.insert_temp(id.with("no-rect"), no.rect);
                d.insert_temp(id.with("sign-rect"), yes.rect);
                d.insert_temp(id.with("sign-id"), yes.id);
            });
            if no.clicked() {
                out = Some(DeletionChoice::Cancel);
            } else if yes.clicked_by(egui::PointerButton::Primary) && ok {
                out = Some(DeletionChoice::Sign {
                    typed: state.typed.clone(),
                    permanent: state.permanent,
                });
            }
        });
        let _ = field;
        ui.label(
            RichText::new("Nothing is deleted without your signature. Every deletion is logged.")
                .small()
                .color(c32(pal::TEXT_MUTE)),
        );
        out
    });
    ctx.request_repaint_after(std::time::Duration::from_millis(60));
    let esc = resp.is_top_modal
        && !resp.any_popup_open
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    if esc {
        return Some(DeletionChoice::Cancel);
    }
    resp.inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Rect};

    const LIST: &str =
        "📁 /home/ko/old (2 items, 3 B)\n  📄 /home/ko/old/a.txt 1 B\n  📄 /home/ko/old/b.txt 2 B";

    fn view() -> DeletionView<'static> {
        DeletionView {
            listing: LIST,
            totals: "2 files, 1 folder, 3 B",
            phrase: "DELETE 3 ITEMS",
            note: None,
        }
    }

    fn raw(size: Vec2, events: Vec<egui::Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..Default::default()
        }
    }

    fn frame(ctx: &Context, ev: Vec<egui::Event>, s: &mut DeletionState) -> Option<DeletionChoice> {
        let mut out = None;
        let _ = ctx.run(raw(Vec2::new(300.0, 380.0), ev), |ctx| {
            egui::CentralPanel::default().show(ctx, |_| {});
            out = deletion_contract_popup(ctx, Id::new("del-test"), &view(), s);
        });
        out
    }

    fn click(ctx: &Context, s: &mut DeletionState, pos: Pos2) -> Option<DeletionChoice> {
        let _ = frame(ctx, vec![egui::Event::PointerMoved(pos)], s);
        let btn = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let a = frame(ctx, vec![btn(true)], s);
        let b = frame(ctx, vec![btn(false)], s);
        a.or(b)
    }

    fn rect(ctx: &Context, which: &str) -> Rect {
        ctx.data(|d| d.get_temp::<Rect>(Id::new("del-test").with(which)))
            .expect("laid out")
    }

    #[test]
    fn phrase_rule_matches_core() {
        assert!(phrase_matches(" delete  3 items ", "DELETE 3 ITEMS"));
        assert!(!phrase_matches("DELETE 2 ITEMS", "DELETE 3 ITEMS"));
        assert!(!phrase_matches("", ""));
    }

    fn key(k: egui::Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Presses and releases `k` over two frames and returns any choice.
    fn tap(ctx: &Context, s: &mut DeletionState, k: egui::Key) -> Option<DeletionChoice> {
        let a = frame(ctx, vec![key(k, true)], s);
        let b = frame(ctx, vec![key(k, false)], s);
        a.or(b)
    }

    #[test]
    fn keyboard_never_signs() {
        for activate in [egui::Key::Enter, egui::Key::Space] {
            let ctx = Context::default();
            // The phrase is typed, so SIGN is live: only a click may sign.
            let mut s = DeletionState {
                typed: "DELETE 3 ITEMS".into(),
                permanent: false,
            };
            for _ in 0..3 {
                assert_eq!(frame(&ctx, vec![], &mut s), None);
            }
            // Tab through every focus stop (and wrap), pressing the
            // activation key at each one: never Sign.
            for _ in 0..8 {
                let t = tap(&ctx, &mut s, egui::Key::Tab);
                assert!(!matches!(t, Some(DeletionChoice::Sign { .. })));
                let r = tap(&ctx, &mut s, activate);
                assert!(
                    !matches!(r, Some(DeletionChoice::Sign { .. })),
                    "Tab+{activate:?} signed"
                );
                if r == Some(DeletionChoice::Cancel) {
                    // REFUSE took the key; keep tabbing from a fresh card.
                    s.typed = "DELETE 3 ITEMS".into();
                }
            }
            let sign_id = ctx
                .data(|d| d.get_temp::<Id>(Id::new("del-test").with("sign-id")))
                .expect("laid out");
            assert!(
                !ctx.memory(|m| m.has_focus(sign_id)),
                "Tab can't land on SIGN"
            );
            // Even with focus forced onto SIGN, the key does nothing.
            ctx.memory_mut(|m| m.request_focus(sign_id));
            let _ = frame(&ctx, vec![], &mut s);
            let r = tap(&ctx, &mut s, activate);
            assert!(
                !matches!(r, Some(DeletionChoice::Sign { .. })),
                "focused SIGN + {activate:?} signed"
            );
            // A real click still signs.
            let sign = rect(&ctx, "sign-rect");
            assert!(matches!(
                click(&ctx, &mut s, sign.center()),
                Some(DeletionChoice::Sign { .. })
            ));
        }
    }

    #[test]
    fn sign_only_with_the_phrase_and_esc_cancels() {
        for themed in [false, true] {
            let ctx = Context::default();
            if themed {
                crate::egui_theme::install(&ctx);
            }
            let mut s = DeletionState::default();
            for _ in 0..3 {
                assert_eq!(frame(&ctx, vec![], &mut s), None);
            }
            let sign = rect(&ctx, "sign-rect");
            // Not typed: the button is dead.
            assert_eq!(click(&ctx, &mut s, sign.center()), None);
            s.typed = "delete 3 items".into();
            s.permanent = true;
            for _ in 0..2 {
                let _ = frame(&ctx, vec![], &mut s);
            }
            let sign = rect(&ctx, "sign-rect");
            assert_eq!(
                click(&ctx, &mut s, sign.center()),
                Some(DeletionChoice::Sign {
                    typed: "delete 3 items".into(),
                    permanent: true
                })
            );
            let no = rect(&ctx, "no-rect");
            assert_eq!(
                click(&ctx, &mut s, no.center()),
                Some(DeletionChoice::Cancel)
            );
            let esc = egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            assert_eq!(frame(&ctx, vec![esc], &mut s), Some(DeletionChoice::Cancel));
        }
    }
}
