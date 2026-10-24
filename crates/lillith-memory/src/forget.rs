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

/// Parses a forget command at the start of `text` ("forget everything",
/// "please forget about my ex", "forget the last 30 minutes", "wipe your
/// memory"). "don't forget to..." and "forget it" are not commands.
pub fn parse_forget(text: &str) -> Option<ForgetCmd> {
    let mut t = text
        .trim()
        .trim_end_matches(|c: char| matches!(c, '.' | '!' | '?' | ' '))
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
    match rest {
        "everything"
        | "all"
        | "it all"
        | "all of it"
        | "everything about me"
        | "me"
        | "all memories"
        | "all your memories" => return Some(ForgetCmd::All),
        "" | "it" | "that" | "this" | "about it" | "about that" | "you" => return None,
        _ => {}
    }
    if let Some(secs) = parse_last(rest) {
        return Some(ForgetCmd::LastSecs(secs));
    }
    let topic = rest
        .strip_prefix("about ")
        .or_else(|| rest.strip_prefix("that "))
        .or_else(|| rest.strip_prefix("everything about "))
        .unwrap_or(rest)
        .trim()
        .trim_start_matches("the ")
        .trim();
    (topic.chars().count() >= 2).then(|| ForgetCmd::Topic(topic.to_string()))
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
        ] {
            assert_eq!(parse_forget(s), None, "{s}");
        }
    }
}
