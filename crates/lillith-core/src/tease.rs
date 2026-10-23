//! "Come closer" tease (`[behavior.tease]`).
//!
//! Every few minutes of calm idle she comes up to the viewer, slow and
//! unhurried:
//!
//! 1. **Approach**: she sways toward the screen (approach clip, e.g.
//!    `idle_flirt_hands_behind_sway`) while her render scale eases up from
//!    her normal size (the overlay default is 0.55) to `peak_scale` × that.
//! 2. **Beat**: the flirty close-up beat at full size (`tease_closeup`:
//!    arms squeeze in, lean toward the viewer, teasing smile).
//! 3. **Pause**: a held smile / wink (`blink_wink`) at full size.
//! 4. **Retreat**: she sways back while the scale eases down to exactly
//!    her normal size, then normal idle resumes.
//!
//! The scale curve is a smoothstep (zero slope at both ends), so the growth
//! starts and lands gently. The overlay keeps her bottom-anchored, centred
//! on the same spot and fully on screen ([`fit_peak`], [`keep_on_screen`]).
//! A click makes her back off early (smoothly, from wherever she is); a drag,
//! resize or minimize cancels straight back to her normal size.
//!
//! Pure and clock-driven like [`crate::idle`]: [`Envelope`] is the scale
//! curve, [`Tease`] the scheduler the overlay ticks. Only clips that exist
//! in the loaded set are used ([`TeaseConfig::plan`]); art is never changed.

use crate::rng::Rng;
use serde::{Deserialize, Serialize};

/// `[behavior.tease]` tunables. Slow and unhurried by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TeaseConfig {
    /// `false` turns the tease off.
    pub enabled: bool,
    /// Calm idle time between two teases, `[min, max]` s (re-picked each
    /// time). Only time she spends idle on her own counts.
    pub every_s: (f32, f32),
    /// Approach: scale eases 1.0 → `peak_scale` over this long (s).
    pub grow_s: f32,
    /// Flirty beat at the peak (s). The overlay stretches it to the beat
    /// clip's length so the clip always plays to its end.
    pub beat_s: f32,
    /// Held smile / pause at the peak after the beat (s).
    pub pause_s: f32,
    /// Retreat: scale eases `peak_scale` → 1.0 over this long (s).
    pub shrink_s: f32,
    /// Peak render-scale factor on top of her normal size (1.6 = 160 %,
    /// i.e. 0.55 → 0.88 at the default size). Lowered automatically if she
    /// would not fit on screen.
    pub peak_scale: f32,
    /// Looping clip(s) for the approach and the retreat, first that exists
    /// wins (none: she just stands while growing).
    pub approach_clips: Vec<String>,
    /// Clip(s) for the flirty beat, first that exists wins. Required: with
    /// none of them in the set the tease never starts.
    pub beat_clips: Vec<String>,
    /// Looping clip(s) for the held smile, first that exists wins (none:
    /// she stands on the plain idle).
    pub pause_clips: Vec<String>,
}

impl Default for TeaseConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            every_s: (180.0, 360.0),
            grow_s: 3.0,
            beat_s: 3.0,
            pause_s: 2.0,
            shrink_s: 3.0,
            peak_scale: 1.6,
            approach_clips: strings(&["idle_flirt_hands_behind_sway", "hip_sway"]),
            beat_clips: strings(&[
                "tease_closeup",
                "bend_forward_wink",
                "lean_tease",
                "idle_flirt_lip_bite",
                "hip_pop_wink",
            ]),
            pause_clips: strings(&["blink_wink"]),
        }
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Largest peak factor the overlay accepts (sanity cap for config typos).
pub const MAX_PEAK: f32 = 3.0;

/// The clips one tease plays (all checked to exist).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeasePlan {
    /// Base clip while approaching / retreating (`None` = stand).
    pub approach: Option<String>,
    /// The flirty beat (played once).
    pub beat: String,
    /// Base clip for the held smile (`None` = stand).
    pub pause: Option<String>,
}

