//! Consent for `lillith chat --spawn-server`: starting llama-server is a
//! program launch, so it goes through the Soul Contract like every other.
//!
//! 1. A desktop is available: the same model-server card as the chat
//!    bubble's, in its own window (`lillith server-card`). Only a click on
//!    Yes approves (`CONFIRM`); No / Esc is `DENY`, an unanswered card
//!    `TIMEOUT`, a closed window `CANCELLED`.
//! 2. No card could be shown: `Start local model server? [y/N]` on the
//!    controlling terminal (`/dev/tty`, `CONIN$` on Windows), never piped
//!    stdin. Only `y` / `yes` approves (`CONFIRM_TTY`); anything else,
//!    including an empty line or end of input, is `DENY_TTY`.
//! 3. No terminal either: `NO_UI`, and chat stays text-only.
//!
//! Every path first audits `PROPOSED`; all lines share the card's id.

use lillith_core::soul_contract::{Answer, PendingAction, SoulContract};
use lillith_mind::chat::{CardChoice, SERVER_CARD_TIMEOUT};
use std::io::{BufRead, Write};

/// The terminal question.
pub const TTY_QUESTION: &str = "Start local model server? [y/N] ";

/// How the start was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Answered (or not) on the card.
    Card(CardChoice),
    /// Answered on the terminal: `true` only for an explicit y / yes.
    Tty(bool),
    /// Neither a card nor a terminal: text-only.
    NoUi,
}

impl Decision {
    /// Only a clicked Yes or a typed y / yes on the tty starts the server.
    pub fn approved(self) -> bool {
        matches!(
            self,
            Decision::Card(CardChoice::Clicked(Answer::Yes)) | Decision::Tty(true)
        )
    }

    /// A line for the terminal.
    pub fn describe(self) -> &'static str {
        match self {
            Decision::Card(CardChoice::Clicked(Answer::Yes)) | Decision::Tty(true) => {
                "approved; starting the local model server"
            }
            Decision::Card(CardChoice::Clicked(Answer::No)) | Decision::Tty(false) => {
                "declined; not starting the local model server (text-only)"
            }
            Decision::Card(CardChoice::TimedOut) => {
                "no answer on the card; not starting the local model server (text-only)"
            }
            Decision::Card(CardChoice::Cancelled) => {
                "card closed; not starting the local model server (text-only)"
            }
            Decision::NoUi => {
                "no card or terminal to ask on; not starting the local model server (text-only)"
            }
        }
    }
}

/// `true` only for an explicit `y` / `yes` (any case, surrounding space
/// ignored). Everything else, including an empty line, is No.
pub fn tty_says_yes(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Asks for `action` and audits the decision in `sc`. `card` shows the
/// card (`None`: no card could be shown); `tty` asks on the controlling
/// terminal (`None`: no terminal). Both are injectable for tests.
pub fn decide(
    sc: &mut SoulContract,
    action: PendingAction,
    card: impl FnOnce(&PendingAction) -> Option<CardChoice>,
    tty: impl FnOnce(&PendingAction) -> Option<String>,
) -> Decision {
    sc.record("PROPOSED", &action);
    if let Some(choice) = card(&action) {
        match choice {
            CardChoice::Clicked(Answer::Yes) => {
                let id = sc.request(action);
                if let Err(e) = sc.confirm(id) {
                    tracing::warn!("model-server approval failed: {e}");
                    return Decision::Card(CardChoice::Cancelled);
                }
            }
            CardChoice::Clicked(Answer::No) => {
                let id = sc.request(action);
                let _ = sc.deny(id);
            }
            CardChoice::TimedOut => sc.record("TIMEOUT", &action),
            CardChoice::Cancelled => sc.record("CANCELLED", &action),
        }
        return Decision::Card(choice);
    }
    match tty(&action) {
        Some(line) => {
            let yes = tty_says_yes(&line);
            sc.record(if yes { "CONFIRM_TTY" } else { "DENY_TTY" }, &action);
            Decision::Tty(yes)
        }
        None => {
            sc.record("NO_UI", &action);
            Decision::NoUi
        }
    }
}

/// A desktop session a window can open on.
pub fn display_available() -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        return true;
    }
    ["WAYLAND_DISPLAY", "DISPLAY"]
        .iter()
        .any(|v| std::env::var_os(v).is_some_and(|x| !x.is_empty()))
}

/// Shows the card in its own process (`lillith server-card`, so this
/// process never owns a window event loop). `None` when there is no
/// desktop or the window couldn't open: the caller falls back to the tty.
pub fn show_card(action: &PendingAction) -> Option<CardChoice> {
    if !display_available() {
        return None;
    }
    let PendingAction::StartModelServer { argv, restart, .. } = action else {
        return None;
    };
    let exe = std::env::current_exe().ok()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("server-card");
    if *restart {
        cmd.arg("--restart");
    }
    cmd.arg("--").args(argv);
    let out = cmd
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let word = String::from_utf8_lossy(&out.stdout);
    lillith_panel::server_card::parse_choice_word(word.lines().last().unwrap_or(""))
}

/// `lillith server-card`: the card window; prints the answer word.
pub fn card_main(argv: Vec<String>, restart: bool) -> anyhow::Result<()> {
    let prompt = PendingAction::start_model_server(argv, restart).prompt();
    let c = lillith_panel::server_card::run_server_card(prompt, SERVER_CARD_TIMEOUT)
        .map_err(|e| anyhow::anyhow!("server card: {e}"))?;
    println!("{}", lillith_panel::server_card::choice_word(c));
    Ok(())
}

