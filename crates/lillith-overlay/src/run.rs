//! Backend dispatch plus the [`Controller`] both backends drive: stage
//! (animator + renderer + mask), drag, placement, position persistence and
//! the M3 behaviour [`Brain`] (clicks, hover, keys, drag, moods, sleep/wake,
//! Wander/Stay, pose cycle).

use crate::backend::Backend;
use crate::chat::{ChatAnchor, ChatHook, ClickGate, Talk};
use crate::drag::{Drag, Placement, Release};
use crate::gravity::{self, FallStep, Frame, Gravity, LandBlend, Mode, PerchCheck, Sit};
use crate::menu_popup::{Fonts, MenuHit, MenuKey, MenuPopup};
use crate::settings_window::{Launch, SettingsWindow};
use crate::stage::{Changed, Stage, StageError};
use crate::ui::{Command, PEEK_SCALE};
use lillith_anim::animator::{AnimatorConfig, Dir};
use lillith_anim::atlas::{Clip, Manifest};
use lillith_anim::avatar::AvatarStore;
use lillith_core::config::{save_overlay, MoveMode, OverlayPos};
use lillith_core::config::{Config, JiggleMode};
use lillith_core::desktop::{Desktop, Landing, Rect};
use lillith_core::interaction::Button;
use lillith_core::menu::{
    on_menu_action, settings_template, MenuAction, MenuEffect, MenuExtras, MenuPage, MenuPick,
    MenuState, OutfitChoice, PoseGroup,
};
use lillith_core::rules::RuleSet;
use lillith_core::seasons::{MonthDay, Outfit, Seasons};
use lillith_core::state::{Action, BehaviorConfig, Brain, Input, Key, Phase};
use lillith_core::tease::{fit_peak, keep_on_screen, Tease, TeaseCue, TeasePhase, TeasePlan};
use lillith_core::Event;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Events from the system monitor (`lillith-sense`) and other async
/// sources, drained into the brain once per frame. Shared so the Wayland →
/// X11 fallback keeps the same feed.
pub type EventFeed = Arc<Mutex<Receiver<Event>>>;

/// Most feed events applied per frame (a burst cannot stall rendering).
pub const MAX_EVENTS_PER_TICK: usize = 16;

/// Receives every action batch the [`Controller`] applies (input, tick,
/// event and menu batches), e.g. the app's `lillith_voice::Voice`
/// (`Action::Say` speaks, `Action::Muted` mutes). Called on the overlay
/// thread right before the batch is applied, so it must not block: queue
/// the work and return.
pub trait ActionSink {
    fn on_actions(&mut self, actions: &[Action]);
    /// A finished chat reply (`chat.end` detail): read it aloud when the
    /// speakers are on. Same contract: queue and return.
    fn on_chat_reply(&mut self, _reply: &str) {}
    /// "Speak out loud (speakers)" state; `None` = no voice (the menu item
    /// is left out).
    fn speaker(&self) -> Option<bool> {
        None
    }
    /// Turns the speakers on/off now (and persists it).
    fn set_speaker(&mut self, _on: bool) {}
    /// config.toml changed (Settings saved): re-read what applies live.
    fn config_changed(&mut self, _path: &std::path::Path) {}
    /// Voice-daemon lip-sync lines that arrived since the last frame
    /// (appended to `out`, never blocks). Returns `true` if the daemon
    /// went away (drop any audio-driven mouth).
    fn lipsync_lines(&mut self, _out: &mut Vec<String>) -> bool {
        false
    }
    /// Cut her voice now: barge-in (`chat.barge_in`, the user started
    /// talking) or the safeword (`chat.safeword`). `reason` is
    /// `barge_in` / `safeword`.
    fn interrupt(&mut self, _reason: &str) {}
}

/// The user started push-to-talk in the bubble: her voice stops.
pub const CHAT_BARGE_IN: &str = "chat.barge_in";
/// The bubble heard the safeword (voice or typed): everything stops.
pub const CHAT_SAFEWORD: &str = "chat.safeword";
/// Talk clip held after speech once the voice daemon is in use (s).
pub const DAEMON_TALK_HOLD_S: f32 = 1.0;

/// Shared [`ActionSink`] handle (cloneable so the Wayland → X11 fallback
/// keeps the same voice).
#[derive(Clone)]
pub struct SinkHandle(pub Arc<Mutex<dyn ActionSink>>);

impl SinkHandle {
    pub fn new(sink: impl ActionSink + 'static) -> Self {
        Self(Arc::new(Mutex::new(sink)))
    }
}

impl std::fmt::Debug for SinkHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SinkHandle")
    }
}
/// Reactions below this priority are skipped while she is talking
/// (hover, idle poses); system alerts and clicks still play.
pub const TALK_INTERRUPT_PRIORITY: u8 = 60;

/// Default (and fallback) seated base clip for perching on a window edge; its
/// `variant_of:sit_swing` takes (`sit_heel_kick`, `sit_cross_legged_bob`,
/// `sit_bored_chin`, `sit_stretch_yawn`) play from it through the animator's
/// variants.
pub const SIT_CLIP: &str = "sit_swing";

/// Clip tag: another seated base a perch may pick (e.g. `perch_sit`), next to
/// [`SIT_CLIP`]. Its own `variant_of:<clip>` takes play from it.
pub const SIT_BASE_TAG: &str = "sit_base";

/// Seated bases a perch picks from: [`SIT_CLIP`] and the clips tagged
/// [`SIT_BASE_TAG`], each only with `seat_y` (sorted; empty without any).
pub fn sit_clips(m: &Manifest) -> Vec<String> {
    m.clips
        .iter()
        .filter(|(n, c)| c.seat_y.is_some() && (n.as_str() == SIT_CLIP || c.has_tag(SIT_BASE_TAG)))
        .map(|(n, _)| n.clone())
        .collect()
}

/// Clip tag: a lounge base a perch may pick instead of a sit (e.g.
/// `perch_lounge`): she lies along the window top. Needs `seat_y` (the line
/// her body rests on) and `seat_x_range` (the part of her lying on the edge).
pub const LOUNGE_BASE_TAG: &str = "lounge_base";

/// A lounge keeps its resting span at least this many px inside the window's
/// left and right edges.
pub const LOUNGE_EDGE_PX: i32 = 8;

/// Lounge bases a perch may pick ([`LOUNGE_BASE_TAG`] with `seat_y` and
/// `seat_x_range`; sorted).
pub fn lounge_clips(m: &Manifest) -> Vec<String> {
    m.clips
        .iter()
        .filter(|(_, c)| {
            c.has_tag(LOUNGE_BASE_TAG) && c.seat_y.is_some() && c.seat_x_range.is_some()
        })
        .map(|(n, _)| n.clone())
        .collect()
}

/// A lounge's rest line above its feet anchor in cell px (like
/// [`seat_cell`], but a lounge lying on its feet line rests at 0).
pub fn lounge_cell(clip: &Clip) -> Option<i32> {
    let d = clip.frames.first()?.anchor.1 - clip.seat_y?;
    (d >= 0).then_some(d)
}

/// [`gravity::lounge_shift`] for lounge `clip` at `scale` on window top
/// `rect`, her feet at column `feet_x`.
pub fn lounge_fit(clip: &Clip, scale: f32, feet_x: i32, rect: Rect) -> Option<i32> {
    let ax = clip.frames.first()?.anchor.0;
    gravity::lounge_shift(
        clip.seat_x_range,
        ax,
        scale,
        feet_x,
        (rect.x, rect.right()),
        LOUNGE_EDGE_PX,
    )
}

/// The landing blend from the drag pose into a seated base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandingBlend {
    /// The blend clip (e.g. `blend_drag_held_to_perch_sit`).
    pub blend: String,
    /// The seated base it hands over to (one of [`sit_clips`]).
    pub to: String,
    /// The blend's own seat line above its feet anchor in cell px (the
    /// base's when the blend has no `seat_y`).
    pub seat_cell: i32,
    /// The base's seat line ([`seat_cell`]).
    pub to_seat_cell: i32,
}

/// Tag the build tool (`tools/video-pipeline/blend_start_match.py`) sets on
/// a landing blend whose first frame matches its `from` clip's `from_frame`
/// where the engine draws it at touchdown (its seat line on the window top,
/// where the drag pose had its feet). Without it the blend would pop in, so
/// [`landing_blend`] skips it.
pub const START_MATCHES_TAG: &str = "start_matches";

/// The manifest's landing blend for a drop in pose `from` (the drag pose,
/// `drag_held`): the first clip (sorted) whose `[blend]` goes from `from` to
/// one of the [`sit_clips`] with seat data and whose first frame is marked
/// as matching `from` ([`START_MATCHES_TAG`]). `None` without one (she then
/// lands standing, as before).
pub fn landing_blend(m: &Manifest, from: &str) -> Option<LandingBlend> {
    let sits = sit_clips(m);
    m.clips.iter().find_map(|(n, c)| {
        let b = c.blend.as_ref()?;
        if b.from != from || !sits.contains(&b.to) {
            return None;
        }
        if !c.has_tag(START_MATCHES_TAG) {
            tracing::debug!(blend = %n, "landing blend skipped: its first frame does not match {from}");
            return None;
        }
        let to_seat_cell = seat_cell(&m.clips[&b.to])?;
        Some(LandingBlend {
            blend: n.clone(),
            to: b.to.clone(),
            seat_cell: seat_cell(c).unwrap_or(to_seat_cell),
            to_seat_cell,
        })
    })
}

/// `overlay` is `sit`'s own blend: its landing blend or a
/// `blend_<from>_to_<clip>` sit-down (not an interruption of the sit).
fn own_sit_blend(sit: &Sit, overlay: &str) -> bool {
    sit.landing.as_ref().is_some_and(|l| l.clip == overlay)
        || (overlay.starts_with("blend_") && overlay.ends_with(&format!("_to_{}", sit.clip)))
}

/// `clip`'s seat line above its feet anchor in cell px (`anchor y - seat_y`
/// of the first frame); `None` without seat data or a seat at/below the feet.
/// Her window drops this × the overlay scale to sit.
pub fn seat_cell(clip: &Clip) -> Option<i32> {
    let d = clip.frames.first()?.anchor.1 - clip.seat_y?;
    (d > 0).then_some(d)
}

/// Half of `clip`'s seat width (`seat_x_range`) in screen px (0 without it).
pub fn half_seat_px(clip: &Clip, scale: f32) -> i32 {
    clip.seat_x_range
        .map_or(0.0, |(a, b)| (b - a).abs() as f32 * 0.5 * scale)
        .round() as i32
}

/// Px of her window kept on screen when dragging off an edge.
pub const KEEP_ON_SCREEN: i32 = 64;

/// Overlay errors.
#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    #[error(transparent)]
    Stage(#[from] StageError),
    #[error("this build has no {0} backend; rebuild with `--features {0}`")]
    NotBuilt(&'static str),
    /// The compositor lacks `zwlr_layer_shell_v1` (e.g. GNOME) or there is
    /// no Wayland socket. Auto-detection falls back to X11.
    #[error("wayland unavailable: {0}")]
    WaylandUnavailable(String),
    #[error("wayland: {0}")]
    Wayland(String),
    #[error("x11: {0}")]
    X11(String),
    #[error("win32: {0}")]
    Win32(String),
}

/// Everything the overlay needs from the app.
#[derive(Debug, Clone)]
pub struct OverlayOptions {
    pub manifest: PathBuf,
    /// Overlay size = manifest cell × `scale`.
    pub scale: f32,
    /// Whole-sprite opacity 0..=1.
    pub opacity: f32,
    /// Redraw rate cap (Hz). Frames only re-render when something moved.
    pub render_hz: f32,
    pub animator: AnimatorConfig,
    /// Saved position (`[overlay]` in config.toml).
    pub position: OverlayPos,
    /// Where to persist the position after a drag (`None` = don't).
    pub config_path: Option<PathBuf>,
    /// `true` if the user forced the backend (no automatic fallback).
    pub forced: bool,
    /// Quit after this long (smoke tests / CPU measurement).
    pub exit_after: Option<Duration>,
    /// M3 behaviour timings (`[behavior]`).
    pub behavior: BehaviorConfig,
    /// Wander or Stay (Follow behaves like Stay until M5).
    pub mode: MoveMode,
    /// Start muted (no SFX cues).
    pub muted: bool,
    /// System-monitor and chat events (`None` = nothing feeds her).
    pub events: Option<EventFeed>,
    /// Gets every applied action batch (voice; `None` = silent).
    pub sink: Option<SinkHandle>,
    /// config.toml the menu's "Open settings" opens (even with `--no-save`;
    /// `None` = `Config::default_path()`).
    pub settings_path: Option<PathBuf>,
    /// Opens the chat bubble (`None` = chat disabled). A single left click
    /// on her (when `chat_on_click`) or `Y`/`Enter` calls it.
    pub chat: Option<ChatHook>,
    /// A plain left click opens the chat (`[chat] open_on_click`).
    pub chat_on_click: bool,
    /// Draw her replies in a speech bubble over her head on the overlay
    /// (`[chat] overlay_speech`); the window grows upward to fit it.
    pub overlay_speech: bool,
    /// Keep the window above others (`always_on_top`).
    pub always_on_top: bool,
    /// Only her head catches clicks (`click_through`).
    pub click_through: bool,
    /// Custom avatars (`<config dir>/avatars`); the selection is shown.
    pub avatars: Option<AvatarStore>,
    /// `[avatar]` pace and redraw rate.
    pub avatar: lillith_core::config::AvatarConfig,
    /// `seasonal_outfit`: `auto` (by date) / `off` / an outfit id
    /// (`assets/seasons.toml` next to the manifest's `generated/` dir).
    pub seasonal_outfit: String,
}

/// Minimized: tiny "peek" icon at the screen edge, restored by a click.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peek {
    pub scale: f32,
    pub placement: Placement,
}

/// Shared overlay logic, independent of the windowing system.
#[derive(Debug)]
pub struct Controller {
    pub stage: Stage,
    pub drag: Drag,
    pub brain: Brain,
    /// Where the window is (last value handed to the compositor/WM).
    pub placement: Placement,
    output: Option<String>,
    config_path: Option<PathBuf>,
    pub opacity: u8,
    pending_move: Option<Placement>,
    screen: Option<(u32, u32)>,
    quit: bool,
    feed: Option<EventFeed>,
    /// Feed events read but not applied yet (spread over frames).
    feed_backlog: std::collections::VecDeque<Event>,
    /// The safeword cut the current reply: its queued tokens / `chat.end`
    /// are ignored (never shown or spoken) until the next `chat.start`.
    chat_stopped: bool,
    sink: Option<SinkHandle>,
    /// Menu "Pause system monitor": feed events are dropped while set.
    pub monitor_paused: bool,
    menu: Option<MenuPopup>,
    /// A new menu was opened and the backend has not shown it yet.
    menu_opened: bool,
    fonts: Fonts,
    settings_path: Option<PathBuf>,
    /// The `lillith-settings` window "Open settings" launches.
    settings: SettingsWindow,
    /// The manifest the stage was loaded from; handed to the settings
    /// window as `$LILLITH_MANIFEST` so its Poses list finds the clips.
    manifest_path: PathBuf,
    chat: Option<ChatHook>,
    chat_on_click: bool,
    click_gate: ClickGate,
    /// Speaking state (chat replies → `talk_loop` lip-flap, speech text).
    pub talk: Talk,
    /// Seconds since start (click timing).
    clock: f32,
    double_click_s: f32,
    /// The window size changed (resize / peek / speech area): the backend
    /// resizes its window and buffer, keeping the feet in place.
    resized: bool,
    /// Always-on-top changed: `Some(new value)` for the backend.
    level: Option<bool>,
    pub always_on_top: bool,
    pub peek: Option<Peek>,
    /// "Show off" clips still to play.
    showoff: std::collections::VecDeque<String>,
    avatars: Option<AvatarStore>,
    avatar_cfg: lillith_core::config::AvatarConfig,
    render_hz: f32,
    hovered: bool,
    /// Drag velocity (px/s, low-passed) for the custom avatar's lean.
    drag_vx: Option<f32>,
    last_drag_x: Option<(f32, i32)>,
    seed: u64,
    /// Poses page of the last opened menu (picks are resolved against it).
    menu_page: Option<MenuPage>,
    /// Falling after a drop and perching on windows ([`gravity`]).
    pub gravity: Gravity,
    /// Window stack to use instead of the KWin bridge's (tests, sims).
    desktop_override: Option<Desktop>,
    /// Chrome accent of the shown avatar (Lillith's magenta by default).
    accent: lillith_theme::Accent,
    /// "Come closer" tease scheduler (`[behavior.tease]`).
    pub tease: Tease,
    /// The running tease: its clips, her normal scale and the window
    /// centre x it grows around.
    tease_run: Option<TeaseRun>,
    /// `assets/seasons.toml` (bundled copy if the set has none).
    seasons: Seasons,
    /// The `seasonal_outfit` setting (`auto` / `off` / an id).
    outfit_setting: String,
    /// Outfit worn now (`None` = base outfit): only its seasonal clips show.
    outfit_active: Option<String>,
    /// The local date [`Controller::outfit_active`] was picked for.
    outfit_date: Option<MonthDay>,
    /// Seconds since the date was last checked (auto mode rolls over).
    outfit_check_s: f32,
    /// The last opened menu is the Avatar ▸ Outfit page.
    menu_outfit_page: bool,
}

/// How often (s) the overlay re-checks today's date for `auto` outfits.
const OUTFIT_DATE_CHECK_S: f32 = 60.0;

/// State of a running "come closer" tease in the overlay.
#[derive(Debug, Clone, PartialEq)]
struct TeaseRun {
    plan: TeasePlan,
    /// Her normal scale (restored exactly at the end).
    base_scale: f32,
    /// Window centre x (px) she grows / shrinks around.
    center_x: i32,
}

/// Reaction priority of the tease beat (above the pose cycle and pranks,
/// below real events).
pub const TEASE_PRIORITY: u8 = 30;

/// Smallest scale change (fraction) worth a window resize mid-tease.
const TEASE_SCALE_STEP: f32 = 0.002;

impl Controller {
    pub fn new(opts: &OverlayOptions) -> Result<Self, OverlayError> {
        let mut stage = Stage::load(&opts.manifest, opts.scale, opts.animator.clone())?;
        if opts.overlay_speech {
            stage = stage.with_speech_area()?;
        }
        let mut c = Self::with_stage(stage, opts);
        c.load_selected_avatar();
        c.sync_walk_speed();
        Ok(c)
    }

    /// Match the wander glide speed to the walk clip's foot-locked ground
    /// speed (`move_speed_px_s` × on-screen scale × playback speed) so her
    /// planted boots don't slide. No-op for manifests without the field.
    /// Walk clips with `root_motion` (scaffold's feet-locked walk, the v2
    /// set) ignore this speed: the brain moves her by the drawn frames'
    /// root motion instead, so the two never fight.
    pub fn sync_walk_speed(&mut self) {
        let anim = self.stage.animator();
        let Some(v) = ["walk_right", "walk_left", "walk"]
            .iter()
            .find_map(|n| anim.manifest().clips.get(*n))
            .and_then(|c| c.move_speed_px_s())
        else {
            return;
        };
        let px_s = v * self.stage.scale() * anim.playback_speed();
        self.brain.set_walk_speed(px_s);
        tracing::debug!(px_s, "walk speed matched to the gait rig");
    }

    pub fn with_stage(stage: Stage, opts: &OverlayOptions) -> Self {
        let clips: Vec<String> = stage.animator().clip_names().map(str::to_string).collect();
        let mut brain = Brain::new(
            opts.behavior.clone(),
            opts.mode,
            RuleSet::bundled(),
            clips,
            opts.animator.seed ^ 0xB4A1_17B4,
        );
        brain.set_muted(opts.muted);
        let placement = Placement::from_config(&opts.position);
        brain.set_position(placement.left as f32);
        Self {
            stage,
            drag: Drag::default(),
            brain,
            placement,
            output: opts.position.output.clone(),
            config_path: opts.config_path.clone(),
            opacity: crate::pixels::opacity_byte(opts.opacity),
            pending_move: None,
            screen: None,
            quit: false,
            feed: opts.events.clone(),
            feed_backlog: std::collections::VecDeque::new(),
            chat_stopped: false,
            sink: opts.sink.clone(),
            monitor_paused: false,
            menu: None,
            menu_opened: false,
            fonts: Fonts::preload(),
            settings_path: opts.settings_path.clone(),
            settings: SettingsWindow::default(),
            manifest_path: opts.manifest.clone(),
            chat: opts.chat.clone(),
            chat_on_click: opts.chat_on_click,
            click_gate: ClickGate::default(),
            talk: Talk::with_speed(opts.animator.talk_speed),
            clock: 0.0,
            double_click_s: opts.behavior.gestures.double_click_s,
            resized: false,
            level: None,
            always_on_top: opts.always_on_top,
            peek: None,
            showoff: Default::default(),
            avatars: opts.avatars.clone(),
            avatar_cfg: opts.avatar.clone(),
            render_hz: opts.render_hz,
            hovered: false,
            drag_vx: None,
            last_drag_x: None,
            seed: opts.animator.seed,
            menu_page: None,
            gravity: Gravity::new(
                opts.behavior.gravity.clone(),
                opts.behavior.floor_offset_px,
                opts.animator.seed,
            ),
            desktop_override: None,
            accent: lillith_theme::Accent::LILLITH,
            tease: Tease::new(opts.behavior.tease.clone(), opts.animator.seed),
            tease_run: None,
            seasons: Seasons::load_or_bundled(
                Seasons::path_for_manifest(&opts.manifest).as_deref(),
            ),
            outfit_setting: opts.seasonal_outfit.clone(),
            outfit_active: None,
            outfit_date: None,
            outfit_check_s: 0.0,
            menu_outfit_page: false,
        }
        .with_click_through(opts.click_through)
        .with_outfit_for(lillith_core::seasons::today())
    }

    fn with_outfit_for(mut self, date: MonthDay) -> Self {
        self.refresh_outfit(date);
        self
    }

    /// Picks the outfit for `date` and the current setting, and hides every
    /// other outfit's seasonal clips from her random picks. An outfit
    /// without clips in this set counts as none (base outfit).
    pub fn refresh_outfit(&mut self, date: MonthDay) {
        let m = self.stage.animator().manifest();
        let active =
            lillith_anim::catalog::worn_outfit(&self.seasons, m, date, &self.outfit_setting);
        let id = active.map(|o| o.id.clone());
        let hidden = lillith_anim::catalog::hidden_seasonal(m, active);
        self.brain.set_hidden_clips(hidden);
        if id != self.outfit_active || self.outfit_date.is_none() {
            tracing::info!(
                setting = %self.outfit_setting,
                outfit = ?id,
                date = %date,
                "seasonal outfit"
            );
        }
        self.outfit_active = id;
        self.outfit_date = Some(date);
        self.outfit_check_s = 0.0;
    }

    /// Re-picks the outfit if the local date changed (auto mode).
    fn check_outfit_date(&mut self) {
        let today = lillith_core::seasons::today();
        if self.outfit_date != Some(today) {
            self.refresh_outfit(today);
        }
    }

    /// The outfit worn now (`None` = base outfit).
    pub fn outfit_active(&self) -> Option<&Outfit> {
        self.seasons.outfit(self.outfit_active.as_deref()?)
    }

    /// The `seasonal_outfit` setting in effect.
    pub fn outfit_setting(&self) -> &str {
        &self.outfit_setting
    }