fn first_of(list: &[String], has_clip: &impl Fn(&str) -> bool) -> Option<String> {
    list.iter().find(|c| !c.is_empty() && has_clip(c)).cloned()
}

impl TeaseConfig {
    /// The clips to use, from what `has_clip` knows; `None` if no beat clip
    /// exists (then there is no tease).
    pub fn plan(&self, has_clip: impl Fn(&str) -> bool) -> Option<TeasePlan> {
        let beat = first_of(&self.beat_clips, &has_clip)?;
        Some(TeasePlan {
            approach: first_of(&self.approach_clips, &has_clip),
            beat,
            pause: first_of(&self.pause_clips, &has_clip),
        })
    }

    /// The scale curve for these timings with peak `peak` (already fitted
    /// to the screen) and a beat at least `beat_min_s` long (the beat
    /// clip's length; 0 = just `beat_s`).
    pub fn envelope(&self, peak: f32, beat_min_s: f32) -> Envelope {
        let beat = if beat_min_s.is_finite() {
            self.beat_s.max(beat_min_s)
        } else {
            self.beat_s
        };
        Envelope::new(self.grow_s, beat, self.pause_s, self.shrink_s, peak)
    }
}

/// Smoothstep `3t² − 2t³` on `[0, 1]` (clamped): monotonic, zero slope at
/// both ends, and exactly 0 / 1 at the endpoints.
pub fn ease_in_out(t: f32) -> f32 {
    let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
    t * t * (3.0 - 2.0 * t)
}

/// Which part of the tease is playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeasePhase {
    /// Walking up, scale easing toward the peak.
    Approach,
    /// The flirty close-up beat at the peak.
    Beat,
    /// Held smile at the peak.
    Pause,
    /// Backing off, scale easing back to 1.0.
    Retreat,
}

/// Scale factor over time: ease up, beat, pause, ease down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Envelope {
    pub grow_s: f32,
    pub beat_s: f32,
    pub pause_s: f32,
    pub shrink_s: f32,
    pub peak: f32,
}

/// Shortest ramp (s), so a zero in the config cannot divide by zero.
const MIN_RAMP_S: f32 = 1e-3;

fn sane(x: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if x.is_finite() {
        x.clamp(lo, hi)
    } else {
        fallback
    }
}

impl Envelope {
    pub fn new(grow_s: f32, beat_s: f32, pause_s: f32, shrink_s: f32, peak: f32) -> Self {
        Self {
            grow_s: sane(grow_s, MIN_RAMP_S, 60.0, 3.0),
            beat_s: sane(beat_s, 0.0, 120.0, 3.0),
            pause_s: sane(pause_s, 0.0, 120.0, 2.0),
            shrink_s: sane(shrink_s, MIN_RAMP_S, 60.0, 3.0),
            peak: sane(peak, 1.0, MAX_PEAK, 1.0),
        }
    }

    /// When the retreat starts (s).
    pub fn retreat_at(&self) -> f32 {
        self.grow_s + self.beat_s + self.pause_s
    }

    /// Whole tease length (s).
    pub fn duration(&self) -> f32 {
        self.retreat_at() + self.shrink_s
    }

    /// Phase at `t` s (`None` once it is over). Zero-length phases are
    /// skipped.
    pub fn phase(&self, t: f32) -> Option<TeasePhase> {
        if t < self.grow_s {
            Some(TeasePhase::Approach)
        } else if t < self.grow_s + self.beat_s {
            Some(TeasePhase::Beat)
        } else if t < self.retreat_at() {
            Some(TeasePhase::Pause)
        } else if t < self.duration() {
            Some(TeasePhase::Retreat)
        } else {
            None
        }
    }

