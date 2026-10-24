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
//! Every tool action first audits `PROPOSED` (before the card), then its
//! outcome line, like the model-server card.
//!
//! Plans and deletions (see `chat.rs`) use the same card through
//! [`ask_card`]: a typed answer there only PICKS (which plan items, Trash or
//! permanent, the typed list phrase); the run itself still needs the card's
//! clicked Yes. The safeword closes an open card (`CANCELLED`).

use lillith_core::card_process::CardText;
use lillith_core::soul_contract::{Answer, PendingAction, SoulContract};
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

/// Shows `text` on the card (only when `display`) and says how it was
/// decided. `card` shows the card (`None`: it could not open); injectable
/// for tests. Nothing is read from stdin and nothing is audited or run here.
pub fn ask_card(
    display: bool,
    text: &CardText,
    card: impl FnOnce(&CardText) -> Option<CardChoice>,
) -> Decision {
    match if display { card(text) } else { None } {
        Some(c) => Decision::Card(c),
        None => Decision::NoUi { display },
    }
}

/// Audits `PROPOSED` for `action`, asks for it on the card (only when
/// `display`), audits the outcome in `sc`, and runs the action only after a
/// clicked Yes. `card` shows the card (`None`: it could not open);
/// injectable for tests. Returns the decision and the result to feed back
/// to her.
pub fn offer(
    sc: &mut SoulContract,
    action: PendingAction,
    display: bool,
    card: impl FnOnce(&CardText) -> Option<CardChoice>,
) -> (Decision, Result<String, String>) {
    sc.record("PROPOSED", &action);
    let choice = match ask_card(display, &card_text(&action), card) {
        Decision::Card(c) => Some(c),
        Decision::NoUi { .. } => None,
    };
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

/// What the card shows for a tool action.
pub fn card_text(action: &PendingAction) -> CardText {
    let p = action.prompt();
    CardText {
        title: p.title.to_string(),
        summary: p.summary.clone(),
        target: p.target.clone().unwrap_or_default(),
        detail: p.detail.clone().unwrap_or_default(),
    }
}

/// Shows the card in its own process (the shared `lillith contract-card`,
/// see `lillith_core::card_process`, so this process never owns a window
/// event loop). Its stdin carries only the card text, never an answer: the
/// card approves only on a primary mouse click on its non-focusable Yes.
/// The safeword ("red" / "OOC: stop") closes it: `Cancelled`, never a Yes.
/// `None` when the window couldn't open.
pub fn show_card(text: &CardText) -> Option<CardChoice> {
    // The safeword (hush) or a stop closes the card.
    use lillith_core::access::kill_switch::{hush_epoch as epoch, hushed_since as stopped_since};
    let exe = std::env::current_exe().ok()?;
    let start = epoch();
    let answer =
        lillith_core::card_process::ask_until(&exe, text, CARD_TIMEOUT, || stopped_since(start));
    after_safeword(card_choice(answer), stopped_since(start))
}

/// A card answered while (or just after) the safeword fired is `Cancelled`,
/// even a Yes: nothing runs.
fn after_safeword(choice: Option<CardChoice>, stopped: bool) -> Option<CardChoice> {
    if stopped {
        Some(CardChoice::Cancelled)
    } else {
        choice
    }
}

/// The shared card's answer as a [`CardChoice`]; `NoUi` is `None`.
fn card_choice(a: lillith_core::card_process::CardAnswer) -> Option<CardChoice> {
    use lillith_core::card_process::CardAnswer;
    Some(match a {
        CardAnswer::Yes => CardChoice::Clicked(Answer::Yes),
        CardAnswer::No => CardChoice::Clicked(Answer::No),
        CardAnswer::TimedOut => CardChoice::TimedOut,
        CardAnswer::Cancelled => CardChoice::Cancelled,
        CardAnswer::NoUi => return None,
    })
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
            assert_eq!(t.decisions(), ["PROPOSED", tag], "{tag}");
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
            assert_eq!(t.decisions(), ["PROPOSED", "NO_UI"]);
        }
    }

    #[test]
    fn proposed_is_written_before_the_card_and_the_outcome_follows() {
        let t = Tmp::new("proposed");
        let mut sc = SoulContract::new(Some(t.0.join("audit.log")));
        let log = t.0.join("audit.log");
        let (d, _) = offer(&mut sc, write(&t), true, |text| {
            // The card is up: PROPOSED is already in the log, nothing else.
            let now = std::fs::read_to_string(&log).unwrap_or_default();
            let tags: Vec<&str> = now.lines().filter_map(|l| l.split('\t').nth(1)).collect();
            assert_eq!(tags, ["PROPOSED"], "{now}");
            assert!(
                now.contains("write_file") && now.contains("out.txt"),
                "{now}"
            );
            assert!(text.detail.contains("hello"));
            Some(CardChoice::Clicked(Answer::Yes))
        });
        assert!(d.approved());
        assert_eq!(t.decisions(), ["PROPOSED", "CONFIRM"]);
    }

    #[test]
    fn the_safeword_turns_any_card_answer_into_cancelled() {
        let yes = Some(CardChoice::Clicked(Answer::Yes));
        assert_eq!(after_safeword(yes, true), Some(CardChoice::Cancelled));
        assert_eq!(after_safeword(None, true), Some(CardChoice::Cancelled));
        assert_eq!(after_safeword(yes, false), yes);
        let t = Tmp::new("red");
        let mut sc = SoulContract::new(Some(t.0.join("audit.log")));
        let (d, res) = offer(&mut sc, write(&t), true, |_| after_safeword(yes, true));
        assert!(!d.approved() && res.is_err());
        assert!(!t.0.join("out.txt").exists());
        assert_eq!(t.decisions(), ["PROPOSED", "CANCELLED"]);
    }

    #[test]
    fn only_the_cards_yes_word_is_a_clicked_yes() {
        use lillith_core::card_process::CardAnswer;
        for w in ["y", "YES", " yes please", "ok", "", "no_ui"] {
            assert_ne!(
                card_choice(CardAnswer::from_output(w)),
                Some(CardChoice::Clicked(Answer::Yes)),
                "{w:?}"
            );
        }
        assert_eq!(
            card_choice(CardAnswer::from_output("yes\n")),
            Some(CardChoice::Clicked(Answer::Yes))
        );
        assert_eq!(card_choice(CardAnswer::NoUi), None);
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
