//! User configuration (`~/.config/lillith/config.toml`).
//!
//! Keys mirror README-SP's config.ini names so existing tunables carry over.
//! TODO(M6): live reload from the egui settings panel.
//!
//! The overlay persists her on-screen position in the `[overlay]` table
//! ([`save_overlay`]); that write only replaces that table and keeps every
//! other key in the file.

use crate::spring::SpringParams;
use crate::state::BehaviorConfig;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Spring tunables with README-SP defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Springs {
    pub chest: SpringParams,
    pub pig: SpringParams,
    pub root: SpringParams,
    pub angle: SpringParams,
    pub scale: SpringParams,
    pub jiggle_gain: f32,
    pub chest_bounce_gain: f32,
    pub pigtail_sway_gain: f32,
    /// `[springs.bounce]`: Settings ▸ Motion ▸ Chest bounce (intensity and
    /// softness). With the physics driver it owns the chest spring's
    /// stiffness/damping; see [`crate::bounce`].
    pub bounce: crate::bounce::ChestBounce,
}

impl Default for Springs {
    fn default() -> Self {
        let p = |stiffness, damping| SpringParams { stiffness, damping };
        Self {
            chest: p(65.0, 9.5),
            pig: p(32.0, 6.5),
            root: p(48.0, 10.0),
            angle: p(42.0, 9.0),
            scale: p(55.0, 11.0),
            jiggle_gain: 0.6,
            chest_bounce_gain: 1.0,
            pigtail_sway_gain: 0.9,
            bounce: Default::default(),
        }
    }
}

impl Springs {
    /// The art's "flirty" spring preset (`spring_proposal.json`
    /// `mood_presets.flirty`, TASTEFUL_CHANGE): chest and pigtail damping a
    /// little lower (one more soft rebound), pigtail sway +0.05; stiffness,
    /// the other gains and clamps unchanged.
    pub fn flirty() -> Self {
        let mut s = Self::default();
        s.chest.damping = 8.5;
        s.pig.damping = 5.8;
        s.pigtail_sway_gain = 0.95;
        s
    }
}

/// How her secondary motion (chest, pigtails, hip) is produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JiggleMode {
    /// Baked-jiggle `<clip>_jiggle` art (engine chest/pigtail/hip springs
    /// off while one is drawn).
    Baked,
    /// Engine-clean clips with the engine springs.
    #[default]
    Springs,
    /// Engine-clean clips, chest/pigtail springs quiet (gains 0).
    Off,
}

impl JiggleMode {
    pub const ALL: [JiggleMode; 3] = [JiggleMode::Baked, JiggleMode::Springs, JiggleMode::Off];

    pub fn label(self) -> &'static str {
        match self {
            JiggleMode::Baked => "Baked jiggle",
            JiggleMode::Springs => "Engine springs",
            JiggleMode::Off => "Off",
        }
    }
}

/// Movement mode (GH#1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MoveMode {
    /// Default: she strolls calmly along the bottom of the screen between
    /// idle poses (G toggles Stay).
    #[default]
    Wander,
    Follow,
    /// She stays where you put her.
    Stay,
}

/// Where the overlay sits (M1). Both backends use the same numbers: the
/// distance in px from the left edge and from the bottom edge of the output
/// (layer-shell margins on Wayland; converted to window x/y on X11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayPos {
    pub margin_left: i32,
    pub margin_bottom: i32,
    /// Output (monitor) name, e.g. `eDP-1`; `None` = compositor's choice.
    pub output: Option<String>,
    /// Before her first frame, check KDE Plasma's KWin for effects that blur
    /// every window (Better Blur DX "blur all", Force Blur, Blur Plus…) and
    /// exclude `lillith` from them, so she never shows up as a black box.
    /// If she can't be excluded, the effect is unloaded for this session
    /// only and you are told once how to make it permanent. Undo with
    /// `lillith fix-kwin --revert`.
    pub auto_fix_kwin: bool,
}

impl Default for OverlayPos {
    fn default() -> Self {
        Self {
            margin_left: 80,
            margin_bottom: 48,
            output: None,
            auto_fix_kwin: true,
        }
    }
}

/// System monitor tunables (`[monitor]`, M4). Thresholds default to the
/// `[thresholds]` numbers in `assets/rules.toml` (Python prototype parity);
/// rule `cooldown_s` values still apply on top inside the brain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorConfig {
    /// Master switch (`lillith --no-monitor` overrides it).
    pub enabled: bool,
    /// procfs/sysfs sampling period (s).
    pub poll_s: f32,
    /// Minimum seconds between two emissions of the same event by the
    /// monitor itself (anti-spam; `user.activity` has its own period).
    pub event_cooldown_s: f32,
    /// `cpu.high` when total CPU ≥ this for `cpu_high_sustain_s`.
    pub cpu_high_percent: f32,
    pub cpu_high_sustain_s: f32,
    /// Re-arms once CPU has stayed below this for `cpu_rearm_sustain_s`.
    pub cpu_rearm_percent: f32,
    pub cpu_rearm_sustain_s: f32,
    /// `ram.high` (MemTotal − MemAvailable) ≥ this for `ram_high_sustain_s`.
    pub ram_high_percent: f32,
    pub ram_high_sustain_s: f32,
    pub ram_rearm_percent: f32,
    /// `battery.low` at or below this while discharging.
    pub battery_low_percent: f32,
    /// Re-arms at or above this, or as soon as AC is plugged in.
    pub battery_rearm_percent: f32,
    /// `net.lost` / `net.restored` only after the new state held this long.
    pub net_debounce_s: f32,
    /// `brightness.changed` when the backlight moved at least this many points.
    pub brightness_min_delta_percent: f32,
    /// Watch removable block devices (`usb.added` / `usb.removed`).
    pub usb: bool,
    /// Detect VPN tunnels (tun/wg/ppp interfaces) → `vpn.up`.
    pub vpn: bool,
    /// Desktop-wide idle detection (ext-idle-notify on Wayland, XScreenSaver on X11).
    pub idle: bool,
    /// Input within this many seconds counts as "the user is active".
    pub activity_window_s: f32,
    /// While active, `user.activity` is sent at most this often (keeps her awake).
    pub activity_every_s: f32,
    /// Idle this long = away; the next input sends `user.returned`.
    pub away_after_s: f32,
    /// inotify on Trash / Screenshots / Downloads.
    pub files: bool,
    /// Session-bus notification monitor (`notify.received`, `calendar.reminder`, `update.available`).
    pub notifications: bool,
    /// Serve `org.lillith.Bridge` for the KWin script (`window.*`).
    pub kwin_bridge: bool,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_s: 2.0,
            event_cooldown_s: 20.0,
            cpu_high_percent: 72.0,
            cpu_high_sustain_s: 10.0,
            cpu_rearm_percent: 62.0,
            cpu_rearm_sustain_s: 10.0,
            ram_high_percent: 85.0,
            ram_high_sustain_s: 10.0,
            ram_rearm_percent: 80.0,
            battery_low_percent: 15.0,
            battery_rearm_percent: 20.0,
            net_debounce_s: 4.0,
            brightness_min_delta_percent: 2.0,
            usb: true,
            vpn: true,
            idle: true,
            activity_window_s: 10.0,
            activity_every_s: 30.0,
            away_after_s: 120.0,
            files: true,
            notifications: true,
            kwin_bridge: true,
        }
    }
}

