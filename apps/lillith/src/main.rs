//! `lillith`: the desktop companion binary.
//!
//! Wires the crates together (PLAN.md §1.2 "Runtime"): the render thread owns
//! the overlay (30 Hz, physics sub-stepped at 120 Hz, sprites ~12 fps); a
//! tokio runtime runs sensors, LLM and voice; everything talks over one
//! event bus into `lillith-core`, which emits commands back to the overlay.
//!
//! Status: M1 overlay + M2 animation engine + M3 behaviour + M4 system
//! monitor (`lillith-sense`, events fed into the brain every frame;
//! `--no-monitor` / `[monitor] enabled = false` turn it off, `lillith status`
//! prints a snapshot, `lillith monitor` prints live events). Plain `lillith`
//! puts her on the desktop (Wayland layer-shell, or the X11 fallback) playing
//! idle life with springs, driven by `lillith_core::state::Brain`: left /
//! double / right / middle click, hover and sustained hover, drag (position
//! is saved), README-SP keys while she has focus (`G` Wander/Stay, `S`
//! sleep, `Q`/`Esc` quit), the secret Left → Double → Right sequence, moods,
//! mischief, sleep/wake and wandering. `lillith event <name>` resolves a rule from
//! `assets/rules.toml`; `lillith preview` lists/plays/renders every manifest
//! clip through the animator headlessly.

use anyhow::Result;
use clap::{Parser, Subcommand};
use lillith_core::config::{Config, MoveMode, OverlayPos};
use lillith_core::{rules::RuleSet, Event};
use lillith_overlay::{Backend, OverlayOptions};
use std::path::PathBuf;

mod preview;

#[derive(Debug, Parser)]
#[command(name = "lillith", version, about = "Lillith desktop companion")]
struct Cli {
    /// Force a windowing backend (default: Wayland if WAYLAND_DISPLAY is set).
    #[arg(long, value_parser = parse_backend)]
    backend: Option<Backend>,
    /// Do not create the tray icon.
    #[arg(long)]
    no_tray: bool,
    /// Disable TTS/STT.
    #[arg(long)]
    no_voice: bool,
    /// Disable the local LLM (offline personality lines only).
    #[arg(long)]
    no_llm: bool,
    /// Disable the M4 system monitor (CPU/RAM/battery/network/idle/
    /// notifications/files/KWin bridge); overrides `[monitor] enabled`.
    #[arg(long)]
    no_monitor: bool,
    /// Draw the baked-jiggle `<clip>_jiggle` variants (when the animation set
    /// has them) instead of the engine-clean clips; the engine's chest /
    /// pigtail / hip springs switch off while one is drawn. Overrides config
    /// `baked_jiggle`.
    #[arg(long)]
    baked_jiggle: bool,
    /// manifest.toml of the animation set (default: $LILLITH_MANIFEST,
    /// ./assets/generated/manifest.toml, ~/.local/share/lillith/generated/manifest.toml).
    #[arg(long)]
    manifest: Option<PathBuf>,
    /// Config file (default: ~/.config/lillith/config.toml).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Overlay size multiplier on the 400 px cell (default: config `scale`, else 0.55 ≈ 220 px).
    #[arg(long)]
    scale: Option<f32>,
    /// Whole-sprite opacity 0..1 (default: config `opacity`).
    #[arg(long)]
    opacity: Option<f32>,
    /// Redraw cap in Hz (default: config `render_hz`, 30).
    #[arg(long)]
    fps: Option<f32>,
    /// Ignore the saved position and start at the default spot.
    #[arg(long)]
    reset_position: bool,
    /// Do not write the position back to config.toml after a drag.
    #[arg(long)]
    no_save: bool,
    /// Movement mode: wander (walk around when idle) or stay (default: config `mode`).
    #[arg(long, value_parser = parse_mode)]
    mode: Option<MoveMode>,
    /// Seconds without interaction until she gets drowsy (default: config `[behavior]`, 300).
    #[arg(long, value_name = "SECONDS")]
    sleepy_after: Option<f32>,
    /// Seconds without interaction until she falls asleep (default 900).
    #[arg(long, value_name = "SECONDS")]
    asleep_after: Option<f32>,
    /// Quit after N seconds (smoke tests, CPU measurements).
    #[arg(long, value_name = "SECONDS")]
    exit_after: Option<f32>,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Fire an event through the rule table (debug injector), e.g. `battery.low`.
    Event {
        name: String,
        /// Optional detail, e.g. a notification summary.
        detail: Option<String>,
    },
    /// List all rules.
    Rules,
    /// One system-monitor snapshot: CPU, RAM, load, battery, backlight,
    /// network, VPN, removable drives (reads procfs/sysfs, ~1 s).
    Status,
    /// Run only the system monitor and print every event it emits
    /// (no overlay; Ctrl-C to stop). Handy to test thresholds.
    Monitor {
        /// Stop after N seconds.
        #[arg(long, value_name = "SECONDS")]
        exit_after: Option<f32>,
    },
    /// Open the chat/settings panel window (Poses tab = clip browser).
    Panel {
        /// manifest.toml for the Poses tab (default: same lookup as `preview`).
        #[arg(long)]
        manifest: Option<std::path::PathBuf>,
    },
    /// Headless clip preview: list clips, write contact sheets / frames,
    /// or simulate idle life (M2).
    Preview(preview::PreviewArgs),
}

