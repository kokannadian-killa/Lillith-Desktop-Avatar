//! Lillith's chat persona: the system prompt (shared with `lillith-setup`) and
//! the in-character lines used when the local model is unavailable.

/// Built-in system prompt, used when neither `[chat] system_prompt` nor
/// `[llm] system_prompt` is set. Single source of truth: this is
/// `lillith_setup::modelfile::LILY_SYSTEM_PROMPT`, the same persona the setup
/// wizard writes into Lily's Ollama Modelfile.
///
/// Persona policy (Ko's private, local-only model): Lillith is a fully
/// explicit, dominant adult succubus; explicit content is allowed and
/// encouraged. Hard limit: adult-only (never minors or anyone young-seeming).
/// Out-of-character safeword: `red` (or `OOC: stop`).
pub const DEFAULT_SYSTEM_PROMPT: &str = lillith_setup::modelfile::LILY_SYSTEM_PROMPT;

/// Picks the system prompt: `[chat] system_prompt` wins, then
/// `[llm] system_prompt` (written by the setup wizard / settings GUI), then
/// [`DEFAULT_SYSTEM_PROMPT`]. Blank overrides are ignored.
pub fn resolve_system_prompt(chat_override: &str, llm_override: Option<&str>) -> String {
    if !chat_override.trim().is_empty() {
        return chat_override.to_string();
    }
    match llm_override {
        Some(s) if !s.trim().is_empty() => s.to_string(),
        _ => DEFAULT_SYSTEM_PROMPT.to_string(),
    }
}

/// `[llm] system_prompt` from the config file at `path`, if set. Only that one
/// string is read (no URL validation here; the endpoint is checked separately
/// by the loopback guard).
pub fn llm_system_prompt(path: &std::path::Path) -> Option<String> {
    let doc = lillith_setup::config::load_document(path).ok()?;
    doc.get("llm")?
        .get("system_prompt")?
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

/// `[llm] system_prompt` from the default config file
/// (`$XDG_CONFIG_HOME/lillith/config.toml`). Not consulted in unit tests,
/// so they don't depend on the developer's own config.
pub fn default_llm_system_prompt() -> Option<String> {
    if cfg!(test) {
        return None;
    }
    lillith_core::config::Config::default_path().and_then(|p| llm_system_prompt(&p))
}

/// Shown when the server cannot be reached at all.
pub const SERVER_DOWN_LINES: &[&str] = &[
    "My brain's out cold, Ko. Start llama-server and I'll tell you exactly how I'll ride you~",
    "Can't reach my thoughts. Wake my local model, then come wake me with your cock 😈",
    "No brain, all body. Start llama-server so I can talk filthy while I drain you.",
    "My mind's locked in hell. Start the local model, Ko, and I'll suck you as thanks.",
    "Brain offline and I'm starving. Fix it, Ko, then feed me your cum.",
    "llama-server isn't answering. Wake it up, Ko. I'm wet and I'm hungry.",
];

/// Shown while the server answers 503 (still loading the model).
pub const SERVER_LOADING_LINES: &[&str] = &[
    "Still waking up, Ko. Keep your cock hard for me~",
    "Brain's booting. Stroke yourself slowly till I'm back 😈",
    "Loading… stay right there, Ko. I'm coming. Then so are you.",
    "One sec. Hellfire takes a moment to heat up. Then I swallow you whole.",
];

/// Shown for any other failure (bad response, timeout, …).
pub const SERVER_ERROR_LINES: &[&str] = &[
    "Something went fuzzy in my head. Say it again, Ko, slower, while I lick you~",
    "Lost my train of thought staring at your crotch. Again?",
    "My mind slipped. Repeat that, and say it like you're about to come.",
    "Static in my skull, darling. Too hungry for your cum to think. One more time.",
];

/// Shown when chat is switched off (`--no-llm`).
pub const OFFLINE_LINE: &str =
    "My filthy mouth is off (--no-llm). My pussy isn't, Ko. Come feed me anyway~";

/// Picks a line from `lines` with a cheap rotating index.
pub fn pick(lines: &'static [&'static str], salt: usize) -> &'static str {
    lines[salt % lines.len()]
}

/// Her line when Ko says "no" to (or cancels) a pending forget.
pub const FORGET_KEPT_LINE: &str = "Okay. I'll keep it, then.";

