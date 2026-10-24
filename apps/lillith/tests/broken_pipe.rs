//! Release QA 46ed853 #9: `lillith rules | head -1` must not panic (exit
//! 101) when the reader closes the pipe; it ends quietly with exit 0.

#[cfg(unix)]
#[test]
fn closed_stdout_pipe_exits_quietly() {
    let bin = env!("CARGO_BIN_EXE_lillith");
    // The reader (`true`) is gone before lillith starts (the left side waits
    // for it), so every write hits EPIPE. lillith's exit code and stderr go
    // to fd 3 / the captured stderr.
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg(r#"exec 3>&1; { sleep 0.3; "$0" rules 2>&4; echo "exit=$?" >&3; } 4>&2 | true"#)
        .arg(bin)
        .env("RUST_LOG", "off")
        .output()
        .expect("run sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("exit=0"),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(!stderr.contains("panicked"), "stderr: {stderr}");
}
