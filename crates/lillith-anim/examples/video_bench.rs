//! Pose-video vs PNG-sheet cost on this box.
//!
//! `cargo run --release -p lillith-anim --example video_bench -- [manifest.toml] [clip...]`
//!
//! For each clip (default: all with a `video`): PNG = load + decode the
//! sheet and crop every cell; MP4 = time to the first frame and to the last
//! frame through [`VideoCache`] (standby ffmpeg, streaming), as the overlay
//! does on a clip change. Then plays every clip through one budgeted cache
//! and prints the resident RAM, evictions and process RSS.

use lillith_anim::atlas::Manifest;
use lillith_anim::render::PageCache;
use lillith_anim::video::{VideoCache, VideoConfig};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok())
        })
        .map_or(0.0, |kb| kb / 1024.0)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "assets/generated/manifest.toml".into()),
    );
    let only: Vec<String> = args.collect();
    let m = Manifest::load(&path).expect("manifest");
    let dir = path.parent().unwrap().to_path_buf();
    let mut names: Vec<&String> = m
        .clips
        .iter()
        .filter(|(n, c)| c.video.is_some() && (only.is_empty() || only.contains(n)))
        .map(|(n, _)| n)
        .collect();
    names.sort();
    // One clip per distinct video (aliases share files).
    names.dedup_by_key(|n| m.clips[*n].video.clone());

    let (mut png_ms, mut first_ms, mut full_ms) = (vec![], vec![], vec![]);
    let (mut png_bytes, mut frames_total) = (0usize, 0usize);
    let cfg = VideoConfig::from_env();
    // One cache for the whole run, like the overlay: the standby ffmpeg is
    // warm and every clip is a cold decode (different file each time).
    let mut vc = VideoCache::new(cfg.clone());
    if let Some(n) = names.first() {
        let c = &m.clips[*n];
        let (w, h) = c.cell_size();
        vc.wait_frame(&c.video_path(&dir).unwrap(), w, h, c.frames.len(), 0);
    }
    for n in &names[1..] {
        let c = &m.clips[*n];
        let (w, h) = c.cell_size();
        frames_total += c.frames.len();

        let t = Instant::now();
        let mut pc = PageCache::with_video(
            &dir,
            VideoConfig {
                enabled: false,
                ..cfg.clone()
            },
        );
        for i in 0..c.frames.len() {
            pc.page_cell(c, i).expect("png cell");
        }
        png_ms.push(ms(t.elapsed()));
        png_bytes += pc.page_bytes();

        // Let the standby respawn finish, as between real clip changes.
        std::thread::sleep(Duration::from_millis(250));
        let p = c.video_path(&dir).unwrap();
        let t = Instant::now();
        vc.wait_frame(&p, w, h, c.frames.len(), 0)
            .expect("first frame");
        first_ms.push(ms(t.elapsed()));
        vc.wait_frame(&p, w, h, c.frames.len(), c.frames.len() - 1)
            .expect("last frame");
        full_ms.push(ms(t.elapsed()));
    }
    let n = names.len() - 1;
    println!("{n} clip videos, {frames_total} frames");
    println!(
        "PNG sheet load+crop per clip:   median {:6.1} ms  p95 {:6.1} ms  (page RAM if all held: {} MiB)",
        pct(&mut png_ms.clone(), 0.5),
        pct(&mut png_ms, 0.95),
        png_bytes >> 20
    );
    println!(
        "MP4 first frame on clip change: median {:6.1} ms  p95 {:6.1} ms",
        pct(&mut first_ms.clone(), 0.5),
        pct(&mut first_ms, 0.95)
    );
    println!(
        "MP4 whole clip decoded:         median {:6.1} ms  p95 {:6.1} ms  ({:.2} ms/frame)",
        pct(&mut full_ms.clone(), 0.5),
        pct(&mut full_ms.clone(), 0.95),
        full_ms.iter().sum::<f64>() / frames_total as f64
    );

    // Budgeted playback of everything, as a long session would.
    let rss0 = rss_mb();
    let mut pc = PageCache::new(&dir);
    for name in &names {
        let c = &m.clips[*name];
        for i in 0..c.frames.len() {
            pc.cell(c, i).expect("cell");
        }
    }
    let s = pc.video().stats();
    println!(
        "budget {} MiB: resident {} clips / {} MiB, {} decodes, {} evictions, {} failures; PNG pages held {} MiB; RSS {:.0} -> {:.0} MiB",
        pc.video().config().budget_bytes >> 20,
        s.resident_clips,
        s.resident_bytes >> 20,
        s.decodes,
        s.evictions,
        s.failures,
        pc.page_bytes() >> 20,
        rss0,
        rss_mb()
    );
}
