//! Layered jiggle: secondary motion for the chest, hips/butt, pigtails/hair
//! and tail (`config.toml [springs.jiggle]`, Settings ▸ Motion ▸ Jiggle).
//!
//! Every body [`Region`] runs up to [`MAX_LAYERS`] spring layers with
//! different frequency, damping ratio, weight and kick lag:
//!
//! | layer | role      | feel                                              |
//! |-------|-----------|---------------------------------------------------|
//! | 0     | primary   | slow, soft mass; carries most of the motion       |
//! | 1     | secondary | ~1.7× faster, arrives a few tens of ms later      |
//! | 2     | flutter   | fast, light ripple that dies out first            |
//!
//! Their sum overlaps and settles naturally (no single sine wobble). All
//! layers are unconditionally stable for the 120 Hz semi-implicit sub-step
//! (ω·h ≤ 0.25) and never under-damped below ζ = 0.15.
//!
//! What drives them ([`Drive`]): footfalls, landings and impacts, perching,
//! sitting down / standing up, hanging (from a window or the cursor) and its
//! swing, perch bounces, hover, clicks and turns, plus body inertia (any root
//! acceleration, see [`LayeredJiggle::set_body_velocity`]) and breathing.
//!
//! **Build-up** (progressive jiggle): every kick charges a per-region energy
//! that decays over [`ENERGY_TAU_S`]. Later kicks are scaled by
//! `1 + build_up · RAMP_MAX · energy`, so a run of bounces (perch bounce,
//! hop loops, running, a swinging hang) grows livelier step by step instead
//! of repeating the same small wobble, then calms down when she rests.
//!
//! **Caps**: each region's displayed offset goes through
//! [`crate::bounce::soft_limit`] with its own knee/cap ([`Region::caps`]),
//! so even 300 % × 200 % never tears or detaches; springs also carry hard
//! velocity/offset clamps.
//!
//! **Zoom**: offsets are in sprite-native px for the normal full-body
//! framing. Push-in / close-up clips frame her bigger; pass the clip's
//! framing zoom ([`framing_zoom`]) to [`LayeredJiggle::offsets`] so the
//! jiggle (and its caps) scale with the figure.
//!
//! **Baked art**: on clips whose pixels already bounce a region (the
//! `baked_motion` clips), only the share of the gain *above 100 %* is added
//! ([`RegionMode::AboveBaked`]), so the default never doubles the bake;
//! regions a clip's own `disable_springs` names stay off.
//!
//! **Layered art**: most clips are single flattened frames, so the renderer
//! folds these offsets into the whole-sprite squash/skew. Clips with region
//! layer art (`lillith.layered_art.v1`, lillith-anim `layered_art`) move
//! those layers by the region offset instead (clamped to the art's slack).

use crate::bounce::soft_limit;
use crate::spring::{Spring, SpringParams};
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;

/// Spring layers per region.
pub const MAX_LAYERS: usize = 3;
/// Overall multiplier range (shown 0 … 300 %).
pub const MAX_MULTIPLIER: f32 = 3.0;
/// Per-region intensity range (shown 0 … 200 %).
pub const MAX_REGION: f32 = 2.0;
/// Extra kick gain at full build-up energy (× `build_up`).
pub const RAMP_MAX: f32 = 0.9;
/// Build-up energy decay time constant (s).
pub const ENERGY_TAU_S: f32 = 1.6;
/// Kick strength (px/s) that charges one full unit of energy.
pub const ENERGY_REF: f32 = 140.0;
/// Largest framing zoom honoured by [`LayeredJiggle::offsets`].
pub const MAX_ZOOM: f32 = 3.0;
/// Framing zoom assumed for `cleavage_zoom` / push-in clips without a
/// `zoom:<factor>` tag.
pub const DEFAULT_PUSH_IN_ZOOM: f32 = 1.6;
/// Breathing swell of the chest (px at full inhale, × intensity).
pub const BREATH_CHEST_PX: f32 = 1.4;
/// Share of body acceleration the layers feel (inertia coupling).
pub const INERTIA: f32 = 0.35;

/// A jiggling body region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    /// Vertical (+ = down).
    Chest,
    /// Hips / butt, lateral (+ = screen right).
    Hips,
    /// Pigtails / hair, lateral.
    Pigtails,
    /// Tail, lateral.
    Tail,
}

impl Region {
    pub const ALL: [Region; 4] = [Region::Chest, Region::Hips, Region::Pigtails, Region::Tail];

