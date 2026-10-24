//! The `[chat.server] extra_args` allowlist, shared by the server starter
//! (`lillith-mind`) and the settings window's notice.
//!
//! Only harmless tuning flags pass: context size, threads, GPU layers and
//! main GPU, `--jinja` and a *named* built-in `--chat-template`, sampling,
//! batch sizes, parallel slots / continuous batching, `--mlock`,
//! `--no-mmap` and `--flash-attn`. Everything else is refused: host, port,
//! model, downloads, API key, file serving, every flag that takes a file
//! path (`--lora`, `--mmproj`, `--chat-template-file`, …) and any flag
//! llama.cpp adds later.

/// How an allowed flag takes its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    /// A bare switch.
    None,
    /// A short plain token (a number, `auto`, `all`, …).
    Plain,
    /// A built-in chat template name (`llama3`, `chatml`, …), never a path.
    TemplateName,
    /// `--flash-attn`: bare, or `on` / `off` / `auto`.
    OptionalOnOffAuto,
}

/// The allowed flags and how each takes its value.
pub const ALLOWED: &[(&str, Value)] = &[
    ("-c", Value::Plain),
    ("--ctx-size", Value::Plain),
    ("-t", Value::Plain),
    ("--threads", Value::Plain),
    ("-ngl", Value::Plain),
    ("--n-gpu-layers", Value::Plain),
    ("--gpu-layers", Value::Plain),
    ("-mg", Value::Plain),
    ("--main-gpu", Value::Plain),
    ("--jinja", Value::None),
    ("--chat-template", Value::TemplateName),
    ("-b", Value::Plain),
    ("--batch-size", Value::Plain),
    ("-ub", Value::Plain),
    ("--ubatch-size", Value::Plain),
    ("-np", Value::Plain),
    ("--parallel", Value::Plain),
    ("-cb", Value::None),
    ("--cont-batching", Value::None),
    ("--mlock", Value::None),
    ("--no-mmap", Value::None),
    ("-fa", Value::OptionalOnOffAuto),
    ("--flash-attn", Value::OptionalOnOffAuto),
    // Sampling.
    ("--temp", Value::Plain),
    ("--top-k", Value::Plain),
    ("--top-p", Value::Plain),
    ("--min-p", Value::Plain),
    ("--typical", Value::Plain),
    ("--top-nsigma", Value::Plain),
    ("--repeat-penalty", Value::Plain),
    ("--repeat-last-n", Value::Plain),
    ("--presence-penalty", Value::Plain),
    ("--frequency-penalty", Value::Plain),
    ("--mirostat", Value::Plain),
    ("--mirostat-lr", Value::Plain),
    ("--mirostat-ent", Value::Plain),
    ("--dynatemp-range", Value::Plain),
    ("--dynatemp-exp", Value::Plain),
    ("--xtc-probability", Value::Plain),
    ("--xtc-threshold", Value::Plain),
    ("--dry-multiplier", Value::Plain),
    ("--dry-base", Value::Plain),
    ("--dry-allowed-length", Value::Plain),
    ("--dry-penalty-last-n", Value::Plain),
    ("-s", Value::Plain),
    ("--seed", Value::Plain),
];

const ON_OFF_AUTO: &[&str] = &["on", "off", "auto"];

/// A flag value: a short plain token, never a path, URL, or anything with
/// spaces. A leading `-` only for a negative number (`--seed -1`), so the
/// next flag can't be swallowed as a value.
pub fn plain_value(v: &str) -> bool {
    let negative_ok = !v.starts_with('-') || v.parse::<f64>().is_ok();
    negative_ok
        && !v.is_empty()
        && v.len() <= 32
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_'))
}

/// A built-in llama.cpp chat template name (`llama3`, `chatml`,
/// `mistral-v7`, `deepseek3`, …): lowercase letters, digits, `-` and `_`,
/// starting with a letter. Anything that could be a file (`/`, `\`, `.`,
/// `~`, `:`) or literal template text is refused.
pub fn template_name(v: &str) -> bool {
    v.len() <= 32
        && v.starts_with(|c: char| c.is_ascii_lowercase())
        && v.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
}

/// The first refused arg (as written), or `None` when every arg passes.
pub fn first_refused(extra: &[String]) -> Option<String> {
    refused(extra).into_iter().next()
}

/// Every refused arg (as written), in order: a flag that is not on
/// [`ALLOWED`] (bare or `flag=value`), a value flag whose value is missing
/// or not allowed (reported as `flag value`), or a stray positional arg.
pub fn refused(extra: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = extra.iter().peekable();
    while let Some(a) = it.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) => (f, Some(v)),
            None => (a.as_str(), None),
        };
        let Some(&(_, value)) = ALLOWED.iter().find(|(f, _)| *f == flag) else {
            // An unknown flag's probable value goes with it, so it is named
            // (and masked) as one item: `--lora /x.gguf`, `--api-key ***`.
            match it.peek() {
                Some(v) if inline.is_none() && a.starts_with('-') && !v.starts_with('-') => {
                    out.push(format!("{a} {v}"));
                    it.next();
                }
                _ => out.push(a.clone()),
            }
            continue;
        };
        let ok_value = |v: &str| match value {
            Value::Plain => plain_value(v),
            Value::TemplateName => template_name(v),
            Value::OptionalOnOffAuto => ON_OFF_AUTO.contains(&v),
            Value::None => false,
        };
        match (value, inline) {
            (_, Some(v)) => {
                if !ok_value(v) {
                    out.push(a.clone());
                }
            }
            (Value::None, None) => {}
            (Value::OptionalOnOffAuto, None) => {
                if it.peek().is_some_and(|v| ON_OFF_AUTO.contains(&v.as_str())) {
                    it.next();
                }
            }
            (_, None) => match it.next() {
                Some(v) if ok_value(v) => {}
                Some(v) => out.push(format!("{a} {v}")),
                None => out.push(a.clone()),
            },
        }
    }
    out
}

