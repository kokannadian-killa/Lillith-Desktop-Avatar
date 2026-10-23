//! "Open settings" → the `lillith-settings` window (crate
//! `lillith-settings-gui`), run as a separate, detached process so the
//! overlay never links egui.
//!
//! The window prints every Companion pick on stdout as one JSON line
//! (`{"menu_action":"set_mode","value":"wander"}`, see
//! [`lillith_core::menu::action_line`] / [`lillith_core::menu::pick_line`];
//! names in [`lillith_core::menu::wire`]). A reader thread parses those
//! lines into [`MenuPick`]s (a [`MenuAction`], or a pose / avatar by name,
//! or a config reload) and only sends them over a channel; the
//! [`crate::run::Controller`] drains the channel on the overlay thread each
//! tick and applies them like a right-click pick (brain, voice, effects).
//! Nothing here touches overlay state or rendering.

use lillith_core::menu::{parse_pick, MenuAction, MenuPick};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// The settings window's binary (`[[bin]]` of `lillith-settings-gui`).
pub const SETTINGS_BIN: &str = "lillith-settings";

/// Environment variable the window (and `lillith_anim::atlas::default_manifest_path`)
/// reads to find the clip manifest; the overlay sets it on the child to the
/// manifest it loaded so the window's Poses list matches her clips.
pub const MANIFEST_ENV: &str = "LILLITH_MANIFEST";

/// Most window picks applied per overlay tick.
pub const MAX_PICKS_PER_TICK: usize = 16;

/// What [`SettingsWindow::open`] did.
#[derive(Debug)]
pub enum Launch {
    /// A new window is up.
    Spawned,
    /// One is already open; nothing spawned.
    AlreadyRunning,
    /// The binary could not be started (not installed / not executable).
    Failed(std::io::Error),
}

/// Tracks the (single) settings window and the picks it sends back.
#[derive(Debug)]
pub struct SettingsWindow {
    program: PathBuf,
    alive: Option<Arc<AtomicBool>>,
    tx: Sender<MenuPick>,
    rx: Receiver<MenuPick>,
}

impl Default for SettingsWindow {
    fn default() -> Self {
        Self::with_program(default_program())
    }
}

/// `lillith-settings` next to the running executable (release tarball,
/// `target/{debug,release}`), else whatever `$PATH` has.
pub fn default_program() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            Some(
                exe.parent()?
                    .join(format!("{SETTINGS_BIN}{}", std::env::consts::EXE_SUFFIX)),
            )
        })
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(SETTINGS_BIN))
}

