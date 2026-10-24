//! Ko's own clips (Pose Builder → Import Video): approved imports live in
//! `<data>/lillith/user-clips/` (`~/.local/share/lillith/user-clips`,
//! Windows `%LOCALAPPDATA%\lillith\user-clips`) next to a
//! `user_manifest.toml` in the normal [`Manifest`] format, with paths
//! relative to that folder.
//!
//! - [`merge_user_clips`] adds them to a loaded built-in set. It is
//!   add-only: a user clip whose name is already taken (built-in or an
//!   earlier user clip) is skipped, never replaces anything.
//! - [`install_clip`] copies one approved candidate in. It never
//!   overwrites: a taken name gets a `_2`, `_3`… suffix, the clip folder is
//!   created with `create_dir` and every file with `create_new`.
//!
//! `LILLITH_USER_CLIPS` overrides the folder; `LILLITH_USER_CLIPS=0` turns
//! the merge off.

use crate::atlas::{AtlasError, Clip, FrameRect, LoopMode, Manifest};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The user clips' manifest file name.
pub const USER_MANIFEST: &str = "user_manifest.toml";
/// Tag every imported clip carries.
pub const USER_TAG: &str = "user";
/// Longest clip name kept (before a collision suffix).
const MAX_NAME: usize = 48;

/// `<data>/lillith/user-clips` (or `$LILLITH_USER_CLIPS`); `None` when
/// turned off (`LILLITH_USER_CLIPS=0`) or there is no data dir.
pub fn user_clips_dir() -> Option<PathBuf> {
    match std::env::var_os("LILLITH_USER_CLIPS").filter(|v| !v.is_empty()) {
        Some(v) if v == "0" => None,
        Some(v) => Some(PathBuf::from(v)),
        None => lillith_core::config::data_dir().map(|d| d.join("user-clips")),
    }
}

/// Loads `manifest_path` and merges the user clips (from
/// [`user_clips_dir`]) into it. A broken user manifest is logged and
/// ignored: the built-in set always loads.
pub fn load_with_user_clips(manifest_path: &Path) -> Result<Manifest, AtlasError> {
    let mut m = Manifest::load(manifest_path)?;
    if let Some(dir) = user_clips_dir() {
        let added = merge_user_clips(&mut m, &dir);
        if !added.is_empty() {
            tracing::info!("user clips: added {} from {}", added.len(), dir.display());
        }
    }
    Ok(m)
}

/// Reads `<dir>/user_manifest.toml` (empty when missing).
pub fn load_user_manifest(dir: &Path) -> Result<Manifest, AtlasError> {
    let p = dir.join(USER_MANIFEST);
    if !p.is_file() {
        return Ok(Manifest::default());
    }
    Manifest::load(&p)
}

/// Adds the user clips in `dir` to `m`, add-only: names already in `m` are
/// skipped (logged). Their `page` / `video` paths are made absolute so they
/// resolve from the built-in manifest's folder. Returns the added names.
pub fn merge_user_clips(m: &mut Manifest, dir: &Path) -> Vec<String> {
    let user = match load_user_manifest(dir) {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!("user clips: {}: {e}", dir.join(USER_MANIFEST).display());
            return Vec::new();
        }
    };
    let mut added = Vec::new();
    for (name, mut clip) in user.clips {
        if m.clips.contains_key(&name) {
            tracing::warn!("user clips: `{name}` is taken by an existing clip, skipped");
            continue;
        }
        clip.page = dir.join(&clip.page).display().to_string();
        clip.video = clip.video.map(|v| dir.join(v).display().to_string());
        m.clips.insert(name.clone(), clip);
        added.push(name);
    }
    added
}

/// Turns a typed name into a clip name: lower-case `a-z 0-9 _`, other runs
/// become `_`, at most 48 chars; `clip` when nothing is left.
pub fn sanitize_name(s: &str) -> String {
    let mut out = String::new();
    for ch in s.trim().chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    let mut out: String = out.trim_matches('_').chars().take(MAX_NAME).collect();
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        "clip".into()
    } else {
        out
    }
}

/// The first of `base`, `base_2`, `base_3`… not in `taken` and with no
/// folder of that name in `dir`.
pub fn unique_name(base: &str, taken: &dyn Fn(&str) -> bool, dir: &Path) -> String {
    let free = |n: &str| !taken(n) && !dir.join(n).exists();
    if free(base) {
        return base.to_string();
    }
    (2..)
        .map(|i| format!("{base}_{i}"))
        .find(|n| free(n))
        .expect("unbounded")
}

