//! A tiny, dependency-free writer for Lillith's audit log, for the
//! launches this crate's tools run themselves (the setup wizard's
//! `ollama create`). `lillith-setup` does not depend on `lillith-core`, so
//! this writes the identical line format as `lillith-core`'s
//! `SoulContract::record_ungated_launch_as`:
//!
//! `<ts>\t<TAG>\t<kind>\t<argv[0]>\tno-card\t$ <argv, --api-key values masked>`
//!
//! (`<ts>` is UTC `YYYY-MM-DDTHH:MM:SSZ`; tabs and newlines inside a column
//! become spaces). Append-only: existing lines are never rewritten.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Tag of an `ollama create` run without a card (same as `lillith-core`'s
/// `OLLAMA_CREATE_UNGATED_TAG`).
pub const OLLAMA_CREATE_UNGATED_TAG: &str = "OLLAMA_CREATE_UNGATED";
/// Kind column of an `ollama create` line (`lillith-core`'s
/// `OLLAMA_CREATE_KIND`).
pub const OLLAMA_CREATE_KIND: &str = "ollama_create";
/// The id column of a card-less line (`lillith-core`'s `NO_CARD`).
pub const NO_CARD: &str = "no-card";

/// `<data>/lillith/audit.log`, the same rule as `lillith-core`'s
/// `config::audit_log_path`: `$XDG_DATA_HOME`, on Windows else
/// `%LOCALAPPDATA%`, else `~/.local/share`.
pub fn audit_log_path() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    let base = var("XDG_DATA_HOME").map(PathBuf::from);
    let base = if cfg!(windows) {
        base.or_else(|| var("LOCALAPPDATA").map(PathBuf::from))
    } else {
        base
    };
    let base = base.or_else(|| var("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("lillith").join("audit.log"))
}

/// `argv` with every `--api-key…` value masked (`lillith-core`'s
/// `mask_secret_args`).
pub fn mask_secret_args(argv: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(argv.len());
    let mut mask = false;
    for a in argv {
        if mask {
            out.push("***".to_string());
            mask = false;
            continue;
        }
        match a.split_once('=') {
            Some((flag, _)) if flag.starts_with("--api-key") => out.push(format!("{flag}=***")),
            _ => {
                mask = a.starts_with("--api-key");
                out.push(a.clone());
            }
        }
    }
    out
}

/// The line for a card-less launch at `now`, ending in one `\n`.
pub fn ungated_launch_line(now: SystemTime, tag: &str, kind: &str, argv: &[String]) -> String {
    let col = |s: &str| s.replace(['\n', '\r', '\t'], " ");
    let bin = argv.first().cloned().unwrap_or_default();
    let shown = format!("$ {}", mask_secret_args(argv).join(" "));
    format!(
        "{}\t{tag}\t{kind}\t{}\t{NO_CARD}\t{}\n",
        format_utc(now),
        col(&bin),
        col(&shown)
    )
}

/// Appends one card-less launch line to `log` (creating it and its folder).
/// Errors are returned, never panicked on.
pub fn record_ungated_launch(
    log: &Path,
    tag: &str,
    kind: &str,
    argv: &[String],
) -> std::io::Result<()> {
    append(
        log,
        &ungated_launch_line(SystemTime::now(), tag, kind, argv),
    )
}

/// Appends `line`; when the file ends mid-line a newline goes first.
fn append(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)?;
    let mut buf = String::with_capacity(line.len() + 1);
    if f.seek(SeekFrom::End(0))? > 0 {
        let mut last = [0u8; 1];
        f.seek(SeekFrom::End(-1))?;
        f.read_exact(&mut last)?;
        if last[0] != b'\n' {
            buf.push('\n');
        }
    }
    buf.push_str(line);
    f.write_all(buf.as_bytes())
}

/// UTC `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_utc(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_line_matches_the_core_format() {
        let t = UNIX_EPOCH + Duration::from_secs(1_761_300_000);
        let argv: Vec<String> = [
            "/usr/bin/ollama",
            "create",
            "lily",
            "-f",
            "/m/Modelfile.lily",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            ungated_launch_line(t, OLLAMA_CREATE_UNGATED_TAG, OLLAMA_CREATE_KIND, &argv),
            "2025-10-24T10:00:00Z\tOLLAMA_CREATE_UNGATED\tollama_create\t/usr/bin/ollama\tno-card\t$ /usr/bin/ollama create lily -f /m/Modelfile.lily\n"
        );
        assert_eq!(format_utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let m = mask_secret_args(&["x", "--api-key", "s", "--api-key=t"].map(String::from));
        assert_eq!(m, ["x", "--api-key", "***", "--api-key=***"]);
        let dir = std::env::temp_dir().join(format!("lillith-setup-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = dir.join("a").join("audit.log");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, "partial").unwrap();
        record_ungated_launch(&log, OLLAMA_CREATE_UNGATED_TAG, OLLAMA_CREATE_KIND, &argv).unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "never glued onto a partial line");
        assert!(lines[1].contains("\tOLLAMA_CREATE_UNGATED\tollama_create\t"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
