//! The dream's curiosity queue: what to look up first.
//!
//! Fed by things Ko asked about or mentioned (memory's `request` events,
//! which chat turns log), newest first, bounded ([`CAP`]) and deduped.
//! Mentions that hit the outgoing-request blocklist (personal names from
//! memory, `[learn.dream] blocklist_terms`, this machine's identifiers)
//! never enter the queue, and neither do the personal topics
//! (`ko-preferences`, `self`).
//!
//! **Zero fingerprinting.** A mention never becomes part of an address: it
//! only picks pages from a fixed public [`CATALOG`] (the same list for
//! everyone, on the hosts the [`SEEDS`] already use) and only for topics
//! `[learn] topics` has. Whatever the queue does not cover falls back to
//! the usual `[learn] topics` rotation ([`SeedPlanner::from_config`]).

use std::collections::{HashSet, VecDeque};

use lillith_core::config::Config;

use super::orchestrator::SeedPlanner;
#[cfg(doc)]
use super::orchestrator::SEEDS;
use super::supervisor::Task;
use crate::online::IdentityDenylist;

/// Most mentions the queue keeps (the oldest go first).
pub const CAP: usize = 24;

/// Longest mention kept (characters); longer text is not a topic.
const MAX_MENTION: usize = 64;

/// Topics that are about Ko or Lillith herself: never looked up online.
pub const PERSONAL_TOPICS: &[&str] = &["ko-preferences", "self"];

/// Fixed public pages: (topic, keywords, address). Keywords are matched as
/// whole words against a mention; the address is never built from one.
pub const CATALOG: &[(&str, &str, &str)] = &[
    (
        "rust",
        "trait traits generic generics",
        "https://doc.rust-lang.org/book/ch10-02-traits.html",
    ),
    (
        "rust",
        "thread threads concurrency mutex channel channels",
        "https://doc.rust-lang.org/book/ch16-00-concurrency.html",
    ),
    (
        "rust",
        "error errors panic unwrap result",
        "https://doc.rust-lang.org/book/ch09-00-error-handling.html",
    ),
    (
        "rust",
        "ownership borrow borrowing borrowck lifetime lifetimes",
        "https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html",
    ),
    (
        "rust",
        "cargo crate crates",
        "https://doc.rust-lang.org/cargo/guide/index.html",
    ),
    (
        "linux-garuda",
        "pacman package packages",
        "https://wiki.archlinux.org/title/Pacman",
    ),
    (
        "linux-garuda",
        "systemd service services unit units",
        "https://wiki.archlinux.org/title/Systemd",
    ),
    (
        "linux-garuda",
        "btrfs snapshot snapshots subvolume",
        "https://wiki.archlinux.org/title/Btrfs",
    ),
    (
        "linux-garuda",
        "wayland",
        "https://wiki.archlinux.org/title/Wayland",
    ),
    (
        "linux-garuda",
        "pipewire audio sound",
        "https://wiki.archlinux.org/title/PipeWire",
    ),
    (
        "linux-garuda",
        "wifi networkmanager network",
        "https://wiki.archlinux.org/title/NetworkManager",
    ),
    (
        "python",
        "class classes object objects",
        "https://docs.python.org/3/tutorial/classes.html",
    ),
    (
        "python",
        "pathlib path paths",
        "https://docs.python.org/3/library/pathlib.html",
    ),
    (
        "python",
        "venv virtualenv pip",
        "https://docs.python.org/3/tutorial/venv.html",
    ),
    (
        "coding",
        "complexity algorithm algorithms",
        "https://en.wikipedia.org/wiki/Big_O_notation",
    ),
    (
        "coding",
        "regex regexp regular expression expressions",
        "https://en.wikipedia.org/wiki/Regular_expression",
    ),
    (
        "tools",
        "git commit branch",
        "https://wiki.archlinux.org/title/Git",
    ),
    (
        "tools",
        "bash shell script",
        "https://wiki.archlinux.org/title/Bash",
    ),
    (
        "fixes",
        "troubleshoot troubleshooting crash crashes broken",
        "https://wiki.archlinux.org/title/General_troubleshooting",
    ),
    (
        "fixes",
        "maintenance cleanup update updates",
        "https://wiki.archlinux.org/title/System_maintenance",
    ),
];

/// Ko's recent mentions, newest first, bounded and deduped.
#[derive(Debug, Clone, Default)]
pub struct CuriosityQueue {
    items: VecDeque<String>,
}

