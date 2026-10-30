//! Chat / Brain: Lily's local LLM, as the first-run setup wizard
//! (Avatar ✔ › Runtime › Pick model › Persona › Test & save) on top of
//! `lillith-setup`.

use super::{SettingsTab, Shared};
use crate::chrome;
use crate::task::Task;
use egui::{pos2, vec2, Align2, RichText, Sense};
use lillith_core::soul_contract::{
    MODEL_SERVER_KIND, OLLAMA_CREATE_KIND, OLLAMA_CREATE_UNGATED_TAG, SETTINGS_UNGATED_TAG,
};
use lillith_setup::check::{self, ServerHealth};
use lillith_setup::config::{self as llm, Backend, LlmConfig};
use lillith_setup::modelfile::{
    lily_modelfile, Instruction, LilyParams, Modelfile, LILY_SYSTEM_PROMPT,
};
use lillith_setup::scan::{
    self, ModelInfo, Recommendation, ScanHandle, ScanOptions, ScanReport, Tier,
};
use lillith_setup::template::ChatTemplate;
use lillith_setup::validate::{self, ValidateCtx};
use lillith_setup::{launch, paths, wizard};
use lillith_theme::egui_theme::{self as th, c32};
use lillith_quantumsec::consent::AwakeningState;
use lillith_theme::geom;
use lillith_theme::palette::{self as pal, Rgba};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Avatar,
    Runtime,
    PickModel,
    Persona,
    TestSave,
    /// QuantumSec / DemonOS shield intro (additive; nothing removed).
    QuantumSec,
}

impl Step {
    pub const ALL: [Step; 6] = [
        Step::Avatar,
        Step::Runtime,
        Step::PickModel,
        Step::Persona,
        Step::TestSave,
        Step::QuantumSec,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Step::Avatar => "Avatar",
            Step::Runtime => "Runtime",
            Step::PickModel => "Pick model",
            Step::Persona => "Persona",
            Step::TestSave => "Test & save",
            Step::QuantumSec => "QuantumSec",
        }
    }
    pub fn from_name(s: &str) -> Option<Self> {
        let k: String = s
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        Some(match k.as_str() {
            "avatar" => Step::Avatar,
            "runtime" => Step::Runtime,
            "pickmodel" | "model" | "pick" => Step::PickModel,
            "persona" | "modelfile" => Step::Persona,
            "testsave" | "test" | "save" => Step::TestSave,
            "quantumsec" | "qsec" | "phantomcore" | "demonos" => Step::QuantumSec,
            _ => return None,
        })
    }
    fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// Rounds `[llm]` floats for the file (temperature to 2 places).
pub fn rounded(cfg: &LlmConfig) -> LlmConfig {
    LlmConfig {
        temperature: (cfg.temperature * 100.0).round() / 100.0,
        ..cfg.clone()
    }
}

pub struct BrainTab {
    config_path: PathBuf,
    cfg: LlmConfig,
    saved: LlmConfig,
    load_error: Option<String>,
    /// Names the `[chat.server] extra_args` llama-server would refuse (the
    /// start is refused and logged as `ARG_REFUSED`), so nothing is
    /// dropped silently. Re-read on load, save and revert.
    args_notice: Option<String>,
    step: Step,
    // scan
    scan: Option<ScanHandle>,
    scan_status: String,
    report: Option<ScanReport>,
    recs: Vec<Recommendation>,
    selected: Option<usize>,
    manual_path: String,
    new_dir: String,
    browse: Option<Task<Option<PathBuf>>>,
    // modelfile
    mf_path: PathBuf,
    mf: Modelfile,
    mf_system: String,
    mf_template: String,
    mf_params: Vec<(String, String)>,
    mf_dirty: bool,
    mf_note: Option<String>,
    // trials
    ping: Option<Task<Result<ServerHealth, String>>>,
    ping_result: Option<Result<ServerHealth, String>>,
    chat: Option<Task<Result<String, String>>>,
    chat_result: Option<Result<String, String>>,
    create: Option<Task<Result<String, String>>>,
    create_result: Option<Result<String, String>>,
    server: Option<std::process::Child>,
    server_note: Option<String>,
}

fn params_of(m: &Modelfile) -> Vec<(String, String)> {
    m.instructions
        .iter()
        .filter_map(|i| match i {
            Instruction::Parameter { key, value } => Some((key.clone(), value.clone())),
            _ => None,
        })
        .collect()
}

/// The notice for refused `[chat.server] extra_args` in the config file
/// at `path` (`None` when every arg passes or the file can't be read).
fn extra_args_notice(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    lillith_setup::extra_args::notice(&lillith_setup::extra_args::from_config_toml(&text))
}

fn gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / 1e9)
}

impl BrainTab {
    pub fn new(config_path: &Path) -> Self {
        let (cfg, load_error) = match llm::load(config_path) {
            Ok(c) => (c, None),
            Err(e) => (LlmConfig::default(), Some(e.to_string())),
        };
        let mf_path = if cfg.modelfile_path.trim().is_empty() {
            paths::default_modelfile_path().unwrap_or_else(|| PathBuf::from("Modelfile.lily"))
        } else {
            paths::expand_tilde(&cfg.modelfile_path)
        };
        let (mf, mf_note) = match Modelfile::from_file(&mf_path) {
            Ok(Ok(m)) => (m, None),
            Ok(Err(e)) => (
                Self::default_modelfile(&cfg),
                Some(format!(
                    "{}: {e} — showing Lily's default",
                    mf_path.display()
                )),
            ),
            Err(_) => (Self::default_modelfile(&cfg), None),
        };
        let mut t = Self {
            config_path: config_path.to_path_buf(),
            saved: cfg.clone(),
            cfg,
            load_error,
            args_notice: extra_args_notice(config_path),
            step: Step::Runtime,
            scan: None,
            scan_status: String::new(),
            report: None,
            recs: Vec::new(),
            selected: None,
            manual_path: String::new(),
            new_dir: String::new(),
            browse: None,
            mf_path,
            mf_system: String::new(),
            mf_template: String::new(),
            mf_params: Vec::new(),
            mf: Modelfile {
                instructions: Vec::new(),
            },
            mf_dirty: false,
            mf_note,
            ping: None,
            ping_result: None,
            chat: None,
            chat_result: None,
            create: None,
            create_result: None,
            server: None,
            server_note: None,
        };
        t.set_modelfile(mf, false);
        t.manual_path = t.cfg.gguf_path.clone();
        t.start_scan();
        t
    }

    fn default_modelfile(cfg: &LlmConfig) -> Modelfile {
        let p = LilyParams {
            template: ChatTemplate::from_name(&cfg.chat_template).unwrap_or(ChatTemplate::ChatMl),
            temperature: cfg.temperature,
            num_ctx: cfg.ctx_size,
            num_thread: cfg.threads,
            system_prompt: if cfg.system_prompt.trim().is_empty() {
                LILY_SYSTEM_PROMPT.into()
            } else {
                cfg.system_prompt.clone()
            },
            ..LilyParams::default()
        };
        let from = if cfg.gguf_path.is_empty() {
            "./model.gguf"
        } else {
            &cfg.gguf_path
        };
        lily_modelfile(from, &p)
    }

    fn set_modelfile(&mut self, m: Modelfile, dirty: bool) {
        self.mf_system = m.system().unwrap_or("").to_string();
        self.mf_template = m.template().unwrap_or("").to_string();
        self.mf_params = params_of(&m);
        self.mf = m;
        self.mf_dirty = dirty;
    }

