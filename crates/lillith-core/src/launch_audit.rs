//! Audit lines for program launches that are not (or not only) decided by
//! a Soul Contract card, and checks for env/config-chosen programs.
//!
//! Every launch site from `reviews/legacy-launches.md` that stays
//! automatic, internal or user-clicked writes one line to Lillith's one
//! audit log ([`crate::config::audit_log_path`], append mode):
//!
//! ```text
//! <rfc3339>\t<TAG>\t<kind>\t<program>\t<card=<id>|no-card>\t$ <masked argv>
//! ```
//!
//! the same column layout as the Soul Contract's own card lines (tag,
//! kind, path, card column, note). Secret-looking values (`--api-key x`,
//! `--token=x`, …) are masked and tabs / newlines are folded to spaces so
//! a configured path or argument can never forge a column or a line.
//!
//! High-frequency launches (the standby ffmpeg decoder, a TTS engine run
//! for every spoken line) use [`record_once`]: one line per process for
//! each `(kind, program)`, so the log records *that* and *what* ran
//! without growing by a line per animation clip or sentence.
//!
//! [`checked_exe`] is the rule for programs chosen by an environment
//! variable or a config value: an absolute path to an existing regular,
//! executable file, or it is refused (with a log line) and the caller
//! falls back to its default or skips the step.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// Tag of a card-less launch record (automatic, internal, user-clicked).
pub const LAUNCH_TAG: &str = "LAUNCH";
/// The card column of a launch no card decided.
pub const NO_CARD: &str = "no-card";

/// Folds tabs, carriage returns and newlines to spaces (one column, one line).
pub fn one_column(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

/// Whether a flag names a secret (`--api-key`, `--token`, `--password`, …).
fn secret_flag(flag: &str) -> bool {
    let f = flag.trim_start_matches('-').to_ascii_lowercase();
    flag.starts_with('-')
        && [
            "api-key", "api_key", "apikey", "token", "password", "passwd", "secret",
        ]
        .iter()
        .any(|s| f.starts_with(s) || f.ends_with(s))
}

/// `argv` with secret values masked: the argument after a secret flag and
/// the value of `--flag=value`.
pub fn mask_argv(argv: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(argv.len());
    let mut mask_next = false;
    for a in argv {
        if mask_next {
            out.push("***".to_string());
            mask_next = false;
            continue;
        }
        match a.split_once('=') {
            Some((flag, _)) if secret_flag(flag) => out.push(format!("{flag}=***")),
            _ => {
                mask_next = secret_flag(a);
                out.push(a.clone());
            }
        }
    }
    out
}

/// Lossy strings of a program and its arguments.
pub fn argv_of<I, S>(program: impl AsRef<OsStr>, args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    std::iter::once(program.as_ref().to_string_lossy().into_owned())
        .chain(
            args.into_iter()
                .map(|a| a.as_ref().to_string_lossy().into_owned()),
        )
        .collect()
}

/// The program and arguments of a built [`std::process::Command`].
pub fn argv_of_command(c: &std::process::Command) -> Vec<String> {
    argv_of(c.get_program(), c.get_args())
}

/// One audit line (with its trailing newline), see the module docs.
pub fn format_line(ts: &str, tag: &str, kind: &str, argv: &[String], card: Option<&str>) -> String {
    let program = one_column(argv.first().map(String::as_str).unwrap_or(""));
    let card = card.map_or_else(
        || NO_CARD.to_string(),
        |c| format!("card={}", one_column(c)),
    );
    let shown = one_column(&format!("$ {}", mask_argv(argv).join(" ")));
    format!(
        "{}\t{}\t{}\t{program}\t{card}\t{shown}\n",
        one_column(ts),
        one_column(tag),
        one_column(kind)
    )
}

/// Appends `line` to `log` (append mode; creates the file and its folder).
pub fn append_line(log: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?;
    f.write_all(line.as_bytes())
}

/// Writes one line to `log` now. A failure is logged, never fatal.
pub fn record_to(log: &Path, tag: &str, kind: &str, argv: &[String], card: Option<&str>) {
    let ts = crate::soul_contract::format_utc(SystemTime::now());
    if let Err(e) = append_line(log, &format_line(&ts, tag, kind, argv, card)) {
        tracing::warn!("could not write audit log {}: {e}", log.display());
    }
}

/// One `LAUNCH` line for a card-less launch in Lillith's audit log.
pub fn record(kind: &str, argv: &[String]) {
    if let Some(log) = crate::config::audit_log_path() {
        record_to(&log, LAUNCH_TAG, kind, argv, None);
    }
}

/// [`record`] for a built command, just before it is spawned.
pub fn record_command(kind: &str, c: &std::process::Command) {
    record(kind, &argv_of_command(c));
}

/// [`record`] once per process for each `(kind, program)`.
pub fn record_once(kind: &str, argv: &[String]) {
    if first_time(kind, argv) {
        record(kind, argv);
    }
}

/// [`record_once`] for a built command.
pub fn record_command_once(kind: &str, c: &std::process::Command) {
    record_once(kind, &argv_of_command(c));
}

/// `true` the first time `(kind, argv[0])` is seen in this process.
fn first_time(kind: &str, argv: &[String]) -> bool {
    static SEEN: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
    let key = (kind.to_string(), argv.first().cloned().unwrap_or_default());
    SEEN.get_or_init(Default::default)
        .lock()
        .map(|mut s| s.insert(key))
        .unwrap_or(true)
}

/// Why a program path was refused by [`checked_exe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExeError {
    NotAbsolute,
    Missing,
    NotRegularFile,
    NotExecutable,
}

