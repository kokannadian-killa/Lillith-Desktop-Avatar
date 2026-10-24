//! Parsing "forget ..." chat commands.

/// What to forget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgetCmd {
    /// Everything (graph, events, errors).
    All,
    /// Memories mentioning a topic / entity.
    Topic(String),
    /// Everything learned in the last N seconds.
    LastSecs(u64),
    /// "Forget the last thing I said": the last user turn (and its reply)
    /// in the chat history, plus what memory learned in the last
    /// [`LAST_TURN_SECS`].
    LastTurn,
}

/// The memory window "the last thing I said" clears (the chat history
/// only loses the last turn).
pub const LAST_TURN_SECS: u64 = 300;

/// Time-window forgets longer than this need the same "yes" as
/// "forget everything" ("forget the last 1000 months" is everything).
pub const CONFIRM_WINDOW_SECS: u64 = 3600;

impl ForgetCmd {
    /// Whether Ko has to say "yes" before this runs: everything, a topic,
    /// or a time window longer than [`CONFIRM_WINDOW_SECS`]. Small windows
    /// and "the last thing I said" run at once.
    pub fn needs_confirm(&self) -> bool {
        match self {
            ForgetCmd::All | ForgetCmd::Topic(_) => true,
            ForgetCmd::LastSecs(s) => *s > CONFIRM_WINDOW_SECS,
            ForgetCmd::LastTurn => false,
        }
    }

    /// The memory window this clears, if it is a time-based forget.
    pub fn window_secs(&self) -> Option<u64> {
        match self {
            ForgetCmd::LastSecs(s) => Some(*s),
            ForgetCmd::LastTurn => Some(LAST_TURN_SECS),
            _ => None,
        }
    }
}

const LEADS: &[&str] = &[
    "lillith",
    "lil",
    "hey",
    "please",
    "pls",
    "ok",
    "okay",
    "babe",
    "can you",
    "could you",
    "i want you to",
    "i need you to",
];

/// Words a topic is stripped of before matching ("forget about my ex" →
/// `ex`). Matching is on whole words only, never substrings.
pub const TOPIC_STOPWORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "my",
    "your",
    "our",
    "his",
    "her",
    "their",
    "its",
    "it",
    "me",
    "i",
    "you",
    "we",
    "he",
    "she",
    "they",
    "him",
    "them",
    "about",
    "of",
    "to",
    "and",
    "or",
    "that",
    "this",
    "these",
    "those",
    "all",
    "everything",
    "anything",
    "stuff",
    "things",
    "thing",
    "ko",
    "ko's",
    "is",
    "was",
    "are",
    "were",
    "be",
    "s",
    "with",
    "on",
    "in",
    "at",
    "for",
    "from",
    "any",
    "some",
    "what",
    "who",
    "said",
    "say",
    "told",
    "tell",
    "know",
    "knew",
    "remember",
    "memory",
    "memories",
];

/// The content words of a forget topic: lowercased alphanumeric tokens minus
/// [`TOPIC_STOPWORDS`], deduplicated, in order. 2-letter words stay (`ex`).
pub fn content_words(topic: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in topic
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
    {
        if TOPIC_STOPWORDS.contains(&w) || out.iter().any(|o| o == w) {
            continue;
        }
        out.push(w.to_string());
    }
    out
}

/// The whole-word tokens of `text` (lowercased alphanumeric runs).
pub fn tokens(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
}

/// Words that end in "s" but are not the plural of the word without it
/// ("news" is not "new"s).
const NOT_PLURALS: &[&str] = &[
    "news",
    "bus",
    "gas",
    "yes",
    "plus",
    "lens",
    "series",
    "species",
    "always",
    "perhaps",
    "its",
    "his",
    "hers",
    "ours",
    "yours",
    "theirs",
    "was",
    "has",
    "does",
    "is",
    "this",
    "thus",
    "us",
    "chaos",
    "physics",
    "mathematics",
    "politics",
    "economics",
    "christmas",
    "mars",
    "texas",
    "paris",
    "james",
    "jesus",
];

