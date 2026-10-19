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
//! With `--check`, the written manifest is re-loaded through
//! `Manifest::load` + `load_page` and every frame rect is bounds-checked.

use anyhow::{bail, ensure, Context, Result};
use clap::Parser;
use lillith_anim::atlas::{load_page, Clip, FrameRect, LoopMode, Manifest};
use serde::Deserialize;
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
    Phases(std::collections::BTreeMap<String, f64>),
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
}

fn default_cell() -> u32 {
    400
}
fn default_true() -> bool {
    true
}

/// Converts one JSON animation into an engine clip.
fn convert(anim: &JsonAnim, page: String, extra_tags: &[String]) -> Result<Clip> {
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
        let clip = convert(anim, page, tags)?;
        if m.clips.insert(anim.name.clone(), clip).is_some() {
            bail!("duplicate animation name {}", anim.name);
        }
    }
    Ok(m)
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
    fn rejects_bad_duration_count() {
        let json: JsonManifest = serde_json::from_str(
            r#"{ "animations": [ { "name": "x", "sheet": "x.png", "frame_count": 3,
                 "fps": 6, "frame_durations_ms": [1, 2] } ] }"#,
        )
        .unwrap();
        assert!(build(&json, Path::new("."), Path::new("."), &[]).is_err());
    }
}