    /// The Modelfile as edited.
    pub fn composed_modelfile(&self) -> Modelfile {
        let mut m = self.mf.clone();
        if !self.cfg.gguf_path.trim().is_empty() {
            m.set_from(self.cfg.gguf_path.trim());
        }
        m.set_system(self.mf_system.clone());
        if !self.mf_template.trim().is_empty() {
            m.set_template(self.mf_template.clone());
        }
        for (k, _) in params_of(&self.mf) {
            m.remove_param(&k);
        }
        let mut keys: Vec<String> = Vec::new();
        for (k, _) in &self.mf_params {
            let k = k.trim().to_ascii_lowercase();
            if !k.is_empty() && !keys.contains(&k) {
                keys.push(k);
            }
        }
        for k in keys {
            let vals: Vec<&str> = self
                .mf_params
                .iter()
                .filter(|(kk, _)| kk.trim().eq_ignore_ascii_case(&k))
                .map(|(_, v)| v.trim())
                .collect();
            m.set_param_values(&k, &vals);
        }
        m
    }

    fn sync_params_from_cfg(&mut self) {
        let t = format!("{}", (self.cfg.temperature * 100.0).round() / 100.0);
        let c = self.cfg.ctx_size.to_string();
        for (k, v) in &mut self.mf_params {
            match k.as_str() {
                "temperature" if *v != t => {
                    *v = t.clone();
                    self.mf_dirty = true;
                }
                "num_ctx" if *v != c => {
                    *v = c.clone();
                    self.mf_dirty = true;
                }
                _ => {}
            }
        }
    }

    fn scan_options(&self) -> ScanOptions {
        let mut o = ScanOptions::default();
        for p in &self.cfg.scan_paths {
            let p = paths::expand_tilde(p);
            if !o.dirs.contains(&p) {
                o.dirs.insert(0, p);
            }
        }
        o
    }

    fn start_scan(&mut self) {
        if let Some(h) = &self.scan {
            h.cancel();
        }
        self.scan = Some(scan::spawn_scan(self.scan_options()));
        self.scan_status = "scanning…".into();
    }

    fn pick_model(&mut self, i: usize) {
        let Some(m) = self.report.as_ref().and_then(|r| r.models.get(i)).cloned() else {
            return;
        };
        self.selected = Some(i);
        self.apply_model(&m);
    }

    fn apply_model(&mut self, m: &ModelInfo) {
        let base = wizard::config_for_model(m, self.cfg.backend);
        self.cfg.gguf_path = base.gguf_path;
        self.cfg.ctx_size = base.ctx_size;
        self.cfg.chat_template = base.chat_template;
        self.manual_path = self.cfg.gguf_path.clone();
        let fresh = wizard::modelfile_for(m, &self.cfg);
        let keep_system = self.mf_dirty && !self.mf_system.trim().is_empty();
        let sys = self.mf_system.clone();
        self.set_modelfile(fresh, true);
        if keep_system {
            self.mf_system = sys;
        }
    }

    fn ctx_validate(&self) -> ValidateCtx {
        let model_ctx = self
            .selected
            .and_then(|i| self.report.as_ref()?.models.get(i)?.context_length);
        ValidateCtx::new()
            .with_backend(self.cfg.backend)
            .with_model_ctx(model_ctx)
    }

    fn check(&self, key: &str, value: &str) -> Result<(), String> {
        validate::field(key, value, &self.ctx_validate()).map_err(|e| e.message)
    }

    fn step_done(&self, s: Step, shared: &Shared) -> bool {
        match s {
            Step::Avatar => true,
            Step::Runtime => shared
                .runtime
                .as_ref()
                .is_some_and(|r| match self.cfg.backend {
                    Backend::LlamaServer => {
                        r.llama_server.is_some() || r.llama_server_api.reachable
                    }
                    Backend::Ollama => r.ollama.is_some() || r.ollama_api.reachable,
                }),
            Step::PickModel => !self.cfg.gguf_path.trim().is_empty(),
            Step::Persona => self.mf_path.exists() && !self.mf_dirty,
            Step::TestSave => matches!(&self.ping_result, Some(Ok(h)) if h.ready),
            Step::QuantumSec => true,
        }
    }

    // ------------------------------------------------------------------
    // drawing
    // ------------------------------------------------------------------