    pub fn name(self) -> &'static str {
        match self {
            Region::Chest => "chest",
            Region::Hips => "hips",
            Region::Pigtails => "pigtails",
            Region::Tail => "tail",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Region::Chest => "Chest",
            Region::Hips => "Hips / butt",
            Region::Pigtails => "Pigtails / hair",
            Region::Tail => "Tail",
        }
    }

    pub fn from_name(s: &str) -> Option<Region> {
        Some(match s {
            "chest" => Region::Chest,
            "hips" | "hip" | "butt" => Region::Hips,
            "pigtails" | "pigtail" | "pig" | "hair" => Region::Pigtails,
            "tail" => Region::Tail,
            _ => return None,
        })
    }

    fn idx(self) -> usize {
        self as usize
    }

    /// `(knee, cap)` in sprite px for the displayed offset.
    pub fn caps(self) -> (f32, f32) {
        match self {
            Region::Chest => (10.0, 20.0),
            Region::Hips => (8.0, 16.0),
            Region::Pigtails => (16.0, 30.0),
            Region::Tail => (18.0, 34.0),
        }
    }

    /// The three layers' tuning.
    pub fn layers(self) -> [LayerSpec; MAX_LAYERS] {
        let l = |freq_hz, zeta, weight, lag_s| LayerSpec {
            freq_hz,
            zeta,
            weight,
            lag_s,
        };
        match self {
            Region::Chest => [
                l(1.6, 0.30, 1.0, 0.0),
                l(2.7, 0.22, 0.45, 0.04),
                l(4.2, 0.18, 0.18, 0.07),
            ],
            Region::Hips => [
                l(1.3, 0.40, 1.0, 0.0),
                l(2.2, 0.28, 0.40, 0.05),
                l(3.6, 0.22, 0.15, 0.09),
            ],
            Region::Pigtails => [
                l(0.9, 0.22, 1.0, 0.0),
                l(1.6, 0.18, 0.50, 0.06),
                l(2.8, 0.16, 0.22, 0.12),
            ],
            Region::Tail => [
                l(0.8, 0.20, 1.0, 0.0),
                l(1.4, 0.16, 0.60, 0.08),
                l(2.4, 0.15, 0.30, 0.16),
            ],
        }
    }
}

/// One spring layer: natural frequency, damping ratio, output weight and
/// how long after a kick it starts moving.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerSpec {
    pub freq_hz: f32,
    pub zeta: f32,
    pub weight: f32,
    pub lag_s: f32,
}

impl LayerSpec {
    pub fn params(&self) -> SpringParams {
        let w = TAU * self.freq_hz;
        SpringParams {
            stiffness: w * w,
            damping: 2.0 * self.zeta * w,
        }
    }
}

/// `config.toml [springs.jiggle]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JiggleSettings {
    /// Master switch for the layered jiggle.
    pub enabled: bool,
    /// Overall multiplier, 0 … [`MAX_MULTIPLIER`].
    pub multiplier: f32,
    /// Per-region intensity, 0 … [`MAX_REGION`].
    pub chest: f32,
    pub hips: f32,
    pub pigtails: f32,
    pub tail: f32,
    /// Spring layers per region, 1 … [`MAX_LAYERS`].
    pub layers: u8,
    /// Progressive build-up, 0 (every kick the same) … 1.
    pub build_up: f32,
    /// Scale the jiggle with push-in / close-up clip framing.
    pub zoom_follow: bool,
}

impl Default for JiggleSettings {
    /// Livelier than the old single-spring look: 150 % overall, every region
    /// at 120 %, three layers, strong build-up.
    fn default() -> Self {
        Self {
            enabled: true,
            multiplier: 1.5,
            chest: 1.2,
            hips: 1.2,
            pigtails: 1.2,
            tail: 1.2,
            layers: MAX_LAYERS as u8,
            build_up: 0.6,
            zoom_follow: true,
        }
    }
}

impl JiggleSettings {
    /// Clamps every value into range (non-finite → default).
    pub fn sanitized(self) -> Self {
        let d = Self::default();
        let c = |v: f32, def: f32, max: f32| {
            if v.is_finite() {
                v.clamp(0.0, max)
            } else {
                def
            }
        };
        Self {
            enabled: self.enabled,
            multiplier: c(self.multiplier, d.multiplier, MAX_MULTIPLIER),
            chest: c(self.chest, d.chest, MAX_REGION),
            hips: c(self.hips, d.hips, MAX_REGION),
            pigtails: c(self.pigtails, d.pigtails, MAX_REGION),
            tail: c(self.tail, d.tail, MAX_REGION),
            layers: self.layers.clamp(1, MAX_LAYERS as u8),
            build_up: c(self.build_up, d.build_up, 1.0),
            zoom_follow: self.zoom_follow,
        }
    }

