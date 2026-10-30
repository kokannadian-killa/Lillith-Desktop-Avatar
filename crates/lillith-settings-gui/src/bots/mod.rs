//! Bots: Lillith's local helper bots (the Bots tab's engine).
//!
//! A bot is a named, supervised **local** worker with one job type
//! ([`BotKind`]): File Organizer, Program Builder, Web Page Builder,
//! OS Builder, Red Team (own machine only) and Pose Builder. Ko gives a bot
//! a job in plain words; the bot thinks on a worker thread ([`worker`]),
//! using its built-in recipe and, where it helps, the local model on
//! loopback (`[llm]`, `127.0.0.1:8080` by default; never anything else),
//! and comes back with a [`Plan`]: a few words plus [`Step`]s.
//!
//! **Nothing a bot proposes runs on its own.** Every step becomes a
//! [`PendingAction`] that is shown on the shared Soul Contract card, and only
//! a mouse click on the card's Yes runs it (typed, spoken or keyboard yes
//! never approves: the popup ignores them). Yes / No are audited
//! (`CONFIRM` / `DENY`), reads are audited as `READ`. Steps are confined to
//! the bot's allowed folders ([`confine`]) and to the operations its kind
//! allows ([`BotKind::allows`]).
//!
//! **Stopping.** Each job has its own stop flag; the tab's Stop, the
//! process-wide kill switch ([`lillith_core::access::kill_switch::stop_all`],
//! also what Ctrl-C / SIGTERM trigger through [`install_ctrlc_stop`]) and
//! closing the window all end every bot. A running command is killed by the
//! kill switch (its runner watches the stop epoch).
//!
//! **No network.** Bots never fetch anything. Online research only ever
//! happens through the Ko-gated Tor dream-learning path (Memory Engine's
//! session, `lillith_learn::workers`), shown read-only through
//! [`DreamStatusSource`]; it is never started from here.
//!
//! The roster (names, descriptions, folders, model, on/off) lives in
//! `bots.toml` next to `config.toml`, so Ko can rename every bot.

pub mod recipes;
pub mod worker;

use lillith_core::access::organize::{OrganizeOp, OrganizePlan};
use lillith_core::build::Language;
use lillith_core::soul_contract::PendingAction;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table};

/// File name of the roster, next to `config.toml`.
pub const ROSTER_FILE: &str = "bots.toml";

/// Longest job text a bot accepts.
pub const MAX_JOB_CHARS: usize = 4000;

/// What a bot does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BotKind {
    FileOrganizer,
    ProgramBuilder,
    WebPageBuilder,
    OsBuilder,
    RedTeam,
    PoseBuilder,
}

/// One operation a step performs (what [`BotKind::allows`] checks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    WriteFile,
    Mkdir,
    Move,
    Copy,
    Run,
    Scaffold,
    Build,
}

impl BotKind {
    pub const ALL: [BotKind; 6] = [
        BotKind::FileOrganizer,
        BotKind::ProgramBuilder,
        BotKind::WebPageBuilder,
        BotKind::OsBuilder,
        BotKind::RedTeam,
        BotKind::PoseBuilder,
    ];