impl std::fmt::Display for ExeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ExeError::NotAbsolute => "not an absolute path",
            ExeError::Missing => "does not exist",
            ExeError::NotRegularFile => "is not a regular file",
            ExeError::NotExecutable => "is not executable",
        })
    }
}

/// `p` if it is an absolute path to an existing regular, executable file
/// (Unix: an execute bit; Windows: `.exe` / `.com`). Symlinks are followed.
pub fn checked_exe(p: &Path) -> Result<PathBuf, ExeError> {
    if !p.is_absolute() {
        return Err(ExeError::NotAbsolute);
    }
    let m = std::fs::metadata(p).map_err(|_| ExeError::Missing)?;
    if !m.is_file() {
        return Err(ExeError::NotRegularFile);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if m.permissions().mode() & 0o111 == 0 {
            return Err(ExeError::NotExecutable);
        }
    }
    #[cfg(not(unix))]
    {
        let ok = p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("com"));
        if !ok {
            return Err(ExeError::NotExecutable);
        }
    }
    Ok(p.to_path_buf())
}

/// The program `$var` names, when it passes [`checked_exe`]. An empty or
/// unset variable is `None` quietly; a refused one is `None` with a log
/// line naming the variable, the value and the reason.
pub fn env_exe(var: &str) -> Option<PathBuf> {
    let v = std::env::var_os(var).filter(|v| !v.is_empty())?;
    checked_or_log(var, Path::new(&v))
}

/// [`checked_exe`] with a warning naming `what` (an env var or config key)
/// when `p` is refused.
pub fn checked_or_log(what: &str, p: &Path) -> Option<PathBuf> {
    match checked_exe(p) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(
                "{what} = {:?} refused: {e} (needs an absolute path to an existing executable \
                 file); using the default instead",
                p.display().to_string()
            );
            None
        }
    }
}

/// `name` on `$PATH` as an absolute, checked executable (relative `PATH`
/// entries are skipped). `name` gets `EXE_SUFFIX` on Windows.
pub fn which_abs(name: &str) -> Option<PathBuf> {
    which_in(name, &std::env::var_os("PATH")?)
}

