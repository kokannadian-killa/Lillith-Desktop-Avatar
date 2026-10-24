//! Ko's own clips (Pose Builder → Import Video): approved imports live in
//! `<data>/lillith/user-clips/` (`~/.local/share/lillith/user-clips`,
//! Windows `%LOCALAPPDATA%\lillith\user-clips`) next to a
//! `user_manifest.toml` in the normal [`Manifest`] format, with paths
//! relative to that folder.
//!
//! - [`merge_user_clips`] adds them to a loaded built-in set. It is
//!   add-only: a user clip whose name is already taken (built-in or an
//!   earlier user clip) is skipped, never replaces anything.
//! - [`install_candidate`] copies one approved candidate folder in. It never
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

/// One approved candidate folder from the pipeline's staging:
/// `<staging>/user_<slug>_<NN>/` with `<stem>.json`, `<stem>.mp4`,
/// `<stem>_sheet.png`, `<stem>_preview.png` and `qa/`.
#[derive(Debug, Clone)]
pub struct Candidate<'a> {
    /// The candidate folder.
    pub folder: &'a Path,
    /// The files' common stem (the pipeline's clip name).
    pub stem: &'a str,
    /// The name Ko typed (sanitized here).
    pub name: &'a str,
}

/// Reads an integer field of the clip JSON.
fn int(v: &serde_json::Value, k: &str) -> Option<i64> {
    v.get(k).and_then(serde_json::Value::as_i64)
}

/// Copies candidate `c` into `dir` add-only and appends its entry to
/// `user_manifest.toml`. The folder is copied as `<dir>/<name>/` with the
/// `<stem>.*` files renamed to `<name>.*`; `<stem>_preview.png` and `qa/`
/// are left behind; the JSON's `name`, `sheet` and `video.file` are
/// updated. `builtin` reports names of the built-in set (never replaced).
/// Returns the name it was saved under (suffixed on a collision).
pub fn install_candidate(
    dir: &Path,
    c: &Candidate<'_>,
    builtin: &dyn Fn(&str) -> bool,
) -> Result<String, AtlasError> {
    let bad = |reason: String| AtlasError::InvalidClip {
        clip: c.stem.to_string(),
        reason,
    };
    let src = |suffix: &str| c.folder.join(format!("{}{suffix}", c.stem));
    let text = std::fs::read_to_string(src(".json"))?;
    let mut js: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| bad(format!("{}.json: {e}", c.stem)))?;
    if !js.is_object() {
        return Err(bad("the clip JSON is not an object".into()));
    }
    let n = int(&js, "frame_count").filter(|&n| n > 0 && n <= 100_000);
    let w = int(&js, "frame_width").or_else(|| int(&js, "canvas_w"));
    let h = int(&js, "frame_height").or_else(|| int(&js, "canvas_h"));
    let (Some(n), Some(w), Some(h)) = (n, w, h) else {
        return Err(bad(
            "frame_count / frame_width / frame_height missing".into()
        ));
    };
    if !(1..=8192).contains(&w) || !(1..=8192).contains(&h) {
        return Err(bad(format!("frame size {w}x{h}")));
    }
    let (n, w, h) = (n as u32, w as u32, h as u32);
    for f in [src(".mp4"), src("_sheet.png")] {
        if !f.is_file() {
            return Err(bad(format!("{} is missing", f.display())));
        }
    }
    // Sheet size from its header only (never decoded in full).
    let (sw, sh) = image::image_dimensions(src("_sheet.png"))?;
    if (sw as u64) < w as u64 * n as u64 || sh < h {
        return Err(bad(format!(
            "sheet is {sw}x{sh}, needs {n} frames of {w}x{h}"
        )));
    }
    let fps = js
        .get("fps")
        .and_then(serde_json::Value::as_f64)
        .filter(|f| f.is_finite() && *f > 0.0)
        .map_or(12.0, |f| f.min(120.0) as f32);
    let anchor_xy = js
        .get("anchor")
        .and_then(|a| a.as_array())
        .and_then(|a| Some((a.first()?.as_i64()? as i32, a.get(1)?.as_i64()? as i32)));
    let baseline = int(&js, "baseline_y").map(|b| b as i32);
    let feet = (
        int(&js, "body_anchor_x")
            .map(|x| x as i32)
            .or(anchor_xy.map(|a| a.0))
            .unwrap_or(w as i32 / 2),
        baseline.or(anchor_xy.map(|a| a.1)).unwrap_or(h as i32 - 8),
    );
    let looping = js
        .get("loop")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let mut tags: Vec<String> = js
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|t| {
            t.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if !tags.iter().any(|t| t == USER_TAG) {
        tags.push(USER_TAG.into());
    }
    // Baked secondary motion: the `baked_motion` tag (kept above) already
    // mutes the engine's jiggle in the animator. An explicit
    // `engine_springs` (text directive or list, as tools/import-manifest
    // reads it) adds `disable_springs`; an unknown group is dropped.
    let disable_springs: Vec<String> = match js.get("engine_springs") {
        Some(serde_json::Value::String(t)) => lillith_core::spring::SpringMask::parse_directive(t),
        Some(v @ serde_json::Value::Array(_)) => {
            serde_json::from_value::<Vec<String>>(v.clone()).unwrap_or_default()
        }
        _ => Vec::new(),
    };
    let disable_springs = if lillith_core::spring::SpringMask::from_groups(&disable_springs).is_ok()
    {
        disable_springs
    } else {
        Vec::new()
    };

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
    js["name"] = name.clone().into();
    js["sheet"] = format!("{name}_sheet.png").into();
    if let Some(v) = js.get_mut("video").filter(|v| v.is_object()) {
        v["file"] = format!("{name}.mp4").into();
    } else {
        js["video"] = serde_json::json!({ "file": format!("{name}.mp4") });
    }
    let written = copy_candidate(c.folder, c.stem, &name, &clip_dir).and_then(|()| {
        let body = serde_json::to_string_pretty(&js)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(clip_dir.join(format!("{name}.json")))?;
        f.write_all(body.as_bytes())?;
        f.sync_all()
    });
    if let Err(e) = written {
        // Only what this call created (a fresh folder) is removed.
        let _ = std::fs::remove_dir_all(&clip_dir);
        return Err(e.into());
    }
    let clip = Clip {
        page: format!("{name}/{name}_sheet.png"),
        fps,
        mode: if looping {
            LoopMode::Loop
        } else {
            LoopMode::Once
        },
        video: Some(format!("{name}/{name}.mp4")),
        baseline_y: baseline,
        tags,
        disable_springs,
        frames: (0..n)
            .map(|i| FrameRect {
                x: i * w,
                y: 0,
                w,
                h,
                anchor: feet,
            })
            .collect(),
        ..toml::from_str::<Clip>("page = \"\"\nframes = []").expect("minimal clip")
    };
    if let Err(e) = clip.validate(&name) {
        let _ = std::fs::remove_dir_all(&clip_dir);
        return Err(e);
    }
    manifest.clips.insert(name.clone(), clip);
    if let Err(e) = write_manifest(dir, &manifest) {
        let _ = std::fs::remove_dir_all(&clip_dir);
        return Err(e);
    }
    Ok(name)
}

