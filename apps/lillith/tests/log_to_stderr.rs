//! Logs must go to stderr, never stdout: `chat-bubble` and `settings` share
//! this binary's logger and use stdout as their JSON event pipe. A bad
//! `--config` makes the headless `status` command log a warning.

#[test]
fn tracing_logs_go_to_stderr_not_stdout() {
    let bin = env!("CARGO_BIN_EXE_lillith");
    let dir = std::env::temp_dir().join(format!("lillith-logstderr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("bad.toml");
    // Invalid TOML: load_config logs a warning and falls back to defaults.
    std::fs::write(&cfg, "this is = = not toml [[[").unwrap();
    let out = std::process::Command::new(bin)
        .arg("--config")
        .arg(&cfg)
        .arg("status")
        .env("RUST_LOG", "debug")
        .env("NO_COLOR", "1")
        .output()
        .expect("run lillith");
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}");
    for level in [" WARN ", " INFO ", " DEBUG ", " TRACE "] {
        assert!(!stdout.contains(level), "log line on stdout: {stdout}");
    }
    assert!(
        stderr.contains("using defaults"),
        "config warning missing from stderr: {stderr}"
    );
}