/// `true` when `plural` is the regular plural of `singular`: `+s`
/// ("job"/"jobs"), `+es` after s/x/z/ch/sh ("box"/"boxes"), or y→`ies`
/// ("hobby"/"hobbies"). The singular needs 3+ letters and must not itself
/// end in "s"; words like "news" are never plurals.
fn is_plural_of(plural: &str, singular: &str) -> bool {
    if singular.chars().count() < 3 || singular.ends_with('s') || NOT_PLURALS.contains(&plural) {
        return false;
    }
    if plural.strip_suffix('s') == Some(singular) {
        return true;
    }
    if ["x", "z", "ch", "sh"].iter().any(|e| singular.ends_with(e))
        && plural.strip_suffix("es") == Some(singular)
    {
        return true;
    }
    if let (Some(stem), Some(pstem)) = (singular.strip_suffix('y'), plural.strip_suffix("ies")) {
        let consonant = stem.chars().last().is_some_and(|c| !"aeiou".contains(c));
        return consonant && stem == pstem;
    }
    false
}

/// `true` when two tokens are the same word: identical, or the exact
/// singular/plural of each other ([`is_plural_of`]).
pub fn same_word(a: &str, b: &str) -> bool {
    a == b || is_plural_of(a, b) || is_plural_of(b, a)
}

/// `true` when `text` contains any of `words` as a whole token (or its
/// exact singular/plural). Prefer [`matches_topic`] for deletes.
pub fn mentions_any(text: &str, words: &[String]) -> bool {
    tokens(text).any(|t| words.iter().any(|w| same_word(&t, w)))
}

/// How many of `n` topic content words one row must contain: all of them
/// for one or two words, else at least two thirds (rounded up).
pub fn required_matches(n: usize) -> usize {
    if n <= 2 {
        n
    } else {
        (2 * n).div_ceil(3)
    }
}

/// `true` when one row (all of `fields` together: key, label, detail ...)
/// mentions enough of the topic's content `words` ([`required_matches`]),
/// each as a whole token or its exact singular/plural. "new job" therefore
/// never matches "new wave music" (only one of two words).
pub fn matches_topic(fields: &[&str], words: &[String]) -> bool {
    if words.is_empty() {
        return false;
    }
    let toks: Vec<String> = fields.iter().flat_map(|f| tokens(f)).collect();
    let hits = words
        .iter()
        .filter(|w| toks.iter().any(|t| same_word(t, w)))
        .count();
    hits >= required_matches(words.len())
}

/// `true` when `text` contains the whole `phrase` as consecutive tokens
/// (exact words, no plural fuzz): "google engineer" is in "met a google
/// engineer" but not in "google chrome".
pub fn contains_phrase(text: &str, phrase: &str) -> bool {
    let p: Vec<String> = tokens(phrase).collect();
    if p.is_empty() {
        return false;
    }
    let t: Vec<String> = tokens(text).collect();
    t.windows(p.len()).any(|w| w == p.as_slice())
}

