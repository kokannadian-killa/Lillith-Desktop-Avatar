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
//! Also carried (entry first, then the per-animation JSON):
//! - `rule_events` (every dotted rule event the clip serves) -> `rule_events`;
//! - `frame_points` (`{ name: [[x, y], ...one per frame] }`) -> `frame_points`
//!   (the clip-level medians stay in `points`); a list whose length is not
//!   `frame_count` is an error;
//! - blend helpers (`blend: true`, `from`, `to`, `from_frame`, `to_frame`,
//!   optional `blend_t` / `easing`) -> `blend = { ... }` plus a `blend` tag.
//!   Blend clips describe `scale` as text ("interpolated ..."); that becomes a
//!   `scale:interpolated` tag;
//! - locomotion metadata `root_motion` (one cell-px advance per frame) and
//!   `contact_frames` (heel-strike frames) -> the clip fields of the same
//!   names.
//!
//! Baked-jiggle batch (2026-10, 197 clips):
//! - `jiggle_baked`, `jiggle_of` (on the `<name>_jiggle` variant) and
//!   `jiggle_variant` (on the engine-clean clip) are carried as fields;
//! - `clean_frame_index` (jiggle frame -> clean frame; slow4 settle
//!   sub-frames make a jiggle clip longer than its clean clip) is carried;
//!   the jiggle clip keeps its own `frame_durations_ms`/`frame_points`, and
//!   per-frame data only the clean clip has (`base_frames`, `frame_points`,
//!   mouth data) is mapped through it. A length mismatch without the index
//!   is an error (except two looping clips); a different total duration is
//!   a warning. `impulse_frames` are not imported (kicks come from
//!   physics.toml recipes, timed in seconds, so they need no mapping);
//! - `engine_springs` (free text, e.g. "disable chest/pigtails/hip for this
//!   clip", or a list of group names) becomes `disable_springs = [...]`; text
//!   that names no spring group is an error;
//! - `allure: true` / `idle_variant: true` become `allure` / `idle_variant`
//!   tags, a `retime` record a `retimed` tag (its holds are already in
//!   `frame_durations_ms`);
//! - a jiggle variant's feet anchor is taken from its clean clip (same frames
//!   warped in place), with a warning when the JSON disagrees.
//!
//! With `--check`, the written manifest is re-loaded through
//! `Manifest::load` + `load_page` and every frame rect is bounds-checked.

