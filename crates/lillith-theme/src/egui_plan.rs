//! The Soul Contract **plan review** (feature `egui`).
//!
//! Everything Lillith wants to change or run arrives as one numbered plan.
//! This modal lists every item (grouped under its folder) with a checkbox,
//! an expandable detail (full paths, auto-renamed names, sizes, diffs,
//! delete listings, exact program / path / args / files / working folder /
//! risky), the totals, and three answers: **APPROVE ALL**, **APPROVE
//! SELECTED** and **DECLINE** (`Esc`). When a delete is in the set being
//! approved, a Trash (default) / Permanent choice and the typed phrase are
//! required too. The caller still checks everything against the plan
//! (`SoulContract::approve_plan`); this widget only reports the choice.

use crate::egui_theme::{additive, c32, skull};
use crate::palette::{self as pal, Rgba};
use egui::{Color32, Context, CornerRadius, FontFamily, FontId, Id, RichText, Stroke, Vec2};
use std::collections::BTreeSet;

/// One plan item, as plain strings (mirrors `PlanItem`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanRow {
    /// 1-based item number (execution order).
    pub n: usize,
    /// Folder (or `programs`) it is grouped under.
    pub group: String,
    /// One-line summary.
    pub line: String,
    /// Labelled facts (from, to, auto-renamed, size, program, …).
    pub fields: Vec<(String, String)>,
    /// Diff / delete listing / undo steps.
    pub detail: Vec<String>,
    /// For deletes: how many files and folders it removes (else 0).
    pub delete_entries: usize,
    /// A program run marked risky.
    pub risky: bool,
    /// Runs elevated through the admin helper — show the badge.
    pub elevated: bool,
}

/// What the popup shows.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlanView<'a> {
    pub rows: &'a [PlanRow],
    /// e.g. `5 item(s): 4 file change(s), 1 program run(s), 3.1 KiB involved`.
    pub totals: &'a str,
    /// A note above the list (e.g. "the files changed — new plan").
    pub note: Option<&'a str>,
}

/// The human's in-progress answer (kept by the caller across frames).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanState {
    /// Ticked items (all ticked when a plan first shows).
    pub checked: BTreeSet<usize>,
    /// Items whose details are open.
    pub expanded: BTreeSet<usize>,
    pub typed: String,
    pub permanent: bool,
    seeded: bool,
}

impl PlanState {
    /// Fresh state for `rows`: every item ticked.
    pub fn for_rows(rows: &[PlanRow]) -> Self {
        Self {
            checked: rows.iter().map(|r| r.n).collect(),
            seeded: true,
            ..Default::default()
        }
    }
}

/// What happened this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanChoice {
    /// Approve these items (all of them for APPROVE ALL). `typed` and
    /// `permanent` matter only when a delete is among them.
    Approve {
        items: Vec<usize>,
        typed: String,
        permanent: bool,
    },
    /// DECLINE or `Esc`: nothing runs.
    Decline,
}

/// The phrase that signs the delete items among `items` (`None` = no delete).
/// Same wording as `DeletionManifest::phrase`.
pub fn delete_phrase(rows: &[PlanRow], items: &BTreeSet<usize>) -> Option<String> {
    let n: usize = rows
        .iter()
        .filter(|r| items.contains(&r.n))
        .map(|r| r.delete_entries)
        .sum();
    match n {
        0 => None,
        1 => Some("DELETE 1 ITEM".into()),
        n => Some(format!("DELETE {n} ITEMS")),
    }
}

/// Below this inner width the plan lays out for a small window.
const NARROW_WIDTH: f32 = 360.0;

fn mono(s: impl Into<String>, size: f32, c: Rgba) -> RichText {
    RichText::new(s)
        .font(FontId::new(size, FontFamily::Monospace))
        .color(c32(c))
}