    /// Scale factor at `t` s: 1.0 at and before the start, `peak` through
    /// the beat and the pause, exactly 1.0 again from the end on.
    pub fn factor(&self, t: f32) -> f32 {
        if t.is_nan() || t <= 0.0 {
            return 1.0;
        }
        let span = self.peak - 1.0;
        match self.phase(t) {
            Some(TeasePhase::Approach) => 1.0 + span * ease_in_out(t / self.grow_s),
            Some(TeasePhase::Beat | TeasePhase::Pause) => self.peak,
            Some(TeasePhase::Retreat) => {
                let u = (t - self.retreat_at()) / self.shrink_s;
                1.0 + span * ease_in_out(1.0 - u)
            }
            None => 1.0,
        }
    }
}

/// Lowers `peak` so a window of `base` px (at factor 1.0) still fits
/// `screen` when grown: the width within the screen width, the height
/// within the room above her bottom margin `bottom`. Never below 1.0 (she
/// is already that size); without a known screen `peak` is kept.
pub fn fit_peak(peak: f32, base: (u32, u32), screen: Option<(u32, u32)>, bottom: i32) -> f32 {
    let peak = sane(peak, 1.0, MAX_PEAK, 1.0);
    let Some((sw, sh)) = screen else {
        return peak;
    };
    let (bw, bh) = (base.0.max(1) as f32, base.1.max(1) as f32);
    let room_h = (sh as f32 - bottom.max(0) as f32).max(0.0);
    let fit = (sw as f32 / bw).min(room_h / bh);
    peak.min(fit).max(1.0)
}

/// Left edge that keeps a `w` px window on a `screen_w` px screen
/// (pinned to 0 if it is wider than the screen).
pub fn keep_on_screen(left: i32, w: u32, screen_w: u32) -> i32 {
    let max = (screen_w as i64 - w as i64).max(0) as i32;
    left.clamp(0, max)
}

/// What the scheduler tells the overlay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TeaseCue {
    /// Enough calm idle: start a tease now ([`Tease::begin`]).
    Due,
    /// The tease entered this phase.
    Phase(TeasePhase),
    /// The tease finished: scale is back to exactly 1.0.
    Done,
}

/// A running tease.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Run {
    env: Envelope,
    /// Elapsed s.
    t: f32,
    /// Last reported phase.
    phase: TeasePhase,
    /// Early retreat: `(t it started, factor it started from)`.
    early: Option<(f32, f32)>,
}

impl Run {
    fn factor(&self) -> f32 {
        match self.early {
            Some((t0, from)) => {
                let u = (self.t - t0) / self.env.shrink_s;
                if u >= 1.0 {
                    1.0
                } else {
                    1.0 + (from - 1.0) * ease_in_out(1.0 - u)
                }
            }
            None => self.env.factor(self.t),
        }
    }

    fn phase_now(&self) -> Option<TeasePhase> {
        match self.early {
            Some((t0, _)) => (self.t - t0 < self.env.shrink_s).then_some(TeasePhase::Retreat),
            None => self.env.phase(self.t),
        }
    }
}

/// The tease scheduler: counts calm idle time down to the next tease, then
/// runs one [`Envelope`].
#[derive(Debug, Clone)]
pub struct Tease {
    cfg: TeaseConfig,
    rng: Rng,
    /// Calm idle seconds left before the next tease.
    wait_s: f32,
    active: Option<Run>,
}

impl Tease {
    pub fn new(cfg: TeaseConfig, seed: u64) -> Self {
        let mut t = Self {
            cfg,
            rng: Rng::new(seed ^ 0x7EA5_E000),
            wait_s: 0.0,
            active: None,
        };
        t.rearm();
        t
    }

    pub fn config(&self) -> &TeaseConfig {
        &self.cfg
    }

    /// New tunables (live settings); a running tease keeps its curve.
    pub fn set_config(&mut self, cfg: TeaseConfig) {
        if cfg != self.cfg {
            self.cfg = cfg;
            self.rearm();
        }
    }

    /// Picks the next wait from `every_s`.
    fn rearm(&mut self) {
        let (a, b) = self.cfg.every_s;
        let (a, b) = (sane(a, 1.0, 1e6, 180.0), sane(b, 1.0, 1e6, 360.0));
        self.wait_s = self.rng.range(a.min(b), a.max(b));
    }

