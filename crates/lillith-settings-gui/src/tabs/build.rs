//! Build: a local, Replit-style "vibe coding" surface. Scaffold, build and
//! run small Rust / Python / fish projects — but entirely on-box and offline.
//!
//! Nothing here runs on its own. Each button *proposes* a
//! [`PendingAction`] (scaffold / build / run) and raises the shared Soul
//! Contract popup ([`lillith_theme::egui_contract`], the same one the chat
//! bubble uses); the action only executes after its **Yes** (audited
//! `CONFIRM`). **No** (or `Esc`) records a `DENY` in the audit log and
//! discards it. While a build/run is in flight the popup's Yes is disabled
//! (No still works). Every
//! project directory is confined to the projects root
//! ([`lillith_core::config::BuildConfig::resolved_projects_dir`], default
//! `~/Projects/lillith-builds`) by [`lillith_core::build::resolve_project_dir`].

use super::{SettingsTab, Shared};
use crate::chrome;
use crate::task::Task;
use egui::RichText;
use lillith_core::build::{self, Action, Language};
use lillith_core::config::BuildConfig;
use lillith_core::soul_contract::{PendingAction, SoulContract};
use lillith_theme::egui_contract::{soul_contract_popup, ContractChoice, ContractView};
use lillith_theme::egui_theme::c32;
use lillith_theme::palette as pal;
use std::path::PathBuf;

/// Config key: the projects root.
const KEY_PROJECTS_DIR: &[&str] = &["build", "projects_dir"];
/// Config key: the default language.
const KEY_DEFAULT_LANG: &[&str] = &["build", "default_language"];

pub struct BuildTab {
    /// Typed project name/path (relative to the projects root).
    name: String,
    /// Typed projects root (empty = the default).
    root_buf: String,
    language: Language,
    template: String,
    /// The action waiting for a Soul Contract Yes/No, with its preview.
    pending: Option<Proposed>,
    /// A build/run/scaffold in flight.
    running: Option<Task<Result<String, String>>>,
    /// The rolling output / status log.
    log: String,
    /// Where confirmed/denied actions are audited (`<data>/lillith/audit.log`).
    audit: Option<PathBuf>,
}

/// A proposed action plus the human text shown on the gate.
struct Proposed {
    action: PendingAction,
    summary: String,
    preview: String,
}

impl BuildTab {
    pub fn new(shared: &Shared) -> Self {
        let st = &shared.store;
        let root_buf = st.get_str(KEY_PROJECTS_DIR).unwrap_or_default();
        let language = st
            .get_str(KEY_DEFAULT_LANG)
            .and_then(|s| Language::parse(&s))
            .unwrap_or(Language::Rust);
        Self {
            name: String::new(),
            root_buf,
            language,
            template: language.default_template().to_string(),
            pending: None,
            running: None,
            log: String::new(),
            audit: lillith_core::config::data_dir().map(|d| d.join("audit.log")),
        }
    }

    /// The projects root the typed (or default) config resolves to.
    fn projects_root(&self) -> Option<PathBuf> {
        BuildConfig {
            enabled: true,
            projects_dir: self.root_buf.clone(),
            default_language: self.language.as_str().into(),
        }
        .resolved_projects_dir()
    }

    /// Resolves the confined project directory for the typed name.
    fn project_dir(&self) -> Result<PathBuf, String> {
        let root = self
            .projects_root()
            .ok_or_else(|| "no projects root (set $HOME or a projects folder)".to_string())?;
        build::resolve_project_dir(&root, &self.name).map_err(|e| e.to_string())
    }

    /// Builds a proposal for `action_kind`, or logs why it couldn't.
    fn propose(&mut self, action_kind: Action) {
        let dir = match self.project_dir() {
            Ok(d) => d,
            Err(e) => {
                self.log_line(&format!("✕ {e}"));
                return;
            }
        };
        let (action, summary) = match action_kind {
            Action::Scaffold => (
                PendingAction::ScaffoldProject {
                    dir: dir.clone(),
                    language: self.language,
                    template: self.template.clone(),
                },
                build::scaffold_summary(&dir, self.language, &self.template),
            ),
            Action::Build => (
                PendingAction::BuildProject {
                    dir: dir.clone(),
                    language: self.language,
                },
                build::build_summary(&dir, self.language),
            ),
            Action::Run => (
                PendingAction::RunProject {
                    dir: dir.clone(),
                    language: self.language,
                },
                build::run_summary(&dir, self.language),
            ),
        };
        let preview = build::preview(&dir, self.language, action_kind, &self.template);
        self.pending = Some(Proposed {
            action,
            summary,
            preview,
        });
    }

    /// Soul Contract **Yes**: run the proposed action in the background.
    fn confirm(&mut self, ctx: &egui::Context) {
        let Some(p) = self.pending.take() else {
            return;
        };
        self.log_line(&format!("▶ {}", p.summary));
        let action = p.action;
        let audit = self.audit.clone();
        self.running = Some(Task::spawn(ctx, "build-action", move || {
            run_confirmed(action, audit)
        }));
    }

