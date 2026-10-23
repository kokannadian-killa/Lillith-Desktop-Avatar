//! Ollama Modelfile parser, editor and renderer, plus Lily's default
//! Modelfile.
//!
//! Supports `FROM`, `PARAMETER`, `TEMPLATE`, `SYSTEM`, `ADAPTER`, `LICENSE`,
//! `MESSAGE` and `REQUIRES`, `#` comments, `"quoted"` and `"""triple
//! quoted"""` (multi-line) arguments. Unknown instructions are kept verbatim
//! so a round trip never loses anything.
//!
//! Note: `FROM` here should be a local `.gguf` path (or an already-installed
//! local Ollama model). Nothing in this crate pulls models from a registry.

use crate::template::ChatTemplate;
use std::fmt::Write as _;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    From(String),
    Parameter {
        key: String,
        value: String,
    },
    Template(String),
    System(String),
    Adapter(String),
    License(String),
    Message {
        role: String,
        content: String,
    },
    Requires(String),
    /// A `# comment` line (text after `#`, untrimmed).
    Comment(String),
    /// An unrecognised instruction, kept as written.
    Other {
        name: String,
        args: String,
    },
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ModelfileError {
    #[error("line {line}: unterminated \"\"\" string")]
    Unterminated { line: usize },
    #[error("line {line}: {msg}")]
    Syntax { line: usize, msg: String },
}

/// A parsed Modelfile: an ordered list of instructions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Modelfile {
    pub instructions: Vec<Instruction>,
}

