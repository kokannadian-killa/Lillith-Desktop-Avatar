//! Cheap rule-based fact extraction (no LLM call).
//!
//! [`RuleExtractor`] pulls preferences ("I like/love/hate/prefer X"),
//! attributes ("my X is Y", "call me X"), reminders ("remind me to ...")
//! and app names out of Ko's message. Anything smarter can plug in through
//! the [`Extractor`] trait later (e.g. an LLM-assisted extractor that runs
//! when the model is idle).

use regex::Regex;
use std::sync::OnceLock;

/// How Ko feels about something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    Like,
    Dislike,
    Prefer,
}

impl Polarity {
    pub fn as_str(self) -> &'static str {
        match self {
            Polarity::Like => "likes",
            Polarity::Dislike => "dislikes",
            Polarity::Prefer => "prefers",
        }
    }
}

/// One thing learned from a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    /// "I love dark synthwave" → `Preference { Like, "dark synthwave" }`.
    Preference { polarity: Polarity, object: String },
    /// "my cat is Nyx" → `Attribute { "cat", "Nyx" }`.
    Attribute { attr: String, value: String },
    /// "remind me to stretch at 5" → `Reminder("stretch at 5")`.
    Reminder(String),
    /// An app mentioned or asked for ("open firefox").
    App(String),
}

/// Pluggable extraction (rule-based by default).
pub trait Extractor: Send {
    /// Facts in Ko's message. `reply` is her answer (context only; facts are
    /// never learned from her own words).
    fn extract(&self, user_msg: &str, reply: &str) -> Vec<Fact>;
}

/// The default regex/heuristic extractor.
#[derive(Debug, Default, Clone, Copy)]
pub struct RuleExtractor;

/// Apps recognised by name (lowercase). Anything after "open"/"launch"/
/// "start" is also taken as an app.
pub const KNOWN_APPS: &[&str] = &[
    "firefox",
    "chrome",
    "chromium",
    "brave",
    "vscode",
    "vs code",
    "steam",
    "discord",
    "spotify",
    "obs",
    "blender",
    "gimp",
    "krita",
    "inkscape",
    "konsole",
    "kitty",
    "alacritty",
    "terminal",
    "dolphin",
    "thunderbird",
    "vlc",
    "mpv",
    "telegram",
    "signal",
    "slack",
    "libreoffice",
    "kdenlive",
    "audacity",
    "lutris",
    "neovim",
    "nvim",
];

const OBJ: &str = r"([^.,!?;:\n]{2,80})";

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("static regex"))
}

fn like_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        &format!(
            r"(?i)\bi\s+(?:really\s+|totally\s+|kinda\s+|just\s+)?(?:like|love|adore|enjoy|dig)\s+{OBJ}"
        ),
    )
}

fn dislike_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        &format!(
            r"(?i)\bi\s+(?:really\s+|totally\s+|just\s+)?(?:hate|dislike|detest|can'?t\s+stand|don'?t\s+like|do\s+not\s+like)\s+{OBJ}"
        ),
    )
}

fn prefer_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(&R, &format!(r"(?i)\bi\s+(?:really\s+)?prefer\s+{OBJ}"))
}

fn my_is_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"(?i)\bmy\s+((?:favou?rite\s+)?[a-z][a-z'\- ]{0,30}?)\s+(?:is|are|=)\s+([^.,!?;\n]{1,60})",
    )
}

fn call_me_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"(?i)\b(?:call\s+me|my\s+name'?s)\s+([a-z][\w\-]{0,30})",
    )
}

fn remind_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"(?i)\b(?:remind\s+me|don'?t\s+let\s+me\s+forget)\s+(?:to\s+|about\s+|that\s+)?([^.!?\n]{2,120})",
    )
}

fn open_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(
        &R,
        r"(?i)\b(?:open|launch|start|run|fire\s+up)\s+(?:up\s+)?(?:the\s+|my\s+)?([a-z][\w\-\.]{1,30})",
    )
}

/// Words that end an extracted object ("I love synthwave but not ...").
const CUTS: &[&str] = &[
    " but ",
    " and then ",
    " because ",
    " cause ",
    " when ",
    " so much",
    " a lot",
    " lol",
    " though",
    " tho",
];

