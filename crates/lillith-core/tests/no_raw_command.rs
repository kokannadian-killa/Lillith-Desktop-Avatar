//! Lint: no raw `Command::new` in shipped source. Every child process must
//! go through `lillith_core::proc::command` (or `lillith_setup::proc`), which
//! sets `CREATE_NO_WINDOW` on Windows so no console window flashes up per
//! spawn (Ko's "cmd window on every video play" bug). Build scripts (crate-root
//! `build.rs`) and integration tests (`tests/` dirs) are exempt: they never ship.

use std::path::{Path, PathBuf};

const HELPERS: &[&str] = &[
    "crates/lillith-core/src/proc.rs",
    "crates/lillith-setup/src/proc.rs",
];

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if name == "tests"
                || name == "target"
                || name.starts_with('.')
                || name.starts_with("target-")
            {
                continue;
            }
            walk(&p, out);
        } else if name.ends_with(".rs") && !(name == "build.rs" && dir.join("Cargo.toml").is_file())
        {
            out.push(p);
        }
    }
}

#[test]
fn no_raw_command_new_outside_the_proc_helper() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    for top in ["crates", "apps", "tools", "android"] {
        walk(&root.join(top), &mut files);
    }
    assert!(files.len() > 50, "walked too few files: {}", files.len());
    let needle = ["Command", "::new("].concat();
    let mut bad = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(&root)
            .unwrap_or(f)
            .to_string_lossy()
            .replace('\\', "/");
        if HELPERS.contains(&rel.as_str()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            if line.contains(&needle) {
                bad.push(format!("{rel}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "raw Command::new (use lillith_core::proc::command so Windows gets CREATE_NO_WINDOW):\n{}",
        bad.join("\n")
    );
}

#[test]
fn gui_binaries_use_the_windows_subsystem() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for f in [
        "apps/lillith/src/main.rs",
        "crates/lillith-settings-gui/src/bin/lillith-settings.rs",
        "crates/lillith-admin/src/main.rs",
    ] {
        let text = std::fs::read_to_string(root.join(f)).unwrap();
        assert!(
            text.contains("#![cfg_attr(windows, windows_subsystem = \"windows\")]"),
            "{f} must be a GUI-subsystem exe on Windows"
        );
    }
}