    /// Stable token in `bots.toml`.
    pub fn as_str(self) -> &'static str {
        match self {
            BotKind::FileOrganizer => "file-organizer",
            BotKind::ProgramBuilder => "program-builder",
            BotKind::WebPageBuilder => "web-page-builder",
            BotKind::OsBuilder => "os-builder",
            BotKind::RedTeam => "red-team",
            BotKind::PoseBuilder => "pose-builder",
        }
    }

    pub fn parse(s: &str) -> Option<BotKind> {
        BotKind::ALL.into_iter().find(|k| k.as_str() == s.trim())
    }

    /// The role line under the bot's name.
    pub fn role(self) -> &'static str {
        match self {
            BotKind::FileOrganizer => "File Organizer",
            BotKind::ProgramBuilder => "Program Builder",
            BotKind::WebPageBuilder => "Web Page Builder",
            BotKind::OsBuilder => "OS Builder",
            BotKind::RedTeam => "Red Team (your machine only)",
            BotKind::PoseBuilder => "Lillith Pose Builder",
        }
    }

    /// Demon name of the stock bot of this kind.
    pub fn demon(self) -> &'static str {
        match self {
            BotKind::FileOrganizer => "Mammon",
            BotKind::ProgramBuilder => "Belphegor",
            BotKind::WebPageBuilder => "Paimon",
            BotKind::OsBuilder => "Baal",
            BotKind::RedTeam => "Andras",
            BotKind::PoseBuilder => "Naamah",
        }
    }

    /// Default description (editable).
    pub fn blurb(self) -> &'static str {
        match self {
            BotKind::FileOrganizer => "Sorts a messy folder into type or month folders. Moves are journaled, so `lillith undo-organize` puts them back.",
            BotKind::ProgramBuilder => "Scaffolds and builds small Rust, Python or fish programs in a folder you choose, offline.",
            BotKind::WebPageBuilder => "Writes static web pages (HTML + CSS) into a folder you choose. Nothing is published.",
            BotKind::OsBuilder => "DemonOS / Phantomcore respin helper: package lists, configs and build scripts. Writes files; never runs a system-changing command without your Yes.",
            BotKind::RedTeam => "Security auditor for your own machine and projects: local, read-only scans and a findings report. Never touches other people's systems.",
            BotKind::PoseBuilder => "Drives the Pose Builder video pipeline: turns your videos into pose candidates for you to approve in Pose Builder.",
        }
    }

    /// Default allowed folders (`~` = home).
    pub fn default_folders(self) -> &'static [&'static str] {
        match self {
            BotKind::FileOrganizer => &["~/Downloads"],
            BotKind::ProgramBuilder => &["~/Projects/lillith-builds"],
            BotKind::WebPageBuilder => &["~/Projects/lillith-sites"],
            BotKind::OsBuilder => &["~/Projects/demonos-respin"],
            BotKind::RedTeam => &["~/Projects"],
            BotKind::PoseBuilder => &["~/Videos"],
        }
    }

    /// Whether this kind may perform `op`.
    pub fn allows(self, op: Op) -> bool {
        use Op::*;
        match self {
            BotKind::FileOrganizer => matches!(op, Mkdir | Move | Copy),
            BotKind::ProgramBuilder => matches!(op, WriteFile | Mkdir | Scaffold | Build | Run),
            BotKind::WebPageBuilder => matches!(op, WriteFile | Mkdir),
            BotKind::OsBuilder => matches!(op, WriteFile | Mkdir | Run),
            // Andras only writes his report.
            BotKind::RedTeam => matches!(op, WriteFile),
            BotKind::PoseBuilder => matches!(op, Run),
        }
    }

    /// Whether the job may be planned by the local model (otherwise the
    /// built-in recipe always plans it).
    pub fn uses_model(self) -> bool {
        matches!(
            self,
            BotKind::ProgramBuilder | BotKind::WebPageBuilder | BotKind::OsBuilder
        )
    }

    /// Avatar accent.
    pub fn color(self) -> lillith_theme::palette::Rgba {
        use lillith_theme::palette as pal;
        match self {
            BotKind::FileOrganizer => pal::EMBER_CORE,
            BotKind::ProgramBuilder => pal::NEON_BLUE,
            BotKind::WebPageBuilder => pal::PINK,
            BotKind::OsBuilder => pal::VIOLET,
            BotKind::RedTeam => pal::BLOOD_NEON,
            BotKind::PoseBuilder => pal::EMBER,
        }
    }

    /// Glyph drawn on the avatar.
    pub fn glyph(self) -> &'static str {
        match self {
            BotKind::FileOrganizer => "🗀",
            BotKind::ProgramBuilder => "⚙",
            BotKind::WebPageBuilder => "🌐",
            BotKind::OsBuilder => "⛭",
            BotKind::RedTeam => "⚔",
            BotKind::PoseBuilder => "★",
        }
    }
}