    pub fn region(&self, r: Region) -> f32 {
        match r {
            Region::Chest => self.chest,
            Region::Hips => self.hips,
            Region::Pigtails => self.pigtails,
            Region::Tail => self.tail,
        }
    }

    pub fn set_region(&mut self, r: Region, v: f32) {
        match r {
            Region::Chest => self.chest = v,
            Region::Hips => self.hips = v,
            Region::Pigtails => self.pigtails = v,
            Region::Tail => self.tail = v,
        }
    }

    /// Effective output gain for `r` (0 when disabled).
    pub fn gain(&self, r: Region) -> f32 {
        if self.enabled {
            self.multiplier * self.region(r)
        } else {
            0.0
        }
    }
}

/// Something that sets the jiggle off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Drive {
    /// A walk/run step; `foot` alternates the lateral sway.
    Footfall { run: bool, foot: u32 },
    /// Landing / impact, `impact` ≈ 0.5 (soft) … 2 (hard).
    Land { impact: f32 },
    /// Landing on a perch (window edge, taskbar).
    Perch,
    /// One bounce of a perch bounce / hop / tiptoe loop.
    Bounce,
    /// Sitting down (seat impact).
    SitDown,
    /// Standing back up (upward push).
    StandUp,
    /// Grabbing a window edge / being picked up: hangs from the hands.
    Hang,
    /// Cursor hover.
    Hover,
    /// Click / poke.
    Click,
    /// Turned around (`dir` = new facing, ±1).
    Turn { dir: f32 },
}

impl Drive {
    /// `(chest, hips, pigtails, tail)` kicks in px/s (chest + = down,
    /// lateral + = screen right).
    pub fn kicks(self) -> [f32; 4] {
        match self {
            Drive::Footfall { run, foot } => {
                let k = if run { 1.6 } else { 1.0 };
                let s = if foot % 2 == 1 { -1.0 } else { 1.0 };
                [24.0 * k, 18.0 * k * s, 16.0 * k * s, 14.0 * k * -s]
            }
            Drive::Land { impact } => {
                let i = if impact.is_finite() {
                    impact.clamp(0.0, 2.0)
                } else {
                    1.0
                };
                [62.0 * i, 34.0 * i, 40.0 * i, -36.0 * i]
            }
            Drive::Perch => [52.0, 30.0, 34.0, -34.0],
            Drive::Bounce => [48.0, 28.0, -30.0, 28.0],
            Drive::SitDown => [50.0, 56.0, 28.0, -34.0],
            Drive::StandUp => [-32.0, -26.0, 20.0, 26.0],
            Drive::Hang => [44.0, 22.0, 36.0, -42.0],
            Drive::Hover => [8.0, 0.0, 10.0, 12.0],
            Drive::Click => [36.0, 20.0, 26.0, -30.0],
            Drive::Turn { dir } => {
                let d = if dir < 0.0 { -1.0 } else { 1.0 };
                [10.0, -16.0 * d, -30.0 * d, -36.0 * d]
            }
        }
    }

    /// The drive a clip start implies, from its name (`hang`, `stand`,
    /// `bounce`/`jump`/`hop`, `land`/`drop`, `sit`, `perch`).
    pub fn for_clip(name: &str) -> Option<Drive> {
        let n = name.to_ascii_lowercase();
        let has = |s: &str| n.contains(s);
        if has("hang") || has("dangle") {
            Some(Drive::Hang)
        } else if has("stand_up") || has("to_stand") || has("sit_swing_to_idle") {
            Some(Drive::StandUp)
        } else if has("jump") {
            Some(Drive::Land { impact: 1.3 })
        } else if has("bounce") || has("hop") || has("tiptoe") {
            Some(Drive::Bounce)
        } else if has("land") || has("drop") || has("slip") || has("_to_perch") {
            Some(Drive::Land { impact: 1.0 })
        } else if has("sit") || has("kneel") {
            Some(Drive::SitDown)
        } else if has("perch") {
            Some(Drive::Perch)
        } else {
            None
        }
    }

    /// `true` for drives a looping clip repeats every pass (so its jiggle
    /// keeps building up).
    pub fn repeats(self) -> bool {
        matches!(self, Drive::Bounce | Drive::Hang)
    }
}