    /// Calm idle seconds left before the next tease is due.
    pub fn wait_s(&self) -> f32 {
        self.wait_s
    }

    pub fn active(&self) -> bool {
        self.active.is_some()
    }

    /// Current phase (`None` when not teasing).
    pub fn phase(&self) -> Option<TeasePhase> {
        self.active.map(|a| a.phase)
    }

    /// Current scale factor (1.0 when not teasing).
    pub fn factor(&self) -> f32 {
        self.active.map_or(1.0, |r| r.factor())
    }

    /// Starts a tease with `peak` (fitted to the screen) and a beat of at
    /// least `beat_min_s`.
    pub fn begin(&mut self, peak: f32, beat_min_s: f32) -> TeaseCue {
        let env = self.cfg.envelope(peak, beat_min_s);
        self.active = Some(Run {
            env,
            t: 0.0,
            phase: TeasePhase::Approach,
            early: None,
        });
        TeaseCue::Phase(TeasePhase::Approach)
    }

    /// Ends a tease at once (drag, resize, minimize): scale snaps to 1.0.
    /// Returns `true` if one was running.
    pub fn cancel(&mut self) -> bool {
        let was = self.active.take().is_some();
        if was {
            self.rearm();
        }
        was
    }

    /// Backs off now (a click): eases from the current factor back to 1.0
    /// over `shrink_s`. Returns the cue if the phase changed.
    pub fn retreat_now(&mut self) -> Option<TeaseCue> {
        let r = self.active.as_mut()?;
        if r.phase == TeasePhase::Retreat {
            return None;
        }
        let from = r.factor();
        r.early = Some((r.t, from));
        r.phase = TeasePhase::Retreat;
        Some(TeaseCue::Phase(TeasePhase::Retreat))
    }

    /// Skips the pending tease (she could not start one now, e.g. no clip)
    /// and waits a fresh `every_s`.
    pub fn postpone(&mut self) {
        self.active = None;
        self.rearm();
    }

    /// Advances by `dt`. While waiting only `calm` time counts (idle on
    /// her own: not walking, dragged, hovered, talking, asleep, …). A
    /// running tease advances regardless.
    pub fn tick(&mut self, dt: f32, calm: bool) -> Option<TeaseCue> {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        if let Some(r) = &mut self.active {
            r.t += dt;
            return match r.phase_now() {
                None => {
                    self.active = None;
                    self.rearm();
                    Some(TeaseCue::Done)
                }
                Some(p) if p != r.phase => {
                    r.phase = p;
                    Some(TeaseCue::Phase(p))
                }
                Some(_) => None,
            };
        }
        if !self.cfg.enabled || !calm {
            return None;
        }
        self.wait_s -= dt;
        (self.wait_s <= 0.0).then_some(TeaseCue::Due)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The overlay's default size (config `scale = 1.0` → 0.55).
    const DEFAULT_SCALE: f32 = 0.55;

    fn env() -> Envelope {
        TeaseConfig::default().envelope(1.6, 0.0)
    }

    #[test]
    fn defaults_are_slow_and_use_the_flirty_clips() {
        let c = TeaseConfig::default();
        assert!(c.enabled);
        assert_eq!(c.every_s, (180.0, 360.0));
        assert_eq!(
            (c.grow_s, c.beat_s, c.pause_s, c.shrink_s),
            (3.0, 3.0, 2.0, 3.0)
        );
        assert_eq!(c.peak_scale, 1.6);
        assert_eq!(c.beat_clips[0], "tease_closeup");
        assert_eq!(c.approach_clips[0], "idle_flirt_hands_behind_sway");
        assert_eq!(c.pause_clips, ["blink_wink"]);
    }

    #[test]
    fn ease_is_monotonic_with_exact_endpoints() {
        assert_eq!(ease_in_out(0.0), 0.0);
        assert_eq!(ease_in_out(1.0), 1.0);
        assert_eq!(ease_in_out(-3.0), 0.0);
        assert_eq!(ease_in_out(7.0), 1.0);
        assert_eq!(ease_in_out(f32::NAN), 0.0);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-6);
        let mut prev = 0.0;
        for i in 1..=1000 {
            let v = ease_in_out(i as f32 / 1000.0);
            assert!(v >= prev, "not monotonic at {i}");
            prev = v;
        }
        // Gentle at both ends (zero slope): the first and last 1 % move <0.1 %.
        assert!(ease_in_out(0.01) < 1e-3);
        assert!(1.0 - ease_in_out(0.99) < 1e-3);
    }