/// Optional local `llama-server` the app starts itself (`[chat.server]`).
/// Both `binary` and `model_path` must be set; otherwise nothing is spawned
/// and the app only talks to whatever already serves [`ChatConfig::endpoint`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlamaServerConfig {
    /// Path to the `llama-server` executable (empty = don't spawn).
    pub binary: String,
    /// Path to the `.gguf` model file (empty = don't spawn).
    pub model_path: String,
    /// Context window (`-c`).
    pub ctx_size: u32,
    /// CPU threads (`-t`); 0 = llama-server's default.
    pub threads: u32,
    /// Layers to offload to the GPU (`-ngl`); `None` = llama-server's default.
    pub gpu_layers: Option<i32>,
    /// Extra command-line arguments, passed through verbatim.
    pub extra_args: Vec<String>,
    /// How long to wait for `/health` after spawning (s).
    pub startup_timeout_s: f32,
}

impl Default for LlamaServerConfig {
    fn default() -> Self {
        Self {
            binary: String::new(),
            model_path: String::new(),
            ctx_size: 4096,
            threads: 0,
            gpu_layers: None,
            extra_args: Vec::new(),
            startup_timeout_s: 90.0,
        }
    }
}

impl LlamaServerConfig {
    /// `true` when both paths are set, i.e. the app should spawn the server.
    pub fn should_spawn(&self) -> bool {
        !self.binary.trim().is_empty() && !self.model_path.trim().is_empty()
    }
}

/// Click-to-chat with a local llama.cpp `llama-server` (`[chat]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatConfig {
    /// Master switch for the chat bubble.
    pub enabled: bool,
    /// A plain left click on her opens the bubble (`Y`/`Enter` always do).
    pub open_on_click: bool,
    /// Base URL of the OpenAI-compatible server (`/v1/chat/completions`
    /// is appended).
    pub endpoint: String,
    /// `model` field of the request (llama-server serves whatever it
    /// loaded and ignores it; other OpenAI-compatible servers need it).
    pub model: String,
    /// System prompt; empty = Lillith's built-in persona.
    pub system_prompt: String,
    pub temperature: f32,
    /// Reply length cap (`max_tokens`).
    pub max_tokens: u32,
    /// How many past user/assistant turns are sent with each message.
    pub history_turns: usize,
    /// Whole-request timeout (s); the connect timeout is fixed at 3 s.
    pub timeout_s: f32,
    /// Keep the conversation across bubble openings and restarts
    /// (`~/.local/share/lillith/chat_history.json`).
    pub save_history: bool,
    /// Optional `Authorization: Bearer` key (llama-server `--api-key`).
    pub api_key: String,
    /// Show her replies in a small speech bubble over her head on the
    /// overlay (the overlay window grows upward to make room).
    pub overlay_speech: bool,
    /// Draw the baked-jiggle talk clips (`talk_loop*_jiggle`) while she
    /// speaks when baked-jiggle mode is on (`baked_jiggle` /
    /// `--baked-jiggle`); `false` keeps the engine-clean talk clips.
    pub talk_jiggle: bool,
    /// Optional self-spawned server.
    pub server: LlamaServerConfig,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            open_on_click: true,
            endpoint: "http://127.0.0.1:8080".into(),
            model: "local".into(),
            system_prompt: String::new(),
            // Matches lillith_setup::modelfile::LILY_TEMPERATURE (pinned by a
            // test in lillith-mind, which depends on both crates).
            temperature: 0.85,
            max_tokens: 320,
            history_turns: 12,
            timeout_s: 120.0,
            save_history: true,
            api_key: String::new(),
            overlay_speech: true,
            talk_jiggle: true,
            server: LlamaServerConfig::default(),
        }
    }
}

/// Animation pacing (`[anim]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimConfig {
    /// Global clip playback speed (frame timing only; physics and timers
    /// stay real-time). 1.0 = as authored, 0.7 = 30% slower.
    pub playback_speed: f32,
    /// Lip-flap pace while she talks: 1.0 = the original fast pace; the
    /// default 0.62 changes mouth shapes ~38% less often and holds the
    /// end-of-reply smile ~1.5 s.
    pub talk_speed: f32,
    /// Smooth mode for her sprite clips: cross-fade into the next frame
    /// toward the end of each frame instead of hard steps.
    pub smooth_frames: bool,
    /// Fraction (0..1) of each frame spent cross-fading (smooth mode).
    pub crossfade: f32,
}

