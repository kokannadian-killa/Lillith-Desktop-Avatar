//! Engine gait (physics-lab v3-v5) ported into lillith-core: arms, torso
//! twist, hip bob, per-step jiggle, timed view turns with hard cuts and the
//! walking tail. Ported from `physics-lab/tests/physics.rs` and run against
//! a verbatim copy of the lab's physics.toml (`tests/fixtures/
//! physics_lab_v5.toml`). The bundled assets/physics.toml carries the same
//! tables but ships `[gait] enabled = false`, and clips that bake their own
//! walk (manifest `disable_springs = ["gait"]`) suppress it even when on;
//! see the "bundled gait switch" section at the end.

use lillith_core::config::Springs as UserSprings;
use lillith_core::physics_config::{ClipBoundary, Gait, PhysicsConfig, BUNDLED_TOML};
use lillith_core::physics_driver::{turn_view_of, Motion, PhysicsDriver};

const HZ: f32 = 120.0;
const LAB_TOML: &str = include_str!("fixtures/physics_lab_v5.toml");

fn lab() -> PhysicsConfig {
    PhysicsConfig::from_toml_str(LAB_TOML).unwrap_or_else(|e| panic!("lab toml:\n{e}"))
}

fn gait(cfg: &PhysicsConfig) -> Gait {
    cfg.gait.expect("[gait]")
}

fn walk(cfg: &PhysicsConfig) -> Motion {
    Motion::Walking {
        vx: cfg.walk.demo_speed_px_s,
        cycle_s: cfg.walk.demo_cycle_s,
    }
}

fn driver(cfg: &PhysicsConfig, mute: bool) -> PhysicsDriver {
    let mut d = PhysicsDriver::from_config(cfg.clone(), 3);
    d.auto_idle = false;
    d.mute_impulses = mute;
    d
}

fn walk_driver(mute: bool) -> PhysicsDriver {
    let cfg = lab();
    let mut d = driver(&cfg, mute);
    d.set_motion(walk(&cfg));
    d
}