    #[test]
    fn envelope_endpoints_are_exact() {
        let e = env();
        assert_eq!(e.duration(), 11.0);
        assert_eq!(e.factor(-1.0), 1.0);
        assert_eq!(e.factor(0.0), 1.0);
        assert_eq!(e.factor(3.0), 1.6, "peak reached exactly at grow_s");
        assert_eq!(e.factor(7.99), 1.6, "still at peak through the pause");
        assert_eq!(e.factor(e.duration()), 1.0);
        assert_eq!(e.factor(e.duration() + 10.0), 1.0);
        // On top of the default size: 0.55 → 0.88 → exactly 0.55 again.
        assert_eq!(DEFAULT_SCALE * e.factor(0.0), DEFAULT_SCALE);
        assert!((DEFAULT_SCALE * e.factor(4.0) - 0.88).abs() < 1e-6);
        assert_eq!(DEFAULT_SCALE * e.factor(e.duration()), DEFAULT_SCALE);
    }

    #[test]
    fn envelope_phases_in_order() {
        let e = env();
        assert_eq!(e.phase(0.5), Some(TeasePhase::Approach));
        assert_eq!(e.phase(3.0), Some(TeasePhase::Beat));
        assert_eq!(e.phase(6.5), Some(TeasePhase::Pause));
        assert_eq!(e.phase(8.0), Some(TeasePhase::Retreat));
        assert_eq!(e.phase(11.0), None);
        // The beat is stretched to the clip, never cut.
        let long = TeaseConfig::default().envelope(1.6, 4.5);
        assert_eq!(long.beat_s, 4.5);
        assert_eq!(TeaseConfig::default().envelope(1.6, 1.0).beat_s, 3.0);
        assert_eq!(TeaseConfig::default().envelope(1.6, f32::NAN).beat_s, 3.0);
    }

