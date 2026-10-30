//! Loneliness → attention (Ko's ask, 2026-10-29): when Ko leaves her alone
//! she grows lonely and uses poses to get his attention. The lonelier she
//! is, the bolder (higher-severity) the poses drawn into her idle pose
//! cycle, and now and then she makes an *attention bid* (a pose right away
//! instead of waiting for the next cycle).
//!
//! Pure and I/O-free, driven by a fake clock like [`crate::state::Brain`]:
//!
//! - **Neglect** `0.0..=1.0`: grows linearly with time since Ko's last
//!   *real* interaction with her (a click, drag, key, long hover, chat,
//!   voice): `neglect += dt / (minutes_to_max · 60) · habit_factor(hour)`.
//! - **Score** `0.0..=1.0` = [`ease`]`(neglect)` (smoothstep): eased, so
//!   a short absence barely registers, the middle climbs steadily and the
//!   top levels out instead of slamming into the cap.
//!   [`habit_factor`] comes from his learned presence by time of day
//!   (the memory DB's `events` table, read only): at hours he is usually
//!   around it is up to 1.5× (rises faster); at hours he is usually away
//!   down to 0.5× (rises slower); no data = 1×. A real interaction resets
//!   neglect and score to 0. Quiet hours and system interactions hold it still.
//! - **Level** `0..=3` ([`MAX_LEVEL`]): thresholds 0.25 / 0.5 / 0.75 with
//!   hysteresis (a level drops only once the score is
//!   `hysteresis` below its threshold, so it does not flicker), then capped
//!   by `max_severity`.
//! - **Picker**: draws from every non-system tagged pose with
//!   `severity <= min(level, max_severity)`, weighted toward the current
//!   level (same level 6, one below 3, further below 1), so calmer poses
//!   still sometimes appear and the cap always holds. No immediate repeats
//!   ([`ShuffleBag`]).
//! - **Never during system interactions** (Soul Contract / deletion cards,
//!   settings and setup wizard, file dialogs, console, installs) or while
//!   the safeword hush is active: no picks, no bids. System interactions
//!   don't count as attention either (they don't reset the score).
//!
//! Nothing here touches the memory DB: the presence profile is passed in.
//! The level is not persisted; a restart starts calm.

use crate::bag::ShuffleBag;
use crate::rng::Rng;
use serde::{Deserialize, Serialize};

/// Highest loneliness level / pose severity.
pub const MAX_LEVEL: u8 = 3;

/// Score at which each level 1..=3 starts.
pub const THRESHOLDS: [f32; 3] = [0.25, 0.5, 0.75];

/// `[behavior.loneliness] mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LonelinessMode {
    /// Score, level-weighted pose picks and attention bids (default).
    #[default]
    Auto,
    /// Off: level 0 always, no bids; the plain mood pose cycle.
    Off,
}

impl LonelinessMode {
    pub fn key(self) -> &'static str {
        match self {
            LonelinessMode::Auto => "auto",
            LonelinessMode::Off => "off",
        }
    }
}

/// `[behavior.loneliness]` in config.toml.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LonelinessConfig {
    /// `auto` (default) or `off`. Right-click ▸ "Lonely poses" flips it.
    pub mode: LonelinessMode,
    /// Highest pose severity she may use, 0..=3 (3 = most explicit).
    pub max_severity: u8,
    /// Minutes of neglect (at a 1× habit factor) from 0 to full loneliness.
    pub minutes_to_max: f32,
    /// Minimum minutes between two attention bids.
    pub bid_cooldown_min: f32,
    /// Minutes after the safeword during which she stays calm.
    pub hush_hold_min: f32,
    /// Local hours `[start, end)` when loneliness neither rises nor bids
    /// (e.g. `[23, 7]` wraps midnight). Empty = none.
    pub quiet_hours: Vec<u8>,
    /// Score margin below a threshold before the level drops.
    pub hysteresis: f32,
}