impl Default for AnimConfig {
    fn default() -> Self {
        Self {
            playback_speed: 0.7,
            talk_speed: 0.62,
            smooth_frames: true,
            crossfade: 0.5,
        }
    }
}

/// Default budget (in rough tokens) for knowledge/skill material injected
/// into a chat turn (see [`MindConfig::max_context_tokens`]).
pub const DEFAULT_MIND_CONTEXT_TOKENS: usize = 800;

/// Default cap (bytes) on a single tool-driven file read (see
/// [`MindConfig::max_read_bytes`]). Keeps a huge file from flooding the
/// prompt or the audit trail.
pub const DEFAULT_MIND_MAX_READ_BYTES: usize = 64 * 1024;

/// Knowledge + skills folders Lillith may read, and the skill-authoring
/// gate (`[mind]`, Phase 1 of the companion plan).
///
/// Everything here is local-only: plain files under her data dir. On each
/// chat turn she indexes the markdown/text in [`MindConfig::knowledge_dir`]
/// and [`MindConfig::skills_dir`] and injects the most relevant snippets
/// into the prompt, capped at [`MindConfig::max_context_tokens`]. She can
/// *draft* a new skill, but it is only written after a Soul Contract
/// confirmation (see [`crate::soul_contract`]); nothing here writes on its
/// own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MindConfig {
    /// Master switch for knowledge/skills injection and skill authoring.
    pub enabled: bool,
    /// Folder of reference knowledge (`.md`/`.markdown`/`.txt`). Empty =
    /// the default `<data>/lillith/knowledge`.
    pub knowledge_dir: String,
    /// Folder of how-to skills she can read and (gated) write. Empty =
    /// the default `<data>/lillith/skills`.
    pub skills_dir: String,
    /// Rough token budget for injected material per turn (~4 chars/token).
    pub max_context_tokens: usize,
    /// Extra folders Lillith's scoped file/tool API may touch, on top of the
    /// always-allowed config dir, `knowledge_dir` and `skills_dir` (Phase 2).
    /// Each entry is a directory root; `~` is expanded. Reads inside these
    /// roots run without a popup (but are audited); writes always need a Soul
    /// Contract Yes. Example: `["~/Projects"]`.
    pub allow_paths: Vec<String>,
    /// Whether she may propose shell commands at all. Even when `true`, every
    /// shell run still needs an explicit Soul Contract Yes (Phase 2).
    pub allow_shell: bool,
    /// Cap (bytes) on a single tool-driven file read.
    pub max_read_bytes: usize,
    /// Roots that re-open the built-in deny list (secrets dirs, `/etc`) for
    /// the tool API. Use sparingly; `~` is expanded (Phase 2).
    pub unlock_paths: Vec<String>,
    /// Default-on access to the user's files, programs and commands
    /// (`[mind.access]`).
    pub access: AccessConfig,
}

/// Default roots for `[mind.access] roots`: home plus the usual mount points.
pub const DEFAULT_ACCESS_ROOTS: &[&str] = &["~", "/mnt", "/media", "/run/media"];

/// `[mind.access]`: what Lillith may do on the machine without being given
/// each folder by hand. Everything is on by default; any switch can be
/// turned off here or in the settings GUI (Access tab).
///
/// - `files`: list / read / search / find by name under `roots` (secrets dirs
///   stay denied; `/proc`, `/sys`, `/dev` are never walked).
/// - `organize`: move / rename / copy / make folders under `roots`. Never
///   overwrites (auto-renames on conflict), journals every batch for
///   `lillith undo-organize`.
/// - `programs`: list installed apps, check if a program is installed, open
///   an app (optionally with files).
/// - `commands`: run CLI commands to finish a task.
///
/// Every change and every program run is proposed as one Soul Contract plan
/// (numbered items, full paths, sizes, diffs, exact commands) that Ko
/// approves in full, in part, or declines; nothing touches disk or launches
/// before that. Reads, listing, searching and "is it installed?" are free.
/// Deletes in a plan additionally need the typed phrase (Trash by default);
/// there is deliberately no auto-approve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccessConfig {
    pub files: bool,
    pub organize: bool,
    pub programs: bool,
    pub commands: bool,
    /// List / read / use the skills in `[mind] skills_dir` on demand.
    pub skills: bool,
    /// On startup, copy a `skills` / `lillith-skills` folder found on the
    /// desktop into `skills_dir` (never overwriting; the original stays).
    pub import_desktop_skills: bool,
    /// Directory roots she may browse and organize (`~` expanded).
    pub roots: Vec<String>,
    /// Seconds a command may run before it is stopped.
    pub command_timeout_s: u64,
    /// `[mind.access.windows]`: cmd / PowerShell and the admin helper.
    pub windows: WindowsAccessConfig,
}

/// `[mind.access.windows]`: Windows-only command access. Every command
/// (elevated or not) is still a Soul Contract plan item Ko approves; these
/// keys only decide whether admin runs are possible and how often Windows
/// shows its UAC prompt.
///
/// - `admin`: allow commands to run elevated through the `lillith-admin`
///   helper (each one still shows an `ELEVATED (admin)` badge on the card).
/// - `admin_prompt`: `"per-session"` (one UAC prompt, the helper stays up
///   until Lillith exits, the safeword, or `admin_idle_timeout_min` without
///   use) or `"per-command"` (a UAC prompt for every elevated command).
/// - `admin_idle_timeout_min`: idle minutes before the helper exits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowsAccessConfig {
    pub admin: bool,
    pub admin_prompt: String,
    pub admin_idle_timeout_min: u64,
}