/// Parses a forget command at the start of `text` ("forget everything",
/// "please forget about my ex", "forget the last 30 minutes", "wipe your
/// memory"). Deliberately narrow: "don't forget to...", "forget it",
/// "forget about it, let's go", long sentences and topics with no content
/// words are not commands. `All`/`Topic` still need a "yes" (see
/// `lillith-mind`).
pub fn parse_forget(text: &str) -> Option<ForgetCmd> {
    let mut t = text
        .trim()
        .trim_end_matches(['.', '!', '?', ' '])
        .to_lowercase();
    loop {
        let before = t.len();
        for l in LEADS {
            if let Some(rest) = t.strip_prefix(l) {
                if rest.is_empty() || rest.starts_with([' ', ',']) {
                    t = rest.trim_start_matches([' ', ',']).to_string();
                }
            }
        }
        if t.len() == before {
            break;
        }
    }
    if matches!(
        t.as_str(),
        "wipe your memory"
            | "wipe memory"
            | "clear your memory"
            | "clear memory"
            | "erase your memory"
    ) {
        return Some(ForgetCmd::All);
    }
    let rest = t.strip_prefix("forget")?;
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None; // "forgetful"
    }
    let rest = rest.trim();
    // A command is a short imperative, not a sentence that happens to start
    // with "forget" ("forget about it, let's go").
    if rest.contains([',', ';', ':', '"', '(', ')'])
        || rest.split_whitespace().count() > 6
        || rest.starts_with("to ")
    {
        return None;
    }
    match rest {
        "everything"
        | "all"
        | "it all"
        | "all of it"
        | "everything about me"
        | "all memories"
        | "all your memories"
        | "everything you know"
        | "everything you know about me" => return Some(ForgetCmd::All),
        "the last thing i said" | "last thing i said" | "what i just said" => {
            return Some(ForgetCmd::LastTurn)
        }
        _ => {}
    }
    if let Some(secs) = parse_last(rest) {
        return Some(ForgetCmd::LastSecs(secs));
    }
    let topic = rest
        .strip_prefix("about ")
        .or_else(|| rest.strip_prefix("everything about "))
        .or_else(|| rest.strip_prefix("all about "))
        .unwrap_or(rest)
        .trim();
    if topic.split_whitespace().count() > 4 || content_words(topic).is_empty() {
        return None; // "forget it", "forget about that", "forget me"
    }
    Some(ForgetCmd::Topic(topic.to_string()))
}