use anyhow::{bail, ensure, Context, Result};
use clap::Parser;
use lillith_anim::atlas::{load_page, Blend, Clip, FrameRect, LoopMode, Manifest, MouthPatch};
use lillith_core::spring::{Channel, SpringMask};
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
    /// Free text, e.g. blend helpers: "interpolated between the two clips".
    Text(String),
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
    // Free-form tags from the entry (else the per-animation JSON), e.g.
    // `baked_motion` or `variant_of:idle` (generated_v2).
    let json_tags: Option<Vec<String>> = typed_field(&anim.name, &anim.extra, side, "tags")?;
    for t in json_tags.unwrap_or_default() {
        if !tags.contains(&t) {
            tags.push(t);
        }
    }
    // Held/dragged poses keep the shared feet anchor (so swapping clips
    // doesn't jump); record how far the feet hang above it.
    if let Some(px) = int_field(&anim.extra, side, "feet_off_ground_px") {
        tags.push(format!("feet_off_ground_px:{px}"));
    }
    // Gait-rig clips (walk/strut/run): the planted boot moves at exactly
    // this canvas speed, so the overlay can translate her without sliding.
    if let Some(v) = anim
        .extra
        .get("move_speed_px_s")
        .or_else(|| side.and_then(|m| m.get("move_speed_px_s")))
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0)
    {
        tags.push(format!("move_speed_px_s:{}", (v * 100.0).round() / 100.0));
    }
    let scale = match &anim.scale {
        Some(JsonScale::Single(s)) => Some(*s),
        Some(JsonScale::Phases(phases)) => {
            // Phase-dependent scale (e.g. wake: seated -> standing) has no
            // single value; keep it as tags.
            tags.extend(phases.iter().map(|(k, v)| format!("scale:{k}={v}")));
            None
        }
        Some(JsonScale::Text(t)) => {
            let word = t.split_whitespace().next().unwrap_or("text");
            tags.push(format!("scale:{word}"));
            None
        }
        None => None,
    };

    // Speaking clips: the baked mouth shape of each frame
    // (`mouth_sequence`, else `lip_flap.baked_sequence`), e.g.
    // ["closed", "mid", "open", "smile", ...]. (`mouth_shapes` in the JSON
    // is the patch-image map, not used here.)
    let mouth_sequence: Vec<String> =
        match typed_field(&anim.name, &anim.extra, side, "mouth_sequence")? {
            Some(v) => v,
            None => typed_field::<Value>(&anim.name, &anim.extra, side, "lip_flap")?
                .and_then(|lf| lf.get("baked_sequence").cloned())
                .map(serde_json::from_value::<Vec<String>>)
                .transpose()
                .with_context(|| format!("{}: lip_flap.baked_sequence", anim.name))?
                .unwrap_or_default(),
        };
    // Mouth patches (`lip_flap.shapes.mouth_<shape>.{patch, centre_in_patch}`),
    // per-frame head tilt and the recommended shape-change rate.
    let lip_flap: Option<Value> = typed_field(&anim.name, &anim.extra, side, "lip_flap")?;
    let mut mouth_patches = BTreeMap::new();
    if let Some(shapes) = lip_flap
        .as_ref()
        .and_then(|lf| lf.get("shapes"))
        .and_then(|s| s.as_object())
    {
        for (key, v) in shapes {
            let shape = key.trim_start_matches("mouth_").to_string();
            let page = v.get("patch").and_then(|p| p.as_str());
            let centre = v
                .get("centre_in_patch")
                .and_then(|c| serde_json::from_value::<(f32, f32)>(c.clone()).ok());
            if let (Some(page), Some(centre)) = (page, centre) {
                mouth_patches.insert(
                    shape,
                    MouthPatch {
                        page: page.to_string(),
                        centre,
                    },
                );
            }
        }
    }
    let mouth_angle_deg: Vec<f32> =
        typed_field(&anim.name, &anim.extra, side, "mouth_angle_deg")?.unwrap_or_default();
    ensure!(
        mouth_angle_deg.is_empty() || mouth_angle_deg.len() == anim.frame_count as usize,
        "{}: mouth_angle_deg has {} entries for {} frames",
        anim.name,
        mouth_angle_deg.len(),
        anim.frame_count
    );
    // Baked-jiggle clips with settle sub-frames: jiggle frame -> clean frame.
    let clean_frame_index: Vec<u32> =
        typed_field(&anim.name, &anim.extra, side, "clean_frame_index")?.unwrap_or_default();
    ensure!(
        clean_frame_index.is_empty() || clean_frame_index.len() == anim.frame_count as usize,
        "{}: clean_frame_index has {} entries for {} frames",
        anim.name,
        clean_frame_index.len(),
        anim.frame_count
    );
    let mouth_shape_ms: Option<(u32, u32)> = lip_flap
        .as_ref()
        .and_then(|lf| lf.pointer("/recommended_rate/mouth_shape_change_ms"))
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    ensure!(
        mouth_sequence.is_empty() || mouth_sequence.len() == anim.frame_count as usize,
        "{}: mouth_sequence has {} entries for {} frames",
        anim.name,
        mouth_sequence.len(),
        anim.frame_count
    );
    let frame_points: BTreeMap<String, Vec<(i32, i32)>> =
        typed_field(&anim.name, &anim.extra, side, "frame_points")?.unwrap_or_default();
    for (pname, v) in &frame_points {
        ensure!(
            v.len() == anim.frame_count as usize,
            "{}: frame_points.{pname} has {} entries for {} frames",
            anim.name,
            v.len(),
            anim.frame_count
        );
    }
    for (key, tag) in [("allure", "allure"), ("idle_variant", "idle_variant")] {
        let on: Option<bool> = typed_field(&anim.name, &anim.extra, side, key)?;
        if on == Some(true) && !tags.iter().any(|t| t == tag) {
            tags.push(tag.into());
        }
    }
    if anim.extra.contains_key("retime") || side.is_some_and(|m| m.contains_key("retime")) {
        tags.push("retimed".into());
    }
    let disable_springs = engine_springs_of(anim, side)?;
    let blend = blend_of(anim, side)?;
    if blend.is_some() {
        tags.push("blend".into());
    }

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
        // Locomotion: per-frame root motion and footfall frames (in-place cycles).
        root_motion: typed_field(&anim.name, &anim.extra, side, "root_motion")?,
        contact_frames: typed_field(&anim.name, &anim.extra, side, "contact_frames")?,
        // Event reaction metadata: which base clip/frames it composes from.
        event: typed_field(&anim.name, &anim.extra, side, "event")?,
        base_clip: typed_field(&anim.name, &anim.extra, side, "base_clip")?,
        base_frames: typed_field(&anim.name, &anim.extra, side, "base_frames")?,
        rule_events: typed_field(&anim.name, &anim.extra, side, "rule_events")?.unwrap_or_default(),
        jiggle_baked: typed_field(&anim.name, &anim.extra, side, "jiggle_baked")?.unwrap_or(false),
        jiggle_of: typed_field(&anim.name, &anim.extra, side, "jiggle_of")?,
        jiggle_variant: typed_field(&anim.name, &anim.extra, side, "jiggle_variant")?,
        disable_springs,
        clean_frame_index,
        frame_points,
        blend,
        mouth_sequence,
        mouth_patches,
        mouth_angle_deg,
        mouth_shape_ms,
        frames,
    })
}

