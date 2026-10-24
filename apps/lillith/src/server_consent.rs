//! Consent for `lillith chat --spawn-server`: starting llama-server is a
//! program launch, so it goes through the Soul Contract like every other.
//!
//! 1. A desktop is available: the same model-server card as the chat
//!    bubble's, in its own window (`lillith server-card`). Only a click on
//!    Yes approves (`CONFIRM`); No / Esc is `DENY`, an unanswered card
//!    `TIMEOUT`, a closed window `CANCELLED`.
//! 2. No card could be shown (no display, or the window couldn't open):
//!    nothing starts (`NO_UI`) and the terminal gets the command to start
//!    llama-server by hand. There is no `[y/N]`: a typed (or piped, or
//!    spoken) yes never approves a launch; nothing is read from the
//!    terminal or stdin.
//!
//! Every path first audits `PROPOSED`; all lines share the card's id.

use lillith_core::soul_contract::{Answer, PendingAction, SoulContract};
use lillith_mind::chat::{CardChoice, SERVER_CARD_TIMEOUT};

/// How the start was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Answered (or not) on the card.
    Card(CardChoice),
    /// No card could be shown: refused, text-only (`display`: whether a
    /// desktop was there and only the window failed).
    NoUi { display: bool },
}

impl Decision {
    /// Only a clicked Yes on the card starts the server.
    pub fn approved(self) -> bool {
        matches!(self, Decision::Card(CardChoice::Clicked(Answer::Yes)))
    }

    /// A line for the terminal.
    pub fn describe(self) -> &'static str {
        match self {
            Decision::Card(CardChoice::Clicked(Answer::Yes)) => {
                "approved; starting the local model server"
            }
            Decision::Card(CardChoice::Clicked(Answer::No)) => {
                "declined; not starting the local model server (text-only)"
            }
            Decision::Card(CardChoice::TimedOut) => {
                "no answer on the card; not starting the local model server (text-only)"
            }
            Decision::Card(CardChoice::Cancelled) => {
                "card closed; not starting the local model server (text-only)"
            }
            Decision::NoUi { display: false } => {
                "no display for the Soul Contract card, and a typed yes never counts; \
                 not starting the local model server (text-only)"
            }
            Decision::NoUi { display: true } => {
                "the Soul Contract card window could not open, and a typed yes never counts; \
                 not starting the local model server (text-only)"
            }
        }
    }
}

/// What to print after a refusal without a card: how to start llama-server
/// by hand (the exact command the card would have shown, secrets masked).
pub fn manual_start_hint(argv: &[String]) -> String {
    let masked = lillith_core::soul_contract::mask_secret_args(argv);
    let cmd = lillith_setup::launch::LaunchCommand {
        program: masked.first().cloned().unwrap_or_default(),
        args: masked.iter().skip(1).cloned().collect(),
    };
    format!(
        "To start the local model server yourself, run this in another terminal:\n  {}\n\
         (if [chat] api_key is set, export LLAMA_API_KEY=<your key> first),\n\
         then run `lillith chat` again; a server that already answers needs no card.",
        cmd.to_shell_string()
    )
}

/// Asks for `action` on the card and audits the decision in `sc`. `card`
/// shows the card (`None`: no card could be shown; `display` says whether
/// a desktop was there). Injectable for tests. Nothing is ever read from a
/// terminal or stdin.
pub fn decide(
    sc: &mut SoulContract,
    action: PendingAction,
    display: bool,
    card: impl FnOnce(&PendingAction) -> Option<CardChoice>,
) -> Decision {
    sc.record("PROPOSED", &action);
    let shown = if display { card(&action) } else { None };
    let Some(choice) = shown else {
        let why = if display {
            "card window could not open; a typed yes never approves; nothing started"
        } else {
            "no display for the card; a typed yes never approves; nothing started"
        };
        sc.record_note("NO_UI", &action, why);
        return Decision::NoUi { display };
    };
    match choice {
        CardChoice::Clicked(Answer::Yes) => {
            let kept = action.clone();
            let id = sc.request(action);
            if let Err(e) = sc.confirm(id) {
                tracing::warn!("model-server approval failed: {e}");
                // No CONFIRM was written: close the card in the log.
                sc.record_note(
                    "CANCELLED",
                    &kept,
                    &format!("approval could not be recorded: {e}"),
                );
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
    Decision::Card(choice)
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
/// process never owns a window event loop). `None` when the window
/// couldn't open.
pub fn show_card(action: &PendingAction) -> Option<CardChoice> {
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

#[cfg(test)]
mod tests {
    use super::*;

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

    fn run(tag: &str, display: bool, card: Option<CardChoice>) -> (Decision, Vec<String>, bool) {
        let log = Log::new(tag);
        let a = action();
        let id = a.card_id().unwrap().to_string();
        let mut card_asked = false;
        let d = decide(&mut log.contract(), a, display, |_| {
            card_asked = true;
            card
        });
        (d, log.decisions(&id), card_asked)
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
            let (d, log, asked) = run(tag, true, Some(choice));
            assert!(asked);
            assert_eq!(d, Decision::Card(choice));
            assert_eq!(d.approved(), choice == yes, "{choice:?}");
            assert_eq!(log, ["PROPOSED", tag]);
        }
    }

    #[test]
    fn without_a_card_nothing_is_approved_and_nothing_is_asked() {
        // No display: the card isn't even tried, and there is no terminal
        // question to answer y to.
        let (d, log, asked) = run("nodisplay", false, Some(CardChoice::Clicked(Answer::Yes)));
        assert!(!asked, "no display: no card window");
        assert_eq!(d, Decision::NoUi { display: false });
        assert!(!d.approved());
        assert_eq!(log, ["PROPOSED", "NO_UI"]);
        assert!(d.describe().contains("typed yes never counts"));
        // A desktop, but the window failed: refused the same way.
        let (d, log, asked) = run("nowindow", true, None);
        assert!(asked);
        assert_eq!(d, Decision::NoUi { display: true });
        assert!(!d.approved());
        assert_eq!(log, ["PROPOSED", "NO_UI"]);
    }

    #[test]
    fn the_manual_hint_names_the_command_with_secrets_masked() {
        let h = manual_start_hint(&[
            "/opt/llama-server".into(),
            "--port".into(),
            "8080".into(),
            "--api-key".into(),
            "sekrit".into(),
        ]);
        assert!(h.contains("/opt/llama-server --port 8080 --api-key"), "{h}");
        assert!(!h.contains("sekrit"), "{h}");
        assert!(h.contains("LLAMA_API_KEY"), "{h}");
    }
}