/// "the last 30 minutes", "last hour", "the past 2 hours", "last 10 min".
fn parse_last(rest: &str) -> Option<u64> {
    let r = rest
        .strip_prefix("the ")
        .unwrap_or(rest)
        .trim_start_matches("last ")
        .trim_start_matches("past ");
    if r.len() == rest.strip_prefix("the ").unwrap_or(rest).len() {
        return None; // no "last"/"past"
    }
    let mut parts = r.split_whitespace();
    if r.split_whitespace().count() > 2 {
        return None; // "the last time we talked about cats"
    }
    let first = parts.next()?;
    let (n, unit) = match first.parse::<u64>() {
        Ok(n) => (n, parts.next()?),
        Err(_) => (1, first),
    };
    let mult = match unit.trim_end_matches('s') {
        "second" | "sec" => 1,
        "minute" | "min" | "m" => 60,
        "hour" | "hr" | "h" => 3600,
        "day" | "d" => 86_400,
        "week" | "wk" | "w" => 604_800,
        "month" | "mo" => 2_592_000,
        _ => return None,
    };
    Some(n.saturating_mul(mult))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        assert_eq!(parse_forget("forget everything"), Some(ForgetCmd::All));
        assert_eq!(
            parse_forget("Lillith, please forget everything!"),
            Some(ForgetCmd::All)
        );
        assert_eq!(parse_forget("wipe your memory"), Some(ForgetCmd::All));
        assert_eq!(
            parse_forget("forget about my ex"),
            Some(ForgetCmd::Topic("my ex".into()))
        );
        assert_eq!(
            parse_forget("forget pineapple"),
            Some(ForgetCmd::Topic("pineapple".into()))
        );
        assert_eq!(
            parse_forget("forget the last 30 minutes"),
            Some(ForgetCmd::LastSecs(1800))
        );
        assert_eq!(
            parse_forget("forget the last hour"),
            Some(ForgetCmd::LastSecs(3600))
        );
        assert_eq!(
            parse_forget("forget past 2 days"),
            Some(ForgetCmd::LastSecs(172_800))
        );
        assert_eq!(
            parse_forget("forget the last week"),
            Some(ForgetCmd::LastSecs(604_800))
        );
        assert_eq!(
            parse_forget("forget the last 2 months"),
            Some(ForgetCmd::LastSecs(5_184_000))
        );
        assert_eq!(
            parse_forget("forget the last thing I said"),
            Some(ForgetCmd::LastTurn)
        );
        assert_eq!(parse_forget("forget it all"), Some(ForgetCmd::All));
    }

    #[test]
    fn topic_content_words_strip_stopwords_and_keep_short_words() {
        assert_eq!(content_words("my ex"), vec!["ex"]);
        assert_eq!(content_words("about your Sarah"), vec!["sarah"]);
        assert_eq!(content_words("the it of my"), Vec::<String>::new());
        assert_eq!(content_words("Ko's new job"), vec!["new", "job"]);
        let w = vec!["cat".to_string()];
        assert!(mentions_any("Ko likes cats", &w));
        assert!(!mentions_any("Ko likes concatenate", &w), "no substrings");
    }

    #[test]
    fn plural_rule_is_exact_singular_plural_only() {
        assert!(same_word("job", "jobs"));
        assert!(same_word("jobs", "job"));
        assert!(same_word("box", "boxes"));
        assert!(same_word("hobby", "hobbies"));
        assert!(!same_word("new", "news"), "news is not new+s");
        assert!(!same_word("news", "new"));
        assert!(!same_word("ex", "exs"), "2-letter words never pluralize");
        assert!(!same_word("bus", "bu"));
        assert!(!same_word("boy", "boies"), "vowel+y is not y->ies");
    }

    #[test]
    fn topic_needs_all_or_two_thirds_of_its_words_in_one_row() {
        assert_eq!(required_matches(1), 1);
        assert_eq!(required_matches(2), 2);
        assert_eq!(required_matches(3), 2);
        assert_eq!(required_matches(4), 3);
        let new_job = content_words("my new job");
        assert!(!matches_topic(
            &["likes:new wave music", "new wave music"],
            &new_job
        ));
        assert!(matches_topic(
            &["attr:job", "my new job at the bakery"],
            &new_job
        ));
        assert!(!matches_topic(
            &["likes:new wave music"],
            &content_words("the news")
        ));
        assert!(matches_topic(&["attr:jobs"], &content_words("my job")));
        let three = content_words("red sports car");
        assert!(matches_topic(&["wants a red car"], &three), "2 of 3");
        assert!(!matches_topic(&["a red apple"], &three), "1 of 3");
    }

    #[test]
    fn values_expand_only_as_a_whole_phrase() {
        assert!(contains_phrase(
            "met a Google engineer today",
            "google engineer"
        ));
        assert!(!contains_phrase("google chrome", "google engineer"));
        assert!(!contains_phrase("engineer at google", "google engineer"));
        assert!(contains_phrase("Sarah texted", "sarah"));
        assert!(!contains_phrase("anything", ""));
    }

    #[test]
    fn large_windows_need_confirmation() {
        assert!(ForgetCmd::All.needs_confirm());
        assert!(ForgetCmd::Topic("ex".into()).needs_confirm());
        assert!(!ForgetCmd::LastTurn.needs_confirm());
        assert!(!parse_forget("forget the last 30 minutes")
            .unwrap()
            .needs_confirm());
        assert!(!parse_forget("forget the last hour")
            .unwrap()
            .needs_confirm());
        assert!(parse_forget("forget the last 2 hours")
            .unwrap()
            .needs_confirm());
        assert!(parse_forget("forget the last day").unwrap().needs_confirm());
        assert!(parse_forget("forget the last 1000 months")
            .unwrap()
            .needs_confirm());
    }

    #[test]
    fn not_commands() {
        for s in [
            "don't forget to drink water",
            "forget it",
            "forget that",
            "I always forget my keys",
            "forgetful today",
            "forget",
            "hello",
            "forget about it, let's go",
            "forget about it",
            "forget me",
            "forget about my",
            "forget to call mom",
            "forget the last time we talked about cats",
            "forget it, I'll do it myself",
            "forget what I said about the deadline yesterday at the office",
        ] {
            assert_eq!(parse_forget(s), None, "{s}");
        }
    }
}
