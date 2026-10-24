//! Consent for a tool action Lillith proposes in console chat
//! (`lillith chat`): writing a file or skill, running a command, launching
//! a program, … Only a mouse click on **Yes** on the Soul Contract card
//! approves. A typed (or piped, or spoken) yes never counts, and nothing is
//! read from stdin or the terminal for this.
//!
//! 1. A desktop is available: the card opens in its own window
//!    (`lillith contract-card`, mouse-only Yes). Yes is `CONFIRM` (and the
//!    action runs), No / Esc `DENY`, an unanswered card `TIMEOUT`, a closed
//!    window `CANCELLED`.
//! 2. No desktop, or the card window can't open: `NO_UI`. Nothing runs and
//!    the console says how to do it by hand.
//!
//! Plans and deletions keep their own console review (see `chat.rs`).

use lillith_core::soul_contract::{Answer, ContractPrompt, PendingAction, SoulContract};
use lillith_mind::chat::CardChoice;
use std::time::Duration;

/// How long the card waits for a click.
pub const CARD_TIMEOUT: Duration = Duration::from_secs(120);

/// How the action was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Answered (or not) on the card.
    Card(CardChoice),
    /// No card could be shown (`display`: a desktop was there but the window
    /// failed). Never approved.
    NoUi { display: bool },
}

impl Decision {
    /// Only a clicked Yes on the card.
    pub fn approved(self) -> bool {
        self == Decision::Card(CardChoice::Clicked(Answer::Yes))
    }

    /// Why nothing ran (`None` when approved).
    pub fn refusal(self) -> Option<&'static str> {
        if self.approved() {
            return None;
        }
        Some(match self {
            Decision::Card(CardChoice::Clicked(Answer::Yes)) => return None,
            Decision::Card(CardChoice::Clicked(Answer::No)) => "declined on the card",
            Decision::Card(CardChoice::TimedOut) => "no answer on the card",
            Decision::Card(CardChoice::Cancelled) => "card closed",
            Decision::NoUi { display: false } => {
                "no display for the Soul Contract card, and a typed yes never counts"
            }
            Decision::NoUi { display: true } => {
                "the Soul Contract card window could not open, and a typed yes never counts"
            }
        })
    }
}

/// Asks for `action` on the card (only when `display`), audits the outcome
/// in `sc`, and runs the action only after a clicked Yes. `card` shows the
/// card (`None`: it could not open); injectable for tests. Returns the
/// decision and the result to feed back to her.
pub fn offer(
    sc: &mut SoulContract,
    action: PendingAction,
    display: bool,
    card: impl FnOnce(&PendingAction) -> Option<CardChoice>,
) -> (Decision, Result<String, String>) {
    let choice = if display { card(&action) } else { None };
    let Some(choice) = choice else {
        let note = if display {
            "card window could not open; a typed yes never approves; nothing ran"
        } else {
            "no display for the card; a typed yes never approves; nothing ran"
        };
        sc.record_note("NO_UI", &action, note);
        let d = Decision::NoUi { display };
        return (
            d,
            Err(format!("{}; nothing ran", d.refusal().unwrap_or(""))),
        );
    };
    let d = Decision::Card(choice);
    match choice {
        CardChoice::Clicked(Answer::Yes) => {
            let id = sc.request(action);
            match sc.confirm(id) {
                Ok(done) => {
                    let mut out = String::new();
                    if let Some(p) = &done.path {
                        out.push_str(&format!("done: {}\n", p.display()));
                    }
                    if let Some(o) = &done.output {
                        out.push_str(o);
                    }
                    (d, Ok(out))
                }
                Err(e) => (d, Err(format!("it failed: {e}"))),
            }
        }
        CardChoice::Clicked(Answer::No) => {
            let id = sc.request(action);
            let _ = sc.deny(id);
            (d, Err("Ko said no".to_string()))
        }
        CardChoice::TimedOut => {
            sc.record("TIMEOUT", &action);
            (d, Err("no answer on the card; nothing ran".to_string()))
        }
        CardChoice::Cancelled => {
            sc.record("CANCELLED", &action);
            (d, Err("the card was closed; nothing ran".to_string()))
        }
    }
}

/// How Ko can do `action` himself when no card could be shown.
pub fn manual_hint(action: &PendingAction) -> String {
    let p = action.prompt();
    let how = match action {
        PendingAction::RunShell { command } | PendingAction::RunCommand { command, .. } => {
            let cwd = match action {
                PendingAction::RunCommand { cwd: Some(d), .. } => {
                    format!(" (in {})", d.display())
                }
                _ => String::new(),
            };
            format!("run it yourself in a terminal{cwd}:\n  $ {command}")
        }
        PendingAction::Launch { argv, .. } => {
            let masked = lillith_core::soul_contract::mask_secret_args(argv);
            format!("start it yourself:\n  $ {}", masked.join(" "))
        }
        PendingAction::WriteFile { path, .. } | PendingAction::WriteSkill { path, .. } => {
            format!("save the text shown above to {} yourself", path.display())
        }
        _ => match &p.target {
            Some(t) => format!("do it yourself: {} ({t})", p.summary),
            None => format!("do it yourself: {}", p.summary),
        },
    };
    format!(
        "Nothing ran and nothing changed. To do this by hand, {how}\n\
         Or run `lillith chat` in a desktop session, where the Soul Contract card can open \
         (only a mouse click on its Yes approves)."
    )
}

