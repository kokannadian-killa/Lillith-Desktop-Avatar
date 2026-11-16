//! Knowledge (P3, docs/SELF_LEARNING.md "Knowledge panel"): what she has
//! learned, as a Settings tab.
//!
//! - **Status**: learning On / Off (`[learn] enabled`, saved with the
//!   window; Off also stops the running learner thread right away through
//!   [`lillith_learn::knowledge::set_switch`]), note counts per topic, the
//!   folder, and whether online trips, practice and page sharing are on.
//! - **Browse / search** every note; pick one to read it.
//! - **Edit** title and body (the note becomes Ko's, `origin: ko`, so the
//!   learner never merges, decays or prunes it) and **Forget** (the file
//!   and its memory row). Both are mouse-only (no keyboard activation) and
//!   audited by [`lillith_learn::knowledge`].
//! - **Sources** and **blocked hosts** (read-only here), and **activity**:
//!   the newest learning lines of the audit log (idle steps, trips,
//!   practice runs, dream sessions).
//!
//! No network; everything is the local knowledge folder and config.

use super::{SettingsTab, Shared};
use crate::chrome;
use egui::{vec2, FontId, RichText, Sense, Stroke};
use lillith_core::config::Config;
use lillith_learn::knowledge::{self, Entry, Status};
use lillith_theme::egui_theme::c32;
use lillith_theme::palette as pal;
use std::path::{Path, PathBuf};

/// Notes listed at once (search narrows it).
const MAX_LISTED: usize = 200;
/// Activity lines shown.
const MAX_ACTIVITY: usize = 30;
/// Re-read the folder at most this often (seconds).
const REFRESH_S: f64 = 5.0;

/// A small button that senses clicks but is not focusable: only a real
/// mouse click (`clicked_by(Primary)`) acts, never Tab + Enter / Space.
fn mouse_button(ui: &mut egui::Ui, label: &str, id: &str) -> egui::Response {
    let font = FontId::proportional(11.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.into(), font, c32(pal::TEXT));
    let size = galley.size() + vec2(10.0, 4.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let r = ui.interact(rect, ui.id().with(("knowledge", label, id)), Sense::CLICK);
    let col = if r.hovered() {
        pal::BLOOD
    } else {
        pal::TEXT_MUTE
    };
    ui.painter().rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, c32(col)),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.min + vec2(5.0, 2.0), galley, c32(pal::TEXT));
    r
}

pub struct KnowledgeTab {
    config_path: PathBuf,
    cfg: Option<Config>,
    entries: Vec<Entry>,
    status: Status,
    activity: Vec<String>,
    loaded_at: Option<f64>,
    error: Option<String>,
    query: String,
    /// The picked note (`rel`) and its edit buffers.
    selected: Option<String>,
    edit_title: String,
    edit_body: String,
    /// Where each listed note's Forget button was drawn (tests).
    pub(crate) forget_hits: Vec<(String, egui::Rect)>,
    /// Where the Save edit button was drawn (tests).
    pub(crate) save_hit: Option<egui::Rect>,
    /// Off marker override (tests); `None`: the runtime dir's.
    pub(crate) marker: Option<PathBuf>,
    /// The audit log (`<data>/lillith/audit.log`; tests point it elsewhere).
    pub(crate) audit: Option<PathBuf>,
}

impl KnowledgeTab {
    pub fn new(config_path: &Path) -> Self {
        Self {
            config_path: config_path.to_path_buf(),
            cfg: None,
            entries: Vec::new(),
            status: Status::default(),
            activity: Vec::new(),
            loaded_at: None,
            error: None,
            query: String::new(),
            selected: None,
            edit_title: String::new(),
            edit_body: String::new(),
            forget_hits: Vec::new(),
            save_hit: None,
            marker: None,
            audit: lillith_core::config::audit_log_path(),
        }
    }