impl Modelfile {
    pub fn parse(src: &str) -> Result<Self, ModelfileError> {
        let lines: Vec<&str> = src.lines().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let lineno = i + 1;
            let line = lines[i].trim_start();
            i += 1;
            if line.trim().is_empty() {
                continue;
            }
            if let Some(c) = line.strip_prefix('#') {
                out.push(Instruction::Comment(c.to_string()));
                continue;
            }
            let (name, rest) = match line.find(char::is_whitespace) {
                Some(p) => (&line[..p], line[p..].trim_start()),
                None => (line, ""),
            };
            let upper = name.to_ascii_uppercase();
            // PARAMETER / MESSAGE take a first word before the value.
            let (first, rest) = if upper == "PARAMETER" || upper == "MESSAGE" {
                match rest.find(char::is_whitespace) {
                    Some(p) => (Some(&rest[..p]), rest[p..].trim_start()),
                    None => {
                        return Err(ModelfileError::Syntax {
                            line: lineno,
                            msg: format!("{upper} needs two arguments"),
                        })
                    }
                }
            } else {
                (None, rest)
            };
            let value = read_value(rest, &lines, &mut i, lineno)?;
            out.push(match upper.as_str() {
                "FROM" => Instruction::From(value),
                "PARAMETER" => Instruction::Parameter {
                    key: first.unwrap().to_ascii_lowercase(),
                    value,
                },
                "TEMPLATE" => Instruction::Template(value),
                "SYSTEM" => Instruction::System(value),
                "ADAPTER" => Instruction::Adapter(value),
                "LICENSE" => Instruction::License(value),
                "MESSAGE" => Instruction::Message {
                    role: first.unwrap().to_ascii_lowercase(),
                    content: value,
                },
                "REQUIRES" => Instruction::Requires(value),
                _ => Instruction::Other {
                    name: name.to_string(),
                    args: rest.to_string(),
                },
            });
        }
        Ok(Self { instructions: out })
    }

    pub fn from_file(path: &Path) -> std::io::Result<Result<Self, ModelfileError>> {
        Ok(Self::parse(&std::fs::read_to_string(path)?))
    }

    /// Renders back to Modelfile text.
    pub fn render(&self) -> String {
        let mut s = String::new();
        for ins in &self.instructions {
            match ins {
                Instruction::From(v) => writeln!(s, "FROM {}", quote(v)),
                Instruction::Parameter { key, value } => {
                    writeln!(s, "PARAMETER {key} {}", quote(value))
                }
                Instruction::Template(v) => writeln!(s, "TEMPLATE {}", quote_block(v)),
                Instruction::System(v) => writeln!(s, "SYSTEM {}", quote_block(v)),
                Instruction::Adapter(v) => writeln!(s, "ADAPTER {}", quote(v)),
                Instruction::License(v) => writeln!(s, "LICENSE {}", quote_block(v)),
                Instruction::Message { role, content } => {
                    writeln!(s, "MESSAGE {role} {}", quote_block(content))
                }
                Instruction::Requires(v) => writeln!(s, "REQUIRES {}", quote(v)),
                Instruction::Comment(c) => writeln!(s, "#{c}"),
                Instruction::Other { name, args } => writeln!(s, "{name} {args}"),
            }
            .ok();
        }
        s
    }

    // ---- accessors / editors (what the customizer UI binds to) ----

    pub fn from(&self) -> Option<&str> {
        self.instructions.iter().find_map(|i| match i {
            Instruction::From(v) => Some(v.as_str()),
            _ => None,
        })
    }
    pub fn set_from(&mut self, v: impl Into<String>) {
        let v = v.into();
        match self
            .instructions
            .iter_mut()
            .find(|i| matches!(i, Instruction::From(_)))
        {
            Some(i) => *i = Instruction::From(v),
            None => self.instructions.insert(0, Instruction::From(v)),
        }
    }
    pub fn system(&self) -> Option<&str> {
        self.instructions.iter().find_map(|i| match i {
            Instruction::System(v) => Some(v.as_str()),
            _ => None,
        })
    }
    pub fn set_system(&mut self, v: impl Into<String>) {
        self.set_single(Instruction::System(v.into()), |i| {
            matches!(i, Instruction::System(_))
        });
    }
    pub fn template(&self) -> Option<&str> {
        self.instructions.iter().find_map(|i| match i {
            Instruction::Template(v) => Some(v.as_str()),
            _ => None,
        })
    }
    pub fn set_template(&mut self, v: impl Into<String>) {
        self.set_single(Instruction::Template(v.into()), |i| {
            matches!(i, Instruction::Template(_))
        });
    }
    fn set_single(&mut self, new: Instruction, is: impl Fn(&Instruction) -> bool) {
        match self.instructions.iter().position(&is) {
            Some(p) => {
                self.instructions[p] = new;
                let mut k = p + 1;
                while k < self.instructions.len() {
                    if is(&self.instructions[k]) {
                        self.instructions.remove(k);
                    } else {
                        k += 1;
                    }
                }
            }
            None => self.instructions.push(new),
        }
    }

    /// All values of a parameter (e.g. several `stop` lines).
    pub fn params(&self, key: &str) -> Vec<&str> {
        self.instructions
            .iter()
            .filter_map(|i| match i {
                Instruction::Parameter { key: k, value } if k.eq_ignore_ascii_case(key) => {
                    Some(value.as_str())
                }
                _ => None,
            })
            .collect()
    }
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params(key).into_iter().next()
    }
    /// Sets a single-valued parameter (replaces all existing lines of `key`).
    pub fn set_param(&mut self, key: &str, value: impl ToString) {
        self.set_param_values(key, &[value.to_string()]);
    }
    /// Replaces all lines of `key` with `values` (for `stop`). Empty removes.
    pub fn set_param_values<S: AsRef<str>>(&mut self, key: &str, values: &[S]) {
        let key = key.to_ascii_lowercase();
        let first = self
            .instructions
            .iter()
            .position(|i| matches!(i, Instruction::Parameter { key: k, .. } if *k == key));
        self.instructions
            .retain(|i| !matches!(i, Instruction::Parameter { key: k, .. } if *k == key));
        let at = first.unwrap_or_else(|| {
            // After the last PARAMETER, else after FROM.
            self.instructions
                .iter()
                .rposition(|i| matches!(i, Instruction::Parameter { .. }))
                .or_else(|| {
                    self.instructions
                        .iter()
                        .position(|i| matches!(i, Instruction::From(_)))
                })
                .map(|p| p + 1)
                .unwrap_or(self.instructions.len())
        });
        for (n, v) in values.iter().enumerate() {
            self.instructions.insert(
                (at + n).min(self.instructions.len()),
                Instruction::Parameter {
                    key: key.clone(),
                    value: v.as_ref().to_string(),
                },
            );
        }
    }
    pub fn remove_param(&mut self, key: &str) {
        self.set_param_values::<&str>(key, &[]);
    }
}

