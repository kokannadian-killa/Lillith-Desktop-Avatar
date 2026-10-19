//! `lillith preview`: headless M2 clip preview (no window needed).

use anyhow::{bail, Context, Result};
use lillith_anim::animator::{Animator, AnimatorConfig, Layer};
use lillith_anim::atlas::{default_manifest_path, Manifest};
use lillith_anim::preview::{clip_frames, contact_sheet, overview, save_png, PreviewOpts};
use lillith_anim::render::PageCache;
use std::path::{Path, PathBuf};

#[derive(Debug, clap::Args)]
pub struct PreviewArgs {
    /// Clip to preview (see `--list`).
    pub clip: Option<String>,
    /// Path to manifest.toml (default: $LILLITH_MANIFEST,
    /// ./assets/generated/manifest.toml, ~/.local/share/lillith/generated/manifest.toml).
    #[arg(long)]
    pub manifest: Option<PathBuf>,
    /// List every clip with frames, timing and metadata.
    #[arg(long)]
    pub list: bool,
    /// Write an overview grid plus one contact sheet per clip.
    #[arg(long)]
    pub all: bool,
    /// Play through the animator with spring physics (breathing, impulses,
    /// footfalls) instead of raw frames.
    #[arg(long)]
    pub physics: bool,
    /// Also write every frame as its own PNG.
    #[arg(long)]
    pub frames: bool,
    /// Output directory.
    #[arg(long, default_value = "preview")]
    pub out: PathBuf,
    /// Output cell size in px.
    #[arg(long, default_value_t = 200)]
    pub cell: u32,
    /// Columns in contact sheets (0 = one row).
    #[arg(long, default_value_t = 0)]
    pub cols: usize,
    /// Simulate idle life for N seconds and print the timeline (blinks,
    /// idle transitions, idle reactions, spring state).
    #[arg(long, value_name = "SECONDS")]
    pub sim: Option<f32>,
    /// Seed for `--sim`.
    #[arg(long, default_value_t = 1)]
    pub seed: u64,
}

fn resolve(path: Option<PathBuf>) -> Result<PathBuf> {
    let p = match path {
        Some(p) => p,
        None => default_manifest_path().context(
            "no manifest.toml found; pass --manifest or set LILLITH_MANIFEST \
             (convert a generated manifest.json with `cargo run -p import-manifest`)",
        )?,
    };
    if !p.is_file() {
        let json = p.with_extension("json");
        if json.is_file() {
            bail!(
                "{} missing; run `cargo run -p import-manifest -- --input {} --tag canonical --check` first",
                p.display(),
                json.display()
            );
        }
        bail!("{} not found", p.display());
    }
    Ok(p)
}

pub fn run(a: PreviewArgs) -> Result<()> {
    let path = resolve(a.manifest)?;
    let manifest = Manifest::load(&path).with_context(|| format!("loading {}", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut pages = PageCache::new(&dir);

    if a.list {
        println!(
            "{:<28} {:>6} {:>6} {:>5} {:>7}  extras",
            "clip", "frames", "fps", "mode", "secs"
        );
        for (name, c) in &manifest.clips {
            let mut extras = Vec::new();
            if c.frame_durations_ms.is_some() {
                extras.push("per-frame-ms".to_string());
            }
            if let Some(e) = &c.event {
                extras.push(format!("event={e}"));
            }
            if !c.points.is_empty() {
                extras.push(format!(
                    "points={}",
                    c.points.keys().cloned().collect::<Vec<_>>().join(",")
                ));
            }
            if c.seat_y.is_some() {
                extras.push("seat".into());
            }
            extras.extend(c.tags.iter().filter(|t| t.starts_with("facing:")).cloned());
            println!(
                "{:<28} {:>6} {:>6.1} {:>5} {:>7.2}  {}",
                name,
                c.frames.len(),
                c.fps,
                format!("{:?}", c.mode).to_lowercase(),
                c.duration_s(),
                extras.join(" ")
            );
        }
        println!("{} clips", manifest.clips.len());
        return Ok(());
    }

    if let Some(secs) = a.sim {
        return simulate(manifest, secs, a.seed);
    }

    std::fs::create_dir_all(&a.out)?;
    let opts = PreviewOpts {
        cell: a.cell,
        physics: a.physics,
        ..PreviewOpts::default()
    };
    let names: Vec<String> = if a.all {
        let (grid, names) = overview(&manifest, &mut pages, a.cell.min(160), 8)?;
        let p = a.out.join("overview.png");
        save_png(&grid, &p)?;
        std::fs::write(a.out.join("overview.txt"), names.join("\n") + "\n")?;
        println!(
            "{} ({} clips, 8 per row; names in overview.txt)",
            p.display(),
            names.len()
        );
        names
    } else if let Some(c) = a.clip {
        if !manifest.clips.contains_key(&c) {
            bail!("unknown clip `{c}`; try `lillith preview --list`");
        }
        vec![c]
    } else {
        bail!("give a clip name, --list, --all or --sim");
    };

    for name in names {
        let frames = clip_frames(&manifest, &mut pages, &name, &opts)?;
        let cols = if a.cols == 0 { frames.len() } else { a.cols };
        let canvases: Vec<_> = frames.iter().map(|f| f.canvas.clone()).collect();
        let sheet = contact_sheet(&canvases, cols, [72, 72, 80, 255]);
        let suffix = if a.physics { "_physics" } else { "" };
        let p = a.out.join(format!("{name}{suffix}.png"));
        save_png(&sheet, &p)?;
        if a.frames {
            let fdir = a.out.join(format!("{name}{suffix}"));
            std::fs::create_dir_all(&fdir)?;
            for (i, f) in frames.iter().enumerate() {
                save_png(&f.canvas, &fdir.join(format!("{name}_{i:03}.png")))?;
            }
        }
        if a.physics {
            let max = frames
                .iter()
                .map(|f| f.transform.y.abs().max(f.transform.chest_offset.abs()))
                .fold(0.0f32, f32::max);
            println!(
                "{} ({} samples @ {} fps, peak spring offset {max:.1}px)",
                p.display(),
                frames.len(),
                opts.sample_fps
            );
        } else {
            println!("{} ({} frames)", p.display(), frames.len());
        }
    }
    Ok(())
}

/// Prints layer/clip changes over `secs` of idle life on a fake clock.
fn simulate(manifest: Manifest, secs: f32, seed: u64) -> Result<()> {
    let mut anim = Animator::new(
        manifest,
        AnimatorConfig {
            seed,
            ..AnimatorConfig::default()
        },
    )?;
    let dt = 1.0 / 60.0;
    let mut last: Option<(Layer, String)> = None;
    let mut t = 0.0;
    let mut blinks = 0;
    while t < secs {
        anim.tick(dt);
        t += dt;
        let f = anim.current();
        let key = (f.layer, f.clip_name.to_string());
        if last.as_ref() != Some(&key) {
            if f.layer == Layer::Blink {
                blinks += 1;
            }
            let tr = f.transform;
            println!(
                "{t:7.2}s  {:<10} {:<22} scale={:.4} chest={:+.2} y={:+.2} angle={:+.3}",
                format!("{:?}", f.layer),
                f.clip_name,
                tr.scale,
                tr.chest_offset,
                tr.y,
                tr.angle
            );
            last = Some(key);
        }
    }
    println!(
        "{blinks} blinks in {secs:.0}s; breath period now {:.2}s",
        anim.idle_life().breath_period()
    );
    Ok(())
}