    fn steps_row(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        let avail = ui.available_width();
        let compact = avail < 700.0;
        let (r, _) = ui.allocate_exact_size(vec2(avail, 40.0), Sense::hover());
        let n = Step::ALL.len() as f32;
        let gap = 18.0;
        let w = (r.width() - gap * (n - 1.0)) / n;
        let p = ui.painter().clone();
        for (i, s) in Step::ALL.iter().enumerate() {
            let x0 = r.left() + i as f32 * (w + gap);
            let rect = egui::Rect::from_min_size(pos2(x0, r.top() + 4.0), vec2(w, 32.0));
            let resp = ui
                .interact(rect, ui.id().with(("step", i)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            let cur = *s == self.step;
            let done = self.step_done(*s, shared);
            let hover = ui.ctx().animate_bool(resp.id, resp.hovered());
            let poly =
                geom::chamfer_rect([rect.left(), rect.top(), rect.right(), rect.bottom()], 8.0);
            th::fill_convex(
                &p,
                &poly,
                c32(if cur {
                    Rgba::hex(0x2a0510)
                } else {
                    Rgba::hex(0x0f070a)
                }),
            );
            if cur {
                th::knife_highlight(&p, rect.shrink2(vec2(3.0, 4.0)), 1.0);
                th::neon_line(&p, &poly, th::accent().base, 1.2, 4.0, true);
            } else {
                th::knife_highlight(&p, rect.shrink2(vec2(3.0, 4.0)), hover * 0.5);
                p.add(egui::Shape::closed_line(
                    poly.iter().map(|q| pos2(q[0], q[1])).collect(),
                    egui::Stroke::new(1.0, c32(pal::IRON_HI)),
                ));
            }
            // number / check medallion
            let mc = pos2(rect.left() + 17.0, rect.center().y);
            let mcol = if cur {
                th::accent().base
            } else if done {
                pal::OK
            } else {
                pal::IRON_EDGE
            };
            let dia = [
                [mc.x, mc.y - 10.0],
                [mc.x + 10.0, mc.y],
                [mc.x, mc.y + 10.0],
                [mc.x - 10.0, mc.y],
            ];
            th::fill_convex(&p, &dia, c32(Rgba::hex(0x0a0306)));
            th::neon_line(
                &p,
                &dia,
                mcol,
                1.2,
                if cur || done { 3.0 } else { 0.5 },
                true,
            );
            let mark = if done && !cur {
                "✔".to_string()
            } else {
                (i + 1).to_string()
            };
            p.text(
                mc,
                Align2::CENTER_CENTER,
                mark,
                th::bold_font(12.0),
                c32(mcol),
            );
            if !compact || cur {
                let label = if compact {
                    s.label().split(' ').next().unwrap_or("")
                } else {
                    s.label()
                };
                p.text(
                    pos2(rect.left() + 33.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    th::bold_font(14.0),
                    c32(if cur {
                        Rgba::hex(0xffffff)
                    } else {
                        pal::TEXT_DIM
                    }),
                );
            }
            if i + 1 < Step::ALL.len() {
                p.text(
                    pos2(rect.right() + gap * 0.5, rect.center().y),
                    Align2::CENTER_CENTER,
                    "›",
                    th::carved_font(18.0),
                    c32(pal::BLOOD_NEON),
                );
            }
            if resp.clicked() {
                self.step = *s;
            }
        }
    }

    fn runtime_ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let wide = ui.available_width() > 760.0;
        let left = |ui: &mut egui::Ui, this: &mut Self, shared: &mut Shared| {
            chrome::section(ui, "RUNTIME_DETECT", Some("loopback probes"), |ui| {
                match &shared.runtime {
                    None => {
                        chrome::label_os(ui, "PROBING THE VOID…", pal::WARN);
                    }
                    Some(r) => {
                        let bin = |ui: &mut egui::Ui, name: &str, b: &Option<lillith_setup::detect::BinaryInfo>| {
                            chrome::row(ui, name, |ui| match b {
                                Some(b) => {
                                    chrome::chip(ui, "FOUND", pal::OK, true);
                                    ui.label(RichText::new(format!("{}  {}", b.path.display(), b.version.clone().unwrap_or_default())).monospace().size(11.5).color(c32(pal::TEXT_DIM)));
                                }
                                None => {
                                    chrome::chip(ui, "NOT FOUND", pal::TEXT_MUTE, false);
                                }
                            });
                        };
                        bin(ui, "llama-server", &r.llama_server);
                        bin(ui, "llama-cli", &r.llama_cli);
                        bin(ui, "ollama", &r.ollama);
                        chrome::row(ui, "server api", |ui| {
                            if r.llama_server_api.reachable {
                                chrome::chip(
                                    ui,
                                    &format!("ONLINE {}", r.llama_server_api.url),
                                    pal::OK,
                                    true,
                                );
                            } else {
                                chrome::chip(
                                    ui,
                                    &format!("OFFLINE {}", r.llama_server_api.url),
                                    pal::BLOOD_NEON,
                                    true,
                                );
                            }
                        });
                        chrome::row(ui, "ollama api", |ui| {
                            if r.ollama_api.reachable {
                                chrome::chip(
                                    ui,
                                    &format!("ONLINE · {} models", r.ollama_api.models.len()),
                                    pal::OK,
                                    true,
                                );
                            } else {
                                chrome::chip(ui, "OFFLINE", pal::TEXT_MUTE, true);
                            }
                        });
                    }
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let label = if shared.detecting {
                        "probing…"
                    } else {
                        "⟳ Re-detect"
                    };
                    if chrome::stone_button(ui, label).clicked() && !shared.detecting {
                        shared.want_redetect = true;
                    }
                    chrome::hint(
                        ui,
                        "binaries on PATH + llama.cpp dirs; APIs on 127.0.0.1 only",
                    );
                });
            });
            chrome::section(ui, "BACKEND", Some("[llm] backend"), |ui| {
                ui.horizontal(|ui| {
                    for (b, rune) in [(Backend::LlamaServer, 8), (Backend::Ollama, 15)] {
                        if chrome::choice(ui, b.as_str(), this.cfg.backend == b, rune).clicked() {
                            this.cfg.backend = b;
                        }
                    }
                });
                if let Some(s) = shared.runtime.as_ref().and_then(|r| r.suggested_backend()) {
                    chrome::hint(ui, &format!("detected: {} looks ready", s.as_str()));
                }
            });
        };
        let right = |ui: &mut egui::Ui, this: &mut Self| {
            chrome::section(ui, "CONNECTION", Some("loopback only"), |ui| {
                chrome::row(ui, "server_url", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut this.cfg.server_url).desired_width(220.0),
                    )
                });
                let v = this.check("server_url", &this.cfg.server_url);
                ui.horizontal(|ui| {
                    ui.add_space(142.0);
                    chrome::validity(ui, &v, "LOOPBACK ONLY");
                });
                chrome::row(ui, "ollama_host", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut this.cfg.ollama_host).desired_width(220.0),
                    )
                });
                let v = this.check("ollama_host", &this.cfg.ollama_host);
                ui.horizontal(|ui| {
                    ui.add_space(142.0);
                    chrome::validity(ui, &v, "LOOPBACK ONLY");
                });
                chrome::row(ui, "llama_server_bin", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut this.cfg.llama_server_bin)
                            .hint_text("(PATH)")
                            .desired_width(220.0),
                    )
                });
                if !this.cfg.llama_server_bin.trim().is_empty() {
                    let v = this.check("llama_server_bin", &this.cfg.llama_server_bin);
                    if let Err(e) = v {
                        ui.label(
                            RichText::new(format!("⚠ {e}"))
                                .small()
                                .color(c32(pal::BLOOD_SHEEN)),
                        );
                    }
                }
                chrome::row(ui, "threads", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut this.cfg.threads)
                            .range(0..=256)
                            .suffix(" (0 = auto)"),
                    )
                });
            });
        };
        if wide {
            ui.columns(2, |c| {
                left(&mut c[0], self, shared);
                right(&mut c[1], self);
            });
        } else {
            left(ui, self, shared);
            right(ui, self);
        }
    }

    fn model_table(&mut self, ui: &mut egui::Ui) {
        let Some(report) = &self.report else {
            chrome::label_os(ui, &self.scan_status.to_uppercase(), pal::WARN);
            return;
        };
        if report.models.is_empty() {
            ui.label(
                RichText::new("No GGUF models found. Add a scan directory or Browse… to one.")
                    .color(c32(pal::TEXT_DIM)),
            );
            return;
        }
        let tier_of = |i: usize| self.recs.iter().find(|r| r.index == i).map(|r| r.tier);
        let mut order: Vec<usize> = self.recs.iter().map(|r| r.index).collect();
        for i in 0..report.models.len() {
            if !order.contains(&i) {
                order.push(i);
            }
        }
        let w = ui.available_width();
        let cols = [0.0, 0.52, 0.66, 0.80, 0.90];
        let head = ["MODEL", "SIZE", "QUANT", "CTX", "RAM"];
        let (hr, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        for (c, h) in cols.iter().zip(head) {
            ui.painter().text(
                pos2(hr.left() + 22.0 + c * (w - 22.0), hr.center().y),
                Align2::LEFT_CENTER,
                h,
                th::display_font(9.5),
                c32(pal::TEXT_MUTE),
            );
        }
        let mut clicked = None;
        for i in order {
            let m = &report.models[i];
            let (r, resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let sel = self.cfg.gguf_path == m.path.display().to_string();
            let hover = ui.ctx().animate_bool(resp.id, resp.hovered());
            let p = ui.painter();
            if sel {
                th::knife_highlight(p, r, 1.0);
            } else {
                th::knife_highlight(p, r, hover * 0.5);
                p.line_segment(
                    [r.left_bottom(), r.right_bottom()],
                    egui::Stroke::new(1.0, c32(pal::BLOOD_DRIED)),
                );
            }
            let (mark, mc) = match tier_of(i) {
                Some(Tier::Recommended) => ("★", pal::OK),
                Some(Tier::Ok) => ("◆", pal::TEXT_DIM),
                Some(Tier::NotRecommended) => ("⚠", pal::WARN),
                None => ("·", pal::TEXT_MUTE),
            };
            p.text(
                pos2(r.left() + 10.0, r.center().y),
                Align2::CENTER_CENTER,
                mark,
                egui::FontId::proportional(13.0),
                c32(mc),
            );
            let tc = if sel { Rgba::hex(0xffffff) } else { pal::TEXT };
            let cell = |k: usize| pos2(r.left() + 22.0 + cols[k] * (w - 22.0), r.center().y);
            let name_w = (cols[1] - cols[0]) * (w - 22.0) - 8.0;
            let name = if m.file_name.is_empty() {
                m.name.clone()
            } else {
                m.file_name.clone()
            };
            let g = p.layout(name, egui::FontId::monospace(12.0), c32(tc), f32::INFINITY);
            let clip = p.with_clip_rect(egui::Rect::from_min_size(
                pos2(cell(0).x, r.top()),
                vec2(name_w, r.height()),
            ));
            clip.galley(pos2(cell(0).x, r.center().y - g.size().y * 0.5), g, c32(tc));
            p.text(
                cell(1),
                Align2::LEFT_CENTER,
                gb(m.size_bytes),
                egui::FontId::monospace(12.0),
                c32(tc),
            );
            let qc = if tier_of(i) == Some(Tier::NotRecommended) {
                pal::WARN
            } else {
                tc
            };
            p.text(
                cell(2),
                Align2::LEFT_CENTER,
                m.quant.clone().unwrap_or_else(|| "?".into()),
                egui::FontId::monospace(12.0),
                c32(qc),
            );
            let ctx = m
                .context_length
                .map(|c| {
                    if c >= 1024 {
                        format!("{}k", c / 1024)
                    } else {
                        c.to_string()
                    }
                })
                .unwrap_or_else(|| "?".into());
            p.text(
                cell(3),
                Align2::LEFT_CENTER,
                ctx,
                egui::FontId::monospace(12.0),
                c32(tc),
            );
            p.text(
                cell(4),
                Align2::LEFT_CENTER,
                gb(m.est_ram_bytes),
                egui::FontId::monospace(12.0),
                c32(tc),
            );
            let tip = self
                .recs
                .iter()
                .find(|r| r.index == i)
                .map(|r| r.reasons.join("\n"))
                .unwrap_or_default();
            let resp = if tip.is_empty() {
                resp
            } else {
                resp.on_hover_text(tip)
            };
            if resp.clicked() {
                clicked = Some(i);
            }
        }
        if let Some(i) = clicked {
            self.pick_model(i);
        }
    }

    fn pick_model_ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let wide = ui.available_width() > 760.0;
        let left = |ui: &mut egui::Ui, this: &mut Self, shared: &mut Shared| {
            let n = this.report.as_ref().map(|r| r.models.len()).unwrap_or(0);
            let ram = scan::total_ram_bytes()
                .map(|b| format!(" · RAM {:.0} GB", b as f64 / 1e9))
                .unwrap_or_default();
            chrome::section(
                ui,
                "GGUF_SCAN_RESULTS",
                Some(&format!("{n} found{ram}")),
                |ui| {
                    this.model_table(ui);
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        let browsing = this.browse.is_some();
                        if chrome::stone_button(
                            ui,
                            if browsing {
                                "… dialog open"
                            } else {
                                "Browse…"
                            },
                        )
                        .clicked()
                            && !browsing
                        {
                            this.browse = browse_gguf(ui.ctx());
                            if this.browse.is_none() {
                                shared.toast(
                                    "no file dialog in this build — type the path below",
                                    pal::WARN,
                                );
                            }
                        }
                        if this.scan.is_some() {
                            if chrome::stone_button(ui, "✕ Cancel scan").clicked() {
                                if let Some(h) = &this.scan {
                                    h.cancel();
                                }
                            }
                        } else if chrome::stone_button(ui, "⟳ Rescan").clicked() {
                            this.start_scan();
                        }
                        chrome::hint(ui, &this.scan_status);
                    });
                    chrome::row(ui, "gguf_path", |ui| {
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut this.manual_path)
                                .hint_text("/path/to/model.gguf")
                                .desired_width(ui.available_width() - 70.0),
                        );
                        if (chrome::stone_button(ui, "Use").clicked()
                            || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))))
                            && !this.manual_path.trim().is_empty()
                        {
                            let p = paths::expand_tilde(this.manual_path.trim());
                            match ModelInfo::from_path(&p, scan::ModelSource::File) {
                                Ok(m) => this.apply_model(&m),
                                Err(e) => {
                                    shared.toast(format!("{}: {e}", p.display()), pal::BLOOD_SHEEN)
                                }
                            }
                        }
                    });
                    let v = this.check("gguf_path", &this.cfg.gguf_path);
                    if let Err(e) = v {
                        ui.label(
                            RichText::new(format!("⚠ {e}"))
                                .small()
                                .color(c32(pal::BLOOD_SHEEN)),
                        );
                    }
                },
            );
            if let Some(i) = this.selected {
                if let Some(rec) = this.recs.iter().find(|r| r.index == i) {
                    chrome::section(ui, "AUGURY", Some("why this model"), |ui| {
                        for r in &rec.reasons {
                            ui.label(
                                RichText::new(format!("† {r}"))
                                    .small()
                                    .color(c32(pal::TEXT_DIM)),
                            );
                        }
                    });
                }
            }
        };
        let right = |ui: &mut egui::Ui, this: &mut Self| {
            chrome::section(ui, "SAMPLING · CONTEXT", Some("[llm]"), |ui| {
                chrome::row(ui, "temperature", |ui| {
                    ui.spacing_mut().slider_width =
                        (ui.available_width() - 64.0).clamp(80.0, 220.0);
                    if ui
                        .add(
                            egui::Slider::new(&mut this.cfg.temperature, 0.0..=2.0)
                                .fixed_decimals(2)
                                .step_by(0.01),
                        )
                        .changed()
                    {
                        this.cfg.temperature = (this.cfg.temperature * 100.0).round() / 100.0;
                        this.sync_params_from_cfg();
                    }
                });
                chrome::row(ui, "ctx_size", |ui| {
                    if ui
                        .add(
                            egui::DragValue::new(&mut this.cfg.ctx_size)
                                .range(256..=131072)
                                .speed(64)
                                .suffix(" tokens"),
                        )
                        .changed()
                    {
                        this.sync_params_from_cfg();
                    }
                });
                if let Err(e) = this.check("ctx_size", &this.cfg.ctx_size.to_string()) {
                    ui.label(
                        RichText::new(format!("⚠ {e}"))
                            .small()
                            .color(c32(pal::BLOOD_SHEEN)),
                    );
                }
                chrome::row(ui, "chat_template", |ui| {
                    egui::ComboBox::from_id_salt("chat_template")
                        .selected_text(this.cfg.chat_template.clone())
                        .show_ui(ui, |ui| {
                            for t in ChatTemplate::ALL {
                                let n = t.llama_server_name();
                                if ui
                                    .selectable_label(this.cfg.chat_template == n, n)
                                    .clicked()
                                {
                                    this.cfg.chat_template = n.into();
                                }
                            }
                        });
                });
            });
            chrome::section(ui, "SCAN_DIRECTORIES", Some("[llm] scan_paths"), |ui| {
                let mut remove = None;
                for (i, d) in this.cfg.scan_paths.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(d)
                                .monospace()
                                .size(12.0)
                                .color(c32(pal::TEXT)),
                        );
                        if ui.small_button("✕").on_hover_text("remove").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    this.cfg.scan_paths.remove(i);
                }
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut this.new_dir)
                            .hint_text("~/models")
                            .desired_width(ui.available_width() - 70.0),
                    );
                    if chrome::stone_button(ui, "+ add").clicked()
                        && !this.new_dir.trim().is_empty()
                    {
                        this.cfg.scan_paths.push(this.new_dir.trim().to_string());
                        this.new_dir.clear();
                        this.start_scan();
                    }
                });
                if let Some(r) = &this.report {
                    let found: Vec<String> = r
                        .dirs
                        .iter()
                        .filter(|d| d.exists)
                        .map(|d| format!("{} ({})", d.path.display(), d.models_found))
                        .collect();
                    chrome::hint(
                        ui,
                        &format!(
                            "also searched: {}",
                            if found.is_empty() {
                                "defaults (none exist)".into()
                            } else {
                                found.join(", ")
                            }
                        ),
                    );
                }
            });
        };
        if wide {
            let total = ui.available_width();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(total * 0.6 - 4.0);
                    left(ui, self, shared);
                });
                ui.vertical(|ui| {
                    ui.set_width(total * 0.4 - 8.0);
                    right(ui, self);
                });
            });
        } else {
            left(ui, self, shared);
            right(ui, self);
        }
    }

    fn persona_ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let from = if self.cfg.gguf_path.is_empty() {
            self.mf.from().unwrap_or("?").to_string()
        } else {
            self.cfg.gguf_path.clone()
        };
        let short = Path::new(&from)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or(from.clone());
        chrome::section(
            ui,
            "MODELFILE · SYSTEM",
            Some(&format!("FROM {short}")),
            |ui| {
                if let Some(n) = &self.mf_note {
                    ui.label(RichText::new(n).small().color(c32(pal::WARN)));
                }
                let r = ui.add(
                    egui::TextEdit::multiline(&mut self.mf_system)
                        .desired_rows(6)
                        .desired_width(f32::INFINITY),
                );
                if r.changed() {
                    self.mf_dirty = true;
                }
                ui.horizontal(|ui| {
                    if chrome::stone_button(ui, "↺ Lily default persona").clicked() {
                        self.mf_system = LILY_SYSTEM_PROMPT.into();
                        self.mf_dirty = true;
                    }
                    chrome::hint(ui, &format!("→ {}", self.mf_path.display()));
                });
            },
        );
        let wide = ui.available_width() > 760.0;
        let params = |ui: &mut egui::Ui, this: &mut Self| {
            chrome::section(ui, "PARAMETER", Some("stop may repeat"), |ui| {
                let mut remove = None;
                egui::Grid::new("mf_params")
                    .num_columns(3)
                    .spacing([6.0, 4.0])
                    .show(ui, |ui| {
                        for (i, (k, v)) in this.mf_params.iter_mut().enumerate() {
                            if ui
                                .add(
                                    egui::TextEdit::singleline(k)
                                        .desired_width(110.0)
                                        .font(egui::TextStyle::Monospace),
                                )
                                .changed()
                            {
                                this.mf_dirty = true;
                            }
                            if ui
                                .add(
                                    egui::TextEdit::singleline(v)
                                        .desired_width(140.0)
                                        .font(egui::TextStyle::Monospace),
                                )
                                .changed()
                            {
                                this.mf_dirty = true;
                            }
                            if ui.small_button("✕").clicked() {
                                remove = Some(i);
                            }
                            ui.end_row();
                        }
                    });
                if let Some(i) = remove {
                    this.mf_params.remove(i);
                    this.mf_dirty = true;
                }
                if chrome::stone_button(ui, "+ parameter").clicked() {
                    this.mf_params.push(("stop".into(), String::new()));
                    this.mf_dirty = true;
                }
            });
        };
        let template = |ui: &mut egui::Ui, this: &mut Self| {
            chrome::section(
                ui,
                "TEMPLATE",
                Some(&this.cfg.chat_template.clone()),
                |ui| {
                    if ui
                        .add(
                            egui::TextEdit::multiline(&mut this.mf_template)
                                .code_editor()
                                .desired_rows(7)
                                .desired_width(f32::INFINITY),
                        )
                        .changed()
                    {
                        this.mf_dirty = true;
                    }
                },
            );
        };
        if wide {
            ui.columns(2, |c| {
                params(&mut c[0], self);
                template(&mut c[1], self);
            });
        } else {
            params(ui, self);
            template(ui, self);
        }
        let has_ollama = shared.runtime.as_ref().is_some_and(|r| r.ollama.is_some());
        chrome::section(
            ui,
            "OLLAMA",
            Some(if has_ollama { "found" } else { "not installed" }),
            |ui| {
                chrome::row(ui, "ollama_model", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.cfg.ollama_model).desired_width(160.0),
                    )
                });
                if let Err(e) = self.check("ollama_model", &self.cfg.ollama_model) {
                    ui.label(
                        RichText::new(format!("⚠ {e}"))
                            .small()
                            .color(c32(pal::BLOOD_SHEEN)),
                    );
                }
                if has_ollama {
                    ui.horizontal(|ui| {
                        let busy = self.create.is_some();
                        let label = if busy {
                            "creating…".to_string()
                        } else {
                            format!("ollama create {}", self.cfg.ollama_model)
                        };
                        if chrome::stone_button(ui, &label).clicked() && !busy {
                            self.run_ollama_create(ui.ctx(), shared);
                        }
                        chrome::hint(
                            ui,
                            "writes the Modelfile first; local FROM only, nothing is downloaded",
                        );
                    });
                    if let Some(r) = &self.create_result {
                        let (t, c) = match r {
                            Ok(o) => (o.as_str(), pal::OK),
                            Err(e) => (e.as_str(), pal::BLOOD_SHEEN),
                        };
                        ui.label(RichText::new(t).monospace().size(11.0).color(c32(c)));
                    }
                } else {
                    chrome::hint(
                        ui,
                        "Install ollama to build a named model; llama-server works without it.",
                    );
                }
            },
        );
    }

    fn write_modelfile(&mut self) -> Result<(), String> {
        let text = self.composed_modelfile().render();
        if let Some(d) = self.mf_path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        std::fs::write(&self.mf_path, text)
            .map_err(|e| format!("{}: {e}", self.mf_path.display()))?;
        self.mf = self.composed_modelfile();
        self.mf_dirty = false;
        Ok(())
    }

    fn run_ollama_create(&mut self, ctx: &egui::Context, shared: &mut Shared) {
        if let Err(e) = self.write_modelfile() {
            self.create_result = Some(Err(e));
            return;
        }
        let Some(bin) = shared
            .runtime
            .as_ref()
            .and_then(|r| r.ollama.as_ref())
            .map(|b| b.path.clone())
        else {
            return;
        };
        let (name, path, cfg) = (
            self.cfg.ollama_model.clone(),
            self.mf_path.clone(),
            rounded(&self.cfg),
        );
        self.create_result = None;
        let audit = lillith_core::config::audit_log_path();
        self.create = Some(Task::spawn(ctx, "ollama-create", move || {
            // No card (Ko exempted it): one plain audit line per launch. A
            // command that fails its checks never runs, so it gets none.
            if let Ok(c) = launch::ollama_create_command(&bin, &name, &path, &cfg) {
                audit_ungated(audit, OLLAMA_CREATE_UNGATED_TAG, OLLAMA_CREATE_KIND, &c);
            }
            launch::ollama_create(&bin, &name, &path, &cfg).map_err(|e| e.to_string())
        }));
    }

    fn test_save_ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        let cmd = launch::llama_server_command(&self.cfg, None);
        chrome::section(
            ui,
            "LAUNCH_COMMAND",
            Some("llama-server · loopback"),
            |ui| match &cmd {
                Ok(c) => {
                    if chrome::code_block(ui, &c.to_shell_string()) {
                        shared.toast("launch command copied", pal::OK);
                    }
                    if self.cfg.backend == Backend::LlamaServer {
                        ui.horizontal(|ui| {
                            let running = self
                                .server
                                .as_mut()
                                .is_some_and(|c| matches!(c.try_wait(), Ok(None)));
                            if running {
                                if chrome::stone_button(ui, "■ Stop llama-server").clicked() {
                                    if let Some(mut c) = self.server.take() {
                                        let _ = c.kill();
                                        let _ = c.wait();
                                    }
                                    self.server_note = Some("stopped".into());
                                }
                            } else if chrome::stone_button(ui, "▶ Start llama-server").clicked() {
                                audit_settings_start(lillith_core::config::audit_log_path(), c);
                                match c
                                    .to_command()
                                    .stdin(std::process::Stdio::null())
                                    .stdout(std::process::Stdio::null())
                                    .stderr(std::process::Stdio::null())
                                    .spawn()
                                {
                                    Ok(child) => {
                                        self.server_note = Some(format!(
                                            "started (pid {}) — model load can take a minute",
                                            child.id()
                                        ));
                                        self.server = Some(child);
                                    }
                                    Err(e) => {
                                        self.server_note = Some(format!("couldn't start: {e}"))
                                    }
                                }
                            }
                            if let Some(n) = &self.server_note {
                                chrome::hint(ui, n);
                            }
                        });
                    }
                }
                Err(e) => {
                    ui.label(RichText::new(format!("⚠ {e}")).color(c32(pal::WARN)));
                }
            },
        );
        let wide = ui.available_width() > 760.0;
        let trials = |ui: &mut egui::Ui, this: &mut Self| {
            chrome::section(ui, "TRIALS", Some("loopback only"), |ui| {
                ui.horizontal(|ui| {
                    let busy = this.ping.is_some();
                    if chrome::stone_button(
                        ui,
                        if busy {
                            "pinging…"
                        } else {
                            "◉ Ping server"
                        },
                    )
                    .clicked()
                        && !busy
                    {
                        let cfg = rounded(&this.cfg);
                        this.ping_result = None;
                        this.ping = Some(Task::spawn(ui.ctx(), "ping", move || {
                            check::ping_server(&cfg).map_err(|e| e.to_string())
                        }));
                    }
                    let busy = this.chat.is_some();
                    if chrome::stone_button(
                        ui,
                        if busy {
                            "waiting for Lily…"
                        } else {
                            "✉ Test chat"
                        },
                    )
                    .clicked()
                        && !busy
                    {
                        let cfg = rounded(&this.cfg);
                        this.chat_result = None;
                        this.chat = Some(Task::spawn(ui.ctx(), "chat", move || {
                            check::test_chat(&cfg, "Say hi to Ko in one short sentence.")
                                .map_err(|e| e.to_string())
                        }));
                    }
                });
                match &this.ping_result {
                    Some(Ok(h)) => {
                        ui.horizontal(|ui| {
                            chrome::chip(
                                ui,
                                if h.ready { "READY" } else { "NOT READY" },
                                if h.ready { pal::OK } else { pal::WARN },
                                true,
                            );
                            ui.label(
                                RichText::new(format!("{} · {}", h.url, h.status))
                                    .monospace()
                                    .size(11.5)
                                    .color(c32(pal::TEXT_DIM)),
                            );
                        });
                        if let Some(m) = &h.model {
                            chrome::hint(
                                ui,
                                &format!(
                                    "model {m}  ctx {}",
                                    h.ctx.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                                ),
                            );
                        }
                    }
                    Some(Err(e)) => {
                        ui.label(
                            RichText::new(format!("⚠ {e}"))
                                .small()
                                .color(c32(pal::BLOOD_SHEEN)),
                        );
                    }
                    None => {}
                }
                match &this.chat_result {
                    Some(Ok(t)) => {
                        ui.label(
                            RichText::new(format!("Lily: {t}"))
                                .color(c32(th::accent().base.lerp(pal::TEXT, 0.4))),
                        );
                    }
                    Some(Err(e)) => {
                        ui.label(
                            RichText::new(format!("⚠ {e}"))
                                .small()
                                .color(c32(pal::BLOOD_SHEEN)),
                        );
                    }
                    None => {}
                }
            });
        };
        let seal = |ui: &mut egui::Ui, this: &mut Self, shared: &mut Shared| {
            chrome::section(ui, "SEAL_THE_PACT", Some("[llm] + Modelfile"), |ui| {
                let errs = validate::validate_all_with(&rounded(&this.cfg), &this.ctx_validate());
                if errs.is_empty() {
                    chrome::chip(ui, "ALL FIELDS VALID", pal::OK, true);
                } else {
                    for e in &errs {
                        ui.label(
                            RichText::new(format!("⚠ {}: {}", e.key, e.message))
                                .small()
                                .color(c32(pal::BLOOD_SHEEN)),
                        );
                    }
                }
                chrome::hint(
                    ui,
                    &format!(
                        "writes [llm] (+[chat] bridge) to {} and the Modelfile to {}",
                        this.config_path.display(),
                        this.mf_path.display()
                    ),
                );
                ui.add_space(4.0);
                let pending = this.pending() + shared.store.pending();
                if chrome::blade_button(ui, &format!("SAVE · {pending} pending"), pending > 0)
                    .clicked()
                {
                    shared.want_save = true;
                }
            });
        };
        if wide {
            ui.columns(2, |c| {
                trials(&mut c[0], self);
                seal(&mut c[1], self, shared);
            });
        } else {
            trials(ui, self);
            seal(ui, self, shared);
        }
    }


    fn quantumsec_wizard_ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        chrome::section(ui, "QUANTUMSEC", Some("DemonOS · PhantomCore"), |ui| {
            ui.label(
                RichText::new("PhantomCore lattice — local-only shield. Protons watch resources; blood seal gates lockdown / unit enable.")
                    .color(c32(pal::TEXT)),
            );
            chrome::hint(
                ui,
                "Nothing leaves the box. Red team = echo dry-run. Consent JSONL under ~/.biotek/consent/. Reports rotate (keep 8).",
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                chrome::chip(ui, "LOCAL ONLY", pal::OK, true);
                chrome::chip(ui, "CONSENT GATED", pal::CYBER_CYAN, true);
                chrome::chip(ui, "NO AUTO-ENABLE", pal::WARN, true);
            });
            ui.add_space(8.0);
            ui.label(
                RichText::new("Open the QuantumSec tab for live proton status, consent log, personalization reports, and systemd user-unit proposals.")
                    .color(c32(pal::TEXT_DIM)),
            );
            ui.add_space(6.0);
            if chrome::blade_button(ui, "Open QuantumSec panel", true).clicked() {
                shared.goto_tab = Some("quantumsec".into());
            }
            if chrome::stone_button(ui, "Mark awakened (local)").clicked() {
                if let Some(path) = AwakeningState::path_default() {
                    let mut state = AwakeningState::load(&path);
                    state.mark_shown();
                    match state.save(&path) {
                        Ok(()) => shared.toast(
                            "Awakening flagged under ~/.biotek/config/",
                            pal::CYBER_CYAN,
                        ),
                        Err(e) => shared.toast(format!("awakening: {e}"), pal::BLOOD_SHEEN),
                    }
                }
            }
        });
    }

    fn avatar_ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        chrome::section(ui, "AVATAR", Some("✔ canon"), |ui| {
            ui.horizontal(|ui| {
                chrome::chip(ui, "LOCKED CANON", pal::OK, true);
                ui.label(
                    RichText::new("Lillith's v2 sprite set is installed. Nothing to do here.")
                        .color(c32(pal::TEXT)),
                );
            });
            chrome::hint(ui, "Size, opacity and position live in the Avatar tab.");
            if chrome::stone_button(ui, "Open the Avatar tab").clicked() {
                shared.goto_tab = Some("avatar".into());
            }
        });
    }
}

