//! `import-manifest`: converts a generated-style `manifest.json` (as written
//! by the image-generation pipeline into `assets/generated/`) into the
//! engine's `manifest.toml` (see `lillith_anim::atlas`).
//!
//! Input shape (only the fields used here):
//!
//! ```json
//! { "animations": [ {
//!     "name": "wake", "sheet": "wake/wake_sheet.png", "frame_count": 10,
//!     "frame_width": 400, "frame_height": 400, "fps": 6, "loop": false,
//!     "baseline_y": 392, "anchor": [200, 392], "body_anchor_x": 200,
//!     "scale": 0.54 | { "seated": 0.29, ... }, "facing": "front",
//!     "frame_durations_ms": [450, ...], "placeholder": false } ] }
//! ```
//!
//! Sheets are horizontal strips: frame `i` is at `x = i * frame_width, y = 0`.
//! The feet anchor is `[body_anchor_x or anchor[0], baseline_y or anchor[1]]`.
//! `sheet` becomes `page` (relative to the output manifest when it sits in the
//! same directory as the input), `loop` becomes `mode = "loop" | "once"`.
//!
//! `[x, y]` integer fields named `*_origin` / `*_point` (e.g. `beam_origin`,
//! `grab_point`) become `points` on the clip; they are taken from the entry
//! and from its per-animation JSON (the entry's `manifest` path) when present.
//! `feet_off_ground_px` becomes a `feet_off_ground_px:<n>` tag; the feet
//! anchor itself stays on the shared baseline.
//!
//! Seat metadata (`seat_y`, `seat_x_range`, `feet_lowest_y`) and event
//! reaction metadata (`event`, `base_clip`, `base_frames`) are carried into
//! the clip fields of the same names. The engine validates that `base_clip`
//! exists and every `base_frames` index is in range.
//!
//! With `--check`, the written manifest is re-loaded through
//! `Manifest::load` + `load_page` and every frame rect is bounds-checked.

use anyhow::{bail, ensure, Context, Result};
use clap::Parser;
use lillith_anim::atlas::{load_page, Clip, FrameRect, LoopMode, Manifest};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(name = "import-manifest", about = "manifest.json -> manifest.toml")]
struct Args {
    /// Generated-style manifest.json.
    #[arg(long, default_value = "assets/generated/manifest.json")]
    input: PathBuf,
    /// Output manifest.toml (default: next to the input).
    #[arg(long)]
    output: Option<PathBuf>,
    /// Extra tag added to every clip (repeatable), e.g. `--tag canonical`.
    #[arg(long = "tag")]
    tags: Vec<String>,
    /// After writing, load the manifest and every page and verify all frames.
    #[arg(long)]
    check: bool,
}