/// Trims an extracted object to something short and storable.
fn clean_object(raw: &str) -> Option<String> {
    let mut s = format!(" {} ", raw.trim().to_lowercase());
    for cut in CUTS {
        if let Some(i) = s.find(cut) {
            s.truncate(i);
        }
    }
    let words: Vec<&str> = s
        .split_whitespace()
        .filter(|w| !matches!(*w, "it" | "that" | "this" | "you" | "to"))
        .take(6)
        .collect();
    let out = words.join(" ");
    let out = out.trim_matches(|c: char| !c.is_alphanumeric()).to_string();
    (out.chars().count() >= 2).then_some(out)
}

/// Lowercase, alphanumeric-only.
fn slug(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '.')
        .collect()
}

impl Extractor for RuleExtractor {
    fn extract(&self, user_msg: &str, _reply: &str) -> Vec<Fact> {
        let mut out = Vec::new();
        let mut push = |f: Fact| {
            if !out.contains(&f) {
                out.push(f);
            }
        };
        // Dislikes first: "I don't like X" must not also read as a like.
        let mut disliked = Vec::new();
        for c in dislike_re().captures_iter(user_msg) {
            if let Some(o) = clean_object(&c[1]) {
                disliked.push(o.clone());
                push(Fact::Preference {
                    polarity: Polarity::Dislike,
                    object: o,
                });
            }
        }
        for c in like_re().captures_iter(user_msg) {
            if let Some(o) = clean_object(&c[1]) {
                if !disliked.contains(&o) {
                    push(Fact::Preference {
                        polarity: Polarity::Like,
                        object: o,
                    });
                }
            }
        }
        for c in prefer_re().captures_iter(user_msg) {
            if let Some(o) = clean_object(&c[1]) {
                push(Fact::Preference {
                    polarity: Polarity::Prefer,
                    object: o,
                });
            }
        }
        for c in my_is_re().captures_iter(user_msg) {
            let attr = c[1].trim().to_lowercase();
            if matches!(attr.as_str(), "god" | "bad" | "turn" | "fault") {
                continue;
            }
            if let Some(v) = clean_object(&c[2]) {
                push(Fact::Attribute { attr, value: v });
            }
        }
        for c in call_me_re().captures_iter(user_msg) {
            push(Fact::Attribute {
                attr: "name".into(),
                value: c[1].to_string(),
            });
        }
        for c in remind_re().captures_iter(user_msg) {
            let t = c[1].trim().trim_end_matches(|c: char| !c.is_alphanumeric());
            if t.len() >= 2 {
                push(Fact::Reminder(t.to_string()));
            }
        }
        let lower = user_msg.to_lowercase();
        for app in KNOWN_APPS {
            if contains_word(&lower, app) {
                push(Fact::App(slug(app)));
            }
        }
        for c in open_re().captures_iter(user_msg) {
            let a = slug(&c[1]);
            if a.len() >= 2 && !STOPWORDS.contains(&a.as_str()) {
                push(Fact::App(a));
            }
        }
        out
    }
}

