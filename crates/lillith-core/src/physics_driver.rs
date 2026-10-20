//! `PhysicsDriver`: turns clips, events, locomotion and mood into spring
//! targets and impulses (PLAN.md §3.1 layer 5; design in
//! `assets/processed/physics/DRIVER_DESIGN.md`, ported from the physics-lab
//! prototype).
//!
//! It owns a [`Physics`] (the six spring.rs channels) plus the procedural
//! tail / horn / tattoo / face / heartbeat [`Overlay`]; tail and horns are
//! NOT spring channels (the figure-8 is a 2-D path and there is no layered
//! art yet). Feed it with [`PhysicsDriver::on_clip_start`],
//! [`PhysicsDriver::on_event`], [`PhysicsDriver::set_motion`],
//! [`PhysicsDriver::set_mood`] and [`PhysicsDriver::tick`]; read
//! [`PhysicsDriver::transform`] and [`PhysicsDriver::overlay`].
//! Everything is data-driven from `assets/physics.toml`
//! ([`PhysicsConfig`]) and deterministic for a given seed.
//!
//! Engine gait (physics-lab v3-v5, optional): with a `[gait]` table in
//! physics.toml, walking also drives a gait phase, hip bob, arm / forearm /
//! torso-twist springs, two alternating jiggle kicks per cycle, a timed view
//! turn with hard cuts (front / 3/4 / side) and a gait-locked tail sway. Read
//! them from [`Overlay::gait`] / [`PhysicsDriver::gait`] ([`GaitPose`]).
//! Without `[gait]`, or with `[gait] enabled = false` (the bundled file),
//! all of it is off and walking behaves exactly as before. While the drawn
//! clip bakes its own walk ([`PhysicsDriver::set_gait_suppressed`], manifest
//! `disable_springs = ["gait"]`) it is off too.

use crate::config::Springs as UserSprings;
use crate::mood::Mood;
use crate::physics_config::{
    mood_key, Gait, ImpulseCfg, MoodMul, PhysicsConfig, SpringCfg, TransitionStep,
};
use crate::rng::Rng;
use crate::spring::{Channel, Physics, Spring, SpringParams, Transform};
use std::collections::VecDeque;
use std::sync::Arc;

/// Locomotion state the driver needs (from lillith-core state machine).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Motion {
    Idle,
    /// `vx` px/s (sign = direction), `cycle_s` = one locomotion clip pass.
    Walking {
        vx: f32,
        cycle_s: f32,
    },
    Running {
        vx: f32,
        cycle_s: f32,
    },
    /// Held by the cursor; `vx` = cursor velocity px/s (drives dangling sway).
    Dragged {
        vx: f32,
    },
    Asleep,
}

impl Motion {
    fn locomotion(&self) -> Option<(f32, f32, bool)> {
        match *self {
            Motion::Walking { vx, cycle_s } => Some((vx, cycle_s, false)),
            Motion::Running { vx, cycle_s } => Some((vx, cycle_s, true)),
            _ => None,
        }
    }
    fn dragged(&self) -> bool {
        matches!(self, Motion::Dragged { .. })
    }
}

/// Procedural layers that are not spring.rs channels (tail, horns, glows,
/// face). Everything the renderer needs this frame besides [`Transform`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overlay {
    /// Driver clock, seconds.
    pub t: f32,
    /// Breath value in [-1, 1] (-1 exhaled rest).
    pub breath: f32,
    pub breath_stage: u8,
    // --- additive overlay outputs (not spring.rs channels)
    pub tail_tip_x: f32,
    pub tail_tip_y: f32,
    pub tail_trail: f32,
    pub heart_glow: f32,
    pub heart_scale: f32,
    pub horn_tilt: f32,
    pub horn_lift: f32,
    pub nebula_rate: f32,
    pub nebula_phase_left: f32,
    pub nebula_phase_right: f32,
    pub rim_glow: f32,
    pub tattoo_glow: f32,
    pub opacity: f32,
    /// 1 open, 0.5 wink, 0 closed.
    pub eye_open: f32,
    pub eyes_wide: bool,
    pub tongue: bool,
    pub mischief: f32,
    /// Horn sparks emitted since the previous [`PhysicsDriver::tick`]
    /// (feed to the particle system once per tick).
    pub sparks_burst: u32,
    /// Tattoo particles emitted since the previous [`PhysicsDriver::tick`].
    pub particles_burst: u32,
    /// Engine gait outputs (physics-lab v3-v5). All rest/0 unless
    /// physics.toml has a `[gait]` table and she is (or just was) walking.
    pub gait: GaitPose,
}

/// Gait outputs for the renderer (DRIVER_DESIGN Addenda v3-v5, SPEC 16-18).
/// Angles in DEGREES; + arm = toward facing. All 0 / rest when not walking.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GaitPose {
    /// 0..1: 0 = RIGHT heel strike, 0.5 = LEFT heel strike (== right_leg_phase).
    pub gait_phase: f32,
    pub left_leg_phase: f32,
    pub right_leg_phase: f32,
    /// Ankle forwardness -1 (back) .. +1 (forward), already x `gait_amp`.
    pub left_leg_fwd: f32,
    pub right_leg_fwd: f32,
    /// Stride weight 0..1 (ramps in after the turn-in, out after a stop).
    pub gait_amp: f32,
    /// Pelvis + upper body vertical offset px (+ down); feet stay planted.
    /// Draw this instead of a root_y footfall bob.
    pub hip_bob_px: f32,
    /// Upper arm about the shoulder (spring-driven).
    pub left_arm_deg: f32,
    pub right_arm_deg: f32,
    /// Forearm RELATIVE to the upper arm (>= `[gait].elbow_min_deg`).
    pub left_forearm_deg: f32,
    pub right_forearm_deg: f32,
    /// + = right shoulder forward (spring-driven).
    pub torso_twist_deg: f32,
    /// Heel strike during the last tick: 0 none, 1 left, 2 right.
    pub heel_strike: u8,
    /// View turn, smoothstepped: 0 = front idle view, 1 = side walk view.
    pub turn_progress: f32,
    /// +1 turning toward the side view, -1 back to front, 0 steady.
    pub turn_dir: i8,
    /// Drawing to show (HARD cuts, never crossfade): 0 front idle,
    /// 1 three-quarter, 2 side walk. == [`turn_view_of`]`(turn_progress)`.
    pub turn_view: u8,
    /// Gait-locked tail sway target (px, + = toward facing); already
    /// included in `tail_tip_x` through the tail spring.
    pub tail_sway_px: f32,
}

/// Which drawing a turn shows at `progress`: 0 front, 1 three-quarter,
/// 2 side (hard cuts at `[gait]` turn_cut_lo / turn_cut_hi).
pub fn turn_view_of(progress: f32, g: &Gait) -> u8 {
    if progress < g.turn_cut_lo {
        0
    } else if progress < g.turn_cut_hi {
        1
    } else {
        2
    }
}

/// Normalised forwardness of one leg at gait phase `u` (stance: linear
/// +1 -> -1; swing: eased -1 -> +1).
pub fn leg_fwd(u: f32, duty: f32) -> f32 {
    let u = u.rem_euclid(1.0);
    if u < duty {
        1.0 - 2.0 * u / duty
    } else {
        -1.0 + 2.0 * smooth((u - duty) / (1.0 - duty))
    }
}

/// Hip bob (px, +down) at gait phase `u` for amp 1, and its d2/du2.
fn hip_bob(u: f32, down: f32, up: f32) -> (f32, f32) {
    let w = 4.0 * std::f32::consts::PI;
    let a = 0.5 * (down + up);
    // +down at u = 0, 0.5 (heel strikes); -up at u = 0.25, 0.75 (mid-stance)
    (down - a * (1.0 - (w * u).cos()), -a * w * w * (w * u).cos())
}

/// Most recent heel strikes kept for [`PhysicsDriver::heel_strikes`].
const STRIKES_KEPT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Target {
    Ch(Channel),
    TailX,
    TailY,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    at: f32,
    target: Target,
    strength: f32,
    /// Breathing kicks: never muted by `mute_impulses`.
    ambient: bool,
}

#[derive(Debug, Clone)]
struct ActiveTransition {
    name: String,
    steps: Vec<TransitionStep>,
    idx: usize,
    t: f32,
    from: [f32; 6],
    started: bool,
}

fn range_u(rng: &mut Rng, r: [u32; 2]) -> u32 {
    let (lo, hi) = (r[0].min(r[1]), r[0].max(r[1]));
    lo + ((hi - lo + 1) as f32 * rng.next_f32()).min((hi - lo) as f32) as u32
}

/// Builds the six channels from physics.toml `[springs]` / `[gains]`
/// (clamps included; same values as `Physics::new` by default).
fn physics_from(cfg: &PhysicsConfig) -> Physics {
    let s = &cfg.springs;
    Physics {
        root_x: spring_from(&s.root_x, 0.0),
        root_y: spring_from(&s.root_y, 0.0),
        angle: spring_from(&s.angle, 0.0),
        scale: spring_from(&s.scale, 1.0),
        chest: spring_from(&s.chest, 0.0),
        pigtail: spring_from(&s.pigtails, 0.0),
        jiggle_gain: cfg.gains.jiggle,
        chest_bounce_gain: cfg.gains.chest_bounce,
        pigtail_sway_gain: cfg.gains.pigtail_sway,
        hz: cfg.physics_hz,
        suppressed: Default::default(),
    }
}