impl CuriosityQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// The queue from mentions given newest first (memory's order).
    pub fn from_mentions<I, S>(mentions: I, deny: &IdentityDenylist) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let all: Vec<S> = mentions.into_iter().collect();
        let mut q = Self::new();
        // Pushed oldest first so the newest ends up in front.
        for m in all.iter().rev() {
            q.push(m.as_ref(), deny);
        }
        q
    }

    /// Adds a mention at the front (a repeat moves to the front). Refused
    /// (`false`): empty or overlong text, a personal topic, or anything the
    /// blocklist hits. The oldest mention drops past [`CAP`].
    pub fn push(&mut self, mention: &str, deny: &IdentityDenylist) -> bool {
        let m = normalize(mention);
        if m.is_empty()
            || m.chars().count() > MAX_MENTION
            || PERSONAL_TOPICS.iter().any(|p| m.contains(p))
            || m.split(' ').any(|w| w == "self" || w == "myself")
            || deny.hit(&m).is_some()
        {
            return false;
        }
        self.items.retain(|x| *x != m);
        self.items.push_front(m);
        self.items.truncate(CAP);
        true
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.items.iter().map(String::as_str)
    }

    /// The dream's plan: catalog pages the queue asks about first (newest
    /// mention first, only `[learn] topics`, never a personal topic), then
    /// the `[learn] topics` rotation; one task per address, no repeats.
    pub fn planner(&self, cfg: &Config) -> SeedPlanner {
        let allowed = |t: &str| {
            !PERSONAL_TOPICS.contains(&t) && cfg.learn.topics.iter().any(|x| x.trim() == t)
        };
        let mut seen: HashSet<String> = HashSet::new();
        let mut tasks = Vec::new();
        for m in &self.items {
            let words: HashSet<&str> = m.split(' ').collect();
            for (topic, keys, url) in CATALOG {
                if allowed(topic)
                    && keys.split(' ').any(|k| words.contains(k))
                    && seen.insert(url.to_string())
                {
                    tasks.push(Task::research(topic, &[url]));
                }
            }
        }
        let asked = tasks.len();
        let mut rotation = SeedPlanner::from_config(cfg);
        while let Some(t) = super::orchestrator::DreamPlanner::next(&mut rotation) {
            if t.urls.iter().all(|u| seen.insert(u.clone())) {
                tasks.push(t);
            }
        }
        tracing::debug!(
            mentions = self.items.len(),
            asked,
            total = tasks.len(),
            "dream curiosity queue"
        );
        SeedPlanner::new(tasks)
    }
}

/// Lowercase words (letters, digits, `-`, `+`, `#`), single-spaced.
fn normalize(s: &str) -> String {
    s.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '+' | '#')))
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deny() -> IdentityDenylist {
        IdentityDenylist::default()
            .with_names(["Mira Okafor"])
            .with_terms(&["project nightjar".to_string()])
    }

    fn urls(p: &mut SeedPlanner) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(t) = super::super::orchestrator::DreamPlanner::next(p) {
            assert!(!PERSONAL_TOPICS.contains(&t.topic.as_str()), "{t:?}");
            out.extend(t.urls);
        }
        out
    }

    #[test]
    fn mentions_go_first_then_the_topic_rotation_without_repeats() {
        let cfg = Config::default();
        let q = CuriosityQueue::from_mentions(["btrfs snapshot", "lifetimes rust"], &deny());
        assert_eq!(
            q.iter().collect::<Vec<_>>(),
            ["btrfs snapshot", "lifetimes rust"]
        );
        let got = urls(&mut q.planner(&cfg));
        assert_eq!(got[0], "https://wiki.archlinux.org/title/Btrfs");
        assert_eq!(
            got[1],
            "https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html"
        );
        let mut uniq = got.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), got.len(), "no repeats: {got:?}");
        // Everything the rotation has is still there.
        let rotation = urls(&mut SeedPlanner::from_config(&cfg));
        assert!(rotation.iter().all(|u| got.contains(u)));
        // Every address is a fixed catalog / seed page.
        assert!(got
            .iter()
            .all(|u| CATALOG.iter().any(|c| c.2 == u) || rotation.contains(u)));
    }

    #[test]
    fn an_empty_queue_is_the_plain_rotation() {
        let cfg = Config::default();
        let q = CuriosityQueue::new();
        assert_eq!(
            urls(&mut q.planner(&cfg)),
            urls(&mut SeedPlanner::from_config(&cfg))
        );
    }

    #[test]
    fn names_blocklist_terms_and_personal_topics_never_enter() {
        let d = deny();
        let mut q = CuriosityQueue::new();
        assert!(!q.push("ask mira about git", &d));
        assert!(!q.push("okafor", &d));
        assert!(!q.push("Project Nightjar btrfs", &d));
        assert!(!q.push("ko-preferences", &d));
        assert!(!q.push("tell me about yourself self", &d));
        assert!(!q.push("   ", &d));
        assert!(!q.push(&"word ".repeat(40), &d));
        assert!(q.is_empty());
        assert!(q.push("Git branch", &d));
        assert_eq!(q.iter().collect::<Vec<_>>(), ["git branch"]);
    }

    #[test]
    fn only_configured_topics_are_looked_up() {
        let mut cfg = Config::default();
        cfg.learn.topics = vec!["python".into(), "ko-preferences".into()];
        let q = CuriosityQueue::from_mentions(["btrfs snapshot", "pip venv"], &deny());
        let got = urls(&mut q.planner(&cfg));
        assert_eq!(got[0], "https://docs.python.org/3/tutorial/venv.html");
        assert!(
            got.iter()
                .all(|u| u.starts_with("https://docs.python.org/")),
            "{got:?}"
        );
    }

    #[test]
    fn bounded_and_deduped_newest_first() {
        let d = deny();
        let mut q = CuriosityQueue::new();
        for i in 0..(CAP + 10) {
            q.push(&format!("topic {i}"), &d);
        }
        assert_eq!(q.len(), CAP);
        assert_eq!(q.iter().next(), Some(format!("topic {}", CAP + 9).as_str()));
        q.push("Topic  5!", &d);
        q.push("topic 20", &d);
        assert_eq!(q.len(), CAP);
        assert_eq!(q.iter().next(), Some("topic 20"));
        assert_eq!(q.iter().filter(|m| *m == "topic 20").count(), 1);
    }
}