/// `toml` with its `[gait]` table (header to the next table) cut out.
fn without_gait_table(toml: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    for line in toml.lines() {
        if line.starts_with('[') {
            skip = line.trim_end().starts_with("[gait]");
        }
        if !skip {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The bundled physics.toml with `[gait] enabled = true` (Ko flips this).
fn bundled_gait_on() -> PhysicsConfig {
    let on = BUNDLED_TOML.replacen("[gait]\nenabled = false", "[gait]\nenabled = true", 1);
    assert_ne!(on, BUNDLED_TOML, "bundled [gait] starts with `enabled = false`");
    let cfg = PhysicsConfig::from_toml_str(&on).unwrap();
    assert!(cfg.active_gait().is_some());
    cfg
}

fn tick(d: &mut PhysicsDriver) {
    d.tick(1.0 / HZ);
}

fn corr(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len() as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut sab, mut saa, mut sbb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        sab += (x - ma) * (y - mb);
        saa += (x - ma) * (x - ma);
        sbb += (y - mb) * (y - mb);
    }
    sab / (saa * sbb).sqrt().max(1e-9)
}

// ---------------------------------------------------------------- config

#[test]
fn lab_toml_parses_with_gait_defaults_matching() {
    let cfg = lab();
    assert_eq!(
        gait(&cfg),
        Gait::default(),
        "Gait::default() == lab v5 [gait]"
    );
    assert_eq!(
        cfg.clip_boundary,
        ClipBoundary {
            velocity_keep: 0.15,
            offset_keep: 0.4,
            on_locomotion_change: true
        }
    );
}

#[test]
fn bundled_toml_has_gait_off_and_lab_boundary() {
    let cfg = PhysicsConfig::bundled();
    let g = cfg.gait.expect("bundled [gait] table");
    assert!(!g.enabled, "bundled gait ships OFF");
    assert!(cfg.active_gait().is_none());
    // Everything else in [gait] is the lab v5 tune.
    assert_eq!(Gait { enabled: true, ..g }, Gait::default());
    assert_eq!(cfg.clip_boundary, lab().clip_boundary);
    let mut d = PhysicsDriver::bundled(&UserSprings::default(), HZ, 3);
    assert!(!d.gait_enabled() && !d.gait_active());
    d.set_motion(Motion::Walking {
        vx: 60.0,
        cycle_s: 1.1667,
    });
    for _ in 0..(3 * 120) {
        tick(&mut d);
    }
    let g = d.overlay().gait;
    // Instant view by Motion, nothing else moves.
    assert_eq!((g.turn_progress, g.turn_view), (1.0, 2));
    assert_eq!(
        (g.gait_amp, g.left_arm_deg, g.torso_twist_deg, g.hip_bob_px),
        (0.0, 0.0, 0.0, 0.0)
    );
}

#[test]
fn partial_gait_table_and_validation() {
    let base = without_gait_table(BUNDLED_TOML);
    let cfg = PhysicsConfig::from_toml_str(&format!(
        "{base}\n[gait]\narm_amplitude_deg = 12.0\n"
    ))
    .unwrap();
    let g = gait(&cfg);
    assert_eq!(g.arm_amplitude_deg, 12.0);
    assert_eq!(g.duty, Gait::default().duty);
    assert!(g.enabled, "a [gait] table without `enabled` is on");
    let err = PhysicsConfig::from_toml_str(&format!(
        "{base}\n[gait]\nduty = 1.5\nturn_cut_lo = 0.9\nturn_cut_hi = 0.1\n"
    ))
    .unwrap_err();
    assert!(err.contains("duty") && err.contains("turn_cut_lo"), "{err}");
}

// ---------------------------------------------------------------- v2 retune regressions (lab fixture)

#[test]
fn stab_recipes_do_not_shove_the_body() {
    let cfg = lab();
    for r in ["knife_tap", "knife_tap_strong", "stab"] {
        for i in &cfg.impulse_recipes[r].impulses {
            assert!(
                i.channel != "root_x" && i.channel != "root_y",
                "{r} kicks {}",
                i.channel
            );
        }
    }
    let mut d = driver(&cfg, false);
    let mut mx = 0f32;
    for i in 0..(4 * 120) {
        if i == 60 {
            d.on_event("knife_tap.strong");
        }
        if i == 240 {
            d.on_event("window.closed");
        }
        tick(&mut d);
        mx = mx.max(d.transform().x.abs());
    }
    assert!(mx < 1e-4, "root_x moved {mx} px on a stab");
}

#[test]
fn walk_to_idle_boundary_is_gentle() {
    let cfg = lab();
    let g = gait(&cfg);
    let mk = || {
        let mut d = walk_driver(false);
        for _ in 0..((4.0 + cfg.walk.demo_cycle_s) * HZ) as usize {
            tick(&mut d);
        }
        d
    };
    let (mut d, mut r) = (mk(), mk());
    let (mut wc, mut wp) = (0f32, 0f32);
    d.set_motion(Motion::Idle);
    let mid = g.stop_ramp_s + 0.5 * g.turn_out_s;
    let (mut chest, mut pig, mut chest_late, mut pig_late) = (0f32, 0f32, 0f32, 0f32);
    for i in 0..(2 * 120) {
        tick(&mut d);
        tick(&mut r);
        let off = |d: &PhysicsDriver| {
            let p = d.physics();
            (
                (p.chest.position - p.chest.target).abs(),
                (p.pigtail.position - p.pigtail.target).abs(),
            )
        };
        let (rc, rp) = off(&r);
        wc = wc.max(rc);
        wp = wp.max(rp);
        let (c, p) = off(&d);
        chest = chest.max(c);
        pig = pig.max(p);
        if i as f32 / HZ > mid {
            chest_late = chest_late.max(c);
            pig_late = pig_late.max(p);
        }
    }
    assert!(
        chest <= wc * 1.05 + 0.05,
        "chest after stop {chest} px exceeds walking {wc} px"
    );
    assert!(
        pig <= wp * 1.05 + 0.05,
        "pigtails after stop {pig} px exceeds walking {wp} px"
    );
    assert!(
        chest_late < 1.2,
        "chest after the turn midpoint {chest_late} px"
    );
    assert!(
        pig_late < 3.0,
        "pigtails after the turn midpoint {pig_late} px"
    );
}

// ---------------------------------------------------------------- gait (v3)

#[test]
fn arms_counter_swing_the_same_side_leg() {
    let mut d = walk_driver(false);
    let cyc = d.config().walk.demo_cycle_s;
    let amp = gait(d.config()).arm_amplitude_deg;
    for _ in 0..(HZ * cyc) as usize {
        tick(&mut d);
    }
    let (mut ra, mut rl, mut la, mut ll) = (vec![], vec![], vec![], vec![]);
    for _ in 0..(HZ * 2.0 * cyc) as usize {
        tick(&mut d);
        let g = d.gait();
        ra.push(g.right_arm_deg);
        rl.push(g.right_leg_fwd);
        la.push(g.left_arm_deg);
        ll.push(g.left_leg_fwd);
    }
    assert!(
        corr(&ra, &rl) < -0.85,
        "right arm vs right leg corr {}",
        corr(&ra, &rl)
    );
    assert!(
        corr(&la, &ll) < -0.85,
        "left arm vs left leg corr {}",
        corr(&la, &ll)
    );
    assert!(
        corr(&ra, &la) < -0.85,
        "left vs right arm corr {}",
        corr(&ra, &la)
    );
    assert!(
        corr(&ra, &ll) > 0.8,
        "right arm vs left leg corr {}",
        corr(&ra, &ll)
    );
    let span =
        ra.iter().cloned().fold(f32::MIN, f32::max) - ra.iter().cloned().fold(f32::MAX, f32::min);
    assert!(
        span > 1.6 * amp && span < 2.2 * amp,
        "arm swing span {span} deg (amp {amp})"
    );
}

#[test]
fn arms_and_twist_settle_after_the_walk_stops() {
    let mut d = walk_driver(false);
    for _ in 0..(4 * 120) {
        tick(&mut d);
    }
    assert!(d.gait().gait_amp > 0.99);
    d.set_motion(Motion::Idle);
    let mut peak_after = 0f32;
    let mut settled_at = None;
    for i in 0..(3 * 120) {
        tick(&mut d);
        let g = d.gait();
        let m = [
            g.left_arm_deg,
            g.right_arm_deg,
            g.left_forearm_deg,
            g.right_forearm_deg,
            g.torso_twist_deg,
        ]
        .iter()
        .fold(0f32, |a, v| a.max(v.abs()));
        peak_after = peak_after.max(m);
        if m >= 0.5 {
            settled_at = None;
        } else if settled_at.is_none() {
            settled_at = Some(i as f32 / HZ);
        }
    }
    let g = d.gait();
    assert_eq!(g.gait_amp, 0.0, "stride weight must ramp out");
    assert!(g.hip_bob_px.abs() < 1e-6 && g.left_leg_fwd.abs() < 1e-6);
    let s = settled_at.expect("arms never settled");
    assert!(s < 1.3, "arms/forearms/twist settle {s} s after stop");
    assert!(
        peak_after < 25.0,
        "no overshoot explosion after stop: {peak_after} deg"
    );
}

#[test]
fn two_jiggle_impulses_per_gait_cycle_alternating() {
    let mut d = walk_driver(false);
    let mut m = walk_driver(true);
    let cyc = d.config().walk.demo_cycle_s;
    let n_cyc = 3.0;
    for _ in 0..(HZ * cyc) as usize {
        tick(&mut d);
        tick(&mut m);
    }
    let t0 = d.time();
    let s0 = d.heel_strikes().count();
    let (mut chest, mut pig, mut raw_chest) = (vec![], vec![], 0f32);
    let mut strike_flags = vec![];
    for _ in 0..(HZ * n_cyc * cyc) as usize {
        tick(&mut d);
        tick(&mut m);
        chest.push(d.transform().chest_offset - m.transform().chest_offset);
        pig.push(d.transform().pigtail_offset - m.transform().pigtail_offset);
        raw_chest = raw_chest.max(d.transform().chest_offset.abs());
        strike_flags.push(d.gait().heel_strike);
    }
    let strikes: Vec<(f32, u32)> = d.heel_strikes().skip(s0).collect();
    assert_eq!(
        strikes.len(),
        (2.0 * n_cyc) as usize,
        "two heel strikes per cycle: {strikes:?}"
    );
    // GaitPose::heel_strike reports each one for exactly one tick (1 left / 2 right).
    let flagged: Vec<u8> = strike_flags.iter().copied().filter(|&f| f != 0).collect();
    assert_eq!(flagged.len(), strikes.len());
    for (f, (_, foot)) in flagged.iter().zip(&strikes) {
        assert_eq!(*f, if *foot == 1 { 2 } else { 1 });
    }
    for w in strikes.windows(2) {
        assert_ne!(w[0].1, w[1].1, "feet alternate: {strikes:?}");
        assert!(
            ((w[1].0 - w[0].0) - cyc / 2.0).abs() < 0.02,
            "half-cycle spacing {strikes:?}"
        );
    }
    let mut last_sign = 0f32;
    for (ts, _) in &strikes {
        let i0 = ((ts - t0) * HZ) as usize;
        if i0 + 48 > chest.len() {
            continue;
        }
        let pk = chest[i0..i0 + 30].iter().cloned().fold(0f32, f32::max);
        assert!(pk > 0.5, "chest bounce after strike at {ts}: {pk} px");
        let pv = pig[i0..i0 + 48]
            .iter()
            .cloned()
            .fold(0f32, |a, v| if v.abs() > a.abs() { v } else { a });
        assert!(pv.abs() > 0.5, "pigtail kick after strike at {ts}: {pv}");
        assert!(
            last_sign == 0.0 || pv.signum() != last_sign,
            "pigtail bias alternates (strike {ts}: {pv})"
        );
        last_sign = pv.signum();
    }
    assert!(
        raw_chest < 3.0,
        "chest stays under the 3 px visual cap: {raw_chest}"
    );
}

#[test]
fn heel_strike_history_is_bounded() {
    let mut d = walk_driver(true);
    for _ in 0..(120 * 60) {
        tick(&mut d); // 60 s = ~68 strikes
    }
    assert_eq!(d.heel_strikes().count(), 64);
}

// ---------------------------------------------------------------- view turn (v4)

#[test]
fn turn_ramps_are_monotonic_and_gate_the_stride() {
    let cfg = lab();
    let g = gait(&cfg);
    let mut d = driver(&cfg, false);
    assert_eq!(d.gait().turn_progress, 0.0);
    d.set_motion(walk(&cfg));
    let mut last = 0f32;
    let mut t_full = None;
    for i in 0..(120 * 2) {
        tick(&mut d);
        let p = d.gait();
        assert!(p.turn_progress >= last - 1e-6, "turn-in not monotonic");
        last = p.turn_progress;
        if p.turn_progress < 1.0 {
            assert_eq!(
                p.gait_amp,
                0.0,
                "stride must wait for the turn (t={})",
                d.time()
            );
            assert_eq!(d.heel_strikes().count(), 0);
            assert!(p.turn_dir >= 0);
        } else if t_full.is_none() {
            t_full = Some((i + 1) as f32 / HZ);
        }
    }
    let tf = t_full.expect("turn-in never completed");
    assert!((tf - g.turn_in_s).abs() < 0.02, "turn-in took {tf} s");
    assert!(d.gait().gait_amp > 0.99);
    d.set_motion(Motion::Idle);
    let (mut amp_last, mut turn_last) = (1.0f32, 1.0f32);
    let (mut t_amp0, mut t_turn0) = (None, None);
    for i in 0..(120 * 2) {
        tick(&mut d);
        let p = d.gait();
        assert!(
            p.gait_amp <= amp_last + 1e-6,
            "stride ramp-out not monotonic"
        );
        assert!(
            p.turn_progress <= turn_last + 1e-6,
            "turn-out not monotonic"
        );
        if p.gait_amp > 0.0 {
            assert_eq!(
                p.turn_progress,
                1.0,
                "turned before the stride stopped (t={})",
                d.time()
            );
        }
        if p.gait_amp == 0.0 && t_amp0.is_none() {
            t_amp0 = Some((i + 1) as f32 / HZ);
        }
        if p.turn_progress == 0.0 && t_turn0.is_none() {
            t_turn0 = Some((i + 1) as f32 / HZ);
        }
        amp_last = p.gait_amp;
        turn_last = p.turn_progress;
    }
    let (a0, t0) = (t_amp0.unwrap(), t_turn0.unwrap());
    assert!((a0 - g.stop_ramp_s).abs() < 0.02, "stride ramp-out {a0} s");
    assert!(
        ((t0 - a0) - g.turn_out_s).abs() < 0.02,
        "turn-out {} s",
        t0 - a0
    );
}

#[test]
fn no_jiggle_spike_at_the_turns() {
    let cfg = lab();
    let g = gait(&cfg);
    let run = |mute: bool| {
        let mut d = driver(&cfg, mute);
        let mut v = vec![];
        for i in 0..(120 * 8) {
            if i == 60 {
                d.set_motion(walk(&cfg));
            }
            if i == 60 + 120 * 5 {
                d.set_motion(Motion::Idle);
            }
            tick(&mut d);
            let t = d.transform();
            v.push((
                d.time(),
                t.chest_offset,
                t.pigtail_offset,
                d.physics().chest.velocity,
            ));
        }
        v
    };
    let (a, b) = (run(false), run(true));
    let win = |lo: f32, hi: f32| -> (f32, f32, f32) {
        let (mut c, mut p, mut cv) = (0f32, 0f32, 0f32);
        for (x, y) in a.iter().zip(&b) {
            if x.0 >= lo && x.0 < hi {
                c = c.max((x.1 - y.1).abs());
                p = p.max((x.2 - y.2).abs());
                cv = cv.max(x.3.abs());
            }
        }
        (c, p, cv)
    };
    let turn_in0 = 0.5;
    let turn_out0 = 5.5 + g.stop_ramp_s;
    let (ci, pi, cvi) = win(turn_in0, turn_in0 + g.turn_in_s + 0.6);
    let (co, po, cvo) = win(turn_out0, turn_out0 + g.turn_out_s + 0.6);
    let (cw, pw, cvw) = win(3.0, 5.0);
    assert!(
        ci < cw && co < cw,
        "chest at turns {ci}/{co} px vs walking step {cw} px"
    );
    assert!(
        pi < pw * 1.5 && po < pw * 1.5,
        "pigtails at turns {pi}/{po} px vs walking {pw} px"
    );
    assert!(
        cvi < cvw && cvo < cvw,
        "chest velocity at turns {cvi}/{cvo} vs walking {cvw}"
    );
    assert!(
        ci > 0.1 && co > 0.1,
        "there is a (small) settle kick at each turn"
    );
}

// ---------------------------------------------------------------- hard cuts + walking tail (v5)

#[test]
fn turn_is_two_hard_cuts_with_a_held_three_quarter_view() {
    let cfg = lab();
    let g = gait(&cfg);
    let mut d = driver(&cfg, false);
    let mut views = vec![];
    for i in 0..(120 * 6) {
        if i == 0 {
            d.set_motion(walk(&cfg));
        }
        if i == 120 * 4 {
            d.set_motion(Motion::Idle);
        }
        tick(&mut d);
        let p = d.overlay().gait;
        assert_eq!(p, d.gait());
        assert_eq!(p.turn_view, turn_view_of(p.turn_progress, &g));
        views.push((d.time(), p.turn_view));
    }
    let changes: Vec<(f32, u8, u8)> = views
        .windows(2)
        .filter(|w| w[0].1 != w[1].1)
        .map(|w| (w[1].0, w[0].1, w[1].1))
        .collect();
    let seq: Vec<(u8, u8)> = changes.iter().map(|c| (c.1, c.2)).collect();
    assert_eq!(
        seq,
        vec![(0, 1), (1, 2), (2, 1), (1, 0)],
        "view sequence {changes:?}"
    );
    let hold_in = changes[1].0 - changes[0].0;
    let hold_out = changes[3].0 - changes[2].0;
    assert!(
        (0.10..=0.15).contains(&hold_in),
        "3/4 held {hold_in} s on the turn-in"
    );
    assert!(
        (0.10..=0.15).contains(&hold_out),
        "3/4 held {hold_out} s on the turn-out"
    );
}

#[test]
fn tail_swings_with_the_gait_and_trails_the_hips() {
    let cfg = lab();
    let run = |mute: bool| {
        let mut d = driver(&cfg, mute);
        d.set_motion(walk(&cfg));
        let mut v = vec![];
        for _ in 0..(120 * 7) {
            tick(&mut d);
            let o = d.overlay();
            v.push((d.time(), o.tail_tip_x, o.gait.tail_sway_px));
        }
        v
    };
    let (a, m) = (run(false), run(true));
    let st: Vec<_> = a.iter().zip(&m).filter(|(x, _)| x.0 >= 3.0).collect();
    let tx: Vec<f32> = st.iter().map(|(x, _)| x.1).collect();
    let sw: Vec<f32> = st.iter().map(|(x, _)| x.2).collect();
    let (lo, hi) = tx
        .iter()
        .fold((f32::MAX, f32::MIN), |(l, h), &x| (l.min(x), h.max(x)));
    assert!(
        lo < -6.0 && hi > 6.0 && lo > -12.5 && hi < 12.5,
        "tail tip x {lo}..{hi}"
    );
    let best = (0..60)
        .map(|l| (l, corr(&sw[..sw.len() - l], &tx[l..])))
        .fold((0, -2.0f32), |b, x| if x.1 > b.1 { x } else { b });
    assert!(best.1 > 0.8, "tail vs gait sway best corr {}", best.1);
    let lag = best.0 as f32 / HZ;
    assert!(
        (0.04..=0.3).contains(&lag),
        "tail trails the sway by {lag} s"
    );
    let flick = st
        .iter()
        .map(|(x, y)| (x.1 - y.1).abs())
        .fold(0.0f32, f32::max);
    assert!(flick > 1.5, "heel-strike flick adds only {flick} px");
}

// ---------------------------------------------------------------- bundled gait switch + baked-gait clips

/// Walks `d` for `secs` and returns (max |root y|, max |arm|, max |hip bob|,
/// heel strikes seen, last gait pose).
fn walk_for(
    d: &mut PhysicsDriver,
    secs: f32,
) -> (f32, f32, f32, usize, lillith_core::physics_driver::GaitPose) {
    let (mut y, mut arm, mut bob) = (0f32, 0f32, 0f32);
    for _ in 0..(secs * HZ) as usize {
        tick(d);
        let g = d.gait();
        y = y.max(d.transform().y.abs());
        arm = arm.max(g.left_arm_deg.abs()).max(g.right_arm_deg.abs());
        bob = bob.max(g.hip_bob_px.abs());
    }
    (y, arm, bob, d.heel_strikes().count(), d.gait())
}

fn bundled_walk(cfg: &PhysicsConfig) -> Motion {
    // The canonical walk_right: 14 frames @ 12 fps.
    Motion::Walking {
        vx: cfg.walk.demo_speed_px_s,
        cycle_s: 14.0 / 12.0,
    }
}

#[test]
fn bundled_gait_off_walk_has_no_root_bob_or_gait() {
    let cfg = PhysicsConfig::bundled();
    let mut d = driver(&cfg, false);
    d.set_motion(bundled_walk(&cfg));
    let (y, arm, bob, strikes, g) = walk_for(&mut d, 3.0);
    assert!(!d.gait_enabled());
    // walk_footfall.root_y = 0: the baked frames own the vertical step.
    assert!(y < 0.05, "root_y bob on a baked walk {y}");
    assert_eq!((arm, bob, strikes, g.gait_amp), (0.0, 0.0, 0, 0.0));
    assert!(d.transform().chest_offset.abs() + d.transform().pigtail_offset.abs() > 0.0);
}

#[test]
fn gait_on_baked_clip_is_suppressed_like_gait_off() {
    let on = bundled_gait_on();
    let mut sup = driver(&on, false);
    sup.set_gait_suppressed(true); // animator: walk_right has disable_springs = ["gait"]
    assert!(sup.gait_enabled() && sup.gait_suppressed() && !sup.gait_active());
    let mut off = driver(&PhysicsConfig::bundled(), false);
    let w = bundled_walk(&on);
    sup.set_motion(w);
    off.set_motion(w);
    for _ in 0..(3 * 120) {
        tick(&mut sup);
        tick(&mut off);
        // Identical to gait off: same kicks, no gait outputs, no extra bob.
        assert_eq!(sup.transform(), off.transform());
        assert_eq!(sup.gait(), off.gait());
        assert_eq!(sup.overlay().tail_tip_x, off.overlay().tail_tip_x);
    }
    let g = sup.gait();
    assert_eq!(
        (g.gait_amp, g.hip_bob_px, g.left_arm_deg, g.torso_twist_deg, g.tail_sway_px),
        (0.0, 0.0, 0.0, 0.0, 0.0)
    );
    assert_eq!(sup.heel_strikes().count(), 0);
}

#[test]
fn gait_on_non_baked_clip_is_driven() {
    let on = bundled_gait_on();
    let mut d = driver(&on, false);
    d.set_motion(bundled_walk(&on));
    assert!(d.gait_active());
    let (y, arm, bob, strikes, g) = walk_for(&mut d, 3.0);
    assert!(g.gait_amp > 0.99, "stride ramped in {}", g.gait_amp);
    assert!(arm > 10.0, "arms swing {arm}");
    assert!(bob > 2.0, "hip bob {bob}");
    assert!(strikes >= 3, "heel strikes {strikes}");
    // The hip bob is the ONLY vertical step motion: no root_y footfall kick
    // on top (walk_footfall.root_y = 0), so no double bob.
    assert!(y < 0.05, "root_y moved {y} on top of the hip bob");
}

#[test]
fn baked_gait_toggle_mid_walk_parks_and_resumes() {
    let on = bundled_gait_on();
    let mut d = driver(&on, false);
    d.set_motion(bundled_walk(&on));
    walk_for(&mut d, 2.0);
    assert!(d.gait_amp() > 0.99);
    // Walk -> strut swap to a baked clip: everything parks at once.
    d.set_gait_suppressed(true);
    let g = d.gait();
    assert_eq!((g.gait_amp, g.left_arm_deg, g.hip_bob_px), (0.0, 0.0, 0.0));
    assert_eq!(g.turn_view, 2, "still the side view while walking");
    walk_for(&mut d, 0.5);
    assert_eq!(d.gait().right_forearm_deg, 0.0);
    // Back to a non-baked clip mid-walk: no re-turn, the stride ramps in.
    d.set_gait_suppressed(false);
    tick(&mut d);
    assert_eq!(d.gait().turn_progress, 1.0);
    let (_, arm, _, _, g) = walk_for(&mut d, 1.0);
    assert!(g.gait_amp > 0.99 && arm > 10.0, "amp {} arm {arm}", g.gait_amp);
    // Runtime switch behaves the same way.
    d.set_gait_enabled(false);
    assert!(!d.gait_enabled() && !d.gait_active());
    assert_eq!(d.gait().gait_amp, 0.0);
    d.set_gait_enabled(true);
    assert!(d.gait_active());
}

#[test]
fn gait_on_with_frame_footfalls_does_not_double_the_steps() {
    let on = bundled_gait_on();
    let mut d = driver(&on, false);
    d.set_frame_footfalls(true); // the animator feeds contact frames
    d.set_motion(bundled_walk(&on));
    let (_, arm, _, strikes, g) = walk_for(&mut d, 2.0);
    assert!(g.gait_amp > 0.99 && arm > 10.0, "gait still drives the body");
    assert_eq!(strikes, 0, "no phase-derived heel strikes on top of contact frames");
    d.on_footfall(1);
    assert_eq!(d.heel_strikes().count(), 1);
}