    #[test]
    fn envelope_eases_up_holds_and_eases_down_smoothly() {
        let e = env();
        let dt = 1.0 / 120.0;
        let n = (e.duration() / dt).ceil() as usize + 2;
        let samples: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                let t = i as f32 * dt;
                (t, e.factor(t))
            })
            .collect();
        // Peak slope of smoothstep is 1.5 × span / ramp.
        let max_step = 1.5 * 0.6 / 3.0 * dt + 1e-5;
        for w in samples.windows(2) {
            let ((t0, a), (t1, b)) = (w[0], w[1]);
            match e.phase(t1) {
                Some(TeasePhase::Approach) => assert!(b >= a, "up at {t1}: {a}->{b}"),
                Some(TeasePhase::Beat | TeasePhase::Pause) if t0 >= e.grow_s => {
                    assert_eq!(b, 1.6, "held at {t1}")
                }
                Some(TeasePhase::Retreat) => assert!(b <= a, "down at {t1}: {a}->{b}"),
                _ => {}
            }
            assert!((1.0..=1.6).contains(&b), "{b} at {t1}");
            assert!((b - a).abs() <= max_step, "jump at {t1}: {a}->{b}");
        }
        assert!(
            (e.factor(1.5) - 1.3).abs() < 1e-5,
            "half way up at half time"
        );
        assert!((e.factor(9.5) - 1.3).abs() < 1e-5, "half way down");
    }

    #[test]
    fn envelope_sanitises_bad_config() {
        let e = Envelope::new(0.0, -1.0, f32::NAN, f32::NAN, 99.0);
        assert!(e.grow_s > 0.0 && e.beat_s == 0.0 && e.pause_s == 2.0 && e.shrink_s == 3.0);
        assert_eq!(e.peak, MAX_PEAK);
        assert_eq!(
            Envelope::new(2.0, 3.0, 1.0, 2.0, 0.5).peak,
            1.0,
            "never shrinks"
        );
        assert!(e.factor(0.0005).is_finite());
        assert_eq!(e.factor(f32::NAN), 1.0);
        let flat = Envelope::new(2.0, 3.0, 1.0, 2.0, 1.0);
        assert!((0..90).all(|i| flat.factor(i as f32 * 0.1) == 1.0));
    }

    #[test]
    fn peak_is_clamped_to_stay_on_screen() {
        // 220×220 window at 1.0 on 1920×1080, bottom margin 48: fits 1.6.
        assert_eq!(fit_peak(1.6, (220, 220), Some((1920, 1080)), 48), 1.6);
        // Short screen: (600 − 100) / 400 = 1.25.
        assert!((fit_peak(1.6, (300, 400), Some((1920, 600)), 100) - 1.25).abs() < 1e-6);
        // Narrow screen: 480 / 400 = 1.2.
        assert!((fit_peak(1.6, (400, 300), Some((480, 2000)), 0) - 1.2).abs() < 1e-6);
        // Already as big as the screen: no growth, never below 1.0.
        assert_eq!(fit_peak(1.6, (400, 400), Some((300, 300)), 0), 1.0);
        // Unknown screen: the configured peak.
        assert_eq!(fit_peak(1.6, (400, 400), None, 0), 1.6);
        // Grown size really fits.
        let p = fit_peak(1.6, (300, 400), Some((1920, 600)), 100);
        assert!((400.0 * p).round() as i32 + 100 <= 600);
    }

    #[test]
    fn keep_on_screen_clamps_left() {
        assert_eq!(keep_on_screen(-30, 200, 1000), 0);
        assert_eq!(keep_on_screen(900, 200, 1000), 800);
        assert_eq!(keep_on_screen(400, 200, 1000), 400);
        assert_eq!(keep_on_screen(50, 1200, 1000), 0, "wider than the screen");
    }

    #[test]
    fn plan_uses_only_clips_that_exist() {
        let c = TeaseConfig::default();
        let all = c.plan(|_| true).unwrap();
        assert_eq!(
            all,
            TeasePlan {
                approach: Some("idle_flirt_hands_behind_sway".into()),
                beat: "tease_closeup".into(),
                pause: Some("blink_wink".into()),
            }
        );
        let some = c
            .plan(|n| matches!(n, "hip_sway" | "idle_flirt_lip_bite" | "hip_pop_wink"))
            .unwrap();
        assert_eq!(some.approach.as_deref(), Some("hip_sway"));
        assert_eq!(some.beat, "idle_flirt_lip_bite");
        assert_eq!(some.pause, None);
        // No beat clip at all: no tease.
        assert_eq!(c.plan(|n| n == "hip_sway"), None);
        assert_eq!(c.plan(|_| false), None);
    }

    fn run_to_end(t: &mut Tease, dt: f32, secs: f32) -> (Vec<TeaseCue>, Vec<f32>) {
        let mut cues = Vec::new();
        let mut factors = Vec::new();
        for _ in 0..(secs / dt) as usize {
            // Runs regardless of `calm` once started.
            if let Some(c) = t.tick(dt, false) {
                cues.push(c);
            }
            factors.push(t.factor());
        }
        (cues, factors)
    }

    #[test]
    fn tease_runs_approach_beat_pause_retreat_then_done() {
        let cfg = TeaseConfig {
            every_s: (10.0, 10.0),
            ..TeaseConfig::default()
        };
        let mut t = Tease::new(cfg, 7);
        assert_eq!(t.wait_s(), 10.0);
        // Busy time does not count.
        for _ in 0..100 {
            assert_eq!(t.tick(1.0, false), None);
        }
        for _ in 0..9 {
            assert_eq!(t.tick(1.0, true), None);
        }
        assert_eq!(t.tick(1.0, true), Some(TeaseCue::Due));
        assert_eq!(t.begin(1.6, 0.0), TeaseCue::Phase(TeasePhase::Approach));
        assert!(t.active());
        assert_eq!(t.phase(), Some(TeasePhase::Approach));
        let (cues, factors) = run_to_end(&mut t, 0.05, 12.0);
        assert_eq!(
            cues,
            [
                TeaseCue::Phase(TeasePhase::Beat),
                TeaseCue::Phase(TeasePhase::Pause),
                TeaseCue::Phase(TeasePhase::Retreat),
                TeaseCue::Done
            ]
        );
        assert!(!t.active());
        assert_eq!(t.factor(), 1.0, "exactly back to her normal size");
        assert_eq!(factors.iter().cloned().fold(0.0, f32::max), 1.6);
        assert_eq!(t.wait_s(), 10.0, "re-armed");
    }

    #[test]
    fn click_retreats_smoothly_from_where_she_is() {
        let mut t = Tease::new(TeaseConfig::default(), 3);
        t.begin(1.6, 0.0);
        for _ in 0..30 {
            t.tick(0.05, true);
        }
        let mid = t.factor();
        assert!(mid > 1.0 && mid < 1.6, "{mid}");
        assert_eq!(t.retreat_now(), Some(TeaseCue::Phase(TeasePhase::Retreat)));
        assert_eq!(t.retreat_now(), None, "already retreating");
        assert_eq!(t.factor(), mid, "no jump");
        let (cues, factors) = run_to_end(&mut t, 0.05, 4.0);
        assert_eq!(cues, [TeaseCue::Done]);
        for w in factors.windows(2) {
            assert!(w[1] <= w[0] + 1e-6, "eases down only");
        }
        assert_eq!(t.factor(), 1.0);
        assert!(!t.active());
    }

    #[test]
    fn every_s_is_randomised_within_range() {
        let mut t = Tease::new(TeaseConfig::default(), 1);
        let mut seen = Vec::new();
        for _ in 0..20 {
            let w = t.wait_s();
            assert!((180.0..=360.0).contains(&w), "{w}");
            seen.push(w);
            t.begin(1.6, 0.0);
            t.cancel();
        }
        seen.dedup();
        assert!(seen.len() > 10, "waits vary: {seen:?}");
    }

    #[test]
    fn cancel_snaps_back_to_one() {
        let mut t = Tease::new(TeaseConfig::default(), 3);
        t.begin(1.6, 0.0);
        t.tick(1.0, true);
        assert!(t.factor() > 1.0);
        assert!(t.cancel());
        assert_eq!(t.factor(), 1.0);
        assert!(!t.active());
        assert!(!t.cancel(), "nothing left to cancel");
        assert_eq!(t.retreat_now(), None);
    }

    #[test]
    fn disabled_never_fires() {
        let mut t = Tease::new(
            TeaseConfig {
                enabled: false,
                every_s: (1.0, 1.0),
                ..TeaseConfig::default()
            },
            1,
        );
        assert!((0..1000).all(|_| t.tick(1.0, true).is_none()));
    }

    #[test]
    fn config_parses_from_toml() {
        let c: TeaseConfig = toml::from_str(
            "every_s = [60.0, 90.0]\npeak_scale = 1.4\nbeat_clips = [\"lean_tease\"]\n",
        )
        .unwrap();
        assert_eq!(c.every_s, (60.0, 90.0));
        assert_eq!(c.peak_scale, 1.4);
        assert_eq!(c.beat_clips, ["lean_tease"]);
        assert_eq!(c.pause_s, 2.0, "unset keys keep defaults");
    }
}
