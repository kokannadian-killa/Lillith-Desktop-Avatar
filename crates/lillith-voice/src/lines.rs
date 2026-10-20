//! What she says: [`LineSource`] maps a lillith-core [`Event`] plus the
//! current [`Mood`] to a spoken line. [`DefaultLines`] is a small, PG table.
//!
//! Lookup for an event name is exact first, then by dropping dotted
//! segments (`window.opened.browser` → `window.opened` → `window`). Within
//! a key, lines for the current mood win over mood-agnostic ones. The pick
//! rotates (no immediate repeat) and is deterministic for a given seed.
//!
//! Events that are frequent or purely physical (`input.hover`, `input.drag`,
//! `user.activity`, `volume.changed`, …) deliberately have no lines.
//! Rule lines from `assets/rules.toml` (`Action::Say`) take precedence; see
//! [`crate::Voice::on_event`].

use lillith_core::rng::Rng;
use lillith_core::{Event, Mood};
use std::collections::BTreeMap;

/// Maps events + mood to lines. Implement it to plug in an LLM, a user
/// table, etc. Called on the caller's thread; keep it cheap.
pub trait LineSource: Send {
    /// A line for `event` in `mood`, or `None` to stay quiet.
    fn line_for(&mut self, event: &Event, mood: Mood) -> Option<String>;
}

/// `(event key, mood or None = any mood, lines)`.
type Entry = (&'static str, Option<Mood>, &'static [&'static str]);

/// The default line table: short, playful, PG.
pub const DEFAULT_TABLE: &[Entry] = &[
    // --- pokes
    (
        "input.click.left",
        None,
        &["Hey!", "Boop.", "Yes?", "Careful, that tickles."],
    ),
    (
        "input.click.left",
        Some(Mood::Mischievous),
        &["Poke me again, I dare you~", "Hehe. Gotcha."],
    ),
    (
        "input.click.left",
        Some(Mood::Sleepy),
        &["Mmh… five more minutes."],
    ),
    (
        "input.click.double",
        None,
        &["Whoa, double tap!", "Okay, okay, I'm awake!"],
    ),
    ("input.click.middle", None, &["Need a hand?", "What's up?"]),
    ("input.secret", None, &["You found me out~"]),
    // --- presence
    (
        "user.returned",
        None,
        &["Welcome back!", "There you are.", "Missed me?"],
    ),
    (
        "user.returned",
        Some(Mood::Sleepy),
        &["Oh… hi. I was resting my eyes."],
    ),
    ("idle.long", None, &["It's quiet… I might nap."]),
    (
        "focus.long_work",
        None,
        &[
            "You've been at it a while. Stretch?",
            "Water break? Just saying.",
        ],
    ),
    // --- system
    (
        "cpu.high",
        None,
        &["Your CPU's working hard.", "Things are heating up in here."],
    ),
    (
        "cpu.high",
        Some(Mood::Alert),
        &["CPU's spiking. Want me to keep an eye on it?"],
    ),
    (
        "ram.high",
        None,
        &["Memory's getting full.", "So many tabs… so little RAM."],
    ),
    (
        "battery.low",
        None,
        &["Battery's low. Plug me in?", "Running on fumes here."],
    ),
    (
        "net.lost",
        None,
        &["We lost the network.", "Hello? Internet?"],
    ),
    (
        "net.restored",
        None,
        &["We're back online.", "Network's back."],
    ),
    ("vpn.up", None, &["Tunnel's up."]),
    ("usb.added", None, &["Ooh, something new plugged in."]),
    ("usb.removed", None, &["Bye, little drive."]),
    ("update.available", None, &["Updates are waiting."]),
    ("journal.error", None, &["Something logged an error."]),
    ("app.crash", None, &["Uh-oh. Something crashed."]),
    (
        "screenshot.taken",
        None,
        &["Say cheese!", "Got it on camera."],
    ),
    ("trash.added", None, &["Into the bin it goes."]),
    ("calendar.reminder", None, &["Reminder time!"]),
    ("timer.fired", None, &["Time's up!", "Ding! Timer's done."]),
    ("notify.received", None, &["You've got a notification."]),
    // --- windows
    (
        "window.opened.browser",
        None,
        &["Browsing time?", "What are we looking up?"],
    ),
    (
        "window.opened.terminal",
        None,
        &["Ooh, a terminal. Very hacker."],
    ),
    (
        "window.opened.terminal",
        Some(Mood::Mischievous),
        &["Let me hack something~"],
    ),
    ("window.opened", None, &["New window!"]),
    ("window.closed", None, &["And it's gone."]),
    // --- mind
    ("llm.thinking", None, &["Hmm, let me think…"]),
    ("help.requested", None, &["I'm here. What do you need?"]),
];

/// Events that never get a default line (frequent / physical).
pub const QUIET_EVENTS: &[&str] = &[
    "input.hover",
    "input.hover_long",
    "input.drag",
    "input.click.right",
    "user.activity",
    "idle.asleep",
    "volume.changed",
    "brightness.changed",
    "clipboard.copied",
    "clipboard.cut",
    "llm.done",
];

/// One table variant: optional mood filter and its candidate lines.
type MoodLines = (Option<Mood>, &'static [&'static str]);

/// The default [`LineSource`].
#[derive(Debug, Clone)]
pub struct DefaultLines {
    table: BTreeMap<&'static str, Vec<MoodLines>>,
    rng: Rng,
    last: BTreeMap<String, usize>,
}

impl DefaultLines {
    pub fn new(seed: u64) -> Self {
        Self::with_table(DEFAULT_TABLE, seed)
    }

    pub fn with_table(entries: &[Entry], seed: u64) -> Self {
        let mut table: BTreeMap<&'static str, Vec<_>> = BTreeMap::new();
        for (k, m, l) in entries {
            if !l.is_empty() {
                table.entry(*k).or_default().push((*m, *l));
            }
        }
        Self {
            table,
            rng: Rng::new(seed),
            last: BTreeMap::new(),
        }
    }

    /// The candidate lines for `name` in `mood` (exact key, then parents;
    /// mood-specific lines replace the generic ones when present).
    pub fn candidates(&self, name: &str, mood: Mood) -> Option<&'static [&'static str]> {
        if QUIET_EVENTS.contains(&name) {
            return None;
        }
        let mut key = name;
        loop {
            if let Some(v) = self.table.get(key) {
                let specific = v.iter().find(|(m, _)| *m == Some(mood)).map(|e| e.1);
                let generic = v.iter().find(|(m, _)| m.is_none()).map(|e| e.1);
                if let Some(l) = specific.or(generic) {
                    return Some(l);
                }
            }
            match key.rfind('.') {
                Some(i) => key = &key[..i],
                None => return None,
            }
        }
    }
}

