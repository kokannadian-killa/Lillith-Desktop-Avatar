//! E2E: `lillith setup --auto-gguf` on a clean profile (temp HOME, a
//! stand-in GGUF at `~/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf`)
//! writes a loopback config. A non-loopback `server_url` is then refused.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

const PREFERRED: &str = "dolphin3.0-llama3.2-3b-q4_k_m.gguf";

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lillith-setup-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_standin_gguf(path: &Path) {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut bytes = b"GGUF".to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    std::fs::write(path, &bytes).unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_len(16 * 1024 * 1024).unwrap();
}

fn setup_cmd(home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_lillith"));
    c.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("NO_COLOR", "1");
    c
}

#[test]
fn clean_profile_auto_gguf_writes_loopback_and_refuses_remote() {
    let home = tmp("home");
    let gguf = home.join("AI/models").join(PREFERRED);
    write_standin_gguf(&gguf);

    let out = setup_cmd(&home)
        .args(["setup", "--auto-gguf"])
        .output()
        .expect("run lillith setup");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "setup failed: {stdout}\n{stderr}");
    assert!(
        stdout.contains(gguf.to_str().unwrap()) && stdout.contains("127.0.0.1:8080"),
        "{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("preferred Dolphin 3B") || stdout.contains(&gguf.display().to_string()),
        "{stdout}"
    );

    let cfg = home.join(".config/lillith/config.toml");
    let text = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        text.contains(&format!("gguf_path = \"{}\"", gguf.display()))
            || text.contains(gguf.to_str().unwrap()),
        "{text}"
    );
    assert!(
        text.contains("server_url = \"http://127.0.0.1:8080\""),
        "{text}"
    );
    assert!(
        text.contains("endpoint = \"http://127.0.0.1:8080\""),
        "{text}"
    );
    assert!(text.contains("ctx_size = 4096"), "{text}");
    assert!(text.contains("--jinja"), "{text}");
    assert!(
        text.contains(&format!("model_path = \"{}\"", gguf.display()))
            || text.contains("model_path"),
        "{text}"
    );
    assert!(
        !text.contains("0.0.0.0")
            && !text.contains("192.168.")
            && !text.contains("[::]")
            && !text.to_ascii_lowercase().contains("openai.com"),
        "{text}"
    );

    // Hand-edit a remote endpoint into the saved file; the next setup
    // must refuse to load or write it (config is validated on read and write).
    let tainted = text.replace(
        "server_url = \"http://127.0.0.1:8080\"",
        "server_url = \"http://192.168.1.9:8080\"",
    );
    assert_ne!(tainted, text, "no server_url line to replace");
    std::fs::write(&cfg, &tainted).unwrap();

    let out = setup_cmd(&home)
        .args(["setup", "--gguf"])
        .arg(&gguf)
        .output()
        .expect("run lillith setup on tainted config");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "setup must refuse a remote server_url: {stdout}\n{stderr}"
    );
    let err = format!("{stdout}{stderr}");
    assert!(
        err.contains("192.168.1.9")
            || err.contains("server_url")
            || err.to_ascii_lowercase().contains("local")
            || err.contains("127.0.0.1"),
        "{err}"
    );
    // The tainted file is not rewritten into a working remote config.
    let after = std::fs::read_to_string(&cfg).unwrap();
    assert!(after.contains("192.168.1.9"), "{after}");

    let _ = std::fs::remove_dir_all(&home);
}