#[derive(Debug, Deserialize)]
struct JsonManifest {
    animations: Vec<JsonAnim>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum JsonScale {
    Single(f64),
    Phases(BTreeMap<String, f64>),
}

#[derive(Debug, Deserialize)]
struct JsonAnim {
    name: String,
    sheet: String,
    frame_count: u32,
    #[serde(default = "default_cell")]
    frame_width: u32,
    #[serde(default = "default_cell")]
    frame_height: u32,
    fps: f32,
    #[serde(rename = "loop", default = "default_true")]
    looping: bool,
    #[serde(default)]
    baseline_y: Option<i32>,
    #[serde(default)]
    anchor: Option<(i32, i32)>,
    #[serde(default)]
    body_anchor_x: Option<i32>,
    #[serde(default)]
    scale: Option<JsonScale>,
    #[serde(default)]
    facing: Option<String>,
    #[serde(default)]
    frame_durations_ms: Option<Vec<u32>>,
    #[serde(default)]
    placeholder: bool,
    /// Per-animation JSON (relative to the top-level manifest), which may
    /// carry extra fields such as `beam_origin` or `grab_point`.
    #[serde(default)]
    manifest: Option<String>,
    /// Every other field of the entry.
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// Attachment points are `[x, y]` fields named `*_origin` or `*_point`
/// (e.g. `beam_origin`, `grab_point`). Other integer pairs such as
/// `code_tween_frames` or `seat_x_range` are not coordinates.
fn is_point_name(key: &str) -> bool {
    key.ends_with("_origin") || key.ends_with("_point")
}

/// Collects attachment points (see [`is_point_name`])
/// from the top-level entry, then from the per-animation JSON for names the
/// entry doesn't already define.
fn collect_points(
    entry: &Map<String, Value>,
    side: Option<&Map<String, Value>>,
) -> BTreeMap<String, (i32, i32)> {
    let mut out = BTreeMap::new();
    for map in std::iter::once(entry).chain(side) {
        for (k, v) in map {
            if !is_point_name(k) || out.contains_key(k) {
                continue;
            }
            if let Some([x, y]) = v.as_array().map(Vec::as_slice) {
                if let (Some(x), Some(y)) = (x.as_i64(), y.as_i64()) {
                    if let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) {
                        out.insert(k.clone(), (x, y));
                    }
                }
            }
        }
    }
    out
}

/// Typed field looked up in the entry, then the per-animation JSON. A field
/// that is present but has the wrong shape is an error, not silently dropped.
fn typed_field<T: serde::de::DeserializeOwned>(
    name: &str,
    entry: &Map<String, Value>,
    side: Option<&Map<String, Value>>,
    key: &str,
) -> Result<Option<T>> {
    match entry.get(key).or_else(|| side.and_then(|m| m.get(key))) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .with_context(|| format!("{name}: field `{key}` has an unexpected shape: {v}")),
    }
}

/// Integer field looked up in the entry, then the per-animation JSON.
fn int_field(
    entry: &Map<String, Value>,
    side: Option<&Map<String, Value>>,
    key: &str,
) -> Option<i64> {
    entry
        .get(key)
        .or_else(|| side.and_then(|m| m.get(key)))
        .and_then(Value::as_i64)
}

fn default_cell() -> u32 {
    400
}
fn default_true() -> bool {
    true
}

/// Converts one JSON animation into an engine clip.
fn convert(
    anim: &JsonAnim,
    side: Option<&Map<String, Value>>,
    page: String,
    extra_tags: &[String],
) -> Result<Clip> {
    ensure!(anim.frame_count > 0, "{}: frame_count is 0", anim.name);
    ensure!(
        anim.fps.is_finite() && anim.fps > 0.0,
        "{}: fps must be > 0",
        anim.name
    );
    if let Some(ms) = &anim.frame_durations_ms {
        ensure!(
            ms.len() == anim.frame_count as usize,
            "{}: {} frame_durations_ms for {} frames",
            anim.name,
            ms.len(),
            anim.frame_count
        );
    }
    let (ax, ay) = anim
        .anchor
        .unwrap_or(((anim.frame_width / 2) as i32, anim.frame_height as i32 - 8));
    let feet = (
        anim.body_anchor_x.unwrap_or(ax),
        anim.baseline_y.unwrap_or(ay),
    );

    let mut tags = extra_tags.to_vec();
    if anim.placeholder {
        tags.push("placeholder".into());
    }
    if let Some(f) = &anim.facing {
        tags.push(format!("facing:{f}"));
    }
    // Held/dragged poses keep the shared feet anchor (so swapping clips
    // doesn't jump); record how far the feet hang above it.
    if let Some(px) = int_field(&anim.extra, side, "feet_off_ground_px") {
        tags.push(format!("feet_off_ground_px:{px}"));
    }
    let scale = match &anim.scale {
        Some(JsonScale::Single(s)) => Some(*s),
        Some(JsonScale::Phases(phases)) => {
            // Phase-dependent scale (e.g. wake: seated -> standing) has no
            // single value; keep it as tags.
            tags.extend(phases.iter().map(|(k, v)| format!("scale:{k}={v}")));
            None
        }
        None => None,
    };

    let frames = (0..anim.frame_count)
        .map(|i| FrameRect {
            x: i * anim.frame_width,
            y: 0,
            w: anim.frame_width,
            h: anim.frame_height,
            anchor: feet,
        })
        .collect();

    Ok(Clip {
        page,
        fps: anim.fps,
        mode: if anim.looping {
            LoopMode::Loop
        } else {
            LoopMode::Once
        },
        frame_durations_ms: anim.frame_durations_ms.clone(),
        baseline_y: anim.baseline_y,
        scale,
        tags,
        points: collect_points(&anim.extra, side),
        // Seat metadata (sit_swing): a line, a range and a scalar; mapped
        // explicitly so they are never mistaken for points or frame indices.
        seat_y: typed_field(&anim.name, &anim.extra, side, "seat_y")?,
        seat_x_range: typed_field(&anim.name, &anim.extra, side, "seat_x_range")?,
        feet_lowest_y: typed_field(&anim.name, &anim.extra, side, "feet_lowest_y")?,
        // Event reaction metadata: which base clip/frames it composes from.
        event: typed_field(&anim.name, &anim.extra, side, "event")?,
        base_clip: typed_field(&anim.name, &anim.extra, side, "base_clip")?,
        base_frames: typed_field(&anim.name, &anim.extra, side, "base_frames")?,
        frames,
    })
}