impl Default for LonelinessConfig {
    fn default() -> Self {
        Self {
            mode: LonelinessMode::Auto,
            max_severity: MAX_LEVEL,
            minutes_to_max: 120.0,
            bid_cooldown_min: 10.0,
            hush_hold_min: 10.0,
            quiet_hours: Vec::new(),
            hysteresis: 0.08,
        }
    }
}

impl LonelinessConfig {
    /// `max_severity` clamped to 0..=[`MAX_LEVEL`].
    pub fn cap(&self) -> u8 {
        self.max_severity.min(MAX_LEVEL)
    }
}

/// One pose the picker may draw (from the clip manifest's `severity` /
/// `system` fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoseTag {
    pub name: String,
    /// 0 (calm) ..= 3 (most explicit).
    pub severity: u8,
    /// Used for system interactions (event reactions, contract signing):
    /// never drawn by the picker.
    pub system: bool,
}

impl PoseTag {
    pub fn new(name: impl Into<String>, severity: u8, system: bool) -> Self {
        Self {
            name: name.into(),
            severity,
            system,
        }
    }
}

/// How lonely-poses state blocks picks right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    Off,
    SystemInteraction,
    Hush,
}

/// Rise multiplier for local `hour` from Ko's presence profile (share of
/// his logged events per 3-hour bucket, any scale; see
/// `lillith_memory::habits::presence_profile`). Bucket share relative to
/// the mean, mapped to `0.5..=1.5`. All zero = 1.
pub fn habit_factor(presence: &[f32; 8], hour: u32) -> f32 {
    let total: f32 = presence.iter().filter(|v| v.is_finite()).sum();
    if total <= 0.0 || !total.is_finite() {
        return 1.0;
    }
    let p = presence[((hour % 24) / 3) as usize];
    let rel = if p.is_finite() { p * 8.0 / total } else { 1.0 };
    (0.5 + 0.5 * rel).clamp(0.5, 1.5)
}

/// Whether `hour` is inside `quiet` (`[start, end)`, wrapping midnight).
pub fn in_quiet_hours(quiet: &[u8], hour: u32) -> bool {
    let [s, e] = match quiet {
        [s, e] => [*s as u32 % 24, *e as u32 % 24],
        _ => return false,
    };
    let h = hour % 24;
    match s.cmp(&e) {
        std::cmp::Ordering::Less => (s..e).contains(&h),
        std::cmp::Ordering::Greater => h >= s || h < e,
        std::cmp::Ordering::Equal => false,
    }
}

/// Easing from neglect to score: smoothstep `x²(3 − 2x)` on `0..=1`.
pub fn ease(x: f32) -> f32 {
    let x = if x.is_finite() {
        x.clamp(0.0, 1.0)
    } else {
        0.0
    };
    x * x * (3.0 - 2.0 * x)
}

/// Level for `score` given the previous level (hysteresis: rising needs
/// the threshold, falling needs `hysteresis` below it).
pub fn level_for(score: f32, prev: u8, hysteresis: f32) -> u8 {
    let mut up = 0u8;
    for (i, t) in THRESHOLDS.iter().enumerate() {
        if score >= *t {
            up = i as u8 + 1;
        }
    }
    if up >= prev {
        return up;
    }
    // Falling: keep each level above `up` while still within the margin.
    let mut lvl = prev.min(MAX_LEVEL);
    while lvl > up && score < THRESHOLDS[lvl as usize - 1] - hysteresis.max(0.0) {
        lvl -= 1;
    }
    lvl
}

/// Shuffle weight of a pose of `severity` at `level` under `cap`:
/// 0 = never (system, or above `min(level, cap)`).
pub fn pick_weight(tag: &PoseTag, level: u8, cap: u8) -> u32 {
    let top = level.min(cap).min(MAX_LEVEL);
    if tag.system || tag.severity > top {
        return 0;
    }
    match top - tag.severity {
        0 => 6,
        1 => 3,
        _ => 1,
    }
}

/// The loneliness engine. See the module docs.
#[derive(Debug, Clone)]
pub struct Loneliness {
    cfg: LonelinessConfig,
    enabled: bool,
    t: f32,
    neglect: f32,
    score: f32,
    level: u8,
    hour: u32,
    presence: [f32; 8],
    system_busy: bool,
    hush_at: Option<f32>,
    last_bid: Option<f32>,
    pool: Vec<PoseTag>,
    bag: ShuffleBag,
    bag_key: Option<(u8, u8)>,
    rng: Rng,
}