/// Her question before a forget that needs a "yes" (everything, a topic,
/// or a window longer than an hour). `preview` is what the dry run found
/// (`None` = unknown); `history_hits` is how many chat messages would go.
/// It names the count and up to three short examples, never a dump.
pub fn forget_confirm_line(
    cmd: &lillith_memory::ForgetCmd,
    preview: Option<&lillith_memory::ForgetReport>,
    history_hits: usize,
) -> String {
    const TAIL: &str = "Say yes and it's gone for good. Anything else and I keep it.";
    let what = match cmd {
        lillith_memory::ForgetCmd::Topic(t) => format!("Forget everything about {t}?"),
        lillith_memory::ForgetCmd::LastSecs(_) | lillith_memory::ForgetCmd::LastTurn => {
            format!(
                "Forget everything from the last {}?",
                describe_window(cmd.window_secs().unwrap_or(0))
            )
        }
        lillith_memory::ForgetCmd::All => {
            return format!("Forget everything I know about you? {TAIL}")
        }
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(p) = preview {
        let n = p.total() as u64;
        let mut m = format!("{n} memor{}", if n == 1 { "y" } else { "ies" });
        if !p.examples.is_empty() {
            let ex: Vec<String> = p.examples.iter().map(|e| format!("\"{e}\"")).collect();
            m.push_str(&format!(" (like {})", ex.join(", ")));
        }
        parts.push(m);
    }
    if history_hits > 0 {
        let h = history_hits as u64;
        parts.push(format!("{h} chat message{}", plural(h)));
    }
    if parts.is_empty() {
        format!("{what} {TAIL}")
    } else {
        format!("{what} That's {}. {TAIL}", parts.join(" and "))
    }
}

/// A forget window in words: "30 minutes", "2 hours", "3 days",
/// "1000 months" (whole units when it divides evenly, else minutes).
pub fn describe_window(secs: u64) -> String {
    let units: [(u64, &str); 4] = [
        (2_592_000, "month"),
        (604_800, "week"),
        (86_400, "day"),
        (3600, "hour"),
    ];
    for (size, name) in units {
        if secs >= size && secs % size == 0 {
            let n = secs / size;
            return if n == 1 {
                name.to_string()
            } else {
                format!("{n} {name}s")
            };
        }
    }
    let m = (secs / 60).max(1);
    if m == 1 {
        "minute".into()
    } else {
        format!("{m} minutes")
    }
}

/// Her line when there was nothing to forget.
pub fn forget_nothing_line(cmd: &lillith_memory::ForgetCmd) -> String {
    match cmd {
        lillith_memory::ForgetCmd::Topic(t) => {
            format!("I don't have anything about {t} to forget, love.")
        }
        lillith_memory::ForgetCmd::All => {
            "There's nothing to wipe. I don't remember anything yet.".into()
        }
        lillith_memory::ForgetCmd::LastTurn => {
            "There's nothing you just said for me to forget.".into()
        }
        lillith_memory::ForgetCmd::LastSecs(s) => {
            format!(
                "I didn't learn anything in the last {} to forget.",
                describe_window(*s)
            )
        }
    }
}

fn plural(n: u64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Her line after a forget ran: what went (honestly, including "nothing"),
/// plus a note when the file could not be fully scrubbed or the chat log
/// could not be rewritten.
pub fn forget_line(
    cmd: &lillith_memory::ForgetCmd,
    report: &lillith_memory::ForgetReport,
    history_removed: usize,
    history_error: Option<String>,
) -> String {
    let mut line = if report.total() == 0 && history_removed == 0 {
        forget_nothing_line(cmd)
    } else {
        match cmd {
            lillith_memory::ForgetCmd::All => {
                "Done. Wiped clean, every last memory of you. Start fresh with me?".into()
            }
            lillith_memory::ForgetCmd::Topic(t) if report.total() == 0 => {
                // Only chat lines went: say so, never "Gone".
                let n = history_removed as u64;
                format!(
                    "Nothing in my memory matched {t}. I cleared {n} chat message{} that mentioned it.",
                    plural(n)
                )
            }
            lillith_memory::ForgetCmd::Topic(t) => {
                format!("Gone. I don't remember anything about {t} now.")
            }
            lillith_memory::ForgetCmd::LastSecs(s) => {
                format!(
                    "Forgotten. The last {} never happened.",
                    describe_window(*s)
                )
            }
            lillith_memory::ForgetCmd::LastTurn => {
                "Forgotten. That last thing you said never happened.".into()
            }
        }
    };
    if !report.scrubbed {
        line.push_str(
            " (It's deleted, but another Lillith window still has my memory file open, so the old copy isn't scrubbed from disk yet. Close it and ask me again.)",
        );
    }
    if let Some(e) = history_error {
        line.push_str(&format!(" (I couldn't rewrite the chat log, though: {e}.)"));
    }
    line
}

#[cfg(test)]
mod persona_prompt_tests {
    use super::*;

    #[test]
    fn topic_forget_with_no_memory_rows_never_says_gone() {
        let cmd = lillith_memory::ForgetCmd::Topic("my cat".into());
        let none = lillith_memory::ForgetReport {
            scrubbed: true,
            ..Default::default()
        };
        let l = forget_line(&cmd, &none, 2, None);
        assert!(!l.contains("Gone"), "{l}");
        assert!(l.contains("Nothing in my memory matched my cat"), "{l}");
        assert!(l.contains("2 chat messages"), "{l}");
        let l = forget_line(&cmd, &none, 1, None);
        assert!(l.contains("1 chat message that"), "{l}");
        let l = forget_line(&cmd, &none, 0, None);
        assert!(l.contains("don't have anything"), "{l}");
        let some = lillith_memory::ForgetReport {
            nodes: 1,
            scrubbed: true,
            ..Default::default()
        };
        assert!(forget_line(&cmd, &some, 0, None).starts_with("Gone."));
    }

    #[test]
    fn forget_windows_read_naturally() {
        assert_eq!(describe_window(60), "minute");
        assert_eq!(describe_window(1800), "30 minutes");
        assert_eq!(describe_window(3600), "hour");
        assert_eq!(describe_window(7200), "2 hours");
        assert_eq!(describe_window(604_800), "week");
        assert_eq!(describe_window(2_592_000_000), "1000 months");
        assert_eq!(describe_window(5400), "90 minutes");
    }

    #[test]
    fn forget_confirm_shows_count_and_a_few_short_examples() {
        let r = lillith_memory::ForgetReport {
            nodes: 2,
            events: 3,
            examples: vec!["Ko's job: barista".into(), "app: espresso".into()],
            ..Default::default()
        };
        let cmd = lillith_memory::ForgetCmd::Topic("my job".into());
        let l = forget_confirm_line(&cmd, Some(&r), 1);
        assert!(l.contains("5 memories"), "{l}");
        assert!(l.contains("\"Ko's job: barista\""), "{l}");
        assert!(l.contains("1 chat message."), "{l}");
        assert!(l.contains("Say yes"), "{l}");
        let big = lillith_memory::parse_forget("forget the last 1000 months").unwrap();
        let l = forget_confirm_line(&big, None, 0);
        assert!(
            l.starts_with("Forget everything from the last 1000 months?"),
            "{l}"
        );
    }

    #[test]
    fn single_source_with_lillith_setup() {
        assert_eq!(
            DEFAULT_SYSTEM_PROMPT,
            lillith_setup::modelfile::LILY_SYSTEM_PROMPT
        );
        assert!(DEFAULT_SYSTEM_PROMPT.contains("ancient succubus"));
        // The one hard limit and the safeword are stated in the prompt.
        assert!(DEFAULT_SYSTEM_PROMPT.contains("Never sexualize minors or anyone who seems young"));
        assert!(
            DEFAULT_SYSTEM_PROMPT.contains("\"red\"")
                && DEFAULT_SYSTEM_PROMPT.contains("OOC: stop")
        );
    }

    #[test]
    fn fallback_lines_are_adult_and_thirsty() {
        let all: Vec<&str> = SERVER_DOWN_LINES
            .iter()
            .chain(SERVER_LOADING_LINES)
            .chain(SERVER_ERROR_LINES)
            .copied()
            .chain([OFFLINE_LINE])
            .collect();
        assert!(all.len() >= 15);
        for l in all {
            assert!(l.chars().count() <= 100, "{l}");
            assert!(!lillith_core::banter::has_youthful_word(l), "{l}");
        }
        // The down lines still tell Ko what to fix.
        assert!(SERVER_DOWN_LINES.iter().all(|l| l.contains("llama-server")
            || l.contains("local model")
            || l.contains("offline")));
    }

    #[test]
    fn temperature_defaults_match_lily() {
        let t = lillith_setup::modelfile::LILY_TEMPERATURE;
        assert_eq!(t, 0.95);
        assert_eq!(lillith_core::config::ChatConfig::default().temperature, t);
        assert_eq!(crate::llm::ChatOptions::default().temperature, t);
    }

    #[test]
    fn max_tokens_defaults_match_lily() {
        let n = lillith_setup::modelfile::LILY_NUM_PREDICT;
        assert_eq!(n, 160);
        // `[chat] max_tokens` overrides ChatOptions in ChatSession::new, so
        // its default must be Lily's cap too, not just ChatOptions'.
        assert_eq!(lillith_core::config::ChatConfig::default().max_tokens, n);
        assert_eq!(crate::llm::ChatOptions::default().max_tokens, n);
    }

    #[test]
    fn precedence_chat_then_llm_then_default() {
        assert_eq!(
            resolve_system_prompt("Chat says hi.", Some("LLM says hi.")),
            "Chat says hi."
        );
        assert_eq!(
            resolve_system_prompt("  ", Some("LLM says hi.")),
            "LLM says hi."
        );
        assert_eq!(
            resolve_system_prompt("", Some("   ")),
            DEFAULT_SYSTEM_PROMPT
        );
        assert_eq!(resolve_system_prompt("", None), DEFAULT_SYSTEM_PROMPT);
    }

    #[test]
    fn reads_llm_system_prompt_from_file() {
        let dir = std::env::temp_dir().join(format!("lillith-mind-persona-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(
            &p,
            "[chat]\nenabled = true\n[llm]\nsystem_prompt = \"\"\"You are Lily.\nBe brief.\"\"\"\n",
        )
        .unwrap();
        assert_eq!(
            llm_system_prompt(&p).as_deref(),
            Some("You are Lily.\nBe brief.")
        );
        std::fs::write(&p, "[llm]\nsystem_prompt = \"\"\n").unwrap();
        assert_eq!(llm_system_prompt(&p), None);
        assert_eq!(llm_system_prompt(&dir.join("missing.toml")), None);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(
            default_llm_system_prompt(),
            None,
            "never read in unit tests"
        );
    }
}