    /// Sets `seasonal_outfit` (`auto` / `off` / an id) and wears it now;
    /// `save` also writes it to config.toml.
    pub fn set_seasonal_outfit(&mut self, value: &str, save: bool) {
        let value = lillith_core::seasons::OutfitSetting::parse(value)
            .key()
            .to_string();
        self.outfit_setting = value;
        let date = self
            .outfit_date
            .unwrap_or_else(lillith_core::seasons::today);
        self.refresh_outfit(date);
        // Drop queued Show-off clips of an outfit she just took off.
        let m = self.stage.animator().manifest();
        let active = self
            .outfit_active
            .as_deref()
            .and_then(|a| self.seasons.outfit(a));
        self.showoff
            .retain(|c| lillith_anim::catalog::seasonal_visible(c, m, active));
        if save {
            if let Some(path) = &self.config_path {
                if let Err(e) =
                    lillith_core::config::save_seasonal_outfit(path, &self.outfit_setting)
                {
                    tracing::warn!("could not save seasonal_outfit: {e}");
                }
            }
        }
    }

    fn with_click_through(mut self, on: bool) -> Self {
        self.stage.set_click_through(on);
        self
    }

    /// Redraw rate the backend should run at (higher while a custom
    /// avatar animates continuously).
    pub fn render_hz(&self) -> f32 {
        if self.stage.avatar_id().is_some() {
            self.render_hz.max(self.avatar_cfg.render_hz)
        } else {
            self.render_hz
        }
    }

    /// A changed always-on-top setting for the backend to apply.
    pub fn take_level(&mut self) -> Option<bool> {
        self.level.take()
    }

    /// Chrome accent of the shown avatar.
    pub fn accent(&self) -> lillith_theme::Accent {
        self.accent
    }

    /// Loads the avatar selected in `<config dir>/avatar.json`.
    pub fn load_selected_avatar(&mut self) {
        let sel = self.avatars.as_ref().and_then(|s| s.selection().selected);
        self.set_avatar(sel.as_deref());
    }

    /// Shows custom avatar `id` (`None` = Lillith), live.
    pub fn set_avatar(&mut self, id: Option<&str>) {
        let av = match (id, &self.avatars) {
            (Some(id), Some(store)) => match store.load(id) {
                Ok(av) => Some(av),
                Err(e) => {
                    tracing::warn!("avatar {id}: {e}; showing Lillith");
                    None
                }
            },
            _ => None,
        };
        tracing::info!(avatar = ?av.as_ref().map(|a| &a.meta.id), "avatar");
        self.accent = av.as_ref().map(|a| a.accent()).unwrap_or_default();
        if let Some(m) = self.menu.as_mut() {
            m.set_accent(self.accent);
        }
        self.stage.set_avatar(av);
        self.stage.set_avatar_pace(self.avatar_cfg.pace);
    }

    /// Resizes her live (`save` also writes `scale` to config.toml).
    pub fn set_scale(&mut self, scale: f32, save: bool) {
        let scale = scale.clamp(crate::ui::SCALE_RANGE.0.min(PEEK_SCALE), 4.0);
        if (self.stage.scale() - scale).abs() > 1e-3 {
            if let Err(e) = self.stage.set_scale(scale) {
                tracing::warn!("resize: {e}");
                return;
            }
            self.resized = true;
            self.resettle();
            if let Some((sw, _)) = self.screen {
                self.set_screen((sw, self.screen.map_or(0, |s| s.1)));
            }
            self.sync_walk_speed();
        }
        if save {
            if let Some(path) = &self.config_path {
                if let Err(e) = lillith_core::config::save_scale(path, scale) {
                    tracing::warn!("could not save scale: {e}");
                }
            }
        }
    }

    /// Minimize: shrink into a small peek icon at the right screen edge.
    pub fn minimize(&mut self) {
        if self.peek.is_some() {
            return;
        }
        self.cancel_tease();
        self.peek = Some(Peek {
            scale: self.stage.scale(),
            placement: self.placement,
        });
        let anim = self.stage.animator_mut();
        anim.clear_overlays();
        anim.stand();
        self.showoff.clear();
        self.gravity.mode = Mode::Settle { wait: 0.0 };
        self.set_scale(PEEK_SCALE, false);
        let w = self.stage.size().0 as i32;
        let left = match self.screen {
            Some((sw, _)) => sw as i32 - w * 3 / 4,
            None => self.placement.left,
        };
        self.pending_move = Some(Placement { left, bottom: 0 });
        tracing::info!("minimized to the peek icon (click it to restore)");
    }

    /// Restores her from the peek icon.
    pub fn restore(&mut self) {
        let Some(p) = self.peek.take() else { return };
        self.set_scale(p.scale, false);
        self.pending_move = Some(p.placement);
        self.brain.set_position(p.placement.left as f32);
        self.gravity.mode = Mode::Settle { wait: 0.0 };
        tracing::info!("restored from the peek icon");
    }

    /// Runs a menu/settings command.
    pub fn command(&mut self, cmd: Command) {
        tracing::info!(?cmd, "ui command");
        match cmd {
            Command::Pose(clip) => {
                self.restore();
                self.retreat_tease();
                self.showoff.clear();
                let anim = self.stage.animator_mut();
                if anim.has_clip(&clip) {
                    anim.clear_overlays();
                    if let Err(e) = anim.react(&clip, 95) {
                        tracing::warn!("pose {clip}: {e}");
                    }
                    self.stage.avatar_react(1.2);
                } else {
                    tracing::warn!("pose {clip}: no such clip");
                }
            }
            Command::ShowOff => {
                self.restore();
                self.retreat_tease();
                self.check_outfit_date();
                let seq = lillith_anim::catalog::show_off_for(
                    self.stage.animator().manifest(),
                    self.seed ^ (self.clock * 1000.0) as u64,
                    self.outfit_active(),
                );
                tracing::info!(?seq, "show off");
                self.stage.animator_mut().clear_overlays();
                self.showoff = seq.into();
            }
            Command::Chat => {
                self.restore();
                self.open_chat();
            }
            Command::Minimize => self.minimize(),
            Command::Scale { scale, save } => {
                self.cancel_tease();
                self.peek = None;
                self.set_scale(scale, save);
            }
            Command::Settings => {
                self.open_settings_window(lillith_core::config::Config::default_path())
            }
            Command::Avatar(id) => {
                if let Some(store) = &self.avatars {
                    if let Err(e) = store.select(id.as_deref()) {
                        tracing::warn!("could not save the avatar choice: {e}");
                    }
                }
                self.set_avatar(id.as_deref());
            }
            Command::ConfigChanged => self.reload_config(),
            Command::Quit => {
                tracing::info!("quit (menu)");
                self.quit = true;
            }
        }
    }

    /// Current lip-flap pace.
    pub fn talk_speed(&self) -> f32 {
        self.talk.speed()
    }

