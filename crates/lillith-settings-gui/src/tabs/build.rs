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
#[cfg(test)]
use lillith_theme::egui_contract::summary_line;
use lillith_theme::egui_contract::{soul_contract_popup, ContractChoice, ContractView, Initiator};
use lillith_theme::egui_theme::c32;
use lillith_theme::palette as pal;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

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
    /// Output lines of the in-flight action, streamed while it runs
    /// (drained into `log` every frame).
    stream: Option<Receiver<String>>,
    /// Whether any line was streamed for the in-flight action (then only
    /// the `[exit N]` tail of the final output is added).
    streamed: bool,
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
            stream: None,
            streamed: false,
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
        let (tx, rx) = channel::<String>();
        let repaint = ctx.clone();
        self.stream = Some(rx);
        self.streamed = false;
        self.running = Some(Task::spawn(ctx, "build-action", move || {
            run_confirmed(action, audit, &mut |line: &str| {
                let _ = tx.send(line.to_string());
                repaint.request_repaint();
            })
        }));
    }

    /// Calls `f` with the Soul Contract view for the pending action, if any.
    fn with_view<R>(&self, f: impl FnOnce(&ContractView<'_>) -> R) -> Option<R> {
        let p = self.pending.as_ref()?;
        let prompt = p.action.prompt().with_detail(p.preview.clone());
        let view = ContractView {
            title: prompt.title,
            summary: &p.summary,
            target: prompt.target.as_deref(),
            detail: prompt.detail.as_deref(),
            busy: self.busy(),
            // Every Build tab action comes from Ko's own button press.
            initiator: Initiator::You,
            scroll_detail: prompt.scrolls(),
        };
        Some(f(&view))
    }

    /// Moves streamed output lines into the log.
    fn drain_stream(&mut self) {
        let lines: Vec<String> = match &self.stream {
            Some(rx) => rx.try_iter().collect(),
            None => return,
        };
        if !lines.is_empty() {
            self.streamed = true;
        }
        for l in lines {
            self.log_line(&l);
        }
    }

    /// Logs the finished action's result. When its output was already
    /// streamed line by line, only the closing `[exit N]` line is added.
    fn finish(&mut self, r: Result<String, String>) {
        self.drain_stream();
        self.stream = None;
        match r {
            Ok(out) if self.streamed => {
                if let Some(tail) = out.lines().last().filter(|l| l.starts_with("[exit ")) {
                    self.log_line(tail);
                }
            }
            Ok(out) => self.log_line(&out),
            Err(e) => self.log_line(&format!("✕ {e}")),
        }
        self.streamed = false;
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
        match open_dir_command(&dir, lillith_core::config::audit_log_path()).spawn() {
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

/// `xdg-open <dir>` for "Open folder", after its one plain audit line
/// (`OPEN_DIR_UNGATED`, no card: Ko exempted it). Not spawned here.
fn open_dir_command(dir: &std::path::Path, audit: Option<PathBuf>) -> std::process::Command {
    use lillith_core::soul_contract as sc;
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(dir);
    SoulContract::new(audit).record_ungated_launch_as(
        sc::OPEN_DIR_UNGATED_TAG,
        sc::OPEN_PATH_KIND,
        &sc::command_argv(&cmd),
    );
    cmd
}

/// Runs a confirmed action through a fresh Soul Contract (audits `CONFIRM`)
/// and returns its captured output, handing build/run output lines to
/// `sink` as they arrive. Runs on a worker thread.
fn run_confirmed(
    action: PendingAction,
    audit: Option<PathBuf>,
    sink: &mut dyn FnMut(&str),
) -> Result<String, String> {
    let mut sc = SoulContract::new(audit);
    let id = sc.request(action);
    sc.confirm_streaming(id, sink)
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
        self.drain_stream();
        if let Some(t) = &mut self.running {
            if let Some(r) = t.poll() {
                self.running = None;
                self.finish(r);
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
                chrome::hint(ui, "running… output streams into the log below");
            }
        });

        let busy = self.busy();
        let choice = self.with_view(|view| {
            soul_contract_popup(ui.ctx(), egui::Id::new("build-soul-contract"), view)
        });
        match choice.flatten() {
            // The popup disables Yes while busy; check again anyway.
            Some(ContractChoice::Yes) if !busy => self.confirm(ui.ctx()),
            Some(ContractChoice::No) => self.deny(),
            _ => {}
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

    #[test]
    fn open_folder_leaves_one_plain_audit_line() {
        let dir = std::env::temp_dir().join(format!("lillith-open-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = dir.join("audit.log");
        let c = open_dir_command(std::path::Path::new("/p/proj"), Some(log.clone()));
        assert_eq!(c.get_program(), "xdg-open");
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(
            text.contains("\tOPEN_DIR_UNGATED\topen_path\txdg-open\tno-card\t$ xdg-open /p/proj"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
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

    #[test]
    fn contract_wording_says_you_asked() {
        let s = shared();
        let mut tab = BuildTab::new(&s);
        tab.root_buf = "/tmp/lillith-builds-wording".into();
        tab.name = "proj".into();
        tab.propose(Action::Build);
        let line = tab.with_view(summary_line).expect("pending");
        assert!(line.starts_with("You asked to "), "{line}");
        assert!(!line.contains("She wants"), "{line}");
    }

    #[test]
    fn run_output_streams_into_log_while_running() {
        let s = shared();
        let mut sh = shared();
        let mut tab = BuildTab::new(&s);
        let dir = std::env::temp_dir().join(format!("lillith-build-stream-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let proj = dir.join("root").join("streamy");
        std::fs::create_dir_all(&proj).unwrap();
        // Not flushed on purpose: the Python runner must not buffer.
        std::fs::write(
            proj.join("main.py"),
            "import sys, time\nprint('first line')\nprint('to stderr', file=sys.stderr)\ntime.sleep(1.5)\nprint('last line')\n",
        )
        .unwrap();
        let audit = dir.join("audit.log");
        tab.audit = Some(audit.clone());
        tab.root_buf = dir.join("root").display().to_string();
        tab.name = "streamy".into();
        tab.language = Language::Python;
        tab.propose(Action::Run);
        let ctx = egui::Context::default();
        tab.confirm(&ctx);
        assert!(tab.busy());
        let t0 = std::time::Instant::now();
        let mut seen_while_running = false;
        while tab.busy() && t0.elapsed() < std::time::Duration::from_secs(20) {
            tab.tick(&ctx, &mut sh);
            // Both early lines are in the log while she is still asleep
            // (before "last line" was even printed).
            if tab.busy()
                && tab.log.contains("first line")
                && tab.log.contains("to stderr")
                && !tab.log.contains("last line")
            {
                seen_while_running = true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!tab.busy(), "the run finished");
        assert!(
            seen_while_running,
            "lines showed up before the end: {}",
            tab.log
        );
        assert!(
            tab.log.contains("last line") && tab.log.ends_with("[exit 0]"),
            "{}",
            tab.log
        );
        assert_eq!(tab.log.matches("first line").count(), 1, "{}", tab.log);
        let log = std::fs::read_to_string(&audit).unwrap_or_default();
        assert!(
            log.contains("CONFIRM") && log.contains("run_project"),
            "{log}"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
