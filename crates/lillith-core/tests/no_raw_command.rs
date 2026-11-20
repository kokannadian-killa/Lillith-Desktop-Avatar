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
    // Matched with all whitespace removed (comment lines dropped), so a
    // call split across lines (`Command\n    ::new(`) is caught too.
    let needles = [
        ["Command", "::new("].concat(),
        ["creation_flags", "("].concat(),
        ["Command", "as"].concat(), // `use std::process::Command as X;`
    ];
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
        // (squashed text, line number of each squashed byte)
        let mut squashed = String::new();
        let mut line_of = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for ch in line.chars().filter(|c| !c.is_whitespace()) {
                squashed.push(ch);
                line_of.extend(std::iter::repeat(i + 1).take(ch.len_utf8()));
            }
        }
        for n in &needles {
            for (at, _) in squashed.match_indices(n.as_str()) {
                if n.ends_with("as") && !is_import_alias(&squashed, at + n.len()) {
                    continue;
                }
                let ln = line_of[at];
                let src = text.lines().nth(ln - 1).unwrap_or("").trim();
                bad.push(format!("{rel}:{ln}: {src}"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "raw Command::new / creation_flags / Command alias outside proc.rs (use lillith_core::proc::{{command, hide, hide_with}} so Windows keeps CREATE_NO_WINDOW):\n{}",
        bad.join("\n")
    );
}

/// `Command as X` only counts when it is an import alias (`as X;`, `as X,`
/// or `as X}`), not e.g. `Command as_str`.
fn is_import_alias(squashed: &str, after: usize) -> bool {
    let rest = &squashed[after..];
    let ident: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    !ident.is_empty() && matches!(rest[ident.len()..].chars().next(), Some(';' | ',' | '}'))
}

#[test]
fn lint_catches_split_calls_and_aliases() {
    let s: String = "let c = std::process::Command\n    ::new(x);"
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(s.contains(&["Command", "::new("].concat()));
    let a: String = "use std::process::Command as Cmd;"
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let n = ["Command", "as"].concat();
    let at = a.find(&n).unwrap();
    assert!(is_import_alias(&a, at + n.len()));
    let b = "x.Commandas_str()";
    assert!(!is_import_alias(b, b.find(&n).unwrap() + n.len()));
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