impl Default for WindowsAccessConfig {
    fn default() -> Self {
        Self {
            admin: true,
            admin_prompt: "per-session".into(),
            admin_idle_timeout_min: 15,
        }
    }
}

impl WindowsAccessConfig {
    /// `true` for `per-command` (anything else is per-session).
    pub fn per_command(&self) -> bool {
        let p = self.admin_prompt.trim().to_ascii_lowercase();
        p == "per-command" || p == "per_command" || p == "command"
    }

    /// Idle timeout, at least one minute.
    pub fn idle_timeout_min(&self) -> u64 {
        self.admin_idle_timeout_min.max(1)
    }
}

impl Default for AccessConfig {
    fn default() -> Self {
        Self {
            files: true,
            organize: true,
            programs: true,
            commands: true,
            skills: true,
            import_desktop_skills: true,
            roots: DEFAULT_ACCESS_ROOTS.iter().map(|s| s.to_string()).collect(),
            command_timeout_s: 30,
            windows: WindowsAccessConfig::default(),
        }
    }
}

impl AccessConfig {
    /// `roots` with `~` expanded; absolute paths kept, blanks dropped.
    pub fn resolved_roots(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for r in &self.roots {
            let p = if r.trim_start().starts_with('~') {
                expand_tilde(r)
            } else if r.trim().is_empty() {
                None
            } else {
                Some(PathBuf::from(r.trim()))
            };
            if let Some(p) = p {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
        out
    }
}

impl Default for MindConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            knowledge_dir: String::new(),
            skills_dir: String::new(),
            max_context_tokens: DEFAULT_MIND_CONTEXT_TOKENS,
            allow_paths: Vec::new(),
            allow_shell: false,
            max_read_bytes: DEFAULT_MIND_MAX_READ_BYTES,
            unlock_paths: Vec::new(),
            access: AccessConfig::default(),
        }
    }
}

impl MindConfig {
    /// Resolved knowledge folder: the configured override, else the default
    /// `<data>/lillith/knowledge` (`None` only if no data dir can be found).
    pub fn resolved_knowledge_dir(&self) -> Option<PathBuf> {
        resolve_dir(&self.knowledge_dir, "knowledge")
    }

    /// Resolved skills folder: the configured override, else the default
    /// `<data>/lillith/skills`.
    pub fn resolved_skills_dir(&self) -> Option<PathBuf> {
        resolve_dir(&self.skills_dir, "skills")
    }

    /// Where skill writes and denials are logged (`<data>/lillith/audit.log`).
    pub fn audit_log_path(&self) -> Option<PathBuf> {
        data_dir().map(|d| d.join("audit.log"))
    }

    /// Clamped token budget (at least 1).
    pub fn context_budget(&self) -> usize {
        self.max_context_tokens.max(1)
    }

    /// Clamped read cap (at least 1 byte).
    pub fn read_cap(&self) -> usize {
        self.max_read_bytes.max(1)
    }

    /// The directory roots the tool API may touch: the config dir, the
    /// resolved knowledge and skills folders, plus every `allow_paths` entry
    /// (with `~` expanded). Deduplicated, order preserved.
    pub fn resolved_allow_roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = Vec::new();
        let mut push = |p: Option<PathBuf>| {
            if let Some(p) = p {
                if !roots.contains(&p) {
                    roots.push(p);
                }
            }
        };
        push(config_dir());
        push(self.resolved_knowledge_dir());
        push(self.resolved_skills_dir());
        for raw in &self.allow_paths {
            push(expand_tilde(raw));
        }
        if self.access.files {
            for r in self.access.resolved_roots() {
                push(Some(r));
            }
        }
        roots
    }

    /// Extra roots that re-open the built-in deny list (`unlock_paths`, with
    /// `~` expanded).
    pub fn resolved_unlock_roots(&self) -> Vec<PathBuf> {
        self.unlock_paths
            .iter()
            .filter_map(|r| expand_tilde(r))
            .collect()
    }
}

/// An override (used verbatim if non-empty) or `<data>/lillith/<name>`.
fn resolve_dir(override_path: &str, name: &str) -> Option<PathBuf> {
    let o = override_path.trim();
    if !o.is_empty() {
        return Some(PathBuf::from(o));
    }
    data_dir().map(|d| d.join(name))
}

/// `$XDG_DATA_HOME/lillith`, else `~/.local/share/lillith`.
pub fn data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("lillith"))
}

/// `$XDG_CONFIG_HOME/lillith`, else `~/.config/lillith`. This is the folder
/// `config.toml` lives in; the tool API always allows it.
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("lillith"))
}

/// Expands a leading `~` or `~/` to `$HOME` and trims surrounding whitespace.
/// An empty string (after trimming) yields `None`.
pub fn expand_tilde(raw: &str) -> Option<PathBuf> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    if t == "~" {
        return crate::access::home();
    }
    // `~\` is the Windows spelling.
    if let Some(rest) = t.strip_prefix("~/").or_else(|| t.strip_prefix("~\\")) {
        if let Some(home) = crate::access::home() {
            return Some(home.join(rest));
        }
    }
    Some(PathBuf::from(t))
}

/// `[build]`: the settings-GUI **Build** tab (Phase 3) — local, offline
/// project scaffolding and building ("vibe coding", but entirely on-box).
///
/// Everything the tab does is gated by the Soul Contract and confined to
/// [`BuildConfig::resolved_projects_dir`]. No network, no cloud: see
/// [`crate::build`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildConfig {
    /// Master switch for the Build tab.
    pub enabled: bool,
    /// Root every scaffolded project lives under. Empty = the default
    /// `~/Projects/lillith-builds`. `~` is expanded.
    pub projects_dir: String,
    /// Language preselected in the tab (`rust` / `python` / `fish`).
    pub default_language: String,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            projects_dir: String::new(),
            default_language: "rust".into(),
        }
    }
}