/// Spare demon names for new bots (Ko can rename any of them).
pub const SPARE_NAMES: &[&str] = &[
    "Asmodeus", "Astaroth", "Leraje", "Marbas", "Vassago", "Furfur", "Orobas", "Purson", "Gremory",
    "Sitri", "Valefor", "Amon",
];

/// One bot in the roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotDef {
    /// Stable id (`mammon`, `bot-7`...).
    pub id: String,
    pub kind: BotKind,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    /// Model name for an Ollama backend; empty = Lillith's local model.
    pub model: String,
    /// Let the local model plan jobs (kinds that [`BotKind::uses_model`]).
    pub use_model: bool,
    /// Allowed folders (`~` allowed); every step is confined to these.
    pub folders: Vec<String>,
}

impl BotDef {
    /// The stock bot of `kind`.
    pub fn stock(kind: BotKind) -> Self {
        Self {
            id: kind.demon().to_ascii_lowercase(),
            kind,
            name: kind.demon().into(),
            description: kind.blurb().into(),
            enabled: true,
            model: String::new(),
            use_model: true,
            folders: kind.default_folders().iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Allowed folders with `~` expanded (unusable entries dropped).
    pub fn resolved_folders(&self) -> Vec<PathBuf> {
        self.folders
            .iter()
            .filter_map(|f| expand_home(f.trim()))
            .collect()
    }
}

/// `~` / `~/x` → home; other paths must be absolute.
pub fn expand_home(s: &str) -> Option<PathBuf> {
    if s.is_empty() {
        return None;
    }
    let p = if s == "~" {
        home()?
    } else if let Some(rest) = s.strip_prefix("~/") {
        home()?.join(rest)
    } else {
        PathBuf::from(s)
    };
    p.is_absolute().then_some(p)
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The six stock bots.
pub fn stock_roster() -> Vec<BotDef> {
    BotKind::ALL.into_iter().map(BotDef::stock).collect()
}

/// `bots.toml` next to `config_path`.
pub fn roster_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(|d| d.join(ROSTER_FILE))
        .unwrap_or_else(|| PathBuf::from(ROSTER_FILE))
}

/// Reads the roster (the stock six when the file is missing or empty).
/// Unknown kinds are skipped; missing fields take the kind's defaults.
pub fn load_roster(path: &Path) -> Result<Vec<BotDef>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(stock_roster()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    parse_roster(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Parses `bots.toml` text.
pub fn parse_roster(text: &str) -> Result<Vec<BotDef>, String> {
    let doc: DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    let Some(arr) = doc.get("bot").and_then(Item::as_array_of_tables) else {
        return Ok(stock_roster());
    };
    let mut out: Vec<BotDef> = Vec::new();
    for t in arr.iter() {
        let s = |k: &str| t.get(k).and_then(Item::as_str).map(str::to_string);
        let Some(kind) = s("kind").as_deref().and_then(BotKind::parse) else {
            continue;
        };
        let mut b = BotDef::stock(kind);
        if let Some(id) = s("id").filter(|v| valid_id(v)) {
            b.id = id;
        }
        if out.iter().any(|o| o.id == b.id) {
            continue;
        }
        if let Some(v) = s("name").filter(|v| !v.trim().is_empty()) {
            b.name = v;
        }
        if let Some(v) = s("description") {
            b.description = v;
        }
        if let Some(v) = s("model") {
            b.model = v;
        }
        if let Some(v) = t.get("enabled").and_then(Item::as_bool) {
            b.enabled = v;
        }
        if let Some(v) = t.get("use_model").and_then(Item::as_bool) {
            b.use_model = v;
        }
        if let Some(a) = t.get("folders").and_then(Item::as_array) {
            b.folders = a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
        }
        out.push(b);
    }
    Ok(out)
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `bots.toml` text for `bots`.
pub fn roster_toml(bots: &[BotDef]) -> String {
    let mut doc = DocumentMut::new();
    let mut arr = ArrayOfTables::new();
    for b in bots {
        let mut t = Table::new();
        t.insert("id", toml_edit::value(b.id.as_str()));
        t.insert("kind", toml_edit::value(b.kind.as_str()));
        t.insert("name", toml_edit::value(b.name.as_str()));
        t.insert("description", toml_edit::value(b.description.as_str()));
        t.insert("enabled", toml_edit::value(b.enabled));
        t.insert("model", toml_edit::value(b.model.as_str()));
        t.insert("use_model", toml_edit::value(b.use_model));
        let mut f = Array::new();
        for d in &b.folders {
            f.push(d.as_str());
        }
        t.insert("folders", toml_edit::value(f));
        arr.push(t);
    }
    doc.insert("bot", Item::ArrayOfTables(arr));
    format!(
        "# Lillith ▸ Settings ▸ Bots: local helper bots. Rename them freely.\n\
         # Every file change, command or launch a bot proposes still needs a\n\
         # mouse click on the Soul Contract card's Yes.\n{doc}"
    )
}

/// Writes the roster atomically.
pub fn save_roster(path: &Path, bots: &[BotDef]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = path.with_extension("toml.settings-tmp");
    std::fs::write(&tmp, roster_toml(bots)).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A fresh bot of `kind` with an unused id and spare demon name.
pub fn new_bot(kind: BotKind, existing: &[BotDef]) -> BotDef {
    let mut b = BotDef::stock(kind);
    let taken = |n: &str| existing.iter().any(|e| e.name.eq_ignore_ascii_case(n));
    b.name = SPARE_NAMES
        .iter()
        .find(|n| !taken(n))
        .map(|n| n.to_string())
        .unwrap_or_else(|| format!("{} {}", kind.demon(), existing.len() + 1));
    let mut n = existing.len() + 1;
    loop {
        let id = format!("bot-{n}");
        if !existing.iter().any(|e| e.id == id) {
            b.id = id;
            break;
        }
        n += 1;
    }
    b
}

// ---------------------------------------------------------------- confinement

/// Resolves `p` (relative = inside the first folder) and checks it stays
/// inside one of `folders`, also through symlinks of existing ancestors.
/// `..` components are refused outright.
pub fn confine(folders: &[PathBuf], p: &Path) -> Result<PathBuf, String> {
    let first = folders
        .first()
        .ok_or_else(|| "this bot has no allowed folder (Settings ▸ Allowed folders)".to_string())?;
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("{}: `..` is not allowed", p.display()));
    }
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        first.join(p)
    };
    let real = real_path(&full);
    for f in folders {
        if real.starts_with(real_path(f)) {
            return Ok(full);
        }
    }
    Err(format!(
        "{} is outside this bot's allowed folders",
        full.display()
    ))
}

/// Canonical form of the deepest existing ancestor plus the rest.
fn real_path(p: &Path) -> PathBuf {
    let mut base = p.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(c) = base.canonicalize() {
            let mut out = c;
            for r in rest.iter().rev() {
                out.push(r);
            }
            return out;
        }
        match (base.file_name().map(|s| s.to_os_string()), base.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                base = parent.to_path_buf();
            }
            _ => return p.to_path_buf(),
        }
    }
}

// ---------------------------------------------------------------- plans

/// One proposed step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    WriteFile { path: PathBuf, body: String },
    Mkdir { path: PathBuf },
    Move { from: PathBuf, to: PathBuf },
    Copy { from: PathBuf, to: PathBuf },
    /// A shell command line (shown verbatim on the card).
    Run { command: String, cwd: Option<PathBuf>, timeout_s: u64 },
    /// A sort plan from [`lillith_core::access::organize::suggest_sort`].
    Organize { plan: OrganizePlan },
    Scaffold { dir: PathBuf, language: Language, template: String },
    Build { dir: PathBuf, language: Language },
}

