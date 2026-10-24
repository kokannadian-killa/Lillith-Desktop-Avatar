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

/// `true` when `text` contains any of `words` as a whole token, or a
/// simple plural/singular of it ("cats" ~ "cat").
pub fn mentions_any(text: &str, words: &[String]) -> bool {
    tokens(text).any(|t| {
        words.iter().any(|w| {
            t == *w
                || t.strip_suffix('s') == Some(w.as_str())
                || w.strip_suffix('s') == Some(t.as_str())
        })
    })
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
            return Some(ForgetCmd::LastSecs(300))
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
            Some(ForgetCmd::LastSecs(300))
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