/// Framing zoom of a clip from its tags / name: `zoom:<factor>` wins, then
/// `cleavage_zoom` / `push_in` / `closeup` (tag or name) give
/// [`DEFAULT_PUSH_IN_ZOOM`]; else 1.
pub fn framing_zoom<'a>(name: &str, tags: impl IntoIterator<Item = &'a str>) -> f32 {
    let mut push = false;
    for t in tags {
        if let Some(v) = t
            .strip_prefix("zoom:")
            .and_then(|v| v.trim().parse::<f32>().ok())
        {
            if v.is_finite() && v > 0.0 {
                return v.clamp(1.0, MAX_ZOOM);
            }
        }
        push |= is_push_in(t);
    }
    if push || is_push_in(name) {
        DEFAULT_PUSH_IN_ZOOM
    } else {
        1.0
    }
}

fn is_push_in(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    s.contains("cleavage_zoom")
        || s.contains("push_in")
        || s.contains("closeup")
        || s.contains("zoom_in")
}

/// How a region's gain applies on a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionMode {
    /// Clean art: the full gain.
    Full,
    /// The art bakes this region's bounce: only the gain above 100 %.
    AboveBaked,
    /// Baked-jiggle art owns the region: none.
    Off,
}

/// Displayed region offsets (px at the given zoom).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct JiggleOffsets {
    /// + = down.
    pub chest: f32,
    /// + = screen right.
    pub hips: f32,
    pub pigtails: f32,
    pub tail: f32,
}

impl JiggleOffsets {
    pub fn get(&self, r: Region) -> f32 {
        match r {
            Region::Chest => self.chest,
            Region::Hips => self.hips,
            Region::Pigtails => self.pigtails,
            Region::Tail => self.tail,
        }
    }

    fn set(&mut self, r: Region, v: f32) {
        match r {
            Region::Chest => self.chest = v,
            Region::Hips => self.hips = v,
            Region::Pigtails => self.pigtails = v,
            Region::Tail => self.tail = v,
        }
    }

    /// Mirrored for a flipped sprite (lateral regions change sign).
    pub fn flipped(self, flip: bool) -> Self {
        if flip {
            Self {
                chest: self.chest,
                hips: -self.hips,
                pigtails: -self.pigtails,
                tail: -self.tail,
            }
        } else {
            self
        }
    }

    pub fn max_abs(&self) -> f32 {
        Region::ALL
            .iter()
            .map(|&r| self.get(r).abs())
            .fold(0.0, f32::max)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Pending {
    due: f32,
    region: Region,
    layer: usize,
    dv: f32,
}

/// The layered springs (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct LayeredJiggle {
    settings: JiggleSettings,
    springs: [[Spring; MAX_LAYERS]; 4],
    specs: [[LayerSpec; MAX_LAYERS]; 4],
    energy: [f32; 4],
    pending: Vec<Pending>,
    time: f32,
    hz: f32,
    body_v: (f32, f32),
    body_v_prev: Option<(f32, f32)>,
    breath: f32,
}

impl Default for LayeredJiggle {
    fn default() -> Self {
        Self::new(JiggleSettings::default(), 120.0)
    }
}

impl LayeredJiggle {
    pub fn new(settings: JiggleSettings, hz: f32) -> Self {
        let specs = Region::ALL.map(Region::layers);
        let springs = Region::ALL.map(|r| {
            let (_, cap) = r.caps();
            r.layers().map(|l| {
                Spring::new(l.params(), 0.0)
                    .with_max_velocity(600.0)
                    .with_max_offset(cap * 2.5)
            })
        });
        Self {
            settings: settings.sanitized(),
            springs,
            specs,
            energy: [0.0; 4],
            pending: Vec::new(),
            time: 0.0,
            hz: if hz.is_finite() && hz > 0.0 {
                hz
            } else {
                120.0
            },
            body_v: (0.0, 0.0),
            body_v_prev: None,
            breath: -1.0,
        }
    }

    pub fn settings(&self) -> &JiggleSettings {
        &self.settings
    }

    /// Live settings change (Settings ▸ Motion); motion in flight continues.
    pub fn set_settings(&mut self, s: JiggleSettings) {
        self.settings = s.sanitized();
        let n = self.settings.layers as usize;
        for r in Region::ALL {
            for l in n..MAX_LAYERS {
                let s = &mut self.springs[r.idx()][l];
                s.position = s.target;
                s.velocity = 0.0;
            }
        }
    }

    /// Build-up energy of `r`, 0 … 1.
    pub fn energy(&self, r: Region) -> f32 {
        self.energy[r.idx()]
    }

    /// Kick gain from the build-up energy.
    pub fn ramp(&self, r: Region) -> f32 {
        1.0 + self.settings.build_up * RAMP_MAX * self.energy[r.idx()]
    }

