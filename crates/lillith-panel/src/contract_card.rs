//! The generic stand-alone Soul Contract card (`lillith contract-card`),
//! see `lillith_core::card_process` for the stdin / stdout protocol. Used
//! by code that runs without an egui window of its own: the KWin
//! pre-start check (before her first frame) and the voice daemon's mic
//! gate. The popup is the chat bubble's: only a primary-button mouse click
//! on its non-focusable Yes answers Yes; Esc or No is No; closing the
//! window cancels; no answer within the timeout is a timeout. The caller
//! audits the decision; this window records and starts nothing.
//!
//! The popup shows at most [`DETAIL_LINES`] lines of 120 chars. When the
//! detail is longer, the card first shows all of it (scrolling, never
//! cut) with a "Continue to the contract" button, which only moves on to
//! the popup; it never approves anything.

use eframe::egui;
use lillith_core::card_process::{CardAnswer, CardText};
use lillith_theme::egui_contract::{
    soul_contract_popup, ContractChoice, ContractView, DETAIL_LINES,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Whether the popup would cut `detail` (so the full page comes first).
pub fn needs_full_page(detail: &str) -> bool {
    detail.lines().count() > DETAIL_LINES || detail.lines().any(|l| l.chars().count() > 120)
}

struct Card {
    text: CardText,
    full_page: bool,
    opened: Instant,
    timeout: Duration,
    out: Arc<Mutex<Option<CardAnswer>>>,
    closing: bool,
}

impl Card {
    fn finish(&mut self, ctx: &egui::Context, c: CardAnswer) {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        if out.is_none() {
            *out = Some(c);
        }
        if !self.closing {
            self.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for Card {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.opened.elapsed() >= self.timeout {
            self.finish(ctx, CardAnswer::TimedOut);
            return;
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        if self.full_page {
            let mut go_on = false;
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading(&self.text.title);
                ui.label(&self.text.summary);
                if !self.text.target.is_empty() {
                    ui.monospace(&self.text.target);
                }
                ui.separator();
                ui.label("The full change (read it, then continue to the contract):");
                egui::ScrollArea::both()
                    .max_height(ui.available_height() - 40.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(egui::RichText::new(&self.text.detail).monospace())
                                .extend(),
                        );
                    });
                ui.separator();
                // Only moves on to the popup: it approves nothing.
                go_on = ui.button("Continue to the contract").clicked();
            });
            if go_on {
                self.full_page = false;
            }
            return;
        }
        egui::CentralPanel::default().show(ctx, |_| {});
        let detail = if needs_full_page(&self.text.detail) {
            format!(
                "{}\n(the full change was on the previous page)",
                self.text.detail
            )
        } else {
            self.text.detail.clone()
        };
        let view = ContractView {
            title: &self.text.title,
            summary: &self.text.summary,
            target: (!self.text.target.is_empty()).then_some(self.text.target.as_str()),
            detail: (!detail.is_empty()).then_some(detail.as_str()),
            ..Default::default()
        };
        if let Some(choice) =
            soul_contract_popup(ctx, egui::Id::new("lillith-contract-card"), &view)
        {
            let a = if choice == ContractChoice::Yes {
                CardAnswer::Yes
            } else {
                CardAnswer::No
            };
            self.finish(ctx, a);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        if out.is_none() {
            *out = Some(CardAnswer::Cancelled);
        }
    }
}

/// Shows the card until it is answered, closed or times out. `Err` when
/// no window could be opened (no display).
pub fn run_contract_card(text: CardText, timeout: Duration) -> eframe::Result<CardAnswer> {
    let out = Arc::new(Mutex::new(None));
    let viewport = egui::ViewportBuilder::default()
        .with_title("Lillith — Soul Contract")
        .with_app_id("lillith-soul-contract")
        .with_inner_size([620.0, 520.0])
        .with_always_on_top();
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let o = out.clone();
    let full_page = needs_full_page(&text.detail);
    eframe::run_native(
        "lillith-contract-card",
        options,
        Box::new(move |cc| {
            lillith_theme::bubble::apply(&cc.egui_ctx);
            cc.egui_ctx.set_fonts(crate::chat_bubble::bubble_fonts());
            Ok(Box::new(Card {
                text,
                full_page,
                opened: Instant::now(),
                timeout,
                out: o,
                closing: false,
            }))
        }),
    )?;
    let c = *out.lock().unwrap_or_else(|e| e.into_inner());
    Ok(c.unwrap_or(CardAnswer::Cancelled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_or_wide_detail_gets_the_full_page_first() {
        assert!(!needs_full_page("a\nb"));
        assert!(needs_full_page(&"x\n".repeat(DETAIL_LINES + 1)));
        assert!(needs_full_page(&"y".repeat(121)));
    }
}