    /// Re-reads config.toml and applies what can change live.
    pub fn reload_config(&mut self) {
        let Some(path) = self.config_path.clone() else {
            return;
        };
        let cfg = match Config::load(&path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("settings: {}: {e}", path.display());
                return;
            }
        };
        self.apply_config(&cfg);
        if let Some(sink) = &self.sink {
            if let Ok(mut s) = sink.0.lock() {
                s.config_changed(&path);
            }
        }
    }

    /// "Speak out loud (speakers)" state from the voice sink.
    pub fn speaker(&self) -> Option<bool> {
        self.sink.as_ref()?.0.lock().ok()?.speaker()
    }

    /// Applies a config live (Settings).
    pub fn apply_config(&mut self, cfg: &Config) {
        let anim = self.stage.animator_mut();
        anim.set_playback_speed(cfg.anim.playback_speed);
        anim.set_smooth_frames(cfg.anim.smooth_frames, cfg.anim.crossfade);
        let mode = cfg.jiggle();
        anim.set_prefer_jiggle_variants(mode == JiggleMode::Baked);
        let mut springs = cfg.springs.clone();
        if mode == JiggleMode::Off {
            springs.jiggle_gain = 0.0;
            springs.chest_bounce_gain = 0.0;
            springs.pigtail_sway_gain = 0.0;
        }
        anim.set_springs(&springs);
        self.talk.set_speed(cfg.anim.talk_speed);
        let speech = cfg.chat.enabled && cfg.chat.overlay_speech;
        let before = self.stage.size();
        if let Err(e) = self.stage.set_speech_enabled(speech) {
            tracing::warn!("speech area: {e}");
        }
        if self.stage.size() != before {
            self.resized = true;
        }
        self.stage.set_click_through(cfg.click_through);
        if cfg.always_on_top != self.always_on_top {
            self.always_on_top = cfg.always_on_top;
            self.level = Some(cfg.always_on_top);
        }
        self.avatar_cfg = cfg.avatar.clone();
        if cfg.seasonal_outfit != self.outfit_setting {
            self.set_seasonal_outfit(&cfg.seasonal_outfit, false);
        }
        self.tease.set_config(cfg.behavior.tease.clone());
        if self.peek.is_none() && cfg.scale > 0.0 {
            self.cancel_tease();
            self.set_scale(cfg.overlay_scale(), false);
        }
        // Avatar choice / strength / anchors may have changed too.
        self.load_selected_avatar();
        self.sync_walk_speed();
        tracing::info!("settings applied");
    }

    /// Next "Show off" clip once the previous one finished.
    fn pump_showoff(&mut self) {
        if self.showoff.is_empty() || self.talk.active() {
            return;
        }
        let anim = self.stage.animator_mut();
        if anim.overlay_clip().is_some() || anim.queued() > 0 {
            return;
        }
        if let Some(clip) = self.showoff.pop_front() {
            if anim.has_clip(&clip) {
                let _ = anim.react(&clip, 95);
                self.stage.avatar_react(1.0);
            }
        }
    }

    /// Output size in px; sets the range she wanders over.
    pub fn set_screen(&mut self, output: (u32, u32)) {
        self.screen = Some(output);
        self.apply_area();
    }

    /// Wander range: the screen, or the window top she is perched on.
    fn apply_area(&mut self) {
        let Some((sw, sh)) = self.screen else { return };
        if let Some(rect) = self.gravity.perch().map(|p| p.rect) {
            let desk = self.desktop();
            let frame = self.frame(&desk, sh);
            let (lo, hi) = gravity::perch_span(&frame, rect, self.perch_margin());
            self.brain.set_area(lo, hi);
        } else {
            let w = self.stage.size().0 as f32;
            self.brain.set_area(0.0, (sw as f32 - w).max(0.0));
        }
    }

    /// Uses `desk` instead of the KWin bridge's window stack (`None` =
    /// back to the bridge). For tests and simulations.
    pub fn set_desktop(&mut self, desk: Option<Desktop>) {
        self.desktop_override = desk;
    }

    fn desktop(&self) -> Desktop {
        match &self.desktop_override {
            Some(d) => d.clone(),
            None => lillith_core::desktop::snapshot(),
        }
    }

    /// Placement ↔ global mapping for the current size and screen.
    fn frame(&self, desk: &Desktop, screen_h: u32) -> Frame {
        let (_, h) = self.stage.size();
        let (fx, fy) = self.stage.feet_anchor();
        Frame {
            origin: desk.screen.map_or((0, 0), |r| (r.x, r.y)),
            screen_h: screen_h as i32,
            feet: (fx.round() as i32, (h as f32 - fy).round() as i32),
        }
    }

    /// Feet kept this far inside a window top: half the widest seat of the
    /// [`sit_clips`] (so any sit fits), at least 16 px.
    fn perch_margin(&self) -> i32 {
        let scale = self.stage.scale();
        let m = self.stage.animator().manifest();
        sit_clips(m)
            .iter()
            .map(|n| half_seat_px(&m.clips[n], scale))
            .fold(16, i32::max)
    }

    /// `true` while gravity may move her (a screen is known, `g_px_s2 > 0`,
    /// not minimized).
    fn gravity_active(&self) -> bool {
        self.screen.is_some() && self.gravity.enabled() && self.peek.is_none()
    }

    /// Re-check her footing next tick (resize, reset, restore).
    fn resettle(&mut self) {
        self.stand_up(false);
        match &mut self.gravity.mode {
            Mode::Held | Mode::Falling(_) => {}
            // Still on her window: stay perched; `perch_hold` puts her feet
            // back on its top at the new size next tick. (A settle from the
            // old, seated height would find nothing under her feet.)
            Mode::Perched(p) => p.rise = None,
            _ => self.gravity.mode = Mode::Settle { wait: 0.0 },
        }
    }

    /// Ends a sit on the edge: back on her feet on top of the window. With
    /// `blend` (the sit ran out) a `<sit> -> idle` blend plays and her window
    /// rises with it; otherwise she is up at once. (The window follows
    /// [`gravity::Perch::bottom`] in [`Self::gravity_tick`].)
    fn stand_up(&mut self, blend: bool) {
        self.drop_sit_blend();
        let Mode::Perched(p) = &mut self.gravity.mode else {
            return;
        };
        let Some(sit) = p.sit.take() else { return };
        let next = self.gravity.sit_pause();
        let anim = self.stage.animator_mut();
        let mut rise_s = None;
        if anim.base() == sit.clip {
            if blend {
                rise_s = anim.stand_blended();
            } else {
                anim.stand();
            }
        }
        if let Mode::Perched(p) = &mut self.gravity.mode {
            p.next_sit_s = next;
            // Up from wherever the sit-down blend had got her.
            let left = sit.sink.map_or(1.0, |e| e.fraction());
            p.rise = rise_s.filter(|&s| s > 0.0).map(|secs| {
                let mut ease = gravity::Ease::new(secs);
                ease.left_s = secs * left;
                ease.total_s = secs;
                gravity::Rise {
                    seat_cell: sit.seat_cell,
                    ease,
                }
            });
        }
        tracing::debug!(clip = %sit.clip, "perch: stood up");
    }

    /// Drops the sit's own landing or sit-down blend if it is still playing
    /// (she leaves the sit before it finished: a grab, a lost perch, a
    /// resettle), so it does not keep drawing over the next pose.
    fn drop_sit_blend(&mut self) {
        let Some(sit) = self.gravity.perch().and_then(|p| p.sit.as_ref()) else {
            return;
        };
        let own = self
            .stage
            .animator()
            .overlay_clip()
            .is_some_and(|o| own_sit_blend(sit, o));
        if own {
            self.stage.animator_mut().clear_overlays();
        }
    }

    /// Starts a fall from `from` (her footing went away).
    fn start_fall(&mut self, from: Placement, why: &str) {
        tracing::info!(why, bottom = from.bottom, "gravity: falling");
        let a = self.brain.begin_fall();
        self.apply(a);
        self.gravity.mode = Mode::Falling(gravity::Fall::new(from.bottom));
    }

    /// She came to rest at `p` on `landing`.
    fn land(&mut self, p: Placement, landing: &Landing, desk: &Desktop) {
        if p != self.pending_move.unwrap_or(self.placement) {
            self.pending_move = Some(p);
        }
        self.gravity.mode = match landing {
            Landing::Window { id, .. } => match desk.window(id) {
                Some(w) => {
                    tracing::info!(window = %w.class, top = w.rect.y, "gravity: perched on a window");
                    Mode::Perched(self.gravity.new_perch(id.clone(), w.rect))
                }
                None => Mode::Floor,
            },
            Landing::Floor { .. } => Mode::Floor,
        };
        self.apply_area();
        if self.brain.phase() == Phase::Dragged {
            let mut a = self.brain.input(Input::DragEnd { left: p.left });
            if self.land_through_blend() {
                // The drop's `Stand` would reset her to the base pose
                // mid-blend; the landing blend and the sit it hands over to
                // own her pose now.
                a.retain(|a| !matches!(a, Action::Stand));
            }
            self.apply(a);
            self.persist(p);
        } else {
            self.brain.set_position(p.left as f32);
        }
    }

    /// Right after landing on a window from a drag or fall (still in the
    /// drag pose): plays the manifest's [`landing_blend`] with its seat line
    /// on the window top, then hands over to its seated base at the blend's
    /// `to_frame` and stays seated for a usual sit. `false` (nothing
    /// changed) without a blend, off a window, or already seated.
    fn land_through_blend(&mut self) -> bool {
        if !matches!(&self.gravity.mode, Mode::Perched(p) if p.sit.is_none()) {
            return false;
        }
        let anim = self.stage.animator();
        let Some(lb) = landing_blend(anim.manifest(), anim.base()) else {
            return false;
        };
        let Ok(dur) = self.stage.animator_mut().land_into(&lb.blend) else {
            return false;
        };
        let left_s = dur + self.gravity.sit_length();
        tracing::debug!(blend = %lb.blend, clip = %lb.to, dur, "perch: landing through a blend");
        if let Mode::Perched(p) = &mut self.gravity.mode {
            p.rise = None;
            p.sit = Some(Sit {
                clip: lb.to,
                left_s,
                seat_cell: lb.to_seat_cell,
                sink: None,
                landing: Some(LandBlend {
                    clip: lb.blend,
                    seat_cell: lb.seat_cell,
                }),
            });
        }
        true
    }

    /// Gravity, perch checks and sits (after the brain moved her this tick).
    fn gravity_tick(&mut self, dt: f32) {
        if !self.gravity_active() || self.drag.active() {
            return;
        }
        let Some((_, sh)) = self.screen else { return };
        let desk = self.desktop();
        let frame = self.frame(&desk, sh);
        let floor_y = gravity::floor_y(&desk, &frame, self.gravity.floor_offset_px);
        let cur = self.pending_move.unwrap_or(self.placement);
        let (fx, fy) = frame.feet_at(cur);
        match self.gravity.mode.clone() {
            Mode::Held => self.gravity.mode = Mode::Settle { wait: 0.0 },
            Mode::Settle { wait } => {
                if !desk.has_bridge && wait > 0.0 {
                    self.gravity.mode = Mode::Settle { wait: wait - dt };
                    return;
                }
                self.hint_no_bridge(&desk);
                let landing = self
                    .gravity
                    .landing(&desk, fx, fy - gravity::SNAP_PX, floor_y);
                let ground = frame.bottom_for_feet(landing.y());
                if cur.bottom > ground + gravity::SNAP_PX {
                    self.start_fall(cur, "not standing on anything");
                } else {
                    // On it (within SNAP_PX: stay put) or below it: snap up.
                    let bottom = if cur.bottom < ground - gravity::SNAP_PX {
                        ground
                    } else {
                        cur.bottom
                    };
                    self.land(Placement { bottom, ..cur }, &landing, &desk);
                }
            }
            Mode::Falling(mut f) => {
                let at = Placement {
                    left: cur.left,
                    bottom: f.bottom.round() as i32,
                };
                let (fx, fy) = frame.feet_at(at);
                self.hint_no_bridge(&desk);
                let landing = self.gravity.landing(&desk, fx, fy, floor_y);
                let ground = frame.bottom_for_feet(landing.y()) as f32;
                match f.step(dt, ground, &self.gravity.cfg) {
                    FallStep::Moving(bottom) => {
                        self.pending_move = Some(Placement { bottom, ..cur });
                        self.gravity.mode = Mode::Falling(f);
                    }
                    FallStep::Landed(bottom) => {
                        self.land(Placement { bottom, ..cur }, &landing, &desk)
                    }
                }
            }
            Mode::Floor => {
                // The floor moved (panel, bridge arrived, resolution).
                let ground = frame.bottom_for_feet(floor_y);
                if cur.bottom > ground + gravity::SNAP_PX {
                    self.start_fall(cur, "the floor dropped");
                } else if cur.bottom < ground - gravity::SNAP_PX {
                    self.pending_move = Some(Placement {
                        bottom: ground,
                        ..cur
                    });
                }
            }
            Mode::Perched(p) => match gravity::check_perch(&desk, &p, fx) {
                PerchCheck::Lost(why) => {
                    let drop = p.lowered_px(self.stage.scale());
                    self.drop_sit_blend();
                    let anim = self.stage.animator_mut();
                    if p.sit.as_ref().is_some_and(|s| anim.base() == s.clip) {
                        anim.stand();
                    }
                    let from = Placement {
                        bottom: cur.bottom + drop,
                        ..cur
                    };
                    self.pending_move = Some(from);
                    self.gravity.mode = Mode::Floor;
                    self.apply_area();
                    self.start_fall(from, why);
                }
                PerchCheck::Resized(r) => {
                    if let Mode::Perched(p) = &mut self.gravity.mode {
                        p.rect = r;
                    }
                    self.apply_area();
                    self.lounge_refit(&frame);
                    self.perch_sit(dt, &frame);
                    self.perch_hold(&frame);
                }
                PerchCheck::Ok => {
                    self.perch_sit(dt, &frame);
                    self.perch_hold(&frame);
                }
            },
        }
    }

    /// Sits down on the window edge now and then while perched and calm;
    /// stands up when the sit is over or anything else takes over.
    fn perch_sit(&mut self, dt: f32, frame: &Frame) {
        let Mode::Perched(p) = &self.gravity.mode else {
            return;
        };
        let rect = p.rect;
        let anim = self.stage.animator();
        if let Some(mut sit) = p.sit.clone() {
            // Its own landing or sit-down blend is not an interruption.
            let interrupted = anim.base() != sit.clip
                || anim.overlay_clip().is_some_and(|o| !own_sit_blend(&sit, o));
            // The landing blend handed over to the seated base: its seat
            // line from now on.
            if sit
                .landing
                .as_ref()
                .is_some_and(|l| anim.overlay_clip() != Some(l.clip.as_str()))
            {
                sit.landing = None;
            }
            if let Some(e) = &mut sit.sink {
                e.step(dt);
                if e.done() {
                    sit.sink = None;
                }
            }
            sit.left_s -= dt;
            let over = sit.left_s <= 0.0;
            if let Mode::Perched(p) = &mut self.gravity.mode {
                p.sit = Some(sit);
            }
            if interrupted {
                self.stand_up(false);
            } else if over {
                self.stand_up(true);
            }
            return;
        }
        if let Some(mut r) = p.rise {
            r.ease.step(dt);
            if let Mode::Perched(p) = &mut self.gravity.mode {
                p.rise = (!r.ease.done()).then_some(r);
            }
            return;
        }
        let calm = self.brain.phase() == Phase::Idle
            && anim.overlay_clip().is_none()
            && anim.queued() == 0
            && !anim.stopping()
            && !anim.base_has_root_motion()
            && !self.talk.active()
            && self.showoff.is_empty();
        if !calm {
            return;
        }
        let next = p.next_sit_s - dt;
        if next > 0.0 {
            if let Mode::Perched(p) = &mut self.gravity.mode {
                p.next_sit_s = next;
            }
            return;
        }
        let m = self.stage.animator().manifest();
        let scale = self.stage.scale();
        let cur = self.pending_move.unwrap_or(self.placement);
        let feet_x = frame.feet_at(cur).0;
        // (clip, seat cell, shift): the sits, plus the lounges that fit along
        // this window top (shifted onto it if need be; too narrow: sits only).
        let mut seated: Vec<(String, i32, i32)> = sit_clips(m)
            .into_iter()
            .filter_map(|n| seat_cell(&m.clips[&n]).map(|d| (n, d, 0)))
            .collect();
        seated.extend(lounge_clips(m).into_iter().filter_map(|n| {
            let c = &m.clips[&n];
            let d = lounge_cell(c)?;
            let dx = lounge_fit(c, scale, feet_x, rect)?;
            Some((n, d, dx))
        }));
        let Some(i) = self.gravity.pick_sit(seated.len()) else {
            return;
        };
        let (clip, seat_cell, dx) = seated[i].clone();
        let Ok(blend_s) = self.stage.animator_mut().sit_down(&clip) else {
            return;
        };
        if dx != 0 {
            self.shift_on_perch(dx);
        }
        let left_s = self.gravity.sit_length();
        // Through a blend the window sinks with it; else all at once.
        let sink = blend_s.filter(|&s| s > 0.0).map(gravity::Ease::new);
        tracing::debug!(left_s, clip = %clip, blend = blend_s.is_some(), "perch: sitting on the edge");
        if let Mode::Perched(p) = &mut self.gravity.mode {
            p.sit = Some(Sit {
                clip,
                left_s,
                seat_cell,
                sink,
                landing: None,
            });
        }
    }

    /// Moves her `dx` px along her window top (a lounge fitting onto it).
    fn shift_on_perch(&mut self, dx: i32) {
        let cur = self.pending_move.unwrap_or(self.placement);
        let to = Placement {
            left: cur.left + dx,
            ..cur
        };
        self.pending_move = Some(to);
        self.brain.set_position(to.left as f32);
        tracing::debug!(dx, "perch: lounge shifted onto the window");
    }

    /// Her window was resized while she lounges on it: back onto it, or up
    /// when it is now too narrow for the lounge.
    fn lounge_refit(&mut self, frame: &Frame) {
        let Some(p) = self.gravity.perch() else {
            return;
        };
        let Some(sit) = &p.sit else { return };
        let m = self.stage.animator().manifest();
        let Some(c) = m
            .clips
            .get(&sit.clip)
            .filter(|c| c.has_tag(LOUNGE_BASE_TAG))
        else {
            return;
        };
        let cur = self.pending_move.unwrap_or(self.placement);
        match lounge_fit(c, self.stage.scale(), frame.feet_at(cur).0, p.rect) {
            Some(0) => {}
            Some(dx) => self.shift_on_perch(dx),
            None => self.stand_up(false),
        }
    }

    /// Keeps her resting on her window at the current scale (resize, the
    /// tease, a sit or stand blend): feet on its top, or the seat line when
    /// sitting.
    fn perch_hold(&mut self, frame: &Frame) {
        let Mode::Perched(p) = &self.gravity.mode else {
            return;
        };
        let bottom = p.bottom(frame, self.stage.scale());
        let cur = self.pending_move.unwrap_or(self.placement);
        if cur.bottom != bottom {
            self.pending_move = Some(Placement { bottom, ..cur });
        }
    }

    /// Logs once that without the bridge she only knows the floor.
    fn hint_no_bridge(&mut self, desk: &Desktop) {
        if !desk.has_bridge && !self.gravity.hinted && self.desktop_override.is_none() {
            self.gravity.hinted = true;
            tracing::info!(
                "gravity: no window list from the KWin bridge, so she lands on the floor                  ([behavior] floor_offset_px). To perch on windows, install and enable the                  kwin/lillith-bridge KWin script (INSTALL.md)."
            );
        }
    }

    pub fn has_screen(&self) -> bool {
        self.screen.is_some()
    }

    /// A wander step the backend should apply (bottom unchanged).
    pub fn take_move(&mut self) -> Option<Placement> {
        self.pending_move.take()
    }

    /// `true` once Q/Esc asked to quit.
    pub fn quit_requested(&self) -> bool {
        self.quit
    }

    /// Advances the brain and the stage by `dt` seconds.
    pub fn tick(&mut self, dt: f32) -> Result<Changed, OverlayError> {
        self.clock += dt;
        self.outfit_check_s += dt;
        if self.outfit_check_s >= OUTFIT_DATE_CHECK_S {
            self.check_outfit_date();
        }
        self.drain_feed();
        self.drain_settings();
        if self.click_gate.poll(self.clock, self.double_click_s) {
            tracing::info!("single click survived the double-click window: opening chat");
            self.open_chat();
        }
        let anim = self.stage.animator();
        // While she talks the brain does not start walks or idle poses.
        let busy = anim.overlay_clip().is_some()
            || anim.queued() > 0
            || self.talk.active()
            || self.gravity.sitting()
            || self.tease.active();
        let actions = self.brain.tick(dt, busy);
        self.apply(actions);
        self.pump_lipsync();
        self.talk.body_free =
            !self.drag.active() && !self.tease.active() && !self.gravity.sitting();
        self.talk.tick(dt, self.stage.animator_mut());
        self.pump_showoff();
        let view = self.talk.speech.as_ref().map(|s| s.view());
        self.stage.set_speech(view);
        if self.stage.avatar_id().is_some() {
            let talk = self.talk.active().then_some(match self.talk.mouth_now {
                Some(lillith_anim::talk::Mouth::Open) => 1.0,
                Some(lillith_anim::talk::Mouth::Mid) => 0.55,
                Some(lillith_anim::talk::Mouth::Smile) => 0.2,
                _ => 0.0,
            });
            self.stage
                .set_avatar_inputs(talk, self.hovered, self.drag_vx);
        }
        let changed = self.stage.tick(dt)?;
        // Walks with root motion move the window by exactly what the drawn
        // frames stepped, so the planted foot stays put on the desktop.
        let dx = self.stage.take_root_motion_px();
        let actions = self.brain.root_motion(dx);
        self.apply(actions);
        let anim = self.stage.animator();
        self.brain
            .set_root_motion(anim.base_has_root_motion() || anim.stopping());
        self.gravity_tick(dt);
        self.tease_tick(dt);
        Ok(changed)
    }

    // ---- "come closer" tease ----

    /// Her normal scale (the tease's base while one runs).
    pub fn normal_scale(&self) -> f32 {
        self.tease_run
            .as_ref()
            .map_or(self.stage.scale(), |r| r.base_scale)
    }

    /// `true` while she is idle on her own, so tease time may count.
    fn tease_calm(&self) -> bool {
        let anim = self.stage.animator();
        self.brain.phase() == Phase::Idle
            && !self.brain.drowsy()
            && !self.brain.hovering()
            && !self.hovered
            && anim.overlay_clip().is_none()
            && anim.queued() == 0
            && !anim.stopping()
            && !anim.base_has_root_motion()
            && !self.talk.active()
            && self.showoff.is_empty()
            && self.peek.is_none()
            && !self.drag.active()
            && self.menu.is_none()
            && !self.gravity.sitting()
            && !matches!(self.gravity.mode, Mode::Held | Mode::Falling(_))
            && self.stage.avatar_id().is_none()
    }

    /// Advances the tease: schedules it, switches its clips per phase and
    /// eases the window scale.
    fn tease_tick(&mut self, dt: f32) {
        let calm = self.tease_calm();
        match self.tease.tick(dt, calm) {
            Some(TeaseCue::Due) => self.start_tease(),
            Some(TeaseCue::Phase(p)) => self.tease_phase(p),
            Some(TeaseCue::Done) => {
                self.finish_tease();
                return;
            }
            None => {}
        }
        if let Some(run) = &self.tease_run {
            let target = run.base_scale * self.tease.factor();
            let diff = (target - self.stage.scale()).abs();
            // Small steps are batched while easing; at the peak it lands exactly.
            let held = matches!(
                self.tease.phase(),
                Some(TeasePhase::Beat | TeasePhase::Pause)
            );
            if diff >= TEASE_SCALE_STEP * run.base_scale || (held && diff > 0.0) {
                self.apply_tease_scale(target);
            }
        }
    }

    /// Starts a tease now if the clips exist and there is room to grow.
    fn start_tease(&mut self) {
        let anim = self.stage.animator();
        let Some(plan) = self.tease.config().plan(|n| anim.has_clip(n)) else {
            tracing::debug!("tease: no beat clip in this set; skipped");
            self.tease.postpone();
            return;
        };
        let beat_s = anim
            .manifest()
            .clips
            .get(&plan.beat)
            .map_or(0.0, |c| c.duration_s());
        let base_scale = self.stage.scale();
        let size = self.stage.size();
        let cur = self.pending_move.unwrap_or(self.placement);
        let peak = fit_peak(
            self.tease.config().peak_scale,
            size,
            self.screen,
            cur.bottom,
        );
        if peak <= 1.0 + 1e-3 {
            tracing::debug!("tease: no room to grow; skipped");
            self.tease.postpone();
            return;
        }
        tracing::info!(?plan, peak, base_scale, "tease: come closer");
        let center_x = cur.left + size.0 as i32 / 2;
        self.tease_run = Some(TeaseRun {
            plan,
            base_scale,
            center_x,
        });
        self.tease.begin(peak, beat_s);
        self.hold_tease_frame(base_scale * peak, center_x);
        self.tease_phase(TeasePhase::Approach);
    }

    /// Grows the window once to the tease's peak size (centred on
    /// `center_x`, kept on screen) while she is still drawn at her normal
    /// size; the easing then only changes her drawing scale inside it. One
    /// resize per tease edge instead of one per easing step: no flicker and
    /// no freshly resized, not-yet-painted window area mid-animation.
    fn hold_tease_frame(&mut self, scale: f32, center_x: i32) {
        let (fw, _) = self.stage.body_size_at(scale);
        let mut left = center_x - fw as i32 / 2;
        if let Some((sw, _)) = self.screen {
            left = keep_on_screen(left, fw, sw);
        }
        let before = self.stage.size();
        let frame = crate::stage::StageFrame {
            scale,
            center_x: (center_x - left) as f32,
        };
        if let Err(e) = self.stage.set_frame(Some(frame)) {
            tracing::warn!("tease frame: {e}");
            return;
        }
        if self.stage.size() != before {
            self.resized = true;
        }
        let cur = self.pending_move.unwrap_or(self.placement);
        if left != cur.left {
            self.pending_move = Some(Placement { left, ..cur });
        }
        self.brain.set_position(left as f32);
        self.apply_area();
    }

    /// Clips for a tease phase.
    fn tease_phase(&mut self, phase: TeasePhase) {
        let Some(run) = self.tease_run.clone() else {
            return;
        };
        tracing::debug!(?phase, "tease phase");
        let anim = self.stage.animator_mut();
        let base = match phase {
            TeasePhase::Approach | TeasePhase::Retreat => run.plan.approach.as_deref(),
            TeasePhase::Beat => {
                // The beat plays once over the plain idle.
                anim.stand();
                if let Err(e) = anim.react(&run.plan.beat, TEASE_PRIORITY) {
                    tracing::debug!("tease beat {}: {e}", run.plan.beat);
                }
                return;
            }
            TeasePhase::Pause => run.plan.pause.as_deref(),
        };
        match base {
            Some(clip) => {
                let _ = anim.set_base(clip, false);
            }
            None => anim.stand(),
        }
    }

    /// The tease is over: back on the idle, exactly at her normal size.
    fn finish_tease(&mut self) {
        let Some(run) = self.tease_run.take() else {
            return;
        };
        self.stage.animator_mut().stand();
        if self.stage.frame().is_some() {
            let before = self.stage.size();
            if let Err(e) = self.stage.set_frame(None) {
                tracing::warn!("tease frame: {e}");
            }
            if self.stage.size() != before {
                self.resized = true;
            }
        }
        self.apply_tease_scale_around(run.base_scale, run.center_x);
        tracing::debug!(scale = run.base_scale, "tease: done");
    }

    /// Ends a tease at once (drag, resize, minimize): her normal size now.
    pub fn cancel_tease(&mut self) {
        if self.tease.cancel() {
            self.finish_tease();
        }
    }

    /// A click / pose while teasing: she backs off smoothly.
    pub fn retreat_tease(&mut self) {
        if let Some(TeaseCue::Phase(p)) = self.tease.retreat_now() {
            self.tease_phase(p);
        }
    }

    fn apply_tease_scale(&mut self, scale: f32) {
        let Some(cx) = self.tease_run.as_ref().map(|r| r.center_x) else {
            return;
        };
        self.apply_tease_scale_around(scale, cx);
    }

    /// Resizes her to `scale` without saving it, keeping the window centred
    /// on `center_x`, her feet on the same line and the window on screen.
    fn apply_tease_scale_around(&mut self, scale: f32, center_x: i32) {
        if self.stage.frame().is_some() {
            // Window held at the peak size: only her drawing scale changes.
            let before = self.stage.size();
            if let Err(e) = self.stage.set_scale(scale) {
                tracing::warn!("tease scale: {e}");
            }
            if self.stage.size() != before {
                self.resized = true;
            }
            return;
        }
        if (self.stage.scale() - scale).abs() > f32::EPSILON {
            if let Err(e) = self.stage.set_scale(scale) {
                tracing::warn!("tease resize: {e}");
                return;
            }
            self.resized = true;
        }
        let w = self.stage.size().0;
        let mut left = center_x - w as i32 / 2;
        if let Some((sw, _)) = self.screen {
            left = keep_on_screen(left, w, sw);
        }
        let cur = self.pending_move.unwrap_or(self.placement);
        if left != cur.left {
            self.pending_move = Some(Placement { left, ..cur });
        }
        self.brain.set_position(left as f32);
        self.apply_area();
    }

    /// Asks the app to open the chat bubble next to her.
    pub fn open_chat(&mut self) {
        let Some(hook) = &self.chat else {
            tracing::info!("chat requested but chat is disabled ([chat] enabled = false)");
            return;
        };
        let (w, h) = self.stage.size();
        let (x, y) = match self.screen {
            Some((_, sh)) => self.placement.to_xy(h, sh),
            None => (self.placement.left, 0),
        };
        // Anchor the chat window to her body, not the speech area above it.
        let pad = self.stage.speech_pad();
        tracing::info!(x, y, w, h, screen = ?self.screen, "chat bubble requested");
        hook.open(ChatAnchor {
            x,
            y: y + pad as i32,
            w,
            h: h - pad,
            screen: self.screen,
        });
    }

    /// Applies brain actions to the animator / window.
    pub fn apply(&mut self, actions: Vec<Action>) {
        if actions.is_empty() {
            return;
        }
        if let Some(sink) = &self.sink {
            // Non-blocking by contract (the voice only queues the line).
            if let Ok(mut s) = sink.0.lock() {
                s.on_actions(&actions);
            }
        }
        for a in actions {
            let anim = self.stage.animator_mut();
            match a {
                Action::React { clip, priority } => {
                    // While she talks, only important events interrupt her.
                    if self.talk.active() && priority < TALK_INTERRUPT_PRIORITY {
                        tracing::debug!(clip, priority, "react skipped (talking)");
                        continue;
                    }
                    tracing::debug!(clip, priority, "react");
                    if anim.has_clip(&clip) {
                        let _ = anim.react(&clip, priority);
                    }
                }
                Action::SetBase { clip } => {
                    if anim.has_clip(&clip) {
                        let _ = anim.set_base(&clip, false);
                    }
                }
                Action::Stand => anim.stand(),
                Action::Walk { .. } | Action::MoveTo { .. } if self.peek.is_some() => {}
                Action::Walk { right } => {
                    tracing::debug!(right, "wander walk");
                    let dir = if right { Dir::Right } else { Dir::Left };
                    if anim.walk(dir, false).is_err() {
                        tracing::debug!("no walk clip; gliding");
                    }
                }
                Action::MoveTo { left } => {
                    self.pending_move = Some(Placement {
                        left,
                        bottom: self.placement.bottom,
                    });
                }
                Action::Say(line) => {
                    tracing::info!("💬 {line}");
                    // Banter shows in her speech bubble too (a chat reply
                    // in progress keeps the bubble).
                    self.talk.show_line(&line);
                }
                Action::Sfx(cue) => tracing::debug!(cue, "sfx (audio lands with lillith-voice)"),
                Action::MoodChanged(m) => tracing::info!(mood = ?m, "mood"),
                Action::ContextMenu => self.open_menu(),
                Action::OpenChat => self.open_chat(),
                Action::Muted(m) => tracing::info!(muted = m, "sound"),
                Action::ModeChanged(m) => tracing::info!(mode = ?m, "movement mode"),
                Action::Quit => {
                    tracing::info!("quit requested");
                    self.quit = true;
                }
            }
        }
    }

    /// Left button down at `at`. Ignored unless it is on her (X11 may
    /// still deliver presses on transparent pixels while the input shape
    /// catches up).
    pub fn press(&mut self, at: (f64, f64), hit_at: (f64, f64), local: bool) -> bool {
        if !self.stage.hit(hit_at.0, hit_at.1) {
            return false;
        }
        self.press_trusted(at, local);
        true
    }

    /// Left button down where the compositor already decided it is on her
    /// (Wayland: it only delivers buttons inside the input region, so a
    /// second hit test against a mask that may be a frame newer or older
    /// than the committed region can only lose real clicks).
    pub fn press_trusted(&mut self, at: (f64, f64), local: bool) {
        self.close_menu();
        self.drag.press(at, self.placement, local);
    }

    /// Right/middle button pressed at window-local `hit_at`.
    pub fn button(&mut self, button: Button, hit_at: (f64, f64)) -> bool {
        if !self.stage.hit(hit_at.0, hit_at.1) {
            return false;
        }
        self.button_trusted(button);
        true
    }

    /// Right/middle button where the compositor already decided it is on
    /// her (see [`Controller::press_trusted`]).
    pub fn button_trusted(&mut self, button: Button) {
        tracing::info!(?button, "button on her");
        let a = self.brain.input(Input::Click(button));
        let menu = a.iter().any(|a| matches!(a, Action::ContextMenu));
        self.apply(a);
        // The brain's rule table may not map right click to the menu: a
        // right click on her always opens it.
        if button == Button::Right {
            if menu {
                tracing::info!("right click: ContextMenu fired by the rule table");
            } else {
                tracing::info!("right click: ContextMenu (built-in)");
                self.open_menu();
            }
        }
    }

    /// Pointer entered/left her silhouette.
    pub fn hover(&mut self, inside: bool) {
        if inside && self.drag.active() {
            return;
        }
        self.hovered = inside;
        let a = self.brain.input(Input::Hover(inside));
        self.apply(a);
    }

    /// A key while she has keyboard focus.
    pub fn key(&mut self, key: Key) {
        if self.menu.is_some() {
            // Esc / Enter go to the open menu (Esc closes it instead of
            // quitting); other keys keep their usual meaning.
            let mk = match key {
                Key::Escape => Some(MenuKey::Escape),
                Key::Enter => Some(MenuKey::Enter),
                _ => None,
            };
            if let Some(k) = mk {
                self.menu_key(k);
                return;
            }
        }
        let a = self.brain.input(Input::Key(key));
        self.apply(a);
    }

    /// A bus event (system monitor feed, tests, the injector).
    pub fn event(&mut self, ev: &Event) {
        if crate::ui::is_ui_event(&ev.name) {
            match Command::from_event(&ev.name, ev.detail.as_deref()) {
                Some(c) => self.command(c),
                None => tracing::debug!(event = %ev.name, "ui event ignored"),
            }
            return;
        }
        if ev.name == CHAT_BARGE_IN || ev.name == CHAT_SAFEWORD {
            let safeword = ev.name == CHAT_SAFEWORD;
            if safeword {
                // The safeword stops her talking (not her work: running
                // commands and the admin helper are left alone).
                tracing::warn!("safeword from the chat bubble: she stops talking");
                lillith_core::access::kill_switch::hush();
                // The reply in flight is dead: wrap up what's shown, and
                // nothing of it still queued is shown or spoken.
                self.chat_stopped = true;
                self.talk
                    .event(crate::chat::CHAT_ERROR, None, self.stage.animator_mut());
            }
            self.talk.reset_lipsync();
            if let Some(sink) = &self.sink {
                if let Ok(mut s) = sink.0.lock() {
                    s.interrupt(if safeword { "safeword" } else { "barge_in" });
                }
            }
            return;
        }
        if ev.name.starts_with("chat.") {
            if ev.name == crate::chat::CHAT_START {
                self.chat_stopped = false;
            } else if self.chat_stopped
                && (ev.name == crate::chat::CHAT_TOKEN || ev.name == crate::chat::CHAT_END)
            {
                tracing::debug!(event = %ev.name, "dropped: reply cut by the safeword");
                return;
            }
            self.talk
                .event(&ev.name, ev.detail.as_deref(), self.stage.animator_mut());
            if ev.name == crate::chat::CHAT_END {
                self.speak_reply(ev.detail.as_deref());
            }
            return;
        }
        tracing::debug!(event = %ev.name, detail = ?ev.detail, "event");
        let a = self.brain.event(ev);
        self.apply(a);
    }

    /// Feeds the voice daemon's lip-sync lines to her mouth (every frame,
    /// before `talk.tick`; not through the event feed, which is capped and
    /// paused with the monitor).
    fn pump_lipsync(&mut self) {
        let Some(sink) = &self.sink else { return };
        let mut lines = Vec::new();
        let gone = sink
            .0
            .lock()
            .map(|mut s| s.lipsync_lines(&mut lines))
            .unwrap_or(false);
        if gone {
            self.talk.reset_lipsync();
        }
        if !lines.is_empty() {
            // The voice daemon is speaking: hold the talk clip between the
            // text and the voice.
            self.talk.set_hold(DAEMON_TALK_HOLD_S);
        }
        for l in &lines {
            self.talk.feed_lipsync(l);
        }
    }

    /// Hands a finished chat reply to the voice (it decides whether the
    /// speakers are on; text in the bubbles is unaffected).
    fn speak_reply(&mut self, reply: Option<&str>) {
        let Some(reply) = reply.map(str::trim).filter(|r| !r.is_empty()) else {
            return;
        };
        if let Some(sink) = &self.sink {
            if let Ok(mut s) = sink.0.lock() {
                s.on_chat_reply(reply);
            }
        }
    }

    /// Applies pending [`EventFeed`] events (at most [`MAX_EVENTS_PER_TICK`]).
    ///
    /// The safeword and barge-in ([`CHAT_SAFEWORD`], [`CHAT_BARGE_IN`])
    /// share this feed with the chat bubble but are applied first, every
    /// frame: never dropped while the system monitor is paused and never
    /// queued behind a token burst.
    fn drain_feed(&mut self) {
        let Some(feed) = &self.feed else { return };
        if let Ok(rx) = feed.lock() {
            self.feed_backlog.extend(rx.try_iter());
        }
        // Chat events queued before the (last) safeword belong to the reply
        // it killed: drop them so they are never shown or spoken.
        if let Some(last) = self
            .feed_backlog
            .iter()
            .rposition(|e| e.name == CHAT_SAFEWORD)
        {
            let mut i = 0;
            self.feed_backlog.retain(|e| {
                let reply = [
                    crate::chat::CHAT_START,
                    crate::chat::CHAT_TOKEN,
                    crate::chat::CHAT_END,
                ]
                .contains(&e.name.as_str());
                let keep = i >= last || !reply;
                i += 1;
                keep
            });
        }
        let (urgent, rest): (std::collections::VecDeque<Event>, _) =
            std::mem::take(&mut self.feed_backlog)
                .into_iter()
                .partition(|ev| ev.name == CHAT_SAFEWORD || ev.name == CHAT_BARGE_IN);
        self.feed_backlog = rest;
        for ev in &urgent {
            self.event(ev);
        }
        if self.monitor_paused {
            if !self.feed_backlog.is_empty() {
                tracing::debug!(dropped = self.feed_backlog.len(), "system monitor paused");
                self.feed_backlog.clear();
            }
            return;
        }
        let n = self.feed_backlog.len().min(MAX_EVENTS_PER_TICK);
        let batch: Vec<Event> = self.feed_backlog.drain(..n).collect();
        for ev in &batch {
            self.event(ev);
        }
    }

    /// Applies picks from the settings window like right-click picks (on
    /// this thread: brain, voice sink and effects).
    fn drain_settings(&mut self) {
        for p in self.settings.drain() {
            tracing::info!(?p, "settings window pick");
            self.settings_pick(p);
        }
    }

    /// Applies one wire pick from the settings window: menu actions like a
    /// right-click pick; poses / avatars by name; config reloads.
    pub fn settings_pick(&mut self, p: MenuPick) {
        match p {
            MenuPick::Action(a) => self.menu_pick(a),
            MenuPick::Pose(clip) => self.command(Command::Pose(clip)),
            MenuPick::Avatar(id) => self.command(Command::Avatar(id)),
            MenuPick::ReloadConfig => self.command(Command::ConfigChanged),
            MenuPick::SeasonalOutfit(id) => self.set_seasonal_outfit(&id, true),
        }
    }

    /// "Settings…": the separate `lillith-settings <config.toml>` window
    /// (scaffold's launcher); `xdg-open config.toml` if it can't start.
    fn open_settings_window(&mut self, default: Option<PathBuf>) {
        self.open_settings_at(default, None);
    }

    /// The settings window on tab `tab` (e.g. `icon`).
    fn open_settings_tab(&mut self, tab: Option<&str>) {
        self.open_settings_at(lillith_core::config::Config::default_path(), tab);
    }

    fn open_settings_at(&mut self, default: Option<PathBuf>, tab: Option<&str>) {
        let Some(path) = self.settings_path.clone().or(default) else {
            tracing::warn!("open settings: no config path (no $HOME?)");
            return;
        };
        match self
            .settings
            .open_tab(&path, Some(&self.manifest_path), tab)
        {
            Launch::Spawned => tracing::info!(
                program = %self.settings.program().display(),
                path = %path.display(),
                "settings window opened"
            ),
            Launch::AlreadyRunning => tracing::info!("settings window already open"),
            Launch::Failed(e) => {
                tracing::warn!(
                    "could not start the settings window `{}` ({e}); \
                     opening config.toml in the default editor instead",
                    self.settings.program().display()
                );
                open_settings(&path);
            }
        }
    }

    /// Pointer moved; returns the new placement while dragging.
    pub fn motion(&mut self, at: (f64, f64)) -> Option<Placement> {
        let was = self.drag.dragging();
        let p = self.drag.motion(at, self.placement)?;
        // Drag velocity for the custom avatar's lean (low-passed there).
        let now = self.clock;
        if let Some((t0, x0)) = self.last_drag_x {
            let dt = (now - t0).max(1.0 / 240.0);
            let v = ((p.left - x0) as f32 / dt).clamp(-3000.0, 3000.0);
            let prev = self.drag_vx.unwrap_or(0.0);
            self.drag_vx = Some(prev + (v - prev) * 0.35);
        }
        self.last_drag_x = Some((now, p.left));
        if !was {
            self.cancel_tease();
            self.drop_sit_blend();
            self.gravity.mode = Mode::Held;
            self.click_gate.cancel();
            let a = self.brain.input(Input::DragStart);
            self.apply(a);
            self.pending_move = None;
        }
        Some(p)
    }

    /// Button released; persists the position after a drag.
    pub fn release(&mut self) -> Release {
        let r = self.drag.release(self.placement);
        self.drag_vx = None;
        self.last_drag_x = None;
        if self.peek.is_some() && !matches!(r, Release::None) {
            if matches!(r, Release::Click) {
                self.restore();
            }
            return r;
        }
        match r {
            Release::Dropped(p) if self.gravity_active() => {
                // She keeps the drag pose and falls; landing ends the drag
                // in the brain and saves the spot (`gravity_tick`).
                self.gravity.mode = Mode::Falling(gravity::Fall::new(p.bottom));
            }
            Release::Dropped(p) => {
                self.gravity.mode = Mode::Settle { wait: 0.0 };
                let a = self.brain.input(Input::DragEnd { left: p.left });
                self.apply(a);
                self.persist(p);
            }
            Release::Click => {
                self.retreat_tease();
                tracing::info!(
                    chat_on_click = self.chat_on_click && self.chat.is_some(),
                    "left click on her (click, not drag)"
                );
                self.stage.avatar_react(0.9);
                let a = self.brain.input(Input::Click(Button::Left));
                self.apply(a);
                if self.chat.is_some() && self.chat_on_click {
                    self.click_gate.click(self.clock, self.double_click_s);
                }
            }
            Release::None => {}
        }
        r
    }

    // ---- right-click menu (M6) ----

    /// Opens the right-click menu for the current state (replaces an open one).
    pub fn open_menu(&mut self) {
        self.open_menu_page(None);
    }

    /// Opens the menu at a Poses page (`None` = the main menu).
    pub fn open_menu_page(&mut self, page: Option<MenuPage>) {
        self.open_menu_at(page, false);
    }

    /// Opens the menu at the Avatar ▸ Outfit page.
    pub fn open_outfit_page(&mut self) {
        self.open_menu_at(None, true);
    }

    fn open_menu_at(&mut self, page: Option<MenuPage>, outfit_page: bool) {
        self.check_outfit_date();
        self.menu_outfit_page = outfit_page;
        let state = self.menu_state(page);
        self.menu_page = page;
        tracing::info!(?page, outfit_page, "right-click menu opened");
        let mut menu = MenuPopup::new(state);
        menu.set_accent(self.accent);
        self.menu = Some(menu);
        self.menu_opened = true;
    }

    /// Everything the menu shows: brain state plus poses (from the
    /// animation set), avatars and chat availability.
    pub fn menu_state(&self, page: Option<MenuPage>) -> MenuState {
        let scale = self.peek.map_or(self.normal_scale(), |p| p.scale);
        let m = self.stage.animator().manifest();
        let active = self.outfit_active();
        let poses = lillith_anim::catalog::grouped_for(m, active)
            .into_iter()
            .map(|(label, clips)| PoseGroup {
                label: label.to_string(),
                clips: clips
                    .into_iter()
                    .map(|c| (c.clone(), lillith_anim::catalog::label(&c)))
                    .collect(),
            })
            .collect();
        let avatars = self
            .avatars
            .as_ref()
            .map(|st| {
                st.selection()
                    .recent
                    .iter()
                    .filter_map(|id| st.load(id).ok().map(|a| (id.clone(), a.meta.name)))
                    .collect()
            })
            .unwrap_or_default();
        MenuState::from_brain(&self.brain, self.monitor_paused, scale).with_extras(MenuExtras {
            poses,
            page,
            avatars,
            avatar: self.stage.avatar_id().map(str::to_string),
            chat: self.chat.is_some(),
            speaker: self.speaker(),
            outfits: self
                .seasons
                .listed()
                .map(|o| OutfitChoice {
                    id: o.id.clone(),
                    label: o.label().to_string(),
                    available: o.available(|c| m.clips.contains_key(c)),
                })
                .collect(),
            outfit: self.outfit_setting.clone(),
            outfit_active: active.map(|o| o.label().to_string()),
            outfit_page: self.menu_outfit_page,
        })
    }

    /// The open menu, if any.
    pub fn menu(&self) -> Option<&MenuPopup> {
        self.menu.as_ref()
    }

    /// `true` once per newly opened menu: the backend should show (or
    /// re-show) its popup at the pointer.
    pub fn take_menu_opened(&mut self) -> bool {
        std::mem::take(&mut self.menu_opened)
    }

    pub fn close_menu(&mut self) {
        if self.menu.take().is_some() {
            tracing::debug!("menu closed");
        }
        self.menu_opened = false;
    }

    /// Pointer over the popup at popup-local `at` (`None` = left it).
    /// Returns `true` if it needs a redraw.
    pub fn menu_pointer(&mut self, at: Option<(f64, f64)>) -> bool {
        self.menu.as_mut().is_some_and(|m| m.pointer(at))
    }

    /// Left click at popup-local `at` (outside = dismiss).
    pub fn menu_click(&mut self, at: (f64, f64)) -> MenuHit {
        let Some(m) = self.menu.as_mut() else {
            return MenuHit::None;
        };
        let hit = m.click(at.0, at.1);
        self.on_menu_hit(hit);
        hit
    }

    /// A key while the menu is open.
    pub fn menu_key(&mut self, key: MenuKey) -> MenuHit {
        let Some(m) = self.menu.as_mut() else {
            return MenuHit::None;
        };
        let hit = m.key(key);
        self.on_menu_hit(hit);
        hit
    }

    fn on_menu_hit(&mut self, hit: MenuHit) {
        match hit {
            MenuHit::Dismiss => self.close_menu(),
            MenuHit::Pick(action) => {
                self.close_menu();
                self.menu_pick(action);
            }
            MenuHit::Changed | MenuHit::None => {}
        }
    }

    /// Forces the next [`Controller::render_menu`] to redraw (expose).
    pub fn invalidate_menu(&mut self) {
        if let Some(m) = self.menu.as_mut() {
            m.invalidate();
        }
    }

    /// Redraws the popup if it changed (`Some` = new pixels to present).
    pub fn render_menu(&mut self) -> Option<&tiny_skia::Pixmap> {
        let m = self.menu.as_mut()?;
        m.render(&mut self.fonts)
    }

    /// Applies a picked leaf: brain actions through [`Controller::apply`]
    /// (and so the voice), then the overlay-side effects.
    pub fn menu_pick(&mut self, action: MenuAction) {
        let state = self.menu_state(self.menu_page);
        tracing::info!(?action, "menu pick");
        let out = on_menu_action(action, &mut self.brain, &state);
        self.apply(out.actions);
        for e in out.effects {
            self.menu_effect(e);
        }
    }

    fn menu_effect(&mut self, e: MenuEffect) {
        match e {
            MenuEffect::SetMonitorPaused(p) => {
                self.monitor_paused = p;
                tracing::info!(paused = p, "system monitor");
            }
            MenuEffect::SetScale(s) => self.command(Command::Scale {
                scale: s,
                save: true,
            }),
            MenuEffect::PlayPose(clip) => self.command(Command::Pose(clip)),
            MenuEffect::ShowOff => self.command(Command::ShowOff),
            MenuEffect::OpenChat => self.command(Command::Chat),
            MenuEffect::Minimize => self.command(Command::Minimize),
            MenuEffect::SetAvatar(id) => self.command(Command::Avatar(id)),
            // Avatar import lives in the settings window (its Avatar tab).
            MenuEffect::ImportAvatar => self.command(Command::Settings),
            MenuEffect::OpenPage(page) => self.open_menu_page(page),
            MenuEffect::OpenOutfitPage => self.open_outfit_page(),
            MenuEffect::SetSeasonalOutfit(v) => self.set_seasonal_outfit(&v, true),
            MenuEffect::SetSpeaker(on) => {
                tracing::info!(on, "speak out loud (speakers)");
                if let Some(sink) = &self.sink {
                    if let Ok(mut s) = sink.0.lock() {
                        s.set_speaker(on);
                    }
                }
            }
            MenuEffect::OpenSettings(default) => self.open_settings_window(default),
            MenuEffect::ChooseIcon => self.open_settings_tab(Some("icon")),
            MenuEffect::ResetPosition(pos) => {
                let p = Placement::from_config(&pos);
                self.brain.set_position(p.left as f32);
                self.resettle();
                self.pending_move = Some(p);
                tracing::info!(left = p.left, bottom = p.bottom, "position reset");
                if let Some(path) = &self.config_path {
                    if let Err(e) = save_overlay(path, &pos) {
                        tracing::warn!("could not save position to {}: {e}", path.display());
                    }
                }
            }
            MenuEffect::ShowAbout(text) => {
                tracing::info!("about: {text}");
                self.apply(vec![Action::Say(text)]);
            }
        }
    }

    /// `true` once after the stage changed size: resize the window to
    /// `stage.size()` and redraw.
    pub fn take_resized(&mut self) -> bool {
        std::mem::take(&mut self.resized)
    }

    /// Logs how much rasterising the session needed (CPU budget check).
    pub fn log_stats(&self) {
        let n = self.stage.renders().max(1);
        tracing::info!(
            renders = self.stage.renders(),
            avg_render_us = (self.stage.render_time().as_micros() as u64) / n,
            "overlay stats"
        );
        let (v, pending) = self.stage.video_stats();
        tracing::info!(
            video_decodes = v.decodes,
            video_resident_clips = v.resident_clips,
            video_resident_mb = v.resident_bytes >> 20,
            video_evictions = v.evictions,
            video_failures = v.failures,
            video_pending_renders = pending,
            png_pages_mb = self.stage.page_bytes() >> 20,
            "pose video stats"
        );
    }

    /// Saves `p` to `[overlay]` in config.toml (logs, never fails the app).
    pub fn persist(&self, p: Placement) {
        let Some(path) = &self.config_path else {
            return;
        };
        let pos = OverlayPos {
            margin_left: p.left,
            margin_bottom: p.bottom,
            output: self.output.clone(),
            ..OverlayPos::default()
        };
        match save_overlay(path, &pos) {
            Ok(()) => {
                tracing::info!(left = p.left, bottom = p.bottom, path = %path.display(), "position saved")
            }
            Err(e) => tracing::warn!("could not save position to {}: {e}", path.display()),
        }
    }
}