/// One approved candidate to copy in.
#[derive(Debug, Clone)]
pub struct NewClip<'a> {
    /// Typed name (sanitized here).
    pub name: &'a str,
    /// The packed MP4 in staging.
    pub mp4: &'a Path,
    /// The preview PNG in staging (also the clip's still fallback page).
    pub preview_png: &'a Path,
    pub frames: u32,
    pub fps: f32,
}

/// Copies `c` into `dir` add-only and appends its entry to
/// `user_manifest.toml`. `builtin` reports names of the built-in set (never
/// replaced). Returns the name it was saved under (suffixed on a
/// collision).
pub fn install_clip(
    dir: &Path,
    c: &NewClip<'_>,
    builtin: &dyn Fn(&str) -> bool,
) -> Result<String, AtlasError> {
    let bad = |reason: String| AtlasError::InvalidClip {
        clip: c.name.to_string(),
        reason,
    };
    if c.frames == 0 || c.frames > 100_000 {
        return Err(bad(format!("{} frames", c.frames)));
    }
    let fps = if c.fps.is_finite() && c.fps > 0.0 {
        c.fps.min(120.0)
    } else {
        12.0
    };
    // Cell size from the preview's header only (never decoded in full).
    let (w, h) = image::image_dimensions(c.preview_png)?;
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return Err(bad(format!("preview is {w}x{h}")));
    }
    std::fs::create_dir_all(dir)?;
    let mut manifest = load_user_manifest(dir)?;
    let base = sanitize_name(c.name);
    // create_dir is the add-only lock: if another writer took the folder
    // between the check and here, move on to the next suffix.
    let mut name;
    let mut tries = 0;
    loop {
        name = unique_name(
            &base,
            &|n| builtin(n) || manifest.clips.contains_key(n),
            dir,
        );
        match std::fs::create_dir(dir.join(&name)) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && tries < 1000 => {
                tries += 1;
            }
            Err(e) => return Err(e.into()),
        }
    }
    let clip_dir = dir.join(&name);
    let (mp4_rel, png_rel) = (
        format!("{name}/{name}.mp4"),
        format!("{name}/{name}_preview.png"),
    );
    let copied = copy_new(c.mp4, &dir.join(&mp4_rel))
        .and_then(|_| copy_new(c.preview_png, &dir.join(&png_rel)));
    if let Err(e) = copied {
        // Only what this call created (a fresh folder) is removed.
        let _ = std::fs::remove_dir_all(&clip_dir);
        return Err(e.into());
    }
    let anchor = (w as i32 / 2, h as i32 - 8);
    let clip = Clip {
        page: png_rel,
        fps,
        mode: LoopMode::Loop,
        video: Some(mp4_rel),
        tags: vec![USER_TAG.into()],
        frames: (0..c.frames)
            .map(|_| FrameRect {
                x: 0,
                y: 0,
                w,
                h,
                anchor,
            })
            .collect(),
        ..toml::from_str::<Clip>("page = \"\"\nframes = []").expect("minimal clip")
    };
    clip.validate(&name)?;
    manifest.clips.insert(name.clone(), clip);
    write_manifest(dir, &manifest)?;
    Ok(name)
}

/// Copies `from` to a NEW file `to` (`create_new`: an existing file is an
/// error, never overwritten), streaming.
fn copy_new(from: &Path, to: &Path) -> std::io::Result<u64> {
    let mut src = std::fs::File::open(from)?;
    let mut dst = OpenOptions::new().write(true).create_new(true).open(to)?;
    let n = std::io::copy(&mut src, &mut dst)?;
    dst.sync_all()?;
    Ok(n)
}