/// `engine_springs` -> spring group names to disable. Accepts the image
/// pipeline's free text ("disable chest/pigtails/hip for this clip") or a
/// list of group names.
fn engine_springs_of(anim: &JsonAnim, side: Option<&Map<String, Value>>) -> Result<Vec<String>> {
    let name = &anim.name;
    let groups = match anim
        .extra
        .get("engine_springs")
        .or_else(|| side.and_then(|m| m.get("engine_springs")))
    {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::String(text)) => {
            let g = SpringMask::parse_directive(text);
            ensure!(
                !g.is_empty(),
                "{name}: engine_springs {text:?} names no spring group to disable"
            );
            g
        }
        Some(v) => serde_json::from_value::<Vec<String>>(v.clone())
            .with_context(|| format!("{name}: engine_springs has an unexpected shape: {v}"))?,
    };
    SpringMask::from_groups(&groups)
        .map_err(|g| anyhow::anyhow!("{name}: engine_springs: unknown spring group `{g}`"))?;
    // Canonical names, in a stable order.
    let mut out: Vec<String> = Vec::new();
    for g in groups {
        let canon = match SpringMask::group(&g) {
            Some(Channel::RootX) => "hip".to_string(),
            Some(c) => c.name().to_string(),
            None => g,
        };
        if !out.contains(&canon) {
            out.push(canon);
        }
    }
    Ok(out)
}

/// (variant name, the clean clip's per-frame anchors, its baseline).
type AnchorFix = (String, Vec<(i32, i32)>, Option<i32>);

/// Jiggle variants are the clean clip's frames warped in place: they must
/// share its feet anchors, or swapping the two would jump. Copies the clean
/// clip's anchors (and baseline) onto the variant and reports any fix.
fn align_jiggle_anchors(m: &mut Manifest) -> Vec<String> {
    let mut notes = Vec::new();
    let fixes: Vec<AnchorFix> = m
        .clips
        .iter()
        .filter_map(|(name, c)| {
            let base = m.clips.get(c.jiggle_of.as_ref()?)?;
            let want: Vec<(i32, i32)> = base.frames.iter().map(|f| f.anchor).collect();
            let have: Vec<(i32, i32)> = c.frames.iter().map(|f| f.anchor).collect();
            (want.len() == have.len() && (want != have || base.baseline_y != c.baseline_y))
                .then(|| (name.clone(), want, base.baseline_y))
        })
        .collect();
    for (name, want, baseline) in fixes {
        let c = m.clips.get_mut(&name).expect("listed above");
        notes.push(format!(
            "warning: {name}: feet anchor {:?} differs from {} {:?}; using the clean clip's",
            c.frames[0].anchor,
            c.jiggle_of.as_deref().unwrap_or("?"),
            want[0]
        ));
        for (f, a) in c.frames.iter_mut().zip(want) {
            f.anchor = a;
        }
        c.baseline_y = baseline;
    }
    notes
}

