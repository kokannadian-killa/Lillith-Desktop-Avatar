//! Dream-learning entry points (docs/SELF_LEARNING.md §11, P7) that every
//! surface shares: the `/dream` chat command parser and a small
//! process-wide hook registry the right-click menu, the chat bubble and the
//! CLI call into.
//!
//! Nothing here approves or starts anything. The only way a dream session
//! starts is a mouse click on Yes on its Soul Contract card
//! (`lillith_learn::workers::dream`); `/dream` and "Dream learning…" only
//! *open* that card. A typed or spoken "yes" is never an approval.
//!
//! The app registers the real hooks ([`set_hooks`]); a process with no
//! hooks (tests, a bare chat bubble) answers [`Dispatch::NotWired`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Session length `/dream` and "Dream learning…" propose when none is
/// given (inside the configured 30–480 min range; see [`default_minutes`]).
pub const DEFAULT_MINUTES: u64 = 120;

/// The bubble → app event that asks the app to open the dream card
/// (detail: minutes, or empty for the default). Starts with `chat.` so the
/// bubble pipe forwards it.
pub const OPEN_EVENT: &str = "chat.dream_open";
/// The bubble → app event that ends the running dream early.
pub const END_EARLY_EVENT: &str = "chat.dream_end_early";
/// The bubble → app event that hard-stops the running dream (`/dream stop`).
pub const STOP_EVENT: &str = "chat.dream_stop";

/// A parsed `/dream …` chat command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DreamCommand {
    /// `/dream` or `/dream 2h` / `90m` / `1h30m` / `120`: open the card.
    Open { minutes: Option<u64> },
    /// `/dream end` (also `end early`, `wake`, `wake up`).
    EndEarly,
    /// `/dream stop`: the hard stop.
    Stop,
    /// `/dream status`.
    Status,
    /// `/dream <junk>`: the reason, for a helpful reply.
    Invalid(String),
}

/// Parses `/dream …` (the whole message; case-insensitive). `None` for
/// anything else, so ordinary chat ("I had a dream") is never a command.
pub fn parse_command(text: &str) -> Option<DreamCommand> {
    let t = text.trim().to_lowercase();
    let rest = t.strip_prefix("/dream")?;
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let arg = rest.trim();
    Some(match arg {
        "" => DreamCommand::Open { minutes: None },
        "end" | "end early" | "wake" | "wake up" => DreamCommand::EndEarly,
        "stop" => DreamCommand::Stop,
        "status" => DreamCommand::Status,
        a => match parse_duration(a) {
            Some(m) => DreamCommand::Open { minutes: Some(m) },
            None => DreamCommand::Invalid(format!(
                "I don't understand \"{a}\" — try /dream, /dream 2h, /dream 90m, /dream end or /dream status"
            )),
        },
    })
}

/// Minutes in `2h`, `90m`, `1h30m`, `1.5h` or a bare `120` (minutes).
/// `None` when unreadable or zero. Range checks are the card's job.
pub fn parse_duration(s: &str) -> Option<u64> {
    let s: String = s
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if s.is_empty() {
        return None;
    }
    if let Ok(m) = s.parse::<u64>() {
        return (m > 0).then_some(m);
    }
    let s = s
        .replace("hours", "h")
        .replace("hour", "h")
        .replace("hrs", "h")
        .replace("hr", "h")
        .replace("minutes", "m")
        .replace("minute", "m")
        .replace("mins", "m")
        .replace("min", "m");
    let mut total = 0f64;
    let mut num = String::new();
    let mut seen_unit = false;
    for c in s.chars() {
        match c {
            '0'..='9' | '.' => num.push(c),
            'h' | 'm' => {
                let v: f64 = num.parse().ok()?;
                if !v.is_finite() || v < 0.0 {
                    return None;
                }
                total += if c == 'h' { v * 60.0 } else { v };
                num.clear();
                seen_unit = true;
            }
            _ => return None,
        }
    }
    if !num.is_empty() || !seen_unit {
        return None;
    }
    let m = total.round();
    (1.0..=100_000.0).contains(&m).then_some(m as u64)
}

/// The length used when none is given: [`DEFAULT_MINUTES`] clamped into
/// `min..=max` (the `[learn.dream]` range).
pub fn default_minutes(min: u64, max: u64) -> u64 {
    let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
    DEFAULT_MINUTES.clamp(lo, hi)
}

/// What a hook call did, for an honest reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatch {
    /// The card is opening (approval is a mouse click on it, nothing else).
    CardOpening,
    /// A session is already running or a card is already up.
    Busy,
    /// The running session was asked to end.
    Ending,
    /// No session is running.
    NotRunning,
    /// A status line.
    Status(String),
    /// This process has no dream wiring.
    NotWired,
}

type OpenHook = Arc<dyn Fn(Option<u64>) -> Dispatch + Send + Sync>;
type EndHook = Arc<dyn Fn() -> Dispatch + Send + Sync>;
type StatusHook = Arc<dyn Fn() -> String + Send + Sync>;

/// The callbacks a surface needs.
#[derive(Clone)]
pub struct Hooks {
    /// Open the dream card for `minutes` (`None` = default).
    pub open: OpenHook,
    /// End the running session early.
    pub end_early: EndHook,
    /// Hard-stop the running session.
    pub stop: EndHook,
    /// One status line.
    pub status: StatusHook,
}

static HOOKS: Mutex<Option<Hooks>> = Mutex::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Registers this process's dream hooks (the app's dream wiring, or the
/// chat bubble's event forwarder). Replaces any earlier set.
pub fn set_hooks(h: Hooks) {
    if let Ok(mut g) = HOOKS.lock() {
        *g = Some(h);
    }
}