    /// Soul Contract **No**: discard and audit a `DENY`.
    fn deny(&mut self) {
        let Some(p) = self.pending.take() else {
            return;
        };
        let mut sc = SoulContract::new(self.audit.clone());
        let id = sc.request(p.action);
        let _ = sc.deny(id);
        self.log_line(&format!("✕ denied: {}", p.summary));
    }

    /// Opens the project folder in the desktop file manager (view-only, so it
    /// is not a gated mutation). Does nothing destructive.
    fn open_folder(&mut self) {
        let dir = match self.project_dir() {
            Ok(d) => d,
            Err(e) => {
                self.log_line(&format!("✕ {e}"));
                return;
            }
        };
        if !dir.exists() {
            self.log_line(&format!("✕ {} does not exist yet", dir.display()));
            return;
        }
        match std::process::Command::new("xdg-open").arg(&dir).spawn() {
            Ok(_) => self.log_line(&format!("⌂ opened {}", dir.display())),
            Err(e) => self.log_line(&format!("✕ couldn't open folder: {e}")),
        }
    }

    fn log_line(&mut self, line: &str) {
        if !self.log.is_empty() {
            self.log.push('\n');
        }
        self.log.push_str(line);
    }
}

/// Runs a confirmed action through a fresh Soul Contract (audits `CONFIRM`)
/// and returns its captured output. Runs on a worker thread.
fn run_confirmed(action: PendingAction, audit: Option<PathBuf>) -> Result<String, String> {
    let mut sc = SoulContract::new(audit);
    let id = sc.request(action);
    sc.confirm(id)
        .map(|c| c.output.unwrap_or_default())
        .map_err(|e| e.to_string())
}

impl SettingsTab for BuildTab {
    fn id(&self) -> &str {
        "build"
    }
    fn title(&self) -> &str {
        "Build"
    }
    fn rune(&self) -> usize {
        11
    }
    fn busy(&self) -> bool {
        self.running.is_some()
    }