    /// Kicks one region (px/s, before intensities): every live layer gets
    /// it after its lag, scaled by the current build-up ramp.
    pub fn kick(&mut self, r: Region, strength: f32) {
        if !strength.is_finite() || strength == 0.0 || !self.settings.enabled {
            return;
        }
        let dv = strength * self.ramp(r);
        let e = &mut self.energy[r.idx()];
        *e = (*e + strength.abs() / ENERGY_REF).min(1.0);
        for l in 0..self.settings.layers as usize {
            let lag = self.specs[r.idx()][l].lag_s;
            if lag <= 0.0 {
                self.springs[r.idx()][l].impulse(dv);
            } else if self.pending.len() < 256 {
                self.pending.push(Pending {
                    due: self.time + lag,
                    region: r,
                    layer: l,
                    dv,
                });
            }
        }
    }

    /// Applies a [`Drive`] scaled by `scale` (e.g. a gait stride weight).
    pub fn drive(&mut self, d: Drive, scale: f32) {
        for (r, k) in Region::ALL.into_iter().zip(d.kicks()) {
            self.kick(r, k * scale);
        }
    }

    /// Her body's screen velocity (px/s): its changes (acceleration) push
    /// the layers by inertia, e.g. walk starts/stops, a swinging hang, the
    /// root spring's hop and landing.
    pub fn set_body_velocity(&mut self, vx: f32, vy: f32) {
        if vx.is_finite() && vy.is_finite() {
            self.body_v = (vx.clamp(-3000.0, 3000.0), vy.clamp(-3000.0, 3000.0));
        }
    }

    /// Breath value in [-1, 1] (-1 exhaled): swells the chest's primary
    /// layer target.
    pub fn set_breath(&mut self, b: f32) {
        if b.is_finite() {
            self.breath = b.clamp(-1.0, 1.0);
        }
    }

    /// Advances by `dt` seconds in fixed sub-steps.
    pub fn tick(&mut self, dt: f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let dt = dt.min(0.25);
        // Inertia: body acceleration → equal and opposite velocity change.
        let (vx, vy) = self.body_v;
        if let Some((px, py)) = self.body_v_prev {
            let (dvx, dvy) = ((vx - px) * INERTIA, (vy - py) * INERTIA);
            let lim = 160.0;
            if dvy.abs() > 1e-3 {
                self.inertia(Region::Chest, -dvy.clamp(-lim, lim));
            }
            if dvx.abs() > 1e-3 {
                for r in [Region::Hips, Region::Pigtails, Region::Tail] {
                    self.inertia(r, -dvx.clamp(-lim, lim));
                }
            }
        }
        self.body_v_prev = Some((vx, vy));
        // Breathing: inhale lifts (−y) the chest a little.
        let swell = -BREATH_CHEST_PX * 0.5 * (self.breath + 1.0);
        self.springs[Region::Chest.idx()][0].target = swell;
        let h = 1.0 / self.hz;
        let mut remaining = dt;
        let n = self.settings.layers as usize;
        while remaining > 1e-7 {
            let s = remaining.min(h);
            self.time += s;
            let now = self.time;
            let mut i = 0;
            while i < self.pending.len() {
                if self.pending[i].due <= now + 1e-6 {
                    let p = self.pending.swap_remove(i);
                    if p.layer < n {
                        self.springs[p.region.idx()][p.layer].impulse(p.dv);
                    }
                } else {
                    i += 1;
                }
            }
            for r in Region::ALL {
                for l in 0..n {
                    self.springs[r.idx()][l].step(s);
                }
            }
            remaining -= s;
        }
        let k = (-dt / ENERGY_TAU_S).exp();
        for e in &mut self.energy {
            *e *= k;
        }
    }

    fn inertia(&mut self, r: Region, dv: f32) {
        // Inertia is continuous: no lag, no energy charge.
        for l in 0..self.settings.layers as usize {
            self.springs[r.idx()][l].impulse(dv);
        }
    }

    /// Raw (un-limited, un-scaled) weighted layer sum of `r`.
    pub fn raw(&self, r: Region) -> f32 {
        let n = self.settings.layers as usize;
        (0..n)
            .map(|l| self.specs[r.idx()][l].weight * self.springs[r.idx()][l].position)
            .sum()
    }

    /// Displayed offsets at framing `zoom` (1 = normal full body): raw ×
    /// multiplier × region intensity through the region's soft knee, then
    /// × zoom (when `zoom_follow` is on).
    pub fn offsets(&self, zoom: f32) -> JiggleOffsets {
        self.offsets_with(zoom, |_| RegionMode::Full)
    }

