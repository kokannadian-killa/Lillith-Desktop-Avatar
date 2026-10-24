//! A stand-alone model-server Soul Contract card (`lillith server-card`),
//! for `lillith chat --spawn-server` when a desktop is available: the same
//! popup as the chat bubble's, in its own small window. Only a click on Yes
//! answers Yes; Esc (or the card's No) is No; closing the window cancels;
//! no answer within the timeout is a timeout. The caller audits the
//! decision; this window records and starts nothing.

use eframe::egui;
use lillith_core::soul_contract::{Answer, ContractPrompt};
use lillith_mind::chat::CardChoice;
use lillith_theme::egui_contract::{soul_contract_popup, ContractChoice, ContractView, Initiator};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The answer as one stdout word (`yes`, `no`, `timeout`, `cancelled`).
pub fn choice_word(c: CardChoice) -> &'static str {
    match c {
        CardChoice::Clicked(Answer::Yes) => "yes",
        CardChoice::Clicked(Answer::No) => "no",
        CardChoice::TimedOut => "timeout",
        CardChoice::Cancelled => "cancelled",
    }
}

/// Parses [`choice_word`] back (`None` for anything else).
pub fn parse_choice_word(w: &str) -> Option<CardChoice> {
    Some(match w.trim() {
        "yes" => CardChoice::Clicked(Answer::Yes),
        "no" => CardChoice::Clicked(Answer::No),
        "timeout" => CardChoice::TimedOut,
        "cancelled" => CardChoice::Cancelled,
        _ => return None,
    })
}

struct Card {
    prompt: ContractPrompt,
    opened: Instant,
    timeout: Duration,
    out: Arc<Mutex<Option<CardChoice>>>,
    closing: bool,
}

impl Card {
    fn finish(&mut self, ctx: &egui::Context, c: CardChoice) {
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
        egui::CentralPanel::default().show(ctx, |_| {});
        if self.opened.elapsed() >= self.timeout {
            self.finish(ctx, CardChoice::TimedOut);
            return;
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        let p = &self.prompt;
        let view = ContractView {
            title: p.title,
            summary: &p.summary,
            target: p.target.as_deref(),
            detail: p.detail.as_deref(),
            busy: false,
            // `--spawn-server` is Ko's own request.
            initiator: Initiator::You,
        };
        if let Some(choice) = soul_contract_popup(ctx, egui::Id::new("lillith-server-card"), &view)
        {
            let a = Answer::from_yes(choice == ContractChoice::Yes);
            self.finish(ctx, CardChoice::Clicked(a));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        if out.is_none() {
            *out = Some(CardChoice::Cancelled);
        }
    }
}

/// Shows the card until it is answered, closed or times out. `Err` when
/// no window could be opened (no display): the caller falls back.
pub fn run_server_card(prompt: ContractPrompt, timeout: Duration) -> eframe::Result<CardChoice> {
    let out = Arc::new(Mutex::new(None));
    let viewport = egui::ViewportBuilder::default()
        .with_title("Lillith — Soul Contract")
        .with_app_id("lillith-soul-contract")
        .with_inner_size([520.0, 420.0])
        .with_always_on_top();
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let o = out.clone();
    eframe::run_native(
        "lillith-server-card",
        options,
        Box::new(move |cc| {
            lillith_theme::bubble::apply(&cc.egui_ctx);
            cc.egui_ctx.set_fonts(crate::chat_bubble::bubble_fonts());
            Ok(Box::new(Card {
                prompt,
                opened: Instant::now(),
                timeout,
                out: o,
                closing: false,
            }))
        }),
    )?;
    let c = *out.lock().unwrap_or_else(|e| e.into_inner());
    Ok(c.unwrap_or(CardChoice::Cancelled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_words_round_trip_and_nothing_else_parses() {
        for c in [
            CardChoice::Clicked(Answer::Yes),
            CardChoice::Clicked(Answer::No),
            CardChoice::TimedOut,
            CardChoice::Cancelled,
        ] {
            assert_eq!(parse_choice_word(choice_word(c)), Some(c));
        }
        for w in ["", "y", "Yes", "YES", "ok", "yes please"] {
            assert_eq!(parse_choice_word(w), None, "{w:?}");
        }
    }
}