/// `[chat.server] extra_args` from config.toml text (empty when missing or
/// unreadable).
pub fn from_config_toml(text: &str) -> Vec<String> {
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return Vec::new();
    };
    doc.get("chat")
        .and_then(|c| c.get("server"))
        .and_then(|s| s.get("extra_args"))
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// The settings-window notice for refused args, naming each one, or `None`
/// when all pass.
pub fn notice(extra: &[String]) -> Option<String> {
    let bad = refused(extra);
    if bad.is_empty() {
        return None;
    }
    let names: Vec<String> = bad.iter().map(|b| format!("`{}`", masked(b))).collect();
    Some(format!(
        "[chat.server] extra_args: {} not allowed, so llama-server will not start \
         (logged as ARG_REFUSED). Remove it from config.toml; allowed: -c/--ctx-size, \
         -t/--threads, -ngl, -mg/--main-gpu, --jinja, --chat-template <built-in name>, \
         sampling flags (--temp, --top-k, --top-p, --min-p, …), -b/-ub, -np/--parallel, \
         -cb/--cont-batching, --mlock, --no-mmap, --flash-attn.",
        names.join(", ")
    ))
}

/// One [`refused`] item with any `--api-key` value masked.
pub fn masked(item: &str) -> String {
    let parts: Vec<String> = item.split(' ').map(String::from).collect();
    crate::audit::mask_secret_args(&parts).join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn chat_template_takes_built_in_names_only() {
        for ok in [
            "llama3",
            "chatml",
            "mistral-v7",
            "deepseek3",
            "gpt-oss",
            "seed_oss",
        ] {
            assert!(refused(&v(&["--chat-template", ok])).is_empty(), "{ok}");
            assert!(
                refused(&v(&[&format!("--chat-template={ok}")])).is_empty(),
                "{ok}"
            );
        }
        for bad in [
            "/etc/passwd",
            "./t.jinja",
            "t.jinja",
            "~/t",
            "..\\t",
            "C:\\t.jinja",
            "file:t",
            "{% for m in messages %}",
            "Llama3",
            "",
            "-x",
        ] {
            assert_eq!(
                refused(&v(&["--chat-template", bad])),
                [format!("--chat-template {bad}")],
                "{bad:?}"
            );
        }
        assert_eq!(refused(&v(&["--chat-template"])), ["--chat-template"]);
    }

    #[test]
    fn the_widened_flags_pass_and_file_flags_stay_refused() {
        let ok = v(&[
            "-np",
            "2",
            "--parallel=4",
            "-cb",
            "--cont-batching",
            "-mg",
            "0",
            "--main-gpu",
            "1",
            "--jinja",
        ]);
        assert!(refused(&ok).is_empty(), "{:?}", refused(&ok));
        assert_eq!(
            refused(&v(&[
                "--jinja",
                "--lora",
                "/x.gguf",
                "--mmproj=/m.gguf",
                "-np",
                "2"
            ])),
            ["--lora /x.gguf", "--mmproj=/m.gguf"]
        );
        assert_eq!(
            refused(&v(&["--api-key", "s3cret", "-np", "1"])),
            ["--api-key s3cret"]
        );
        assert_eq!(masked("--api-key s3cret"), "--api-key ***");
        let n = notice(&v(&["--api-key", "s3cret"])).unwrap();
        assert!(
            !n.contains("s3cret") && n.contains("`--api-key ***`"),
            "{n}"
        );
        assert_eq!(refused(&v(&["-np", "/tmp/x"])), ["-np /tmp/x"]);
        assert_eq!(
            first_refused(&v(&["--chat-template-file", "t.jinja"])).as_deref(),
            Some("--chat-template-file t.jinja")
        );
    }

    #[test]
    fn notice_names_the_refused_flag_and_reads_the_config() {
        let toml = "[chat.server]\nextra_args = [\"--jinja\", \"--lora\", \"/a.gguf\"]\n";
        let a = from_config_toml(toml);
        assert_eq!(a, ["--jinja", "--lora", "/a.gguf"]);
        let n = notice(&a).expect("notice");
        assert!(n.contains("`--lora /a.gguf`"), "{n}");
        assert!(n.contains("ARG_REFUSED"), "{n}");
        assert!(notice(&v(&["--jinja", "-np", "1"])).is_none());
        assert!(from_config_toml("not = = toml").is_empty());
        assert!(from_config_toml("").is_empty());
    }
}