    /// [`Self::offsets`] with a per-region [`RegionMode`] (baked art).
    pub fn offsets_with(&self, zoom: f32, mode: impl Fn(Region) -> RegionMode) -> JiggleOffsets {
        let z = if self.settings.zoom_follow && zoom.is_finite() {
            zoom.clamp(1.0, MAX_ZOOM)
        } else {
            1.0
        };
        let mut o = JiggleOffsets::default();
        for r in Region::ALL {
            let (knee, cap) = r.caps();
            let g = self.settings.gain(r);
            let g = match mode(r) {
                RegionMode::Full => g,
                RegionMode::AboveBaked => (g - 1.0).max(0.0),
                RegionMode::Off => 0.0,
            };
            let v = soft_limit(self.raw(r) * g, knee, cap) * z;
            o.set(r, if v.is_finite() { v } else { 0.0 });
        }
        o
    }

    /// `true` when every layer is (nearly) at rest.
    pub fn at_rest(&self, eps: f32) -> bool {
        self.pending.is_empty()
            && self
                .springs
                .iter()
                .flatten()
                .all(|s| s.offset().abs() < eps && s.velocity.abs() < eps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    fn run(j: &mut LayeredJiggle, secs: f32, f: impl Fn(&LayeredJiggle) -> f32) -> f32 {
        let mut peak = 0.0f32;
        for _ in 0..(secs / DT) as usize {
            j.tick(DT);
            peak = peak.max(f(j));
        }
        peak
    }

    #[test]
    fn layers_are_stable_and_never_underdamped() {
        for r in Region::ALL {
            for l in r.layers() {
                let w = TAU * l.freq_hz;
                assert!(w / 120.0 <= 0.25, "{r:?} {l:?}");
                assert!(l.zeta >= 0.15 && l.zeta < 1.0, "{r:?} {l:?}");
            }
        }
    }

    #[test]
    fn defaults_are_livelier_and_sanitize_clamps() {
        let d = JiggleSettings::default();
        assert!(d.enabled && d.multiplier > 1.0 && d.layers == 3);
        for r in Region::ALL {
            assert!(d.region(r) > 1.0, "{r:?}");
        }
        let s = JiggleSettings {
            multiplier: 99.0,
            chest: -1.0,
            hips: f32::NAN,
            layers: 9,
            build_up: 4.0,
            ..d
        }
        .sanitized();
        assert_eq!(s.multiplier, MAX_MULTIPLIER);
        assert_eq!(s.chest, 0.0);
        assert_eq!(s.hips, d.hips);
        assert_eq!(s.layers, MAX_LAYERS as u8);
        assert_eq!(s.build_up, 1.0);
        let s = JiggleSettings { layers: 0, ..d }.sanitized();
        assert_eq!(s.layers, 1);
    }

    #[test]
    fn a_landing_moves_every_region_then_settles() {
        let mut j = LayeredJiggle::default();
        j.drive(Drive::Land { impact: 1.0 }, 1.0);
        let mut peaks = [0.0f32; 4];
        for _ in 0..(1.0 / DT) as usize {
            j.tick(DT);
            let o = j.offsets(1.0);
            for r in Region::ALL {
                peaks[r.idx()] = peaks[r.idx()].max(o.get(r).abs());
            }
        }
        for r in Region::ALL {
            assert!(peaks[r.idx()] > 2.0, "{r:?} {peaks:?}");
        }
        run(&mut j, 8.0, |_| 0.0);
        assert!(j.at_rest(0.05), "settles: {:?}", j.offsets(1.0));
    }

    #[test]
    fn layers_overlap_instead_of_one_sine() {
        // Three layers: the secondary/flutter arrive later, so the response
        // is not a scaled copy of the single-layer one.
        let one = JiggleSettings {
            layers: 1,
            ..Default::default()
        };
        let mut a = LayeredJiggle::new(one, 120.0);
        let mut b = LayeredJiggle::default();
        a.kick(Region::Chest, 60.0);
        b.kick(Region::Chest, 60.0);
        let (mut xa, mut xb) = (Vec::new(), Vec::new());
        for _ in 0..120 {
            a.tick(DT);
            b.tick(DT);
            xa.push(a.raw(Region::Chest));
            xb.push(b.raw(Region::Chest));
        }
        let k = xb[10] / xa[10];
        let differs = xa.iter().zip(&xb).any(|(p, q)| (q - p * k).abs() > 0.3);
        assert!(differs, "layered response is a plain rescale");
        // Zero crossings happen at different times than the 1-layer curve.
        let zc = |v: &[f32]| v.windows(2).position(|w| w[0].signum() != w[1].signum());
        assert_ne!(zc(&xa), zc(&xb));
    }

    #[test]
    fn repeated_bounces_build_up_then_calm() {
        let mut j = LayeredJiggle::default();
        let mut first = 0.0;
        let mut last = 0.0;
        for i in 0..6 {
            j.drive(Drive::Bounce, 1.0);
            let p = run(&mut j, 0.6, |j| j.offsets(1.0).chest.abs());
            if i == 0 {
                first = p;
            }
            last = p;
        }
        assert!(j.energy(Region::Chest) > 0.5);
        assert!(last > first * 1.25, "builds up: {first} -> {last}");
        run(&mut j, 8.0, |_| 0.0);
        assert!(j.energy(Region::Chest) < 0.05, "energy decays");
        // No build-up: every bounce the same size.
        let mut flat = LayeredJiggle::new(
            JiggleSettings {
                build_up: 0.0,
                ..Default::default()
            },
            120.0,
        );
        let mut ps = Vec::new();
        for _ in 0..4 {
            flat.drive(Drive::Bounce, 1.0);
            ps.push(run(&mut flat, 0.6, |j| j.offsets(1.0).chest.abs()));
        }
        assert!(ps[3] < ps[0] * 1.2, "{ps:?}");
    }

    #[test]
    fn caps_hold_under_a_pile_of_kicks_at_max_settings() {
        let s = JiggleSettings {
            multiplier: MAX_MULTIPLIER,
            chest: MAX_REGION,
            hips: MAX_REGION,
            pigtails: MAX_REGION,
            tail: MAX_REGION,
            build_up: 1.0,
            ..Default::default()
        };
        let mut j = LayeredJiggle::new(s, 120.0);
        for _ in 0..200 {
            j.drive(Drive::Land { impact: 2.0 }, 1.0);
            j.drive(Drive::Click, 1.0);
            j.tick(DT);
            for z in [1.0, 2.0, 9.0] {
                let o = j.offsets(z);
                for r in Region::ALL {
                    let cap = r.caps().1 * z.min(MAX_ZOOM);
                    let v = o.get(r);
                    assert!(v.is_finite() && v.abs() <= cap + 1e-3, "{r:?} {v} > {cap}");
                }
            }
        }
    }

    #[test]
    fn zero_or_disabled_means_rest() {
        let mut j = LayeredJiggle::new(
            JiggleSettings {
                enabled: false,
                ..Default::default()
            },
            120.0,
        );
        j.drive(Drive::Land { impact: 2.0 }, 1.0);
        j.tick(0.2);
        assert_eq!(j.offsets(1.0), JiggleOffsets::default());
        let mut j = LayeredJiggle::new(
            JiggleSettings {
                tail: 0.0,
                ..Default::default()
            },
            120.0,
        );
        j.drive(Drive::Hang, 1.0);
        j.tick(0.2);
        let o = j.offsets(1.0);
        assert_eq!(o.tail, 0.0);
        assert!(o.pigtails.abs() > 0.1);
    }

    #[test]
    fn zoom_scales_offsets() {
        let mut j = LayeredJiggle::default();
        j.drive(Drive::Perch, 1.0);
        j.tick(0.15);
        let a = j.offsets(1.0);
        let b = j.offsets(2.0);
        for r in Region::ALL {
            assert!((b.get(r) - 2.0 * a.get(r)).abs() < 1e-4, "{r:?}");
        }
        let mut off = LayeredJiggle::new(
            JiggleSettings {
                zoom_follow: false,
                ..Default::default()
            },
            120.0,
        );
        off.drive(Drive::Perch, 1.0);
        off.tick(0.15);
        assert_eq!(off.offsets(2.0), off.offsets(1.0));
    }

    #[test]
    fn framing_zoom_from_tags_and_names() {
        assert_eq!(framing_zoom("idle", ["canonical"]), 1.0);
        assert_eq!(framing_zoom("x", ["cleavage_zoom"]), DEFAULT_PUSH_IN_ZOOM);
        assert_eq!(framing_zoom("perch_push_in_wink", []), DEFAULT_PUSH_IN_ZOOM);
        assert_eq!(framing_zoom("x", ["cleavage_zoom", "zoom:2.2"]), 2.2);
        assert_eq!(framing_zoom("x", ["zoom:40"]), MAX_ZOOM);
        assert_eq!(framing_zoom("x", ["zoom:abc"]), 1.0);
    }

    #[test]
    fn clip_names_map_to_drives() {
        use Drive::*;
        assert_eq!(Drive::for_clip("window_hang_swing"), Some(Hang));
        assert_eq!(Drive::for_clip("blend_sit_swing_to_idle"), Some(StandUp));
        assert_eq!(Drive::for_clip("sit_stand_up"), Some(StandUp));
        assert_eq!(Drive::for_clip("perch_bounce_kick"), Some(Bounce));
        assert_eq!(Drive::for_clip("tiptoe_bounce"), Some(Bounce));
        assert_eq!(Drive::for_clip("victory_jump"), Some(Land { impact: 1.3 }));
        assert_eq!(
            Drive::for_clip("blend_drag_held_to_perch_sit"),
            Some(Land { impact: 1.0 })
        );
        assert_eq!(Drive::for_clip("sit_bored_chin"), Some(SitDown));
        assert_eq!(Drive::for_clip("perch_lounge"), Some(Perch));
        assert_eq!(Drive::for_clip("idle"), None);
        assert!(Bounce.repeats() && Hang.repeats() && !SitDown.repeats());
    }

    #[test]
    fn body_acceleration_swings_hair_and_tail_against_it() {
        let mut j = LayeredJiggle::default();
        j.tick(DT);
        j.set_body_velocity(300.0, 0.0); // starts moving right
        j.tick(DT);
        run(&mut j, 0.1, |_| 0.0);
        let o = j.offsets(1.0);
        assert!(o.pigtails < -0.5 && o.tail < -0.5 && o.hips < 0.0, "{o:?}");
        // Constant velocity: no further push, it settles.
        run(&mut j, 8.0, |_| 0.0);
        assert!(j.offsets(1.0).max_abs() < 0.1);
    }

    #[test]
    fn breathing_lifts_the_chest_a_little() {
        let mut j = LayeredJiggle::default();
        j.set_breath(1.0);
        run(&mut j, 3.0, |_| 0.0);
        let c = j.offsets(1.0).chest;
        assert!(c < -0.5 && c > -BREATH_CHEST_PX * 4.0, "{c}");
    }

    #[test]
    fn region_names_and_flip() {
        for r in Region::ALL {
            assert_eq!(Region::from_name(r.name()), Some(r));
        }
        let o = JiggleOffsets {
            chest: 1.0,
            hips: 2.0,
            pigtails: 3.0,
            tail: 4.0,
        };
        assert_eq!(o.flipped(true).chest, 1.0);
        assert_eq!(o.flipped(true).tail, -4.0);
        assert_eq!(o.flipped(false), o);
    }

    #[test]
    fn baked_regions_only_get_the_share_above_100_percent() {
        let mut j = LayeredJiggle::default();
        j.drive(Drive::Land { impact: 1.0 }, 1.0);
        j.tick(0.12);
        let full = j.offsets(1.0);
        let baked = j.offsets_with(1.0, |_| RegionMode::AboveBaked);
        let off = j.offsets_with(1.0, |r| {
            if r == Region::Tail {
                RegionMode::Off
            } else {
                RegionMode::Full
            }
        });
        let g = j.settings().gain(Region::Chest); // 1.8 by default
        assert!(baked.chest.abs() > 0.1 && baked.chest.abs() < full.chest.abs());
        // Linear part of the knee: exactly (g - 1) / g of the full offset.
        let raw = j.raw(Region::Chest);
        assert!((baked.chest - raw * (g - 1.0)).abs() < 1e-3 || baked.chest.abs() > 10.0);
        assert_eq!(off.tail, 0.0);
        assert_eq!(off.chest, full.chest);
        // At 100 % a baked region adds nothing.
        let mut one = LayeredJiggle::new(
            JiggleSettings {
                multiplier: 1.0,
                chest: 1.0,
                ..Default::default()
            },
            120.0,
        );
        one.drive(Drive::Land { impact: 1.0 }, 1.0);
        one.tick(0.12);
        assert_eq!(one.offsets_with(1.0, |_| RegionMode::AboveBaked).chest, 0.0);
    }

    #[test]
    fn fewer_layers_live_switch_parks_extra_layers() {
        let mut j = LayeredJiggle::default();
        j.kick(Region::Tail, 80.0);
        j.tick(0.3);
        j.set_settings(JiggleSettings {
            layers: 1,
            ..Default::default()
        });
        j.tick(DT);
        assert!(j.offsets(1.0).tail.abs() > 0.1);
        run(&mut j, 10.0, |_| 0.0);
        assert!(j.at_rest(0.05));
    }
}
