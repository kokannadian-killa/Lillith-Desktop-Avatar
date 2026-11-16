//! `lillith chat` must close a stale model-server card a dead process left
//! `PROPOSED`, and must leave a card whose process is still running (a live
//! overlay) alone. The scan used to run only when the overlay started.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lillith-chat-orphan-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn dead_pid() -> u32 {
    (2_000_000..2_000_200)
        .find(|p| !std::path::Path::new("/proc").join(p.to_string()).exists())
        .expect("a free pid")
}

#[test]
fn lillith_chat_closes_orphaned_cards_and_keeps_live_ones() {
    let home = tmp("home");
    let data = home.join(".local/share");
    let log_dir = data.join("lillith");
    std::fs::create_dir_all(&log_dir).unwrap();
    let log = log_dir.join("audit.log");

    let live_id = format!("1-{:x}-0", 1u32); // pid 1 is always running
    let dead_id = format!("1-{:x}-0", dead_pid());
    std::fs::write(
        &log,
        format!(
            "2023-01-01T00:00:00Z\tPROPOSED\tstart_model_server\t/opt/llama-server\tcard={live_id}\n\
             2023-01-01T00:00:01Z\tPROPOSED\tstart_model_server\t/opt/llama-server\tcard={dead_id}\n"
        ),
    )
    .unwrap();

    // A closed loopback port: one-shot chat fails fast and still runs the scan.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let cfg = home.join("config.toml");
    std::fs::write(
        &cfg,
        format!("[chat]\nenabled = true\nendpoint = \"http://127.0.0.1:{port}\"\ntimeout_s = 1\n"),
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_lillith"))
        .arg("--config")
        .arg(&cfg)
        .args(["chat", "--no-history", "hi"])
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", &data)
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("NO_COLOR", "1")
        .output()
        .expect("run lillith chat");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    // May fail the completion (nothing is listening); the audit log is the check.
    let _ = (stdout, stderr, out.status);
    let audit = std::fs::read_to_string(&log).expect("audit log");
    assert!(
        audit.contains(&format!(
            "\tCANCELLED\tstart_model_server\t/opt/llama-server\tcard={dead_id}\torphaned PROPOSED closed by lillith chat"
        )),
        "dead-pid card must be cancelled:\n{audit}"
    );
    assert!(
        !audit.contains(&format!("card={live_id}\torphaned")),
        "live-pid card must stay open:\n{audit}"
    );
    assert!(
        audit.contains(&format!(
            "PROPOSED\tstart_model_server\t/opt/llama-server\tcard={live_id}"
        )),
        "live PROPOSED line must be kept:\n{audit}"
    );

    let _ = std::fs::remove_dir_all(&home);
}