fn spring_from(c: &SpringCfg, target: f32) -> Spring {
    let mut s = Spring::new(
        SpringParams {
            stiffness: c.stiffness,
            damping: c.damping,
        },
        target,
    )
    .with_max_velocity(c.max_velocity);
    if let Some(m) = c.max_offset {
        s = s.with_max_offset(m);
    }
    s
}

/// Brief-shaped breath (phys_breathing.txt): inhale rise, hold, slower
/// exhale, settle. Value in [-1, 1] (-1 = exhaled rest) and stage 0..=3.
pub fn shaped_breath(phase: f32, inhale: f32, hold: f32, exhale: f32) -> (f32, u8) {
    let a = inhale;
    let b = a + hold;
    let c = b + exhale;
    if phase < a {
        (-1.0 + 2.0 * smooth(phase / a), 0)
    } else if phase < b {
        (1.0, 1)
    } else if phase < c {
        (1.0 - 2.0 * smooth((phase - b) / exhale), 2)
    } else {
        (-1.0, 3)
    }
}

/// The physics driver (see the module docs).
#[derive(Debug, Clone)]
pub struct PhysicsDriver {
    cfg: Arc<PhysicsConfig>,
    physics: Physics,
    rng: Rng,
    time: f32,
    // mood
    mood: Mood,
    mood_from: MoodMul,
    mood_to: MoodMul,
    blend_t: f32,
    mood_revert: Option<(Mood, f32)>,
    // breath
    breath_phase: f32,
    breath_period: f32,
    breath_value: f32,
    breath_stage: u8,
    // motion
    motion: Motion,
    facing: f32,
    walk_phase: f32,
    foot: u32,
    /// Footfalls come from the clip player ([`PhysicsDriver::on_footfall`]
    /// on the art's contact frames) instead of fixed cycle phases.
    frame_footfalls: bool,
    // gait (only active with a [gait] table that is enabled)
    /// Runtime switch, starts as `[gait].enabled` ([`Self::set_gait_enabled`]).
    gait_switch: bool,
    /// The drawn clip bakes its own gait ([`Self::set_gait_suppressed`]).
    gait_suppressed: bool,
    gait_cycle: f32,
    gait_lin: f32,
    arm_l: Spring,
    arm_r: Spring,
    fore_l: Spring,
    fore_r: Spring,
    twist: Spring,
    heel_strike: u8,
    turn_lin: f32,
    turn_dir: i8,
    gait_vx: f32,
    strikes: VecDeque<(f32, u32)>,
    // transitions (additive offsets per channel; scale offset from 1.0)
    trans: [f32; 6],
    active: Option<ActiveTransition>,
    /// Run the driver's own idle-transition scheduler. Off by default in the
    /// engine (`IdleLife` schedules; the animator calls `on_clip_start` /
    /// `start_transition`); the physics-lab harness turns it on.
    pub auto_idle: bool,
    next_transition: f32,
    family_idx: usize,
    // impulses
    pending: Vec<Pending>,
    pending_fx: Vec<(f32, String, f32)>,
    last_event_t: f32,
    /// Analysis only: drop every impulse (targets still move). Lets the
    /// harness diff "with kicks" vs "without" to isolate the jiggle response.
    pub mute_impulses: bool,
    /// Events fired during the last tick (e.g. `mischief.interrupt`), for
    /// the caller to route; cleared at the start of every tick.
    fired: Vec<String>,
    // overlay
    tail_x: Spring,
    tail_y: Spring,
    horn: Spring,
    tail_phase: f32,
    nebula_l: f32,
    nebula_r: f32,
    burst_t: f32,
    spike_t: f32,
    trail_t: f32,
    heart_flare: f32,
    rim_flare: f32,
    tattoo_flare: f32,
    tattoo_tau: f32,
    sparks_emitted: u32,
    particles_emitted: u32,
    sparks_last: u32,
    particles_last: u32,
    // face
    next_blink: f32,
    blink_t: f32,
    wink_t: f32,
    widen_t: f32,
    tongue_t: f32,
    tap_count: u32,
    tongue_every: u32,
    // mischief
    mischief: f32,
    next_spont_flick: f32,
}

const CH: [Channel; 6] = Channel::ALL;
fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

impl PhysicsDriver {
    /// Driver over `cfg` with the user's `config.toml [springs]` applied on
    /// top (a no-op with default springs). `physics_hz` overrides the file's
    /// rate when > 0.
    pub fn new(
        mut cfg: PhysicsConfig,
        user_springs: &UserSprings,
        physics_hz: f32,
        seed: u64,
    ) -> Self {
        cfg.apply_user_springs(user_springs);
        if physics_hz > 0.0 {
            cfg.physics_hz = physics_hz;
        }
        Self::from_config(cfg, seed)
    }

    /// Driver over the bundled `assets/physics.toml`.
    pub fn bundled(user_springs: &UserSprings, physics_hz: f32, seed: u64) -> Self {
        Self::new(PhysicsConfig::bundled(), user_springs, physics_hz, seed)
    }

    /// Driver over `cfg` exactly as given (no user overrides).
    pub fn from_config(cfg: PhysicsConfig, seed: u64) -> Self {
        let cfg = Arc::new(cfg);
        let physics = physics_from(&cfg);
        let mut rng = Rng::new(seed);
        let mood = Mood::default();
        let m = cfg.mood(mood);
        let breath_period =
            rng.range(cfg.breathing.period_s[0], cfg.breathing.period_s[1]) * m.breath_period;
        let next_blink = rng.range(cfg.face.blink_every_s[0], cfg.face.blink_every_s[1]);
        let next_transition = rng.range(
            cfg.idle.transition_every_s[0],
            cfg.idle.transition_every_s[1],
        );
        let tongue_every = range_u(&mut rng, cfg.face.tongue_every_n_taps);
        let next_spont_flick = rng.range(
            cfg.mischief.spontaneous_flick_every_s[0],
            cfg.mischief.spontaneous_flick_every_s[1],
        );
        let tail_x = spring_from(&cfg.tail.tip_spring, 0.0);
        let tail_y = spring_from(&cfg.tail.tip_spring, 0.0);
        let horn = spring_from(&cfg.horn.tilt_spring, 0.0);
        let g = cfg.gait.unwrap_or_default();
        let arm = || spring_from(&g.arm_spring, 0.0);
        let fore = || spring_from(&g.forearm_spring, 0.0);
        let (arm_l, arm_r, fore_l, fore_r) = (arm(), arm(), fore(), fore());
        let twist = spring_from(&g.twist_spring, 0.0);
        let gait_cycle = cfg.walk.demo_cycle_s;
        let mut d = Self {
            physics,
            rng,
            time: 0.0,
            mood,
            mood_from: m,
            mood_to: m,
            blend_t: 1.0,
            mood_revert: None,
            breath_phase: 0.0,
            breath_period,
            breath_value: -1.0,
            breath_stage: 3,
            motion: Motion::Idle,
            facing: 1.0,
            walk_phase: 0.0,
            foot: 0,
            frame_footfalls: false,
            gait_switch: cfg.active_gait().is_some(),
            gait_suppressed: false,
            gait_cycle,
            gait_lin: 0.0,
            arm_l,
            arm_r,
            fore_l,
            fore_r,
            twist,
            heel_strike: 0,
            turn_lin: 0.0,
            turn_dir: 0,
            gait_vx: 0.0,
            strikes: VecDeque::new(),
            trans: [0.0; 6],
            active: None,
            auto_idle: false,
            next_transition,
            family_idx: 0,
            pending: Vec::new(),
            pending_fx: Vec::new(),
            last_event_t: f32::NEG_INFINITY,
            mute_impulses: false,
            fired: Vec::new(),
            tail_x,
            tail_y,
            horn,
            tail_phase: 0.0,
            nebula_l: 0.0,
            nebula_r: 0.0,
            burst_t: 0.0,
            spike_t: 0.0,
            trail_t: 0.0,
            heart_flare: 0.0,
            rim_flare: 0.0,
            tattoo_flare: 0.0,
            tattoo_tau: 0.35,
            sparks_emitted: 0,
            particles_emitted: 0,
            sparks_last: 0,
            particles_last: 0,
            next_blink,
            blink_t: 0.0,
            wink_t: 0.0,
            widen_t: 0.0,
            tongue_t: 0.0,
            tap_count: 0,
            tongue_every,
            mischief: 0.0,
            next_spont_flick,
            cfg,
        };
        // Start at the exhaled rest so t=0 has no pop.
        let (b, _) = d.breath_curve(0.0);
        d.breath_value = b;
        d.apply_targets();
        for c in CH {
            let s = d.physics.spring_mut(c);
            s.position = s.target;
        }
        d
    }

    // ------------------------------------------------------------------ API