#[cfg(feature = "file-dialog")]
fn browse_gguf(ctx: &egui::Context) -> Option<Task<Option<PathBuf>>> {
    Some(Task::spawn(ctx, "browse", || {
        let mut d = rfd::FileDialog::new()
            .set_title("Pick a GGUF model")
            .add_filter("GGUF model", &["gguf"]);
        if let Some(h) = paths::home_dir() {
            d = d.set_directory(h);
        }
        d.pick_file()
    }))
}

#[cfg(not(feature = "file-dialog"))]
fn browse_gguf(_ctx: &egui::Context) -> Option<Task<Option<PathBuf>>> {
    None
}

impl SettingsTab for BrainTab {
    fn id(&self) -> &str {
        "brain"
    }
    fn title(&self) -> &str {
        "Chat / Brain"
    }
    fn rune(&self) -> usize {
        4
    }
    fn tick(&mut self, ctx: &egui::Context, shared: &mut Shared) {
        // All-in-one download: offer the bundled llama-server (a pending
        // change, saved with the rest) when none is configured yet.
        if self.cfg.llama_server_bin.trim().is_empty() {
            if let Some(b) = shared
                .runtime
                .as_ref()
                .and_then(|r| r.llama_server.as_ref())
            {
                if lillith_setup::bundle::llama_dirs()
                    .iter()
                    .any(|d| b.path.parent() == Some(d.as_path()))
                {
                    self.cfg.llama_server_bin = b.path.display().to_string();
                }
            }
        }
        if let Some(h) = &mut self.scan {
            if let Some(p) = h.progress() {
                self.scan_status =
                    format!("scanning {} · {} found", p.dir.display(), p.models_found);
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            if let Some(rep) = h.try_recv() {
                self.recs = scan::recommend(&rep.models, scan::total_ram_bytes());
                self.selected = rep
                    .models
                    .iter()
                    .position(|m| m.path.display().to_string() == self.cfg.gguf_path);
                let (n, d) = (
                    rep.models.len(),
                    rep.dirs.iter().filter(|d| d.exists).count(),
                );
                self.scan_status = format!(
                    "{n} model{} in {d} dir{}",
                    if n == 1 { "" } else { "s" },
                    if d == 1 { "" } else { "s" }
                );
                self.report = Some(rep);
                self.scan = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        if let Some(t) = &mut self.browse {
            if let Some(r) = t.poll() {
                self.browse = None;
                if let Some(p) = r {
                    self.manual_path = p.display().to_string();
                    match ModelInfo::from_path(&p, scan::ModelSource::File) {
                        Ok(m) => self.apply_model(&m),
                        Err(e) => shared.toast(format!("{}: {e}", p.display()), pal::BLOOD_SHEEN),
                    }
                }
            }
        }
        if let Some(t) = &mut self.ping {
            if let Some(r) = t.poll() {
                self.ping = None;
                self.ping_result = Some(r);
                shared.want_redetect = true;
            }
        }
        if let Some(t) = &mut self.chat {
            if let Some(r) = t.poll() {
                self.chat = None;
                self.chat_result = Some(r);
            }
        }
        if let Some(t) = &mut self.create {
            if let Some(r) = t.poll() {
                self.create = None;
                self.create_result = Some(r);
            }
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared) {
        if let Some(e) = &self.load_error {
            ui.label(
                RichText::new(format!(
                    "⚠ config: {e} (showing defaults; saving will not touch a broken file)"
                ))
                .color(c32(pal::WARN)),
            );
        }
        if let Some(n) = &self.args_notice {
            ui.label(RichText::new(format!("⚠ {n}")).color(c32(pal::WARN)));
        }
        self.steps_row(ui, shared);
        let i = self.step.index();
        ui.label(
            RichText::new(format!("FIRST-RUN SETUP · step {} of {} · point Lily at a local GGUF + llama-server (loopback only); QuantumSec seals last", i + 1, Step::ALL.len()))
                .font(th::display_font(9.5))
                .color(c32(pal::TEXT_MUTE)),
        );
        ui.add_space(4.0);
        match self.step {
            Step::Avatar => self.avatar_ui(ui, shared),
            Step::Runtime => self.runtime_ui(ui, shared),
            Step::PickModel => self.pick_model_ui(ui, shared),
            Step::Persona => self.persona_ui(ui, shared),
            Step::TestSave => self.test_save_ui(ui, shared),
            Step::QuantumSec => self.quantumsec_wizard_ui(ui, shared),
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if i > 0 && chrome::stone_button(ui, "‹ Back").clicked() {
                self.step = Step::ALL[i - 1];
            }
            if i + 1 < Step::ALL.len() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if chrome::blade_button(
                        ui,
                        &format!("NEXT · {}", Step::ALL[i + 1].label().to_uppercase()),
                        true,
                    )
                    .clicked()
                    {
                        self.step = Step::ALL[i + 1];
                    }
                });
            }
        });
    }
    fn pending(&self) -> usize {
        (rounded(&self.cfg) != rounded(&self.saved)) as usize + self.mf_dirty as usize
    }
    fn save(&mut self, _shared: &mut Shared) -> Result<(), String> {
        if self.load_error.is_some() {
            return Err("config.toml could not be parsed; fix it by hand first".into());
        }
        let cfg = rounded(&self.cfg);
        if cfg != rounded(&self.saved) {
            let errs = validate::validate_all_with(&cfg, &self.ctx_validate());
            if let Some(e) = errs.first() {
                return Err(format!("{}: {}", e.key, e.message));
            }
            llm::save(&self.config_path, &cfg, true).map_err(|e| e.to_string())?;
            self.cfg = cfg.clone();
            self.saved = cfg;
        }
        if self.mf_dirty {
            self.write_modelfile()?;
        }
        self.args_notice = extra_args_notice(&self.config_path);
        Ok(())
    }
    fn revert(&mut self, _shared: &mut Shared) {
        self.cfg = self.saved.clone();
        self.args_notice = extra_args_notice(&self.config_path);
        let m = Modelfile::from_file(&self.mf_path)
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_else(|| Self::default_modelfile(&self.cfg));
        self.set_modelfile(m, false);
        self.manual_path = self.cfg.gguf_path.clone();
    }
    fn busy(&self) -> bool {
        self.scan.is_some() || self.ping.is_some() || self.chat.is_some()
    }
    fn deep_link(&mut self, arg: &str) {
        if let Some(s) = Step::from_name(arg) {
            self.step = s;
        }
    }
}

