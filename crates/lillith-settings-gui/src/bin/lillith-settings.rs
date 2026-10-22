//! `lillith-settings [CONFIG_PATH] [options]` — Lillith's settings window.

use lillith_settings_gui::{run_with, Options};
use std::path::PathBuf;

const HELP: &str = "lillith-settings — Lillith's settings / setup wizard window

USAGE: lillith-settings [CONFIG_PATH] [OPTIONS]

  CONFIG_PATH          config.toml to edit (default: ~/.config/lillith/config.toml)
  --tab ID             open a tab: companion motion brain avatar voice about sealed-1..3
  --step NAME          Chat/Brain wizard step: avatar runtime pick-model persona test-save;
                       Companion: poses/N opens Poses category N;
                       Avatar: import:PATH imports an image, anchors:ID edits one
  --size WxH           initial window size (default 960x660, min 640x480)
  --decorated          use the window manager's title bar
  --manifest PATH      animation set manifest.toml for Companion ▸ Poses
                       (default: the overlay's lookup)
  --quiet              don't print menu_action lines on stdout
  --screenshot PNG     save the first settled frame to PNG and exit
  -h, --help           this help

Companion picks are saved to config.toml and printed as JSON lines on stdout,
e.g. {\"menu_action\":\"set_mode\",\"value\":\"wander\"}; every successful
save of config.toml prints {\"menu_action\":\"reload_config\"}.";

fn main() {
    let mut opts = Options {
        print_actions: true,
        ..Default::default()
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = |name: &str| {
            args.next()
                .unwrap_or_else(|| die(&format!("{name} needs a value")))
        };
        match a.as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                return;
            }
            "--tab" => opts.start_tab = Some(val("--tab")),
            "--step" => {
                opts.deep_link = Some(val("--step"));
                opts.start_tab.get_or_insert_with(|| "brain".into());
            }
            "--size" => {
                let v = val("--size");
                let (w, h) = v
                    .split_once(['x', 'X'])
                    .unwrap_or_else(|| die("--size WxH"));
                let p = |s: &str| {
                    s.trim()
                        .parse::<f32>()
                        .unwrap_or_else(|_| die("--size WxH"))
                };
                opts.inner_size = Some([p(w).max(640.0), p(h).max(480.0)]);
            }
            "--decorated" => opts.decorated = true,
            "--quiet" => opts.print_actions = false,
            "--manifest" => opts.manifest = Some(PathBuf::from(val("--manifest"))),
            "--screenshot" => opts.screenshot = Some(PathBuf::from(val("--screenshot"))),
            s if s.starts_with('-') => die(&format!("unknown option {s}\n\n{HELP}")),
            path => opts.config_path = Some(PathBuf::from(path)),
        }
    }
    if let Err(e) = run_with(opts, None) {
        die(&format!("{e}"));
    }
}

fn die(msg: &str) -> ! {
    eprintln!("lillith-settings: {msg}");
    std::process::exit(2)
}