impl BuildConfig {
    /// The resolved projects root: the configured override, else the default
    /// `~/Projects/lillith-builds` (`None` only if `$HOME` is unset and no
    /// override is given).
    pub fn resolved_projects_dir(&self) -> Option<PathBuf> {
        let o = self.projects_dir.trim();
        if !o.is_empty() {
            return expand_tilde(o);
        }
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Projects/lillith-builds"))
    }

    /// The preselected language, defaulting to Rust on an unknown token.
    pub fn resolved_default_language(&self) -> crate::build::Language {
        crate::build::Language::parse(&self.default_language)
            .unwrap_or(crate::build::Language::Rust)
    }
}

/// `[avatar]`: custom (single-image) avatars, animated procedurally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AvatarConfig {
    /// Motion pace of a custom avatar (1 = default; lower = slower).
    pub pace: f32,
    /// Redraw rate (Hz) while a custom avatar is shown: it moves every
    /// frame, so a higher rate looks smoother (at some CPU cost).
    pub render_hz: f32,
}

impl Default for AvatarConfig {
    fn default() -> Self {
        Self {
            pace: 0.8,
            render_hz: 60.0,
        }
    }
}

/// Overlay scale used when config `scale` is left at 1.0 (its default):
/// ~220 px tall from the 400 px cells (Ko's choice).
pub const DEFAULT_OVERLAY_SCALE: f32 = 0.55;

/// Top-level config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub springs: Springs,
    pub mode: MoveMode,
    pub scale: f32,
    pub opacity: f32,
    pub physics_hz: f32,
    pub render_hz: f32,
    pub sprite_fps: f32,
    /// Legacy top-level override of `[behavior] pose_cycle_s` (unset = use
    /// `[behavior]`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pose_cycle_s: Option<f32>,
    /// Draw baked-jiggle `<clip>_jiggle` variants instead of the engine-clean
    /// clips (engine chest/pigtail/hip springs off while one is drawn).
    /// Superseded by `jiggle_mode` when that is set.
    pub baked_jiggle: bool,
    /// `baked` / `springs` / `off` (unset = from `baked_jiggle`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jiggle_mode: Option<JiggleMode>,
    /// Keep the overlay above other windows.
    pub always_on_top: bool,
    /// Clicks pass through her body to the desktop; only her head stays
    /// clickable (a grab handle for the menu and dragging).
    pub click_through: bool,
    /// Start Lillith with the desktop session (XDG autostart entry).
    pub autostart: bool,
    /// App icon (id from `branding/icons.toml`, e.g. `phantomcore-red`,
    /// `classic`): menu entry, desktop shortcut, Windows shortcuts. Settings
    /// ▸ Icon or `lillith icon <id>` change it; unknown ids mean the default.
    pub icon: String,
    pub avatar: AvatarConfig,
    pub muted: bool,
    pub voice_enabled: bool,
    pub llm_enabled: bool,
    /// `ollama` or `llama-server` (open question, PLAN.md §6 Q4).
    pub llm_backend: String,
    pub llm_url: String,
    pub llm_model: String,
    pub overlay: OverlayPos,
    /// M3 behaviour timings (`[behavior]`).
    pub behavior: BehaviorConfig,
    /// M4 system monitor (`[monitor]`).
    pub monitor: MonitorConfig,
    /// Click-to-chat with a local llama-server (`[chat]`).
    pub chat: ChatConfig,
    /// Animation pacing (`[anim]`).
    pub anim: AnimConfig,
    /// Knowledge + skills folders and the skill-authoring gate (`[mind]`).
    pub mind: MindConfig,
    /// Local project scaffolding/building (`[build]`, Phase 3).
    pub build: BuildConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            springs: Springs::default(),
            mode: MoveMode::default(),
            // A 300 px tall window from the 400 px cells.
            scale: 1.0,
            opacity: 1.0,
            physics_hz: 120.0,
            render_hz: 30.0,
            sprite_fps: 12.0,
            pose_cycle_s: None,
            baked_jiggle: false,
            jiggle_mode: None,
            always_on_top: true,
            click_through: false,
            autostart: false,
            icon: crate::icons::DEFAULT_ICON.into(),
            avatar: AvatarConfig::default(),
            muted: false,
            voice_enabled: true,
            llm_enabled: true,
            llm_backend: "ollama".into(),
            llm_url: "http://127.0.0.1:11434".into(),
            llm_model: "dolphin3".into(),
            overlay: OverlayPos::default(),
            behavior: BehaviorConfig::default(),
            monitor: MonitorConfig::default(),
            chat: ChatConfig::default(),
            anim: AnimConfig::default(),
            mind: MindConfig::default(),
            build: BuildConfig::default(),
        }
    }
}

impl Config {
    /// The overlay scale this config asks for: `scale = 1.0` (the default)
    /// means [`DEFAULT_OVERLAY_SCALE`]; any other positive value is used
    /// as is.
    pub fn overlay_scale(&self) -> f32 {
        let s = self.scale;
        if (s - 1.0).abs() < 1e-6 || !s.is_finite() || s <= 0.0 {
            DEFAULT_OVERLAY_SCALE
        } else {
            s
        }
    }

    /// Effective jiggle mode (`jiggle_mode`, else from `baked_jiggle`).
    pub fn jiggle(&self) -> JiggleMode {
        self.jiggle_mode.unwrap_or(if self.baked_jiggle {
            JiggleMode::Baked
        } else {
            JiggleMode::Springs
        })
    }