impl Step {
    pub fn op(&self) -> Op {
        match self {
            Step::WriteFile { .. } => Op::WriteFile,
            Step::Mkdir { .. } => Op::Mkdir,
            Step::Move { .. } | Step::Organize { .. } => Op::Move,
            Step::Copy { .. } => Op::Copy,
            Step::Run { .. } => Op::Run,
            Step::Scaffold { .. } => Op::Scaffold,
            Step::Build { .. } => Op::Build,
        }
    }

    /// Paths the step touches (all must pass [`confine`]).
    pub fn paths(&self) -> Vec<&Path> {
        match self {
            Step::WriteFile { path, .. } | Step::Mkdir { path } => vec![path],
            Step::Move { from, to } | Step::Copy { from, to } => vec![from, to],
            Step::Run { cwd, .. } => cwd.iter().map(|p| p.as_path()).collect(),
            Step::Organize { plan } => plan.ops.iter().flat_map(|o| o.paths()).collect(),
            Step::Scaffold { dir, .. } | Step::Build { dir, .. } => vec![dir],
        }
    }

    /// The file or folder a successful step produced (for the pane's
    /// "files produced" list).
    pub fn produces(&self) -> Option<PathBuf> {
        match self {
            Step::WriteFile { path, .. } | Step::Mkdir { path } => Some(path.clone()),
            Step::Move { to, .. } | Step::Copy { to, .. } => Some(to.clone()),
            Step::Scaffold { dir, .. } => Some(dir.clone()),
            _ => None,
        }
    }