impl Loneliness {
    pub fn new(cfg: LonelinessConfig, seed: u64) -> Self {
        Self {
            enabled: cfg.mode == LonelinessMode::Auto,
            cfg,
            t: 0.0,
            neglect: 0.0,
            score: 0.0,
            level: 0,
            hour: 12,
            presence: [0.0; 8],
            system_busy: false,
            hush_at: None,
            last_bid: None,
            pool: Vec::new(),
            bag: ShuffleBag::default(),
            bag_key: None,
            rng: Rng::new(seed ^ 0x10_4E11_7E55),
        }
    }

    pub fn config(&self) -> &LonelinessConfig {
        &self.cfg
    }

    /// Replaces the config (live reload); keeps the score.
    pub fn set_config(&mut self, cfg: LonelinessConfig) {
        self.enabled = cfg.mode == LonelinessMode::Auto;
        self.cfg = cfg;
        self.bag_key = None;
    }

    /// On (`auto`) / off (right-click toggle). Off also calms her.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.neglect = 0.0;
            self.score = 0.0;
            self.level = 0;
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The poses she may pick from (manifest `severity` / `system`).
    pub fn set_pool(&mut self, pool: Vec<PoseTag>) {
        self.pool = pool;
        self.bag_key = None;
    }

    pub fn pool(&self) -> &[PoseTag] {
        &self.pool
    }

    /// Ko's presence per 3-hour bucket (any scale, read from memory).
    pub fn set_presence(&mut self, presence: [f32; 8]) {
        self.presence = presence;
    }

    /// Local hour of day now (the overlay updates it).
    pub fn set_hour(&mut self, hour: u32) {
        self.hour = hour % 24;
    }

    /// A system interaction (card, settings, wizard, dialog, console,
    /// install) is open.
    pub fn set_system_busy(&mut self, busy: bool) {
        self.system_busy = busy;
    }

    /// The safeword was said: calm for `hush_hold_min`.
    pub fn note_hush(&mut self) {
        self.hush_at = Some(self.t);
    }

    /// Ko really interacted with her: she's not lonely any more.
    pub fn attend(&mut self) {
        self.neglect = 0.0;
        self.score = 0.0;
        self.level = 0;
    }

    /// Eased loneliness score, `0.0..=1.0`.
    pub fn score(&self) -> f32 {
        self.score
    }

    /// Raw (linear) neglect, `0.0..=1.0`.
    pub fn neglect(&self) -> f32 {
        self.neglect
    }

    /// Current level, capped by `max_severity`; 0 when off.
    pub fn level(&self) -> u8 {
        if self.enabled {
            self.level.min(self.cfg.cap())
        } else {
            0
        }
    }

    pub fn hushed(&self) -> bool {
        self.hush_at
            .is_some_and(|h| self.t - h < self.cfg.hush_hold_min.max(0.0) * 60.0)
    }

    /// Why picks / bids are blocked now, if they are.
    pub fn blocked(&self) -> Option<Block> {
        if !self.enabled {
            Some(Block::Off)
        } else if self.system_busy {
            Some(Block::SystemInteraction)
        } else if self.hushed() {
            Some(Block::Hush)
        } else {
            None
        }
    }

    fn quiet(&self) -> bool {
        in_quiet_hours(&self.cfg.quiet_hours, self.hour)
    }