/// Raises the plan review for one frame. Call every frame while a plan is
/// pending.
pub fn plan_review_popup(
    ctx: &Context,
    id: Id,
    view: &PlanView<'_>,
    state: &mut PlanState,
) -> Option<PlanChoice> {
    if !state.seeded {
        *state = PlanState::for_rows(view.rows);
    }
    let screen = ctx.screen_rect();
    let w = crate::egui_contract::plate_width(screen.width());
    let list_h = (screen.height() * 0.45).clamp(90.0, 420.0);
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
    let all: BTreeSet<usize> = view.rows.iter().map(|r| r.n).collect();
    let resp = modal.show(ctx, |ui| {
        ui.set_width(w - 24.0);
        // In a small window (the 300 px chat bubble) the heading shrinks and
        // the rows / buttons stack, so nothing runs off the plate's edges.
        let narrow = ui.available_width() < NARROW_WIDTH;
        let mut out = None;
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
            let heading = if narrow { 13.0 } else { 17.0 };
            ui.label(mono("SOUL CONTRACT — THE PLAN", heading, pal::BLOOD_NEON));
            ui.label(
                RichText::new("Check every step. Nothing happens until you approve.")
                    .color(c32(pal::TEXT)),
            );
        });
        if let Some(n) = view.note.filter(|n| !n.is_empty()) {
            ui.label(RichText::new(n).color(c32(pal::WARN)));
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .small_button(RichText::new("select all").color(c32(pal::BONE)))
                .clicked()
            {
                state.checked = all.clone();
            }
            if ui
                .small_button(RichText::new("select none").color(c32(pal::BONE)))
                .clicked()
            {
                state.checked.clear();
            }
            ui.label(
                RichText::new(format!("{}/{} selected", state.checked.len(), all.len()))
                    .color(c32(pal::TEXT_DIM)),
            );
        });
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
                        let mut group: Option<&str> = None;
                        for row in view.rows {
                            if group != Some(row.group.as_str()) {
                                group = Some(row.group.as_str());
                                let kids: BTreeSet<usize> = view
                                    .rows
                                    .iter()
                                    .filter(|r| r.group == row.group)
                                    .map(|r| r.n)
                                    .collect();
                                let mut on = kids.is_subset(&state.checked);
                                let g = ui.checkbox(
                                    &mut on,
                                    mono(format!("📁 {}", row.group), 12.0, pal::EMBER_CORE),
                                );
                                if g.changed() {
                                    if on {
                                        state.checked.extend(kids);
                                    } else {
                                        state.checked.retain(|n| !kids.contains(n));
                                    }
                                }
                            }
                            ui.horizontal(|ui| {
                                ui.add_space(14.0);
                                let mut on = state.checked.contains(&row.n);
                                let colour = if row.delete_entries > 0 || row.risky || row.elevated
                                {
                                    pal::BLOOD_NEON
                                } else {
                                    pal::TERMINAL_TEXT
                                };
                                let c = ui.checkbox(
                                    &mut on,
                                    mono(format!("{:>2}. {}", row.n, row.line), 11.0, colour),
                                );
                                if row.elevated {
                                    ui.label(mono("ELEVATED (admin)", 10.0, pal::BLOOD_NEON));
                                }
                                ui.ctx()
                                    .data_mut(|d| d.insert_temp(id.with(("row", row.n)), c.rect));
                                if c.changed() {
                                    if on {
                                        state.checked.insert(row.n);
                                    } else {
                                        state.checked.remove(&row.n);
                                    }
                                }
                                let open = state.expanded.contains(&row.n);
                                if ui
                                    .small_button(if open { "▾ details" } else { "▸ details" })
                                    .clicked()
                                {
                                    if open {
                                        state.expanded.remove(&row.n);
                                    } else {
                                        state.expanded.insert(row.n);
                                    }
                                }
                            });
                            if state.expanded.contains(&row.n) {
                                ui.indent(id.with(("d", row.n)), |ui| {
                                    for (k, v) in &row.fields {
                                        ui.label(mono(format!("{k}: {v}"), 11.0, pal::TEXT_DIM));
                                    }
                                    for l in &row.detail {
                                        let c = if l.starts_with('+') {
                                            pal::OK
                                        } else if l.starts_with('-') {
                                            pal::BLOOD_NEON
                                        } else {
                                            pal::TERMINAL_TEXT
                                        };
                                        ui.label(mono(l, 10.5, c));
                                    }
                                });
                            }
                        }
                    });
            });
        ui.label(
            RichText::new(format!("Total: {}", view.totals))
                .strong()
                .color(c32(pal::EMBER_CORE)),
        );
        let sel_phrase = delete_phrase(view.rows, &state.checked);
        let all_phrase = delete_phrase(view.rows, &all);
        if all_phrase.is_some() {
            ui.horizontal_wrapped(|ui| {
                ui.radio_value(
                    &mut state.permanent,
                    false,
                    RichText::new("Deletes go to Trash").color(c32(pal::TEXT)),
                );
                ui.radio_value(
                    &mut state.permanent,
                    true,
                    RichText::new("Permanent").color(c32(pal::BLOOD_NEON)),
                );
            });
            let hint = match (&sel_phrase, &all_phrase) {
                (Some(s), Some(a)) if s != a => {
                    format!("To approve deletes, type: {s} (selected) or {a} (all)")
                }
                (_, Some(a)) => format!("To approve deletes, type: {a}"),
                _ => String::new(),
            };
            ui.label(RichText::new(hint).color(c32(pal::TEXT_DIM)));
            ui.add(
                egui::TextEdit::singleline(&mut state.typed)
                    .id(id.with("phrase"))
                    .hint_text(all_phrase.as_deref().unwrap_or(""))
                    .desired_width(f32::INFINITY),
            );
        }
        let typed_ok = |p: &Option<String>| match p {
            None => true,
            Some(p) => crate::egui_deletion::phrase_matches(&state.typed, p),
        };
        let all_ok = typed_ok(&all_phrase);
        let sel_ok = !state.checked.is_empty() && typed_ok(&sel_phrase);
        ui.add_space(6.0);
        let buttons = |ui: &mut egui::Ui| {
            let third = if narrow {
                ui.available_width()
            } else {
                (ui.available_width() - 16.0) / 3.0
            };
            let no = ui.add_sized(
                [third, 30.0],
                egui::Button::new(RichText::new("DECLINE").color(c32(pal::BONE)))
                    .fill(c32(pal::IRON)),
            );
            let some = ui.add_enabled_ui(sel_ok, |ui| {
                ui.add_sized(
                    [third, 30.0],
                    egui::Button::new(
                        RichText::new(format!("APPROVE SELECTED ({})", state.checked.len()))
                            .color(c32(pal::TEXT)),
                    )
                    .fill(c32(pal::IRON_MID))
                    .stroke(Stroke::new(1.0, c32(pal::EMBER))),
                )
            });
            let every = ui.add_enabled_ui(all_ok, |ui| {
                ui.add_sized(
                    [third, 30.0],
                    egui::Button::new(RichText::new("APPROVE ALL").strong().color(c32(pal::TEXT)))
                        .fill(c32(pal::BLOOD_DEEP))
                        .stroke(Stroke::new(1.5, c32(pal::BLOOD_NEON))),
                )
            });
            ui.ctx().data_mut(|d| {
                d.insert_temp(id.with("no-rect"), no.rect);
                d.insert_temp(id.with("some-rect"), some.inner.rect);
                d.insert_temp(id.with("all-rect"), every.inner.rect);
            });
            if no.clicked() {
                out = Some(PlanChoice::Decline);
            } else if every.inner.clicked() && all_ok {
                out = Some(PlanChoice::Approve {
                    items: all.iter().copied().collect(),
                    typed: state.typed.clone(),
                    permanent: state.permanent,
                });
            } else if some.inner.clicked() && sel_ok {
                out = Some(PlanChoice::Approve {
                    items: state.checked.iter().copied().collect(),
                    typed: state.typed.clone(),
                    permanent: state.permanent,
                });
            }
        };
        if narrow {
            ui.vertical(buttons);
        } else {
            ui.horizontal(buttons);
        }
        ui.label(
            RichText::new(
                "Only what you approve runs, exactly as listed. Every decision is logged.",
            )
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
        return Some(PlanChoice::Decline);
    }
    resp.inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Rect};

    fn rows() -> Vec<PlanRow> {
        vec![
            PlanRow {
                n: 1,
                group: "/home/ko/Downloads".into(),
                line: "move /home/ko/Downloads/a.png → /home/ko/Downloads/Images/a.png".into(),
                ..Default::default()
            },
            PlanRow {
                n: 2,
                group: "/home/ko/Downloads".into(),
                line: "delete /home/ko/Downloads/old (2 files)".into(),
                delete_entries: 3,
                ..Default::default()
            },
            PlanRow {
                n: 3,
                group: "programs".into(),
                line: "run $ df -h".into(),
                ..Default::default()
            },
            PlanRow {
                n: 4,
                group: "programs".into(),
                line: "cmd $ ver".into(),
                elevated: true,
                risky: true,
                ..Default::default()
            },
        ]
    }

    fn raw(events: Vec<egui::Event>, size: Vec2) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..Default::default()
        }
    }

    fn frame(ctx: &Context, ev: Vec<egui::Event>, s: &mut PlanState) -> Option<PlanChoice> {
        frame_sized(ctx, ev, s, Vec2::new(420.0, 560.0))
    }

    fn frame_sized(
        ctx: &Context,
        ev: Vec<egui::Event>,
        s: &mut PlanState,
        size: Vec2,
    ) -> Option<PlanChoice> {
        let rows = rows();
        let view = PlanView {
            rows: &rows,
            totals: "3 item(s)",
            note: None,
        };
        let mut out = None;
        let _ = ctx.run(raw(ev, size), |ctx| {
            egui::CentralPanel::default().show(ctx, |_| {});
            out = plan_review_popup(ctx, Id::new("plan-test"), &view, s);
        });
        out
    }

    fn click(ctx: &Context, s: &mut PlanState, pos: Pos2) -> Option<PlanChoice> {
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

    fn rect(ctx: &Context, key: impl std::hash::Hash) -> Rect {
        ctx.data(|d| d.get_temp::<Rect>(Id::new("plan-test").with(key)))
            .expect("laid out")
    }

    #[test]
    fn phrases_cover_only_the_selected_deletes() {
        let r = rows();
        assert_eq!(delete_phrase(&r, &[1, 3].into()), None);
        assert_eq!(
            delete_phrase(&r, &[2].into()).as_deref(),
            Some("DELETE 3 ITEMS")
        );
    }

    #[test]
    fn approve_all_needs_the_phrase_subset_without_deletes_does_not() {
        for themed in [false, true] {
            let ctx = Context::default();
            if themed {
                crate::egui_theme::install(&ctx);
            }
            let mut s = PlanState::default();
            for _ in 0..3 {
                assert_eq!(frame(&ctx, vec![], &mut s), None);
            }
            assert_eq!(s.checked.len(), 4, "everything ticked at first");
            // APPROVE ALL is dead until the delete phrase is typed.
            assert_eq!(click(&ctx, &mut s, rect(&ctx, "all-rect").center()), None);
            // Untick the delete (item 2): APPROVE SELECTED works without a phrase.
            let row2 = rect(&ctx, ("row", 2usize));
            let _ = click(&ctx, &mut s, Pos2::new(row2.left() + 6.0, row2.center().y));
            assert_eq!(s.checked, [1, 3, 4].into());
            for _ in 0..2 {
                let _ = frame(&ctx, vec![], &mut s);
            }
            assert_eq!(
                click(&ctx, &mut s, rect(&ctx, "some-rect").center()),
                Some(PlanChoice::Approve {
                    items: vec![1, 3, 4],
                    typed: String::new(),
                    permanent: false
                })
            );
            s.typed = "delete 3 items".into();
            for _ in 0..2 {
                let _ = frame(&ctx, vec![], &mut s);
            }
            assert_eq!(
                click(&ctx, &mut s, rect(&ctx, "all-rect").center()),
                Some(PlanChoice::Approve {
                    items: vec![1, 2, 3, 4],
                    typed: "delete 3 items".into(),
                    permanent: false
                })
            );
            assert_eq!(
                click(&ctx, &mut s, rect(&ctx, "no-rect").center()),
                Some(PlanChoice::Decline)
            );
        }
    }

    #[test]
    fn the_buttons_stay_inside_the_small_chat_bubble() {
        // The chat bubble is 300 px wide: the plan must fit, buttons and all.
        for themed in [false, true] {
            let ctx = Context::default();
            if themed {
                crate::egui_theme::install(&ctx);
            }
            let mut s = PlanState::default();
            for _ in 0..3 {
                let _ = frame_sized(&ctx, vec![], &mut s, Vec2::new(300.0, 380.0));
            }
            for key in ["no-rect", "some-rect", "all-rect"] {
                let r = rect(&ctx, key);
                assert!(
                    r.left() >= 0.0 && r.right() <= 300.0,
                    "{key} {r:?} runs off a 300 px window"
                );
            }
        }
    }
}