    /// The configured icon, or the default when `icon` is unknown.
    pub fn icon_info(&self) -> &'static crate::icons::IconInfo {
        crate::icons::resolve(Some(&self.icon))
    }

    /// Writes the whole config to `path` (atomically), keeping unknown
    /// top-level keys/tables already in the file.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let me = toml::Value::try_from(self)?;
        update_config(path, |table| {
            if let toml::Value::Table(t) = me {
                for (k, v) in t {
                    table.insert(k, v);
                }
            }
            if self.jiggle_mode.is_none() {
                table.remove("jiggle_mode");
            }
        })
    }

    /// Parses a config file; missing keys fall back to defaults.
    pub fn from_toml_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// `$XDG_CONFIG_HOME/lillith/config.toml`, else `~/.config/lillith/config.toml`
    /// (Windows: `%APPDATA%\lillith\config.toml`).
    pub fn default_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        #[cfg(windows)]
        let base = base.or_else(|| {
            std::env::var_os("APPDATA")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        });
        let base =
            base.or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("lillith").join("config.toml"))
    }

    /// Loads `path`; a missing file gives the defaults.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(Self::from_toml_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Config load/save errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config io: {0}")]
    Io(#[from] std::io::Error),
    #[error("config parse: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("config write: {0}")]
    Ser(#[from] toml::ser::Error),
}

/// Writes `pos` into the `[overlay]` table of `path`, creating the file and
/// its directory if needed. Other keys are kept (comments are not).
pub fn save_overlay(path: &Path, pos: &OverlayPos) -> Result<(), ConfigError> {
    update_config(path, |t| {
        let entry = t
            .entry("overlay")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !entry.is_table() {
            *entry = toml::Value::Table(toml::Table::new());
        }
        let o = entry.as_table_mut().expect("table");
        o.insert(
            "margin_left".into(),
            toml::Value::Integer(pos.margin_left.into()),
        );
        o.insert(
            "margin_bottom".into(),
            toml::Value::Integer(pos.margin_bottom.into()),
        );
        match &pos.output {
            Some(name) => {
                o.insert("output".into(), toml::Value::String(name.clone()));
            }
            None => {
                o.remove("output");
            }
        }
    })
}

/// Writes the top-level `scale` of `path` (the menu's Resize pick), keeping
/// the other keys.
pub fn save_scale(path: &Path, scale: f32) -> Result<(), ConfigError> {
    // Round to 2 decimals so 0.55 doesn't land as 0.550000011920929.
    let v = (scale as f64 * 100.0).round() / 100.0;
    update_config(path, |t| {
        t.insert("scale".into(), toml::Value::Float(v));
    })
}

/// Writes the top-level `icon` of `path`, keeping the other keys.
pub fn save_icon(path: &Path, id: &str) -> Result<(), ConfigError> {
    update_config(path, |t| {
        t.insert("icon".into(), toml::Value::String(id.to_string()));
    })
}

/// Read-modify-write of the config file as a TOML table (atomic rename;
/// creates the file and directory; comments are not kept).
pub fn update_config(path: &Path, f: impl FnOnce(&mut toml::Table)) -> Result<(), ConfigError> {
    let mut table: toml::Table = match std::fs::read_to_string(path) {
        Ok(s) => s.parse()?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => return Err(e.into()),
    };
    f(&mut table);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, toml::to_string_pretty(&table)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// `$XDG_CONFIG_HOME/autostart` (else `~/.config/autostart`).
pub fn autostart_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| d.join("autostart"))
}