/// Writes `user_manifest.toml` through a temp file + rename, so a crash
/// never leaves half a manifest. Only entries are ever added to it.
fn write_manifest(dir: &Path, m: &Manifest) -> Result<(), AtlasError> {
    let body = toml::to_string_pretty(m)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = dir.join(format!(".{USER_MANIFEST}.{}.tmp", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(
            b"# Ko's imported clips (Pose Builder). Add-only: Lillith never replaces a clip.\n\n",
        )?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, dir.join(USER_MANIFEST))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("lillith-userclips-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn staged(dir: &Path) -> (PathBuf, PathBuf) {
        let mp4 = dir.join("cand.mp4");
        std::fs::write(&mp4, b"not really a video").unwrap();
        let png = dir.join("cand.png");
        image::RgbaImage::new(40, 30).save(&png).unwrap();
        (mp4, png)
    }

    const BUILTIN: &str = r#"
[clips.idle]
page = "idle/idle.png"
frames = [ { x = 0, y = 0, w = 40, h = 30 } ]
"#;

    #[test]
    fn names_are_sanitized() {
        assert_eq!(sanitize_name("  My Hot Pose!.mp4 "), "my_hot_pose_mp4");
        assert_eq!(sanitize_name("../../etc"), "etc");
        assert_eq!(sanitize_name("***"), "clip");
        assert!(sanitize_name(&"a".repeat(200)).len() <= MAX_NAME);
    }

    #[test]
    fn install_is_add_only_and_suffixes_collisions() {
        let root = tmp("install");
        let (stage, user) = (root.join("stage"), root.join("user-clips"));
        std::fs::create_dir_all(&stage).unwrap();
        let (mp4, png) = staged(&stage);
        let builtin = Manifest::from_toml_str(BUILTIN).unwrap();
        let is_builtin = |n: &str| builtin.clips.contains_key(n);
        let new = |name| NewClip {
            name,
            mp4: &mp4,
            preview_png: &png,
            frames: 3,
            fps: 24.0,
        };
        // A built-in name is never replaced: it gets a suffix.
        assert_eq!(
            install_clip(&user, &new("idle"), &is_builtin).unwrap(),
            "idle_2"
        );
        assert_eq!(
            install_clip(&user, &new("Wave"), &is_builtin).unwrap(),
            "wave"
        );
        // An existing user clip is never replaced either.
        let before = std::fs::read(user.join("wave/wave.mp4")).unwrap();
        assert_eq!(
            install_clip(&user, &new("wave"), &is_builtin).unwrap(),
            "wave_2"
        );
        assert_eq!(std::fs::read(user.join("wave/wave.mp4")).unwrap(), before);
        // A stray folder of that name (no manifest entry) is skipped too.
        std::fs::create_dir_all(user.join("wave_3")).unwrap();
        assert_eq!(
            install_clip(&user, &new("wave"), &is_builtin).unwrap(),
            "wave_4"
        );
        let um = load_user_manifest(&user).unwrap();
        let names: Vec<&String> = um.clips.keys().collect();
        assert_eq!(names, ["idle_2", "wave", "wave_2", "wave_4"]);
        let c = &um.clips["wave"];
        assert_eq!(c.frames.len(), 3);
        assert_eq!((c.frames[0].w, c.frames[0].h), (40, 30));
        assert_eq!(c.video.as_deref(), Some("wave/wave.mp4"));
        assert!(c.tags.iter().any(|t| t == USER_TAG));
        assert!(user.join("wave_2/wave_2_preview.png").is_file());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn copy_never_overwrites() {
        let root = tmp("copy");
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::write(&a, b"new").unwrap();
        std::fs::write(&b, b"old").unwrap();
        assert!(copy_new(&a, &b).is_err());
        assert_eq!(std::fs::read(&b).unwrap(), b"old");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_loader_merges_user_clips_add_only() {
        let root = tmp("merge");
        let (stage, user) = (root.join("stage"), root.join("user-clips"));
        std::fs::create_dir_all(&stage).unwrap();
        let (mp4, png) = staged(&stage);
        let none = |_: &str| false;
        let new = |name| NewClip {
            name,
            mp4: &mp4,
            preview_png: &png,
            frames: 2,
            fps: 12.0,
        };
        install_clip(&user, &new("idle"), &none).unwrap(); // clashes with built-in idle
        install_clip(&user, &new("strut"), &none).unwrap();
        let mut m = Manifest::from_toml_str(BUILTIN).unwrap();
        let builtin_idle = m.clips["idle"].clone();
        let added = merge_user_clips(&mut m, &user);
        assert_eq!(added, ["strut"]);
        assert_eq!(m.clips["idle"], builtin_idle, "the built-in clip is kept");
        let s = &m.clips["strut"];
        let page = PathBuf::from(&s.page);
        assert!(page.is_absolute() && page.is_file(), "{page:?}");
        assert_eq!(
            s.video_path(Path::new("/elsewhere")),
            Some(user.join("strut/strut.mp4"))
        );
        // Missing folder or broken manifest: nothing added, nothing fails.
        let mut m2 = Manifest::from_toml_str(BUILTIN).unwrap();
        assert!(merge_user_clips(&mut m2, &root.join("nope")).is_empty());
        std::fs::write(user.join(USER_MANIFEST), "clips = 5").unwrap();
        assert!(merge_user_clips(&mut m2, &user).is_empty());
        assert_eq!(m2.clips.len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }
}