    fn tick(&mut self, _ctx: &egui::Context, _shared: &mut Shared) {
        if let Some(t) = &mut self.running {
            if let Some(r) = t.poll() {
                self.running = None;
                match r {
                    Ok(out) => self.log_line(&out),
                    Err(e) => self.log_line(&format!("✕ {e}")),
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        ui.label(
            RichText::new(
                "Scaffold, build and run small local projects. Everything runs on-box with \
                 already-installed tools (cargo / python3 / fish), offline, and every step \
                 needs a Soul Contract Yes.",
            )
            .color(c32(pal::TEXT_DIM)),
        );
        ui.add_space(6.0);

        chrome::section(ui, "PROJECT", Some("[build]"), |ui| {
            chrome::row(ui, "Name", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.name)
                        .hint_text("my-app")
                        .desired_width(260.0),
                );
            });
            chrome::row(ui, "Language", |ui| {
                let prev = self.language;
                egui::ComboBox::from_id_salt("build-language")
                    .selected_text(self.language.label())
                    .show_ui(ui, |ui| {
                        for l in Language::ALL {
                            if ui.selectable_label(self.language == l, l.label()).clicked() {
                                self.language = l;
                            }
                        }
                    });
                if self.language != prev {
                    // Keep the template valid for the new language.
                    if !self.language.has_template(&self.template) {
                        self.template = self.language.default_template().to_string();
                    }
                    shared
                        .store
                        .set_str(KEY_DEFAULT_LANG, self.language.as_str());
                }
            });
            chrome::row(ui, "Template", |ui| {
                egui::ComboBox::from_id_salt("build-template")
                    .selected_text(self.template.clone())
                    .show_ui(ui, |ui| {
                        for t in self.language.templates() {
                            if ui.selectable_label(self.template == *t, *t).clicked() {
                                self.template = (*t).to_string();
                            }
                        }
                    });
            });
            chrome::row(ui, "Projects root", |ui| {
                let hint = self
                    .projects_root()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "~/Projects/lillith-builds".into());
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.root_buf)
                            .hint_text(hint)
                            .desired_width(320.0),
                    )
                    .changed()
                {
                    shared.store.set_str(KEY_PROJECTS_DIR, self.root_buf.trim());
                }
            });
            ui.horizontal(|ui| {
                ui.add_space(142.0);
                chrome::hint(
                    ui,
                    "Projects are confined to this folder; add it to [mind] allow_paths so \
                     Lillith can read it in chat too.",
                );
            });
        });

        chrome::section(ui, "ACTIONS", Some("Soul Contract gated"), |ui| {
            ui.horizontal_wrapped(|ui| {
                if chrome::stone_button(ui, "✦ New project").clicked() {
                    self.propose(Action::Scaffold);
                }
                if chrome::stone_button(ui, "⚙ Build").clicked() {
                    self.propose(Action::Build);
                }
                if chrome::stone_button(ui, "▶ Run").clicked() {
                    self.propose(Action::Run);
                }
                if chrome::stone_button(ui, "⌂ Open folder").clicked() {
                    self.open_folder();
                }
            });
            if self.busy() {
                chrome::hint(ui, "running… output lands in the log below");
            }
        });

        if let Some(p) = &self.pending {
            let prompt = p.action.prompt().with_detail(p.preview.clone());
            let view = ContractView {
                title: prompt.title,
                summary: &p.summary,
                target: prompt.target.as_deref(),
                detail: prompt.detail.as_deref(),
                busy: self.busy(),
            };
            let choice = soul_contract_popup(ui.ctx(), egui::Id::new("build-soul-contract"), &view);
            match choice {
                // The popup disables Yes while busy; check again anyway.
                Some(ContractChoice::Yes) if !self.busy() => self.confirm(ui.ctx()),
                Some(ContractChoice::No) => self.deny(),
                _ => {}
            }
        }

        chrome::section(ui, "LOG", Some("local only"), |ui| {
            if self.log.is_empty() {
                chrome::hint(ui, "no output yet");
            } else {
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        chrome::code_block(ui, &self.log);
                    });
                if chrome::stone_button(ui, "clear log").clicked() {
                    self.log.clear();
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabs::tests::shared;

    #[test]
    fn new_seeds_from_config_store() {
        let mut s = shared();
        s.store.set_str(KEY_DEFAULT_LANG, "python");
        s.store
            .set_str(KEY_PROJECTS_DIR, "/tmp/lillith-builds-test");
        let tab = BuildTab::new(&s);
        assert_eq!(tab.language, Language::Python);
        assert_eq!(tab.template, "script");
        assert_eq!(tab.root_buf, "/tmp/lillith-builds-test");
        assert_eq!(tab.id(), "build");
    }

    #[test]
    fn propose_scaffold_sets_pending_gate() {
        let s = shared();
        let mut tab = BuildTab::new(&s);
        tab.root_buf = "/tmp/lillith-builds-xyz".into();
        tab.name = "demo".into();
        tab.language = Language::Rust;
        tab.template = "bin".into();
        tab.propose(Action::Scaffold);
        let p = tab.pending.as_ref().expect("a gate was proposed");
        assert!(matches!(p.action, PendingAction::ScaffoldProject { .. }));
        assert!(p.action.needs_confirmation());
        assert!(p.preview.contains("--offline"), "{}", p.preview);
        // Nothing has run or written yet.
        assert!(tab.running.is_none());
        assert!(!std::path::Path::new("/tmp/lillith-builds-xyz/demo").exists());
    }

    #[test]
    fn propose_rejects_escape_from_root() {
        let s = shared();
        let mut tab = BuildTab::new(&s);
        tab.root_buf = "/tmp/lillith-builds-abc".into();
        tab.name = "../../etc".into();
        tab.propose(Action::Scaffold);
        assert!(tab.pending.is_none(), "must not propose outside the root");
        assert!(tab.log.contains("✕"), "an error was logged: {}", tab.log);
    }

    #[test]
    fn deny_clears_pending_without_running() {
        let s = shared();
        let mut tab = BuildTab::new(&s);
        tab.root_buf = "/tmp/lillith-builds-deny".into();
        tab.name = "proj".into();
        tab.propose(Action::Build);
        assert!(tab.pending.is_some());
        tab.deny();
        assert!(tab.pending.is_none());
        assert!(tab.log.contains("denied"), "{}", tab.log);
    }

    /// Draws the tab once (headless) with `events`.
    fn frame(ctx: &egui::Context, tab: &mut BuildTab, s: &mut Shared, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 700.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| tab.ui(ui, s));
        });
    }

    #[test]
    fn pending_raises_popup_and_escape_denies_with_audit() {
        let mut s = shared();
        let mut tab = BuildTab::new(&s);
        let dir = std::env::temp_dir().join(format!("lillith-build-popup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let audit = dir.join("audit.log");
        let _ = std::fs::remove_file(&audit);
        tab.audit = Some(audit.clone());
        tab.root_buf = dir.join("root").display().to_string();
        tab.name = "proj".into();
        tab.propose(Action::Run);
        let ctx = egui::Context::default();
        lillith_theme::egui_theme::install(&ctx);
        for _ in 0..3 {
            frame(&ctx, &mut tab, &mut s, vec![]);
        }
        // The popup is up and nothing has been decided or run yet.
        assert!(ctx
            .read_response(egui::Id::new("build-soul-contract").with("yes"))
            .is_some());
        assert!(tab.pending.is_some());
        assert!(tab.running.is_none());
        let esc = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, &mut tab, &mut s, vec![esc]);
        assert!(tab.pending.is_none());
        assert!(tab.running.is_none(), "No must never run the action");
        assert!(tab.log.contains("denied"), "{}", tab.log);
        let log = std::fs::read_to_string(&audit).unwrap_or_default();
        assert!(log.contains("DENY") && log.contains("run_project"), "{log}");
        assert!(!log.contains("CONFIRM"), "{log}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