fn parse_backend(s: &str) -> Result<Backend, String> {
    s.parse()
}

fn parse_mode(s: &str) -> Result<MoveMode, String> {
    match s.to_ascii_lowercase().as_str() {
        "wander" => Ok(MoveMode::Wander),
        "stay" => Ok(MoveMode::Stay),
        "follow" => Ok(MoveMode::Follow),
        other => Err(format!(
            "unknown mode {other} (expected wander|stay|follow)"
        )),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lillith=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let rules = RuleSet::bundled();

    match cli.command {
        Some(Cmd::Event { name, detail }) => {
            let ev = match detail {
                Some(d) => Event::with_detail(name, d),
                None => Event::new(name),
            };
            match rules.find(&ev.name) {
                Some(rule) => println!(
                    "{} -> clip={:?} pose={} then={:?} gesture={:?} mood={:?} sfx={:?} line={:?}",
                    ev.name,
                    rules.clip_for(&ev.name),
                    rule.pose,
                    rule.then,
                    rule.gesture,
                    rule.mood,
                    rule.sfx,
                    rule.line_for(&ev)
                ),
                None => anyhow::bail!("no rule for event {:?}", ev.name),
            }
        }
        Some(Cmd::Rules) => {
            for r in &rules.rules {
                println!(
                    "{:<26} {:<20} {}",
                    r.event,
                    r.pose,
                    rules.clip_for(&r.event).unwrap_or("-")
                );
            }
        }
        Some(Cmd::Status) => {
            let cfg = load_config(&cli).0.monitor;
            let sys = lillith_sense::sysfs::Sys::default();
            let mut m = lillith_sense::Monitor::new(cfg.clone());
            m.step(0.0, &lillith_sense::Sample::read(&sys, &cfg));
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let s = lillith_sense::Sample::read(&sys, &cfg);
            m.step(1.0, &s);
            println!("{}", m.summary(&s));
        }
        Some(Cmd::Monitor { exit_after }) => {
            let mut cfg = load_config(&cli).0.monitor;
            cfg.enabled = true;
            let sink: lillith_sense::Sink = std::sync::Arc::new(|ev: Event| match &ev.detail {
                Some(d) => println!("{} ({d})", ev.name),
                None => println!("{}", ev.name),
            });
            let _h = lillith_sense::spawn(cfg, sink);
            match exit_after {
                Some(s) => tokio::time::sleep(std::time::Duration::from_secs_f32(s)).await,
                None => tokio::signal::ctrl_c().await?,
            }
        }
        Some(Cmd::Panel { manifest }) => {
            lillith_panel::run_with(manifest.or(cli.manifest))
                .map_err(|e| anyhow::anyhow!("panel: {e}"))?;
        }
        Some(Cmd::Preview(args)) => preview::run(args)?,
        None => run_overlay(&cli, rules.rules.len())?,
    }
    Ok(())
}

/// Default overlay scale when config.toml leaves `scale` at 1.0: a 220 px
/// tall window from the 400 px cells.
const DEFAULT_OVERLAY_SCALE: f32 = 0.55;

/// Loads `--config` / `~/.config/lillith/config.toml` (defaults on error).
fn load_config(cli: &Cli) -> (Config, Option<PathBuf>) {
    let config_path = cli.config.clone().or_else(Config::default_path);
    let cfg = match &config_path {
        Some(p) if p.exists() => Config::load(p).unwrap_or_else(|e| {
            tracing::warn!("{}: {e}; using defaults", p.display());
            Config::default()
        }),
        _ => Config::default(),
    };
    (cfg, config_path)
}

/// Starts the system monitor (unless disabled) and returns the feed the
/// overlay drains plus the handle that keeps the monitors alive.
fn start_monitor(
    cli: &Cli,
    cfg: &Config,
) -> (
    Option<lillith_overlay::EventFeed>,
    Option<lillith_sense::MonitorHandle>,
) {
    if cli.no_monitor || !cfg.monitor.enabled {
        tracing::info!("system monitor off (--no-monitor or [monitor] enabled = false)");
        return (None, None);
    }
    let (tx, rx) = std::sync::mpsc::channel::<Event>();
    let sink: lillith_sense::Sink = std::sync::Arc::new(move |ev| {
        let _ = tx.send(ev);
    });
    let handle = lillith_sense::spawn(cfg.monitor.clone(), sink);
    (
        Some(std::sync::Arc::new(std::sync::Mutex::new(rx))),
        Some(handle),
    )
}

fn run_overlay(cli: &Cli, rules: usize) -> Result<()> {
    let backend = Backend::detect_env(cli.backend);
    let (cfg, config_path) = load_config(cli);
    let manifest = preview::resolve(cli.manifest.clone())?;
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let scale = cli
        .scale
        .unwrap_or(if (cfg.scale - 1.0).abs() < f32::EPSILON {
            DEFAULT_OVERLAY_SCALE
        } else {
            cfg.scale
        });
    let mut behavior = cfg.behavior.clone();
    if let Some(s) = cfg.pose_cycle_s {
        behavior.pose_cycle_s = s;
    }
    if let Some(s) = cli.sleepy_after {
        behavior.sleepy_after_s = s;
    }
    if let Some(s) = cli.asleep_after {
        behavior.asleep_after_s = s;
    }
    if behavior.asleep_after_s < behavior.sleepy_after_s {
        behavior.sleepy_after_s = behavior.asleep_after_s;
    }
    let mode = cli.mode.unwrap_or(cfg.mode);
    let opts = OverlayOptions {
        manifest: manifest.clone(),
        scale,
        opacity: cli.opacity.unwrap_or(cfg.opacity),
        render_hz: cli.fps.unwrap_or(cfg.render_hz),
        animator: lillith_anim::animator::AnimatorConfig {
            springs: cfg.springs.clone(),
            physics_hz: cfg.physics_hz,
            idle: behavior.idle.clone(),
            seed,
            // Tuned jiggle physics from assets/physics.toml (PhysicsDriver).
            physics_driver: true,
            prefer_jiggle_variants: cli.baked_jiggle || cfg.baked_jiggle,
            ..Default::default()
        },
        position: if cli.reset_position {
            OverlayPos::default()
        } else {
            cfg.overlay.clone()
        },
        config_path: if cli.no_save {
            None
        } else {
            config_path.clone()
        },
        forced: cli.backend.is_some(),
        exit_after: cli.exit_after.map(std::time::Duration::from_secs_f32),
        behavior,
        mode,
        muted: cfg.muted,
        events: None,
    };
    tracing::info!(
        ?backend,
        manifest = %manifest.display(),
        config = ?config_path,
        scale,
        tray = !cli.no_tray,
        voice = !cli.no_voice,
        llm = !cli.no_llm,
        monitor = !cli.no_monitor && cfg.monitor.enabled,
        rules,
        ?mode,
        "Lillith overlay (M4): click/hover/drag her; keys while focused (G wander/stay, S sleep, Q/Esc quit)"
    );
    let (feed, _monitor) = start_monitor(cli, &cfg);
    let opts = OverlayOptions {
        events: feed,
        ..opts
    };
    lillith_overlay::run(backend, opts).map_err(|e| anyhow::anyhow!("overlay: {e}"))
}