/// Writes or removes the XDG autostart entry (`lillith.desktop` in `dir`)
/// that starts `exe` with the desktop session.
pub fn set_autostart(
    dir: &std::path::Path,
    exe: &std::path::Path,
    on: bool,
) -> std::io::Result<()> {
    let file = dir.join("lillith.desktop");
    if on {
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            &file,
            format!(
                "[Desktop Entry]\nType=Application\nName=Lillith\nComment=Desktop companion\nExec={}\nX-GNOME-Autostart-enabled=true\nTerminal=false\n",
                exe.display()
            ),
        )
    } else {
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_key_parses_defaults_and_saves() {
        assert_eq!(Config::default().icon, "phantomcore-red");
        let c = Config::from_toml_str("scale = 0.75\n").unwrap();
        assert_eq!(c.icon, crate::icons::DEFAULT_ICON);
        let c = Config::from_toml_str("icon = \"glitch\"\n[chat]\nenabled = true\n").unwrap();
        assert_eq!(c.icon, "glitch");
        assert_eq!(c.icon_info().name, "Glitch");
        let c = Config::from_toml_str("icon = \"no-such-icon\"\n").unwrap();
        assert_eq!(c.icon_info().id, crate::icons::DEFAULT_ICON);
        assert!(Config::from_toml_str("icon = 3\n").is_err());
        // the settings template carries the key
        assert!(crate::menu::settings_template().contains("icon = \"phantomcore-red\""));
        let dir = std::env::temp_dir().join(format!("lillith-cfg-icon-{}", std::process::id()));
        let p = dir.join("config.toml");
        save_scale(&p, 0.75).unwrap();
        save_icon(&p, "classic").unwrap();
        let back = Config::load(&p).unwrap();
        assert_eq!(back.icon, "classic");
        assert_eq!(back.scale, 0.75);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scale_one_means_the_055_default() {
        let mut c = Config::default();
        assert_eq!(c.scale, 1.0);
        assert_eq!(c.overlay_scale(), DEFAULT_OVERLAY_SCALE);
        assert_eq!(DEFAULT_OVERLAY_SCALE, 0.55);
        for (s, want) in [(0.75, 0.75), (1.25, 1.25), (0.45, 0.45)] {
            c.scale = s;
            assert_eq!(c.overlay_scale(), want);
        }
        for bad in [0.0, -1.0, f32::NAN] {
            c.scale = bad;
            assert_eq!(c.overlay_scale(), DEFAULT_OVERLAY_SCALE);
        }
    }

    #[test]
    fn save_scale_keeps_other_keys() {
        let dir = std::env::temp_dir().join(format!("lillith-cfg-scale-{}", std::process::id()));
        let p = dir.join("config.toml");
        save_overlay(
            &p,
            &OverlayPos {
                margin_left: 7,
                ..OverlayPos::default()
            },
        )
        .unwrap();
        save_scale(&p, 0.75).unwrap();
        let c = Config::load(&p).unwrap();
        assert_eq!(c.scale, 0.75);
        assert_eq!(c.overlay.margin_left, 7);
        save_scale(&p, 0.45).unwrap();
        assert_eq!(Config::load(&p).unwrap().scale, 0.45);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_file_gives_defaults() {
        let c = Config::from_toml_str("").unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn partial_override() {
        let c = Config::from_toml_str(
            "mode = \"stay\"\n[springs.chest]\nstiffness = 70.0\ndamping = 9.0\n",
        )
        .unwrap();
        assert_eq!(c.mode, MoveMode::Stay);
        assert_eq!(c.springs.chest.stiffness, 70.0);
        assert_eq!(c.springs.pig.stiffness, 32.0);
    }

    #[test]
    fn monitor_table() {
        let c =
            Config::from_toml_str("[monitor]\nenabled = false\ncpu_high_percent = 90.0\n").unwrap();
        assert!(!c.monitor.enabled);
        assert_eq!(c.monitor.cpu_high_percent, 90.0);
        assert_eq!(c.monitor.battery_low_percent, 15.0);
        assert!(c.monitor.cpu_rearm_percent < MonitorConfig::default().cpu_high_percent);
    }

    #[test]
    fn chat_table() {
        let c = Config::from_toml_str(
            "[chat]\nendpoint = \"http://127.0.0.1:9090\"\ntemperature = 0.5\n\
             [chat.server]\nbinary = \"/opt/llama-server\"\nmodel_path = \"/m/q.gguf\"\n",
        )
        .unwrap();
        assert_eq!(c.chat.endpoint, "http://127.0.0.1:9090");
        assert_eq!(c.chat.temperature, 0.5);
        assert_eq!(c.chat.history_turns, 12);
        assert!(c.chat.server.should_spawn());
        assert!(!ChatConfig::default().server.should_spawn());
        let a = Config::from_toml_str("[anim]\ntalk_speed = 1.0\n")
            .unwrap()
            .anim;
        assert_eq!((a.talk_speed, a.playback_speed), (1.0, 0.7));
        let d = ChatConfig::default();
        assert!(d.overlay_speech && d.talk_jiggle);
        let c = Config::from_toml_str("[chat]\noverlay_speech = false\ntalk_jiggle = false\n")
            .unwrap()
            .chat;
        assert!(!c.overlay_speech && !c.talk_jiggle);
        // Serialises (tables after plain keys).
        assert!(toml::to_string(&Config::default()).is_ok());
    }

    #[test]
    fn build_table_defaults_and_override() {
        use crate::build::Language;
        // Defaults: enabled, empty projects_dir (= ~/Projects/lillith-builds),
        // rust preselected. Old configs that omit [build] still parse.
        let d = Config::default().build;
        assert!(d.enabled);
        assert!(d.projects_dir.is_empty());
        assert_eq!(d.resolved_default_language(), Language::Rust);
        if let Some(root) = d.resolved_projects_dir() {
            assert!(root.ends_with("Projects/lillith-builds"), "{root:?}");
        }

        // Overrides parse and are used verbatim.
        let c = Config::from_toml_str(
            "[build]\nenabled = false\nprojects_dir = \"/srv/builds\"\ndefault_language = \"python\"\n",
        )
        .unwrap()
        .build;
        assert!(!c.enabled);
        assert_eq!(
            c.resolved_projects_dir(),
            Some(PathBuf::from("/srv/builds"))
        );
        assert_eq!(c.resolved_default_language(), Language::Python);

        // An unknown language falls back to Rust.
        let bad = BuildConfig {
            default_language: "klingon".into(),
            ..BuildConfig::default()
        };
        assert_eq!(bad.resolved_default_language(), Language::Rust);

        // A config with no [build] table still loads with build defaults.
        let none = Config::from_toml_str("mode = \"stay\"\n").unwrap();
        assert!(none.build.enabled);
    }

    #[test]
    fn mind_table_defaults_and_override() {
        // Defaults: enabled, empty (= default) dirs, ~800 token budget.
        let d = Config::default().mind;
        assert!(d.enabled);
        assert_eq!(d.max_context_tokens, DEFAULT_MIND_CONTEXT_TOKENS);
        assert!(d.knowledge_dir.is_empty() && d.skills_dir.is_empty());
        assert_eq!(d.context_budget(), DEFAULT_MIND_CONTEXT_TOKENS);

        // Empty budget is clamped to at least 1.
        let zeroed = MindConfig {
            max_context_tokens: 0,
            ..MindConfig::default()
        };
        assert_eq!(zeroed.context_budget(), 1);

        // Overrides parse and are used verbatim.
        let c = Config::from_toml_str(
            "[mind]\nenabled = false\nknowledge_dir = \"/srv/kb\"\nmax_context_tokens = 256\n",
        )
        .unwrap()
        .mind;
        assert!(!c.enabled);
        assert_eq!(c.max_context_tokens, 256);
        assert_eq!(c.resolved_knowledge_dir(), Some(PathBuf::from("/srv/kb")));
        // skills_dir left empty -> default under the data dir (ends with the
        // folder name when a data dir exists).
        if let Some(sk) = c.resolved_skills_dir() {
            assert!(sk.ends_with("lillith/skills"));
        }

        // Phase 2 tool-API defaults.
        assert!(d.allow_paths.is_empty());
        assert!(!d.allow_shell, "shell is off by default");
        assert_eq!(d.max_read_bytes, DEFAULT_MIND_MAX_READ_BYTES);
        assert!(d.unlock_paths.is_empty());
        assert_eq!(
            MindConfig {
                max_read_bytes: 0,
                ..MindConfig::default()
            }
            .read_cap(),
            1
        );

        // Phase 2 keys parse; `~` expands against the current HOME (read, never
        // mutated, so parallel tests are unaffected); old configs that omit
        // these keys still load with the defaults asserted above.
        let c2 = Config::from_toml_str(
            "[mind]\nallow_shell = true\nmax_read_bytes = 2048\n\
             allow_paths = [\"~/Projects\", \"/srv/data\"]\n\
             unlock_paths = [\"~/.secrets\"]\n",
        )
        .unwrap()
        .mind;
        assert!(c2.allow_shell);
        assert_eq!(c2.max_read_bytes, 2048);
        let roots = c2.resolved_allow_roots();
        assert!(roots.contains(&PathBuf::from("/srv/data")), "{roots:?}");
        // The config dir is always allowed.
        assert!(
            roots.iter().any(|r| r.ends_with("lillith")),
            "config dir allowed: {roots:?}"
        );
        if let Some(home) = std::env::var_os("HOME") {
            assert!(
                roots.contains(&PathBuf::from(&home).join("Projects")),
                "~ expands: {roots:?}"
            );
            assert_eq!(
                c2.resolved_unlock_roots(),
                vec![PathBuf::from(&home).join(".secrets")]
            );
        }
    }

    #[test]
    fn mind_default_dirs_follow_xdg_data_home() {
        // resolve against an explicit XDG_DATA_HOME so the test is hermetic.
        let tmp = std::env::temp_dir().join(format!("lillith-mind-xdg-{}", std::process::id()));
        std::env::set_var("XDG_DATA_HOME", &tmp);
        let c = MindConfig::default();
        assert_eq!(
            c.resolved_knowledge_dir(),
            Some(tmp.join("lillith/knowledge"))
        );
        assert_eq!(c.resolved_skills_dir(), Some(tmp.join("lillith/skills")));
        assert_eq!(c.audit_log_path(), Some(tmp.join("lillith/audit.log")));
        std::env::remove_var("XDG_DATA_HOME");
    }

    #[test]
    fn overlay_position_round_trip_keeps_other_keys() {
        let dir = std::env::temp_dir().join(format!("lillith-cfg-{}", std::process::id()));
        let path = dir.join("sub/config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        // Missing file -> defaults.
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "mode = \"stay\"\nscale = 0.8\n").unwrap();
        let pos = OverlayPos {
            margin_left: 1234,
            margin_bottom: 7,
            output: Some("eDP-1".into()),
            ..OverlayPos::default()
        };
        save_overlay(&path, &pos).unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(c.overlay, pos);
        assert_eq!(c.mode, MoveMode::Stay);
        assert_eq!(c.scale, 0.8);
        // Second save replaces only the table.
        let pos2 = OverlayPos {
            margin_left: -3,
            ..OverlayPos::default()
        };
        save_overlay(&path, &pos2).unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(c.overlay, pos2);
        assert_eq!(c.scale, 0.8);
        // A drag-save keeps Ko's other [overlay] keys (auto_fix_kwin).
        assert!(c.overlay.auto_fix_kwin, "defaults to on");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("auto_fix_kwin"), "default not written");
        std::fs::write(
            &path,
            format!("{text}\n").replace("[overlay]", "[overlay]\nauto_fix_kwin = false"),
        )
        .unwrap();
        save_overlay(&path, &pos).unwrap();
        let c = Config::load(&path).unwrap();
        assert!(!c.overlay.auto_fix_kwin, "kept across a position save");
        assert_eq!(c.overlay.margin_left, 1234);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn autostart_entry() {
        let d = std::env::temp_dir().join(format!("lillith-autostart-{}", std::process::id()));
        set_autostart(&d, Path::new("/usr/bin/lillith"), true).unwrap();
        let s = std::fs::read_to_string(d.join("lillith.desktop")).unwrap();
        assert!(s.contains("Exec=/usr/bin/lillith"));
        set_autostart(&d, Path::new("/usr/bin/lillith"), false).unwrap();
        assert!(!d.join("lillith.desktop").exists());
        set_autostart(&d, Path::new("/usr/bin/lillith"), false).unwrap();
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn windows_access_keys_parse_with_defaults() {
        let c: Config = toml::from_str(
            "[mind.access.windows]\nadmin_prompt = \"per-command\"\nadmin_idle_timeout_min = 0\n",
        )
        .unwrap();
        let w = &c.mind.access.windows;
        assert!(w.admin && w.per_command());
        assert_eq!(w.idle_timeout_min(), 1);
        let d = WindowsAccessConfig::default();
        assert!(!d.per_command() && d.admin_prompt == "per-session");
        assert_eq!(d.idle_timeout_min(), 15);
    }

    #[test]
    fn settings_fields_round_trip() {
        let mut c = Config::default();
        assert!(c.anim.smooth_frames && c.always_on_top && !c.click_through);
        assert_eq!(c.jiggle(), JiggleMode::Springs);
        c.baked_jiggle = true;
        assert_eq!(c.jiggle(), JiggleMode::Baked);
        c.jiggle_mode = Some(JiggleMode::Off);
        c.springs = Springs::flirty();
        c.avatar.pace = 0.6;
        let d = std::env::temp_dir().join(format!("lillith-cfg-save-{}", std::process::id()));
        let p = d.join("config.toml");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(&p, "custom_key = 3\n").unwrap();
        c.save(&p).unwrap();
        let back = Config::load(&p).unwrap();
        assert_eq!(back.jiggle(), JiggleMode::Off);
        assert_eq!(back.springs, Springs::flirty());
        assert!((back.avatar.pace - 0.6).abs() < 1e-6);
        assert!(std::fs::read_to_string(&p).unwrap().contains("custom_key"));
        save_scale(&p, 1.3).unwrap();
        assert!((Config::load(&p).unwrap().scale - 1.3).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(d);
    }
}