/// Blend-helper metadata when the entry or its JSON has `blend: true`.
fn blend_of(anim: &JsonAnim, side: Option<&Map<String, Value>>) -> Result<Option<Blend>> {
    let name = &anim.name;
    let is_blend: Option<bool> = typed_field(name, &anim.extra, side, "blend")?;
    if is_blend != Some(true) {
        return Ok(None);
    }
    let need = |key: &str| -> Result<Value> {
        anim.extra
            .get(key)
            .or_else(|| side.and_then(|m| m.get(key)))
            .cloned()
            .with_context(|| format!("{name}: blend clip without `{key}`"))
    };
    Ok(Some(Blend {
        from: serde_json::from_value(need("from")?)
            .with_context(|| format!("{name}: blend `from`"))?,
        to: serde_json::from_value(need("to")?).with_context(|| format!("{name}: blend `to`"))?,
        from_frame: serde_json::from_value(need("from_frame")?)
            .with_context(|| format!("{name}: blend `from_frame`"))?,
        to_frame: serde_json::from_value(need("to_frame")?)
            .with_context(|| format!("{name}: blend `to_frame`"))?,
        blend_t: typed_field(name, &anim.extra, side, "blend_t")?,
        easing: typed_field(name, &anim.extra, side, "easing")?,
    }))
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
    for note in align_jiggle_anchors(&mut m) {
        eprintln!("{note}");
    }
    for note in map_jiggle_frames(&mut m)? {
        eprintln!("{note}");
    }
    Ok(m)
}