/// Whether `hay` (lowercase) contains `word` on word boundaries.
pub fn contains_word(hay: &str, word: &str) -> bool {
    let mut start = 0;
    while let Some(i) = hay[start..].find(word) {
        let a = start + i;
        let b = a + word.len();
        let before = hay[..a]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after = hay[b..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if before && after {
            return true;
        }
        start = a + word.len().max(1);
    }
    false
}

/// Common words ignored for retrieval and intent keys.
pub const STOPWORDS: &[&str] = &[
    "the",
    "and",
    "for",
    "you",
    "your",
    "are",
    "was",
    "were",
    "but",
    "not",
    "with",
    "this",
    "that",
    "what",
    "when",
    "where",
    "who",
    "why",
    "how",
    "can",
    "could",
    "would",
    "should",
    "will",
    "just",
    "have",
    "has",
    "had",
    "its",
    "it's",
    "i'm",
    "im",
    "me",
    "my",
    "mine",
    "our",
    "out",
    "about",
    "from",
    "into",
    "then",
    "than",
    "them",
    "they",
    "there",
    "here",
    "some",
    "any",
    "all",
    "get",
    "got",
    "let",
    "lets",
    "let's",
    "please",
    "pls",
    "hey",
    "lillith",
    "lil",
    "babe",
    "now",
    "today",
    "again",
    "also",
    "too",
    "very",
    "really",
    "ok",
    "okay",
    "yes",
    "no",
    "do",
    "does",
    "did",
    "is",
    "be",
    "to",
    "of",
    "in",
    "on",
    "at",
    "a",
    "an",
    "it",
    "so",
    "up",
    "want",
    "need",
    "like",
    "know",
    "tell",
    "give",
    "make",
    "show",
    "something",
    "thing",
    "it",
    "one",
    "him",
    "her",
    "his",
    "she",
    "he",
    "we",
    "us",
    "am",
    "or",
    "if",
    "yeah",
];

/// Content words of `text`: lowercase, alphanumeric, ≥ 3 chars (or digits),
/// stopwords removed, deduplicated, at most `max`.
pub fn keywords(text: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        if (w.chars().count() < 3 && !w.chars().all(|c| c.is_ascii_digit()))
            || STOPWORDS.contains(&w)
        {
            continue;
        }
        if !out.iter().any(|o| o == w) {
            out.push(w.to_string());
        }
        if out.len() >= max {
            break;
        }
    }
    out
}

/// A short, stable key for "what was asked" (first two content words,
/// sorted), used to spot repeated requests. `None` for chit-chat.
pub fn intent_key(text: &str) -> Option<String> {
    let mut k = keywords(text, 2);
    if k.is_empty() {
        return None;
    }
    k.sort();
    Some(k.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ex(s: &str) -> Vec<Fact> {
        RuleExtractor.extract(s, "")
    }

    fn pref(p: Polarity, o: &str) -> Fact {
        Fact::Preference {
            polarity: p,
            object: o.into(),
        }
    }

    #[test]
    fn preferences() {
        assert_eq!(
            ex("I love dark synthwave"),
            vec![pref(Polarity::Like, "dark synthwave")]
        );
        assert!(ex("I really hate pineapple on pizza, seriously")
            .contains(&pref(Polarity::Dislike, "pineapple on pizza")));
        assert_eq!(
            ex("i don't like mornings"),
            vec![pref(Polarity::Dislike, "mornings")],
            "a dislike is not also a like"
        );
        assert!(ex("I prefer tea over coffee").contains(&pref(Polarity::Prefer, "tea over coffee")));
        assert!(ex("I like metal but not pop").contains(&pref(Polarity::Like, "metal")));
    }

    #[test]
    fn attributes_names_and_reminders() {
        let f = ex("my cat is Nyx. Call me Ko");
        assert!(f.contains(&Fact::Attribute {
            attr: "cat".into(),
            value: "nyx".into()
        }));
        assert!(f.contains(&Fact::Attribute {
            attr: "name".into(),
            value: "Ko".into()
        }));
        assert!(ex("my favorite band is Ghost").contains(&Fact::Attribute {
            attr: "favorite band".into(),
            value: "ghost".into()
        }));
        assert_eq!(
            ex("remind me to stretch at 5pm!"),
            vec![Fact::Reminder("stretch at 5pm".into())]
        );
    }

    #[test]
    fn apps() {
        let f = ex("open firefox and then launch krita");
        assert!(f.contains(&Fact::App("firefox".into())));
        assert!(f.contains(&Fact::App("krita".into())));
        assert!(!ex("the firefoxes").contains(&Fact::App("firefox".into())));
        assert!(ex("hello there").is_empty());
    }

    #[test]
    fn keyword_and_intent() {
        assert_eq!(
            keywords("What's the weather in Edmonton today?", 8),
            vec!["weather", "edmonton"]
        );
        assert_eq!(
            intent_key("check the weather").as_deref(),
            Some("check weather")
        );
        assert_eq!(
            intent_key("weather check?"),
            intent_key("check the weather")
        );
        assert_eq!(intent_key("hey lillith"), None);
        assert!(contains_word("use vs code now", "vs code"));
        assert!(!contains_word("obsidian", "obs"));
    }
}
