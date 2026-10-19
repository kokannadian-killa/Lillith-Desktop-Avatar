//! `slice-sheets`: offline asset pipeline (PLAN.md §4). TODO(M0b).
//!
//! 1. 6 alpha sheets (1792×1008, 3×10, cell 179.2×336): per row, find each
//!    body's alpha centroid near `(c+0.5)*179.2`, crop an overlapping
//!    ~240×336 window, drop neighbours' connected components, record the
//!    feet anchor (lowest opaque pixel).
//! 2. Baked-checkerboard images: flood-fill the two checker greys from the borders.
//! 3. 52 pose stills: background removal, ×2 upscale, name per pose map.
//! 4. 80 frames (black bg): key black → alpha, order by similarity, split loops.
//! 5. Write atlases + `manifest.toml` (clip → frames, fps, mode, anchor,
//!    per-frame input-mask runs, tags).

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "slice-sheets", about = "Build Lillith atlases from source art")]
struct Args {
    /// Source art directory (sheets/, poses/, reference/, frames_640x960/).
    #[arg(long, default_value = "assets/source")]
    input: PathBuf,
    /// Output directory for atlases + manifest.toml.
    #[arg(long, default_value = "assets/atlas")]
    output: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let sheets = args.input.join("sheets");
    let mut count = 0usize;
    if sheets.is_dir() {
        for entry in std::fs::read_dir(&sheets).with_context(|| format!("reading {sheets:?}"))? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "png") {
                let (w, h) =
                    image::image_dimensions(&path).with_context(|| format!("probing {path:?}"))?;
                println!("{}: {w}x{h} (slicing TODO)", path.display());
                count += 1;
            }
        }
    }
    println!(
        "found {count} sheet(s) in {}; output would go to {}",
        sheets.display(),
        args.output.display()
    );
    Ok(())
}