impl BrainTab {
    /// Jump to a wizard step by name (`runtime`, `pick-model`, ...).
    pub fn set_step(&mut self, s: Step) {
        self.step = s;
    }
}

/// The "Start llama-server" button runs without a card: it leaves one
/// plain `SETTINGS_UNGATED` audit line with the command (api key masked).
fn audit_settings_start(log: Option<PathBuf>, c: &launch::LaunchCommand) {
    audit_ungated(log, SETTINGS_UNGATED_TAG, MODEL_SERVER_KIND, c);
}

/// One plain, card-less launch line (`tag`, kind `kind`) for `c`, secrets
/// masked: `<ts>\t<tag>\t<kind>\t<program>\tno-card\t$ <argv>`.
fn audit_ungated(log: Option<PathBuf>, tag: &str, kind: &str, c: &launch::LaunchCommand) {
    let argv: Vec<String> = std::iter::once(c.program.clone())
        .chain(c.args.iter().cloned())
        .collect();
    lillith_core::soul_contract::SoulContract::new(log).record_ungated_launch_as(tag, kind, &argv);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wizard_writes_the_same_ollama_create_line_as_core() {
        let dir = std::env::temp_dir().join(format!("lillith-wiz-fmt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (dir.join("core.log"), dir.join("setup.log"));
        let argv: Vec<String> = [
            "/usr/bin/ollama",
            "create",
            "lily",
            "-f",
            "/m/Model\tfile",
            "--api-key",
            "s3cret",
        ]
        .map(String::from)
        .to_vec();
        lillith_core::soul_contract::SoulContract::new(Some(a.clone())).record_ungated_launch_as(
            OLLAMA_CREATE_UNGATED_TAG,
            OLLAMA_CREATE_KIND,
            &argv,
        );
        lillith_setup::audit::record_ungated_launch(
            &b,
            lillith_setup::audit::OLLAMA_CREATE_UNGATED_TAG,
            lillith_setup::audit::OLLAMA_CREATE_KIND,
            &argv,
        )
        .unwrap();
        let cols = |p: &Path| -> Vec<String> {
            let t = std::fs::read_to_string(p).unwrap();
            assert_eq!(t.lines().count(), 1, "{t}");
            assert!(t.ends_with('\n'));
            let c: Vec<String> = t.trim_end().split('\t').map(String::from).collect();
            assert_eq!(c[0].len(), "2026-10-24T10:00:00Z".len(), "{t}");
            c[1..].to_vec()
        };
        assert_eq!(cols(&a), cols(&b));
        assert!(!std::fs::read_to_string(&b).unwrap().contains("s3cret"));
        assert_eq!(
            lillith_setup::audit::OLLAMA_CREATE_UNGATED_TAG,
            OLLAMA_CREATE_UNGATED_TAG
        );
        assert_eq!(lillith_setup::audit::OLLAMA_CREATE_KIND, OLLAMA_CREATE_KIND);
        assert_eq!(
            lillith_setup::audit::NO_CARD,
            lillith_core::soul_contract::NO_CARD
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refused_extra_arg_shows_a_notice_naming_it() {
        let dir = std::env::temp_dir().join(format!("lillith-args-notice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(
            &p,
            "[chat.server]\nextra_args = [\"--jinja\", \"--chat-template\", \"./t.jinja\", \"-np\", \"2\"]\n",
        )
        .unwrap();
        let n = extra_args_notice(&p).expect("a notice");
        assert!(n.contains("`--chat-template ./t.jinja`"), "{n}");
        assert!(n.contains("ARG_REFUSED"), "{n}");
        std::fs::write(
            &p,
            "[chat.server]\nextra_args = [\"--chat-template\", \"llama3\", \"-cb\"]\n",
        )
        .unwrap();
        assert!(extra_args_notice(&p).is_none());
        assert!(extra_args_notice(&dir.join("missing.toml")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ollama_create_leaves_one_plain_masked_audit_line() {
        let dir = std::env::temp_dir().join(format!("lillith-ungated-oc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("audit.log");
        let c = launch::LaunchCommand {
            program: "/usr/bin/ollama".into(),
            args: ["create", "lily", "-f", "/m/Modelfile.lily"]
                .map(String::from)
                .to_vec(),
        };
        audit_ungated(
            Some(log.clone()),
            OLLAMA_CREATE_UNGATED_TAG,
            OLLAMA_CREATE_KIND,
            &c,
        );
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains(
            "\tOLLAMA_CREATE_UNGATED\tollama_create\t/usr/bin/ollama\tno-card\t$ /usr/bin/ollama create lily -f /m/Modelfile.lily"
        ), "{text}");
        // It never opens or closes a card.
        assert!(
            lillith_core::soul_contract::SoulContract::new(Some(log.clone()))
                .open_model_server_cards()
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_start_button_leaves_a_plain_masked_audit_line() {
        let dir = std::env::temp_dir().join(format!("lillith-ungated-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("audit.log");
        let c = launch::LaunchCommand {
            program: "/opt/llama/llama-server".into(),
            args: [
                "-m",
                "/m.gguf",
                "--api-key",
                "sekrit",
                "--host",
                "127.0.0.1",
            ]
            .map(String::from)
            .to_vec(),
        };
        audit_settings_start(Some(log.clone()), &c);
        audit_settings_start(Some(log.clone()), &c);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(!text.contains("sekrit"), "{text}");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "one line per click, appended");
        assert!(lines[0].contains(
            "\tSETTINGS_UNGATED\tstart_model_server\t/opt/llama/llama-server\tno-card\t$ /opt/llama/llama-server -m /m.gguf --api-key *** --host 127.0.0.1"
        ), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn llm_round_trip_with_rounding() {
        let dir =
            std::env::temp_dir().join(format!("lillith-settings-brain-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(&p, "mode = \"wander\"\n[overlay]\nmargin_left = 9\n").unwrap();
        let mut tab = BrainTab::new(&p);
        tab.cfg.temperature = 0.700_000_05;
        tab.cfg.gguf_path = "/models/Dolphin3.0-Llama3.2-3B-Q4_K_M.gguf".into();
        tab.cfg.ctx_size = 4096;
        tab.mf_path = dir.join("Modelfile.lily");
        tab.mf_dirty = true;
        assert!(tab.pending() >= 1);
        // gguf doesn't exist on disk → validation may refuse; use validate-free path check
        let cfg = rounded(&tab.cfg);
        llm::save(&p, &cfg, true).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("temperature = 0.7\n"), "{text}");
        assert!(text.contains("mode = \"wander\""));
        assert!(text.contains("margin_left = 9"));
        let back = llm::load(&p).unwrap();
        assert_eq!(back.gguf_path, cfg.gguf_path);
        assert!((back.temperature - 0.7).abs() < 1e-6);
        tab.write_modelfile().unwrap();
        let mf = Modelfile::from_file(&dir.join("Modelfile.lily"))
            .unwrap()
            .unwrap();
        assert_eq!(
            mf.from(),
            Some("/models/Dolphin3.0-Llama3.2-3B-Q4_K_M.gguf")
        );
        assert!(mf.system().unwrap().contains("Lily"));
    }

    #[test]
    fn params_edit_compose() {
        let p = std::env::temp_dir().join(format!(
            "lillith-settings-brain2-{}.toml",
            std::process::id()
        ));
        let mut tab = BrainTab::new(&p);
        tab.mf_params = vec![
            ("temperature".into(), "0.65".into()),
            ("stop".into(), "<|im_end|>".into()),
            ("stop".into(), "<|im_start|>".into()),
        ];
        let m = tab.composed_modelfile();
        assert_eq!(m.param("temperature"), Some("0.65"));
        assert_eq!(m.params("stop").len(), 2);
        assert_eq!(m.param("top_p"), None, "removed rows are dropped");
    }

    #[test]
    fn step_names() {
        assert_eq!(Step::from_name("pick-model"), Some(Step::PickModel));
        assert_eq!(Step::from_name("Test & save"), Some(Step::TestSave));
        assert_eq!(Step::from_name("quantumsec"), Some(Step::QuantumSec));
        assert_eq!(Step::ALL.len(), 6);
    }
}