/// A desktop session a window can open on.
pub fn display_available() -> bool {
    crate::server_consent::display_available()
}

/// Shows the card in its own process (`lillith contract-card`, so this
/// process never owns a window event loop). Its stdin is closed: it can't
/// read a typed answer. `None` when the window couldn't open.
pub fn show_card(action: &PendingAction) -> Option<CardChoice> {
    let p = action.prompt();
    let exe = std::env::current_exe().ok()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("contract-card")
        .arg("--title")
        .arg(p.title)
        .arg("--kind")
        .arg(p.kind)
        .arg("--summary")
        .arg(&p.summary);
    if let Some(t) = &p.target {
        cmd.arg("--target").arg(t);
    }
    if let Some(d) = &p.detail {
        cmd.arg("--detail").arg(d);
    }
    let out = cmd
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let word = String::from_utf8_lossy(&out.stdout);
    lillith_panel::server_card::parse_choice_word(word.lines().last().unwrap_or(""))
}

/// `lillith contract-card`: the card window for a proposed tool action;
/// prints the answer word (`yes` / `no` / `timeout` / `cancelled`).
pub fn card_main(
    kind: String,
    title: String,
    summary: String,
    target: Option<String>,
    detail: Option<String>,
) -> anyhow::Result<()> {
    // A short-lived process: the prompt's static strings live until exit.
    let prompt = ContractPrompt {
        kind: Box::leak(kind.into_boxed_str()),
        title: Box::leak(title.into_boxed_str()),
        summary,
        target,
        detail,
    };
    let c = lillith_panel::server_card::run_contract_card(
        prompt,
        CARD_TIMEOUT,
        lillith_panel::server_card::Initiator::Her,
    )
    .map_err(|e| anyhow::anyhow!("contract card: {e}"))?;
    println!("{}", lillith_panel::server_card::choice_word(c));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) struct Tmp(pub std::path::PathBuf);

    impl Tmp {
        pub(crate) fn new(tag: &str) -> Self {
            let d = std::env::temp_dir().join(format!(
                "lillith-contract-consent-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Self(d)
        }

        fn decisions(&self) -> Vec<String> {
            std::fs::read_to_string(self.0.join("audit.log"))
                .unwrap_or_default()
                .lines()
                .filter_map(|l| l.split('\t').nth(1).map(String::from))
                .collect()
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(t: &Tmp) -> PendingAction {
        PendingAction::WriteFile {
            path: t.0.join("out.txt"),
            body: "hello".into(),
        }
    }

    #[test]
    fn only_a_clicked_yes_runs_it() {
        let yes = CardChoice::Clicked(Answer::Yes);
        for (choice, tag) in [
            (yes, "CONFIRM"),
            (CardChoice::Clicked(Answer::No), "DENY"),
            (CardChoice::TimedOut, "TIMEOUT"),
            (CardChoice::Cancelled, "CANCELLED"),
        ] {
            let t = Tmp::new(tag);
            let mut sc = SoulContract::new(Some(t.0.join("audit.log")));
            let (d, res) = offer(&mut sc, write(&t), true, |_| Some(choice));
            assert_eq!(d, Decision::Card(choice));
            assert_eq!(d.approved(), choice == yes);
            assert_eq!(res.is_ok(), choice == yes, "{res:?}");
            assert_eq!(t.0.join("out.txt").exists(), choice == yes, "{tag}");
            assert_eq!(t.decisions(), [tag], "{tag}");
        }
    }

    #[test]
    fn without_a_card_nothing_runs_and_nothing_is_asked() {
        for display in [false, true] {
            let t = Tmp::new(&format!("noui-{display}"));
            let mut sc = SoulContract::new(Some(t.0.join("audit.log")));
            let mut asked = false;
            let (d, res) = offer(&mut sc, write(&t), display, |_| {
                asked = true;
                None
            });
            assert_eq!(asked, display, "the card is tried only with a display");
            assert_eq!(d, Decision::NoUi { display });
            assert!(!d.approved() && res.is_err());
            assert!(!t.0.join("out.txt").exists());
            assert_eq!(t.decisions(), ["NO_UI"]);
        }
    }

    #[test]
    fn the_hint_says_how_to_do_it_by_hand() {
        let h = manual_hint(&PendingAction::RunShell {
            command: "ls -la".into(),
        });
        assert!(h.contains("$ ls -la") && h.contains("Nothing ran"), "{h}");
        let h = manual_hint(&PendingAction::Launch {
            argv: ["/bin/app", "--api-key", "s3cret"]
                .map(String::from)
                .to_vec(),
            app: None,
        });
        assert!(
            !h.contains("s3cret") && h.contains("/bin/app --api-key ***"),
            "{h}"
        );
        let t = Tmp::new("hint");
        assert!(manual_hint(&write(&t)).contains("out.txt"));
    }
}
