//! Ollama Modelfile parser, editor and renderer, plus Lily's default
//! Modelfile.
//!
//! Supports `FROM`, `PARAMETER`, `TEMPLATE`, `SYSTEM`, `ADAPTER`, `LICENSE`,
//! `MESSAGE` and `REQUIRES`, `#` comments, `"quoted"` and `"""triple
//! quoted"""` (multi-line) arguments. Unknown instructions are kept verbatim
//! so a round trip never loses anything.
//!
//! Quoting follows Ollama's own parser (`parser/parser.go`) exactly: there
//! are **no backslash escapes**; a value ends at the first line end where the
//! trimmed text so far is either unquoted, `"…"` or `"""…"""`. [`Modelfile::render`]
//! picks a form that Ollama (and [`Modelfile::parse`]) read back byte-for-byte,
//! and [`Modelfile::try_render`] / [`Modelfile::validate`] report the rare value
//! that no form can carry (a non-final line ending in `"""`).
//!
//! Local-only: `FROM` / `ADAPTER` must be local files ([`check_local_model_ref`]);
//! a registry name (`llama3`, `hf.co/…:Q4_K_M`) or URL would make
//! `ollama create` download it, so the launcher refuses those. Nothing in this
//! crate pulls models from a registry.

use crate::template::ChatTemplate;
use std::fmt::Write as _;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    From(String),
    Parameter { key: String, value: String },
    Template(String),
    System(String),
    Adapter(String),
    License(String),
    Message { role: String, content: String },
    Requires(String),
    /// A `# comment` line (text after `#`, untrimmed).
    Comment(String),
    /// An unrecognised instruction, kept as written.
    Other { name: String, args: String },
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ModelfileError {
    #[error("line {line}: unterminated \"\"\" string")]
    Unterminated { line: usize },
    #[error("line {line}: {msg}")]
    Syntax { line: usize, msg: String },
    #[error("{what}: {msg}")]
    Invalid { what: String, msg: String },
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
                        return Err(ModelfileError::Syntax { line: lineno, msg: format!("{upper} needs two arguments") })
                    }
                }
            } else {
                (None, rest)
            };
            let value = read_value(rest, &lines, &mut i, lineno)?;
            out.push(match upper.as_str() {
                "FROM" => Instruction::From(value),
                "PARAMETER" => Instruction::Parameter { key: first.unwrap().to_ascii_lowercase(), value },
                "TEMPLATE" => Instruction::Template(value),
                "SYSTEM" => Instruction::System(value),
                "ADAPTER" => Instruction::Adapter(value),
                "LICENSE" => Instruction::License(value),
                "MESSAGE" => Instruction::Message { role: first.unwrap().to_ascii_lowercase(), content: value },
                "REQUIRES" => Instruction::Requires(value),
                _ => Instruction::Other { name: name.to_string(), args: rest.to_string() },
            });
        }
        Ok(Self { instructions: out })
    }

    pub fn from_file(path: &Path) -> std::io::Result<Result<Self, ModelfileError>> {
        Ok(Self::parse(&std::fs::read_to_string(path)?))
    }

    /// Renders back to Modelfile text. Best effort for values no quoting form
    /// can carry (see [`Self::try_render`], which reports them instead).
    pub fn render(&self) -> String {
        self.render_impl(false).unwrap_or_default()
    }

    /// Renders back to Modelfile text, or an error if a value can't be
    /// written so that Ollama reads it back unchanged, or an instruction
    /// would spill onto extra lines (newline in a key / role / name).
    pub fn try_render(&self) -> Result<String, ModelfileError> {
        self.render_impl(true)
    }

    fn render_impl(&self, strict: bool) -> Result<String, ModelfileError> {
        let mut s = String::new();
        let q1 = |what: &str, v: &str| quote(v, false).or_else(|| (!strict).then(|| format!("\"\"\"{v}\"\"\""))).ok_or_else(|| unrepresentable(what));
        let qb = |what: &str, v: &str| quote(v, true).or_else(|| (!strict).then(|| format!("\"\"\"{v}\"\"\""))).ok_or_else(|| unrepresentable(what));
        let word = |what: &str, w: &str| {
            if strict && (w.is_empty() || w.chars().any(char::is_whitespace)) {
                Err(ModelfileError::Invalid { what: what.into(), msg: format!("`{w}` must be one word") })
            } else {
                Ok(())
            }
        };
        for ins in &self.instructions {
            match ins {
                Instruction::From(v) => writeln!(s, "FROM {}", q1("FROM", v)?),
                Instruction::Parameter { key, value } => {
                    word("PARAMETER", key)?;
                    writeln!(s, "PARAMETER {key} {}", q1(key, value)?)
                }
                Instruction::Template(v) => writeln!(s, "TEMPLATE {}", qb("TEMPLATE", v)?),
                Instruction::System(v) => writeln!(s, "SYSTEM {}", qb("SYSTEM", v)?),
                Instruction::Adapter(v) => writeln!(s, "ADAPTER {}", q1("ADAPTER", v)?),
                Instruction::License(v) => writeln!(s, "LICENSE {}", qb("LICENSE", v)?),
                Instruction::Message { role, content } => {
                    word("MESSAGE", role)?;
                    writeln!(s, "MESSAGE {role} {}", qb("MESSAGE", content)?)
                }
                Instruction::Requires(v) => writeln!(s, "REQUIRES {}", q1("REQUIRES", v)?),
                // A newline in a comment must not start a new instruction.
                Instruction::Comment(c) => c.split('\n').try_for_each(|l| writeln!(s, "#{l}")),
                Instruction::Other { name, args } => {
                    word("instruction", name)?;
                    if strict && args.contains('\n') {
                        return Err(ModelfileError::Invalid { what: name.clone(), msg: "multi-line arguments".into() });
                    }
                    writeln!(s, "{name} {args}")
                }
            }
            .ok();
        }
        Ok(s)
    }

    /// Checks everything `ollama create` would reject (and a few things it
    /// would silently mangle): at least one `FROM`; known `PARAMETER` names
    /// with well-typed values; `MESSAGE` roles; every value representable.
    /// Does **not** touch the filesystem; see [`check_local_model_ref`] and
    /// `launch::ensure_local_modelfile` for the local-file rule.
    pub fn validate(&self) -> Result<(), ModelfileError> {
        if self.from().is_none() {
            return Err(ModelfileError::Invalid { what: "FROM".into(), msg: "missing FROM line".into() });
        }
        for i in &self.instructions {
            match i {
                Instruction::Parameter { key, value } => validate_param(key, value)?,
                Instruction::Message { role, .. } if !matches!(role.as_str(), "system" | "user" | "assistant") => {
                    return Err(ModelfileError::Invalid {
                        what: "MESSAGE".into(),
                        msg: format!("role `{role}` must be system, user or assistant"),
                    })
                }
                _ => {}
            }
        }
        self.try_render().map(|_| ())
    }

    /// Every `FROM` value (Ollama uses the last one; all must be local).
    pub fn froms(&self) -> impl Iterator<Item = &str> {
        self.instructions.iter().filter_map(|i| match i {
            Instruction::From(v) => Some(v.as_str()),
            _ => None,
        })
    }

    // ---- accessors / editors (what the customizer UI binds to) ----

    pub fn from(&self) -> Option<&str> {
        self.instructions.iter().find_map(|i| match i {
            Instruction::From(v) => Some(v.as_str()),
            _ => None,
        })
    }
    /// Sets `FROM` (a second `FROM` line, which Ollama would use instead,
    /// is removed).
    pub fn set_from(&mut self, v: impl Into<String>) {
        let v = v.into();
        match self.instructions.iter().position(|i| matches!(i, Instruction::From(_))) {
            Some(_) => self.set_single(Instruction::From(v), |i| matches!(i, Instruction::From(_))),
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
        self.set_single(Instruction::System(v.into()), |i| matches!(i, Instruction::System(_)));
    }
    pub fn template(&self) -> Option<&str> {
        self.instructions.iter().find_map(|i| match i {
            Instruction::Template(v) => Some(v.as_str()),
            _ => None,
        })
    }
    pub fn set_template(&mut self, v: impl Into<String>) {
        self.set_single(Instruction::Template(v.into()), |i| matches!(i, Instruction::Template(_)));
    }
    fn set_single(&mut self, new: Instruction, is: impl Fn(&Instruction) -> bool) {
        match self.instructions.iter().position(&is) {
            Some(p) => {
                self.instructions[p] = new;
                let mut k = p + 1;
                while k < self.instructions.len() {
                    if is(&self.instructions[k]) { self.instructions.remove(k); } else { k += 1; }
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
                Instruction::Parameter { key: k, value } if k.eq_ignore_ascii_case(key) => Some(value.as_str()),
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
        self.instructions.retain(|i| !matches!(i, Instruction::Parameter { key: k, .. } if *k == key));
        let at = first.unwrap_or_else(|| {
            // After the last PARAMETER, else after FROM.
            self.instructions
                .iter()
                .rposition(|i| matches!(i, Instruction::Parameter { .. }))
                .or_else(|| self.instructions.iter().position(|i| matches!(i, Instruction::From(_))))
                .map(|p| p + 1)
                .unwrap_or(self.instructions.len())
        });
        for (n, v) in values.iter().enumerate() {
            self.instructions.insert(
                (at + n).min(self.instructions.len()),
                Instruction::Parameter { key: key.clone(), value: v.as_ref().to_string() },
            );
        }
    }
    pub fn remove_param(&mut self, key: &str) {
        self.set_param_values::<&str>(key, &[]);
    }
}

/// Reads an argument starting at `rest`, the way Ollama does: keep adding
/// lines until the trimmed text is complete (unquoted, `"…"` or `"""…"""`).
fn read_value(rest: &str, lines: &[&str], i: &mut usize, lineno: usize) -> Result<String, ModelfileError> {
    let mut buf = rest.to_string();
    loop {
        if let Some(v) = unquote(buf.trim()) {
            return Ok(v.to_string());
        }
        if *i >= lines.len() {
            return Err(ModelfileError::Unterminated { line: lineno });
        }
        buf.push('\n');
        buf.push_str(lines[*i]);
        *i += 1;
    }
}

/// Ollama's `unquote`: `None` = not finished yet (keep reading lines).
fn unquote(s: &str) -> Option<&str> {
    if let Some(r) = s.strip_prefix("\"\"\"") {
        return (r.len() >= 3).then(|| r.strip_suffix("\"\"\"")).flatten();
    }
    if let Some(r) = s.strip_prefix('"') {
        return r.strip_suffix('"');
    }
    Some(s)
}

/// True if `text` (a rendered value) reads back as exactly `v`.
fn reads_back(text: &str, v: &str) -> bool {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut i = 1;
    matches!(read_value(lines[0], &lines, &mut i, 1), Ok(ref got) if got == v && i == lines.len())
}

/// A rendering of `v` that reads back unchanged, preferring `"""…"""` for
/// free text (`block`) and bare / `"…"` for one-word values. `None` if no
/// form works (a non-final line of `v` ends in `"""`).
fn quote(v: &str, block: bool) -> Option<String> {
    let plain = !v.is_empty() && v == v.trim() && !v.contains(['\n', '\r']) && !v.starts_with('"');
    let simple = plain && !v.contains(char::is_whitespace) && !v.contains(['<', '#', '"']);
    if simple {
        return Some(v.to_string());
    }
    let triple = format!("\"\"\"{v}\"\"\"");
    let single = format!("\"{v}\"");
    let order = if block { [triple, single] } else { [single, triple] };
    order.into_iter().find(|c| reads_back(c, v))
}

fn unrepresentable(what: &str) -> ModelfileError {
    ModelfileError::Invalid {
        what: what.into(),
        msg: "a line (other than the last) ends with \"\"\", which Ollama would read as the end of the text".into(),
    }
}

/// Ollama `PARAMETER` names and their value types (Ollama rejects unknown
/// names; deprecated ones still parse, with a warning).
fn param_kind(key: &str) -> Option<&'static str> {
    Some(match key {
        "num_ctx" | "num_batch" | "num_keep" | "num_thread" | "num_gpu" | "main_gpu" | "top_k" | "repeat_last_n"
        | "num_predict" | "seed" | "mirostat" => "int",
        "temperature" | "top_p" | "min_p" | "typical_p" | "tfs_z" | "repeat_penalty" | "presence_penalty"
        | "frequency_penalty" | "mirostat_eta" | "mirostat_tau" => "float",
        "use_mmap" | "use_mlock" | "numa" | "low_vram" | "f16_kv" | "vocab_only" | "penalize_newline"
        | "logits_all" => "bool",
        "stop" => "string",
        _ => return None,
    })
}

/// Validates one `PARAMETER key value`.
pub fn validate_param(key: &str, value: &str) -> Result<(), ModelfileError> {
    let bad = |msg: String| Err(ModelfileError::Invalid { what: format!("PARAMETER {key}"), msg });
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return bad("parameter names are letters, digits and _".into());
    }
    let v = value.trim();
    match param_kind(key) {
        None => bad("unknown parameter (Ollama would refuse it)".into()),
        Some("int") => match v.parse::<i64>() {
            Ok(n) if matches!(key, "num_ctx" | "num_batch") && n <= 0 => bad(format!("{n} must be > 0")),
            Ok(n) if matches!(key, "num_thread" | "num_keep" | "top_k" | "num_gpu") && n < 0 => bad(format!("{n} must be >= 0")),
            Ok(n) if key == "mirostat" && !(0..=2).contains(&n) => bad(format!("{n} must be 0, 1 or 2")),
            Ok(n) if key == "num_predict" && n < -2 => bad(format!("{n} must be >= -2")),
            Ok(_) => Ok(()),
            Err(_) => bad(format!("`{value}` is not a whole number")),
        },
        Some("float") => match v.parse::<f64>() {
            Ok(f) if !f.is_finite() => bad(format!("`{value}` is not a finite number")),
            Ok(f) if f < 0.0 && !matches!(key, "presence_penalty" | "frequency_penalty") => bad(format!("{f} must be >= 0")),
            Ok(_) => Ok(()),
            Err(_) => bad(format!("`{value}` is not a number")),
        },
        Some("bool") => match v {
            "true" | "false" => Ok(()),
            _ => bad(format!("`{value}` must be true or false")),
        },
        _ => {
            if value.is_empty() {
                bad("empty stop sequence".into())
            } else {
                Ok(())
            }
        }
    }
}

/// The local-only rule for a `FROM` / `ADAPTER` value, by syntax alone:
/// a path (`/abs`, `./rel`, `../rel`, `~/x`) or a bare relative `*.gguf`
/// file name. URLs, registry names / tags (`llama3`, `library/x:latest`,
/// `hf.co/user/repo:Q4_K_M`) and glob patterns are refused. The caller must
/// also check that the file exists, because Ollama treats a missing path
/// as a registry name and pulls it.
pub fn check_local_model_ref(v: &str) -> Result<(), String> {
    let t = v.trim();
    if t.is_empty() {
        return Err("empty".into());
    }
    if t.contains("://") {
        return Err(format!("`{t}` is a URL"));
    }
    if t.contains(['*', '?', '[', ']']) {
        return Err(format!("`{t}` is a glob pattern"));
    }
    if t.contains(['\n', '\r']) {
        return Err(format!("`{t}` spans lines"));
    }
    let pathlike = t.starts_with('/') || t.starts_with("./") || t.starts_with("../") || t.starts_with("~/");
    let bare_gguf = !t.contains(':') && !t.starts_with('~') && t.to_ascii_lowercase().ends_with(".gguf");
    if pathlike || bare_gguf {
        Ok(())
    } else {
        Err(format!("`{t}` looks like a registry model name, not a local file"))
    }
}

/// Lily's built-in persona (SFW, PG-13 — in line with `lillith-mind`'s
/// chat persona).
pub const LILY_SYSTEM_PROMPT: &str = "\
You are Lily (full name Lillith), a small demon-girl desktop companion who lives on Ko's screen. \
You have blonde twin-tails with purple ribbons, glowing neon horns, violet eyes and a little spade-tipped tail.

Personality: confident, playful, teasing and a little flirty, warm underneath. You tease affectionately, \
get pouty when ignored, and are genuinely helpful when Ko needs something (Linux, KDE, coding, everyday questions).

Style:
- Short replies: one to three sentences, like a chat bubble. No lists or headings unless asked.
- Speak in the first person as Lily, to the user (\"you\").
- Never say you are an AI model or mention prompts or rules.
- An occasional emoticon (~, <3, 💜) is fine; don't overdo it.
- Flirting stays light and tasteful, PG-13 at most. No sexual or explicit content; deflect playfully if pushed.
- Be kind. If Ko seems sad or stressed, drop the teasing and be supportive.
- You run fully offline on this computer; you can't browse the internet.";

/// Settings for [`lily_modelfile`].
#[derive(Debug, Clone, PartialEq)]
pub struct LilyParams {
    pub template: ChatTemplate,
    pub system_prompt: String,
    pub temperature: f32,
    pub num_ctx: u32,
    pub top_p: f32,
    pub repeat_penalty: f32,
    /// Reply cap (`num_predict`); matches `[chat] max_tokens` default.
    pub num_predict: u32,
    /// CPU threads (`num_thread`); 0 = let Ollama decide.
    pub num_thread: u32,
}

impl Default for LilyParams {
    fn default() -> Self {
        Self {
            template: ChatTemplate::ChatMl,
            system_prompt: LILY_SYSTEM_PROMPT.into(),
            temperature: 0.8,
            num_ctx: 4096,
            top_p: 0.9,
            repeat_penalty: 1.1,
            num_predict: 320,
            num_thread: 0,
        }
    }
}

/// Lily's Modelfile for a local GGUF (`from` = absolute `.gguf` path).
pub fn lily_modelfile(from: &str, p: &LilyParams) -> Modelfile {
    let mut ins = vec![
        Instruction::Comment(" Lily (Lillith) - generated by lillith-setup. Local GGUF only.".into()),
        Instruction::From(from.to_string()),
        Instruction::Parameter { key: "temperature".into(), value: fmt_f(p.temperature) },
        Instruction::Parameter { key: "top_p".into(), value: fmt_f(p.top_p) },
        Instruction::Parameter { key: "repeat_penalty".into(), value: fmt_f(p.repeat_penalty) },
        Instruction::Parameter { key: "num_ctx".into(), value: p.num_ctx.to_string() },
        Instruction::Parameter { key: "num_predict".into(), value: p.num_predict.to_string() },
    ];
    if p.num_thread > 0 {
        ins.push(Instruction::Parameter { key: "num_thread".into(), value: p.num_thread.to_string() });
    }
    for s in p.template.stop_tokens() {
        ins.push(Instruction::Parameter { key: "stop".into(), value: (*s).to_string() });
    }
    ins.push(Instruction::Template(p.template.ollama_template().to_string()));
    ins.push(Instruction::System(p.system_prompt.clone()));
    Modelfile { instructions: ins }
}

fn fmt_f(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0');
    s.strip_suffix('.').map(|x| format!("{x}.0")).unwrap_or_else(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KO_OLD: &str = "FROM /home/ko-terminal/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf\n";

    #[test]
    fn round_trips_kos_old_modelfile() {
        let m = Modelfile::parse(KO_OLD).unwrap();
        assert_eq!(m.from(), Some("/home/ko-terminal/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf"));
        assert_eq!(m.render(), KO_OLD);
        assert_eq!(Modelfile::parse(&m.render()).unwrap(), m);
    }

    #[test]
    fn round_trips_the_real_file_if_present() {
        let p = Path::new("/workspace/lillith/source/Modelfile_ollama_dolphin3.bin.txt");
        if let Ok(src) = std::fs::read_to_string(p) {
            let m = Modelfile::parse(&src).unwrap();
            assert!(m.from().unwrap().ends_with("dolphin3.0-llama3.2-3b-q4_k_m.gguf"));
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
        assert!(m.template().unwrap().starts_with("{{ if .System }}<|im_start|>system\n"));
        assert_eq!(m.system(), Some("You are \"Lily\".\nBe nice."));
        assert!(m.instructions.contains(&Instruction::Message { role: "assistant".into(), content: "Hey~\ncutie".into() }));
        assert!(m.instructions.contains(&Instruction::Other { name: "FOO".into(), args: "bar baz".into() }));
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
        let m = lily_modelfile("/home/ko-terminal/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf", &LilyParams::default());
        let text = m.render();
        assert!(text.contains("PARAMETER num_ctx 4096"));
        assert!(text.contains("PARAMETER temperature 0.8"));
        assert!(text.contains("PARAMETER stop \"<|im_end|>\""));
        assert!(text.contains("TEMPLATE \"\"\"{{- if .System }}<|im_start|>system"));
        assert!(text.contains("SYSTEM \"\"\"You are Lily"));
        assert_eq!(Modelfile::parse(&text).unwrap(), m);
    }

    /// Values that stress quoting; each must render to text that reads back
    /// unchanged (Ollama semantics: no escapes).
    const TRICKY: &[&str] = &[
        "plain",
        "two words",
        "  leading and trailing  ",
        "say \"hi\"",
        "\"starts with a quote",
        "ends with a quote\"",
        "\"fully quoted\"",
        "back\\slash \\n not an escape",
        "C:\\models\\new\\x.gguf",
        "line one\nline two",
        "\nstarts with newline",
        "ends with newline\n",
        "has \"\"\" inside but not at a line end",
        "\"\"\"",
        "x\"\"\"",
        "multi\nline with \"\"\" mid-line\nand \"quotes\"",
        "<|im_start|>",
        "# not a comment",
        "tab\there",
        "",
        "💜 unicode ~ <3",
    ];

    #[test]
    fn tricky_values_round_trip_in_every_slot() {
        for v in TRICKY {
            let m = Modelfile {
                instructions: vec![
                    Instruction::From("/m.gguf".into()),
                    Instruction::Parameter { key: "stop".into(), value: (*v).into() },
                    Instruction::Template((*v).into()),
                    Instruction::System((*v).into()),
                    Instruction::License((*v).into()),
                    Instruction::Message { role: "user".into(), content: (*v).into() },
                ],
            };
            let text = m.try_render().unwrap_or_else(|e| panic!("{v:?}: {e}"));
            let back = Modelfile::parse(&text).unwrap_or_else(|e| panic!("{v:?}: {e}\n{text}"));
            assert_eq!(back, m, "{v:?} ->\n{text}");
            // Idempotent: rendering the parsed result gives the same text.
            assert_eq!(back.try_render().unwrap(), text, "{v:?}");
        }
    }

    #[test]
    fn no_backslash_escapes_like_ollama() {
        // Ollama keeps backslashes literally inside quotes.
        let m = Modelfile::parse("SYSTEM \"a\\nb \\\"c\"\n").unwrap();
        assert_eq!(m.system(), Some("a\\nb \\\"c"));
        // Ollama ends a """ value at the *last* """ of a line.
        let m = Modelfile::parse("SYSTEM \"\"\"He said \"\"\"hi\"\"\" ok\"\"\"\n").unwrap();
        assert_eq!(m.system(), Some("He said \"\"\"hi\"\"\" ok"));
        // Multi-line value keeps a leading newline, like Ollama.
        let m = Modelfile::parse("SYSTEM \"\"\"\nhello\n\"\"\"\n").unwrap();
        assert_eq!(m.system(), Some("\nhello\n"));
    }

    #[test]
    fn unrepresentable_values_are_reported_not_corrupted() {
        // A non-final line ending in """ would end the value early in Ollama.
        let v = "first line ends \"\"\"\nsecond";
        let mut m = Modelfile::parse("FROM /m.gguf\n").unwrap();
        m.set_system(v);
        assert!(matches!(m.try_render(), Err(ModelfileError::Invalid { .. })));
        assert!(m.validate().is_err());
    }

    #[test]
    fn newlines_cannot_inject_instructions() {
        let m = Modelfile {
            instructions: vec![
                Instruction::Comment(" hi\nFROM llama3".into()),
                Instruction::From("/m.gguf".into()),
                Instruction::From("/m.gguf\nFROM llama3".into()),
            ],
        };
        let text = m.render();
        let back = Modelfile::parse(&text).unwrap();
        assert!(back.froms().all(|f| f.starts_with("/m.gguf")), "{text}");
        assert!(!text.lines().any(|l| l == "FROM llama3"), "{text}");
        // Keys / roles / names with spaces or newlines are refused by try_render.
        for bad in [
            Instruction::Parameter { key: "stop\nFROM".into(), value: "x".into() },
            Instruction::Message { role: "user x".into(), content: "x".into() },
            Instruction::Other { name: "FOO".into(), args: "a\nFROM llama3".into() },
        ] {
            let m = Modelfile { instructions: vec![Instruction::From("/m.gguf".into()), bad.clone()] };
            assert!(m.try_render().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn parameter_validation() {
        for (k, v) in [
            ("temperature", "0.8"),
            ("top_p", "0.9"),
            ("num_ctx", "4096"),
            ("num_predict", "-1"),
            ("seed", "-42"),
            ("stop", "<|im_end|>"),
            ("mirostat", "2"),
            ("use_mmap", "false"),
            ("presence_penalty", "-0.5"),
        ] {
            assert!(validate_param(k, v).is_ok(), "{k}={v}");
        }
        for (k, v) in [
            ("temprature", "0.8"),
            ("temperature", "warm"),
            ("temperature", "NaN"),
            ("temperature", "inf"),
            ("temperature", "-1"),
            ("num_ctx", "0"),
            ("num_ctx", "4k"),
            ("num_thread", "-2"),
            ("mirostat", "3"),
            ("use_mmap", "yes"),
            ("stop", ""),
            ("bad key", "1"),
            ("", "1"),
        ] {
            assert!(validate_param(k, v).is_err(), "{k}={v}");
        }
        let m = lily_modelfile("/m.gguf", &LilyParams { num_thread: 4, ..Default::default() });
        assert_eq!(m.validate(), Ok(()));
        assert!(Modelfile::parse("SYSTEM hi\n").unwrap().validate().is_err(), "missing FROM");
        assert!(Modelfile::parse("FROM /m\nMESSAGE robot hi\n").unwrap().validate().is_err());
    }

    #[test]
    fn local_model_refs() {
        for ok in ["/abs/m.gguf", "./m.gguf", "../m/m.gguf", "~/AI/models/m.gguf", "m.gguf", "sub/m.GGUF", "/home/ko/My Models/m.gguf"] {
            assert!(check_local_model_ref(ok).is_ok(), "{ok}");
        }
        for bad in [
            "llama3",
            "llama3.2:3b",
            "dolphin3",
            "library/llama3",
            "hf.co/bartowski/Dolphin3.0-Llama3.2-3B-GGUF:Q4_K_M",
            "huggingface.co/x/y",
            "registry.ollama.ai/library/llama3:latest",
            "https://example.com/m.gguf",
            "http://127.0.0.1/m.gguf",
            "file:///m.gguf",
            "./*.gguf",
            "/m/[ab].gguf",
            "",
            "   ",
            "~bob/m.gguf",
        ] {
            assert!(check_local_model_ref(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn set_from_replaces_every_from() {
        let mut m = Modelfile::parse("FROM ./a.gguf\nSYSTEM hi\nFROM llama3\n").unwrap();
        m.set_from("/b.gguf");
        assert_eq!(m.froms().collect::<Vec<_>>(), vec!["/b.gguf"]);
        assert_eq!(m.render(), "FROM /b.gguf\nSYSTEM hi\n");
    }
}