impl SettingsWindow {
    pub fn with_program(program: impl Into<PathBuf>) -> Self {
        let (tx, rx) = channel();
        Self {
            program: program.into(),
            alive: None,
            tx,
            rx,
        }
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// `true` while a window we started is still running.
    pub fn running(&self) -> bool {
        self.alive
            .as_ref()
            .is_some_and(|a| a.load(Ordering::Acquire))
    }

    /// Starts `lillith-settings <config>` unless one is already open.
    /// `manifest` is the clip manifest the overlay loaded; when given, the
    /// child gets it as `$LILLITH_MANIFEST` (made absolute, since the
    /// window's lookup would otherwise fall back to `./assets/generated`
    /// or the XDG data dir and may find nothing). `None` leaves the
    /// child's environment alone.
    pub fn open(&mut self, config: &Path, manifest: Option<&Path>) -> Launch {
        if self.running() {
            return Launch::AlreadyRunning;
        }
        let mut cmd = Command::new(&self.program);
        cmd.arg(config)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(m) = manifest {
            let m = std::path::absolute(m).unwrap_or_else(|_| m.to_path_buf());
            cmd.env(MANIFEST_ENV, m);
        }
        #[cfg(unix)]
        {
            // Own process group: a Ctrl-C meant for the overlay's terminal
            // does not close the window, and it outlives the overlay.
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return Launch::Failed(e),
        };
        let Some(stdout) = child.stdout.take() else {
            return Launch::Failed(std::io::Error::other("no stdout pipe"));
        };
        let alive = Arc::new(AtomicBool::new(true));
        let flag = alive.clone();
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new()
            .name("lillith-settings".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    match parse_pick(line) {
                        // The window is already open; ignore re-entry.
                        Some(MenuPick::Action(MenuAction::OpenSettings)) => {}
                        Some(a) => {
                            if tx.send(a).is_err() {
                                break;
                            }
                        }
                        None => tracing::debug!(line, "settings window: ignored line"),
                    }
                }
                let status = child.wait();
                tracing::info!(?status, "settings window closed");
                flag.store(false, Ordering::Release);
            });
        if let Err(e) = spawned {
            return Launch::Failed(e);
        }
        self.alive = Some(alive);
        Launch::Spawned
    }

    /// Picks the window sent since the last call (at most
    /// [`MAX_PICKS_PER_TICK`]).
    pub fn drain(&self) -> Vec<MenuPick> {
        self.rx.try_iter().take(MAX_PICKS_PER_TICK).collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lillith_core::config::MoveMode;
    use lillith_core::menu::Size;
    use std::time::{Duration, Instant};

    /// A fake `lillith-settings`: prints a few lines, then waits for
    /// `release` to appear before exiting.
    pub(crate) fn fake_bin(tag: &str, lines: &[&str]) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("lillith-sw-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let release = dir.join("release");
        let mut sh = String::from("#!/bin/sh\n");
        sh.push_str(&format!("echo \"$1\" > '{}'\n", dir.join("arg").display()));
        sh.push_str(&format!(
            "printf '%s' \"${{{MANIFEST_ENV}:-<unset>}}\" > '{}'\n",
            dir.join("manifest_env").display()
        ));
        for l in lines {
            sh.push_str(&format!("printf '%s\\n' '{l}'\n"));
        }
        sh.push_str(&format!(
            "while [ ! -e '{}' ]; do sleep 0.05; done\n",
            release.display()
        ));
        let bin = dir.join(SETTINGS_BIN);
        std::fs::write(&bin, sh).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, bin)
    }

    pub(crate) fn wait_for(mut f: impl FnMut() -> bool) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn picks_come_back_and_only_one_window_runs() {
        let (dir, bin) = fake_bin(
            "picks",
            &[
                r#"{"menu_action":"set_mode","value":"wander"}"#,
                "some log noise",
                r#"{"menu_action":"open_settings"}"#,
                r#"{"menu_action":"set_size","value":"large"}"#,
                r#"{"menu_action":"toggle_mute"}"#,
            ],
        );
        let mut w = SettingsWindow::with_program(&bin);
        let cfg = dir.join("config.toml");
        let manifest = dir.join("gen/manifest.toml");
        assert!(matches!(w.open(&cfg, Some(&manifest)), Launch::Spawned));
        assert!(w.running());
        assert!(matches!(
            w.open(&cfg, Some(&manifest)),
            Launch::AlreadyRunning
        ));
        let mut got = Vec::new();
        assert!(wait_for(|| {
            got.extend(w.drain());
            got.len() >= 3
        }));
        assert_eq!(
            got,
            [
                MenuPick::Action(MenuAction::SetMode(MoveMode::Wander)),
                MenuPick::Action(MenuAction::SetSize(Size::Large)),
                MenuPick::Action(MenuAction::ToggleMute),
            ]
        );
        let arg = std::fs::read_to_string(dir.join("arg")).unwrap();
        assert_eq!(arg.trim(), cfg.display().to_string());
        let env = std::fs::read_to_string(dir.join("manifest_env")).unwrap();
        assert_eq!(
            env,
            manifest.display().to_string(),
            "child sees the manifest"
        );
        std::fs::write(dir.join("release"), "").unwrap();
        assert!(wait_for(|| !w.running()), "exit is noticed");
        assert!(matches!(w.open(&cfg, None), Launch::Spawned), "can reopen");
        assert!(wait_for(|| !w.running()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_binary_fails_cleanly() {
        let mut w = SettingsWindow::with_program("/nonexistent/lillith-settings");
        match w.open(Path::new("/tmp/x.toml"), None) {
            Launch::Failed(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
            other => panic!("{other:?}"),
        }
        assert!(!w.running());
        assert!(w.drain().is_empty());
    }
}