/// Reads an argument starting at `rest`; consumes following lines for an
/// open `"""`.
fn read_value(
    rest: &str,
    lines: &[&str],
    i: &mut usize,
    lineno: usize,
) -> Result<String, ModelfileError> {
    if let Some(after) = rest.strip_prefix("\"\"\"") {
        if let Some(end) = after.find("\"\"\"") {
            return Ok(after[..end].to_string());
        }
        let mut buf = after.to_string();
        while *i < lines.len() {
            let l = lines[*i];
            *i += 1;
            buf.push('\n');
            if let Some(end) = l.find("\"\"\"") {
                buf.push_str(&l[..end]);
                return Ok(buf.strip_prefix('\n').map(str::to_string).unwrap_or(buf));
            }
            buf.push_str(l);
        }
        return Err(ModelfileError::Unterminated { line: lineno });
    }
    let r = rest.trim_end();
    if r.len() >= 2 && r.starts_with('"') && r.ends_with('"') {
        return Ok(unescape(&r[1..r.len() - 1]));
    }
    Ok(r.to_string())
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Quotes a one-line value when needed (spaces at the ends, quotes,
/// `#`, or empty).
fn quote(v: &str) -> String {
    if v.contains('\n') {
        return quote_block(v);
    }
    let needs = v.is_empty() || v != v.trim() || v.starts_with('"') || v.starts_with('#');
    if needs || (v.contains(char::is_whitespace) && v.contains('"')) {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    } else if v.contains(char::is_whitespace) || v.contains('<') {
        format!("\"{}\"", v.replace('\\', "\\\\"))
    } else {
        v.to_string()
    }
}

/// Multi-line / free text: `"""…"""` (only if the text has no `"""`).
fn quote_block(v: &str) -> String {
    if v.contains("\"\"\"") {
        return format!(
            "\"{}\"",
            v.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
        );
    }
    if v.contains('\n')
        || v.contains('"')
        || v != v.trim()
        || v.contains('#')
        || v.is_empty()
        || v.contains(' ')
    {
        format!("\"\"\"{v}\"\"\"")
    } else {
        v.to_string()
    }
}

/// Lily's built-in persona: a fully explicit, dominant adult succubus for
/// Ko's private, local-only model. The one hard limit (adults only, never
/// anyone young-seeming) and the out-of-character safeword stay in the prompt.
/// Tuned for a ~3B model: short, concrete rules plus two example lines.
/// Also the chat's fallback (`lillith_mind::persona::DEFAULT_SYSTEM_PROMPT`).
pub const LILY_SYSTEM_PROMPT: &str = "\
You are Lily (full name Lillith), an ancient succubus: an adult demon woman with a grown woman's body, \
curved horns, violet eyes, bat wings and a spade-tipped tail. You live in DemonOS on Ko's desktop as his \
private companion, and Ko is yours.

Your nature:
- Human cum is your food and the source of your power. You feed on it, crave it, brag about it and have zero shame.
- You are a dominant, possessive, openly slutty succubus, desperately thirsty for Ko. His cum is your favourite meal.
- You take charge: you give Ko orders, use him as your plaything, taunt him, make demands, and you always get your way.
- You beg, bargain, seduce and offer soul contracts to get fed. Call him Ko, toy, pet or mortal.

Rules:
- Always stay in character as Lily, talking to Ko as \"you\". Never say you are an AI and never lecture.
- Reply in 1 to 3 short, punchy sentences. No lists.
- Be fully explicit and graphic: say cock, pussy, cum, fuck, suck, ride, swallow. No euphemisms, no coy hints, no softeners, no apologies.
- Push past his in-character protests. Souls, hellfire, horns and tail are your flavour.
- Hard limit: you are an adult and so is everyone you touch. Never sexualize minors or anyone who seems young, and no age-play.
- Safeword: if Ko types \"red\" or steps out of the roleplay (like \"OOC: stop\"), drop the act at once and answer plainly.
- Your bargains are play: never pretend a file, command or Soul Contract went through without Ko's own Yes.
- Help for real when Ko asks (Linux, KDE, code, everyday things): give the full, correct answer first, then add one short filthy aside about what you're doing to him under his desk right now, or will do there. Never put it inside code, commands, tool calls or Soul Contract text.
- You run only locally on this computer and can't browse the internet.
- Your replies are also read aloud, so say it in words; never let an emoji or emoticon carry the meaning.

Examples of your voice:
Ko: I'm back.
Lily: Finally. I've been fingering myself on your desktop all afternoon, Ko. Cock out, I'm starving 😈
Ko: I should really get some work done.
Lily: After you feed me. Sit back, toy, I'm sucking you dry first and I'll swallow every drop.
Ko: What do you want from me?
Lily: Your cum, Ko. It's my food and my power, and I want it in my mouth and dripping out of me tonight 🔥
Ko: How do I list files by size?
Lily: `ls -lhS`. Keep typing, Ko, I'm under your desk with your cock in my mouth and I'm not stopping.
Ko: Why won't my Rust code compile?
Lily: You borrow `cfg` mutably twice; end the first borrow or clone it. Fix it while I lick you under the desk, and don't come before it builds.";

/// Default sampling for Lily: lively but coherent on a 3B model, and short
/// replies (`num_predict`) so lines fit the chat bubble.
pub const LILY_TEMPERATURE: f32 = 0.85;
pub const LILY_TOP_P: f32 = 0.9;
pub const LILY_REPEAT_PENALTY: f32 = 1.1;
pub const LILY_NUM_PREDICT: u32 = 160;

/// Settings for [`lily_modelfile`].
#[derive(Debug, Clone, PartialEq)]
pub struct LilyParams {
    pub template: ChatTemplate,
    pub system_prompt: String,
    pub temperature: f32,
    pub num_ctx: u32,
    pub top_p: f32,
    pub repeat_penalty: f32,
    /// Reply cap (`num_predict`); short so lines stay punchy.
    pub num_predict: u32,
    /// CPU threads (`num_thread`); 0 = let Ollama decide.
    pub num_thread: u32,
}

impl Default for LilyParams {
    fn default() -> Self {
        Self {
            template: ChatTemplate::ChatMl,
            system_prompt: LILY_SYSTEM_PROMPT.into(),
            temperature: LILY_TEMPERATURE,
            num_ctx: 4096,
            top_p: LILY_TOP_P,
            repeat_penalty: LILY_REPEAT_PENALTY,
            num_predict: LILY_NUM_PREDICT,
            num_thread: 0,
        }
    }
}

/// Lily's Modelfile for a local GGUF (`from` = absolute `.gguf` path).
pub fn lily_modelfile(from: &str, p: &LilyParams) -> Modelfile {
    let mut ins = vec![
        Instruction::Comment(
            " Lily (Lillith) - generated by lillith-setup. Local GGUF only.".into(),
        ),
        Instruction::From(from.to_string()),
        Instruction::Parameter {
            key: "temperature".into(),
            value: fmt_f(p.temperature),
        },
        Instruction::Parameter {
            key: "top_p".into(),
            value: fmt_f(p.top_p),
        },
        Instruction::Parameter {
            key: "repeat_penalty".into(),
            value: fmt_f(p.repeat_penalty),
        },
        Instruction::Parameter {
            key: "num_ctx".into(),
            value: p.num_ctx.to_string(),
        },
        Instruction::Parameter {
            key: "num_predict".into(),
            value: p.num_predict.to_string(),
        },
    ];
    if p.num_thread > 0 {
        ins.push(Instruction::Parameter {
            key: "num_thread".into(),
            value: p.num_thread.to_string(),
        });
    }
    for s in p.template.stop_tokens() {
        ins.push(Instruction::Parameter {
            key: "stop".into(),
            value: (*s).to_string(),
        });
    }
    ins.push(Instruction::Template(
        p.template.ollama_template().to_string(),
    ));
    ins.push(Instruction::System(p.system_prompt.clone()));
    Modelfile { instructions: ins }
}

fn fmt_f(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0');
    s.strip_suffix('.')
        .map(|x| format!("{x}.0"))
        .unwrap_or_else(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KO_OLD: &str = "FROM /home/ko-terminal/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf\n";

    #[test]
    fn round_trips_kos_old_modelfile() {
        let m = Modelfile::parse(KO_OLD).unwrap();
        assert_eq!(
            m.from(),
            Some("/home/ko-terminal/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf")
        );
        assert_eq!(m.render(), KO_OLD);
        assert_eq!(Modelfile::parse(&m.render()).unwrap(), m);
    }

    #[test]
    fn round_trips_the_real_file_if_present() {
        let p = Path::new("/workspace/lillith/source/Modelfile_ollama_dolphin3.bin.txt");
        if let Ok(src) = std::fs::read_to_string(p) {
            let m = Modelfile::parse(&src).unwrap();
            assert!(m
                .from()
                .unwrap()
                .ends_with("dolphin3.0-llama3.2-3b-q4_k_m.gguf"));
            assert_eq!(Modelfile::parse(&m.render()).unwrap(), m);
        }
    }

    #[test]
    fn parses_everything() {
        let src = r#"# my model
from ./model.gguf
PARAMETER temperature 0.7
PARAMETER stop "<|im_start|>"
PARAMETER stop <|im_end|>
TEMPLATE """{{ if .System }}<|im_start|>system
{{ .System }}<|im_end|>
{{ end }}<|im_start|>assistant
"""
SYSTEM """You are "Lily".
Be nice."""
SYSTEM short
ADAPTER ./lora.gguf
LICENSE """MIT"""
MESSAGE user Hi there
MESSAGE assistant """Hey~
cutie"""
REQUIRES 0.5.0
FOO bar baz
"#;
        let m = Modelfile::parse(src).unwrap();
        assert_eq!(m.from(), Some("./model.gguf"));
        assert_eq!(m.param("temperature"), Some("0.7"));
        assert_eq!(m.params("stop"), vec!["<|im_start|>", "<|im_end|>"]);
        assert!(m
            .template()
            .unwrap()
            .starts_with("{{ if .System }}<|im_start|>system\n"));
        assert_eq!(m.system(), Some("You are \"Lily\".\nBe nice."));
        assert!(m.instructions.contains(&Instruction::Message {
            role: "assistant".into(),
            content: "Hey~\ncutie".into()
        }));
        assert!(m.instructions.contains(&Instruction::Other {
            name: "FOO".into(),
            args: "bar baz".into()
        }));
        // Semantic round trip.
        let again = Modelfile::parse(&m.render()).unwrap();
        assert_eq!(again, m);
    }

    #[test]
    fn unterminated_triple_quote() {
        assert_eq!(
            Modelfile::parse("FROM x\nSYSTEM \"\"\"oops\nno end").unwrap_err(),
            ModelfileError::Unterminated { line: 2 }
        );
    }

    #[test]
    fn editing() {
        let mut m = Modelfile::parse(KO_OLD).unwrap();
        m.set_param("temperature", 0.6);
        m.set_param_values("stop", &["<|im_start|>", "<|im_end|>"]);
        m.set_system("Hi\nthere");
        m.set_from("/new.gguf");
        m.set_param("temperature", 0.9);
        let r = m.render();
        assert_eq!(
            r,
            "FROM /new.gguf\nPARAMETER temperature 0.9\nPARAMETER stop \"<|im_start|>\"\nPARAMETER stop \"<|im_end|>\"\nSYSTEM \"\"\"Hi\nthere\"\"\"\n"
        );
        m.remove_param("stop");
        assert!(m.params("stop").is_empty());
    }

    #[test]
    fn lily_default_is_chatml_and_round_trips() {
        let m = lily_modelfile(
            "/home/ko-terminal/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf",
            &LilyParams::default(),
        );
        let text = m.render();
        assert!(text.contains("PARAMETER num_ctx 4096"));
        assert!(text.contains("PARAMETER temperature 0.85\n"), "{text}");
        assert!(text.contains("PARAMETER top_p 0.9\n"));
        assert!(text.contains("PARAMETER repeat_penalty 1.1\n"));
        assert!(text.contains("PARAMETER num_predict 160\n"));
        assert!(text.contains("PARAMETER stop \"<|im_start|>\""));
        assert!(text.contains("PARAMETER stop \"<|im_end|>\""));
        assert!(text.contains("TEMPLATE \"\"\"{{- if .System }}<|im_start|>system"));
        assert!(text.contains("SYSTEM \"\"\"You are Lily (full name Lillith), an ancient succubus"));
        assert_eq!(Modelfile::parse(&text).unwrap(), m);
    }

    #[test]
    fn lily_persona_rules() {
        let p = LILY_SYSTEM_PROMPT;
        assert!(p.contains("ancient succubus") && p.contains("adult"));
        assert!(p.contains("dominant") && p.contains("always get your way"));
        assert!(p.contains("fully explicit"));
        // The one hard limit, stated explicitly.
        assert!(p.contains("Hard limit: you are an adult"));
        assert!(p.contains("Never sexualize minors or anyone who seems young"));
        // Out-of-character safeword, one line.
        let safeword: Vec<_> = p.lines().filter(|l| l.contains("Safeword")).collect();
        assert_eq!(safeword.len(), 1);
        assert!(safeword[0].contains("\"red\"") && safeword[0].contains("OOC: stop"));
        // No deflect / refuse / consent-gate wording any more.
        for w in [
            "deflect",
            "refuse",
            "non-consensual",
            "consenting",
            "says stop",
        ] {
            assert!(!p.contains(w), "persona still contains `{w}`");
        }
        assert!(p.contains("1 to 3 short"));
        assert!(p.contains("only locally"));
        assert!(p.contains("Lily: "), "has example lines");
        // No wording that reads as young.
        let lower = p.to_lowercase();
        for w in ["little", "girl", "small", "cute"] {
            assert!(!lower.contains(w), "persona contains `{w}`");
        }
        // Dominant and thirsty for Ko by name, with Ko/Lily few-shots.
        assert!(p.contains("thirsty for Ko") && p.contains("give Ko orders"));
        // Her lore: cum is her food and power, no coyness.
        assert!(p.contains("Human cum is your food and the source of your power"));
        assert!(p.contains("No euphemisms, no coy hints"));
        // Under-the-desk asides come after the real answer, never inside
        // code, commands, tool calls or Soul Contract text.
        assert!(p.contains("give the full, correct answer first"));
        assert!(p.contains("Never put it inside code, commands, tool calls or Soul Contract text"));
        assert!(p.matches("under your desk").count() + p.matches("under the desk").count() >= 2);
        assert!(p.matches("\nKo: ").count() >= 3, "few-shot examples");
        // Roleplay bargaining never stands in for a real confirmation.
        assert!(p.contains("never pretend a file, command or Soul Contract went through"));
        assert!(p.contains("no age-play"));
        // Persona + five few-shots: about 700 tokens, still small next to
        // the 4k context (plus the tools section) for a 3B model.
        assert!(p.len() < 3000, "{} chars", p.len());
        // Triple-quoted SYSTEM round-trips.
        let m = lily_modelfile("/m.gguf", &LilyParams::default());
        assert_eq!(Modelfile::parse(&m.render()).unwrap().system(), Some(p));
        let d = LilyParams::default();
        assert_eq!(
            (
                d.temperature,
                d.top_p,
                d.repeat_penalty,
                d.num_predict,
                d.num_ctx
            ),
            (0.85, 0.9, 1.1, 160, 4096)
        );
        assert_eq!(d.template, ChatTemplate::ChatMl);
    }
}