/// Asks [`TTY_QUESTION`] on the controlling terminal and reads one line
/// from it (never from stdin, which may be a pipe). `None` when there is no
/// terminal; a read error or end of input is an empty line (No).
pub fn ask_tty(action: &PendingAction) -> Option<String> {
    let (input, mut output) = open_tty()?;
    if let PendingAction::StartModelServer { argv, .. } = action {
        let _ = writeln!(output, "[soul contract] $ {}", argv.join(" "));
    }
    let _ = write!(output, "{TTY_QUESTION}");
    let _ = output.flush();
    let mut line = String::new();
    if std::io::BufReader::new(input).read_line(&mut line).is_err() {
        line.clear();
    }
    Some(line)
}

#[cfg(unix)]
fn open_tty() -> Option<(std::fs::File, std::fs::File)> {
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let w = f.try_clone().ok()?;
    Some((f, w))
}

#[cfg(windows)]
fn open_tty() -> Option<(std::fs::File, std::fs::File)> {
    let input = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONIN$")
        .ok()?;
    let output = std::fs::OpenOptions::new()
        .write(true)
        .open("CONOUT$")
        .ok()?;
    Some((input, output))
}

#[cfg(not(any(unix, windows)))]
fn open_tty() -> Option<(std::fs::File, std::fs::File)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_y_or_yes_is_yes() {
        for yes in ["y", "Y", "yes", "YES", " Yes \n", "y\r\n"] {
            assert!(tty_says_yes(yes), "{yes:?}");
        }
        for no in [
            "",
            "\n",
            "n",
            "no",
            "N",
            "ye",
            "yess",
            "yes please",
            "sure",
            "ok",
            "1",
            "y y",
            "oui",
        ] {
            assert!(!tty_says_yes(no), "{no:?}");
        }
    }

    struct Log(std::path::PathBuf);

    impl Log {
        fn new(tag: &str) -> Self {
            let d = std::env::temp_dir().join(format!(
                "lillith-consent-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Self(d)
        }

        fn contract(&self) -> SoulContract {
            SoulContract::new(Some(self.0.join("audit.log")))
        }

        /// The decision column of every line, in order, all for one card.
        fn decisions(&self, id: &str) -> Vec<String> {
            std::fs::read_to_string(self.0.join("audit.log"))
                .unwrap_or_default()
                .lines()
                .map(|l| {
                    let c: Vec<&str> = l.split('\t').collect();
                    assert_eq!(c[2], "start_model_server", "{l}");
                    assert_eq!(c[3], "/opt/llama-server", "{l}");
                    assert_eq!(c[4], format!("card={id}"), "{l}");
                    c[1].to_string()
                })
                .collect()
        }
    }

    impl Drop for Log {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn action() -> PendingAction {
        PendingAction::start_model_server(
            vec!["/opt/llama-server".into(), "--port".into(), "8080".into()],
            false,
        )
    }

    fn run(
        tag: &str,
        card: Option<CardChoice>,
        tty: Option<&str>,
    ) -> (Decision, Vec<String>, bool) {
        let log = Log::new(tag);
        let a = action();
        let id = a.card_id().unwrap().to_string();
        let mut tty_asked = false;
        let d = decide(
            &mut log.contract(),
            a,
            |_| card,
            |_| {
                tty_asked = true;
                tty.map(str::to_string)
            },
        );
        (d, log.decisions(&id), tty_asked)
    }

    #[test]
    fn the_card_decides_when_it_can_be_shown() {
        let yes = CardChoice::Clicked(Answer::Yes);
        let no = CardChoice::Clicked(Answer::No);
        for (choice, tag) in [
            (yes, "CONFIRM"),
            (no, "DENY"),
            (CardChoice::TimedOut, "TIMEOUT"),
            (CardChoice::Cancelled, "CANCELLED"),
        ] {
            let (d, log, tty_asked) = run(tag, Some(choice), Some("yes"));
            assert_eq!(d, Decision::Card(choice));
            assert_eq!(d.approved(), choice == yes, "{choice:?}");
            assert_eq!(log, ["PROPOSED", tag]);
            assert!(!tty_asked, "a shown card is never bypassed by the tty");
        }
    }

    #[test]
    fn without_a_card_the_tty_decides_and_only_y_or_yes_approves() {
        let (d, log, _) = run("tty-yes", None, Some("y\n"));
        assert!(d.approved());
        assert_eq!(log, ["PROPOSED", "CONFIRM_TTY"]);
        for (i, answer) in ["", "\n", "n\n", "maybe\n", "yes please\n"]
            .into_iter()
            .enumerate()
        {
            let (d, log, _) = run(&format!("tty-no-{i}"), None, Some(answer));
            assert_eq!(d, Decision::Tty(false), "{answer:?}");
            assert!(!d.approved());
            assert_eq!(log, ["PROPOSED", "DENY_TTY"], "{answer:?}");
        }
    }

    #[test]
    fn no_card_and_no_tty_is_no_ui_and_text_only() {
        let (d, log, tty_asked) = run("noui", None, None);
        assert_eq!(d, Decision::NoUi);
        assert!(!d.approved() && tty_asked);
        assert_eq!(log, ["PROPOSED", "NO_UI"]);
        assert!(d.describe().contains("text-only"));
    }
}