    /// Advances the clock by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.t += dt;
        if !self.enabled || self.system_busy || self.quiet() {
            return;
        }
        let span = (self.cfg.minutes_to_max.max(0.1)) * 60.0;
        let f = habit_factor(&self.presence, self.hour);
        self.neglect = (self.neglect + dt / span * f).clamp(0.0, 1.0);
        self.score = ease(self.neglect);
        self.level = level_for(self.score, self.level, self.cfg.hysteresis);
    }

    /// Whether to make an attention bid now (and records it): lonely
    /// (level ≥ 1), not blocked, not quiet hours, cooldown over.
    pub fn want_bid(&mut self) -> bool {
        if self.blocked().is_some() || self.level() == 0 || self.quiet() {
            return false;
        }
        let cool = self.cfg.bid_cooldown_min.max(0.0) * 60.0;
        if self.last_bid.is_some_and(|b| self.t - b < cool) {
            return false;
        }
        self.last_bid = Some(self.t);
        true
    }

    /// Next pose for the idle pose cycle at the current level, among the
    /// pool entries `has` accepts (loaded, not hidden). `None` = use the
    /// calm mood cycle (off, level 0, blocked, or nothing tagged).
    pub fn pick(&mut self, has: impl Fn(&str) -> bool) -> Option<String> {
        if self.blocked().is_some() {
            return None;
        }
        let level = self.level();
        if level == 0 {
            return None;
        }
        let cap = self.cfg.cap();
        if self.bag_key != Some((level, cap)) {
            let last = self.bag.last().map(str::to_string);
            self.bag = ShuffleBag::new(
                self.pool
                    .iter()
                    .filter(|p| has(&p.name))
                    .map(|p| (p.name.clone(), pick_weight(p, level, cap))),
            );
            if let Some(l) = last {
                self.bag.set_last(&l);
            }
            self.bag_key = Some((level, cap));
        }
        self.bag.next(&mut self.rng).map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool() -> Vec<PoseTag> {
        let mut v = Vec::new();
        for s in 0..=3u8 {
            for i in 0..3 {
                v.push(PoseTag::new(format!("s{s}_{i}"), s, false));
            }
        }
        v.push(PoseTag::new("event_wifi", 0, true));
        v.push(PoseTag::new("soul_contract_sign", 2, true));
        v
    }

    fn sev(name: &str) -> u8 {
        name.as_bytes()[1] - b'0'
    }

    fn eng(cfg: LonelinessConfig) -> Loneliness {
        let mut l = Loneliness::new(cfg, 7);
        l.set_pool(pool());
        l
    }

    fn run_min(l: &mut Loneliness, min: f32) {
        for _ in 0..(min * 6.0) as usize {
            l.tick(10.0);
        }
    }

    #[test]
    fn score_rises_with_neglect_and_resets_on_attention() {
        let mut l = eng(LonelinessConfig::default());
        assert_eq!((l.score(), l.level()), (0.0, 0));
        run_min(&mut l, 38.0);
        assert_eq!(l.level(), 0, "eased: 38 min is still calm");
        run_min(&mut l, 3.0);
        assert!((l.neglect() - 41.0 / 120.0).abs() < 0.01, "{}", l.neglect());
        assert!(
            (l.score() - ease(41.0 / 120.0)).abs() < 0.01,
            "{}",
            l.score()
        );
        assert_eq!(l.level(), 1);
        run_min(&mut l, 60.0);
        assert_eq!(l.level(), 3);
        run_min(&mut l, 600.0);
        assert_eq!(l.score(), 1.0, "clamped");
        l.attend();
        assert_eq!((l.score(), l.neglect(), l.level()), (0.0, 0.0, 0));
    }

    #[test]
    fn habits_change_the_rise_rate() {
        // Usually around 18-21h, never 3-6h.
        let mut p = [1.0f32; 8];
        p[6] = 9.0;
        p[1] = 0.0;
        assert!(habit_factor(&p, 19) > 1.2);
        assert!(habit_factor(&p, 4) <= 0.5 + 1e-6);
        assert_eq!(habit_factor(&[0.0; 8], 4), 1.0);
        let mut around = eng(LonelinessConfig::default());
        around.set_presence(p);
        around.set_hour(19);
        let mut away = eng(LonelinessConfig::default());
        away.set_presence(p);
        away.set_hour(4);
        run_min(&mut around, 30.0);
        run_min(&mut away, 30.0);
        assert!(around.score() > 0.3 && away.score() < 0.15);
        assert!(around.level() > away.level());
    }

    #[test]
    fn system_interactions_hold_the_score_and_block_everything() {
        let mut l = eng(LonelinessConfig::default());
        run_min(&mut l, 100.0);
        assert_eq!(l.level(), 3);
        l.set_system_busy(true);
        let s = l.score();
        run_min(&mut l, 30.0);
        assert_eq!(l.score(), s, "no rise while a card/settings is open");
        assert_eq!(l.level(), 3, "and no reset: it isn't attention");
        assert_eq!(l.blocked(), Some(Block::SystemInteraction));
        for _ in 0..50 {
            assert!(!l.want_bid());
            assert_eq!(l.pick(|_| true), None);
        }
        l.set_system_busy(false);
        assert!(l.want_bid());
        assert!(l.pick(|_| true).is_some());
    }

    #[test]
    fn hush_blocks_bids_and_picks_for_the_hold() {
        let mut l = eng(LonelinessConfig::default());
        run_min(&mut l, 100.0);
        l.note_hush();
        assert_eq!(l.blocked(), Some(Block::Hush));
        assert!(!l.want_bid());
        assert_eq!(l.pick(|_| true), None);
        run_min(&mut l, 9.0);
        assert!(!l.want_bid());
        run_min(&mut l, 2.0);
        assert_eq!(l.blocked(), None);
        assert!(l.want_bid());
    }

    #[test]
    fn easing_is_smooth_monotonic_and_bounded() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert!((ease(0.5) - 0.5).abs() < 1e-6);
        assert_eq!(ease(-3.0), 0.0);
        assert_eq!(ease(7.0), 1.0);
        assert_eq!(ease(f32::NAN), 0.0);
        let mut prev = 0.0;
        for i in 1..=100 {
            let e = ease(i as f32 / 100.0);
            assert!(e >= prev, "monotonic");
            prev = e;
        }
        // Slow start, slow finish: the ends move less than the middle.
        assert!(ease(0.1) < 0.1 && ease(0.9) > 0.9);
    }

    #[test]
    fn hysteresis_stops_flicker() {
        assert_eq!(level_for(0.25, 0, 0.08), 1);
        assert_eq!(level_for(0.24, 1, 0.08), 1, "just under: stays");
        assert_eq!(level_for(0.18, 1, 0.08), 1);
        assert_eq!(level_for(0.16, 1, 0.08), 0);
        assert_eq!(level_for(0.49, 3, 0.08), 2);
        assert_eq!(level_for(0.70, 3, 0.08), 3);
        assert_eq!(level_for(0.1, 3, 0.08), 0);
        // Oscillating around a threshold never flips the level back down.
        let mut lvl = 0;
        let mut flips = 0;
        for i in 0..100 {
            let s = 0.5 + if i % 2 == 0 { 0.01 } else { -0.01 };
            let n = level_for(s, lvl, 0.08);
            if n != lvl {
                flips += 1;
            }
            lvl = n;
        }
        assert_eq!(flips, 1);
    }

    #[test]
    fn cap_always_holds_and_lower_severities_still_appear() {
        for cap in 0..=3u8 {
            let mut l = eng(LonelinessConfig {
                max_severity: cap,
                ..Default::default()
            });
            run_min(&mut l, 200.0);
            assert_eq!(l.level(), cap);
            let mut seen = [0u32; 4];
            for _ in 0..600 {
                match l.pick(|_| true) {
                    Some(c) => {
                        assert!(!c.starts_with("event") && c != "soul_contract_sign");
                        let s = sev(&c);
                        assert!(s <= cap, "cap {cap}: {c}");
                        seen[s as usize] += 1;
                    }
                    None => assert_eq!(cap, 0, "only cap 0 means calm"),
                }
            }
            if cap == 3 {
                assert!(seen[3] > seen[2] && seen[2] > seen[0], "{seen:?}");
                assert!(seen[0] > 0 && seen[1] > 0, "lower ones still: {seen:?}");
            }
        }
        // A config value above 3 is clamped.
        let c = LonelinessConfig {
            max_severity: 9,
            ..Default::default()
        };
        assert_eq!(c.cap(), 3);
    }

    #[test]
    fn picks_follow_the_level() {
        let mut l = eng(LonelinessConfig::default());
        assert_eq!(l.pick(|_| true), None, "level 0: calm mood cycle");
        run_min(&mut l, 42.0);
        assert_eq!(l.level(), 1);
        for _ in 0..200 {
            assert!(sev(&l.pick(|_| true).unwrap()) <= 1);
        }
        // Clips the brain lacks / hides are never picked.
        let mut l2 = eng(LonelinessConfig::default());
        run_min(&mut l2, 200.0);
        for _ in 0..200 {
            let c = l2.pick(|n| !n.starts_with("s3")).unwrap();
            assert!(!c.starts_with("s3"));
        }
    }

    #[test]
    fn bid_cooldown() {
        let mut l = eng(LonelinessConfig::default());
        assert!(!l.want_bid(), "not lonely yet");
        run_min(&mut l, 41.0);
        assert!(l.want_bid());
        assert!(!l.want_bid());
        run_min(&mut l, 9.0);
        assert!(!l.want_bid());
        run_min(&mut l, 1.5);
        assert!(l.want_bid());
    }

    #[test]
    fn off_and_quiet_hours() {
        let mut l = eng(LonelinessConfig {
            mode: LonelinessMode::Off,
            ..Default::default()
        });
        run_min(&mut l, 300.0);
        assert_eq!((l.level(), l.score()), (0, 0.0));
        assert!(!l.want_bid() && l.pick(|_| true).is_none());
        l.set_enabled(true);
        run_min(&mut l, 100.0);
        assert_eq!(l.level(), 3);
        l.set_enabled(false);
        assert_eq!(l.level(), 0);

        assert!(in_quiet_hours(&[23, 7], 2) && in_quiet_hours(&[23, 7], 23));
        assert!(!in_quiet_hours(&[23, 7], 7) && !in_quiet_hours(&[23, 7], 12));
        assert!(in_quiet_hours(&[9, 17], 12) && !in_quiet_hours(&[9, 17], 17));
        assert!(!in_quiet_hours(&[], 3) && !in_quiet_hours(&[5, 5], 5));
        let mut q = eng(LonelinessConfig {
            quiet_hours: vec![23, 7],
            ..Default::default()
        });
        q.set_hour(2);
        run_min(&mut q, 300.0);
        assert_eq!(q.score(), 0.0);
        q.set_hour(12);
        run_min(&mut q, 100.0);
        q.set_hour(2);
        assert!(!q.want_bid(), "no bids in quiet hours");
    }

    #[test]
    fn config_round_trips() {
        let c: LonelinessConfig =
            toml::from_str("mode = \"off\"\nmax_severity = 2\nquiet_hours = [23, 7]").unwrap();
        assert_eq!(c.mode, LonelinessMode::Off);
        assert_eq!(c.cap(), 2);
        assert_eq!(c.minutes_to_max, 120.0);
        let d = LonelinessConfig::default();
        assert_eq!(d.mode, LonelinessMode::Auto);
        assert_eq!(d.cap(), 3);
    }

    /// The idle / attention path is pure state: no storage at all, so it
    /// can never wipe, recreate or migrate Ko's memory DB.
    #[test]
    fn idle_path_never_touches_the_memory_db() {
        let manifest = include_str!("../Cargo.toml");
        for dep in [["rus", "qlite"], ["lillith-", "memory"]] {
            assert!(!manifest.contains(&dep.concat()), "{dep:?}");
        }
        let src = include_str!("loneliness.rs");
        for bad in [
            ["std::", "fs"],
            ["rus", "qlite"],
            ["File", "::"],
            ["remove_", "file"],
            ["DROP ", "TABLE"],
        ] {
            assert!(!src.contains(&bad.concat()), "{bad:?}");
        }
        let mut l = Loneliness::new(LonelinessConfig::default(), 1);
        l.set_pool(vec![PoseTag::new("a", 1, false)]);
        for _ in 0..10_000 {
            l.tick(1.0);
            let _ = l.want_bid();
            let _ = l.pick(|_| true);
        }
        assert_eq!(l.level(), 3);
    }
}