/// Baked-jiggle clips whose frame count differs from their clean clip must
/// carry `clean_frame_index` (unless both loop). Their own timing and
/// per-frame data are kept; per-frame data only the clean clip has
/// (`base_frames`, attachment `frame_points`, `mouth_sequence`,
/// `mouth_angle_deg`) is mapped through the index. Returns warnings (e.g.
/// a total duration that differs from the clean clip's).
fn map_jiggle_frames(m: &mut Manifest) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    let names: Vec<String> = m.clips.keys().cloned().collect();
    for name in names {
        let clip = &m.clips[&name];
        let Some(clean_name) = clip.jiggle_of.clone() else {
            continue;
        };
        let Some(clean) = m.clips.get(&clean_name).cloned() else {
            continue;
        };
        let n = clip.frames.len();
        let cfi = clip.clean_frame_index.clone();
        if cfi.is_empty() {
            let both_loop = clip.mode == LoopMode::Loop && clean.mode == LoopMode::Loop;
            ensure!(
                n == clean.frames.len() || both_loop,
                "{name}: {n} frames but `{clean_name}` has {}; needs clean_frame_index",
                clean.frames.len()
            );
            continue;
        }
        if let Some(&bad) = cfi.iter().find(|&&i| i as usize >= clean.frames.len()) {
            bail!(
                "{name}: clean_frame_index {bad} out of range for `{clean_name}` ({} frames)",
                clean.frames.len()
            );
        }
        let (dj, dc) = (clip.duration_s(), clean.duration_s());
        if (dj - dc).abs() > 0.002 * n.max(1) as f32 {
            notes.push(format!(
                "warning: {name}: one pass is {:.3}s but `{clean_name}` is {:.3}s",
                dj, dc
            ));
        }
        let map = |i: usize| cfi[i] as usize;
        let clip = m.clips.get_mut(&name).expect("present");
        if clip.base_frames.is_none() {
            if let Some(bf) = clean
                .base_frames
                .as_ref()
                .filter(|b| b.len() == clean.frames.len())
            {
                clip.base_frames = Some((0..n).map(|i| bf[map(i)]).collect());
                if clip.base_clip.is_none() {
                    clip.base_clip = clean.base_clip.clone();
                }
            }
        }
        for (key, pts) in &clean.frame_points {
            if pts.len() == clean.frames.len() && !clip.frame_points.contains_key(key) {
                clip.frame_points
                    .insert(key.clone(), (0..n).map(|i| pts[map(i)]).collect());
            }
        }
        if clip.mouth_sequence.is_empty() && clean.mouth_sequence.len() == clean.frames.len() {
            clip.mouth_sequence = (0..n)
                .map(|i| clean.mouth_sequence[map(i)].clone())
                .collect();
        }
        if clip.mouth_angle_deg.is_empty() && clean.mouth_angle_deg.len() == clean.frames.len() {
            clip.mouth_angle_deg = (0..n).map(|i| clean.mouth_angle_deg[map(i)]).collect();
        }
    }
    Ok(notes)
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
        if !clip.rule_events.is_empty() {
            println!("   rule_events {:?}", clip.rule_events);
        }
        if let Some(b) = &clip.blend {
            println!(
                "   blend {}[{}] -> {}[{}]  t {:?}",
                b.from,
                b.from_frame,
                b.to,
                b.to_frame,
                b.blend_t.as_deref().unwrap_or(&[])
            );
        }
        if !clip.frame_points.is_empty() {
            let cell = clip.frames[0];
            let outside: usize = clip
                .frame_points
                .values()
                .flatten()
                .filter(|&&(x, y)| x < 0 || y < 0 || x as u32 >= cell.w || y as u32 >= cell.h)
                .count();
            println!(
                "   frame_points {:?} x {} frames{}",
                clip.frame_points.keys().collect::<Vec<_>>(),
                clip.frames.len(),
                if outside > 0 {
                    format!("  [{outside} outside cell]")
                } else {
                    String::new()
                }
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
    let blends = m.clips.values().filter(|c| c.is_blend()).count();
    let events = m.clips.values().filter(|c| c.event.is_some()).count();
    let with_fp = m
        .clips
        .values()
        .filter(|c| !c.frame_points.is_empty())
        .count();
    let count = |f: &dyn Fn(&Clip) -> bool| m.clips.values().filter(|c| f(c)).count();
    let baked = count(&|c| c.jiggle_baked);
    let clean = m.clips.len() - baked;
    let paired = count(&|c| c.jiggle_variant.is_some());
    let masked = count(&|c| !c.spring_mask().is_empty());
    let allure = count(&|c| c.has_tag("allure"));
    let allure_clean = count(&|c| c.has_tag("allure") && !c.jiggle_baked);
    let idle_var = count(&|c| c.has_tag("idle_variant"));
    let idle_var_clean = count(&|c| c.has_tag("idle_variant") && !c.jiggle_baked);
    let timed = count(&|c| c.frame_durations_ms.is_some());
    let events_clean = count(&|c| c.event.is_some() && !c.jiggle_baked);
    let blends_clean = count(&|c| c.is_blend() && !c.jiggle_baked);
    println!(
        "{} clip(s) loaded and in bounds ({events} event, {blends} blend, {with_fp} with frame_points)",
        m.clips.len()
    );
    println!(
        "  {clean} engine-clean ({paired} with a jiggle_variant, {events_clean} event, {blends_clean} blend, \
         {allure_clean} allure, {idle_var_clean} idle_variant) + {baked} baked _jiggle \
         ({masked} with disable_springs)"
    );
    println!(
        "  tags: {allure} allure, {idle_var} idle_variant (incl. jiggle variants); \
         {timed} with per-frame durations"
    );
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
    fn gait_clips_carry_their_ground_speed() {
        // The rebuilt walk/strut/run clips add move_speed_px_s + a gait
        // object; the speed becomes a tag, the rest is ignored.
        let json: JsonManifest = serde_json::from_str(
            r#"{ "animations": [ { "name": "walk_right", "sheet": "w.png", "frame_count": 1,
                 "fps": 9, "loop": true, "anchor": [200, 392], "baseline_y": 392,
                 "move_speed_px_s": 134, "contact_frames": [0, 5],
                 "gait": { "cycle_s": 1.15, "duty_factor": 0.6, "per_frame": [] } } ] }"#,
        )
        .unwrap();
        let clip = convert(&json.animations[0], None, "w.png".into(), &[]).unwrap();
        assert!(clip.tags.contains(&"move_speed_px_s:134".to_string()));
        assert_eq!(clip.move_speed_px_s(), Some(134.0));
        let side: Value = serde_json::from_str(r#"{ "move_speed_px_s": 346.5 }"#).unwrap();
        let json: JsonManifest = serde_json::from_str(
            r#"{ "animations": [ { "name": "run_right", "sheet": "r.png", "frame_count": 1,
                 "fps": 9, "anchor": [200, 392] } ] }"#,
        )
        .unwrap();
        let clip = convert(&json.animations[0], side.as_object(), "r.png".into(), &[]).unwrap();
        assert_eq!(clip.move_speed_px_s(), Some(346.5));
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
    fn carries_mouth_patches_tilt_and_shape_rate() {
        let json: JsonManifest = serde_json::from_str(
            r#"{ "animations": [ { "name": "talk_loop", "sheet": "t.png", "frame_count": 2,
                 "fps": 5, "anchor": [200, 392], "baseline_y": 392, "dir": "talk_loop/",
                 "mouth_point": [193, 85],
                 "mouth_shapes": { "mouth_open": "talk_loop/mouth/mouth_open.png" } } ] }"#,
        )
        .unwrap();
        let side: Value = serde_json::from_str(
            r#"{ "mouth_angle_deg": [0.0, 1.5],
                 "lip_flap": {
                   "shapes": {
                     "mouth_open": { "patch": "talk_loop/mouth/mouth_open.png",
                                     "patch_size": [18, 13], "centre_in_patch": [9.0, 5.96] },
                     "mouth_closed": { "patch": "talk_loop/mouth/mouth_closed.png",
                                       "centre_in_patch": [9.0, 6.0] } },
                   "baked_sequence": ["mouth_closed", "mouth_open"],
                   "recommended_rate": { "mouth_shape_change_ms": [200, 260] } } }"#,
        )
        .unwrap();
        let clip = convert(&json.animations[0], side.as_object(), "t.png".into(), &[]).unwrap();
        assert_eq!(clip.mouth_patches.len(), 2);
        assert_eq!(
            clip.mouth_patches["open"],
            MouthPatch {
                page: "talk_loop/mouth/mouth_open.png".into(),
                centre: (9.0, 5.96)
            }
        );
        assert_eq!(clip.mouth_angle_deg, vec![0.0, 1.5]);
        assert_eq!(clip.mouth_shape_ms, Some((200, 260)));
        assert_eq!(clip.mouth_sequence, vec!["mouth_closed", "mouth_open"]);
        assert_eq!(clip.point("mouth_point"), Some((193, 85)));
    }

    #[test]
    fn maps_jiggle_subframes_through_clean_frame_index() {
        let json: JsonManifest = serde_json::from_str(
            r#"{ "animations": [
                { "name": "ev", "sheet": "e.png", "frame_count": 2, "fps": 5, "loop": false,
                  "anchor": [200, 392], "baseline_y": 392,
                  "jiggle_variant": "ev_jiggle", "frame_durations_ms": [200, 400] },
                { "name": "ev_jiggle", "sheet": "j.png", "frame_count": 4, "fps": 5, "loop": false,
                  "anchor": [200, 392], "baseline_y": 392, "jiggle_baked": true, "jiggle_of": "ev",
                  "engine_springs": "disable chest/pigtails/hip for this clip",
                  "clean_frame_index": [0, 1, 1, 1], "frame_durations_ms": [200, 134, 133, 133] },
                { "name": "think", "sheet": "t.png", "frame_count": 3, "fps": 5,
                  "anchor": [200, 392], "baseline_y": 392 } ] }"#,
        )
        .unwrap();
        let a = &json.animations;
        let ev_side: Value = serde_json::from_str(
            r#"{ "base_clip": "think", "base_frames": [2, 0],
                 "frame_points": { "hand_point": [[10, 10], [20, 20]] } }"#,
        )
        .unwrap();
        let jig_side: Value = serde_json::from_str(
            r#"{ "frame_points": { "chest_point": [[1, 1], [2, 2], [3, 3], [4, 4]] } }"#,
        )
        .unwrap();
        let mut m = Manifest::default();
        m.clips.insert(
            "ev".into(),
            convert(&a[0], ev_side.as_object(), "e.png".into(), &[]).unwrap(),
        );
        m.clips.insert(
            "ev_jiggle".into(),
            convert(&a[1], jig_side.as_object(), "j.png".into(), &[]).unwrap(),
        );
        m.clips.insert(
            "think".into(),
            convert(&a[2], None, "t.png".into(), &[]).unwrap(),
        );
        let notes = map_jiggle_frames(&mut m).unwrap();
        assert!(notes.is_empty(), "{notes:?}");
        let j = &m.clips["ev_jiggle"];
        // Own timing and own per-frame data kept; clean-only data mapped.
        assert_eq!(
            j.frame_durations_ms.as_deref(),
            Some(&[200, 134, 133, 133][..])
        );
        assert_eq!(j.clean_frame_index, vec![0, 1, 1, 1]);
        assert_eq!(j.frame_points["chest_point"].len(), 4);
        assert_eq!(
            j.frame_points["hand_point"],
            vec![(10, 10), (20, 20), (20, 20), (20, 20)]
        );
        assert_eq!(j.base_frames.as_deref(), Some(&[2, 0, 0, 0][..]));
        assert_eq!(j.base_clip.as_deref(), Some("think"));
        let back = Manifest::from_toml_str(&toml::to_string(&m).unwrap()).unwrap();
        assert_eq!(back.clips["ev_jiggle"].clean_frame(3), 1);
        // Without the index the length mismatch is an error.
        m.clips
            .get_mut("ev_jiggle")
            .unwrap()
            .clean_frame_index
            .clear();
        assert!(map_jiggle_frames(&mut m).is_err());
        // A different total duration is reported.
        let mut m2 = m.clone();
        let j = m2.clips.get_mut("ev_jiggle").unwrap();
        j.clean_frame_index = vec![0, 1, 1, 1];
        j.frame_durations_ms = Some(vec![200, 200, 200, 200]);
        assert_eq!(map_jiggle_frames(&mut m2).unwrap().len(), 1);
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
            r#"{ "seat_y": 230, "seat_x_range": [136, 279], "feet_lowest_y": 364,
                 "root_motion": [20.5], "contact_frames": [0] }"#,
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
        assert_eq!(sit.root_motion.as_deref(), Some(&[20.5][..]));
        assert_eq!(sit.contact_frames.as_deref(), Some(&[0][..]));
        assert_eq!(think.root_motion, None);
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

    #[test]
    fn carries_frame_points_blend_and_rule_events() {
        let json: JsonManifest = serde_json::from_str(
            r#"{ "animations": [
              { "name": "idle", "sheet": "i.png", "frame_count": 2, "fps": 8 },
              { "name": "wave", "sheet": "w.png", "frame_count": 1, "fps": 8 },
              { "name": "blend_idle_to_wave", "sheet": "b.png", "frame_count": 2, "fps": 12,
                "loop": false, "scale": "interpolated between the two clips" } ] }"#,
        )
        .unwrap();
        let idle_side: Value = serde_json::from_str(
            r#"{ "chest_point": [240, 130],
                 "frame_points": { "chest_point": [[241, 131], [243, 129]] } }"#,
        )
        .unwrap();
        let wave_side: Value = serde_json::from_str(
            r#"{ "event": "window.closed", "rule_events": ["window.closed"] }"#,
        )
        .unwrap();
        let blend_side: Value = serde_json::from_str(
            r#"{ "blend": true, "from": "idle", "to": "wave", "from_frame": 1, "to_frame": 0,
                 "blend_t": [0.33, 0.66], "easing": "smoothstep",
                 "scale": "interpolated between the two clips" }"#,
        )
        .unwrap();
        let a = &json.animations;
        let idle = convert(&a[0], idle_side.as_object(), "i.png".into(), &[]).unwrap();
        let wave = convert(&a[1], wave_side.as_object(), "w.png".into(), &[]).unwrap();
        let bl = convert(&a[2], blend_side.as_object(), "b.png".into(), &[]).unwrap();
        assert_eq!(idle.point("chest_point"), Some((240, 130)));
        assert_eq!(
            idle.frame_points["chest_point"],
            vec![(241, 131), (243, 129)]
        );
        assert_eq!(idle.point_at("chest_point", 1), Some((243, 129)));
        assert_eq!(wave.rule_events, vec!["window.closed"]);
        let b = bl.blend.clone().unwrap();
        assert_eq!(
            (b.from.as_str(), b.from_frame, b.to.as_str(), b.to_frame),
            ("idle", 1, "wave", 0)
        );
        assert_eq!(b.blend_t, Some(vec![0.33, 0.66]));
        assert!(bl.tags.contains(&"blend".to_string()));
        assert!(bl.tags.contains(&"scale:interpolated".to_string()));
        assert_eq!(bl.scale, None);
        let m = Manifest {
            clips: [
                ("idle".to_string(), idle),
                ("wave".to_string(), wave),
                ("blend_idle_to_wave".to_string(), bl),
            ]
            .into(),
        };
        let back = Manifest::from_toml_str(&toml::to_string(&m).unwrap()).unwrap();
        assert_eq!(back.clips, m.clips);
    }

    #[test]
    fn rejects_bad_frame_points_and_incomplete_blends() {
        let json: JsonManifest = serde_json::from_str(
            r#"{ "animations": [ { "name": "x", "sheet": "x.png", "frame_count": 2, "fps": 8 } ] }"#,
        )
        .unwrap();
        let short: Value =
            serde_json::from_str(r#"{ "frame_points": { "hip_point": [[1, 1]] } }"#).unwrap();
        assert!(convert(&json.animations[0], short.as_object(), "x.png".into(), &[]).is_err());
        let partial: Value = serde_json::from_str(r#"{ "blend": true, "from": "idle" }"#).unwrap();
        assert!(convert(
            &json.animations[0],
            partial.as_object(),
            "x.png".into(),
            &[]
        )
        .is_err());
    }

    #[test]
    fn carries_jiggle_pairs_engine_springs_and_tags() {
        let json: JsonManifest = serde_json::from_str(
            r#"{ "animations": [
              { "name": "idle_flirt_hip_lean", "sheet": "c.png", "frame_count": 2, "fps": 6,
                "loop": false, "frame_durations_ms": [167, 600], "allure": true,
                "idle_variant": true, "retime": { "fps": 6 },
                "jiggle_variant": "idle_flirt_hip_lean_jiggle",
                "anchor": [200, 392], "baseline_y": 392, "body_anchor_x": 169 },
              { "name": "idle_flirt_hip_lean_jiggle", "sheet": "j.png", "frame_count": 2, "fps": 6,
                "loop": false, "frame_durations_ms": [167, 600], "allure": true,
                "idle_variant": true, "retime": { "fps": 6 }, "jiggle_baked": true, "jiggle_of": "idle_flirt_hip_lean",
                "engine_springs": "disable chest/pigtails/hip for this clip",
                "anchor": [200, 392], "baseline_y": 392, "body_anchor_x": 231 } ] }"#,
        )
        .unwrap();
        let m = build(&json, Path::new("."), Path::new("."), &[]).unwrap();
        let (c, j) = (
            &m.clips["idle_flirt_hip_lean"],
            &m.clips["idle_flirt_hip_lean_jiggle"],
        );
        assert_eq!(
            c.jiggle_variant.as_deref(),
            Some("idle_flirt_hip_lean_jiggle")
        );
        assert!(!c.jiggle_baked && c.disable_springs.is_empty());
        assert!(j.jiggle_baked);
        assert_eq!(j.jiggle_of.as_deref(), Some("idle_flirt_hip_lean"));
        assert_eq!(j.disable_springs, ["chest", "pigtails", "hip"]);
        assert!(j.spring_mask().contains(Channel::Chest));
        for clip in [c, j] {
            for t in ["allure", "idle_variant", "retimed"] {
                assert!(clip.has_tag(t), "{t}");
            }
            assert_eq!(clip.frame_durations_ms.as_deref(), Some(&[167, 600][..]));
        }
        // Anchor mismatch on the variant is fixed from the clean clip.
        assert_eq!(j.frames[1].anchor, (169, 392));
        let back = Manifest::from_toml_str(&toml::to_string(&m).unwrap()).unwrap();
        assert_eq!(back.clips, m.clips);
    }

    #[test]
    fn engine_springs_must_name_known_groups() {
        let json: JsonManifest = serde_json::from_str(
            r#"{ "animations": [ { "name": "x", "sheet": "x.png", "frame_count": 1, "fps": 6 } ] }"#,
        )
        .unwrap();
        for bad in [
            r#"{ "engine_springs": "keep them all" }"#,
            r#"{ "engine_springs": ["chest", "elbow"] }"#,
            r#"{ "engine_springs": 3 }"#,
        ] {
            let side: Value = serde_json::from_str(bad).unwrap();
            assert!(
                convert(&json.animations[0], side.as_object(), "x.png".into(), &[]).is_err(),
                "{bad}"
            );
        }
        let ok: Value = serde_json::from_str(r#"{ "engine_springs": ["pig", "hips"] }"#).unwrap();
        let c = convert(&json.animations[0], ok.as_object(), "x.png".into(), &[]).unwrap();
        assert_eq!(c.disable_springs, ["pigtails", "hip"]);
    }
}