impl Default for DefaultLines {
    fn default() -> Self {
        Self::new(0x11_77_1A)
    }
}

impl LineSource for DefaultLines {
    fn line_for(&mut self, event: &Event, mood: Mood) -> Option<String> {
        let lines = self.candidates(&event.name, mood)?;
        let key = format!("{}/{mood:?}", event.name);
        let n = lines.len();
        let mut i = self.rng.index(n);
        if n > 1 && self.last.get(&key) == Some(&i) {
            i = (i + 1) % n; // never the same line twice in a row
        }
        self.last.insert(key, i);
        Some(lines[i].to_string())
    }
}

/// A [`LineSource`] that never speaks (e.g. "speak rule lines only").
#[derive(Debug, Clone, Copy, Default)]
pub struct NoLines;

impl LineSource for NoLines {
    fn line_for(&mut self, _: &Event, _: Mood) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_parent_and_mood_lookup() {
        let l = DefaultLines::default();
        assert!(l.candidates("cpu.high", Mood::Playful).unwrap().len() >= 2);
        let alert = l.candidates("cpu.high", Mood::Alert).unwrap();
        assert!(alert[0].contains("spiking"), "mood-specific wins");
        // window.opened.app has no entry → falls back to window.opened.
        assert_eq!(
            l.candidates("window.opened.app", Mood::Happy).unwrap(),
            &["New window!"]
        );
        // Terminal has its own; Mischievous gets the mood line.
        let t = l
            .candidates("window.opened.terminal", Mood::Mischievous)
            .unwrap();
        assert!(t[0].contains("hack"));
        assert!(l.candidates("no.such.event", Mood::Playful).is_none());
        assert!(
            l.candidates("input.hover", Mood::Playful).is_none(),
            "quiet event"
        );
    }

    #[test]
    fn no_immediate_repeat_and_deterministic() {
        let ev = Event::new("input.click.left");
        let mut a = DefaultLines::new(7);
        let mut b = DefaultLines::new(7);
        let mut prev = String::new();
        for _ in 0..40 {
            let x = a.line_for(&ev, Mood::Playful).unwrap();
            assert_eq!(
                Some(&x),
                b.line_for(&ev, Mood::Playful).as_ref(),
                "same seed, same lines"
            );
            assert_ne!(x, prev, "no repeat in a row");
            prev = x;
        }
    }

    #[test]
    fn table_is_tasteful_and_short() {
        for (k, _, lines) in DEFAULT_TABLE {
            assert!(!QUIET_EVENTS.contains(k), "{k} is both quiet and voiced");
            for l in *lines {
                assert!(!l.is_empty() && l.chars().count() <= 60, "{k}: {l}");
            }
        }
    }

    #[test]
    fn every_keyed_event_exists_in_rules_toml() {
        let rules = lillith_core::rules::RuleSet::bundled();
        let names: Vec<&str> = rules.rules.iter().map(|r| r.event.as_str()).collect();
        for (k, _, _) in DEFAULT_TABLE {
            // A key matches a rule event exactly or as a dotted parent.
            let ok = names
                .iter()
                .any(|n| *n == *k || n.starts_with(&format!("{k}.")))
                || ["user.returned", "idle.long"].contains(k);
            assert!(ok, "line key {k} matches no rules.toml event");
        }
    }
}