    /// The Soul Contract action for this step (`journal` = the organize
    /// undo journal folder).
    pub fn action(&self, journal: &Path) -> PendingAction {
        let org = |ops: Vec<OrganizeOp>| PendingAction::Organize {
            plan: OrganizePlan { ops },
            journal: journal.to_path_buf(),
        };
        match self.clone() {
            Step::WriteFile { path, body } => PendingAction::WriteFile { path, body },
            Step::Mkdir { path } => org(vec![OrganizeOp::Mkdir { path }]),
            Step::Move { from, to } => org(vec![OrganizeOp::Move { from, to }]),
            Step::Copy { from, to } => org(vec![OrganizeOp::Copy { from, to }]),
            Step::Organize { plan } => PendingAction::Organize {
                plan,
                journal: journal.to_path_buf(),
            },
            Step::Run {
                command,
                cwd,
                timeout_s,
            } => PendingAction::RunCommand {
                command,
                cwd,
                timeout_s,
            },
            Step::Scaffold {
                dir,
                language,
                template,
            } => PendingAction::ScaffoldProject {
                dir,
                language,
                template,
            },
            Step::Build { dir, language } => PendingAction::BuildProject { dir, language },
        }
    }
}

/// What a bot came back with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// The bot's own words for Ko.
    pub say: String,
    pub steps: Vec<Step>,
    /// Findings / notes shown in the pane (e.g. Andras' report summary).
    pub notes: Vec<String>,
}

/// Checks every step against the bot's kind and folders. Refused steps are
/// dropped with a reason.
pub fn vet(bot: &BotDef, plan: Plan) -> (Plan, Vec<String>) {
    let folders = bot.resolved_folders();
    let mut refused = Vec::new();
    let mut keep = Vec::new();
    for s in plan.steps {
        if !bot.kind.allows(s.op()) {
            refused.push(format!(
                "refused a {:?} step: a {} doesn't do that",
                s.op(),
                bot.kind.role()
            ));
            continue;
        }
        if let Some(e) = s.paths().iter().find_map(|p| confine(&folders, p).err()) {
            refused.push(format!("refused a step: {e}"));
            continue;
        }
        keep.push(s);
    }
    (
        Plan {
            steps: keep,
            ..plan
        },
        refused,
    )
}

