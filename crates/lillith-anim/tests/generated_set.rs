//! Smoke test against the real canonical generated set, when it is present.
//!
//! The art is not in git yet (assets/README.md), so this test looks for
//! `$LILLITH_MANIFEST`, then `assets/generated/manifest.toml`, then the
//! build-box path, and is skipped (passes) if none exists.

use lillith_anim::animator::{Animator, AnimatorConfig, Dir, Layer};
use lillith_anim::atlas::Manifest;
use lillith_anim::preview::{clip_frames, PreviewOpts};
use lillith_anim::render::PageCache;
use std::path::PathBuf;

fn find_manifest() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::env::var_os("LILLITH_MANIFEST")
        .map(PathBuf::from)
        .into_iter()
        .chain([
            root.join("assets/generated/manifest.toml"),
            PathBuf::from("/workspace/lillith/assets/generated/manifest.toml"),
        ])
        .find(|p| p.is_file())
}

#[test]
fn every_generated_clip_plays_and_renders() {
    let Some(path) = find_manifest() else {
        eprintln!("generated manifest not found; skipping");
        return;
    };
    let m = Manifest::load(&path).unwrap();
    assert!(m.clips.len() >= 64, "{} clips", m.clips.len());
    let mut pages = PageCache::new(path.parent().unwrap());
    let opts = PreviewOpts {
        cell: 64,
        ..PreviewOpts::default()
    };
    for name in m.clips.keys() {
        let frames = clip_frames(&m, &mut pages, name, &opts).unwrap();
        assert_eq!(frames.len(), m.clips[name].frames.len(), "{name}");
        assert!(
            frames
                .iter()
                .any(|f| f.canvas.pixels().iter().any(|p| p.alpha() > 0)),
            "{name}: blank"
        );
    }

    let mut a = Animator::new(m.clone(), AnimatorConfig::default()).unwrap();
    // v2 baked_motion clips: engine chest / pigtail / hip jiggle off (the
    // video already has it), via the same SpringMask as disable_springs.
    for (name, clip) in m.clips.iter().filter(|(_, c)| c.has_tag("baked_motion")) {
        let mask = a.clip_mask(clip);
        for ch in [
            lillith_core::spring::Channel::Chest,
            lillith_core::spring::Channel::Pigtail,
            lillith_core::spring::Channel::RootX,
        ] {
            assert!(mask.contains(ch), "{name}: {ch:?} not masked");
        }
    }
    assert_eq!(a.base(), "idle");
    assert!(
        !a.blink_frames().is_empty(),
        "blink_wink gives blink frames"
    );
    // Baked-walk clips (video walk/strut/run takes + their idle blends,
    // plus the batch-13 walk variants and the batch-16 stalk prowls)
    // suppress the engine gait; nothing else does.
    let mut baked: Vec<&str> = m
        .clips
        .iter()
        .filter(|(_, c)| c.bakes_gait())
        .map(|(n, _)| n.as_str())
        .collect();
    baked.sort_unstable();
    assert_eq!(
        baked,
        [
            "blend_idle_to_walk_left",
            "blend_idle_to_walk_right",
            "blend_walk_left_to_idle",
            "blend_walk_right_to_idle",
            "run_left",
            "run_right",
            "stalk_prowl_left",
            "stalk_prowl_right",
            "strut_left",
            "strut_right",
            "walk_left",
            "walk_right",
            "walk_sassy_left",
            "walk_sassy_right",
            "walk_saunter_left",
            "walk_saunter_right",
            "walk_skip_left",
            "walk_skip_right",
        ]
    );
    for n in &baked {
        assert!(m.clips[*n].spring_mask().is_empty(), "{n}: gait only");
    }
    a.walk(Dir::Left, false).unwrap();
    // Starting from idle plays the turn blend first, then hands over to the
    // walk on the frame the blend leads into.
    assert_eq!(a.base(), "walk_left");
    assert_eq!(a.current().clip_name, "blend_idle_to_walk_left");
    for _ in 0..60 {
        a.tick(1.0 / 60.0);
    }
    assert_eq!(a.current().clip_name, "walk_left");
    assert!(a.take_root_motion() < 0.0, "walking left moves left");
    a.walk(Dir::Right, true).unwrap();
    assert_eq!(a.current().clip_name, "run_right");
    a.stand();
    for name in m.clips.keys() {
        a.react(name, 1).unwrap();
    }
    // Play the whole queue: every clip shows up as a reaction.
    let mut seen = std::collections::BTreeSet::new();
    // Budget: every clip once at the default playback speed, plus slack
    // (the slowed art runs ~20 min of reactions).
    let total_s: f32 = m.clips.values().map(|c| c.duration_s()).sum::<f32>()
        / AnimatorConfig::default().playback_speed.clamp(0.1, 4.0);
    for _ in 0..((total_s * 1.2 + 60.0) * 60.0) as usize {
        a.tick(1.0 / 60.0);
        let f = a.current();
        if f.layer == Layer::Reaction {
            seen.insert(f.clip_name.to_string());
        }
        if a.overlay_clip().is_none() && a.queued() == 0 && seen.len() >= m.clips.len() {
            break;
        }
    }
    assert_eq!(seen.len(), m.clips.len());

    // Baked-jiggle mode: every reaction with a jiggle sibling draws all of
    // the sibling's own frames (settle sub-frames included).
    let mut j = Animator::new(
        m.clone(),
        AnimatorConfig {
            prefer_jiggle_variants: true,
            idle_enabled: false,
            ..AnimatorConfig::default()
        },
    )
    .unwrap();
    for (name, clip) in &m.clips {
        let Some(v) = clip.jiggle_variant.as_deref() else {
            continue;
        };
        if clip.mode != lillith_anim::atlas::LoopMode::Once {
            continue;
        }
        j.clear_overlays();
        j.react(name, 1).unwrap();
        let mut frames = std::collections::BTreeSet::new();
        for _ in 0..(60 * 60) {
            if j.overlay_clip().is_none() {
                break;
            }
            let f = j.current();
            assert_eq!(f.clip_name, v);
            assert!(f.clean_frame() < clip.frames.len(), "{v}");
            frames.insert(f.frame);
            j.tick(1.0 / 60.0);
        }
        assert_eq!(frames.len(), m.clips[v].frames.len(), "{v}");
    }
}
