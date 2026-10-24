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

/// Her line after a "forget ..." command was carried out.
pub fn forget_line(cmd: &lillith_memory::ForgetCmd) -> String {
    match cmd {
        lillith_memory::ForgetCmd::All => {
            "Done. Wiped clean, every last memory of you. Start fresh with me?".into()
        }
        lillith_memory::ForgetCmd::Topic(t) => {
            format!("Gone. I don't remember anything about {t} now.")
        }
        lillith_memory::ForgetCmd::LastSecs(s) => {
            let m = (*s / 60).max(1);
            format!(
                "Forgotten. The last {m} minute{} never happened.",
                if m == 1 { "" } else { "s" }
            )
        }
    }
}

#[cfg(test)]
mod persona_prompt_tests {
    use super::*;

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