/// The first balanced `{...}` JSON object in `text` (models like to wrap
/// it in prose or ``` fences).
pub fn extract_json(text: &str) -> Option<Value> {
    let bytes = text.as_bytes();
    let mut start = 0;
    while let Some(off) = text[start..].find('{') {
        let s = start + off;
        let (mut depth, mut in_str, mut esc) = (0i32, false, false);
        for (i, &b) in bytes.iter().enumerate().skip(s) {
            if in_str {
                match b {
                    _ if esc => esc = false,
                    b'\\' => esc = true,
                    b'"' => in_str = false,
                    _ => {}
                }
                continue;
            }
            match b {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        if let Ok(v) = serde_json::from_str::<Value>(&text[s..=i]) {
                            return Some(v);
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
        start = s + 1;
    }
    None
}

/// Parses the model's plan (`{"say": "...", "steps": [{"op": ...}]}`).
/// Paths stay as given (relative = the bot's first folder, see [`confine`]).
pub fn parse_model_plan(text: &str, base: &Path) -> Option<Plan> {
    let v = extract_json(text)?;
    let say = v
        .get("say")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut steps = Vec::new();
    let p = |s: &Value, k: &str| -> Option<PathBuf> {
        let t = s.get(k)?.as_str()?.trim();
        (!t.is_empty()).then(|| {
            let p = PathBuf::from(t);
            if p.is_absolute() {
                p
            } else {
                base.join(p)
            }
        })
    };
    for s in v.get("steps").and_then(Value::as_array).into_iter().flatten() {
        let op = s.get("op").and_then(Value::as_str).unwrap_or("");
        let step = match op {
            "write_file" => p(s, "path").map(|path| Step::WriteFile {
                path,
                body: s
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            }),
            "mkdir" => p(s, "path").map(|path| Step::Mkdir { path }),
            "move" => p(s, "from").zip(p(s, "to")).map(|(from, to)| Step::Move { from, to }),
            "copy" => p(s, "from").zip(p(s, "to")).map(|(from, to)| Step::Copy { from, to }),
            "run" => s
                .get("command")
                .and_then(Value::as_str)
                .filter(|c| !c.trim().is_empty())
                .map(|c| Step::Run {
                    command: c.trim().to_string(),
                    cwd: p(s, "cwd").or_else(|| Some(base.to_path_buf())),
                    timeout_s: 600,
                }),
            "scaffold" | "build" => {
                let lang = s
                    .get("language")
                    .and_then(Value::as_str)
                    .and_then(Language::parse)
                    .unwrap_or(Language::Rust);
                p(s, "dir").map(|dir| {
                    if op == "scaffold" {
                        Step::Scaffold {
                            dir,
                            language: lang,
                            template: s
                                .get("template")
                                .and_then(Value::as_str)
                                .filter(|t| lang.has_template(t))
                                .unwrap_or(lang.default_template())
                                .to_string(),
                        }
                    } else {
                        Step::Build {
                            dir,
                            language: lang,
                        }
                    }
                })
            }
            _ => None,
        };
        steps.extend(step);
    }
    Some(Plan {
        say,
        steps,
        notes: Vec::new(),
    })
}

/// The system prompt a bot's model call uses (plan-only, JSON out).
pub fn system_prompt(bot: &BotDef, base: &Path) -> String {
    let ops = [
        (Op::WriteFile, r#"{"op":"write_file","path":"rel/path","body":"full file text"}"#),
        (Op::Mkdir, r#"{"op":"mkdir","path":"rel/dir"}"#),
        (Op::Move, r#"{"op":"move","from":"a","to":"b"}"#),
        (Op::Copy, r#"{"op":"copy","from":"a","to":"b"}"#),
        (Op::Run, r#"{"op":"run","command":"shell command","cwd":"rel/dir"}"#),
        (Op::Scaffold, r#"{"op":"scaffold","dir":"name","language":"rust|python|fish"}"#),
        (Op::Build, r#"{"op":"build","dir":"name","language":"rust|python|fish"}"#),
    ];
    let allowed: Vec<&str> = ops
        .iter()
        .filter(|(o, _)| bot.kind.allows(*o))
        .map(|(_, s)| *s)
        .collect();
    format!(
        "You are {name}, one of Lillith's local helper bots: {role}. {desc}\n\
         You work only inside {base} and only on this computer; you have no internet. \
         You never act yourself: you propose steps and Ko approves each one with a click. \
         Never propose deleting anything, sudo, network downloads, or touching other people's systems.\n\
         Reply with ONE JSON object and nothing else: \
         {{\"say\": \"one or two friendly sentences for Ko\", \"steps\": [ ... ]}} \
         where each step is one of:\n{steps}\n\
         Paths are relative to {base}. Keep it to at most 8 steps.",
        name = bot.name,
        role = bot.kind.role(),
        desc = bot.description,
        base = base.display(),
        steps = allowed.join("\n"),
    )
}

// ---------------------------------------------------------------- status

/// What a bot is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BotStatus {
    #[default]
    Idle,
    Working,
    NeedsApproval,
    Done,
    Failed,
    Stopped,
    Off,
}

impl BotStatus {
    pub fn label(self) -> &'static str {
        match self {
            BotStatus::Idle => "idle",
            BotStatus::Working => "working",
            BotStatus::NeedsApproval => "needs approval",
            BotStatus::Done => "done",
            BotStatus::Failed => "failed",
            BotStatus::Stopped => "stopped",
            BotStatus::Off => "off",
        }
    }

    pub fn color(self) -> lillith_theme::palette::Rgba {
        use lillith_theme::palette as pal;
        match self {
            BotStatus::Idle | BotStatus::Off => pal::TEXT_MUTE,
            BotStatus::Working => pal::NEON_BLUE,
            BotStatus::NeedsApproval => pal::EMBER_CORE,
            BotStatus::Done => pal::VIOLET,
            BotStatus::Failed | BotStatus::Stopped => pal::BLOOD_NEON,
        }
    }
}

// ---------------------------------------------------------------- dream hook

/// A dream-learning session as the Bots tab shows it (read-only).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DreamStatus {
    pub session: String,
    /// `running`, `ending`, `done`, `stopped`...
    pub state: String,
    pub minutes_left: u64,
    pub jobs_done: u32,
    pub jobs_total: u32,
    pub note: String,
}

/// Hook for Memory Engine's session-status API. The Bots tab only reads
/// it; dream sessions are started (Ko-gated, Tor-only) elsewhere.
pub trait DreamStatusSource: Send {
    /// The current or last session, `None` when there was none.
    fn status(&self) -> Option<DreamStatus>;
}

/// The default source until Memory Engine plugs in: no session.
pub struct NoDream;

impl DreamStatusSource for NoDream {
    fn status(&self) -> Option<DreamStatus> {
        None
    }
}

// ---------------------------------------------------------------- Ctrl-C

#[cfg(unix)]
static CTRL_C: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_signal(_: libc::c_int) {
    CTRL_C.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Ctrl-C / SIGTERM: fire the kill switch (stops every bot, build, import
/// and command started from this window), then exit 130. The handler only
/// sets a flag; a watcher thread does the work.
pub fn install_ctrlc_stop() {
    #[cfg(unix)]
    {
        let h = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        // SAFETY: the handler only stores to an atomic (async-signal-safe).
        unsafe {
            libc::signal(libc::SIGINT, h);
            libc::signal(libc::SIGTERM, h);
        }
        let _ = std::thread::Builder::new()
            .name("lillith-settings-ctrlc".into())
            .spawn(|| loop {
                if CTRL_C.load(std::sync::atomic::Ordering::SeqCst) {
                    let _ = lillith_core::access::kill_switch::stop_all();
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    std::process::exit(130);
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            });
    }
}

#[cfg(test)]
mod tests;
