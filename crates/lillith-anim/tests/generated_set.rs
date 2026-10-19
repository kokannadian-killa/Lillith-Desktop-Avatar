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
    assert_eq!(a.base(), "idle");
    assert!(
        !a.blink_frames().is_empty(),
        "blink_wink gives blink frames"
    );
    a.walk(Dir::Left, false).unwrap();
    assert_eq!(a.current().clip_name, "walk_left");
    a.walk(Dir::Right, true).unwrap();
    assert_eq!(a.current().clip_name, "run_right");
    a.stand();
    for name in m.clips.keys() {
        a.react(name, 1).unwrap();
    }
    // Play the whole queue: every clip shows up as a reaction.
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..(60 * 60 * 10) {
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
}
