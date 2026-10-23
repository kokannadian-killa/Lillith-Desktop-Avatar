//! [`speech_text`]: what Lillith's *voice* says for a line she *typed*.
//!
//! The chat bubble keeps every emoji and emoticon; the TTS engines would
//! read them out ("smiling face with horns", "colon right parenthesis",
//! "asterisk winks asterisk"), which makes no sense. This pure function
//! strips them before any engine sees the text:
//!
//! - Unicode emoji / pictographs (incl. ZWJ sequences, skin tones,
//!   variation selectors, regional-indicator flags, tag sequences, keycaps).
//! - Stand-alone text emoticons (`:)` `;-)` `:D` `xD` `<3` `^_^` `o_O` …);
//!   only whole whitespace tokens, so `12:30`, URLs and ratios stay.
//! - `:shortcodes:` (`:smiling_imp:`), kaomoji bracket faces (`(^_^)`,
//!   `( ͡° ͜ʖ ͡°)`, `¯\_(ツ)_/¯`, table flips).
//! - `*roleplay actions*` (`*winks*`, `*bites lip*`): stage directions, not
//!   speech. Markdown `**bold**` keeps its words.
//! - Decorative `~` ("there you are~").
//!
//! Leftover spaces and doubled punctuation are collapsed. If nothing
//! speakable (no letter or digit) remains, the result is empty and the
//! caller skips TTS.

/// The speakable part of `text` (see the module docs). Empty means "say
/// nothing".
pub fn speech_text(text: &str) -> String {
    let s = text.replace("**", "");
    let s = strip_shrugs(&s);
    let s = strip_emoji(&s);
    let s = strip_actions(&s);
    let s = strip_bracket_faces(&s);
    let s = strip_shortcodes(&s);
    let s = strip_tokens(&s);
    let s = tidy(&s);
    if s.chars().any(char::is_alphanumeric) {
        s
    } else {
        String::new()
    }
}

// ---------------------------------------------------------------- emoji --

/// Always-emoji code points (emoji presentation by default or pictographs
/// nobody types as text).
fn is_pictograph(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1FAFF   // mahjong … symbols & pictographs ext-A (incl. flags, skin tones)
        | 0x2600..=0x27BF   // misc symbols + dingbats (☀ ❤ ✨ ✔)
        | 0x231A..=0x231B | 0x2328 | 0x23CF | 0x23E9..=0x23F3 | 0x23F8..=0x23FA
        | 0x2B05..=0x2B07 | 0x2B1B..=0x2B1C | 0x2B50 | 0x2B55
        | 0x2934..=0x2935 | 0x3030 | 0x303D | 0x3297 | 0x3299
    )
}

/// Pictographs that are ordinary text unless followed by U+FE0F
/// (© ® ™ ‼ ⁉ ℹ Ⓜ ↔ ▶ …).
fn is_text_default_pictograph(c: char) -> bool {
    matches!(c as u32,
        0x00A9 | 0x00AE | 0x203C | 0x2049 | 0x2122 | 0x2139 | 0x24C2
        | 0x2194..=0x2199 | 0x21A9..=0x21AA | 0x25AA..=0x25AB | 0x25B6 | 0x25C0
        | 0x25FB..=0x25FE
    )
}

/// Joiners / modifiers that only make sense inside an emoji sequence.
fn is_emoji_glue(c: char) -> bool {
    matches!(c as u32,
        0x200D              // zero-width joiner
        | 0xFE00..=0xFE0F   // variation selectors
        | 0x20E3            // combining keycap
        | 0x1F3FB..=0x1F3FF // skin tones
        | 0xE0020..=0xE007F // tag sequences (subdivision flags)
    )
}

