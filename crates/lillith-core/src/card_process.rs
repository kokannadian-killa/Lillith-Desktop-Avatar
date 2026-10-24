//! The stand-alone Soul Contract card (`lillith contract-card`) for code
//! that runs before (or outside) an egui window: the KWin pre-start check
//! and the voice daemon's mic gate. winit allows one event loop per
//! process, so the card is its own short-lived process.
//!
//! Protocol (stdin, UTF-8): line 1 title, line 2 summary, line 3 target
//! (may be empty), the rest is the detail shown verbatim. The card prints
//! one word on stdout: `yes` (a primary-button mouse click on the
//! non-focusable Yes; voice, keys and Esc never approve), `no`,
//! `timeout` or `cancelled` (window closed).

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What the card shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardText {
    pub title: String,
    pub summary: String,
    pub target: String,
    pub detail: String,
}

/// The card's answer, or why there wasn't one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardAnswer {
    Yes,
    No,
    TimedOut,
    Cancelled,
    /// No card could be shown (no display, no card binary, it crashed).
    NoUi,
}

impl CardAnswer {
    /// The word the card prints (and the voice daemon reads).
    pub fn word(self) -> &'static str {
        match self {
            CardAnswer::Yes => "yes",
            CardAnswer::No => "no",
            CardAnswer::TimedOut => "timeout",
            CardAnswer::Cancelled => "cancelled",
            CardAnswer::NoUi => "no_ui",
        }
    }

    /// Parses the card's stdout: only an exact `yes` line is a Yes;
    /// anything unexpected is [`CardAnswer::NoUi`].
    pub fn from_output(out: &str) -> Self {
        match out.trim() {
            "yes" => CardAnswer::Yes,
            "no" => CardAnswer::No,
            "timeout" => CardAnswer::TimedOut,
            "cancelled" => CardAnswer::Cancelled,
            _ => CardAnswer::NoUi,
        }
    }
}

/// Single-line fields keep the protocol unambiguous.
fn one_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ")
}

/// The card's stdin.
pub fn encode(t: &CardText) -> String {
    format!(
        "{}\n{}\n{}\n{}",
        one_line(&t.title),
        one_line(&t.summary),
        one_line(&t.target),
        t.detail
    )
}

/// Inverse of [`encode`] (missing lines are empty).
pub fn decode(s: &str) -> CardText {
    let mut it = s.splitn(4, '\n');
    let mut next = || it.next().unwrap_or("").trim_end_matches('\r').to_string();
    let title = next();
    let summary = next();
    let target = next();
    let detail = next();
    CardText {
        title,
        summary,
        target,
        detail,
    }
}

/// A fresh card id (`<prefix>-<unix ms>-<pid>`), for the audit log.
pub fn new_card_id(prefix: &str) -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{prefix}-{ms}-{}", std::process::id())
}

/// Whether a window can be shown at all (Linux: a Wayland or X display).
pub fn display_available() -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        return true;
    }
    ["WAYLAND_DISPLAY", "DISPLAY"]
        .iter()
        .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
}

/// Shows the card with `exe contract-card --timeout <secs>` and waits for
/// the answer (killed, as [`CardAnswer::TimedOut`], a few seconds after
/// its own timeout).
pub fn ask(exe: &Path, text: &CardText, timeout: Duration) -> CardAnswer {
    ask_until(exe, text, timeout, || false)
}

/// [`ask`] that also closes the card when `stop()` turns true (the
/// safeword): the card process is killed and the answer is
/// [`CardAnswer::Cancelled`], never a Yes.
pub fn ask_until(
    exe: &Path,
    text: &CardText,
    timeout: Duration,
    stop: impl Fn() -> bool,
) -> CardAnswer {
    if !display_available() {
        return CardAnswer::NoUi;
    }
    show_and_wait(exe, text, timeout, stop)
}

/// Spawns the card and waits for its word (no display check; tested).
fn show_and_wait(
    exe: &Path,
    text: &CardText,
    timeout: Duration,
    stop: impl Fn() -> bool,
) -> CardAnswer {
    let mut child = match Command::new(exe)
        .arg("contract-card")
        .arg("--timeout")
        .arg(timeout.as_secs().max(1).to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "Soul Contract card could not start ({}): {e}",
                exe.display()
            );
            return CardAnswer::NoUi;
        }
    };
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(encode(text).as_bytes());
    }
    let hard = Instant::now() + timeout + Duration::from_secs(10);
    loop {
        if stop() {
            let _ = child.kill();
            let _ = child.wait();
            return CardAnswer::Cancelled;
        }
        match child.try_wait() {
            Ok(Some(_)) => {
                let mut out = String::new();
                if let Some(mut o) = child.stdout.take() {
                    let _ = o.read_to_string(&mut out);
                }
                return CardAnswer::from_output(&out);
            }
            Ok(None) if Instant::now() >= hard => {
                let _ = child.kill();
                let _ = child.wait();
                return CardAnswer::TimedOut;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return CardAnswer::NoUi,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdin_round_trips_and_single_line_fields_cannot_spill() {
        let t = CardText {
            title: "KWin\nchange".into(),
            summary: "s".into(),
            target: String::new(),
            detail: "a\n  b\n\nc".into(),
        };
        let d = decode(&encode(&t));
        assert_eq!(d.title, "KWin change");
        assert_eq!(d.target, "");
        assert_eq!(d.detail, "a\n  b\n\nc");
        assert_eq!(
            decode("only"),
            CardText {
                title: "only".into(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn only_an_exact_yes_is_yes() {
        assert_eq!(CardAnswer::from_output("yes\n"), CardAnswer::Yes);
        assert_eq!(CardAnswer::from_output("no"), CardAnswer::No);
        assert_eq!(CardAnswer::from_output("timeout"), CardAnswer::TimedOut);
        assert_eq!(CardAnswer::from_output("cancelled"), CardAnswer::Cancelled);
        for junk in ["", "y", "YES", "yes please", "ok"] {
            assert_eq!(CardAnswer::from_output(junk), CardAnswer::NoUi, "{junk}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_safeword_closes_an_open_card_and_it_is_never_a_yes() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = std::env::temp_dir().join(format!(
            "lillith-card-safeword-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A "card" that would answer yes after a long wait.
        let exe = dir.join("card.sh");
        std::fs::write(&exe, "#!/bin/sh\ncat >/dev/null\nsleep 30\necho yes\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let said_red = AtomicBool::new(false);
        let t0 = Instant::now();
        let a = show_and_wait(&exe, &CardText::default(), Duration::from_secs(60), || {
            if t0.elapsed() > Duration::from_millis(200) {
                said_red.store(true, Ordering::SeqCst);
            }
            said_red.load(Ordering::SeqCst)
        });
        assert_eq!(a, CardAnswer::Cancelled);
        assert!(
            t0.elapsed() < Duration::from_secs(10),
            "the card was not closed"
        );
        // The same card left alone does answer (so Cancelled above is the
        // safeword, not a broken script).
        let exe = dir.join("card-yes.sh");
        std::fs::write(&exe, "#!/bin/sh\ncat >/dev/null\necho yes\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let a = show_and_wait(&exe, &CardText::default(), Duration::from_secs(60), || {
            false
        });
        assert_eq!(a, CardAnswer::Yes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_card_binary_is_no_ui() {
        let a = ask(
            Path::new("/nonexistent/lillith-card"),
            &CardText::default(),
            Duration::from_secs(1),
        );
        assert_eq!(a, CardAnswer::NoUi);
    }
}