/// Copies a candidate folder's contents into the fresh `to`, renaming
/// `<stem>…` files to `<name>…`. Leaves out `<stem>.json` (rewritten by
/// the caller), `<stem>_preview.png`, `qa/`, hidden entries and symlinks.
fn copy_candidate(from: &Path, stem: &str, name: &str, to: &Path) -> std::io::Result<()> {
    let (json, preview) = (format!("{stem}.json"), format!("{stem}_preview.png"));
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let fname = e.file_name().to_string_lossy().into_owned();
        if fname == json || fname == preview || fname == "qa" || fname.starts_with('.') {
            continue;
        }
        let out = match fname.strip_prefix(stem) {
            Some(rest) => format!("{name}{rest}"),
            None => fname.clone(),
        };
        let ty = e.file_type()?;
        if ty.is_dir() {
            copy_tree(&e.path(), &to.join(&out))?;
        } else if ty.is_file() {
            copy_new(&e.path(), &to.join(&out))?;
        }
    }
    Ok(())
}

/// Copies a directory tree into the NEW folder `to` (add-only).
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let ty = e.file_type()?;
        if ty.is_dir() {
            copy_tree(&e.path(), &to.join(e.file_name()))?;
        } else if ty.is_file() {
            copy_new(&e.path(), &to.join(e.file_name()))?;
        }
    }
    Ok(())
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

    /// A pipeline candidate folder like user_import.py writes.
    fn staged(stage: &Path, stem: &str, frames: u32) -> PathBuf {
        let d = stage.join(stem);
        std::fs::create_dir_all(d.join("qa")).unwrap();
        std::fs::write(d.join(format!("{stem}.mp4")), b"not really a video").unwrap();
        image::RgbaImage::new(40 * frames, 30)
            .save(d.join(format!("{stem}_sheet.png")))
            .unwrap();
        image::RgbaImage::new(40, 30)
            .save(d.join(format!("{stem}_preview.png")))
            .unwrap();
        std::fs::write(d.join("qa/report.png"), b"x").unwrap();
        let js = serde_json::json!({
            "name": stem, "frame_count": frames, "frame_width": 40, "frame_height": 30,
            "fps": 12, "loop": false, "baseline_y": 28, "anchor": [20, 28], "body_anchor_x": 21,
            "sheet": format!("{stem}_sheet.png"), "tags": ["baked_motion", "user_import"],
            "engine_note": "Mute or strongly reduce engine chest/pigtail springs and breathing for this clip, or the motion doubles.",
            "engine_springs": "disable chest/pigtails for this clip",
            "video": {"file": format!("{stem}.mp4"), "layout": "alpha_side_by_side"},
        });
        std::fs::write(d.join(format!("{stem}.json")), js.to_string()).unwrap();
        d
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
        let folder = staged(&stage, "user_clip_01", 3);
        let builtin = Manifest::from_toml_str(BUILTIN).unwrap();
        let is_builtin = |n: &str| builtin.clips.contains_key(n);
        let new = |name| Candidate {
            folder: &folder,
            stem: "user_clip_01",
            name,
        };
        // A built-in name is never replaced: it gets a suffix.
        assert_eq!(
            install_candidate(&user, &new("idle"), &is_builtin).unwrap(),
            "idle_2"
        );
        assert_eq!(
            install_candidate(&user, &new("Wave"), &is_builtin).unwrap(),
            "wave"
        );
        // An existing user clip is never replaced either.
        let before = std::fs::read(user.join("wave/wave.mp4")).unwrap();
        assert_eq!(
            install_candidate(&user, &new("wave"), &is_builtin).unwrap(),
            "wave_2"
        );
        assert_eq!(std::fs::read(user.join("wave/wave.mp4")).unwrap(), before);
        // A stray folder of that name (no manifest entry) is skipped too.
        std::fs::create_dir_all(user.join("wave_3")).unwrap();
        assert_eq!(
            install_candidate(&user, &new("wave"), &is_builtin).unwrap(),
            "wave_4"
        );
        let um = load_user_manifest(&user).unwrap();
        let names: Vec<&String> = um.clips.keys().collect();
        assert_eq!(names, ["idle_2", "wave", "wave_2", "wave_4"]);
        let c = &um.clips["wave"];
        assert_eq!(c.frames.len(), 3);
        assert_eq!((c.frames[2].x, c.frames[0].w, c.frames[0].h), (80, 40, 30));
        assert_eq!(c.frames[0].anchor, (21, 28));
        assert_eq!(c.page, "wave/wave_sheet.png");
        assert_eq!(c.video.as_deref(), Some("wave/wave.mp4"));
        assert_eq!(c.mode, LoopMode::Once);
        assert!(c.tags.iter().any(|t| t == USER_TAG));
        assert!(c.has_tag("baked_motion"), "baked motion mutes the jiggle");
        assert_eq!(c.disable_springs, ["chest", "pigtails"]);
        // Renamed files, no preview / qa, JSON updated.
        let w2 = user.join("wave_2");
        let mut files: Vec<String> = std::fs::read_dir(&w2)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(files, ["wave_2.json", "wave_2.mp4", "wave_2_sheet.png"]);
        let js: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(w2.join("wave_2.json")).unwrap())
                .unwrap();
        assert_eq!(js["name"], "wave_2");
        assert_eq!(js["sheet"], "wave_2_sheet.png");
        assert_eq!(js["video"]["file"], "wave_2.mp4");
        assert_eq!(js["video"]["layout"], "alpha_side_by_side");
        // The staged candidate is untouched.
        assert!(folder.join("user_clip_01_preview.png").is_file());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_bad_candidate_leaves_nothing_behind() {
        let root = tmp("bad");
        let (stage, user) = (root.join("stage"), root.join("user-clips"));
        let folder = staged(&stage, "user_b_01", 3);
        std::fs::remove_file(folder.join("user_b_01_sheet.png")).unwrap();
        let c = Candidate {
            folder: &folder,
            stem: "user_b_01",
            name: "b",
        };
        assert!(install_candidate(&user, &c, &|_| false).is_err());
        assert!(!user.join("b").exists());
        // A sheet too small for the frames is refused too.
        image::RgbaImage::new(40, 30)
            .save(folder.join("user_b_01_sheet.png"))
            .unwrap();
        assert!(install_candidate(&user, &c, &|_| false).is_err());
        assert!(!user.join("b").exists());
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
        let folder = staged(&stage, "user_x_01", 2);
        let none = |_: &str| false;
        let new = |name| Candidate {
            folder: &folder,
            stem: "user_x_01",
            name,
        };
        install_candidate(&user, &new("idle"), &none).unwrap(); // clashes with built-in idle
        install_candidate(&user, &new("strut"), &none).unwrap();
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