fn dir_of(p: &Path) -> PathBuf {
    match p.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Builds the engine manifest. `page`s stay relative to `in_dir` when the
/// output lives there too; otherwise they are made absolute.
fn build(json: &JsonManifest, in_dir: &Path, out_dir: &Path, tags: &[String]) -> Result<Manifest> {
    let relative = same_dir(in_dir, out_dir);
    let mut m = Manifest::default();
    for anim in &json.animations {
        let page = if relative {
            anim.sheet.clone()
        } else {
            let p = in_dir.join(&anim.sheet);
            p.canonicalize().unwrap_or(p).to_string_lossy().into_owned()
        };
        let side = match &anim.manifest {
            Some(rel) => read_side_json(&in_dir.join(rel))?,
            None => None,
        };
        let clip = convert(anim, side.as_ref(), page, tags)?;
        if m.clips.insert(anim.name.clone(), clip).is_some() {
            bail!("duplicate animation name {}", anim.name);
        }
    }
    Ok(m)
}

/// Reads a per-animation JSON object; a missing file is not an error.
fn read_side_json(path: &Path) -> Result<Option<Map<String, Value>>> {
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {path:?}"))?;
    match serde_json::from_str(&text).with_context(|| format!("parsing {path:?}"))? {
        Value::Object(m) => Ok(Some(m)),
        _ => bail!("{path:?}: expected a JSON object"),
    }
}

/// Loads `path` like the engine does and bounds-checks every frame.
fn check(path: &Path) -> Result<()> {
    let m = Manifest::load(path).with_context(|| format!("Manifest::load {path:?}"))?;
    let dir = dir_of(path);
    for (name, clip) in &m.clips {
        let page_path = clip.page_path(&dir);
        let page = load_page(&page_path).with_context(|| format!("{name}: {page_path:?}"))?;
        for (i, f) in clip.frames.iter().enumerate() {
            ensure!(
                f.x + f.w <= page.width() && f.y + f.h <= page.height(),
                "{name} frame {i}: rect {f:?} outside {}x{} page",
                page.width(),
                page.height()
            );
        }
        println!(
            "ok {name:<11} {:>2} frames  page {}x{}  {:?}  {:.2}s/pass ({})  anchor {:?}",
            clip.frames.len(),
            page.width(),
            page.height(),
            clip.mode,
            clip.duration_s(),
            if clip.frame_durations_ms.is_some() {
                "per-frame ms".to_string()
            } else {
                format!("{} fps", clip.fps)
            },
            clip.frames[0].anchor,
        );
        if let Some(ev) = &clip.event {
            println!(
                "   event {ev} <- base {} frames {:?}",
                clip.base_clip.as_deref().unwrap_or("-"),
                clip.base_frames.as_deref().unwrap_or(&[])
            );
        }
        if clip.seat_y.is_some() || clip.seat_x_range.is_some() || clip.feet_lowest_y.is_some() {
            println!(
                "   seat_y {:?}  seat_x_range {:?}  feet_lowest_y {:?}",
                clip.seat_y, clip.seat_x_range, clip.feet_lowest_y
            );
        }
        let cell = clip.frames[0];
        for (pname, &(px, py)) in &clip.points {
            let inside = px >= 0 && py >= 0 && (px as u32) < cell.w && (py as u32) < cell.h;
            println!(
                "   point {pname} = ({px}, {py}){}",
                if inside { "" } else { "  [outside cell]" }
            );
        }
    }
    println!("{} clip(s) loaded and in bounds", m.clips.len());
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let text = std::fs::read_to_string(&args.input)
        .with_context(|| format!("reading {:?}", args.input))?;
    let json: JsonManifest =
        serde_json::from_str(&text).with_context(|| format!("parsing {:?}", args.input))?;
    let in_dir = dir_of(&args.input);
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| in_dir.join("manifest.toml"));
    let manifest = build(&json, &in_dir, &dir_of(&output), &args.tags)?;

    let header = format!(
        "# Generated by tools/import-manifest from {}. Do not edit by hand.\n\n",
        args.input.display()
    );
    let body = toml::to_string(&manifest).context("serializing manifest.toml")?;
    std::fs::write(&output, header + &body).with_context(|| format!("writing {output:?}"))?;
    println!(
        "wrote {} clip(s) to {}",
        manifest.clips.len(),
        output.display()
    );

    if args.check {
        check(&output)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{ "animations": [
      { "name": "run_right", "sheet": "run_right/run_right_sheet.png", "frame_count": 3,
        "frame_width": 400, "frame_height": 400, "fps": 12, "loop": true,
        "baseline_y": 392, "anchor": [200, 392], "body_anchor_x": 231, "scale": 0.54,
        "facing": "right" },
      { "name": "wake", "sheet": "wake/wake_sheet.png", "frame_count": 2,
        "fps": 6, "loop": false, "baseline_y": 392, "anchor": [200, 392],
        "scale": { "seated": 0.29, "idle": 0.54 }, "frame_durations_ms": [450, 350] }
    ] }"#;

    fn sample() -> Manifest {
        let json: JsonManifest = serde_json::from_str(SAMPLE).unwrap();
        build(
            &json,
            Path::new("."),
            Path::new("."),
            &["canonical".to_string()],
        )
        .unwrap()
    }

    #[test]
    fn converts_strip_and_anchors() {
        let m = sample();
        let run = &m.clips["run_right"];
        assert_eq!(run.page, "run_right/run_right_sheet.png");
        assert_eq!(run.mode, LoopMode::Loop);
        assert_eq!(run.frames.len(), 3);
        assert_eq!(run.frames[2].x, 800);
        assert_eq!(run.frames[2].anchor, (231, 392));
        assert_eq!(run.scale, Some(0.54));
        assert_eq!(run.tags, vec!["canonical", "facing:right"]);
    }

    #[test]
    fn converts_once_durations_and_phase_scale() {
        let m = sample();
        let wake = &m.clips["wake"];
        assert_eq!(wake.mode, LoopMode::Once);
        assert_eq!(wake.frame_durations_ms.as_deref(), Some(&[450, 350][..]));
        assert_eq!(wake.frames[1].anchor, (200, 392));
        assert_eq!(wake.scale, None);
        assert!(wake.tags.contains(&"scale:seated=0.29".to_string()));
    }

    #[test]
    fn output_is_accepted_by_engine() {
        let s = toml::to_string(&sample()).unwrap();
        let back = Manifest::from_toml_str(&s).unwrap();
        assert_eq!(back.clips, sample().clips);
    }

    #[test]
    fn carries_points_from_entry_and_side_json() {
        let json: JsonManifest = serde_json::from_str(
            r#"{ "animations": [ { "name": "drag_held", "sheet": "d.png", "frame_count": 1,
                 "fps": 10, "anchor": [200, 392], "baseline_y": 392,
                 "grab_point": [200, 76], "dir": "d/" } ] }"#,
        )
        .unwrap();
        let side: Value = serde_json::from_str(
            r##"{ "grab_point": [1, 1], "beam_origin": [306, 78], "anchor": [0, 0],
                 "code_tween_frames": [3, 8], "seat_x_range": [10, 20],
                 "palette": ["#fff", "#000"], "feet_off_ground_px": 16 }"##,
        )
        .unwrap();
        let clip = convert(&json.animations[0], side.as_object(), "d.png".into(), &[]).unwrap();
        // Entry wins over side JSON; anchor, non-point pairs and non-integer
        // pairs are skipped.
        assert_eq!(clip.point("grab_point"), Some((200, 76)));
        assert_eq!(clip.point("beam_origin"), Some((306, 78)));
        assert_eq!(clip.points.len(), 2);
        // Feet anchor stays on the baseline; hang distance becomes a tag.
        assert_eq!(clip.frames[0].anchor, (200, 392));
        assert!(clip.tags.contains(&"feet_off_ground_px:16".to_string()));
        let back = Manifest::from_toml_str(
            &toml::to_string(&Manifest {
                clips: [("drag_held".to_string(), clip.clone())].into(),
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(back.clips["drag_held"], clip);
    }

    #[test]
    fn carries_seat_and_event_metadata() {
        let json: JsonManifest = serde_json::from_str(
            r#"{ "animations": [
              { "name": "think", "sheet": "t.png", "frame_count": 3, "fps": 6 },
              { "name": "event_wifi", "sheet": "e.png", "frame_count": 2, "fps": 10,
                "loop": false, "manifest": "does/not/exist.json" },
              { "name": "sit_swing", "sheet": "s.png", "frame_count": 1, "fps": 8,
                "baseline_y": 392, "anchor": [200, 392] } ] }"#,
        )
        .unwrap();
        let ev_side: Value = serde_json::from_str(
            r#"{ "event": "wifi", "base_clip": "think", "base_frames": [2, 0] }"#,
        )
        .unwrap();
        let seat_side: Value = serde_json::from_str(
            r#"{ "seat_y": 230, "seat_x_range": [136, 279], "feet_lowest_y": 364 }"#,
        )
        .unwrap();
        let a = &json.animations;
        let think = convert(&a[0], None, "t.png".into(), &[]).unwrap();
        let ev = convert(&a[1], ev_side.as_object(), "e.png".into(), &[]).unwrap();
        let sit = convert(&a[2], seat_side.as_object(), "s.png".into(), &[]).unwrap();
        assert_eq!(ev.event.as_deref(), Some("wifi"));
        assert_eq!(ev.base_clip.as_deref(), Some("think"));
        assert_eq!(ev.base_frames.as_deref(), Some(&[2, 0][..]));
        assert_eq!(sit.seat_y, Some(230));
        assert_eq!(sit.seat_x_range, Some((136, 279)));
        assert_eq!(sit.feet_lowest_y, Some(364));
        // Not misread as attachment points; feet anchor untouched.
        assert!(sit.points.is_empty());
        assert_eq!(sit.frames[0].anchor, (200, 392));
        assert_eq!(think.event, None);
        assert_eq!(think.seat_y, None);

        let m = Manifest {
            clips: [
                ("think".to_string(), think),
                ("event_wifi".to_string(), ev),
                ("sit_swing".to_string(), sit),
            ]
            .into(),
        };
        let back = Manifest::from_toml_str(&toml::to_string(&m).unwrap()).unwrap();
        assert_eq!(back.clips, m.clips);
    }

    #[test]
    fn rejects_malformed_metadata() {
        let json: JsonManifest = serde_json::from_str(
            r#"{ "animations": [ { "name": "s", "sheet": "s.png", "frame_count": 1,
                 "fps": 8, "seat_x_range": [1, 2, 3] } ] }"#,
        )
        .unwrap();
        assert!(convert(&json.animations[0], None, "s.png".into(), &[]).is_err());
    }

    #[test]
    fn rejects_bad_duration_count() {
        let json: JsonManifest = serde_json::from_str(
            r#"{ "animations": [ { "name": "x", "sheet": "x.png", "frame_count": 3,
                 "fps": 6, "frame_durations_ms": [1, 2] } ] }"#,
        )
        .unwrap();
        assert!(build(&json, Path::new("."), Path::new("."), &[]).is_err());
    }
}
