//! Find files by name under a folder (read-only, bounded).

use std::path::{Path, PathBuf};

/// Kernel / device trees never walked.
const SKIP_ROOTS: &[&str] = &["/proc", "/sys", "/dev", "/run/user"];
/// Folder names skipped while walking (huge, rarely what Ko means).
const SKIP_NAMES: &[&str] = &["node_modules", "target", ".git", ".cache", "__pycache__"];

/// `*` / `?` wildcard match (case-insensitive). A pattern with no wildcard
/// matches as a substring.
pub fn name_matches(name: &str, pattern: &str) -> bool {
    let n = name.to_lowercase();
    let p = pattern.to_lowercase();
    if !p.contains('*') && !p.contains('?') {
        return n.contains(&p);
    }
    glob(n.as_bytes(), p.as_bytes())
}

fn glob(n: &[u8], p: &[u8]) -> bool {
    let (mut i, mut j) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while i < n.len() {
        if j < p.len() && (p[j] == b'?' || p[j] == n[i]) {
            i += 1;
            j += 1;
        } else if j < p.len() && p[j] == b'*' {
            star = Some(j);
            mark = i;
            j += 1;
        } else if let Some(s) = star {
            j = s + 1;
            mark += 1;
            i = mark;
        } else {
            return false;
        }
    }
    while j < p.len() && p[j] == b'*' {
        j += 1;
    }
    j == p.len()
}

/// Paths under `root` whose name matches `pattern`, at most `max` results,
/// visiting at most `budget` entries. Symlinks are not followed; hidden
/// folders are skipped unless the pattern starts with `.`.
pub fn find_by_name(root: &Path, pattern: &str, max: usize, budget: usize) -> (Vec<PathBuf>, bool) {
    let mut out = Vec::new();
    let mut seen = 0usize;
    let mut stack = vec![root.to_path_buf()];
    let hidden_ok = pattern.starts_with('.');
    while let Some(dir) = stack.pop() {
        if SKIP_ROOTS.iter().any(|s| dir.starts_with(s)) {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut kids: Vec<_> = rd.flatten().collect();
        kids.sort_by_key(|e| e.file_name());
        for e in kids {
            seen += 1;
            if seen > budget {
                return (out, true);
            }
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if name_matches(&name, pattern) {
                out.push(e.path());
                if out.len() >= max {
                    return (out, true);
                }
            }
            if ft.is_dir()
                && !ft.is_symlink()
                && !SKIP_NAMES.contains(&name.as_str())
                && (hidden_ok || !name.starts_with('.'))
            {
                stack.push(e.path());
            }
        }
    }
    (out, false)
}

/// [`find_by_name`] as text for chat.
pub fn find_report(root: &Path, pattern: &str) -> String {
    let (hits, more) = find_by_name(root, pattern, 200, 300_000);
    if hits.is_empty() {
        return format!("nothing named like {pattern:?} under {}", root.display());
    }
    let mut s = hits
        .iter()
        .map(|p| {
            if p.is_dir() {
                format!("{}/", p.display())
            } else {
                p.display().to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if more {
        s.push_str("\n…[more results; narrow the folder or pattern]");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::super::testutil::tmp;
    use super::*;

    #[test]
    fn globs_and_substrings() {
        assert!(name_matches("Holiday.JPG", "*.jpg"));
        assert!(name_matches("report-2026.pdf", "report"));
        assert!(!name_matches("a.png", "*.jpg"));
        assert!(name_matches("abc", "a?c"));
    }

    #[test]
    fn finds_nested_files_skipping_hidden() {
        let d = tmp("find");
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::create_dir_all(d.join(".hid")).unwrap();
        std::fs::write(d.join("a/b/tax.pdf"), "").unwrap();
        std::fs::write(d.join(".hid/tax2.pdf"), "").unwrap();
        let (h, more) = find_by_name(&d, "*.pdf", 10, 1000);
        assert_eq!(h, vec![d.join("a/b/tax.pdf")]);
        assert!(!more);
    }
}