fn strip_emoji(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        let next = cs.get(i + 1).copied();
        // Keycaps: 1️⃣ #⃣ *️⃣ (base, optional FE0F, U+20E3).
        if matches!(c, '0'..='9' | '#' | '*') {
            let j = if next == Some('\u{FE0F}') {
                i + 2
            } else {
                i + 1
            };
            if cs.get(j) == Some(&'\u{20E3}') {
                out.push(' ');
                i = j + 1;
                continue;
            }
        }
        let starts =
            is_pictograph(c) || (is_text_default_pictograph(c) && next == Some('\u{FE0F}'));
        if starts {
            // Swallow the whole sequence: glue, and pictographs joined by ZWJ.
            i += 1;
            while i < cs.len() {
                let d = cs[i];
                if is_emoji_glue(d) {
                    i += 1;
                    if d == '\u{200D}' && i < cs.len() {
                        // ZWJ joins the next element whatever it is.
                        i += 1;
                    }
                } else if (0x1F1E6..=0x1F1FF).contains(&(d as u32)) {
                    // Second regional indicator of a flag.
                    i += 1;
                } else {
                    break;
                }
            }
            out.push(' ');
            continue;
        }
        // Stray variation selectors after normal text are invisible noise.
        if matches!(c as u32, 0xFE0E | 0xFE0F) {
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

// -------------------------------------------------------------- actions --

/// Removes `*stage directions*`: an opening `*` not glued to a word on its
/// left and followed by a non-space, a closing `*` (same line, ≤ 60 chars
/// later) preceded by a non-space and not glued to a word on its right, and
/// at least one letter inside. `2 * 3 * 4` and `a*b*c` are left alone.
fn strip_actions(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    'outer: while i < cs.len() {
        let c = cs[i];
        if c == '*' {
            let left_ok = i == 0 || !cs[i - 1].is_alphanumeric();
            let right_ok = cs
                .get(i + 1)
                .is_some_and(|n| !n.is_whitespace() && *n != '*');
            if left_ok && right_ok {
                let mut j = i + 1;
                while j < cs.len() && j - i <= 61 && cs[j] != '\n' {
                    if cs[j] == '*' {
                        let inner = &cs[i + 1..j];
                        let close_ok = !cs[j - 1].is_whitespace()
                            && cs.get(j + 1).is_none_or(|n| !n.is_alphanumeric());
                        if close_ok && inner.iter().any(|ch| ch.is_alphabetic()) {
                            out.push(' ');
                            i = j + 1;
                            continue 'outer;
                        }
                        break;
                    }
                    j += 1;
                }
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

// ---------------------------------------------------------- kaomoji etc --

/// Characters that only show up in kaomoji / ASCII-art faces.
fn is_face_char(c: char) -> bool {
    matches!(
        c,
        '^' | '°'
            | '▽'
            | 'ω'
            | '･'
            | '・'
            | '\u{0361}'
            | '\u{035C}'
            | 'ʖ'
            | 'ᴥ'
            | '•'
            | 'ಠ'
            | '╯'
            | '╰'
            | '□'
            | '∀'
            | '´'
            | '`'
            | 'ﾉ'
            | 'ツ'
            | '‿'
            | '◕'
            | '☆'
            | '≧'
            | '≦'
            | '◡'
            | '¬'
            | '¯'
            | '︵'
            | '┻'
            | '━'
            | '┳'
            | 'ᗜ'
            | 'ಥ'
            | '益'
            | 'Д'
            | 'ᐛ'
            | '｀'
            | '＾'
            | '∇'
            | '◉'
            | '⊙'
            | 'ò'
            | 'ó'
            | '◔'
            | '﹏'
            | '_'
            | '*'
            | '>'
            | '<'
            | ';'
            | '~'
            | '≖'
            | 'ᵕ'
            | '꒳'
    ) || ('\u{2500}'..='\u{257F}').contains(&c) // box drawing (┻━┻)
}

fn strip_shrugs(s: &str) -> String {
    let mut s = s.to_string();
    for shrug in ["¯\\\\_(ツ)_/¯", "¯\\_(ツ)_/¯", "¯\\_(シ)_/¯", "¯_(ツ)_¯"] {
        s = s.replace(shrug, " ");
    }
    s
}

/// `( … )`, `（ … ）`, `ʕ … ʔ` groups (≤ 24 chars, one line) with no ASCII
/// letters/digits and at least one face character: `(^_^)`, `(╯°□°）`,
/// `( ͡° ͜ʖ ͡°)`. `(2)`, `(30%)`, `(...)` and `(!)` stay.
fn strip_bracket_faces(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    'outer: while i < cs.len() {
        let c = cs[i];
        if matches!(c, '(' | '（' | 'ʕ') {
            let mut j = i + 1;
            while j < cs.len() && j - i <= 25 && cs[j] != '\n' {
                if matches!(cs[j], ')' | '）' | 'ʔ') {
                    let inner = &cs[i + 1..j];
                    let visible = inner.iter().filter(|ch| !ch.is_whitespace()).count();
                    if visible >= 2
                        && !inner.iter().any(|ch| ch.is_ascii_alphanumeric())
                        && inner.iter().any(|ch| is_face_char(*ch))
                    {
                        out.push(' ');
                        i = j + 1;
                        continue 'outer;
                    }
                    break;
                }
                if matches!(cs[j], '(' | '（' | 'ʕ') {
                    break;
                }
                j += 1;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// `:smiling_imp:` / `:heart:` / `:+1:` at a token start. Needs a letter
/// inside or be `:+1:` / `:-1:` (so `12:30:45` stays), and no word glued on the left (so
/// `a:b:c` and URLs stay).
fn strip_shortcodes(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    'outer: while i < cs.len() {
        if cs[i] == ':' && (i == 0 || cs[i - 1].is_whitespace() || "([{\"'".contains(cs[i - 1])) {
            let mut j = i + 1;
            while j < cs.len()
                && (cs[j].is_ascii_alphanumeric() || matches!(cs[j], '_' | '+' | '-'))
            {
                j += 1;
            }
            let inner = &cs[i + 1..j];
            if cs.get(j) == Some(&':')
                && inner.len() >= 2
                && (inner.iter().any(|ch| ch.is_ascii_alphabetic())
                    || matches!(inner, ['+' | '-', '1']))
                && cs.get(j + 1).is_none_or(|n| !n.is_alphanumeric())
            {
                out.push(' ');
                i = j + 1;
                continue 'outer;
            }
        }
        out.push(cs[i]);
        i += 1;
    }
    out
}

// ----------------------------------------------------------- emoticons --

/// `true` if the whole token is a text emoticon.
fn is_emoticon(t: &str) -> bool {
    let cs: Vec<char> = t.chars().collect();
    if cs.is_empty() {
        return false;
    }
    // Hearts: <3 <33 </3
    if let Some(rest) = t.strip_prefix("</").or_else(|| t.strip_prefix('<')) {
        if !rest.is_empty() && rest.chars().all(|c| c == '3') {
            return true;
        }
    }
    // Western: [>}]? eyes [:;=] nose? mouth+   (:) ;-) :D :P :'( >:) =3 :-*)
    {
        let mut k = 0;
        if matches!(cs[k], '>' | '}' | ']') && cs.len() > 2 {
            k += 1;
        }
        if matches!(cs.get(k), Some(':' | ';' | '=')) {
            k += 1;
            if matches!(cs.get(k), Some('-' | '\'' | '^' | 'o')) && cs.len() > k + 1 {
                k += 1;
            }
            let mouth = &cs[k..];
            if !mouth.is_empty()
                && mouth.iter().all(|c| {
                    matches!(c, ')' | '(' | ']' | '[' | 'D' | 'P' | 'p' | 'O' | 'o' | '3'
                        | '*' | '|' | '\\' | '/' | '$' | '@' | 'x' | 'X' | 'S' | 's' | 'c' | 'C')
                })
                // A mouth of letters must be one kind repeated (`:DDD`, not `:Sc`).
                && (mouth.iter().all(|c| !c.is_alphabetic())
                    || mouth.iter().all(|c| *c == mouth[0]))
            {
                return true;
            }
        }
    }
    // Reversed: (: (-:
    if matches!(t, "(:" | "(-:" | "(;" | "(-;") {
        return true;
    }
    // xD XD xP xd xDD (not "XP", "xd" only lowercase pair)
    if cs.len() >= 2 && matches!(cs[0], 'x' | 'X') {
        let rest = &cs[1..];
        if rest.iter().all(|c| *c == 'D') || (t == "xd") || (t == "xP" || t == "xp") {
            return true;
        }
    }
    // Eastern: ^_^ ^^ ^.^ -_- o_O O_o >_< >.< T_T ;_; x_x u_u *_* ^^;
    let eye = |c: char| {
        matches!(
            c,
            '^' | 'o'
                | 'O'
                | '0'
                | 'T'
                | 'x'
                | 'X'
                | 'u'
                | 'U'
                | ';'
                | '>'
                | '<'
                | '-'
                | '*'
                | '@'
                | '~'
                | '='
        )
    };
    let mut core: Vec<char> = cs.clone();
    if core.first() == Some(&'(') && core.last() == Some(&')') && core.len() > 2 {
        core = core[1..core.len() - 1].to_vec();
    }
    let eastern = |v: &[char]| match v {
        ['^', '^'] => true,
        [a, '_', b] => eye(*a) && eye(*b),
        [a, '.', b] => {
            matches!(a, '^' | 'o' | 'O' | '>' | '<') && matches!(b, '^' | 'o' | 'O' | '>' | '<')
        }
        _ => false,
    };
    if eastern(&core) || (core.last() == Some(&';') && eastern(&core[..core.len() - 1])) {
        return true;
    }
    matches!(t, "\\o/" | "\\O/")
}

/// Emoticon glued to the end of a word (`thanks:)`, `you<3`): only the
/// unambiguous paren / heart faces, and never after a digit.
fn strip_glued_suffix(t: &str) -> Option<&str> {
    for face in [":-)", ";-)", ":-(", ":)", ";)", ":(", ":')", ":D", "<3"] {
        if let Some(head) = t.strip_suffix(face) {
            if head.chars().last().is_some_and(|c| c.is_alphabetic()) {
                return Some(head);
            }
        }
    }
    None
}

/// Token pass: drops emoticon tokens (keeping trailing sentence
/// punctuation), glued emoticon suffixes, and leftover tokens made only of
/// face characters (`╯︵`, `┻━┻`, `^^`).
fn strip_tokens(s: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for raw in s.split_whitespace() {
        // Trailing sentence punctuation after a face: `:)!` `<3.`
        let body = raw.trim_end_matches([',', '.', '!', '?']);
        let tail = &raw[body.len()..];
        if !body.is_empty() && is_emoticon(body) {
            if !tail.is_empty() {
                words.push(tail.to_string());
            }
            continue;
        }
        if is_emoticon(raw) {
            continue;
        }
        if let Some(head) = strip_glued_suffix(body) {
            words.push(format!("{head}{tail}"));
            continue;
        }
        let no_alnum = !raw.chars().any(char::is_alphanumeric);
        if no_alnum
            && raw.chars().any(|c| !c.is_ascii() && is_face_char(c))
            && raw.chars().all(|c| is_face_char(c) || ",.!?".contains(c))
        {
            continue;
        }
        // Decorative tildes at a token's end: "you~" → "you", "cutie~!" →
        // "cutie!"; "~5 min" and "~/code" keep theirs.
        let w: String = {
            let cs: Vec<char> = raw.chars().collect();
            cs.iter()
                .enumerate()
                .filter(|(k, c)| {
                    **c != '~'
                        || !cs[k + 1..]
                            .iter()
                            .all(|n| matches!(n, '~' | '.' | ',' | '!' | '?'))
                })
                .map(|(_, c)| *c)
                .collect()
        };
        if !w.is_empty() {
            words.push(w);
        }
    }
    words.join(" ")
}

// ----------------------------------------------------------------- tidy --

/// Collapses whitespace, removes spaces before punctuation, merges the
/// punctuation pile-ups removal leaves behind (`you , .` → `you.`), and
/// drops punctuation left at the very start.
fn tidy(s: &str) -> String {
    let words: Vec<&str> = s.split_whitespace().collect();
    let joined = words.join(" ");
    let cs: Vec<char> = joined.chars().collect();
    let soft = |c: char| matches!(c, ',' | ';' | ':');
    let hard = |c: char| matches!(c, '.' | '!' | '?' | '…');
    let mut out: Vec<char> = Vec::with_capacity(cs.len());
    for (k, &c) in cs.iter().enumerate() {
        let next = cs.get(k + 1).copied();
        let is_punct = soft(c) || hard(c);
        // A pure-punctuation run ends at whitespace or end of text.
        let punct_ends_token = next.is_none_or(|n| n.is_whitespace() || soft(n) || hard(n));
        if is_punct && punct_ends_token {
            // Drop the space before it.
            if out.last() == Some(&' ') {
                out.pop();
            }
            match out.last().copied() {
                // Nothing before it: leading punctuation.
                None => continue,
                Some(p) if soft(c) && (soft(p) || hard(p)) => continue,
                Some(p) if hard(c) && soft(p) => {
                    out.pop();
                }
                // `.` after `!`/`?`: keep the stronger mark.
                Some(p) if c == '.' && matches!(p, '!' | '?') => continue,
                _ => {}
            }
        }
        if c == ' ' && out.is_empty() {
            continue;
        }
        out.push(c);
    }
    let mut out: String = out.into_iter().collect();
    // Leading punctuation glued to the first word (`, hi` became `,hi`).
    while out.starts_with(|c: char| soft(c) || c == '.' || c == '!' || c == '?') {
        out.remove(0);
        out = out.trim_start().to_string();
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::speech_text as sp;

    #[test]
    fn unicode_emoji_are_dropped() {
        assert_eq!(
            sp("Brain still booting! Don't go anywhere. 😈"),
            "Brain still booting! Don't go anywhere."
        );
        assert_eq!(sp("Ooh, what's in there? 👀"), "Ooh, what's in there?");
        assert_eq!(sp("Gotcha! 🗡️"), "Gotcha!");
        assert_eq!(sp("hi😈there"), "hi there");
        assert_eq!(sp("❤ you ✨"), "you");
        assert_eq!(sp("I'm yours 💜💜💜."), "I'm yours.");
    }

    #[test]
    fn zwj_skin_tone_flag_keycap_sequences() {
        assert_eq!(sp("family 👨‍👩‍👧‍👦 time"), "family time");
        assert_eq!(sp("wave 👋🏽 hi"), "wave hi");
        assert_eq!(sp("trans 🏳️‍⚧️ pride"), "trans pride");
        assert_eq!(sp("go 🇨🇦 go"), "go go");
        assert_eq!(sp("scotland 🏴󠁧󠁢󠁳󠁣󠁴󠁿 yes"), "scotland yes");
        assert_eq!(sp("pick 1️⃣ or #️⃣"), "pick or");
        assert_eq!(sp("heart ❤️ red"), "heart red");
        assert_eq!(sp("woman facepalming 🤦‍♀️."), "woman facepalming.");
    }

    #[test]
    fn text_default_symbols_stay_without_fe0f() {
        assert_eq!(sp("Lillith © 2026"), "Lillith © 2026");
        assert_eq!(sp("copyright ©️ sign"), "copyright sign");
    }

    #[test]
    fn text_emoticons_dropped_as_tokens() {
        for e in [
            ":)", ";)", ":D", ":P", ":p", "xD", "XD", "<3", "<333", "</3", ";-)", ":-*", ":*",
            "^_^", "^^", "o_O", "O_o", "-_-", ">_<", ">.<", "T_T", ";_;", ":(", ":'(", ">:)", ":3",
            "=)", ":-D", ":)))", "(:", ":O", ":|", ":/", "\\o/", "(^_^)",
        ] {
            assert_eq!(sp(&format!("hey {e} you")), "hey you", "{e}");
        }
        assert_eq!(sp("Come here :) now"), "Come here now");
        assert_eq!(sp("miss you <3"), "miss you");
        assert_eq!(sp("gotcha ;-)!"), "gotcha!");
        assert_eq!(sp("thanks:)"), "thanks");
        assert_eq!(sp("love you<3"), "love you");
    }

    #[test]
    fn shortcodes_and_kaomoji() {
        assert_eq!(sp("naughty :smiling_imp: boy"), "naughty boy");
        assert_eq!(sp("yes :+1:"), "yes");
        assert_eq!(sp("happy (^▽^) day"), "happy day");
        assert_eq!(sp("hmm ( ͡° ͜ʖ ͡°) really"), "hmm really");
        assert_eq!(sp("whatever ¯\\_(ツ)_/¯"), "whatever");
        assert_eq!(sp("rage (╯°□°）╯︵ ┻━┻ done"), "rage done");
        assert_eq!(sp("bear ʕ•ᴥ•ʔ hug"), "bear hug");
    }

    #[test]
    fn roleplay_actions_dropped() {
        assert_eq!(sp("*winks* Come here, darling."), "Come here, darling.");
        assert_eq!(sp("Mmm *bites lip* you're mine."), "Mmm you're mine.");
        assert_eq!(sp("Finally. *wraps my tail around you*"), "Finally.");
        assert_eq!(
            sp("there you are~ You kept me waiting~"),
            "there you are You kept me waiting"
        );
    }

    #[test]
    fn nothing_speakable_means_empty() {
        assert_eq!(sp("😈"), "");
        assert_eq!(sp("😈💜 :) <3"), "");
        assert_eq!(sp("*winks*"), "");
        assert_eq!(sp("  ...  "), "");
        assert_eq!(sp(""), "");
    }

    #[test]
    fn leftover_punctuation_collapses() {
        assert_eq!(sp("You're mine 😈."), "You're mine.");
        assert_eq!(sp("Oh 😈, darling"), "Oh, darling");
        assert_eq!(sp("😈, hi"), "hi");
        assert_eq!(sp("wow :) , ok"), "wow, ok");
        assert_eq!(sp("tease 😈 !"), "tease!");
    }

    #[test]
    fn normal_text_is_unchanged() {
        for t in [
            "Hello, darling. How was your day?",
            "Don't you dare close that terminal; I'm not done with you!",
            "Wait... what?!",
            "Meet me at 12:30, or 3:45pm at the latest.",
            "Open https://example.com/a_b?x=1 and http://localhost:8080/ now.",
            "The ratio is 16:9 and the score was 3:2.",
            "Run ls -lhS, then cd ~/code.",
            "It costs $5 (about 30%) - see note (2).",
            "2 * 3 * 4 = 24, and a*b*c is fine.",
            "Note: this is the Plan D: approach.",
            "Windows XP was old.",
            "Use x) as a list marker, or option (b).",
            "That's about ~5 minutes.",
            "Ko's \"special\" request.",
            "Lillith © 2026",
            "Привет, дорогой! 日本語もOK。",
        ] {
            assert_eq!(sp(t), t, "changed: {t}");
        }
    }

    #[test]
    fn markdown_bold_keeps_words() {
        assert_eq!(sp("You are **mine**."), "You are mine.");
    }
}