/// [`which_abs`] over an explicit `PATH` value (testable).
pub fn which_in(name: &str, path: &OsStr) -> Option<PathBuf> {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(path)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(&file))
        .find_map(|p| checked_exe(&p).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn line_has_the_card_layout_and_masks_secrets() {
        let l = format_line(
            "2026-10-24T10:00:00Z",
            LAUNCH_TAG,
            "kwin_dbus",
            &s(&["/usr/bin/x", "--api-key", "hunter2", "--token=abc", "-v"]),
            None,
        );
        assert_eq!(
            l,
            "2026-10-24T10:00:00Z\tLAUNCH\tkwin_dbus\t/usr/bin/x\tno-card\t\
             $ /usr/bin/x --api-key *** --token=*** -v\n"
        );
        assert!(!l.contains("hunter2") && !l.contains("abc"));
        let c = format_line("t", "CONFIRM", "kwin_fix", &s(&["a"]), Some("1-2-3"));
        assert_eq!(c, "t\tCONFIRM\tkwin_fix\ta\tcard=1-2-3\t$ a\n");
    }

    #[test]
    fn tabs_and_newlines_never_forge_columns_or_lines() {
        let l = format_line(
            "t",
            "LAUNCH",
            "k\tx",
            &s(&["/a\tb", "x\ny\tz", "--password\n", "p\tw"]),
            Some("c\nd"),
        );
        assert_eq!(l.matches('\n').count(), 1, "{l:?}");
        assert!(l.ends_with('\n'));
        assert_eq!(l.trim_end().split('\t').count(), 6, "{l:?}");
        assert!(!l.contains("p w") && !l.contains("p\tw"), "{l:?}");
    }

    #[test]
    fn masking_covers_common_secret_flags_only() {
        assert_eq!(
            mask_argv(&s(&[
                "x",
                "--password",
                "pw",
                "--secret-file=/s",
                "--keep",
                "k"
            ])),
            s(&["x", "--password", "***", "--secret-file=***", "--keep", "k"])
        );
        // A plain value that happens to say "token" is not a flag.
        assert_eq!(mask_argv(&s(&["token", "x"])), s(&["token", "x"]));
    }

    #[test]
    fn record_to_appends_one_line_per_call() {
        let d = std::env::temp_dir().join(format!("lillith-launch-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let log = d.join("sub/audit.log");
        record_to(&log, LAUNCH_TAG, "a", &s(&["/bin/a"]), None);
        record_to(&log, LAUNCH_TAG, "b", &s(&["/bin/b", "x"]), Some("id"));
        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with("\tLAUNCH\ta\t/bin/a\tno-card\t$ /bin/a"));
        assert!(lines[1].ends_with("\tLAUNCH\tb\t/bin/b\tcard=id\t$ /bin/b x"));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn once_per_kind_and_program() {
        let a = s(&["/x/once-test-prog", "1"]);
        let b = s(&["/x/once-test-prog", "2"]);
        assert!(first_time("once-test", &a));
        assert!(!first_time("once-test", &b));
        assert!(first_time("once-test-2", &a));
        assert!(first_time("once-test", &s(&["/x/other"])));
    }

    #[test]
    fn checked_exe_rules() {
        assert_eq!(checked_exe(Path::new("ffmpeg")), Err(ExeError::NotAbsolute));
        assert_eq!(
            checked_exe(Path::new("./ffmpeg")),
            Err(ExeError::NotAbsolute)
        );
        let d = std::env::temp_dir().join(format!("lillith-exe-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(checked_exe(&d.join("nope")), Err(ExeError::Missing));
        assert_eq!(checked_exe(&d), Err(ExeError::NotRegularFile));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let f = d.join("tool");
            std::fs::write(&f, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(checked_exe(&f), Err(ExeError::NotExecutable));
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(checked_exe(&f), Ok(f.clone()));
            // PATH lookup skips relative entries and finds the checked one.
            let path = std::env::join_paths([PathBuf::from("rel"), d.clone()]).unwrap();
            assert_eq!(which_in("tool", &path), Some(f.clone()));
            let rel_only = std::env::join_paths([PathBuf::from(".")]).unwrap();
            assert_eq!(which_in("tool", &rel_only), None);
        }
        std::fs::remove_dir_all(&d).ok();
    }
}
