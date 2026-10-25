//! Drives the Android engine over the real generated clip set, the way the
//! overlay service does (60 Hz ticks, touches in screen px).

use lillith_android::engine::{Engine, EngineOptions, Touch, REQ_MENU};
use std::path::PathBuf;

const DT: f32 = 1.0 / 60.0;
const SCREEN: (u32, u32) = (1080, 2200);

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/generated/manifest.toml")
}

fn engine(density: f32) -> Engine {
    let mut o = EngineOptions::new(manifest(), density);
    o.seed = 7;
    let mut e = Engine::load(o).expect("engine loads the generated set");
    e.set_screen(SCREEN.0, SCREEN.1);
    e
}

fn run(e: &mut Engine, secs: f32) -> usize {
    let mut changed = 0;
    for _ in 0..(secs / DT) as usize {
        if e.tick(DT) {
            changed += 1;
        }
    }
    changed
}

/// A screen point on her body (opaque pixel), searching from the centre.
fn on_body(e: &Engine) -> (f32, f32) {
    let (w, h) = e.size();
    let (l, t) = e.position();
    for dy in (0..h / 2).step_by(4) {
        for dx in (0..w / 2).step_by(4) {
            for (x, y) in [
                (w / 2 + dx, h / 2 + dy),
                (w / 2 - dx, h / 2 + dy),
                (w / 2 + dx, h / 2 - dy),
            ] {
                if e.hit(x as f32, y as f32) {
                    return ((l + x as i32) as f32, (t + y as i32) as f32);
                }
            }
        }
    }
    panic!("no opaque pixel in the frame");
}

#[test]
fn default_scale_matches_desktop_in_dp() {
    let e = engine(1.0);
    // 400 px cells × 0.55 = 220 px at density 1 (the desktop default size).
    assert_eq!(e.size(), (220, 220));
    let e3 = engine(3.0);
    assert_eq!(e3.size(), (660, 660));
}

#[test]
fn renders_and_stands_on_the_bottom_of_the_screen() {
    let mut e = engine(2.0);
    assert!(e.tick(DT), "first tick draws");
    assert!(
        e.pixels().iter().skip(3).step_by(4).any(|&a| a > 200),
        "opaque pixels"
    );
    let (_, top) = e.position();
    assert_eq!(top as f32, e.floor_top().round());
    // She keeps animating (idle life) and stays on screen.
    let frames = run(&mut e, 20.0);
    assert!(frames > 20, "only {frames} redraws in 20 s");
    let (l, t) = e.position();
    let (w, _) = e.size();
    assert!(l >= 0 && l + w as i32 <= SCREEN.0 as i32 && t as f32 <= e.floor_top() + 0.5);
    assert!(e.cache_bytes() <= lillith_android::pages::DEFAULT_BUDGET + 64 * 1024 * 1024);
}

#[test]
fn drag_drop_falls_back_to_the_floor() {
    let mut e = engine(2.0);
    run(&mut e, 0.5);
    let (x, y) = on_body(&e);
    assert!(e.touch(Touch::Down, x, y));
    for i in 1..=30 {
        e.touch(Touch::Move, x - i as f32 * 8.0, y - i as f32 * 30.0);
        e.tick(DT);
    }
    assert!(e.dragging());
    let lifted = e.position().1;
    assert!(
        (lifted as f32) < e.floor_top() - 500.0,
        "lifted to {lifted}"
    );
    e.touch(Touch::Up, x - 240.0, y - 900.0);
    assert!(e.falling(), "falls after the drop");
    let mut landed_after = None;
    for i in 0..600 {
        e.tick(DT);
        if !e.falling() {
            landed_after = Some(i);
            break;
        }
    }
    assert!(landed_after.is_some(), "never landed");
    assert_eq!(e.position().1 as f32, e.floor_top().round());
}

#[test]
fn tap_reacts_and_misses_on_transparent_pixels() {
    let mut e = engine(2.0);
    run(&mut e, 0.5);
    let (l, t) = e.position();
    // Top-left corner of the cell is transparent: not her.
    assert!(!e.touch(Touch::Down, l as f32 + 1.0, t as f32 + 1.0));
    let (x, y) = on_body(&e);
    assert!(e.touch(Touch::Down, x, y));
    e.tick(DT);
    assert!(e.touch(Touch::Up, x, y));
    let before = e.base().to_string();
    run(&mut e, 0.3);
    // A tap is a left click for the brain: no drag, no fall.
    assert!(!e.falling() && !e.dragging(), "base {before}");
}

#[test]
fn long_press_asks_for_the_menu() {
    let mut e = engine(2.0);
    run(&mut e, 0.5);
    let (x, y) = on_body(&e);
    e.touch(Touch::Down, x, y);
    run(&mut e, 0.8);
    e.touch(Touch::Up, x, y);
    assert_ne!(e.take_requests() & REQ_MENU, 0);
    assert_eq!(e.take_requests(), 0, "requests are taken once");
}

#[test]
fn rotation_keeps_her_on_screen() {
    let mut e = engine(2.0);
    run(&mut e, 0.2);
    e.set_screen(2200, 1080);
    run(&mut e, 3.0);
    let (l, t) = e.position();
    let (w, _) = e.size();
    assert!(l >= 0 && l + w as i32 <= 2200);
    assert_eq!(t as f32, e.floor_top().round());
}

/// The Android clip pack: clips whose sheets aren't installed yet are left
/// out (with anything pointing at them) and she still loads and plays.
#[test]
fn missing_pack_clips_are_left_out_until_installed() {
    use lillith_android::engine::drop_uninstalled_clips;
    use lillith_android::pages::{Pages, DEFAULT_BUDGET};
    use lillith_anim::atlas::Manifest;
    let full = Manifest::load(&manifest()).unwrap();
    let total = full.clips.len();
    let pack = |page: &str| {
        let c = page.split('/').next().unwrap_or("");
        c.starts_with("valentines_") || c.starts_with("spring_") || c.ends_with("_long")
    };
    let mut m = full.clone();
    let gone = drop_uninstalled_clips(&mut m, |p| !pack(p));
    assert!(gone > 0 && m.clips.len() + gone == total);
    assert!(m.clips.values().all(|c| !pack(&c.page)));
    for c in m.clips.values() {
        for r in [&c.base_clip, &c.jiggle_of, &c.jiggle_variant]
            .into_iter()
            .flatten()
        {
            assert!(m.clips.contains_key(r), "dangling {r}");
        }
        if let Some(b) = &c.blend {
            assert!(m.clips.contains_key(&b.from) && m.clips.contains_key(&b.to));
        }
    }
    // A blend whose source is missing goes too.
    let mut m2 = full.clone();
    let (name, from) = full
        .clips
        .iter()
        .find_map(|(n, c)| c.blend.as_ref().map(|b| (n.clone(), b.from.clone())))
        .expect("the set has blend clips");
    let from_page = full.clips[&from].page.clone();
    drop_uninstalled_clips(&mut m2, |p| p != from_page);
    assert!(!m2.clips.contains_key(&name) && !m2.clips.contains_key(&from));
    // Nothing missing: nothing dropped.
    let mut m3 = full.clone();
    assert_eq!(drop_uninstalled_clips(&mut m3, |_| true), 0);

    let dir = manifest().parent().unwrap().to_path_buf();
    let mut o = EngineOptions::new(manifest(), 1.0);
    o.seed = 7;
    let mut e =
        Engine::new(m, Pages::new(dir, DEFAULT_BUDGET), &o).expect("engine loads the core set");
    e.set_screen(SCREEN.0, SCREEN.1);
    assert!(run(&mut e, 2.0) > 0);
}