/// Drops the hooks (tests; shutdown).
pub fn clear_hooks() {
    if let Ok(mut g) = HOOKS.lock() {
        *g = None;
    }
}

fn hooks() -> Option<Hooks> {
    HOOKS.lock().ok().and_then(|g| g.clone())
}

/// "Dream learning…" (right-click) / `/dream [duration]`: opens the dream
/// card. Never approves anything.
pub fn open_card(minutes: Option<u64>) -> Dispatch {
    match hooks() {
        Some(h) => (h.open)(minutes),
        None => Dispatch::NotWired,
    }
}

/// "End dream early" (right-click) / `/dream end`.
pub fn end_early() -> Dispatch {
    match hooks() {
        Some(h) => (h.end_early)(),
        None => Dispatch::NotWired,
    }
}

/// `/dream stop`: the hard stop (the kill switch and Ctrl-C stop it too).
pub fn stop() -> Dispatch {
    match hooks() {
        Some(h) => (h.stop)(),
        None => Dispatch::NotWired,
    }
}

/// One status line (`/dream status`).
pub fn status_line() -> Dispatch {
    match hooks() {
        Some(h) => Dispatch::Status((h.status)()),
        None => Dispatch::NotWired,
    }
}

/// Marks a session running / not (the menu shows "End dream early" while
/// it is). Set by the orchestrator's owner.
pub fn set_active(on: bool) {
    ACTIVE.store(on, Ordering::SeqCst);
}

/// What the right-click menu shows ([`crate::menu::MenuExtras::dream`]):
/// `None` when this process has no dream wiring, else whether a session
/// is running.
pub fn menu_state() -> Option<bool> {
    hooks().map(|_| is_active())
}

/// Whether a dream session is running in this process.
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

/// Runs a parsed command through the hooks.
pub fn dispatch(cmd: &DreamCommand) -> Dispatch {
    match cmd {
        DreamCommand::Open { minutes } => open_card(*minutes),
        DreamCommand::EndEarly => end_early(),
        DreamCommand::Stop => stop(),
        DreamCommand::Status => status_line(),
        DreamCommand::Invalid(why) => Dispatch::Status(why.clone()),
    }
}

/// Her chat line for a dispatch result.
pub fn reply_line(cmd: &DreamCommand, d: &Dispatch) -> String {
    match d {
        Dispatch::CardOpening => match cmd {
            DreamCommand::Open { minutes: Some(m) } => format!(
                "Mm, a {m}-minute dream? I've put the card up — click Yes on it if you want me to go. Typing yes won't do it."
            ),
            _ => "I've put the dream card up — click Yes on it if you want me to go. Typing yes won't do it.".into(),
        },
        Dispatch::Busy => "I'm already dreaming (or the card is already up).".into(),
        Dispatch::Ending => match cmd {
            DreamCommand::Stop => "Stopping the dream now.".into(),
            _ => "Waking up — I'll tell you what I found.".into(),
        },
        Dispatch::NotRunning => "I'm not dreaming right now.".into(),
        Dispatch::Status(s) => s.clone(),
        Dispatch::NotWired => "Dream learning isn't wired up here — right-click me → Dream learning…, or run `lillith dream`.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dream_commands() {
        assert_eq!(
            parse_command("/dream"),
            Some(DreamCommand::Open { minutes: None })
        );
        assert_eq!(
            parse_command("  /Dream 2h "),
            Some(DreamCommand::Open { minutes: Some(120) })
        );
        assert_eq!(
            parse_command("/dream 90m"),
            Some(DreamCommand::Open { minutes: Some(90) })
        );
        assert_eq!(
            parse_command("/dream 1h30m"),
            Some(DreamCommand::Open { minutes: Some(90) })
        );
        assert_eq!(
            parse_command("/dream 1.5h"),
            Some(DreamCommand::Open { minutes: Some(90) })
        );
        assert_eq!(
            parse_command("/dream 45"),
            Some(DreamCommand::Open { minutes: Some(45) })
        );
        assert_eq!(
            parse_command("/dream 2 hours"),
            Some(DreamCommand::Open { minutes: Some(120) })
        );
        assert_eq!(parse_command("/dream end"), Some(DreamCommand::EndEarly));
        assert_eq!(parse_command("/dream stop"), Some(DreamCommand::Stop));
        assert_eq!(parse_command("/dream status"), Some(DreamCommand::Status));
        assert!(matches!(
            parse_command("/dream banana"),
            Some(DreamCommand::Invalid(_))
        ));
        assert!(matches!(
            parse_command("/dream 0m"),
            Some(DreamCommand::Invalid(_))
        ));
        assert_eq!(parse_command("I had a dream"), None);
        assert_eq!(parse_command("/dreamy"), None);
        assert_eq!(parse_command("yes"), None);
    }

    #[test]
    fn default_is_inside_the_range() {
        assert_eq!(default_minutes(30, 480), 120);
        assert_eq!(default_minutes(30, 60), 60);
        assert_eq!(default_minutes(200, 480), 200);
        assert_eq!(default_minutes(480, 30), 120);
    }

    #[test]
    fn replies_never_claim_a_typed_yes_works() {
        let open = DreamCommand::Open { minutes: Some(90) };
        let l = reply_line(&open, &Dispatch::CardOpening);
        assert!(l.contains("click Yes") && l.contains("90"));
        assert!(l.contains("Typing yes won't"));
        assert!(reply_line(&DreamCommand::EndEarly, &Dispatch::NotWired).contains("isn't"));
    }
}
