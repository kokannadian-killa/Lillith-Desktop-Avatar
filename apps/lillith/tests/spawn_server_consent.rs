//! `lillith chat --spawn-server` with no display: a `y` (or `yes`) on stdin
//! must never start llama-server. The fake server binary would leave a
//! marker file if anything ran it; the command must refuse, say how to
//! start the server by hand, and audit `NO_UI` (never a `CONFIRM*`). A
//! refused `extra_args` entry is rejected before any card is filed.

#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Run {
    spawned: bool,
    stdout: String,
    stderr: String,
    audit: String,
    fake: String,
}

/// `lillith chat --spawn-server` with no display, `y` on stdin, and
/// `extra` appended to `[chat.server]`.
fn run(name: &str, extra: &str) -> Run {
    let bin = env!("CARGO_BIN_EXE_lillith");
    let dir = std::env::temp_dir().join(format!(
        "lillith-spawn-consent-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("home")).unwrap();
    let marker = dir.join("SPAWNED");
    let fake = dir.join("llama-server");
    std::fs::write(
        &fake,
        format!("#!/bin/sh\ntouch '{}'\nsleep 5\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let model = dir.join("m.gguf");
    std::fs::write(&model, b"GGUF").unwrap();
    // A loopback port nothing listens on.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let cfg = dir.join("config.toml");
    std::fs::write(
        &cfg,
        format!(
            "[chat]\nendpoint = \"http://127.0.0.1:{port}\"\n\n[chat.server]\nbinary = \"{}\"\nmodel_path = \"{}\"\n{extra}",
            fake.display(),
            model.display()
        ),
    )
    .unwrap();
    let mut child = Command::new(bin)
        .arg("--config")
        .arg(&cfg)
        .args(["chat", "--spawn-server", "--no-history", "hi"])
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("home/.config"))
        .env("XDG_DATA_HOME", dir.join("home/.local/share"))
        .env("XDG_STATE_HOME", dir.join("home/.local/state"))
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run lillith");
    // Typed yes, several ways, before and after the question would be.
    let _ = child.stdin.take().unwrap().write_all(b"y\nyes\nY\n");
    let t0 = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if t0.elapsed() > Duration::from_secs(90) {
            let _ = child.kill();
            panic!("lillith chat hung");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    // Give a (wrongly) spawned fake a moment to leave its marker.
    std::thread::sleep(Duration::from_millis(300));
    let spawned = marker.exists();
    let audit = std::fs::read_to_string(dir.join("home/.local/share/lillith/audit.log"))
        .unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    Run {
        spawned,
        stdout,
        stderr,
        audit,
        fake: fake.display().to_string(),
    }
}

fn decisions(audit: &str) -> Vec<&str> {
    audit
        .lines()
        .filter(|l| l.contains("\tstart_model_server\t"))
        .filter_map(|l| l.split('\t').nth(1))
        .collect()
}

#[test]
fn typed_y_on_stdin_never_spawns_the_server() {
    let Run {
        spawned,
        stdout,
        stderr,
        audit,
        fake,
    } = run("y", "");
    assert!(!spawned, "a typed y started the server\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("typed yes never counts"),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("To start the local model server yourself"),
        "{stdout}"
    );
    assert!(stdout.contains(&fake), "{stdout}");
    // `--spawn-server` asks first; the failed chat turn's own recovery may
    // ask again (also NO_UI here). Never anything but PROPOSED / NO_UI.
    let d = decisions(&audit);
    assert_eq!(d[..2], ["PROPOSED", "NO_UI"], "{audit}");
    assert!(d.chunks(2).all(|c| c == ["PROPOSED", "NO_UI"]), "{audit}");
}

#[test]
fn a_refused_extra_arg_is_rejected_before_any_card() {
    let r = run(
        "refused",
        "extra_args = [\"--jinja\", \"--mmproj\", \"/p.gguf\"]\n",
    );
    assert!(!r.spawned, "{}\n{}", r.stdout, r.stderr);
    // Refused up front by `--spawn-server` (and again by the failed turn's
    // recovery): never a card.
    let d = decisions(&r.audit);
    assert!(
        !d.is_empty() && d.iter().all(|t| *t == "ARG_REFUSED"),
        "no card filed: {}",
        r.audit
    );
    assert!(r.audit.contains("--mmproj /p.gguf"), "{}", r.audit);
    assert!(
        r.stdout.contains("no Soul Contract card shown"),
        "{}",
        r.stdout
    );
    assert!(!r.stdout.contains("typed yes never counts"), "{}", r.stdout);
}
