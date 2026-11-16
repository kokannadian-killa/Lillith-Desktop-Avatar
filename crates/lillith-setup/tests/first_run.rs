//! Clean-profile first-run: a stand-in GGUF in `~/AI/models` is picked,
//! the written config is loopback-only, and a remote `server_url` is refused.

use lillith_setup::config::save;
use lillith_setup::launch::{self, LOOPBACK_HOST};
use lillith_setup::local::LocalUrl;
use lillith_setup::pick::{self, Reason};
use lillith_setup::scan::{self, PREFERRED_3B_FILE};
use lillith_setup::wizard;
use std::fs;
use std::path::{Path, PathBuf};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "lillith-setup-first-run-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// Sparse stand-in GGUF (header only). `pick` and `scan` ignore files
/// under 16 MB, so the file is that large on disk as a hole.
fn write_standin_gguf(path: &Path) {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d).unwrap();
    }
    // GGUF v3, zero tensors, zero KV — enough for the filename pickers.
    let mut bytes = b"GGUF".to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    fs::write(path, &bytes).unwrap();
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_len(16 * 1024 * 1024).unwrap();
}

#[test]
fn clean_profile_first_run_writes_loopback_and_refuses_remote() {
    let root = tmp("clean");
    let gguf = root.join("AI/models").join(PREFERRED_3B_FILE);
    write_standin_gguf(&gguf);

    let found = pick::find_ggufs(&[root.join("AI/models")], 2);
    assert_eq!(found.len(), 1, "{found:?}");
    let pick = pick::pick_default(&found, Some(&gguf), Some(8 * 1024 * 1024 * 1024)).unwrap();
    assert_eq!(pick.reason, Reason::Preferred);
    assert_eq!(pick.path, gguf);

    let info = scan::ModelInfo::from_path(&gguf, scan::ModelSource::File).unwrap();
    let mut cfg = wizard::config_for_model(&info, lillith_setup::config::Backend::LlamaServer);
    assert_eq!(cfg.gguf_path, gguf.display().to_string());
    assert_eq!(cfg.server_url, "http://127.0.0.1:8080");
    let url = LocalUrl::parse(&cfg.server_url, 8080).unwrap();
    assert_eq!(url.ip, std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    assert_eq!(url.port, 8080);

    let path = root.join("config.toml");
    save(&path, &cfg, true).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("127.0.0.1"), "{text}");
    assert!(text.contains(&gguf.display().to_string()), "{text}");
    assert!(text.contains("8080") && text.contains("--jinja"), "{text}");
    assert!(
        !text.contains("0.0.0.0") && !text.contains("[::]"),
        "{text}"
    );

    let cmd = launch::llama_server_command_with(&cfg, None, 2).unwrap();
    let args = cmd.args.iter().map(String::as_str).collect::<Vec<_>>();
    assert_eq!(cmd_arg(&args, "--host"), Some(LOOPBACK_HOST));
    assert_eq!(cmd_arg(&args, "--port"), Some("8080"));
    assert_eq!(cmd_arg(&args, "-c"), Some("4096"));
    assert_eq!(cmd_arg(&args, "-t"), Some("2"));
    assert!(args.contains(&"--jinja"), "{args:?}");
    assert_eq!(cmd_arg(&args, "-m"), Some(gguf.to_str().unwrap()));

    // A remote endpoint must not be written (or launched).
    cfg.server_url = "http://192.168.1.9:8080".into();
    let err = save(&path, &cfg, true).unwrap_err().to_string();
    assert!(
        err.contains("server_url")
            && (err.contains("local") || err.contains("loopback") || err.contains("127.0.0.1")),
        "{err}"
    );
    let still = fs::read_to_string(&path).unwrap();
    assert!(!still.contains("192.168.1.9"), "{still}");
    assert!(
        launch::llama_server_command_with(&cfg, None, 2).is_err(),
        "launch must refuse a remote server_url"
    );

    let _ = fs::remove_dir_all(&root);
}

fn cmd_arg<'a>(args: &'a [&str], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| *a == flag)
        .and_then(|i| args.get(i + 1))
        .copied()
}