/// "Open settings" fallback (no `lillith-settings`): writes the template
/// if the file is missing, then
/// `xdg-open`s it without blocking (a thread reaps the child).
fn open_settings(path: &std::path::Path) {
    if !path.exists() {
        let r = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(path, settings_template()));
        match r {
            Ok(()) => tracing::info!(path = %path.display(), "settings file created"),
            Err(e) => {
                tracing::warn!("could not create {}: {e}", path.display());
                return;
            }
        }
    }
    // Not gated by a card; one audit line records the launch.
    match lillith_core::menu::spawn_open_settings(path, lillith_core::config::audit_log_path()) {
        Ok(mut child) => {
            tracing::info!(path = %path.display(), "opening settings");
            let _ = std::thread::Builder::new()
                .name("xdg-open".into())
                .spawn(move || {
                    let _ = child.wait();
                });
        }
        Err(e) => tracing::warn!("xdg-open {}: {e}", path.display()),
    }
}

/// Runs the overlay until she is closed. With an auto-detected Wayland
/// backend, a compositor without layer-shell falls back to X11/XWayland.
pub fn run(backend: Backend, opts: OverlayOptions) -> Result<(), OverlayError> {
    #[cfg(not(windows))]
    tracing::info!(
        ?backend,
        forced = opts.forced,
        session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default(),
        wayland_display = std::env::var("WAYLAND_DISPLAY").unwrap_or_default(),
        display = std::env::var("DISPLAY").unwrap_or_default(),
        "overlay backend selected"
    );
    // Windows has one backend whatever `--backend` says.
    #[cfg(windows)]
    {
        tracing::info!(requested = ?backend, "overlay backend: win32");
        crate::win32::run(&opts)
    }
    #[cfg(not(windows))]
    match backend {
        Backend::Wayland => match run_wayland(&opts) {
            Err(OverlayError::WaylandUnavailable(why)) if !opts.forced => {
                tracing::warn!("{why}; falling back to the X11 backend");
                run_x11(&opts)
            }
            Err(OverlayError::NotBuilt(_)) if !opts.forced => run_x11(&opts),
            r => r,
        },
        Backend::X11 => run_x11(&opts),
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn run_wayland(opts: &OverlayOptions) -> Result<(), OverlayError> {
    #[cfg(feature = "wayland")]
    {
        crate::wayland::run(opts)
    }
    #[cfg(not(feature = "wayland"))]
    {
        let _ = opts;
        Err(OverlayError::NotBuilt("wayland"))
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn run_x11(opts: &OverlayOptions) -> Result<(), OverlayError> {
    #[cfg(feature = "x11")]
    {
        crate::x11::run(opts)
    }
    #[cfg(not(feature = "x11"))]
    {
        let _ = opts;
        Err(OverlayError::NotBuilt("x11"))
    }
}

/// Frame period for `hz` (clamped 1..=120).
pub fn period(hz: f32) -> Duration {
    Duration::from_secs_f32(1.0 / hz.clamp(1.0, 120.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::tests::fixture;
    use lillith_anim::render::PageCache;
    use lillith_core::config::Config;
    use lillith_core::menu::Size;

    #[test]
    fn drag_persists_and_click_is_not_a_drag() {
        let (dir, m) = fixture("ctl");
        let cfg_path = dir.join("cfg/config.toml");
        let opts = OverlayOptions {
            manifest: dir.join("manifest.toml"),
            scale: 2.0,
            opacity: 1.0,
            render_hz: 30.0,
            animator: AnimatorConfig {
                idle_enabled: false,
                ..AnimatorConfig::default()
            },
            position: OverlayPos {
                margin_left: 100,
                margin_bottom: 50,
                output: None,
                ..OverlayPos::default()
            },
            config_path: Some(cfg_path.clone()),
            forced: false,
            exit_after: None,
            behavior: BehaviorConfig::default(),
            mode: MoveMode::Stay,
            muted: false,
            events: None,
            sink: None,
            settings_path: None,
            chat: None,
            chat_on_click: false,
            overlay_speech: false,
            always_on_top: true,
            click_through: false,
            avatars: None,
            avatar: Default::default(),
            seasonal_outfit: "auto".into(),
        };
        let stage = Stage::new(m, PageCache::new(&dir), 2.0, opts.animator.clone()).unwrap();
        let mut c = Controller::with_stage(stage, &opts);
        c.stage.render().unwrap();
        // Press on a transparent pixel is ignored (it would not even reach
        // us through the input region).
        assert!(!c.press((1.0, 1.0), (1.0, 1.0), true));
        // Click on her: no movement, nothing saved.
        assert!(c.press((40.0, 40.0), (40.0, 40.0), true));
        assert_eq!(c.release(), Release::Click);
        assert!(!cfg_path.exists());
        // Drag (surface-local, Wayland style) by +30, -20.
        assert!(c.press((40.0, 40.0), (40.0, 40.0), true));
        let p = c.motion((70.0, 20.0)).unwrap();
        assert_eq!(
            p,
            Placement {
                left: 130,
                bottom: 70
            }
        );
        c.placement = p;
        assert_eq!(c.release(), Release::Dropped(p));
        let saved = Config::load(&cfg_path).unwrap().overlay;
        assert_eq!((saved.margin_left, saved.margin_bottom), (130, 70));
        let _ = std::fs::remove_dir_all(dir);
    }

    fn m3_manifest_with(
        tag: &str,
        extra: &str,
    ) -> (std::path::PathBuf, lillith_anim::atlas::Manifest) {
        let (dir, _) = fixture(tag);
        let frames = r#"page = "idle.png"
fps = 8.0
frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 40, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
"#;
        let mut toml = String::new();
        for c in [
            "idle",
            "knife_tap",
            "wave",
            "drag_held",
            "walk_right",
            "sleep",
            "wake",
            "event_input_click_right",
        ] {
            toml.push_str(&format!("[clips.{c}]\n{frames}\n"));
            if c == "walk_right" {
                // Gait-rig ground speed (importer tag).
                toml.push_str("tags = [\"move_speed_px_s:40\"]\n");
            }
        }
        toml.push_str(extra);
        (
            dir,
            lillith_anim::atlas::Manifest::from_toml_str(&toml).unwrap(),
        )
    }

    /// Controller with the tease clips, due after 5 s of calm idle.
    fn tease_controller(tag: &str) -> (std::path::PathBuf, Controller) {
        let frames = r#"page = "idle.png"
fps = 8.0
frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] }, { x = 40, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
"#;
        let mut extra = String::new();
        for c in ["tease_closeup", "hip_sway", "blink_wink"] {
            extra.push_str(&format!("[clips.{c}]\n{frames}\n"));
        }
        let (dir, mut c) = m3_controller_with(tag, MoveMode::Stay, &extra);
        c.tease = Tease::new(
            lillith_core::tease::TeaseConfig {
                every_s: (5.0, 5.0),
                approach_clips: vec!["hip_sway".into()],
                ..Default::default()
            },
            1,
        );
        (dir, c)
    }

    #[test]
    fn tease_comes_closer_then_back_to_exactly_her_size() {
        let (dir, mut c) = tease_controller("tease_seq");
        let dt = 1.0 / 30.0;
        let base = c.stage.scale();
        let size0 = c.stage.size();
        let center0 = c.placement.left + size0.0 as i32 / 2;
        // Calm idle until it is due.
        let mut ticks = 0;
        while !c.tease.active() {
            c.tick(dt).unwrap();
            ticks += 1;
            assert!(ticks < 6 * 30, "tease never started");
        }
        assert_eq!(c.tease.phase(), Some(TeasePhase::Approach));
        assert_eq!(
            c.stage.animator().base(),
            "hip_sway",
            "sways up to the viewer"
        );
        let mut phases = vec![TeasePhase::Approach];
        let mut scales = vec![c.stage.scale()];
        let mut beat_played = false;
        let mut pause_base = String::new();
        for _ in 0..(14.0 / dt) as usize {
            if let Some(m) = c.take_move() {
                c.placement = m;
            }
            c.tick(dt).unwrap();
            scales.push(c.stage.scale());
            let anim = c.stage.animator();
            match c.tease.phase() {
                Some(p) if phases.last() != Some(&p) => phases.push(p),
                _ => {}
            }
            if c.tease.phase() == Some(TeasePhase::Beat)
                && anim.overlay_clip() == Some("tease_closeup")
            {
                beat_played = true;
            }
            if c.tease.phase() == Some(TeasePhase::Pause) {
                pause_base = anim.base().to_string();
            }
            if !c.tease.active() {
                break;
            }
        }
        assert_eq!(
            phases,
            [
                TeasePhase::Approach,
                TeasePhase::Beat,
                TeasePhase::Pause,
                TeasePhase::Retreat
            ]
        );
        assert!(beat_played, "tease_closeup played at the peak");
        assert_eq!(pause_base, "blink_wink", "held smile / wink");
        assert!(!c.tease.active(), "tease finished");
        let peak = scales.iter().cloned().fold(0.0, f32::max);
        assert!((peak - base * 1.6).abs() < 1e-3, "{peak}");
        // Smooth: no big jumps between frames.
        for w in scales.windows(2) {
            assert!((w[1] - w[0]).abs() < 0.02 * base, "{} -> {}", w[0], w[1]);
        }
        // Exactly back to her size, on the idle, centred where she was.
        assert_eq!(c.stage.scale(), base);
        assert_eq!(c.stage.size(), size0);
        assert_eq!(c.stage.animator().base(), "idle");
        if let Some(m) = c.take_move() {
            c.placement = m;
        }
        assert_eq!(c.placement.left + size0.0 as i32 / 2, center0);
        // Nothing was saved and the menu shows her normal size.
        assert!(!dir.join("cfg/config.toml").exists());
        assert_eq!(c.normal_scale(), base);
    }

    #[test]
    fn tease_resizes_the_window_once_each_way_and_stays_transparent() {
        let (dir, mut c) = tease_controller("tease_frame");
        let dt = 1.0 / 30.0;
        let size0 = c.stage.size();
        let _ = c.take_resized();
        let mut center0: Option<f32> = None;
        let mut sizes = vec![size0];
        let mut resizes = 0;
        let mut started = false;
        for _ in 0..(20.0 / dt) as usize {
            if let Some(m) = c.take_move() {
                c.placement = m;
            }
            c.tick(dt).unwrap();
            // The backend applies the move together with the new buffer.
            if let Some(m) = c.take_move() {
                c.placement = m;
            }
            if c.take_resized() {
                resizes += 1;
            }
            let size = c.stage.size();
            if sizes.last() != Some(&size) {
                sizes.push(size);
            }
            started |= c.tease.active();
            if c.tease.active() {
                // Her feet stay put on screen while she grows inside the
                // held (peak-sized) window.
                let feet_x = c.placement.left as f32 + c.stage.feet_anchor().0;
                let c0 = *center0.get_or_insert(feet_x);
                assert!((feet_x - c0).abs() <= 1.5, "{feet_x} vs {c0}");
            }
            // Every buffer she presents is transparent around her.
            let (w, h) = c.stage.size();
            let px = c.stage.canvas().data();
            for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
                let i = ((y * w + x) * 4) as usize;
                assert_eq!(px[i + 3], 0, "corner ({x},{y}) of {w}x{h} is transparent");
            }
            if started && !c.tease.active() {
                break;
            }
        }
        assert!(started, "tease ran");
        // Grown once to the peak window, shrunk once back: not per step.
        assert_eq!(sizes.len(), 3, "{sizes:?}");
        assert!(sizes[1].0 > size0.0 && sizes[1].1 > size0.1);
        assert_eq!(sizes[2], size0);
        assert!(resizes <= 2, "{resizes} window resizes");
        assert!(c.stage.frame().is_none(), "frame released");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn tease_click_backs_off_and_drag_cancels() {
        let (_dir, mut c) = tease_controller("tease_cancel");
        let dt = 1.0 / 30.0;
        let base = c.stage.scale();
        while !c.tease.active() {
            c.tick(dt).unwrap();
        }
        for _ in 0..45 {
            c.tick(dt).unwrap();
        }
        let mid = c.stage.scale();
        assert!(mid > base, "{mid}");
        assert!(
            (c.normal_scale() - base).abs() < 1e-6,
            "menu shows her size"
        );
        // A click: smooth retreat from where she is.
        c.retreat_tease();
        assert_eq!(c.tease.phase(), Some(TeasePhase::Retreat));
        let mut prev = c.stage.scale();
        while c.tease.active() {
            c.tick(dt).unwrap();
            assert!(c.stage.scale() <= prev + 1e-6);
            prev = c.stage.scale();
        }
        assert_eq!(c.stage.scale(), base);
        // Next tease, then a drag: straight back to her size.
        c.tease = Tease::new(
            lillith_core::tease::TeaseConfig {
                every_s: (1.0, 1.0),
                ..Default::default()
            },
            2,
        );
        while !c.tease.active() {
            c.tick(dt).unwrap();
        }
        for _ in 0..45 {
            c.tick(dt).unwrap();
        }
        assert!(c.stage.scale() > base);
        c.press_trusted((40.0, 40.0), true);
        assert!(c.motion((300.0, 300.0)).is_some());
        assert!(!c.tease.active());
        assert_eq!(c.stage.scale(), base);
    }

    #[test]
    fn tease_skipped_without_its_clips() {
        let (_dir, mut c) = m3_controller("tease_none", MoveMode::Stay);
        c.tease = Tease::new(
            lillith_core::tease::TeaseConfig {
                every_s: (1.0, 1.0),
                ..Default::default()
            },
            1,
        );
        let base = c.stage.scale();
        for _ in 0..90 {
            c.tick(1.0 / 30.0).unwrap();
            assert!(!c.tease.active());
        }
        assert_eq!(c.stage.scale(), base);
    }

    #[test]
    fn walk_speed_follows_the_gait_rig_and_scale() {
        let (dir, mut c) = m3_controller("gait", MoveMode::Wander);
        let before = c.brain.config().wander_speed_px_s;
        c.sync_walk_speed();
        // 40 canvas px/s x scale 2 x playback 1.
        assert!(
            (c.brain.config().wander_speed_px_s - 80.0).abs() < 1e-3,
            "{before}"
        );
        c.set_scale(1.0, false);
        assert!((c.brain.config().wander_speed_px_s - 40.0).abs() < 1e-3);
        c.stage.animator_mut().set_playback_speed(0.5);
        c.sync_walk_speed();
        assert!((c.brain.config().wander_speed_px_s - 20.0).abs() < 1e-3);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn m3_controller(tag: &str, mode: MoveMode) -> (std::path::PathBuf, Controller) {
        m3_controller_with(tag, mode, "")
    }

    fn m3_controller_with(
        tag: &str,
        mode: MoveMode,
        extra: &str,
    ) -> (std::path::PathBuf, Controller) {
        let (dir, m) = m3_manifest_with(tag, extra);
        let opts = OverlayOptions {
            manifest: dir.join("manifest.toml"),
            scale: 2.0,
            opacity: 1.0,
            render_hz: 30.0,
            animator: AnimatorConfig {
                idle_enabled: false,
                ..AnimatorConfig::default()
            },
            position: OverlayPos {
                margin_left: 100,
                margin_bottom: 50,
                output: None,
                ..OverlayPos::default()
            },
            config_path: None,
            forced: false,
            exit_after: None,
            behavior: BehaviorConfig {
                pose_cycle_s: 1e6,
                ..BehaviorConfig::default()
            },
            mode,
            muted: false,
            events: None,
            sink: None,
            settings_path: None,
            chat: None,
            chat_on_click: false,
            overlay_speech: false,
            always_on_top: true,
            click_through: false,
            avatars: None,
            avatar: Default::default(),
            seasonal_outfit: "auto".into(),
        };
        let stage = Stage::new(m, PageCache::new(&dir), 2.0, opts.animator.clone()).unwrap();
        let mut c = Controller::with_stage(stage, &opts);
        c.stage.render().unwrap();
        (dir, c)
    }

    #[test]
    fn brain_drives_the_animator() {
        let (dir, mut c) = m3_controller("m3", MoveMode::Stay);
        // Left click on her -> knife_tap reaction.
        assert!(c.press((40.0, 40.0), (40.0, 40.0), true));
        assert_eq!(c.release(), Release::Click);
        assert_eq!(c.stage.animator().overlay_clip(), Some("knife_tap"));
        // Right click on her: cleaning reaction, never quits.
        assert!(c.button(Button::Right, (40.0, 40.0)));
        assert!(!c.quit_requested());
        assert!(!c.button(Button::Middle, (1.0, 1.0)), "transparent pixel");
        // Drag -> drag_held base; drop -> idle.
        assert!(c.press((40.0, 40.0), (40.0, 40.0), true));
        c.motion((70.0, 40.0)).unwrap();
        assert_eq!(c.stage.animator().base(), "drag_held");
        c.release();
        assert_eq!(c.stage.animator().base(), "idle");
        // Sleep, then proximity wakes her.
        c.event(&Event::new("idle.asleep"));
        assert_eq!(c.stage.animator().base(), "sleep");
        c.hover(true);
        assert_eq!(c.stage.animator().base(), "idle");
        assert_eq!(c.stage.animator().overlay_clip(), Some("wake"));
        // Q quits.
        c.key(Key::Char('q'));
        assert!(c.quit_requested());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Runs `secs` of frames at 60 Hz, applying moves like a backend;
    /// returns every `bottom` the window took.
    fn run_frames(c: &mut Controller, secs: f32) -> Vec<i32> {
        let mut bottoms = Vec::new();
        for _ in 0..(secs * 60.0) as usize {
            c.tick(1.0 / 60.0).unwrap();
            if let Some(p) = c.take_move() {
                c.placement = p;
                bottoms.push(p.bottom);
            }
        }
        bottoms
    }

    /// Drags her (surface-local) by `(dx, dy)` and drops her there.
    fn drag_drop(c: &mut Controller, dx: f64, dy: f64) -> Placement {
        assert!(c.press((40.0, 40.0), (40.0, 40.0), true));
        let p = c.motion((40.0 + dx, 40.0 + dy)).unwrap();
        c.placement = p;
        assert_eq!(c.stage.animator().base(), "drag_held");
        assert_eq!(c.release(), Release::Dropped(p));
        p
    }

    #[test]
    fn drop_falls_with_a_soft_bounce_to_the_floor() {
        let (dir, mut c) = m3_controller("grav_floor", MoveMode::Stay);
        c.set_screen((1200, 800));
        c.set_desktop(Some(Desktop::default()));
        let p = drag_drop(&mut c, 0.0, -300.0);
        assert_eq!(p.bottom, 350);
        // Still held while falling: no landing (and no save) yet.
        assert!(matches!(c.gravity.mode, Mode::Falling(_)));
        let mut all = run_frames(&mut c, 0.1);
        assert_eq!(
            c.stage.animator().base(),
            "drag_held",
            "falls in the drag pose"
        );
        all.extend(run_frames(&mut c, 3.0));
        // No bridge: the floor is `[behavior] floor_offset_px` (48).
        assert_eq!(c.placement.bottom, 48);
        assert_eq!(*all.iter().min().unwrap(), 48, "never through the floor");
        let first = all.iter().position(|&b| b == 48).unwrap();
        assert!(
            all[first..].iter().any(|&b| b > 48),
            "soft landing bounce: {all:?}"
        );
        assert!(all[..first].windows(2).all(|w| w[1] <= w[0]), "falls down");
        assert_eq!(c.gravity.mode, Mode::Floor);
        assert_eq!(c.brain.phase(), Phase::Idle, "landing ends the drag");
        assert_eq!(c.stage.animator().base(), "idle");
        assert_eq!(c.placement.left, p.left, "falls straight down");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn wayland_trusted_press_drag_release_still_falls() {
        let (dir, mut c) = m3_controller("grav_trusted", MoveMode::Stay);
        c.set_screen((1200, 800));
        c.set_desktop(Some(Desktop::default()));
        // Wayland path: press_trusted (no hit test), surface-local motion.
        c.press_trusted((40.0, 40.0), true);
        // Under DRAG_THRESHOLD (6 px): still a click candidate.
        assert_eq!(c.motion((44.0, 37.0)), None);
        assert!(matches!(c.gravity.mode, Mode::Settle { .. }));
        let p = c.motion((40.0, -260.0)).unwrap();
        c.placement = p;
        assert_eq!(c.release(), Release::Dropped(p));
        assert!(matches!(c.gravity.mode, Mode::Falling(_)));
        run_frames(&mut c, 3.0);
        assert_eq!(c.placement.bottom, 48);
        assert_eq!(c.gravity.mode, Mode::Floor);
        // A plain click on her never makes her fall.
        c.press_trusted((40.0, 40.0), true);
        assert_eq!(c.release(), Release::Click);
        assert_eq!(c.gravity.mode, Mode::Floor);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn gravity_off_keeps_her_where_dropped() {
        let (dir, mut c) = m3_controller("grav_off", MoveMode::Stay);
        c.gravity.cfg.g_px_s2 = 0.0;
        c.set_screen((1200, 800));
        c.set_desktop(Some(Desktop::default()));
        let p = drag_drop(&mut c, 0.0, -300.0);
        run_frames(&mut c, 3.0);
        assert_eq!(c.placement, p);
        assert_eq!(c.stage.animator().base(), "idle");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn perches_on_a_window_sits_and_falls_when_it_moves_or_closes() {
        use lillith_core::desktop::{DeskWindow, Rect};
        // Seat line 18 cell px above the feet (anchor y 38): 36 px at scale 2.
        let sit = r#"[clips.sit_swing]
page = "idle.png"
fps = 8.0
mode = "loop"
seat_y = 20
seat_x_range = [14, 26]
frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
"#;
        let (dir, mut c) = m3_controller_with("grav_perch", MoveMode::Stay, sit);
        c.set_screen((1200, 800));
        let desk = |rects: &[Rect]| {
            let mut d = Desktop::default();
            d.set_area(Rect::new(0, 0, 1200, 800), Rect::new(0, 0, 1200, 760));
            d.set_windows(
                rects
                    .iter()
                    .enumerate()
                    .map(|(i, r)| DeskWindow {
                        id: format!("w{i}"),
                        class: "kate".into(),
                        rect: *r,
                    })
                    .collect(),
            );
            d
        };
        let win = Rect::new(50, 300, 600, 400);
        c.set_desktop(Some(desk(&[win])));
        // Feet at x 100 + 40 (inside 50..650), y 800 - 700 - 4 = 96 (above 300).
        drag_drop(&mut c, 0.0, -650.0);
        run_frames(&mut c, 2.0);
        // Feet on the window top: bottom = 800 - 300 - 4.
        assert_eq!(c.placement.bottom, 496);
        assert!(matches!(&c.gravity.mode, Mode::Perched(p) if p.id == "w0"));

        // She sits on the edge within `sit_every_s`, seat on the edge.
        run_frames(&mut c, 21.0);
        assert!(c.gravity.sitting(), "{:?}", c.gravity.mode);
        assert_eq!(c.stage.animator().base(), "sit_swing");
        assert_eq!(c.placement.bottom, 496 - 36);

        // The window moves down: she stands, falls and lands on it again.
        c.set_desktop(Some(desk(&[Rect::new(50, 350, 600, 400)])));
        c.tick(1.0 / 60.0).unwrap();
        assert!(matches!(c.gravity.mode, Mode::Falling(_)));
        assert_eq!(c.stage.animator().base(), "drag_held");
        run_frames(&mut c, 2.0);
        assert_eq!(c.placement.bottom, 800 - 350 - 4);
        assert!(matches!(c.gravity.mode, Mode::Perched(_)));

        // The window closes (or minimizes): down to the work-area floor (760).
        c.set_desktop(Some(desk(&[])));
        run_frames(&mut c, 2.0);
        assert_eq!(c.placement.bottom, 800 - 760 - 4);
        assert_eq!(c.gravity.mode, Mode::Floor);
        assert_eq!(c.stage.animator().base(), "idle");

        // Dropped beside the window (feet outside its x-span): the floor.
        c.set_desktop(Some(desk(&[win])));
        drag_drop(&mut c, 700.0, -600.0);
        run_frames(&mut c, 2.0);
        assert_eq!(c.placement.bottom, 36);
        assert_eq!(c.gravity.mode, Mode::Floor);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sit_clips_take_tagged_bases_and_fall_back_to_sit_swing() {
        let one = r#"page = "p.png"
frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
"#;
        let m = |extra: &str| {
            lillith_anim::atlas::Manifest::from_toml_str(&format!("[clips.idle]\n{one}\n{extra}"))
                .unwrap()
        };
        let swing = format!("[clips.sit_swing]\nseat_y = 20\n{one}\n");
        let perch = format!("[clips.perch_sit]\nseat_y = 18\ntags = [\"sit_base\"]\n{one}\n");
        // A variant take with seat data is drawn from its base, not picked.
        let variant = format!(
            "[clips.sit_bored_chin]\nseat_y = 20\ntags = [\"variant_of:sit_swing\"]\n{one}\n"
        );
        let untagged = format!("[clips.sit_lean]\nseat_y = 20\n{one}\n");
        let no_seat = format!("[clips.perch_peek]\ntags = [\"sit_base\"]\n{one}\n");
        // Default: sit_swing only.
        assert_eq!(
            sit_clips(&m(&format!("{swing}{variant}{untagged}"))),
            [SIT_CLIP]
        );
        // Tagged seated bases join it.
        assert_eq!(
            sit_clips(&m(&format!("{swing}{perch}{no_seat}"))),
            ["perch_sit", "sit_swing"]
        );
        // Without sit_swing the tagged base alone; without either nothing.
        assert_eq!(sit_clips(&m(&perch)), ["perch_sit"]);
        assert!(sit_clips(&m(&no_seat)).is_empty());
        assert!(sit_clips(&m("")).is_empty());
    }

    #[test]
    fn seat_math_uses_each_clips_own_seat() {
        let m = lillith_anim::atlas::Manifest::from_toml_str(
            r#"[clips.sit_swing]
page = "p.png"
seat_y = 230
seat_x_range = [136, 279]
frames = [ { x = 0, y = 0, w = 400, h = 400, anchor = [200, 392] } ]
[clips.perch_sit]
page = "p.png"
seat_y = 250
seat_x_range = [150, 222]
frames = [ { x = 0, y = 0, w = 400, h = 400, anchor = [200, 392] } ]
[clips.odd]
page = "p.png"
seat_y = 395
frames = [ { x = 0, y = 0, w = 400, h = 400, anchor = [200, 392] } ]
"#,
        )
        .unwrap();
        let c = |n: &str| &m.clips[n];
        assert_eq!(seat_cell(c("sit_swing")), Some(162));
        assert_eq!(seat_cell(c("perch_sit")), Some(142));
        let px = |n: &str, s: f32| gravity::scaled_px(seat_cell(c(n)).unwrap(), s, 1.0);
        assert_eq!(px("sit_swing", 0.55), 89);
        assert_eq!(px("sit_swing", 1.0), 162);
        assert_eq!(px("perch_sit", 0.55), 78);
        assert_eq!(px("perch_sit", 1.0), 142);
        // Seat at/below the feet, or no seat: no drop.
        assert_eq!(seat_cell(c("odd")), None);
        assert_eq!(half_seat_px(c("sit_swing"), 1.0), 72);
        assert_eq!(half_seat_px(c("perch_sit"), 0.5), 18);
        assert_eq!(half_seat_px(c("odd"), 1.0), 0);
    }

    #[test]
    fn perches_on_a_tagged_sit_clip_through_its_blends() {
        use lillith_core::desktop::{DeskWindow, Rect};
        // No sit_swing: the tagged base is the only sit. Seat 18 cell px
        // above the feet: 36 px at scale 2. Blends: 4 frames at 8 fps.
        let clip = |name: &str, extra: &str| {
            format!(
                "[clips.{name}]\npage = \"idle.png\"\nfps = 8.0\n{extra}frames = [ {f}, {f}, {f}, {f} ]\n",
                f = "{ x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] }"
            )
        };
        let mut sit = clip(
            "perch_sit",
            "mode = \"loop\"\nseat_y = 20\nseat_x_range = [14, 26]\ntags = [\"sit_base\"]\n",
        );
        for (from, to) in [("idle", "perch_sit"), ("perch_sit", "idle")] {
            let n = format!("blend_{from}_to_{to}");
            sit.push_str(&clip(&n, "mode = \"once\"\n"));
            sit.push_str(&format!(
                "[clips.{n}.blend]\nfrom = \"{from}\"\nto = \"{to}\"\nfrom_frame = 0\nto_frame = 0\n"
            ));
        }
        let (dir, mut c) = m3_controller_with("grav_perch_tag", MoveMode::Stay, &sit);
        c.gravity.cfg.sit_for_s = (2.0, 2.0);
        c.set_screen((1200, 800));
        let mut d = Desktop::default();
        d.set_area(Rect::new(0, 0, 1200, 800), Rect::new(0, 0, 1200, 760));
        d.set_windows(vec![DeskWindow {
            id: "w0".into(),
            class: "kate".into(),
            rect: Rect::new(50, 300, 600, 400),
        }]);
        c.set_desktop(Some(d));
        drag_drop(&mut c, 0.0, -650.0);
        run_frames(&mut c, 2.0);
        assert_eq!(c.placement.bottom, 496);
        // Wait for the sit to start.
        let mut waited = 0.0;
        while !c.gravity.sitting() {
            run_frames(&mut c, 1.0 / 60.0);
            waited += 1.0 / 60.0;
            assert!(waited < 25.0, "never sat");
        }
        assert_eq!(c.stage.animator().base(), "perch_sit");
        assert_eq!(
            c.stage.animator().overlay_clip(),
            Some("blend_idle_to_perch_sit")
        );
        // The window sinks with the 0.5 s blend instead of jumping.
        let down = run_frames(&mut c, 0.6);
        assert!(down.len() > 10, "{down:?}");
        assert!(down.windows(2).all(|w| w[1] <= w[0] && w[0] - w[1] < 36));
        assert_eq!(c.placement.bottom, 496 - 36);
        assert!(c.gravity.sitting(), "the own blend is no interruption");
        assert_eq!(c.stage.animator().overlay_clip(), None);
        // The sit runs out: up through the stand blend, rising with it.
        while c.gravity.sitting() {
            run_frames(&mut c, 1.0 / 60.0);
        }
        assert_eq!(c.stage.animator().base(), "idle");
        assert_eq!(
            c.stage.animator().overlay_clip(),
            Some("blend_perch_sit_to_idle")
        );
        let up = run_frames(&mut c, 0.6);
        assert!(up.len() > 10, "{up:?}");
        assert!(up.windows(2).all(|w| w[1] >= w[0] && w[1] - w[0] < 36));
        assert_eq!(c.placement.bottom, 496);
        assert!(matches!(&c.gravity.mode, Mode::Perched(p) if p.rise.is_none()));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `[clips.<name>]` of `n` 40×40 frames (anchor [20, 38]) at 8 fps.
    fn clip_n(name: &str, n: usize, extra: &str) -> String {
        let f = "{ x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] }";
        let frames = vec![f; n].join(", ");
        format!("[clips.{name}]\npage = \"idle.png\"\nfps = 8.0\n{extra}frames = [ {frames} ]\n")
    }

    /// `[clips.<name>.blend]` from `from` to `to`.
    fn blend_of(name: &str, from: &str, to: &str) -> String {
        format!("[clips.{name}.blend]\nfrom = \"{from}\"\nto = \"{to}\"\nfrom_frame = 0\nto_frame = 0\n")
    }

    /// One window `w0` at `rect` above a 1200×760 work area.
    fn one_window(rect: lillith_core::desktop::Rect) -> Desktop {
        use lillith_core::desktop::{DeskWindow, Rect};
        let mut d = Desktop::default();
        d.set_area(Rect::new(0, 0, 1200, 800), Rect::new(0, 0, 1200, 760));
        d.set_windows(vec![DeskWindow {
            id: "w0".into(),
            class: "kate".into(),
            rect,
        }]);
        d
    }

    /// One 60 Hz frame, applying the move like a backend.
    fn step(c: &mut Controller) {
        c.tick(1.0 / 60.0).unwrap();
        if let Some(p) = c.take_move() {
            c.placement = p;
        }
    }

    #[test]
    fn landing_blend_is_found_by_its_blend_fields() {
        let m = |toml: &str| {
            lillith_anim::atlas::Manifest::from_toml_str(&format!(
                "{}{}{toml}",
                clip_n("idle", 1, ""),
                clip_n("drag_held", 1, "")
            ))
            .unwrap()
        };
        let perch = clip_n(
            "perch_sit",
            1,
            "seat_y = 20\nseat_x_range = [14, 26]\ntags = [\"sit_base\"]\n",
        );
        let land = |name: &str, extra: &str, to: &str| {
            format!(
                "{}{}",
                clip_n(name, 2, &format!("{extra}tags = [\"start_matches\"]\n")),
                blend_of(name, "drag_held", to)
            )
        };
        // Its own seat line (anchor 38 - seat_y 16), and the base's.
        let with_seat = land("land_a", "seat_y = 16\n", "perch_sit");
        let got = landing_blend(&m(&format!("{perch}{with_seat}")), "drag_held").unwrap();
        assert_eq!(
            got,
            LandingBlend {
                blend: "land_a".into(),
                to: "perch_sit".into(),
                seat_cell: 22,
                to_seat_cell: 18,
            }
        );
        // Matched on `from` (the pose she is dropped in), not the name.
        assert_eq!(
            landing_blend(&m(&format!("{perch}{with_seat}")), "idle"),
            None
        );
        // No seat_y on the blend: the base's seat line.
        let no_seat = land("land_b", "", "perch_sit");
        let got = landing_blend(&m(&format!("{perch}{no_seat}")), "drag_held").unwrap();
        assert_eq!((got.seat_cell, got.to_seat_cell), (18, 18));
        // A blend into a clip that is not a sit base, an idle -> sit blend,
        // or a sit base without seat data: none.
        let wave = format!("{}{}", clip_n("wave", 1, ""), land("land_c", "", "wave"));
        assert_eq!(
            landing_blend(&m(&format!("{perch}{wave}")), "drag_held"),
            None
        );
        let idle_in = format!(
            "{}{}",
            clip_n("blend_idle_to_perch_sit", 2, ""),
            blend_of("blend_idle_to_perch_sit", "idle", "perch_sit")
        );
        assert_eq!(
            landing_blend(&m(&format!("{perch}{idle_in}")), "drag_held"),
            None
        );
        let seatless = clip_n("perch_sit", 1, "tags = [\"sit_base\"]\n");
        assert_eq!(
            landing_blend(&m(&format!("{seatless}{with_seat}")), "drag_held"),
            None
        );
        // A blend whose first frame does not match the drag pose (no
        // `start_matches` tag from the build tool): skipped, she lands
        // standing rather than popping into it.
        let unmatched = format!(
            "{}{}",
            clip_n("land_e", 2, "seat_y = 16\n"),
            blend_of("land_e", "drag_held", "perch_sit")
        );
        assert_eq!(
            landing_blend(&m(&format!("{perch}{unmatched}")), "drag_held"),
            None
        );
        // ...and a matching one next to it is still found.
        assert_eq!(
            landing_blend(&m(&format!("{perch}{unmatched}{with_seat}")), "drag_held")
                .map(|l| l.blend),
            Some("land_a".into())
        );
        // sit_swing counts as a sit base too.
        let swing = clip_n("sit_swing", 1, "seat_y = 20\n");
        let to_swing = land("land_d", "", "sit_swing");
        assert_eq!(
            landing_blend(&m(&format!("{swing}{to_swing}")), "drag_held").map(|l| l.to),
            Some("sit_swing".into())
        );
    }

    #[test]
    fn lands_on_a_window_through_the_landing_blend_then_sits() {
        use lillith_core::desktop::Rect;
        // perch_sit: seat 18 cell px above the feet (36 px at scale 2), 4
        // frames. The landing blend: 6 frames (0.75 s), seat 22 cell px up.
        let mut toml = clip_n(
            "perch_sit",
            4,
            "mode = \"loop\"\nseat_y = 20\nseat_x_range = [14, 26]\ntags = [\"sit_base\"]\n",
        );
        let blend = "blend_drag_held_to_perch_sit";
        toml.push_str(&clip_n(
            blend,
            6,
            "mode = \"once\"\nseat_y = 16\ntags = [\"start_matches\"]\n",
        ));
        toml.push_str(&blend_of(blend, "drag_held", "perch_sit"));
        let (dir, mut c) = m3_controller_with("grav_land_blend", MoveMode::Stay, &toml);
        c.gravity.cfg.sit_for_s = (2.0, 2.0);
        c.set_screen((1200, 800));
        c.set_desktop(Some(one_window(Rect::new(50, 300, 600, 400))));
        drag_drop(&mut c, 0.0, -650.0);
        let mut t = 0.0;
        loop {
            step(&mut c);
            if matches!(c.gravity.mode, Mode::Perched(_)) {
                break;
            }
            t += 1.0 / 60.0;
            assert!(t < 3.0, "never landed: {:?}", c.gravity.mode);
            assert_eq!(
                c.stage.animator().base(),
                "drag_held",
                "falls in the drag pose"
            );
        }
        // Landed on the window: the blend plays over perch_sit; the drop
        // ended the drag without resetting her to idle.
        assert_eq!(c.brain.phase(), Phase::Idle, "landing ends the drag");
        assert_eq!(c.stage.animator().base(), "perch_sit");
        assert_eq!(c.stage.animator().overlay_clip(), Some(blend));
        assert!(c.gravity.sitting());
        // The blend's seat line on the window top (standing bottom 496)
        // from its first frame, for as long as it plays.
        let mut during = Vec::new();
        step(&mut c);
        while c.stage.animator().overlay_clip() == Some(blend) {
            during.push(c.placement.bottom);
            assert_eq!(c.stage.animator().base(), "perch_sit", "no reset mid-blend");
            step(&mut c);
        }
        assert!(during.len() > 30, "{during:?}");
        assert!(during.iter().all(|&b| b == 496 - 44), "{during:?}");
        // Handed over to perch_sit at frame 0 (a free-running base would be
        // on frame 2 after 0.75 s), still seated, now on its own seat line.
        assert_eq!(c.stage.animator().base(), "perch_sit");
        assert_eq!(c.stage.animator().base_frame(), 0);
        assert_eq!(c.stage.animator().overlay_clip(), None);
        assert!(c.gravity.sitting());
        step(&mut c);
        assert_eq!(c.placement.bottom, 496 - 36);
        // A usual sit after it, then up on her feet on the window top.
        let mut t = 0.0;
        while c.gravity.sitting() {
            step(&mut c);
            t += 1.0 / 60.0;
            assert!(t < 3.0, "never stood up");
        }
        assert!(t > 1.5, "sat {t} s after the landing");
        step(&mut c);
        assert_eq!(c.stage.animator().base(), "idle");
        assert_eq!(c.placement.bottom, 496);

        // Grabbed again mid-landing: the blend stops drawing over the drag pose.
        drag_drop(&mut c, 0.0, -200.0);
        while !matches!(c.gravity.mode, Mode::Perched(_)) {
            step(&mut c);
        }
        step(&mut c);
        assert_eq!(c.stage.animator().overlay_clip(), Some(blend));
        assert!(c.press((40.0, 40.0), (40.0, 40.0), true));
        c.motion((40.0, 0.0)).unwrap();
        assert_eq!(c.gravity.mode, Mode::Held);
        assert_eq!(c.stage.animator().base(), "drag_held");
        assert_eq!(c.stage.animator().overlay_clip(), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lands_standing_without_a_landing_blend() {
        use lillith_core::desktop::Rect;
        // Only blends that do not lead from the drag pose into a sit base,
        // and a drag -> perch_sit landing blend whose first frame does not
        // match the drag pose (no `start_matches` tag): she lands standing
        // instead of popping into it.
        let mut toml = clip_n(
            "perch_sit",
            4,
            "mode = \"loop\"\nseat_y = 20\ntags = [\"sit_base\"]\n",
        );
        for (n, from, to) in [
            ("blend_idle_to_perch_sit", "idle", "perch_sit"),
            ("blend_drag_held_to_wave", "drag_held", "wave"),
            ("blend_drag_held_to_perch_sit", "drag_held", "perch_sit"),
        ] {
            toml.push_str(&clip_n(n, 6, "mode = \"once\"\n"));
            toml.push_str(&blend_of(n, from, to));
        }
        let (dir, mut c) = m3_controller_with("grav_land_plain", MoveMode::Stay, &toml);
        c.set_screen((1200, 800));
        c.set_desktop(Some(one_window(Rect::new(50, 300, 600, 400))));
        drag_drop(&mut c, 0.0, -650.0);
        while !matches!(c.gravity.mode, Mode::Perched(_)) {
            step(&mut c);
        }
        step(&mut c);
        assert_eq!(c.brain.phase(), Phase::Idle);
        assert_eq!(c.stage.animator().base(), "idle");
        assert_eq!(c.stage.animator().overlay_clip(), None);
        assert!(!c.gravity.sitting());
        assert_eq!(c.placement.bottom, 496);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lounge_clips_need_the_tag_and_seat_data() {
        let m = |toml: &str| {
            lillith_anim::atlas::Manifest::from_toml_str(&format!(
                "{}{}{toml}",
                clip_n("idle", 1, ""),
                clip_n("drag_held", 1, "")
            ))
            .unwrap()
        };
        let lounge = clip_n(
            "perch_lounge",
            1,
            "seat_y = 36\nseat_x_range = [2, 38]\ntags = [\"lounge_base\"]\n",
        );
        let no_range = clip_n("lounge_b", 1, "seat_y = 36\ntags = [\"lounge_base\"]\n");
        let untagged = clip_n("lounge_c", 1, "seat_y = 36\nseat_x_range = [2, 38]\n");
        let flat = clip_n(
            "lounge_d",
            1,
            "seat_y = 38\nseat_x_range = [2, 38]\ntags = [\"lounge_base\"]\n",
        );
        let mm = m(&format!("{lounge}{no_range}{untagged}{flat}"));
        assert_eq!(lounge_clips(&mm), ["lounge_d", "perch_lounge"]);
        // Not a sit: the sit picker and the perch margin leave lounges out.
        assert!(sit_clips(&mm).is_empty());
        // Rest line 2 cell px above the feet; lying on the feet line: 0.
        assert_eq!(lounge_cell(&mm.clips["perch_lounge"]), Some(2));
        assert_eq!(lounge_cell(&mm.clips["lounge_d"]), Some(0));
        assert_eq!(seat_cell(&mm.clips["lounge_d"]), None);
        // Fit at scale 2 (span -36..36 around the feet) on a 50..650 top.
        let c = &mm.clips["perch_lounge"];
        let win = Rect::new(50, 300, 600, 400);
        assert_eq!(lounge_fit(c, 2.0, 300, win), Some(0));
        assert_eq!(lounge_fit(c, 2.0, 620, win), Some(642 - 656));
        assert_eq!(lounge_fit(c, 2.0, 60, win), Some(58 - 24));
        assert_eq!(lounge_fit(c, 2.0, 540, Rect::new(500, 300, 84, 400)), None);
        assert_eq!(lounge_fit(&mm.clips["lounge_b"], 2.0, 300, win), None);
    }

    #[test]
    fn lounges_along_wide_window_tops_and_only_sits_on_narrow_ones() {
        use lillith_core::desktop::Rect;
        // sit_swing: seat 18 cell px up. perch_lounge: rests 2 cell px up
        // (4 px at scale 2) on x 2..38 of the cell (anchor x 20): 72 px wide.
        let sit = "mode = \"loop\"\nseat_y = 20\nseat_x_range = [14, 26]\n";
        let lounge = "mode = \"loop\"\nseat_y = 36\nseat_x_range = [2, 38]\n\
                      tags = [\"lounge_base\"]\n";
        let toml = format!(
            "{}{}",
            clip_n("sit_swing", 1, sit),
            clip_n("perch_lounge", 1, lounge)
        );
        let controller = |tag: &str, rect: Rect, left: i32| {
            let (dir, mut c) = m3_controller_with(tag, MoveMode::Stay, &toml);
            c.gravity.cfg.sit_every_s = (0.2, 0.2);
            c.gravity.cfg.sit_for_s = (0.5, 0.5);
            c.set_screen((1200, 800));
            c.set_desktop(Some(one_window(rect)));
            c.placement = Placement { left, bottom: 700 };
            c.gravity.mode = Mode::Settle { wait: 0.0 };
            (dir, c)
        };
        // Wide top 50..650, feet at x 620: a lounge there would stick out
        // past 642 (8 px inside the right edge), so it moves her 14 px left.
        let (dir, mut c) = controller("grav_lounge_wide", Rect::new(50, 300, 600, 400), 580);
        let (mut sat, mut lounged) = (false, false);
        let mut t = 0.0;
        while !(sat && lounged) {
            step(&mut c);
            t += 1.0 / 60.0;
            assert!(t < 60.0, "sat {sat}, lounged {lounged}");
            if !c.gravity.sitting() {
                continue;
            }
            match c.stage.animator().base() {
                "sit_swing" => sat = true,
                "perch_lounge" if !lounged => {
                    lounged = true;
                    step(&mut c);
                    assert_eq!(c.placement.left, 580 - 14, "shifted onto the top");
                    assert_eq!(c.placement.bottom, 496 - 4, "rests on its own line");
                }
                _ => {}
            }
        }
        let _ = std::fs::remove_dir_all(dir);

        // Narrow top 500..584 (perchable, but 72 + 2 × 8 px do not fit):
        // sits only.
        let (dir, mut c) = controller("grav_lounge_narrow", Rect::new(500, 300, 84, 400), 500);
        let mut sits = 0;
        let mut was = false;
        for _ in 0..(20 * 60) {
            step(&mut c);
            let now = c.gravity.sitting();
            if now && !was {
                sits += 1;
            }
            was = now;
            assert_ne!(c.stage.animator().base(), "perch_lounge");
        }
        assert!(matches!(&c.gravity.mode, Mode::Perched(_)));
        assert!(sits >= 5, "{sits}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Global y of her drawn feet anchor minus `y` (window px are floats:
    /// the feet anchor is `anchor y × scale` into the canvas).
    fn feet_gap(c: &Controller, screen_h: i32, y: i32) -> f32 {
        let (_, h) = c.stage.size();
        let top = screen_h - c.placement.bottom - h as i32;
        top as f32 + c.stage.feet_anchor().1 - y as f32
    }

    #[test]
    fn lands_and_sits_flush_at_any_scale() {
        use lillith_core::desktop::{DeskWindow, Rect};
        // Seat line 18 cell px above the feet anchor (38).
        let sit = r#"[clips.sit_swing]
page = "idle.png"
fps = 8.0
mode = "loop"
seat_y = 20
seat_x_range = [14, 26]
frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
"#;
        let win = Rect::new(50, 300, 600, 400);
        let desk = |rects: &[Rect]| {
            let mut d = Desktop::default();
            d.set_area(Rect::new(0, 0, 1200, 800), Rect::new(0, 0, 1200, 760));
            d.set_windows(
                rects
                    .iter()
                    .enumerate()
                    .map(|(i, r)| DeskWindow {
                        id: format!("w{i}"),
                        class: "kate".into(),
                        rect: *r,
                    })
                    .collect(),
            );
            d
        };
        for (scale, other) in [(0.55, 1.0), (1.0, 0.55)] {
            let (dir, mut c) =
                m3_controller_with(&format!("grav_flush_{scale}"), MoveMode::Stay, sit);
            c.set_screen((1200, 800));
            c.set_scale(scale, false);
            c.set_desktop(Some(desk(&[win])));
            // High above the window top: she falls onto it.
            c.placement = Placement {
                left: 100,
                bottom: 700,
            };
            c.gravity.mode = Mode::Settle { wait: 0.0 };
            run_frames(&mut c, 3.0);
            // Standing: feet on the window top.
            assert!(matches!(&c.gravity.mode, Mode::Perched(_)), "{scale}");
            assert!(
                feet_gap(&c, 800, 300).abs() <= 0.5,
                "{scale}: {}",
                feet_gap(&c, 800, 300)
            );
            // Seated: the seat line (18 cell px above the feet) on the top.
            let mut waited = 0.0;
            while !c.gravity.sitting() {
                run_frames(&mut c, 0.1);
                waited += 0.1;
                assert!(waited < 25.0, "never sat");
            }
            run_frames(&mut c, 0.1);
            let seat = feet_gap(&c, 800, 300) - 18.0 * scale;
            assert!(seat.abs() <= 1.0, "{scale}: seat off by {seat}");
            // Scaled while perched (Resize): she re-lands flush at the new size.
            c.set_scale(other, false);
            run_frames(&mut c, 2.0);
            let g = feet_gap(&c, 800, 300);
            let seat = if c.gravity.sitting() {
                18.0 * other
            } else {
                0.0
            };
            assert!(
                (g - seat).abs() <= 1.0,
                "{scale}->{other}: off by {}",
                g - seat
            );
            // The window closes: down to the work-area floor (760), flush.
            c.set_desktop(Some(desk(&[])));
            run_frames(&mut c, 3.0);
            assert_eq!(c.gravity.mode, Mode::Floor);
            assert!(
                feet_gap(&c, 800, 760).abs() <= 0.5,
                "{other}: floor {}",
                feet_gap(&c, 800, 760)
            );
            // And after another resize on the floor (within the floor's
            // usual SNAP_PX dead band).
            c.set_scale(scale, false);
            run_frames(&mut c, 2.0);
            assert!(
                feet_gap(&c, 800, 760).abs() <= gravity::SNAP_PX as f32 + 0.5,
                "{scale}: floor {}",
                feet_gap(&c, 800, 760)
            );
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn perch_follows_the_tease_scale_while_seated() {
        use lillith_core::desktop::{DeskWindow, Rect};
        let sit = r#"[clips.sit_swing]
page = "idle.png"
fps = 8.0
mode = "loop"
seat_y = 20
frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
"#;
        let (dir, mut c) = m3_controller_with("grav_tease_seat", MoveMode::Stay, sit);
        c.set_screen((1200, 800));
        c.set_scale(0.55, false);
        let mut d = Desktop::default();
        d.set_area(Rect::new(0, 0, 1200, 800), Rect::new(0, 0, 1200, 760));
        d.set_windows(vec![DeskWindow {
            id: "w0".into(),
            class: "kate".into(),
            rect: Rect::new(50, 300, 600, 400),
        }]);
        c.set_desktop(Some(d));
        c.placement = Placement {
            left: 100,
            bottom: 700,
        };
        c.gravity.mode = Mode::Settle { wait: 0.0 };
        run_frames(&mut c, 3.0);
        while !c.gravity.sitting() {
            run_frames(&mut c, 0.1);
        }
        // The drawing scale changes under her without a resettle (the tease
        // eases her size): the seat stays on the edge at every step.
        for s in [0.7, 0.85, 1.0] {
            c.apply_tease_scale_around(s, 200);
            c.tick(1.0 / 60.0).unwrap();
            if let Some(p) = c.take_move() {
                c.placement = p;
            }
            assert!(c.gravity.sitting());
            let seat = feet_gap(&c, 800, 300) - 18.0 * s;
            assert!(seat.abs() <= 1.0, "{s}: seat off by {seat}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn startup_without_the_bridge_settles_on_the_floor() {
        let (dir, mut c) = m3_controller("grav_start", MoveMode::Stay);
        c.placement.bottom = 400;
        c.set_screen((1200, 800));
        c.set_desktop(Some(Desktop::default()));
        // Waits for the bridge first.
        run_frames(&mut c, 1.0);
        assert_eq!(c.placement.bottom, 400);
        run_frames(&mut c, 4.0);
        assert_eq!(c.placement.bottom, 48);
        // Below the floor line: snapped up onto it.
        c.placement.bottom = 0;
        run_frames(&mut c, 0.5);
        assert_eq!(c.placement.bottom, 48);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn monitor_feed_reaches_the_brain() {
        let (dir, mut c) = m3_controller("m4feed", MoveMode::Stay);
        let (tx, rx) = std::sync::mpsc::channel();
        c.feed = Some(Arc::new(Mutex::new(rx)));
        tx.send(Event::new("idle.asleep")).unwrap();
        c.tick(0.05).unwrap();
        assert_eq!(c.stage.animator().base(), "sleep");
        // Desktop input after a long break (presence monitor) wakes her.
        tx.send(Event::with_detail("user.returned", "away 6m"))
            .unwrap();
        c.tick(0.05).unwrap();
        assert_eq!(c.stage.animator().base(), "idle");
        // A burst is spread over frames.
        for _ in 0..(MAX_EVENTS_PER_TICK + 4) {
            tx.send(Event::new("user.activity")).unwrap();
        }
        c.tick(0.05).unwrap();
        assert_eq!(c.feed_backlog.len(), 4, "rest waits for the next frame");
        drop(tx);
        c.tick(0.05).unwrap(); // disconnected feed is harmless
        let _ = std::fs::remove_dir_all(dir);
    }

    #[derive(Default)]
    struct Rec(Arc<Mutex<Vec<Action>>>);
    impl ActionSink for Rec {
        fn on_actions(&mut self, actions: &[Action]) {
            self.0.lock().unwrap().extend_from_slice(actions);
        }
    }

    #[test]
    fn every_applied_batch_reaches_the_sink() {
        let (dir, mut c) = m3_controller("sink", MoveMode::Stay);
        let seen = Arc::new(Mutex::new(Vec::new()));
        c.sink = Some(SinkHandle::new(Rec(seen.clone())));
        c.apply(vec![Action::Say("hi".into()), Action::Muted(true)]);
        // Input batches go through the same path (M key = mute toggle).
        c.key(Key::Char('m'));
        let got = seen.lock().unwrap().clone();
        assert_eq!(&got[..2], &[Action::Say("hi".into()), Action::Muted(true)]);
        assert!(got.len() >= 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A voice-like sink: records replies, speaker switch and reloads.
    #[derive(Default, Clone)]
    struct Spk {
        replies: Arc<Mutex<Vec<String>>>,
        on: Arc<Mutex<bool>>,
        reloads: Arc<Mutex<u32>>,
    }
    impl ActionSink for Spk {
        fn on_actions(&mut self, _: &[Action]) {}
        fn on_chat_reply(&mut self, reply: &str) {
            if *self.on.lock().unwrap() {
                self.replies.lock().unwrap().push(reply.to_string());
            }
        }
        fn speaker(&self) -> Option<bool> {
            Some(*self.on.lock().unwrap())
        }
        fn set_speaker(&mut self, on: bool) {
            *self.on.lock().unwrap() = on;
        }
        fn config_changed(&mut self, _: &std::path::Path) {
            *self.reloads.lock().unwrap() += 1;
        }
    }

    /// A voice sink with a lip-sync queue and recorded interrupts.
    #[derive(Default, Clone)]
    struct LipSink {
        lines: Arc<Mutex<Vec<String>>>,
        gone: Arc<Mutex<bool>>,
        interrupts: Arc<Mutex<Vec<String>>>,
        spoken: Arc<Mutex<Vec<String>>>,
    }
    impl ActionSink for LipSink {
        fn on_actions(&mut self, _: &[Action]) {}
        fn on_chat_reply(&mut self, reply: &str) {
            self.spoken.lock().unwrap().push(reply.to_string());
        }
        fn lipsync_lines(&mut self, out: &mut Vec<String>) -> bool {
            out.append(&mut self.lines.lock().unwrap());
            std::mem::take(&mut *self.gone.lock().unwrap())
        }
        fn interrupt(&mut self, reason: &str) {
            self.interrupts.lock().unwrap().push(reason.to_string());
        }
    }

    #[test]
    fn daemon_lipsync_feeds_the_mouth_and_barge_in_interrupts() {
        let (dir, mut c) = m3_controller("lipsync", MoveMode::Stay);
        let sink = LipSink::default();
        c.sink = Some(SinkHandle::new(sink.clone()));
        let pts: Vec<String> = (0..=60).map(|i| format!("[{},0.6]", i * 33)).collect();
        sink.lines.lock().unwrap().extend([
            format!(
                r#"{{"ev":"envelope","utt":3,"chunk":0,"fps":30,"pts":[{}]}}"#,
                pts.join(",")
            ),
            r#"{"ev":"chunk_start","utt":3,"chunk":0,"mono_ms":42}"#.to_string(),
        ]);
        c.tick(0.016).unwrap();
        assert!(c.talk.active(), "lip-sync lines drive her mouth");
        assert!(sink.lines.lock().unwrap().is_empty(), "drained every frame");
        // Barge-in: the voice is cut and the audio mouth dropped.
        c.event(&Event::new(CHAT_BARGE_IN));
        assert_eq!(*sink.interrupts.lock().unwrap(), ["barge_in"]);
        c.tick(0.016).unwrap();
        assert!(!c.talk.active());
        // Daemon gone: reset.
        sink.lines
            .lock()
            .unwrap()
            .push(r#"{"ev":"chunk_start","utt":4,"chunk":0,"mono_ms":1}"#.to_string());
        c.tick(0.016).unwrap();
        assert!(c.talk.active());
        *sink.gone.lock().unwrap() = true;
        c.tick(0.016).unwrap();
        assert!(!c.talk.active());
        // Safeword from the bubble: voice cut, nothing killed.
        let before = lillith_core::access::kill_switch::epoch();
        c.event(&Event::new(CHAT_SAFEWORD));
        assert!(
            !lillith_core::access::kill_switch::stopped_since(before),
            "the safeword no longer pulls the kill switch"
        );
        assert!(
            lillith_core::access::kill_switch::hush_epoch() > 0,
            "it hushes"
        );
        assert_eq!(*sink.interrupts.lock().unwrap(), ["barge_in", "safeword"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn safeword_kills_the_queued_reply_it_interrupts() {
        let (dir, mut c) = m3_controller("safeword-reply", MoveMode::Stay);
        let sink = LipSink::default();
        c.sink = Some(SinkHandle::new(sink.clone()));
        let (tx, rx) = std::sync::mpsc::channel();
        c.feed = Some(Arc::new(Mutex::new(rx)));
        let send = |name: &str, d: Option<&str>| {
            tx.send(match d {
                Some(d) => Event::with_detail(name, d),
                None => Event::new(name),
            })
            .unwrap();
        };
        send(crate::chat::CHAT_START, None);
        for _ in 0..(MAX_EVENTS_PER_TICK * 2) {
            send(crate::chat::CHAT_TOKEN, Some("la "));
        }
        let full = format!("{}SECRET", "la ".repeat(MAX_EVENTS_PER_TICK * 2));
        send(crate::chat::CHAT_END, Some(&full));
        send(CHAT_SAFEWORD, None);
        for _ in 0..10 {
            c.tick(0.016).unwrap();
        }
        assert!(sink.spoken.lock().unwrap().is_empty(), "nothing spoken");
        let shown = c
            .talk
            .speech
            .as_ref()
            .map(|s| s.text.clone())
            .unwrap_or_default();
        assert!(!shown.contains("SECRET"), "reply not re-shown: {shown:?}");
        // Belt and braces: a stale chat.end arriving after the safeword (no
        // new chat.start) is ignored too.
        send(crate::chat::CHAT_END, Some("late SECRET"));
        c.tick(0.016).unwrap();
        assert!(sink.spoken.lock().unwrap().is_empty());
        // The next reply works normally.
        send(crate::chat::CHAT_START, None);
        send(crate::chat::CHAT_TOKEN, Some("hi"));
        send(crate::chat::CHAT_END, Some("hi"));
        c.tick(0.016).unwrap();
        assert_eq!(*sink.spoken.lock().unwrap(), ["hi"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn safeword_beats_a_paused_monitor_and_a_token_burst() {
        let (dir, mut c) = m3_controller("safeword-feed", MoveMode::Stay);
        let sink = LipSink::default();
        c.sink = Some(SinkHandle::new(sink.clone()));
        let (tx, rx) = std::sync::mpsc::channel();
        c.feed = Some(Arc::new(Mutex::new(rx)));
        // System monitor paused: other events are dropped, the safeword not.
        c.monitor_paused = true;
        tx.send(Event::new("user.activity")).unwrap();
        tx.send(Event::new(CHAT_SAFEWORD)).unwrap();
        let before = lillith_core::access::kill_switch::epoch();
        c.tick(0.016).unwrap();
        assert!(!lillith_core::access::kill_switch::stopped_since(before));
        assert_eq!(*sink.interrupts.lock().unwrap(), ["safeword"]);
        assert!(c.feed_backlog.is_empty());
        // Unpaused: a burst of tokens doesn't delay it past this frame.
        c.monitor_paused = false;
        tx.send(Event::new(crate::chat::CHAT_START)).unwrap();
        for _ in 0..(MAX_EVENTS_PER_TICK * 2) {
            tx.send(Event::with_detail(crate::chat::CHAT_TOKEN, "la "))
                .unwrap();
        }
        tx.send(Event::new(CHAT_BARGE_IN)).unwrap();
        tx.send(Event::new(CHAT_SAFEWORD)).unwrap();
        let before = lillith_core::access::kill_switch::epoch();
        c.tick(0.016).unwrap();
        assert!(!lillith_core::access::kill_switch::stopped_since(before));
        assert_eq!(
            *sink.interrupts.lock().unwrap(),
            ["safeword", "barge_in", "safeword"]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn chat_reply_reaches_the_voice_and_speaker_toggle_is_live() {
        let (dir, mut c) = m3_controller("speaker", MoveMode::Stay);
        let spk = Spk::default();
        *spk.on.lock().unwrap() = true;
        c.sink = Some(SinkHandle::new(spk.clone()));
        // Without a voice the item is left out; with one it shows its state.
        assert_eq!(c.menu_state(None).extras.speaker, Some(true));
        c.event(&Event::new(crate::chat::CHAT_START));
        c.event(&Event::with_detail(crate::chat::CHAT_TOKEN, "Hey you~ "));
        c.event(&Event::with_detail(
            crate::chat::CHAT_END,
            "Hey you~ miss me?",
        ));
        assert_eq!(*spk.replies.lock().unwrap(), ["Hey you~ miss me?"]);
        // Errors / empty ends are not spoken.
        c.event(&Event::with_detail(crate::chat::CHAT_ERROR, "boom"));
        c.event(&Event::with_detail(crate::chat::CHAT_END, "  "));
        assert_eq!(spk.replies.lock().unwrap().len(), 1);
        // Right-click ▸ Speak out loud: off, live; the text still shows.
        c.menu_pick(MenuAction::ToggleSpeaker);
        assert!(!*spk.on.lock().unwrap());
        assert_eq!(c.menu_state(None).extras.speaker, Some(false));
        c.event(&Event::new(crate::chat::CHAT_START));
        c.event(&Event::with_detail(crate::chat::CHAT_END, "Quiet now"));
        assert_eq!(spk.replies.lock().unwrap().len(), 1, "speakers off");
        assert_eq!(c.talk.speech.as_ref().unwrap().text, "Quiet now");
        c.menu_pick(MenuAction::ToggleSpeaker);
        assert!(*spk.on.lock().unwrap());
        // Settings save → reload_config → the voice re-reads [voice].
        let cfg_path = dir.join("cfg/config.toml");
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(&cfg_path, "[voice]\nenabled = false\n").unwrap();
        c.config_path = Some(cfg_path);
        c.reload_config();
        assert_eq!(*spk.reloads.lock().unwrap(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn banter_lines_show_in_the_speech_bubble() {
        let (dir, mut c) = m3_controller("banter_bubble", MoveMode::Stay);
        assert_eq!(c.menu_state(None).extras.speaker, None, "no voice wired");
        c.apply(vec![Action::Say("Miss me, Ko?".into())]);
        assert_eq!(c.talk.speech.as_ref().unwrap().text, "Miss me, Ko?");
        assert!(!c.talk.active(), "no lip-flap for a one-off line");
        // A chat reply takes the bubble and is not replaced by banter.
        c.event(&Event::new(crate::chat::CHAT_START));
        c.event(&Event::with_detail(crate::chat::CHAT_TOKEN, "Reply"));
        c.apply(vec![Action::Say("Banter".into())]);
        assert_eq!(c.talk.speech.as_ref().unwrap().text, "Reply");
        c.event(&Event::with_detail(crate::chat::CHAT_END, "Reply!"));
        c.apply(vec![Action::Say("Banter".into())]);
        assert_eq!(c.talk.speech.as_ref().unwrap().text, "Reply!");
        // Once the reply faded, banter shows again; then fades on its own.
        for _ in 0..200 {
            c.tick(0.05).unwrap();
        }
        assert!(c.talk.speech.is_none());
        c.apply(vec![Action::Say("Banter".into())]);
        assert_eq!(c.talk.speech.as_ref().unwrap().text, "Banter");
        for _ in 0..200 {
            c.tick(0.05).unwrap();
        }
        assert!(c.talk.speech.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn right_click_menu_picks_apply() {
        let (dir, mut c) = m3_controller("menu", MoveMode::Stay);
        let cfg_path = dir.join("cfg/config.toml");
        c.config_path = Some(cfg_path.clone());
        let seen = Arc::new(Mutex::new(Vec::new()));
        c.sink = Some(SinkHandle::new(Rec(seen.clone())));
        // Right click on her opens the menu (never quits).
        assert!(c.button(Button::Right, (40.0, 40.0)));
        assert!(c.menu().is_some());
        assert!(c.take_menu_opened());
        assert!(!c.take_menu_opened());
        assert!(c.render_menu().is_some());
        // Esc closes the menu instead of quitting.
        c.key(Key::Escape);
        assert!(c.menu().is_none());
        assert!(!c.quit_requested());

        // Mood ▸ Alert through real clicks on the popup rows.
        c.open_menu();
        let row_y = |c: &Controller, pred: &dyn Fn(&crate::menu_popup::Row) -> bool| {
            let rows = c.menu().unwrap().rows();
            let i = rows.iter().position(pred).unwrap();
            let top: u32 = crate::menu_popup::TOP
                + rows[..i]
                    .iter()
                    .map(|r| match r {
                        crate::menu_popup::Row::Separator => crate::menu_popup::SEP_H,
                        _ => crate::menu_popup::ROW_H,
                    })
                    .sum::<u32>();
            top as f64 + 5.0
        };
        use crate::menu_popup::Row;
        let y = row_y(
            &c,
            &|r| matches!(r, Row::Header { label, .. } if label == "Mood"),
        );
        assert_eq!(c.menu_click((30.0, y)), MenuHit::Changed);
        let y = row_y(
            &c,
            &|r| matches!(r, Row::Leaf { label, .. } if label == "Alert"),
        );
        assert!(matches!(c.menu_click((40.0, y)), MenuHit::Pick(_)));
        assert!(c.menu().is_none(), "a pick closes the menu");
        assert_eq!(c.brain.mood(), lillith_core::Mood::Alert);
        assert!(seen
            .lock()
            .unwrap()
            .contains(&Action::MoodChanged(lillith_core::Mood::Alert)));
        // Click outside dismisses.
        c.open_menu();
        assert_eq!(c.menu_click((-3.0, 4.0)), MenuHit::Dismiss);
        assert!(c.menu().is_none());

        // Mute reaches the voice sink.
        c.menu_pick(MenuAction::ToggleMute);
        assert!(c.brain.muted());
        assert!(seen.lock().unwrap().contains(&Action::Muted(true)));

        // Pause system monitor: feed events are dropped.
        let (tx, rx) = std::sync::mpsc::channel();
        c.feed = Some(Arc::new(Mutex::new(rx)));
        c.menu_pick(MenuAction::ToggleMonitor);
        assert!(c.monitor_paused);
        tx.send(Event::new("idle.asleep")).unwrap();
        c.tick(0.05).unwrap();
        assert_ne!(c.stage.animator().base(), "sleep");
        c.menu_pick(MenuAction::ToggleMonitor);
        assert!(!c.monitor_paused);
        tx.send(Event::new("idle.asleep")).unwrap();
        c.tick(0.05).unwrap();
        assert_eq!(c.stage.animator().base(), "sleep");

        // Size ▸ Large: stage resized, scale persisted.
        let before = c.stage.size();
        c.menu_pick(MenuAction::SetSize(lillith_core::menu::Size::Large));
        assert!(c.take_resized());
        assert_eq!(c.stage.scale(), 0.75);
        assert!(c.stage.size().0 < before.0, "2.0 -> 0.75");
        c.stage.render().unwrap();
        let saved = Config::load(&cfg_path).unwrap();
        assert_eq!(saved.scale, 0.75);

        // Reset position: default spot, moved and saved.
        c.menu_pick(MenuAction::ResetPosition);
        let p = c.take_move().unwrap();
        assert_eq!((p.left, p.bottom), (80, 48));
        let saved = Config::load(&cfg_path).unwrap();
        assert_eq!(saved.overlay, OverlayPos::default());
        assert_eq!(saved.scale, 0.75, "other keys kept");

        // About speaks through the sink; Quit quits.
        c.menu_pick(MenuAction::About);
        assert!(seen
            .lock()
            .unwrap()
            .iter()
            .any(|a| matches!(a, Action::Say(t) if t.contains("Lillith"))));
        c.menu_pick(MenuAction::Quit);
        assert!(c.quit_requested());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[cfg(unix)] // fake_bin is a shell script
    fn settings_window_picks_apply_on_tick() {
        use crate::settings_window::tests::{fake_bin, wait_for};
        let (dir, mut c) = m3_controller("settingswin", MoveMode::Stay);
        let (bin_dir, bin) = fake_bin(
            "ctl",
            &[
                r#"{"menu_action":"set_mood","value":"alert"}"#,
                r#"{"menu_action":"toggle_mute"}"#,
                r#"{"menu_action":"set_mode","value":"follow"}"#,
            ],
        );
        c.settings = SettingsWindow::with_program(&bin);
        c.settings_path = Some(dir.join("cfg/config.toml"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        c.sink = Some(SinkHandle::new(Rec(seen.clone())));
        c.menu_pick(MenuAction::OpenSettings);
        assert!(c.settings.running());
        // Picks only land when the overlay thread ticks.
        assert!(wait_for(|| {
            c.tick(0.01).unwrap();
            c.brain.muted() && c.brain.mode() == MoveMode::Follow
        }));
        assert_eq!(c.brain.mood(), lillith_core::Mood::Alert);
        // The window is told where the overlay's own clips live.
        let env = std::fs::read_to_string(bin_dir.join("manifest_env")).unwrap();
        assert_eq!(
            env,
            std::path::absolute(&c.manifest_path)
                .unwrap()
                .display()
                .to_string()
        );
        let got = seen.lock().unwrap().clone();
        assert!(got.contains(&Action::MoodChanged(lillith_core::Mood::Alert)));
        assert!(got.contains(&Action::Muted(true)), "voice sees it");
        // A second "Open settings" while it is up spawns nothing.
        c.menu_pick(MenuAction::OpenSettings);
        assert!(c.settings.running());
        std::fs::write(bin_dir.join("release"), "").unwrap();
        assert!(wait_for(|| !c.settings.running()));
        let _ = std::fs::remove_dir_all(bin_dir);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn click_opens_chat_and_reply_drives_talk_loop() {
        let (dir, _) = fixture("chat");
        let frames = |n: usize| {
            let f: Vec<String> = (0..n)
                .map(|i| {
                    format!(
                        "{{ x = {}, y = 0, w = 40, h = 40, anchor = [20, 38] }}",
                        (i % 2) * 40
                    )
                })
                .collect();
            format!(
                "page = \"idle.png\"\nfps = 8.0\nframes = [ {} ]\n",
                f.join(", ")
            )
        };
        let toml = format!(
            "[clips.idle]\n{}[clips.wave]\n{}[clips.knife_tap]\n{}[clips.talk_loop]\n{}",
            frames(2),
            frames(2),
            frames(2),
            frames(4)
        );
        let m = lillith_anim::atlas::Manifest::from_toml_str(&toml).unwrap();
        let opened = Arc::new(Mutex::new(Vec::new()));
        let sink = opened.clone();
        let opts = OverlayOptions {
            manifest: dir.join("manifest.toml"),
            scale: 2.0,
            opacity: 1.0,
            render_hz: 30.0,
            animator: AnimatorConfig {
                idle_enabled: false,
                ..AnimatorConfig::default()
            },
            position: OverlayPos {
                margin_left: 100,
                margin_bottom: 50,
                output: None,
                ..OverlayPos::default()
            },
            config_path: None,
            forced: false,
            exit_after: None,
            behavior: BehaviorConfig::default(),
            mode: MoveMode::Stay,
            muted: false,
            events: None,
            sink: None,
            settings_path: None,
            chat: Some(ChatHook::new(move |a| sink.lock().unwrap().push(a))),
            chat_on_click: true,
            overlay_speech: false,
            always_on_top: true,
            click_through: false,
            avatars: None,
            avatar: Default::default(),
            seasonal_outfit: "auto".into(),
        };
        let stage = Stage::new(m, PageCache::new(&dir), 2.0, opts.animator.clone()).unwrap();
        let mut c = Controller::with_stage(stage, &opts);
        c.set_screen((1920, 1080));
        c.stage.render().unwrap();
        // Single click: opens after the double-click window.
        assert!(c.press((40.0, 40.0), (40.0, 40.0), true));
        assert_eq!(c.release(), Release::Click);
        c.tick(0.1).unwrap();
        assert!(
            opened.lock().unwrap().is_empty(),
            "waits for a possible double click"
        );
        c.tick(0.4).unwrap();
        let a = opened.lock().unwrap()[0];
        assert_eq!((a.x, a.w, a.h), (100, 80, 80));
        assert_eq!(a.y, 1080 - 50 - 80);
        // Double click: no second bubble.
        for _ in 0..2 {
            assert!(c.press((40.0, 40.0), (40.0, 40.0), true));
            c.release();
            c.tick(0.05).unwrap();
        }
        c.tick(1.0).unwrap();
        assert_eq!(opened.lock().unwrap().len(), 1);
        // Drag: no bubble.
        assert!(c.press((40.0, 40.0), (40.0, 40.0), true));
        c.motion((70.0, 20.0));
        c.release();
        c.tick(1.0).unwrap();
        // She falls back to the floor and lands (gravity) before talking.
        for _ in 0..30 {
            c.tick(0.1).unwrap();
        }
        assert_eq!(c.gravity.mode, Mode::Floor);
        assert_eq!(opened.lock().unwrap().len(), 1);
        // Right-click menu ▸ Chat opens it too.
        assert!(c.menu_state(None).extras.chat);
        c.menu_pick(MenuAction::Chat);
        c.tick(0.1).unwrap();
        assert_eq!(opened.lock().unwrap().len(), 2);

        // A streamed reply: talk_loop with lip-flap, then back to idle.
        c.event(&Event::new(crate::chat::CHAT_START));
        assert_eq!(c.stage.animator().base(), "talk_loop");
        c.event(&Event::with_detail(crate::chat::CHAT_TOKEN, "Hey cutie"));
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..20 {
            c.tick(1.0 / 30.0).unwrap();
            seen.insert(c.stage.animator().current().frame);
        }
        assert!(
            seen.contains(&0) && seen.contains(&2),
            "mouth moves: {seen:?}"
        );
        assert_eq!(c.talk.speech.as_ref().unwrap().text, "Hey cutie");
        c.event(&Event::with_detail(crate::chat::CHAT_END, "Hey cutie~"));
        assert_eq!(c.talk.speech.as_ref().unwrap().text, "Hey cutie~");
        // Slow default pace: still talking/smiling after 1 s, idle by 5 s.
        for _ in 0..30 {
            c.tick(1.0 / 30.0).unwrap();
        }
        assert!(c.talk.active());
        for _ in 0..120 {
            c.tick(1.0 / 30.0).unwrap();
        }
        assert!(!c.talk.active());
        assert_eq!(c.stage.animator().base(), "idle");
        assert_eq!(c.stage.animator().pinned_base_frame(), None);
        // Y/Enter opens the bubble too.
        c.key(Key::Enter);
        assert_eq!(opened.lock().unwrap().len(), 3);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Talk fixture: idle + talk_loop with mouth patches (`m/<shape>.png`,
    /// solid colours) unless `patches` is false (then the files are absent).
    fn talk_controller(tag: &str, patches: bool, speech: bool) -> (std::path::PathBuf, Controller) {
        let (dir, _) = fixture(tag);
        std::fs::create_dir_all(dir.join("m")).unwrap();
        let colours = [
            ("closed", [0u8, 200, 0, 255]),
            ("mid", [0, 0, 200, 255]),
            ("open", [0, 0, 255, 255]),
            ("smile", [200, 200, 0, 255]),
        ];
        let mut table = String::new();
        for (shape, rgba) in colours {
            let rel = format!("m/{shape}.png");
            if patches {
                let px: Vec<u8> = rgba.iter().copied().cycle().take(6 * 4 * 4).collect();
                tiny_skia::Pixmap::from_vec(px, tiny_skia::IntSize::from_wh(6, 4).unwrap())
                    .unwrap()
                    .save_png(dir.join(&rel))
                    .unwrap();
            } else {
                let _ = std::fs::remove_file(dir.join(&rel));
            }
            table += &format!(
                "[clips.talk_loop.mouth_patches.{shape}]\npage = \"{rel}\"\ncentre = [3.0, 2.0]\n"
            );
        }
        let toml = format!(
            r#"
            [clips.idle]
            page = "idle.png"
            fps = 8.0
            frames = [ {{ x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] }} ]
            [clips.talk_loop]
            page = "idle.png"
            fps = 8.0
            mouth_angle_deg = [0.0, 3.0]
            mouth_shape_ms = [200, 260]
            mouth_sequence = ["closed", "open"]
            frames = [ {{ x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] }}, {{ x = 40, y = 0, w = 40, h = 40, anchor = [20, 38] }} ]
            [clips.talk_loop.points]
            mouth_point = [20, 12]
            [clips.talk_loop.frame_points]
            mouth_point = [[20, 12], [21, 12]]
            {table}
            "#
        );
        let m = lillith_anim::atlas::Manifest::from_toml_str(&toml).unwrap();
        let opts = OverlayOptions {
            manifest: dir.join("manifest.toml"),
            scale: 2.0,
            opacity: 1.0,
            render_hz: 30.0,
            animator: AnimatorConfig {
                idle_enabled: false,
                breath_scale: 0.0,
                breath_chest: 0.0,
                ..AnimatorConfig::default()
            },
            position: OverlayPos::default(),
            config_path: None,
            forced: false,
            exit_after: None,
            behavior: BehaviorConfig::default(),
            mode: MoveMode::Stay,
            muted: false,
            events: None,
            sink: None,
            settings_path: None,
            chat: None,
            chat_on_click: false,
            overlay_speech: speech,
            always_on_top: true,
            click_through: false,
            avatars: None,
            avatar: Default::default(),
            seasonal_outfit: "auto".into(),
        };
        let mut stage = Stage::new(m, PageCache::new(&dir), 2.0, opts.animator.clone()).unwrap();
        if speech {
            stage = stage.with_speech_area().unwrap();
        }
        let mut c = Controller::with_stage(stage, &opts);
        c.set_screen((1920, 1080));
        c.stage.render().unwrap();
        (dir, c)
    }

    fn count_px(c: &Controller, rgb: [u8; 3], rows: std::ops::Range<u32>) -> usize {
        let cv = c.stage.canvas();
        let w = cv.width();
        cv.data()
            .chunks_exact(4)
            .enumerate()
            .filter(|(i, p)| {
                rows.contains(&(*i as u32 / w))
                    && p[3] > 200
                    && p[..3].iter().zip(rgb).all(|(a, b)| a.abs_diff(b) < 30)
            })
            .count()
    }

    #[test]
    fn reply_shows_speech_bubble_and_pastes_mouth_patches() {
        let (dir, mut c) = talk_controller("lips", true, true);
        let pad = c.stage.speech_pad();
        assert!(pad > 30, "speech area {pad}");
        assert_eq!(c.stage.size(), (80, 80 + pad));
        // Her body is still hit-tested (shifted down by the pad); the empty
        // speech area is click-through.
        assert!(c.stage.hit(40.0, pad as f64 + 40.0));
        assert!(!c.stage.hit(40.0, pad as f64 / 2.0));

        c.event(&Event::new(crate::chat::CHAT_START));
        assert_eq!(c.stage.animator().base(), "talk_loop");
        assert!(c.talk.uses_patches());
        c.event(&Event::with_detail(
            crate::chat::CHAT_TOKEN,
            "Hello there cutie, I missed you so much today! 🥰",
        ));
        let mut shapes = std::collections::BTreeSet::new();
        let mut mouth_px = 0;
        for _ in 0..90 {
            c.tick(1.0 / 30.0).unwrap();
            if let Some(m) = c.stage.animator().current().mouth {
                shapes.insert(m.shape.name());
                // Patch pasted around the mouth point (cell y 12 -> canvas
                // pad + 24 at scale 2).
                mouth_px += count_px(&c, [0, 0, 255], pad + 16..pad + 34)
                    + count_px(&c, [0, 0, 200], pad + 16..pad + 34)
                    + count_px(&c, [0, 200, 0], pad + 16..pad + 34);
            }
        }
        assert!(shapes.len() >= 2, "mouth changes shape: {shapes:?}");
        assert!(mouth_px > 0, "patch pixels drawn");
        // The body keeps playing (no frame pin) in patch mode.
        assert_eq!(c.stage.animator().pinned_base_frame(), None);
        // Bubble: text revealed gradually, drawn in the speech area, and
        // never part of the click mask.
        let shown = c.talk.speech.as_ref().unwrap().revealed_text().to_string();
        assert!(!shown.is_empty() && shown.len() < 50, "revealed {shown:?}");
        assert!(!c.stage.speech_lines().is_empty());
        assert!(
            count_px(
                &c,
                [
                    crate::speech::STROKE[0],
                    crate::speech::STROKE[1],
                    crate::speech::STROKE[2]
                ],
                0..pad
            ) > 20,
            "bubble outline"
        );
        assert!(
            !c.stage.hit(40.0, pad as f64 / 2.0),
            "bubble is click-through"
        );

        c.event(&Event::with_detail(
            crate::chat::CHAT_END,
            "Hello there cutie, I missed you so much today! 🥰",
        ));
        // Fully revealed, held, then faded and gone; idle again.
        for _ in 0..(30 * 4) {
            c.tick(1.0 / 30.0).unwrap();
        }
        assert!(c.talk.speech.is_some(), "still visible right after");
        for _ in 0..(30 * 8) {
            c.tick(1.0 / 30.0).unwrap();
        }
        assert!(c.talk.speech.is_none());
        assert!(!c.talk.active());
        assert_eq!(c.stage.animator().base(), "idle");
        assert!(c.stage.animator().current().mouth.is_none());
        assert_eq!(
            count_px(
                &c,
                [
                    crate::speech::STROKE[0],
                    crate::speech::STROKE[1],
                    crate::speech::STROKE[2]
                ],
                0..pad
            ),
            0,
            "bubble gone"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_mouth_patches_fall_back_to_frame_picking() {
        let (dir, mut c) = talk_controller("nolips", false, false);
        assert_eq!(c.stage.speech_pad(), 0);
        assert_eq!(c.stage.size(), (80, 80));
        c.event(&Event::new(crate::chat::CHAT_START));
        assert!(!c.talk.uses_patches());
        c.event(&Event::with_detail(crate::chat::CHAT_TOKEN, "Oh hi, you!"));
        let mut frames = std::collections::BTreeSet::new();
        for _ in 0..45 {
            c.tick(1.0 / 30.0).unwrap();
            assert!(c.stage.animator().current().mouth.is_none());
            frames.insert(c.stage.animator().current().frame);
        }
        assert_eq!(frames.len(), 2, "baked mouths picked: {frames:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn talk_without_talk_clip_or_mouth_patches_degrades_gracefully() {
        // Like the v2 set: no talk_loop, no mouth patches/points.
        let (dir, mut c) = m3_controller("notalk", MoveMode::Stay);
        assert!(!c.stage.animator().has_clip("talk_loop"));
        let base = c.stage.animator().base().to_string();
        c.event(&Event::new(crate::chat::CHAT_START));
        assert!(!c.talk.uses_patches());
        c.event(&Event::with_detail(crate::chat::CHAT_TOKEN, "Hey you~ "));
        c.event(&Event::with_detail(crate::chat::CHAT_TOKEN, "miss me?"));
        for _ in 0..30 {
            c.tick(1.0 / 30.0).unwrap();
            let a = c.stage.animator();
            assert_eq!(a.base(), base, "base clip untouched");
            assert_eq!(a.pinned_base_frame(), None, "no frame pinning on idle");
            assert!(a.current().mouth.is_none(), "no mouth patch drawn");
        }
        assert!(c.talk.active(), "still talks (speech bubble, voice)");
        assert_eq!(c.talk.speech.as_ref().unwrap().text, "Hey you~ miss me?");
        c.event(&Event::with_detail(
            crate::chat::CHAT_END,
            "Hey you~ miss me?",
        ));
        for _ in 0..300 {
            c.tick(1.0 / 30.0).unwrap();
        }
        assert!(!c.talk.active());
        assert_eq!(c.stage.animator().base(), base);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn wander_moves_the_window() {
        let (dir, mut c) = m3_controller("m3w", MoveMode::Wander);
        c.set_screen((1200, 800));
        let mut moved = Vec::new();
        let mut walked = false;
        for _ in 0..1200 {
            c.tick(0.1).unwrap();
            // walk_left is walk_right mirrored in this manifest.
            walked |= c.stage.animator().base() == "walk_right";
            if let Some(p) = c.take_move() {
                assert_eq!(p.bottom, 50, "only x changes");
                c.placement = p;
                moved.push(p.left);
            }
        }
        assert!(!moved.is_empty(), "she wandered");
        assert!(walked, "walk clip played");
        assert!(moved.iter().all(|&x| (0..=1200 - 80).contains(&x)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_backend_feature_is_reported() {
        let opts = OverlayOptions {
            manifest: "/nonexistent/manifest.toml".into(),
            scale: 1.0,
            opacity: 1.0,
            render_hz: 30.0,
            animator: AnimatorConfig::default(),
            position: OverlayPos::default(),
            config_path: None,
            forced: true,
            exit_after: Some(Duration::ZERO),
            behavior: BehaviorConfig::default(),
            mode: MoveMode::Stay,
            muted: false,
            events: None,
            sink: None,
            settings_path: None,
            chat: None,
            chat_on_click: false,
            overlay_speech: false,
            always_on_top: true,
            click_through: false,
            avatars: None,
            avatar: Default::default(),
            seasonal_outfit: "auto".into(),
        };
        // Either the backend is not compiled in, or the manifest is missing:
        // both are clean errors, never a panic or a hang.
        assert!(run(Backend::X11, opts).is_err());
        assert_eq!(period(30.0), Duration::from_secs_f32(1.0 / 30.0));
        assert_eq!(period(0.0), Duration::from_secs(1));
    }

    #[test]
    fn menu_commands_pose_resize_peek() {
        let (dir, mut c) = m3_controller("menupicks", MoveMode::Stay);
        c.set_screen((1920, 1080));
        // Poses ▸ <category> page ▸ wave: plays once on top of idle.
        let st = c.menu_state(None);
        let n = (0..64)
            .find(|&i| st.extras.pose_at(i) == Some("wave"))
            .expect("wave is in the Poses menu");
        c.menu_pick(MenuAction::PosePage(MenuPage::default()));
        assert!(c.menu().is_some(), "a Poses page reopens the popup");
        assert!(c.take_menu_opened());
        c.close_menu();
        c.menu_pick(MenuAction::Pose(n as u16));
        assert_eq!(c.stage.animator().overlay_clip(), Some("wave"));
        // Out-of-range pose: ignored, no panic.
        c.menu_pick(MenuAction::Pose(9999));
        // Resize ▸ Large (live): the stage changes, the backend is told once.
        let before = c.stage.size();
        c.menu_pick(MenuAction::SetSize(Size::Large));
        assert!(c.take_resized());
        assert!(!c.take_resized());
        assert!((c.stage.scale() - 0.75).abs() < 1e-6);
        let exp = |v: u32| (v as f32 * 0.75 / 2.0).round() as i64;
        let now = c.stage.size();
        assert!(
            (now.0 as i64 - exp(before.0)).abs() <= 1,
            "{before:?} → {now:?}"
        );
        assert!(
            (now.1 as i64 - exp(before.1)).abs() <= 1,
            "{before:?} → {now:?}"
        );
        // Bigger: one step.
        c.menu_pick(MenuAction::ScaleStep(1));
        assert!((c.stage.scale() - 0.8).abs() < 1e-6);
        assert!(c.take_resized());
        c.menu_pick(MenuAction::SetSize(Size::Huge));
        assert!((c.stage.scale() - 1.25).abs() < 1e-6);
        assert!(c.take_resized());
        // Minimize to the peek icon at the right edge, then restore.
        let placed = c.placement;
        c.menu_pick(MenuAction::Minimize);
        assert!(c.peek.is_some());
        assert!((c.stage.scale() - PEEK_SCALE).abs() < 1e-6);
        assert!(c.take_resized());
        let m = c.take_move().expect("moves to the edge");
        assert!(m.left > 1800 && m.bottom == 0, "{m:?}");
        // While peeking she does not wander.
        c.apply(vec![Action::MoveTo { left: 5 }]);
        assert!(c.take_move().is_none());
        c.restore();
        assert!(c.peek.is_none());
        assert!((c.stage.scale() - 1.25).abs() < 1e-6);
        assert_eq!(c.take_move(), Some(placed));
        // Show off: an allure pose starts (no panic without allure clips).
        c.menu_pick(MenuAction::ShowOff);
        // Chat without a chat hook: disabled in the menu, a no-op if picked.
        assert!(!c.menu_state(None).extras.chat);
        c.menu_pick(MenuAction::Chat);
        // Close quits.
        c.menu_pick(MenuAction::Quit);
        assert!(c.quit_requested());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn seasonal_outfit_menu_pick_persist_and_live() {
        let frames = r#"page = "idle.png"
mode = "loop"
frames = [ { x = 0, y = 0, w = 40, h = 40, anchor = [20, 38] } ]
"#;
        let mut extra = String::new();
        for c in ["halloween_hat_tip_wink", "halloween_cape_swirl_arch"] {
            extra.push_str(&format!(
                "[clips.{c}]\n{frames}tags = [\"allure\", \"seasonal\", \"season:halloween\", \"outfit:halloween\"]\n"
            ));
        }
        extra.push_str(&format!(
            "[clips.xmas_bow_wink]\n{frames}tags = [\"seasonal\", \"season:xmas\", \"outfit:xmas\"]\n"
        ));
        let (dir, mut c) = m3_controller_with("seasons", MoveMode::Stay, &extra);
        let cfg = dir.join("cfg/config.toml");
        c.config_path = Some(cfg.clone());
        let md = |m, d| MonthDay::new(m, d).unwrap();
        let flirty = |c: &Controller| -> Vec<String> {
            c.menu_state(None)
                .extras
                .poses
                .iter()
                .filter(|g| g.label == lillith_anim::catalog::CATEGORIES[0])
                .flat_map(|g| g.clips.iter().map(|(n, _)| n.clone()))
                .collect()
        };
        // Auto on Oct 31: the witch outfit; only Christmas clips hidden.
        c.refresh_outfit(md(10, 31));
        assert_eq!(c.outfit_active().map(|o| o.id.as_str()), Some("halloween"));
        assert_eq!(
            c.brain.hidden_clips().iter().collect::<Vec<_>>(),
            ["xmas_bow_wink"]
        );
        assert_eq!(flirty(&c).len(), 2, "{:?}", flirty(&c));
        let x = c.menu_state(None).extras;
        assert_eq!(x.outfits.len(), 6);
        assert!(x.outfits[0].available && x.outfits.iter().skip(1).all(|o| !o.available));
        assert_eq!(x.outfit_active.as_deref(), Some("Micro Witch (Halloween)"));
        // Show off while she wears it includes one of its clips.
        c.menu_pick(MenuAction::ShowOff);
        let witch = |n: &str| n.starts_with("halloween_");
        assert!(
            c.showoff.iter().any(|n| witch(n))
                || c.stage.animator().overlay_clip().is_some_and(witch),
            "{:?}",
            c.showoff
        );
        // Nov 3: none; every seasonal clip hidden, none in the pages.
        c.refresh_outfit(md(11, 3));
        assert_eq!(c.outfit_active(), None);
        assert_eq!(c.brain.hidden_clips().len(), 3);
        assert!(flirty(&c).is_empty());
        // Avatar ▸ Outfit opens the page with the current choice checked.
        c.menu_pick(MenuAction::OutfitPage);
        let popup = c.menu().expect("outfit page");
        assert!(popup.state.extras.outfit_page);
        let page = popup.menu.clone();
        assert_eq!(
            page.check_of(MenuAction::Outfit(0)),
            Some(lillith_core::menu::Check::Radio(true))
        );
        c.close_menu();
        // Manual pick in July: persisted and worn now.
        c.refresh_outfit(md(7, 15));
        c.menu_pick(MenuAction::Outfit(2));
        assert_eq!(c.outfit_setting(), "halloween");
        assert_eq!(c.outfit_active().map(|o| o.id.as_str()), Some("halloween"));
        assert_eq!(Config::load(&cfg).unwrap().seasonal_outfit, "halloween");
        // "Coming soon" outfits can't be picked.
        c.menu_pick(MenuAction::Outfit(3));
        assert_eq!(c.outfit_setting(), "halloween");
        // Off: base outfit, saved.
        c.menu_pick(MenuAction::Outfit(1));
        assert_eq!(c.outfit_active(), None);
        assert_eq!(Config::load(&cfg).unwrap().seasonal_outfit, "off");
        assert!(c.showoff.iter().all(|n| !n.starts_with("halloween_")));
        // The settings window sends ids by name.
        c.settings_pick(MenuPick::SeasonalOutfit("halloween".into()));
        assert_eq!(c.outfit_active().map(|o| o.id.as_str()), Some("halloween"));
        // A config reload applies the key live.
        let cf = Config {
            seasonal_outfit: "off".into(),
            ..Config::default()
        };
        c.apply_config(&cf);
        assert_eq!(c.outfit_active(), None);
        // No clip was removed from the set.
        assert!(c.stage.animator().has_clip("xmas_bow_wink"));
        assert!(c.stage.animator().has_clip("halloween_hat_tip_wink"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn right_click_opens_the_popup_menu() {
        let (dir, mut c) = m3_controller("menuhook", MoveMode::Stay);
        c.set_screen((1920, 1080));
        assert!(c.button(Button::Right, (40.0, 40.0)));
        assert!(c.menu().is_some(), "her own popup, not a child window");
        assert!(c.take_menu_opened());
        c.close_menu();
        // Off her: nothing.
        assert!(!c.button(Button::Right, (1.0, 1.0)));
        assert!(c.menu().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[cfg(unix)] // fake_bin is a shell script
    fn settings_window_maps_the_new_menu_items() {
        use crate::settings_window::tests::{fake_bin, wait_for};
        let (dir, mut c) = m3_controller("swnew", MoveMode::Stay);
        c.set_screen((1920, 1080));
        c.settings_path = Some(dir.join("cfg/config.toml"));
        let (bin_dir, bin) = fake_bin(
            "swnew",
            &[
                r#"{"menu_action":"set_size","value":"xl"}"#,
                r#"{"menu_action":"pose","value":"wave"}"#,
                r#"{"menu_action":"avatar","value":"lillith"}"#,
                r#"{"menu_action":"reload_config"}"#,
            ],
        );
        c.settings = SettingsWindow::with_program(&bin);
        c.menu_pick(MenuAction::OpenSettings);
        assert!(c.settings.running());
        assert!(wait_for(|| {
            c.tick(0.01).unwrap();
            c.stage.animator().overlay_clip() == Some("wave")
        }));
        assert!((c.stage.scale() - 0.95).abs() < 1e-6, "Resize ▸ XL");
        assert_eq!(c.stage.avatar_id(), None);
        // Avatar ▸ Custom image… goes to the same (single) window.
        c.menu_pick(MenuAction::ImportAvatar);
        assert!(c.settings.running());
        std::fs::write(bin_dir.join("release"), "").unwrap();
        assert!(wait_for(|| !c.settings.running()));
        let _ = std::fs::remove_dir_all(bin_dir);

        // A second window: Minimize to peek, Show off, Close.
        let (bin_dir, bin) = fake_bin(
            "swnew2",
            &[
                r#"{"menu_action":"show_off"}"#,
                r#"{"menu_action":"minimize"}"#,
                r#"{"menu_action":"quit"}"#,
            ],
        );
        c.settings = SettingsWindow::with_program(&bin);
        c.menu_pick(MenuAction::ImportAvatar);
        assert!(c.settings.running(), "Custom image… opens the window");
        assert!(wait_for(|| {
            c.tick(0.01).unwrap();
            c.quit_requested()
        }));
        assert!(c.peek.is_some(), "minimized to peek");
        std::fs::write(bin_dir.join("release"), "").unwrap();
        assert!(wait_for(|| !c.settings.running()));
        let _ = std::fs::remove_dir_all(bin_dir);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn click_through_keeps_only_the_head() {
        let (dir, mut c) = m3_controller("ct", MoveMode::Stay);
        let full = c.stage.mask().to_vec();
        let full_h = full.iter().map(|r| r.y + r.h).max().unwrap();
        c.stage.set_click_through(true);
        c.stage.render().unwrap();
        let head = c.stage.mask().to_vec();
        assert!(!head.is_empty(), "her head stays clickable");
        let head_h = head.iter().map(|r| r.y + r.h).max().unwrap();
        assert!(head_h < full_h / 2, "{head_h} vs {full_h}");
        assert!(!c.stage.hit(40.0, 70.0), "body passes clicks through");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_reload_applies_live() {
        let (dir, mut c) = m3_controller("reload", MoveMode::Stay);
        let path = dir.join("config.toml");
        let mut cfg = Config::default();
        cfg.anim.playback_speed = 0.5;
        cfg.anim.talk_speed = 0.4;
        cfg.jiggle_mode = Some(JiggleMode::Off);
        cfg.click_through = true;
        cfg.always_on_top = false;
        cfg.scale = 1.5;
        cfg.save(&path).unwrap();
        c.config_path = Some(path);
        c.event(&Event::new("settings.changed"));
        assert!(c.stage.click_through());
        assert_eq!(c.take_level(), Some(false));
        assert!((c.stage.scale() - 1.5).abs() < 1e-6);
        assert!(c.take_resized());
        assert!((c.talk_speed() - 0.4).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn chest_bounce_slider_applies_live_over_the_wire() {
        // Settings ▸ Motion ▸ Chest bounce saves [springs.bounce] and sends
        // reload_config; the running driver picks it up without a restart.
        let (dir, mut c) = m3_controller("bounce", MoveMode::Stay);
        let (_, m) = m3_manifest_with("bounce", "");
        let anim = AnimatorConfig {
            idle_enabled: false,
            physics_driver: true,
            ..AnimatorConfig::default()
        };
        c.stage = Stage::new(m, PageCache::new(&dir), 2.0, anim).unwrap();
        let path = dir.join("config.toml");
        c.config_path = Some(path.clone());
        let gain = |c: &Controller| {
            let d = c.stage.animator().driver().expect("physics driver");
            (d.physics().chest_bounce_gain, d.config().springs.chest)
        };
        let reload = r#"{"menu_action":"reload_config"}"#;
        let mut cfg = Config::default();
        let mut seen = Vec::new();
        for intensity in [0.0, 1.0, 3.0, 1.0] {
            cfg.springs.bounce.intensity = intensity;
            cfg.save(&path).unwrap();
            c.settings_pick(lillith_core::menu::parse_pick(reload).unwrap());
            c.tick(1.0 / 30.0).unwrap();
            seen.push(gain(&c));
        }
        assert_eq!(seen[0].0, 0.0, "0% = off");
        assert!(seen[2].0 > 2.9 * seen[1].0 - 1e-4, "300%: {seen:?}");
        assert_eq!(seen[3], seen[1], "back to 100% restores it exactly");
        // softness retunes the spring live
        cfg.springs.bounce.softness = 0.0;
        cfg.save(&path).unwrap();
        c.settings_pick(lillith_core::menu::parse_pick(reload).unwrap());
        assert!(gain(&c).1.stiffness > seen[1].1.stiffness, "firmer");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn custom_avatar_animates_smoothly_every_frame() {
        let (dir, mut c) = m3_controller("avatar", MoveMode::Stay);
        // Store one avatar made from a flat-background "character".
        let mut img = image::RgbaImage::from_pixel(120, 200, image::Rgba([30, 140, 130, 255]));
        for y in 10..195 {
            for x in 35..85 {
                let head = y < 60 && ((x as i32 - 60).pow(2) + (y as i32 - 35).pow(2)) < 400;
                if head || y >= 60 {
                    img.put_pixel(x, y, image::Rgba([240, 120, 170, 255]));
                }
            }
        }
        let src = dir.join("buddy.png");
        img.save(&src).unwrap();
        let (fit, meta) = lillith_anim::avatar::import_image(&src, Default::default()).unwrap();
        let store = AvatarStore::new(&dir);
        let id = store.save(&fit, &meta).unwrap();
        // Imported once (Settings ▸ Avatar), then back to Lillith: a recent.
        store.select(Some(&id)).unwrap();
        store.select(None).unwrap();
        c.avatars = Some(store);
        // Avatar ▸ <recent #1>.
        let st = c.menu_state(None);
        assert_eq!(
            st.extras.avatars.first().map(|a| a.0.as_str()),
            Some(id.as_str())
        );
        assert_eq!(
            c.accent(),
            lillith_theme::Accent::LILLITH,
            "Lillith default"
        );
        c.menu_pick(MenuAction::Avatar(1));
        assert_eq!(c.stage.avatar_id(), Some(id.as_str()));
        assert!(c.render_hz() >= 60.0, "smoother redraw for avatars");
        // The menu follows the avatar's (derived, pink-ish) accent.
        assert_ne!(c.accent(), lillith_theme::Accent::LILLITH);
        c.open_menu();
        assert_eq!(c.menu().unwrap().accent(), c.accent());
        c.close_menu();
        // Every tick re-renders (continuous motion), and consecutive
        // frames differ only a little (no jumps).
        c.tick(1.0 / 60.0).unwrap();
        let mut prev = c.stage.canvas().data().to_vec();
        let mut max_changed = 0usize;
        let mut renders = 0;
        for _ in 0..90 {
            let ch = c.tick(1.0 / 60.0).unwrap();
            renders += ch.pixels as usize;
            let cur = c.stage.canvas().data();
            let changed = cur
                .chunks_exact(4)
                .zip(prev.chunks_exact(4))
                .filter(|(a, b)| a[3].abs_diff(b[3]) > 64)
                .count();
            max_changed = max_changed.max(changed);
            prev = cur.to_vec();
        }
        assert!(renders >= 80, "renders {renders}");
        let ink = prev.chunks_exact(4).filter(|p| p[3] > 128).count();
        assert!(ink > 500, "avatar drawn ({ink})");
        assert!(
            max_changed * 20 < ink,
            "smooth: {max_changed} of {ink} px flipped"
        );
        // Persisted and back to Lillith.
        assert_eq!(
            c.avatars.as_ref().unwrap().selection().selected.as_deref(),
            Some(id.as_str())
        );
        c.menu_pick(MenuAction::Avatar(0));
        assert_eq!(c.stage.avatar_id(), None);
        assert_eq!(c.accent(), lillith_theme::Accent::LILLITH);
        let _ = std::fs::remove_dir_all(dir);
    }
}