    pub fn time(&self) -> f32 {
        self.time
    }
    pub fn config(&self) -> &PhysicsConfig {
        &self.cfg
    }
    pub fn physics(&self) -> &Physics {
        &self.physics
    }
    pub fn physics_mut(&mut self) -> &mut Physics {
        &mut self.physics
    }
    /// The spring transform lillith-anim applies to the sprite frame.
    pub fn transform(&self) -> Transform {
        self.physics.transform()
    }
    pub fn mood(&self) -> Mood {
        self.mood
    }
    /// Facing sign for `signed` impulses: +1 right, -1 left.
    pub fn facing(&self) -> f32 {
        self.facing
    }
    pub fn set_facing(&mut self, right: bool) {
        self.facing = if right { 1.0 } else { -1.0 };
    }
    /// `true` if physics.toml has an `[events."<event>"]` entry.
    pub fn knows_event(&self, event: &str) -> bool {
        self.cfg.events.contains_key(event)
    }
    /// Events the driver fired itself during the last tick
    /// (`mischief.interrupt`).
    pub fn fired(&self) -> &[String] {
        &self.fired
    }
    /// Any interaction calms her down (mischief reset); `input.*` events do
    /// this automatically.
    pub fn on_interaction(&mut self) {
        self.mischief = 0.0;
    }
    /// Raw kick on a named channel (`Command::Impulse { channel, strength }`),
    /// scaled by the mood gain like every other kick. Unknown names are ignored.
    pub fn impulse_named(&mut self, channel: &str, strength: f32) -> bool {
        match Channel::from_name(channel) {
            Some(c) => {
                self.schedule(Target::Ch(c), strength, 0.0);
                true
            }
            None => false,
        }
    }
    pub fn motion(&self) -> Motion {
        self.motion
    }
    pub fn mood_mul(&self) -> MoodMul {
        MoodMul::lerp(&self.mood_from, &self.mood_to, smooth(self.blend_t))
    }
    pub fn transition_active(&self) -> Option<&str> {
        self.active.as_ref().map(|a| a.name.as_str())
    }

    /// Mood change; multipliers blend over `[mood_blend].blend_s`. Gain uses the
    /// new mood immediately (see physics.toml [moods] header).
    pub fn set_mood(&mut self, mood: Mood) {
        let Some(&to) = self.cfg.moods.get(&mood_key(mood)) else {
            return;
        };
        self.mood_from = self.mood_mul();
        self.mood_to = to;
        self.blend_t = if self.cfg.mood_blend.blend_s > 0.0 {
            0.0
        } else {
            1.0
        };
        self.mood = mood;
    }

    /// Snap to a mood with no blend (tests / harness setup).
    pub fn force_mood(&mut self, mood: Mood) {
        self.set_mood(mood);
        self.mood_from = self.mood_to;
        self.blend_t = 1.0;
        self.apply_mood_params();
    }

    /// `true`: locomotion footfalls are fed frame-accurately by the clip
    /// player through [`PhysicsDriver::on_footfall`]; the cycle-derived ones
    /// (fixed phases 0 and 1/2 of the cycle) are skipped.
    pub fn set_frame_footfalls(&mut self, on: bool) {
        self.frame_footfalls = on;
    }

    pub fn set_motion(&mut self, m: Motion) {
        let was = self.motion.locomotion();
        let now = m.locomotion();
        // With timed turns the boundary fires at each view cut instead (substep).
        let turning = self
            .live_gait()
            .is_some_and(|g| g.turn_in_s > 0.0 || g.turn_out_s > 0.0);
        if self.cfg.clip_boundary.on_locomotion_change && was.is_some() != now.is_some() && !turning
        {
            self.on_clip_boundary(); // a pose/view swap is NOT an impulse source
        }
        match (was, now) {
            (None, Some((vx, _, _))) => {
                self.facing = if vx < 0.0 { -1.0 } else { 1.0 };
                let a = self.cfg.walk.start_tip_angle * self.facing;
                self.schedule(Target::Ch(Channel::Angle), a, 0.0);
                // With [gait], a walk restarted while the stride is still
                // ramping out keeps its phase (no foot jump).
                if self.live_gait().is_none() || self.gait_lin <= 0.0 {
                    self.walk_phase = 0.0;
                    self.foot = 0;
                }
                self.active = None; // walking cancels idle transitions
                self.trans = [0.0; 6];
            }
            (Some((vx, _, _)), None) => {
                let s = if vx < 0.0 { -1.0 } else { 1.0 };
                self.schedule(
                    Target::Ch(Channel::Angle),
                    -self.cfg.walk.start_tip_angle * s,
                    0.0,
                );
                self.schedule(Target::Ch(Channel::Chest), self.cfg.walk.stop_chest, 0.0);
            }
            _ => {}
        }
        if m.dragged() && !self.motion.dragged() {
            let c = Arc::clone(&self.cfg);
            let (r, k) = (c.drag.pickup_recipe.as_str(), c.drag.pickup_scale);
            self.apply_recipe(r, k);
            self.active = None;
        } else if !m.dragged() && self.motion.dragged() {
            let c = Arc::clone(&self.cfg);
            let (r, k) = (c.drag.drop_recipe.as_str(), c.drag.drop_scale);
            self.apply_recipe(r, k); // soft landing (proposal: 0.8x)
        }
        self.motion = m;
    }

    /// Clip/pose boundary (e.g. side walk -> front idle): bleeds spring
    /// velocity and pulls offsets toward their targets by `[clip_boundary]`,
    /// so the swap settles as a small motion instead of replaying the old
    /// clip's momentum. Kicks scheduled after it still fire. Automatic on
    /// locomotion changes / turn cuts when `on_locomotion_change` is set; the
    /// animator may also call it on other pose-family swaps.
    pub fn on_clip_boundary(&mut self) {
        let cb = self.cfg.clip_boundary;
        for c in CH {
            let s = self.physics.spring_mut(c);
            s.velocity *= cb.velocity_keep;
            s.position = s.target + (s.position - s.target) * cb.offset_keep;
        }
    }

    /// `true` if engine gait is on: physics.toml has a `[gait]` table, its
    /// `enabled` is not false (the bundled file ships `false`) and
    /// [`Self::set_gait_enabled`] has not switched it off. Clips that bake
    /// their own walk can still suppress it ([`Self::gait_active`]).
    pub fn gait_enabled(&self) -> bool {
        self.gait_switch && self.cfg.gait.is_some()
    }

    /// Runtime gait switch (needs a `[gait]` table; no-op without one).
    /// Turning it off parks the gait state like a baked clip does.
    pub fn set_gait_enabled(&mut self, on: bool) {
        let was = self.live_gait().is_some();
        self.gait_switch = on;
        self.gait_changed(was);
    }

    /// The drawn clip already bakes the walk (legs, arms, hip bob, tail):
    /// the animator calls this from the clip's manifest `disable_springs`
    /// (`"gait"`, [`crate::spring::SpringMask::GAIT_GROUP`]), the same way
    /// baked-jiggle clips switch spring channels off. While suppressed the
    /// engine gait does not run: every [`GaitPose`] output except the view
    /// (instant swap by `Motion`) reads rest, no gait twist / tail / bob
    /// inertia, and footfalls use the plain `[walk]` kicks (whose walk
    /// `root_y` is 0 in the bundled file, so no extra bob on baked frames).
    pub fn set_gait_suppressed(&mut self, on: bool) {
        if on == self.gait_suppressed {
            return;
        }
        let was = self.live_gait().is_some();
        self.gait_suppressed = on;
        self.gait_changed(was);
    }

    /// Live gait switched off -> park it; switched on -> take over without a
    /// pop (a walk in progress already shows the side view: start turned and
    /// let the stride ramp in).
    fn gait_changed(&mut self, was: bool) {
        if was != self.live_gait().is_some() {
            self.park_gait();
        }
    }

    /// The `root_y` a footfall recipe may kick right now: 0 while the drawn
    /// clip bakes its own gait ([`Self::set_gait_suppressed`], whether or not
    /// `[gait]` is enabled) or while the engine gait is driving (its hip bob
    /// supplies the vertical step); `recipe_root_y` otherwise (gait off on an
    /// engine-clean clip: unchanged behaviour).
    pub fn footfall_root_y(&self, recipe_root_y: f32) -> f32 {
        if self.gait_suppressed || self.live_gait().is_some() {
            0.0
        } else {
            recipe_root_y
        }
    }

    /// `true` while the drawn clip suppresses the engine gait.
    pub fn gait_suppressed(&self) -> bool {
        self.gait_suppressed
    }

    /// `true` if the engine gait is driving right now (enabled and not
    /// suppressed by a baked-gait clip).
    pub fn gait_active(&self) -> bool {
        self.live_gait().is_some()
    }

    /// `[gait]` if it is enabled and not suppressed.
    fn live_gait(&self) -> Option<Gait> {
        if self.gait_switch && !self.gait_suppressed {
            self.cfg.gait
        } else {
            None
        }
    }

    /// Gait off (or on) mid-flight: stride, arms, twist and tail sway back to
    /// rest; the view is whatever `Motion` says (side while walking).
    fn park_gait(&mut self) {
        self.gait_lin = 0.0;
        self.turn_lin = if self.motion.locomotion().is_some() { 1.0 } else { 0.0 };
        self.turn_dir = 0;
        self.heel_strike = 0;
        for s in [
            &mut self.arm_l,
            &mut self.arm_r,
            &mut self.fore_l,
            &mut self.fore_r,
            &mut self.twist,
        ] {
            s.target = 0.0;
            s.position = 0.0;
            s.velocity = 0.0;
        }
        self.tail_x.target = 0.0;
    }