    /// Re-reads config, notes and the activity log.
    pub fn refresh(&mut self, now: f64) {
        self.loaded_at = Some(now);
        let cfg = match Config::load(&self.config_path) {
            Ok(c) => c,
            // No config yet: the defaults.
            Err(_) if !self.config_path.exists() => Config::default(),
            Err(e) => {
                self.error = Some(format!("config: {e}"));
                return;
            }
        };
        match knowledge::list(&cfg) {
            Ok(v) => {
                self.entries = v;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        self.status = knowledge::status(&cfg, &self.entries);
        if let Some(m) = &self.marker {
            self.status.learning = cfg.learn.enabled && !knowledge::switched_off_at(m);
        }
        self.activity = knowledge::activity(self.audit.as_deref(), MAX_ACTIVITY);
        if let Some(rel) = &self.selected {
            if !self.entries.iter().any(|e| &e.rel == rel) {
                self.selected = None;
            }
        }
        self.cfg = Some(cfg);
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn select(&mut self, rel: &str) {
        if let Some(e) = self.entries.iter().find(|e| e.rel == rel) {
            self.selected = Some(rel.to_string());
            self.edit_title = e.title.clone();
            self.edit_body = e.body.clone();
        }
    }

    fn forget(&mut self, rel: &str, shared: &mut Shared, now: f64) {
        let Some(cfg) = &self.cfg else { return };
        match knowledge::forget(cfg, rel, self.audit.as_deref()) {
            Ok(title) => shared.toast(format!("Forgot \"{title}\"."), pal::TEXT),
            Err(e) => shared.toast(format!("Couldn't forget: {e}"), pal::BLOOD),
        }
        self.refresh(now);
    }

    fn save_edit(&mut self, shared: &mut Shared, now: f64) {
        let (Some(cfg), Some(rel)) = (&self.cfg, self.selected.clone()) else {
            return;
        };
        match knowledge::edit(
            cfg,
            &rel,
            &self.edit_title,
            &self.edit_body,
            self.audit.as_deref(),
        ) {
            Ok(e) => {
                shared.toast(format!("Saved \"{}\" as your note.", e.title), pal::TEXT);
                self.refresh(now);
                self.select(&e.rel);
            }
            Err(e) => shared.toast(format!("Not saved: {e}"), pal::BLOOD),
        }
    }

    fn set_learning(&mut self, on: bool, shared: &mut Shared) {
        shared.store.set_bool(&["learn", "enabled"], on);
        let r = match &self.marker {
            Some(m) => knowledge::set_switch_at(m, on),
            None => knowledge::set_switch(on),
        };
        if let Err(e) = r {
            shared.toast(format!("Couldn't switch the learner: {e}"), pal::BLOOD);
        }
        self.status.learning = on;
        shared.toast(
            if on {
                "Learning on (saved with Save; starts with her next start)."
            } else {
                "Learning off: the learner stops now (Save keeps it off)."
            },
            pal::TEXT,
        );
    }
}

impl SettingsTab for KnowledgeTab {
    fn id(&self) -> &str {
        "knowledge"
    }
    fn title(&self) -> &str {
        "Knowledge"
    }
    fn rune(&self) -> usize {
        5
    }

    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let now = shared.time;
        if self.loaded_at.is_none_or(|t| now - t >= REFRESH_S) {
            self.refresh(now);
        }
        self.forget_hits.clear();
        self.save_hit = None;
        let st = self.status.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            chrome::section(ui, "KNOWLEDGE", Some("local only"), |ui| {
                if let Some(e) = &self.error {
                    ui.label(RichText::new(e).color(c32(pal::BLOOD)));
                }
                let on = shared
                    .store
                    .get_bool(&["learn", "enabled"])
                    .unwrap_or(st.learning);
                if chrome::rune_check(ui, on, "Learning (idle reading of installed docs)").clicked()
                {
                    self.set_learning(!on, shared);
                }
                if !st.memory {
                    chrome::hint(ui, "Memory is off: nothing new is learned or indexed.");
                }
                ui.label(
                    RichText::new(format!(
                        "{} notes ({} yours) · online trips {} · practice {} · page sharing {}",
                        st.notes,
                        st.ko_notes,
                        if st.online { "on" } else { "off" },
                        if st.practice { "on" } else { "off" },
                        if st.share_socket { "on" } else { "off" },
                    ))
                    .color(c32(pal::TEXT_DIM)),
                );
                if !st.by_topic.is_empty() {
                    let t: Vec<String> = st
                        .by_topic
                        .iter()
                        .map(|(t, n)| format!("{t} {n}"))
                        .collect();
                    ui.label(
                        RichText::new(t.join(" · "))
                            .small()
                            .color(c32(pal::TEXT_MUTE)),
                    );
                }
                if let Some(f) = &st.folder {
                    chrome::row(ui, "folder", |ui| ui.monospace(f.display().to_string()));
                }
            });
            chrome::section(ui, "NOTES", None, |ui| {
                chrome::row(ui, "search", |ui| ui.text_edit_singleline(&mut self.query));
                let hits: Vec<Entry> = knowledge::search(&self.entries, &self.query)
                    .into_iter()
                    .take(MAX_LISTED)
                    .cloned()
                    .collect();
                if hits.is_empty() {
                    chrome::hint(ui, "No notes yet.");
                }
                let mut pick = None;
                let mut forget = None;
                for e in &hits {
                    ui.horizontal(|ui| {
                        let b = mouse_button(ui, "Forget", &e.rel);
                        self.forget_hits.push((e.rel.clone(), b.rect));
                        if b.clicked_by(egui::PointerButton::Primary) {
                            forget = Some(e.rel.clone());
                        }
                        let mark = if e.origin == "ko" { " ✎" } else { "" };
                        let text = format!("{} · {} · {}{mark}", e.title, e.topic, e.kind);
                        let sel = self.selected.as_deref() == Some(e.rel.as_str());
                        if ui.selectable_label(sel, text).clicked() {
                            pick = Some(e.rel.clone());
                        }
                    });
                }
                if let Some(rel) = pick {
                    self.select(&rel);
                }
                if let Some(rel) = forget {
                    self.forget(&rel, shared, now);
                }
            });
            if let Some(rel) = self.selected.clone() {
                let url = self
                    .entries
                    .iter()
                    .find(|e| e.rel == rel)
                    .and_then(|e| e.url.clone());
                chrome::section(ui, "NOTE", Some(&rel), |ui| {
                    if let Some(u) = url {
                        chrome::row(ui, "from", |ui| ui.monospace(u));
                    }
                    chrome::row(ui, "title", |ui| {
                        ui.text_edit_singleline(&mut self.edit_title)
                    });
                    ui.add(
                        egui::TextEdit::multiline(&mut self.edit_body)
                            .desired_rows(8)
                            .desired_width(f32::INFINITY),
                    );
                    let b = mouse_button(ui, "Save edit", &rel);
                    self.save_hit = Some(b.rect);
                    if b.clicked_by(egui::PointerButton::Primary) {
                        self.save_edit(shared, now);
                    }
                    chrome::hint(
                        ui,
                        "Saving makes it your note: she won't merge or prune it.",
                    );
                });
            }
            if let Some(cfg) = &self.cfg {
                chrome::section(ui, "SOURCES", None, |ui| {
                    ui.label(RichText::new(cfg.learn.sources.join(", ")).color(c32(pal::TEXT_DIM)));
                    let blocked = if cfg.learn.block_hosts.is_empty() {
                        "none".to_string()
                    } else {
                        cfg.learn.block_hosts.join(", ")
                    };
                    chrome::row(ui, "blocked hosts", |ui| ui.label(blocked));
                    chrome::hint(
                        ui,
                        "Change these in config.toml ([learn] sources / block_hosts).",
                    );
                });
            }
            chrome::section(ui, "ACTIVITY", None, |ui| {
                if self.activity.is_empty() {
                    chrome::hint(ui, "Nothing learned yet.");
                }
                for l in self.activity.iter().rev() {
                    ui.label(
                        RichText::new(l)
                            .small()
                            .monospace()
                            .color(c32(pal::TEXT_MUTE)),
                    );
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::tests::shared;
    use lillith_learn::store::{Note, NoteStore};

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup(name: &str) -> (Dir, PathBuf, Config) {
        let d = std::env::temp_dir().join(format!("lillith-ktab-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let k = d.join("knowledge");
        let cfg_path = d.join("config.toml");
        std::fs::write(
            &cfg_path,
            format!(
                "[mind]\nknowledge_dir = {:?}\n\n[memory]\ndb_path = {:?}\n",
                k.display().to_string(),
                d.join("memory.db").display().to_string()
            ),
        )
        .unwrap();
        let cfg = Config::load(&cfg_path).unwrap();
        (Dir(d), cfg_path, cfg)
    }

    fn put(cfg: &Config, id: &str, title: &str, t: f64) {
        let dir = cfg.mind.resolved_knowledge_dir().unwrap();
        let st = NoteStore::open(&dir, &cfg.learn.topics, false).unwrap();
        let lock = st.lock().unwrap().unwrap();
        st.write(
            &lock,
            &Note {
                id: id.into(),
                title: title.into(),
                topic: cfg.learn.topics[0].clone(),
                kind: "fact".into(),
                source_type: "man".into(),
                source_ref: "man:ls".into(),
                source_hash: "fnv1a:0".into(),
                created: t,
                updated: t,
                confidence: 0.5,
                quiz_asked: 0,
                quiz_passed: 0,
                links: vec![],
                origin: "learned".into(),
                body: format!("{title} body"),
                web: None,
            },
        )
        .unwrap();
    }

    fn frame(
        ctx: &egui::Context,
        tab: &mut KnowledgeTab,
        s: &mut Shared,
        events: Vec<egui::Event>,
    ) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 1400.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| tab.ui(ui, s));
        });
    }

    fn click(at: egui::Pos2) -> Vec<egui::Event> {
        let b = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![egui::Event::PointerMoved(at), b(true), b(false)]
    }

    fn key_tap(k: egui::Key) -> Vec<egui::Event> {
        let e = |pressed| egui::Event::Key {
            key: k,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        vec![e(true), e(false)]
    }

    #[test]
    fn the_tab_is_registered() {
        let s = shared();
        let r = super::super::TabRegistry::with_defaults(&s);
        assert!(r.index_of("knowledge").is_some(), "{:?}", r.ids());
    }

    #[test]
    fn lists_notes_and_forget_is_mouse_only() {
        let (d, path, cfg) = setup("forget");
        put(&cfg, "n-0000000000a1", "Older", 1.0);
        put(&cfg, "n-0000000000a2", "Newer", 2.0);
        let mut t = KnowledgeTab::new(&path);
        t.audit = Some(d.0.join("audit.log"));
        let mut s = shared();
        let ctx = egui::Context::default();
        lillith_theme::egui_theme::install(&ctx);
        for _ in 0..2 {
            frame(&ctx, &mut t, &mut s, vec![]);
        }
        assert_eq!(t.entries().len(), 2);
        assert_eq!(t.forget_hits.len(), 2);
        for _ in 0..40 {
            frame(&ctx, &mut t, &mut s, key_tap(egui::Key::Tab));
            frame(&ctx, &mut t, &mut s, key_tap(egui::Key::Enter));
            frame(&ctx, &mut t, &mut s, key_tap(egui::Key::Space));
        }
        assert_eq!(t.entries().len(), 2, "a key forgot a note");
        // Newest first: a click on its Forget removes that one only.
        let (rel, r) = t.forget_hits[0].clone();
        assert!(rel.ends_with("n-0000000000a2.md"), "{rel}");
        frame(&ctx, &mut t, &mut s, click(r.center()));
        frame(&ctx, &mut t, &mut s, vec![]);
        assert_eq!(t.entries().len(), 1);
        assert_eq!(t.entries()[0].title, "Older");
        let audit = std::fs::read_to_string(d.0.join("audit.log")).unwrap();
        assert!(audit.contains("\tLEARN\tforget\t"), "{audit}");
    }

    #[test]
    fn editing_makes_it_kos_note_and_off_stops_the_learner() {
        let (d, path, cfg) = setup("edit");
        put(&cfg, "n-0000000000b1", "Draft", 1.0);
        let mut t = KnowledgeTab::new(&path);
        t.marker = Some(d.0.join("run/learn-off"));
        t.audit = Some(d.0.join("audit.log"));
        let mut s = shared();
        t.refresh(0.0);
        let rel = t.entries()[0].rel.clone();
        t.select(&rel);
        t.edit_title = "Mine".into();
        t.edit_body = "Ko's own words.".into();
        t.save_edit(&mut s, 1.0);
        let e = &t.entries()[0];
        assert_eq!((e.title.as_str(), e.origin.as_str()), ("Mine", "ko"));
        assert_eq!(t.status.ko_notes, 1);
        // Off: the config value is staged and the marker stops the learner.
        assert!(t.status.learning);
        t.set_learning(false, &mut s);
        assert_eq!(s.store.get_bool(&["learn", "enabled"]), Some(false));
        assert!(lillith_learn::knowledge::switched_off_at(
            t.marker.as_ref().unwrap()
        ));
        t.set_learning(true, &mut s);
        assert!(!lillith_learn::knowledge::switched_off_at(
            t.marker.as_ref().unwrap()
        ));
    }
}
