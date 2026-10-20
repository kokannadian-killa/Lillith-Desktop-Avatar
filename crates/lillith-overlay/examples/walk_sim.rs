//! Headless wander simulation for checking foot placement.
//!
//! Runs the real `Controller` (brain + animator + physics driver) at a fixed
//! tick rate with Wander on, and dumps what would be on screen: one line per
//! tick in `<out>/ticks.csv` (time, window left, clip, frame, flip, phase)
//! and the rendered RGBA canvas of every tick appended to `<out>/frames.rgba`
//! (premultiplied, `w*h*4` bytes each). Pair with a script that composites
//! the canvases at their window x to measure planted-foot drift.
//!
//! ```text
//! cargo run -p lillith-overlay --example walk_sim -- \
//!     assets/generated/manifest.toml /tmp/walk_sim [seconds] [scale] [hz]
//! ```

use lillith_anim::animator::AnimatorConfig;
use lillith_core::config::{MoveMode, OverlayPos};
use lillith_core::state::BehaviorConfig;
use lillith_overlay::run::{Controller, OverlayOptions};
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let manifest = PathBuf::from(args.get(1).expect("manifest path"));
    let out = PathBuf::from(args.get(2).expect("output dir"));
    let secs: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(30.0);
    let scale: f32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0.55);
    let hz: f32 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(60.0);
    std::fs::create_dir_all(&out).expect("output dir");

    let behavior = BehaviorConfig {
        wander_every_s: (0.8, 1.2),
        pose_cycle_s: 1.0e6,
        sleepy_after_s: 1.0e6,
        asleep_after_s: 1.0e6,
        ..BehaviorConfig::default()
    };
    let opts = OverlayOptions {
        manifest,
        scale,
        opacity: 1.0,
        render_hz: hz,
        animator: AnimatorConfig {
            seed: 7,
            physics_driver: true,
            idle_enabled: false,
            ..AnimatorConfig::default()
        },
        position: OverlayPos {
            margin_left: 0,
            margin_bottom: 0,
            output: None,
        },
        config_path: None,
        forced: false,
        exit_after: None,
        behavior,
        mode: MoveMode::Wander,
        muted: true,
        events: None,
    };
    let mut c = Controller::new(&opts).expect("controller");
    let (w, h) = c.stage.size();
    c.set_screen((w + 700, 800));
    let mut csv = std::fs::File::create(out.join("ticks.csv")).unwrap();
    let mut raw = std::io::BufWriter::new(std::fs::File::create(out.join("frames.rgba")).unwrap());
    writeln!(csv, "# w={w} h={h} hz={hz} scale={scale}").unwrap();
    writeln!(csv, "t,left,clip,frame,flip,phase").unwrap();
    let dt = 1.0 / hz;
    let n = (secs * hz) as usize;
    for i in 0..n {
        c.tick(dt).unwrap();
        if let Some(p) = c.take_move() {
            c.placement = p;
        }
        c.stage.render().unwrap();
        let f = c.stage.animator().current();
        writeln!(
            csv,
            "{:.4},{},{},{},{},{:?}",
            (i + 1) as f32 * dt,
            c.placement.left,
            f.clip_name,
            f.frame,
            f.flip as u8,
            c.brain.phase()
        )
        .unwrap();
        raw.write_all(c.stage.canvas().data()).unwrap();
    }
}