    /// Stride weight 0..1 (smoothstep of the linear ramp; 0 without `[gait]`).
    pub fn gait_amp(&self) -> f32 {
        smooth(self.gait_lin)
    }

    /// The most recent heel strikes as `(driver time, foot)`, foot 0 = left,
    /// 1 = right (bounded; analysis / footstep SFX).
    pub fn heel_strikes(&self) -> impl Iterator<Item = (f32, u32)> + '_ {
        self.strikes.iter().copied()
    }

    /// Gait outputs for this frame (also in [`Overlay::gait`]).
    pub fn gait(&self) -> GaitPose {
        let Some(g) = self.live_gait() else {
            // No (live) [gait]: instant view swap by Motion, nothing else moves.
            let p = if self.motion.locomotion().is_some() {
                1.0
            } else {
                0.0
            };
            return GaitPose {
                turn_progress: p,
                turn_view: if p > 0.0 { 2 } else { 0 },
                ..GaitPose::default()
            };
        };
        let amp = self.gait_amp();
        let u = self.walk_phase.rem_euclid(1.0);
        let ul = (u + 0.5).rem_euclid(1.0);
        let (bob, _) = hip_bob(u, g.hip_bob_down_px, g.hip_bob_up_px);
        let turn = smooth(self.turn_lin);
        GaitPose {
            gait_phase: u,
            left_leg_phase: ul,
            right_leg_phase: u,
            left_leg_fwd: amp * leg_fwd(ul, g.duty),
            right_leg_fwd: amp * leg_fwd(u, g.duty),
            gait_amp: amp,
            hip_bob_px: amp * bob,
            left_arm_deg: self.arm_l.position,
            right_arm_deg: self.arm_r.position,
            left_forearm_deg: self.fore_l.position - self.arm_l.position,
            right_forearm_deg: self.fore_r.position - self.arm_r.position,
            torso_twist_deg: self.twist.position,
            heel_strike: self.heel_strike,
            turn_progress: turn,
            turn_dir: self.turn_dir,
            turn_view: turn_view_of(turn, &g),
            tail_sway_px: self.tail_x.target,
        }
    }

    /// A clip (reaction, gesture, idle transition, locomotion) just started.
    /// Idle-transition clips run their [idle_transitions] sequence; others get
    /// their [clip_recipes] kick unless an event kicked within 0.15 s (dedupe:
    /// the rule's event and its clip start arrive together).
    pub fn on_clip_start(&mut self, clip: &str) {
        let cfg = Arc::clone(&self.cfg);
        if cfg.idle_transitions.contains_key(clip) {
            if cfg.idle.baked_transition_clips.iter().any(|c| c == clip) {
                self.start_transition_kicks_only(clip);
            } else {
                self.start_transition(clip);
            }
            return;
        }
        if cfg
            .clip_recipes
            .locomotion_prefixes
            .iter()
            .any(|p| clip.starts_with(p.as_str()))
        {
            return;
        }
        if self.time - self.last_event_t < 0.15 {
            return;
        }
        let r = cfg
            .clip_recipes
            .exact
            .get(clip)
            .unwrap_or(&cfg.clip_recipes.default);
        if r != "none" {
            let d = cfg
                .clip_recipes
                .impact_delay_s
                .get(clip)
                .copied()
                .unwrap_or(0.0);
            self.apply_recipe_delayed(r, 1.0, d);
        }
    }

    /// A dotted event (rules.toml name). Returns false if unknown (no-op).
    pub fn on_event(&mut self, event: &str) -> bool {
        let cfg = Arc::clone(&self.cfg);
        let Some(e) = cfg.events.get(event) else {
            return false;
        };
        if let Some(m) = e.mood {
            self.set_mood(m);
        }
        if event.starts_with("input.") {
            self.mischief = 0.0; // any interaction calms her down (repo Mischief::reset)
        }
        self.apply_recipe(&e.recipe, e.scale);
        self.last_event_t = self.time;
        true
    }

    /// Optional frame-accurate footfall from the clip player (instead of the
    /// cycle-derived ones). `foot` 0 = left, 1 = right.
    pub fn on_footfall(&mut self, foot: u32) {
        let Some((_, _, running)) = self.motion.locomotion() else {
            return;
        };
        let f = if running {
            self.cfg.walk.run_footfall
        } else {
            self.cfg.walk.walk_footfall
        };
        let alt = if self.cfg.walk.alternate_pigtails && foot % 2 == 1 {
            -1.0
        } else {
            1.0
        };
        // The footfall's root_y bob is dropped when something else already
        // supplies the vertical step: the drawn clip bakes its own walk
        // (`disable_springs = ["gait"]`, the video already bobs) or the engine
        // gait is driving (its hip_bob_px is the bob). Otherwise a run clip's
        // root_y kick would double it. See `Self::footfall_root_y`.
        let root_y = self.footfall_root_y(f.root_y);
        let Some(g) = self.live_gait() else {
            if root_y != 0.0 {
                self.schedule(Target::Ch(Channel::RootY), root_y, 0.0);
            }
            self.schedule(Target::Ch(Channel::Chest), f.chest, 0.0);
            self.schedule(
                Target::Ch(Channel::Pigtail),
                f.pigtails * alt * self.facing,
                self.cfg.pigtails.lag_s,
            );
            return;
        };
        // Gait: every step's jiggle scales with the stride weight (no kick
        // before the legs move), plus tail droop / alternating flick and an
        // alternating torso counter-twist (right strike -> right shoulder back).
        let k = self.gait_amp();
        debug_assert_eq!(root_y, 0.0, "live gait owns the vertical step");
        self.schedule(Target::Ch(Channel::Chest), f.chest * k, 0.0);
        self.schedule(
            Target::Ch(Channel::Pigtail),
            f.pigtails * alt * self.facing * k,
            self.cfg.pigtails.lag_s,
        );
        self.schedule(Target::TailY, g.step_tail_y * k, 0.03);
        self.schedule(Target::TailX, g.step_tail_x * alt * self.facing * k, 0.03);
        let right = if foot % 2 == 1 { 1.0 } else { -1.0 };
        if !self.mute_impulses {
            self.twist.impulse(-g.step_twist * right * k);
        }
        self.heel_strike = if foot % 2 == 1 { 2 } else { 1 };
        if self.strikes.len() >= STRIKES_KEPT {
            self.strikes.pop_front();
        }
        self.strikes.push_back((self.time, foot % 2));
    }

    /// Advance by `dt` seconds (any size; processed in fixed 1/hz sub-steps
    /// so delayed kicks land on the right sub-step).
    pub fn tick(&mut self, dt: f32) {
        self.fired.clear();
        self.heel_strike = 0;
        let h = 1.0 / self.physics.hz;
        let mut remaining = dt;
        while remaining > 1e-7 {
            let s = remaining.min(h);
            self.substep(s);
            remaining -= s;
        }
        // Bursts emitted since the previous tick (events between ticks included).
        self.sparks_last = std::mem::take(&mut self.sparks_emitted);
        self.particles_last = std::mem::take(&mut self.particles_emitted);
    }

    /// Procedural overlay state for this frame.
    pub fn overlay(&self) -> Overlay {
        let m = self.mood_mul();
        let c = &self.cfg;
        let b = self.breath_value;
        let inhale01 = (b + 1.0) * 0.5;
        let beat = 60.0 / c.heartbeat.bpm;
        let x = self.time.rem_euclid(beat);
        let g = |u: f32| (-(u / 0.04) * (u / 0.04)).exp();
        let pulse = c.heartbeat.pulse_amplitude
            * (g(x) + c.heartbeat.dub_ratio * g(x - c.heartbeat.dub_delay_s));
        let w = std::f32::consts::TAU * self.tail_phase;
        // The idle figure-8 gives way to the gait-locked sway while walking.
        let f8 = self
            .live_gait()
            .map_or(1.0, |g| 1.0 - self.gait_amp() * (1.0 - g.tail_figure8_walk));
        let tail_amp =
            m.tail_amplitude * (1.0 + c.mischief.tail_amplitude_boost * self.mischief) * f8;
        Overlay {
            t: self.time,
            breath: b,
            breath_stage: self.breath_stage,
            tail_tip_x: c.tail.figure8_amp_x_px * tail_amp * w.sin() + self.tail_x.position,
            tail_tip_y: c.tail.figure8_amp_y_px * tail_amp * (2.0 * w).sin()
                - c.tail.inhale_lift_px * inhale01
                + self.tail_y.position,
            tail_trail: (self.trail_t / c.tail.trail_s).clamp(0.0, 1.0),
            heart_glow: ((c.tail.heart_glow_base + c.tail.heart_glow_breath * b) * m.glow
                + self.heart_flare)
                .clamp(0.0, 1.0),
            heart_scale: 1.0 + c.tail.heart_scale_pulse * inhale01,
            horn_tilt: {
                let m = self.cfg.horn.tilt_spring.max_offset.unwrap_or(0.07);
                self.horn.position.clamp(-m, m)
            },
            horn_lift: if self.spike_t > 0.0 {
                c.horn.spike_lift_px * smooth(self.spike_t / c.horn.spike_s * 4.0)
            } else {
                0.0
            },
            nebula_rate: self.nebula_rate(),
            nebula_phase_left: self.nebula_l,
            nebula_phase_right: self.nebula_r,
            rim_glow: (c.horn.rim_glow_base * m.glow + self.rim_flare).clamp(0.0, 1.0),
            tattoo_glow: ((c.tattoo.glow_base + c.tattoo.glow_breath * b) * m.glow
                + self.tattoo_flare)
                .clamp(0.0, 1.0),
            opacity: (c.heartbeat.base_opacity + pulse).min(1.0),
            eye_open: if self.blink_t > 0.0 {
                0.0
            } else if self.wink_t > 0.0 {
                0.5
            } else {
                1.0
            },
            eyes_wide: self.widen_t > 0.0,
            tongue: self.tongue_t > 0.0,
            mischief: self.mischief,
            sparks_burst: self.sparks_last,
            particles_burst: self.particles_last,
            gait: self.gait(),
        }
    }

    /// Full spring-target sequence (use when the animator stays on the idle clip).
    pub fn start_transition(&mut self, name: &str) {
        self.begin_transition(name, false);
    }

    /// Only the per-step kicks/fx (the playing clip already bakes the body motion).
    pub fn start_transition_kicks_only(&mut self, name: &str) {
        self.begin_transition(name, true);
    }

    fn begin_transition(&mut self, name: &str, kicks_only: bool) {
        if self.motion.locomotion().is_some() || self.motion.dragged() {
            return; // held back while busy (repo idle.rs)
        }
        let Some(mut steps) = self.cfg.idle_transitions.get(name).cloned() else {
            return;
        };
        if kicks_only {
            for s in &mut steps {
                s.targets.clear();
            }
        }
        self.active = Some(ActiveTransition {
            name: name.to_string(),
            steps,
            idx: 0,
            t: 0.0,
            from: self.trans,
            started: false,
        });
    }

    // ------------------------------------------------------------ internals

    fn nebula_rate(&self) -> f32 {
        let c = &self.cfg.horn;
        let m = self.mood_mul();
        let stage = match self.breath_stage {
            0 => c.nebula_inhale_mult,
            2 => c.nebula_exhale_mult,
            _ => 1.0,
        };
        let burst = if self.burst_t > 0.0 {
            c.burst_mult
        } else {
            1.0
        };
        (1.0 / c.nebula_period_default_s) * m.nebula_speed * stage * burst
    }

    fn breath_curve(&self, phase: f32) -> (f32, u8) {
        let b = &self.cfg.breathing;
        if b.shape == "sine" {
            let v = (phase * std::f32::consts::TAU).sin();
            (v, if (0.25..0.75).contains(&phase) { 2 } else { 0 })
        } else {
            shaped_breath(phase, b.inhale, b.hold, b.exhale)
        }
    }

    fn schedule(&mut self, target: Target, strength: f32, delay: f32) {
        self.pending.push(Pending {
            at: self.time + delay.max(0.0),
            target,
            strength,
            ambient: false,
        });
    }

    fn schedule_ambient(&mut self, target: Target, strength: f32, delay: f32) {
        self.pending.push(Pending {
            at: self.time + delay.max(0.0),
            target,
            strength,
            ambient: true,
        });
    }

    fn schedule_impulse(&mut self, i: &ImpulseCfg, scale: f32) {
        let Some(ch) = Channel::from_name(&i.channel) else {
            return;
        };
        let sign = if i.signed { self.facing } else { 1.0 };
        let s = i.strength * scale * sign;
        self.schedule(Target::Ch(ch), s, i.delay_s);
        if i.double_ratio != 0.0 {
            let dd = if i.double_delay_s > 0.0 {
                i.double_delay_s
            } else {
                match ch {
                    Channel::Chest => self.cfg.chest.double_jiggle_delay_s,
                    Channel::Pigtail => self.cfg.pigtails.double_bounce_delay_s,
                    _ => self.cfg.chest.double_jiggle_delay_s,
                }
            };
            self.schedule(Target::Ch(ch), s * i.double_ratio, i.delay_s + dd);
        }
    }

    pub fn apply_recipe(&mut self, name: &str, scale: f32) {
        self.apply_recipe_delayed(name, scale, 0.0);
    }

    /// Recipe whose kicks and fx all land `extra` seconds from now (impact frame sync).
    pub fn apply_recipe_delayed(&mut self, name: &str, scale: f32, extra: f32) {
        let cfg = Arc::clone(&self.cfg);
        let Some(r) = cfg.impulse_recipes.get(name) else {
            return;
        };
        for i in &r.impulses {
            let mut i = i.clone();
            i.delay_s += extra;
            self.schedule_impulse(&i, scale);
        }
        for f in &r.fx {
            if extra > 0.0 {
                self.pending_fx.push((self.time + extra, f.clone(), scale));
            } else {
                self.fx(f, scale);
            }
        }
    }

    fn fx(&mut self, name: &str, scale: f32) {
        let c = Arc::clone(&self.cfg);
        let m = self.mood_mul();
        let tail_amp = m.tail_amplitude * (1.0 + c.mischief.tail_amplitude_boost * self.mischief);
        match name {
            "tail_flick_light" | "tail_flick_strong" => {
                let strong = name.ends_with("strong");
                let f = if strong {
                    c.tail.flick_strong
                } else {
                    c.tail.flick_light
                } * scale
                    * tail_amp;
                self.schedule(Target::TailY, f, 0.06);
                self.schedule(Target::TailX, -f * c.tail.flick_x_ratio * self.facing, 0.06);
                self.heart_flare += c.tail.heart_glow_flare * if strong { 1.0 } else { 0.6 };
                self.trail_t = c.tail.trail_s;
            }
            "horn_burst" => {
                self.burst_t = c.horn.burst_s;
                self.rim_flare += c.horn.rim_glow_flare;
                self.sparks_emitted += range_u(&mut self.rng, c.horn.burst_sparks);
            }
            "horn_spike" => self.spike_t = c.horn.spike_s,
            "horn_brighten" => self.rim_flare += 0.5 * c.horn.rim_glow_flare,
            "tattoo_flare_light" => {
                self.tattoo_flare = self.tattoo_flare.max(c.tattoo.flare_light * scale.min(1.5));
                self.tattoo_tau = c.tattoo.flare_tau_light_s;
            }
            "tattoo_flare_strong" => {
                self.tattoo_flare = self
                    .tattoo_flare
                    .max(c.tattoo.flare_strong * scale.min(1.5));
                self.tattoo_tau = c.tattoo.flare_tau_strong_s;
            }
            "particles" => {
                self.particles_emitted += range_u(&mut self.rng, c.tattoo.particles_per_tap)
            }
            "eyes_widen" => self.widen_t = c.face.oh_widen_s,
            "wink" => self.wink_t = 0.25,
            "wink_chance" => {
                if self.rng.next_f32() < c.face.wink_on_strong_tap_chance {
                    self.wink_t = 0.25;
                }
            }
            "tongue_count" => {
                self.tap_count += 1;
                if self.tap_count >= self.tongue_every {
                    self.tap_count = 0;
                    self.tongue_every = range_u(&mut self.rng, c.face.tongue_every_n_taps);
                    self.tongue_t = if self.mood == Mood::Mischievous {
                        c.face.tongue_peek_long_s
                    } else {
                        c.face.tongue_peek_s
                    };
                }
            }
            "lazy_blink" => self.blink_t = c.face.lazy_blink_close_s,
            _ => {}
        }
    }

    fn fire(&mut self, p: Pending) {
        if self.mute_impulses && !p.ambient {
            return;
        }
        let m = self.mood_to; // new mood's gain applies immediately
        let mut s = p.strength * m.gain;
        match p.target {
            Target::Ch(Channel::Pigtail) => {
                s *= 1.0 + self.cfg.mischief.pigtail_gain_boost * self.mischief;
                self.physics.impulse(Channel::Pigtail, s);
                let echo = s.abs() * self.cfg.chest.sympathetic_from_pigtails;
                if echo > 0.0 {
                    self.physics.impulse(Channel::Chest, echo);
                }
            }
            Target::Ch(c) => self.physics.impulse(c, s),
            Target::TailX => self.tail_x.impulse(s),
            Target::TailY => self.tail_y.impulse(s),
        }
    }

    fn apply_mood_params(&mut self) {
        let m = self.mood_mul();
        let cfg = Arc::clone(&self.cfg);
        let s = &cfg.springs;
        for (c, base) in [
            (Channel::RootX, s.root_x),
            (Channel::RootY, s.root_y),
            (Channel::Angle, s.angle),
            (Channel::Scale, s.scale),
            (Channel::Chest, s.chest),
            (Channel::Pigtail, s.pigtails),
        ] {
            self.physics.set_params(
                c,
                SpringParams {
                    stiffness: base.stiffness * m.stiffness,
                    damping: base.damping * m.damping,
                },
            );
        }
        let t = self.cfg.tail.tip_spring;
        let tp = SpringParams {
            stiffness: t.stiffness * m.stiffness,
            damping: t.damping * m.damping,
        };
        self.tail_x.params = tp;
        self.tail_y.params = tp;
        let hz = self.cfg.horn.tilt_spring;
        self.horn.params = SpringParams {
            stiffness: hz.stiffness * m.stiffness,
            damping: hz.damping * m.damping,
        };
    }

    fn apply_targets(&mut self) {
        let m = self.mood_mul();
        let b = self.breath_value;
        let br = &self.cfg.breathing;
        let amp = m.breath_amplitude;
        let ia = m.idle_amplitude;
        let pg = &self.cfg.pigtails;
        let vx = match self.motion {
            Motion::Dragged { vx } => vx,
            // With [gait] the hair's walking trail eases in/out with the stride.
            _ if self.live_gait().is_some() => self.gait_vx * self.gait_amp(),
            m => m.locomotion().map(|l| l.0).unwrap_or(0.0),
        };
        let drag_angle = match self.motion {
            Motion::Dragged { vx } => (vx * self.cfg.drag.hold_angle_per_px_s).clamp(
                -self.cfg.drag.hold_angle_clamp,
                self.cfg.drag.hold_angle_clamp,
            ),
            _ => 0.0,
        };
        let sway = pg.micro_sway_px
            * ia
            * (std::f32::consts::TAU * self.time / pg.micro_sway_period_s).sin();
        let vel = (-vx * pg.velocity_coupling).clamp(-pg.velocity_clamp_px, pg.velocity_clamp_px);
        let t = self.trans;
        let f = self.facing;
        self.physics.set_target(Channel::RootX, t[0] * ia * f);
        self.physics.set_target(Channel::RootY, t[1] * ia);
        self.physics
            .set_target(Channel::Angle, t[2] * ia * f + drag_angle);
        self.physics.set_target(
            Channel::Scale,
            1.0 + t[3] * ia + br.scale_amplitude * amp * b,
        );
        self.physics
            .set_target(Channel::Chest, t[4] * ia + br.chest_amplitude_px * amp * b);
        self.physics
            .set_target(Channel::Pigtail, t[5] * ia + sway + vel);
    }

    fn step_transition(&mut self, h: f32) {
        let Some(mut a) = self.active.take() else {
            return;
        };
        if !a.started {
            a.started = true;
            a.from = self.trans;
            let step = a.steps[a.idx].clone();
            for i in &step.impulses {
                self.schedule_impulse(i, 1.0);
            }
            for f in &step.fx {
                self.fx(f, 1.0);
            }
        }
        a.t += h;
        let step = &a.steps[a.idx];
        let k = if step.ramp_s > 0.0 {
            smooth(a.t / step.ramp_s)
        } else {
            1.0
        };
        for (i, c) in CH.iter().enumerate() {
            if let Some(&v) = step.targets.get(c.name()) {
                let to = if *c == Channel::Scale { v - 1.0 } else { v };
                self.trans[i] = a.from[i] + (to - a.from[i]) * k;
            }
        }
        if a.t >= step.ramp_s + step.hold_s {
            a.idx += 1;
            a.t = 0.0;
            a.started = false;
            if a.idx >= a.steps.len() {
                return; // done
            }
        }
        self.active = Some(a);
    }

    /// One sub-step of the engine gait (physics-lab v3-v5): view turn with
    /// hard cuts, stride ramp, phase + heel strikes, arm/forearm/twist and
    /// tail-sway targets, hip-bob inertia. Runs before `apply_targets`.
    fn step_gait(&mut self, c: &PhysicsConfig, g: Gait, h: f32) {
        let loco = self.motion.locomotion();
        // View turn: walking -> turn to the side first; stopped -> only once
        // the stride has ramped out.
        let before_turn = self.turn_lin;
        if loco.is_some() {
            self.turn_lin = if g.turn_in_s > 0.0 {
                (self.turn_lin + h / g.turn_in_s).min(1.0)
            } else {
                1.0
            };
        } else if self.gait_lin <= 0.0 {
            self.turn_lin = if g.turn_out_s > 0.0 {
                (self.turn_lin - h / g.turn_out_s).max(0.0)
            } else {
                0.0
            };
        }
        self.turn_dir = if self.turn_lin > before_turn {
            1
        } else if self.turn_lin < before_turn {
            -1
        } else {
            0
        };
        let view = turn_view_of(smooth(self.turn_lin), &g);
        if view != turn_view_of(smooth(before_turn), &g) {
            // HARD CUT between drawings: bleed momentum, then one small settle
            // kick so the swap reads as a weight shift.
            if c.clip_boundary.on_locomotion_change {
                self.on_clip_boundary();
            }
            let dir = if self.turn_lin > before_turn {
                -1.0
            } else {
                1.0
            };
            self.schedule(Target::Ch(Channel::Chest), g.turn_chest, 0.0);
            self.schedule(
                Target::Ch(Channel::Pigtail),
                g.turn_pigtails * dir * self.facing,
                c.pigtails.lag_s,
            );
        }
        let turned = self.turn_lin >= 1.0;
        if let Some((vx, cycle, _)) = loco {
            self.gait_cycle = cycle.max(0.05);
            self.gait_vx = vx;
            if turned {
                self.gait_lin = (self.gait_lin + h / g.start_ramp_s.max(1e-3)).min(1.0);
            }
        } else {
            self.gait_lin = (self.gait_lin - h / g.stop_ramp_s.max(1e-3)).max(0.0);
        }
        // Phase 0 = RIGHT heel strike, 0.5 = LEFT; keeps running while the
        // stride ramps out after a stop.
        if (loco.is_some() && turned) || self.gait_lin > 0.0 {
            let n = c.walk.footfalls_per_cycle.max(1) as f32;
            let first = loco.is_some() && self.walk_phase == 0.0 && self.foot == 0;
            let before = (self.walk_phase * n).floor();
            self.walk_phase += h / self.gait_cycle;
            // Frame-accurate footfalls (contact frames) replace the phase ones.
            if loco.is_some()
                && !self.frame_footfalls
                && ((self.walk_phase * n).floor() > before || first)
            {
                // integer phase = RIGHT heel strike (foot 1), half = LEFT (foot 0)
                let idx = (self.walk_phase * n).floor() as i64;
                let foot = if idx.rem_euclid(2) == 0 { 1 } else { 0 };
                self.foot += 1;
                self.on_footfall(foot);
            }
        }
        let amp = self.gait_amp();
        let u = self.walk_phase.rem_euclid(1.0);
        let fr = amp * leg_fwd(u, g.duty);
        let fl = amp * leg_fwd(u + 0.5, g.duty);
        // arms counter-swing the SAME-side leg (right leg forward -> right arm back)
        self.arm_r.target = -g.arm_amplitude_deg * fr;
        self.arm_l.target = -g.arm_amplitude_deg * fl;
        // relaxed elbow: bend grows from 0 at the back swing to
        // forearm_flex_deg at the front (half at neutral), no kink at 0
        let flex = |a: f32| {
            amp * g.forearm_flex_deg
                * (0.5 + 0.5 * a / g.arm_amplitude_deg.max(1e-3)).clamp(0.0, 1.0)
        };
        self.fore_r.target = self.arm_r.position * g.forearm_follow + flex(self.arm_r.position);
        self.fore_l.target = self.arm_l.position * g.forearm_follow + flex(self.arm_l.position);
        self.twist.target = -g.torso_twist_deg * fr;
        // tail: gait-locked side-to-side sway, one swing per cycle, chased by
        // the tail spring so it trails the hips; heel-strike flicks on top
        self.tail_x.target = amp
            * g.tail_sway_px
            * self.facing
            * (std::f32::consts::TAU * (u + g.tail_sway_phase)).sin();
        // hip-bob inertia: the body accelerates, the soft parts lag
        let (_, d2) = hip_bob(u, g.hip_bob_down_px, g.hip_bob_up_px);
        let acc = amp * d2 / (self.gait_cycle * self.gait_cycle); // px/s^2, +down
        if !self.mute_impulses && amp > 0.0 {
            if !self.physics.suppressed.contains(Channel::Chest) {
                self.physics.chest.velocity -= g.bob_inertia_chest * acc * h * self.mood_to.gain;
            }
            self.tail_y.velocity -= g.bob_inertia_tail_y * acc * h;
        }
    }

    fn substep(&mut self, h: f32) {
        self.time += h;
        let c = Arc::clone(&self.cfg);

        // mood blend + revert
        if self.blend_t < 1.0 {
            self.blend_t = (self.blend_t + h / c.mood_blend.blend_s).min(1.0);
        }
        if let Some((m, t)) = self.mood_revert.take() {
            if self.time >= t {
                self.set_mood(m)
            } else {
                self.mood_revert = Some((m, t))
            }
        }
        self.apply_mood_params();
        let m = self.mood_mul();

        // mischief
        if self.motion != Motion::Asleep {
            self.mischief += c.mischief.rate_per_s * h;
            if self.mischief >= c.mischief.threshold {
                self.mischief = 0.0;
                let prev = self.mood;
                self.on_event(&c.mischief.on_threshold_event);
                self.fired.push(c.mischief.on_threshold_event.clone());
                self.set_mood(c.mischief.on_threshold_mood);
                self.mood_revert = Some((prev, self.time + c.mischief.on_threshold_mood_s));
            }
            if self.mischief > c.mischief.spontaneous_flick_above {
                self.next_spont_flick -= h;
                if self.next_spont_flick <= 0.0 {
                    self.next_spont_flick = self.rng.range(
                        c.mischief.spontaneous_flick_every_s[0],
                        c.mischief.spontaneous_flick_every_s[1],
                    );
                    self.fx("tail_flick_light", 1.0);
                }
            }
        }

        // breathing (period re-picked per cycle, scaled by mood)
        self.breath_phase += h / self.breath_period;
        if self.breath_phase >= 1.0 {
            self.breath_phase = self.breath_phase.fract();
            self.breath_period = self
                .rng
                .range(c.breathing.period_s[0], c.breathing.period_s[1])
                * m.breath_period;
        }
        let (b, stage) = self.breath_curve(self.breath_phase);
        if stage != self.breath_stage && c.breathing.shape != "sine" {
            let amp = m.breath_amplitude;
            match stage {
                0 => self.schedule_ambient(
                    Target::Ch(Channel::Pigtail),
                    c.breathing.inhale_pigtails * amp,
                    c.pigtails.lag_s,
                ),
                2 => {
                    self.schedule_ambient(
                        Target::Ch(Channel::Pigtail),
                        c.breathing.exhale_pigtails * amp,
                        c.pigtails.lag_s,
                    );
                    let chance = c.face.tongue_on_exhale_chance
                        * (1.0 + c.mischief.tongue_chance_boost * self.mischief);
                    if self.rng.next_f32() < chance {
                        self.tongue_t = c.face.tongue_peek_s;
                    }
                    self.particles_emitted +=
                        range_u(&mut self.rng, c.tattoo.idle_drift_per_breath);
                }
                3 => self.schedule_ambient(
                    Target::Ch(Channel::Chest),
                    c.breathing.exhale_bounce_chest * amp,
                    0.0,
                ),
                _ => {}
            }
        }
        self.breath_value = b;
        self.breath_stage = stage;

        // idle-transition scheduler
        if self.auto_idle
            && self.motion == Motion::Idle
            && self.active.is_none()
            && !c.idle.family_order.is_empty()
        {
            self.next_transition -= h;
            if self.next_transition <= 0.0 {
                let n = c.idle.family_order[self.family_idx % c.idle.family_order.len()].clone();
                self.family_idx += 1;
                self.start_transition(&n);
                self.next_transition = self
                    .rng
                    .range(c.idle.transition_every_s[0], c.idle.transition_every_s[1])
                    * m.idle_interval;
            }
        }
        self.step_transition(h);

        if let Some(g) = self.live_gait() {
            self.step_gait(&c, g, h);
        } else if let Some((_, cycle, _)) = self.motion.locomotion() {
            // locomotion footfalls (cycle-derived)
            let n = c.walk.footfalls_per_cycle.max(1) as f32;
            let before = (self.walk_phase * n).floor();
            self.walk_phase += h / cycle.max(0.05);
            if !self.frame_footfalls
                && ((self.walk_phase * n).floor() > before
                    || (self.walk_phase <= h / cycle && self.foot == 0))
            {
                let foot = self.foot;
                self.foot += 1;
                self.on_footfall(foot);
            }
        }

        self.apply_targets();

        // due kicks
        let now = self.time + 1e-6;
        let mut due = Vec::new();
        self.pending.retain(|p| {
            if p.at <= now {
                due.push(*p);
                false
            } else {
                true
            }
        });
        for p in due {
            self.fire(p);
        }
        let mut fx_due = Vec::new();
        self.pending_fx.retain(|p| {
            if p.0 <= now {
                fx_due.push(p.clone());
                false
            } else {
                true
            }
        });
        for (_, f, k) in fx_due {
            self.fx(&f, k);
        }

        // integrate
        self.physics.step(h);
        let hz = self.physics.hz;
        self.tail_x.advance(h, hz);
        self.tail_y.advance(h, hz);
        self.horn.target = self.physics.angle.position * c.horn.tilt_follow;
        self.horn.advance(h, hz);
        if let Some(g) = self.live_gait() {
            self.arm_l.advance(h, hz);
            self.arm_r.advance(h, hz);
            self.fore_l.advance(h, hz);
            self.fore_r.advance(h, hz);
            // elbow joint stop: the forearm may lag but never hyperextend
            let stop = g.elbow_min_deg;
            for (fore, arm) in [
                (&mut self.fore_l, &self.arm_l),
                (&mut self.fore_r, &self.arm_r),
            ] {
                if fore.position - arm.position < stop {
                    fore.position = arm.position + stop;
                    fore.velocity = fore.velocity.max(arm.velocity);
                }
            }
            self.twist.advance(h, hz);
        }

        // overlay timers
        let w_speed = m.tail_speed / c.tail.figure8_period_default_s;
        self.tail_phase = (self.tail_phase + h * w_speed).fract();
        let r = self.nebula_rate();
        self.nebula_l = (self.nebula_l + r * h * c.horn.nebula_left_dir).rem_euclid(1.0);
        self.nebula_r = (self.nebula_r + r * h * c.horn.nebula_right_dir).rem_euclid(1.0);
        let dec = |v: &mut f32| *v = (*v - h).max(0.0);
        dec(&mut self.burst_t);
        dec(&mut self.spike_t);
        dec(&mut self.trail_t);
        dec(&mut self.wink_t);
        dec(&mut self.widen_t);
        dec(&mut self.tongue_t);
        dec(&mut self.blink_t);
        self.heart_flare *= (-h / c.tail.heart_glow_tau_s).exp();
        self.rim_flare *= (-h / c.horn.rim_glow_tau_s).exp();
        self.tattoo_flare *= (-h / self.tattoo_tau).exp();
        self.next_blink -= h;
        if self.next_blink <= 0.0 {
            self.next_blink = self
                .rng
                .range(c.face.blink_every_s[0], c.face.blink_every_s[1])
                * m.blink_interval;
            self.blink_t = if m.blink_interval > 1.3 {
                c.face.lazy_blink_close_s
            } else {
                c.face.blink_close_s
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HZ: f32 = 120.0;

    fn driver(seed: u64) -> PhysicsDriver {
        PhysicsDriver::bundled(&UserSprings::default(), HZ, seed)
    }

    fn channels(t: Transform) -> [f32; 6] {
        [t.x, t.y, t.angle, t.scale, t.chest_offset, t.pigtail_offset]
    }

    /// Runs `events` / `clips` (name, at seconds) twice, once with impulses
    /// muted, and returns per-sample (t, with - without) for every channel:
    /// the pure jiggle response, independent of breathing.
    fn response(
        mood: Mood,
        events: &[(&str, f32)],
        clips: &[(&str, f32)],
        secs: f32,
    ) -> Vec<(f32, [f32; 6])> {
        let run = |mute: bool| {
            let mut d = driver(42);
            d.force_mood(mood);
            d.mute_impulses = mute;
            let mut out = Vec::new();
            let steps = (secs * HZ) as usize;
            for i in 0..steps {
                let t = i as f32 / HZ;
                for (e, at) in events {
                    if (t - at).abs() < 0.5 / HZ {
                        d.on_event(e);
                    }
                }
                for (c, at) in clips {
                    if (t - at).abs() < 0.5 / HZ {
                        d.on_clip_start(c);
                    }
                }
                d.tick(1.0 / HZ);
                out.push((d.time(), channels(d.transform())));
            }
            out
        };
        let (a, b) = (run(false), run(true));
        a.iter()
            .zip(&b)
            .map(|((t, x), (_, y))| {
                let mut d = [0.0; 6];
                for i in 0..6 {
                    d[i] = x[i] - y[i];
                }
                (*t, d)
            })
            .collect()
    }

    fn peak(r: &[(f32, [f32; 6])], ch: usize) -> f32 {
        r.iter().map(|(_, d)| d[ch].abs()).fold(0.0, f32::max)
    }

    const ROOT_X: usize = 0;
    const ROOT_Y: usize = 1;
    const ANGLE: usize = 2;
    const CHEST: usize = 4;
    const PIG: usize = 5;

    #[test]
    fn bundled_driver_matches_repo_springs() {
        let d = driver(1);
        let reference = Physics::default();
        for c in Channel::ALL {
            let (a, b) = (d.physics().spring(c), reference.spring(c));
            match c {
                // physics-lab motion-review retune (zeta ~0.75) beats the
                // README-SP defaults; an untouched config.toml keeps it.
                Channel::Chest => assert_eq!(
                    (a.params.stiffness, a.params.damping),
                    (90.0, 14.5),
                    "{c:?}"
                ),
                Channel::Pigtail => assert_eq!(
                    (a.params.stiffness, a.params.damping),
                    (40.0, 9.5),
                    "{c:?}"
                ),
                _ => assert_eq!(a.params, b.params, "{c:?}"),
            }
            assert_eq!(a.max_velocity, b.max_velocity, "{c:?}");
            assert_eq!(a.max_offset, b.max_offset, "{c:?}");
        }
        assert_eq!(d.physics().hz, 120.0);
        assert_eq!(d.mood(), Mood::Playful);
        assert!(!d.auto_idle, "IdleLife schedules in the engine");
        assert!(d.knows_event("cpu.high") && !d.knows_event("no.such.event"));
    }

    #[test]
    fn knife_tap_impulse_jiggles_then_settles() {
        // Light tap: single chest jiggle in the brief's 1-2 cm (~1.2-2.6 px).
        let r = response(Mood::Playful, &[("knife_tap", 0.5)], &[], 4.0);
        let chest = peak(&r, CHEST);
        assert!((1.2..=2.6).contains(&chest), "light tap chest peak {chest}");
        assert!(peak(&r, PIG) > 2.5, "pigtails swing: {}", peak(&r, PIG));
        // Motion review: the tap sways the planted upper body (angle) only;
        // the old root_x kick slid her whole body sideways.
        assert!(peak(&r, ANGLE) > 0.001, "upper-body sway {}", peak(&r, ANGLE));
        assert!(peak(&r, ROOT_X) < 0.5, "planted, no slide {}", peak(&r, ROOT_X));
        // Nothing left after ~3 s.
        let tail = r
            .iter()
            .filter(|(t, _)| *t > 3.5)
            .map(|(_, d)| d[CHEST].abs().max(d[PIG].abs()))
            .fold(0.0, f32::max);
        assert!(tail < 0.05, "still moving {tail}");

        // Press-stab: stronger, with a second softer chest lobe.
        let r = response(Mood::Playful, &[("knife_tap.strong", 0.5)], &[], 3.0);
        assert!(
            peak(&r, CHEST) > chest * 1.3,
            "strong {} vs light {chest}",
            peak(&r, CHEST)
        );
        let d: Vec<f32> = r.iter().map(|(_, d)| d[CHEST]).collect();
        let pk = d.iter().cloned().fold(0.0, f32::max);
        let maxima: Vec<f32> = (1..d.len() - 1)
            .filter(|&k| d[k] > d[k - 1] && d[k] >= d[k + 1] && d[k] > 0.2 * pk)
            .map(|k| d[k])
            .collect();
        assert!(maxima.len() >= 2, "double jiggle, maxima {maxima:?}");
        let ratio = maxima[1] / maxima[0];
        assert!((0.3..=0.9).contains(&ratio), "second/first {ratio}");
    }

    #[test]
    fn knife_tap_clip_kick_lands_on_impact_frame() {
        // physics.toml impact_delay_s.knife_tap = 0.4: nothing before it.
        let r = response(Mood::Playful, &[], &[("knife_tap", 0.5)], 2.0);
        let early = r
            .iter()
            .filter(|(t, _)| *t < 0.5 + 0.38)
            .map(|(_, d)| d.iter().map(|v| v.abs()).fold(0.0, f32::max))
            .fold(0.0, f32::max);
        assert!(early < 1e-6, "kicked before the impact frame: {early}");
        assert!(peak(&r, CHEST) > 1.0);
    }

    #[test]
    fn cpu_high_impulse_hops_and_switches_mood() {
        let mut d = driver(3);
        assert!(d.on_event("cpu.high"));
        assert_eq!(d.mood(), Mood::Alert, "rule mood applied before the kick");
        // root_y kick is immediate (hop up = negative y).
        assert!(
            d.physics().root_y.velocity < 0.0 || {
                d.tick(1.0 / HZ);
                d.physics().root_y.velocity < 0.0
            }
        );
        let r = response(Mood::Playful, &[("cpu.high", 0.5)], &[], 3.0);
        let hop = r.iter().map(|(_, d)| d[ROOT_Y]).fold(0.0, f32::min);
        assert!(hop < -2.5, "cpu.high hop {hop}");
        assert!(peak(&r, CHEST) > 3.0, "chest {}", peak(&r, CHEST));
        assert!(peak(&r, PIG) > 5.0, "pigtails {}", peak(&r, PIG));
        // Overlay: horn burst sparks + tail flick from the `high` recipe fx.
        let mut d = driver(3);
        d.on_event("cpu.high");
        d.tick(1.0 / HZ);
        let o = d.overlay();
        assert!(o.sparks_burst > 0 && o.particles_burst > 0, "{o:?}");
        d.tick(1.0 / HZ);
        assert_eq!(d.overlay().sparks_burst, 0, "bursts are per tick");
        let mut low = 0f32;
        for _ in 0..60 {
            d.tick(1.0 / HZ);
            low = low.min(d.overlay().tail_tip_y);
        }
        assert!(low < -12.0, "tail flick {low}");
        // Unknown events are a no-op.
        assert!(!d.on_event("no.such.event"));
    }

    #[test]
    fn event_then_clip_is_not_double_kicked() {
        let a = response(Mood::Playful, &[("cpu.high", 0.5)], &[], 2.0);
        let b = response(
            Mood::Playful,
            &[("cpu.high", 0.5)],
            &[("event_cpu_busy", 0.5)],
            2.0,
        );
        for ((_, x), (_, y)) in a.iter().zip(&b) {
            assert!((x[CHEST] - y[CHEST]).abs() < 1e-6);
        }
        let c = response(Mood::Playful, &[], &[("event_cpu_busy", 0.5)], 2.0);
        assert!(peak(&c, CHEST) > 2.0, "bare clip start kicks");
    }

    #[test]
    fn moods_scale_the_response() {
        let p = response(Mood::Playful, &[("knife_tap.strong", 0.5)], &[], 3.0);
        let s = response(Mood::Sleepy, &[("knife_tap.strong", 0.5)], &[], 3.0);
        assert!(peak(&p, CHEST) > 1.8 * peak(&s, CHEST));
        assert!(peak(&p, PIG) > 1.8 * peak(&s, PIG));
    }

    #[test]
    fn breathing_and_walk_footfalls() {
        let mut d = driver(7);
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for _ in 0..(10 * 120) {
            d.tick(1.0 / HZ);
            let s = d.transform().scale;
            lo = lo.min(s);
            hi = hi.max(s);
        }
        assert!(
            // physics-lab retune: scale_amplitude 0.006 -> 0.003 (lab test range)
            (0.004..=0.008).contains(&(hi - lo)),
            "scale p2p {}",
            hi - lo
        );

        let mut d = driver(3);
        let cycle = d.config().walk.demo_cycle_s;
        d.set_motion(Motion::Walking {
            vx: 60.0,
            cycle_s: cycle,
        });
        let (mut chest, mut pig_sum) = (0f32, 0f32);
        let n = 3 * 120;
        for _ in 0..n {
            d.tick(1.0 / HZ);
            chest = chest.max(d.transform().chest_offset);
            pig_sum += d.transform().pigtail_offset;
        }
        assert!(chest > 1.2, "footfall chest bounce {chest}");
        assert!(pig_sum / n as f32 <= -0.5, "pigtails trail the motion");
    }

    #[test]
    fn baked_transition_clips_get_kicks_only() {
        let mut d = driver(5);
        d.start_transition("lean_forward");
        let mut max_x = 0f32;
        for _ in 0..(3 * 120) {
            d.tick(1.0 / HZ);
            max_x = max_x.max(d.transform().x);
        }
        assert!((6.0..=10.5).contains(&max_x), "lean {max_x}");
        assert!(d.transition_active().is_none());
        d.on_clip_start("lean_forward"); // baked clip: no root_x targets
        let mut max_x2 = 0f32;
        for _ in 0..(3 * 120) {
            d.tick(1.0 / HZ);
            max_x2 = max_x2.max(d.transform().x.abs());
        }
        assert!(max_x2 < 0.5, "baked clip moved root_x {max_x2}");
    }

    #[test]
    fn mischief_fires_interrupt_and_overlay_is_procedural() {
        let mut cfg = PhysicsConfig::bundled();
        cfg.mischief.rate_per_s = 0.5;
        let mut d = PhysicsDriver::from_config(cfg, 9);
        let mut saw = false;
        for _ in 0..(3 * 120) {
            d.tick(1.0 / HZ);
            saw |= d.fired().iter().any(|e| e == "mischief.interrupt");
        }
        assert!(saw);
        assert_eq!(d.mood(), Mood::Mischievous);
        // Tail / horn live in the overlay, the six spring channels are unchanged.
        assert_eq!(Channel::ALL.len(), 6);
        let o = d.overlay();
        assert!(o.horn_tilt.abs() <= 0.07 + 1e-6);
        assert!(o.opacity > 0.9 && o.opacity <= 1.0);
    }

    #[test]
    fn impulse_named_accepts_physics_toml_names() {
        let mut d = driver(2);
        assert!(d.impulse_named("pigtails", 40.0));
        assert!(d.impulse_named("pigtail", 1.0));
        assert!(
            !d.impulse_named("tail_tip_x", 1.0),
            "overlay, not a channel"
        );
        d.tick(1.0 / HZ);
        assert!(d.physics().pigtail.velocity.abs() > 10.0);
    }

    #[test]
    fn drag_drop_is_softer_than_pickup() {
        let mut d = driver(11);
        let v0 = d.physics().root_y.velocity;
        d.set_motion(Motion::Dragged { vx: 0.0 });
        d.tick(1.0 / HZ);
        let pickup = (d.physics().root_y.velocity - v0).abs();
        for _ in 0..(2 * 120) {
            d.tick(1.0 / HZ);
        }
        let v1 = d.physics().root_y.velocity;
        d.set_motion(Motion::Idle);
        d.tick(1.0 / HZ);
        let drop = (d.physics().root_y.velocity - v1).abs();
        assert!(
            drop < pickup && drop > 0.6 * pickup,
            "pickup {pickup} drop {drop}"
        );
    }
}
