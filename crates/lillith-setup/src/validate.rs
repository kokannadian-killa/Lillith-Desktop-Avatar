//! Per-field validation for settings forms: each `[llm]` key gets a friendly
//! message suitable for showing under a text box. The URL checks use the same
//! loopback rule as the rest of the crate ([`crate::local`]).

use crate::config::{Backend, LlmConfig};
use crate::local::LocalUrl;
use crate::template::ChatTemplate;
use serde::Serialize;
use std::path::Path;

/// A problem with one setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{key}: {message}")]
pub struct FieldError {
    /// The `[llm]` key (`temperature`, `gguf_path`, ...).
    pub key: String,
    /// Friendly, user-facing text.
    pub message: String,
}

impl FieldError {
    fn new(key: &str, message: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            message: message.into(),
        }
    }
}

/// Extra context for cross-field checks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValidateCtx {
    /// Backend chosen in the form (`gguf_path` is required for llama-server).
    pub backend: Option<Backend>,
    /// Training context of the picked model (`ModelInfo::context_length`).
    pub model_ctx: Option<u64>,
}

impl ValidateCtx {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_backend(mut self, b: Backend) -> Self {
        self.backend = Some(b);
        self
    }
    pub fn with_model_ctx(mut self, c: Option<u64>) -> Self {
        self.model_ctx = c;
        self
    }
}

pub const TEMPERATURE_RANGE: (f32, f32) = (0.0, 2.0);
pub const CTX_RANGE: (u32, u32) = (256, 131072);
pub const MAX_THREADS: u32 = 256;

/// Validates the text `value` of one `[llm]` key. Unknown keys are an error
/// (so a typo in a form binding is caught); free-text keys
/// (`system_prompt`) always pass.
pub fn field(key: &str, value: &str, ctx: &ValidateCtx) -> Result<(), FieldError> {
    let v = value.trim();
    let err = |m: String| Err(FieldError::new(key, m));
    match key {
        "backend" => match Backend::parse(v) {
            Some(_) => Ok(()),
            None => err("Choose \"llama-server\" or \"ollama\".".into()),
        },
        "temperature" => match v.parse::<f32>() {
            Ok(t) if t.is_finite() && (TEMPERATURE_RANGE.0..=TEMPERATURE_RANGE.1).contains(&t) => {
                Ok(())
            }
            Ok(_) => err(format!(
                "Temperature must be between {} and {} (0.7–0.9 is a good range for Lily).",
                TEMPERATURE_RANGE.0, TEMPERATURE_RANGE.1
            )),
            Err(_) => err("Enter a number, like 0.8.".into()),
        },
        "ctx_size" => {
            let Ok(n) = v.parse::<u32>() else {
                return err("Enter a whole number of tokens, like 4096.".into());
            };
            if !(CTX_RANGE.0..=CTX_RANGE.1).contains(&n) {
                return err(format!(
                    "Context must be between {} and {} tokens.",
                    CTX_RANGE.0, CTX_RANGE.1
                ));
            }
            if let Some(m) = ctx.model_ctx {
                if u64::from(n) > m {
                    return err(format!(
                        "This model only supports {m} tokens of context; use {m} or less."
                    ));
                }
            }
            Ok(())
        }
        "threads" => match v.parse::<u32>() {
            Ok(n) if n <= MAX_THREADS => Ok(()),
            Ok(_) => err(format!("Use 0 (automatic) or up to {MAX_THREADS} threads.")),
            Err(_) => err("Enter 0 for automatic, or a whole number of CPU threads.".into()),
        },
        "server_url" => url(key, v, 8080),
        "ollama_host" => url(key, v, 11434),
        "gguf_path" => {
            if v.is_empty() {
                return if ctx.backend == Some(Backend::Ollama) {
                    Ok(())
                } else {
                    err("Pick a .gguf model file.".into())
                };
            }
            let p = crate::paths::expand_tilde(v);
            gguf_file(key, &p)
        }
        "llama_server_bin" => {
            if v.is_empty() {
                return Ok(()); // found on PATH
            }
            let p = crate::paths::expand_tilde(v);
            if !p.is_file() {
                return err(
                    "No program at this path (leave empty to find llama-server automatically)."
                        .into(),
                );
            }
            if crate::detect::is_executable(&p) {
                Ok(())
            } else if cfg!(windows) {
                err("This isn't a program (pick llama-server.exe).".into())
            } else {
                err("This file isn't executable (chmod +x it).".into())
            }
        }
        "ollama_model" => {
            if v.is_empty() {
                return if ctx.backend == Some(Backend::Ollama) {
                    err("Give the Ollama model a name, like lily.".into())
                } else {
                    Ok(())
                };
            }
            if v.chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-:/".contains(c))
            {
                Ok(())
            } else {
                err("Use letters, numbers, '.', '_', '-', ':' or '/' only (no spaces).".into())
            }
        }
        "chat_template" => {
            if ChatTemplate::from_name(v).is_some() {
                Ok(())
            } else if v.is_empty() {
                err("Pick a chat template (chatml for Dolphin 3 and Qwen).".into())
            } else {
                // llama-server has more built-ins than we know; allow but explain.
                let known: Vec<_> = ChatTemplate::ALL
                    .iter()
                    .map(|t| t.llama_server_name())
                    .collect();
                if v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
                {
                    Ok(())
                } else {
                    err(format!(
                        "Unknown template; common ones are {}.",
                        known.join(", ")
                    ))
                }
            }
        }
        "modelfile_path" => {
            if v.is_empty() {
                return Ok(());
            }
            let p = crate::paths::expand_tilde(v);
            match p.parent() {
                Some(d) if d.as_os_str().is_empty() || d.is_dir() || !d.exists() => Ok(()),
                _ => err("The folder for this file isn't usable.".into()),
            }
        }
        "scan_paths" => {
            for part in v
                .split(['\n', ','])
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let p = crate::paths::expand_tilde(part);
                if p.exists() && !p.is_dir() {
                    return err(format!("{part} is a file, not a folder."));
                }
            }
            Ok(())
        }
        "system_prompt" => Ok(()),
        _ => err(format!("Unknown setting `{key}`.")),
    }
}

fn url(key: &str, v: &str, port: u16) -> Result<(), FieldError> {
    LocalUrl::parse(v, port).map(|_| ()).map_err(|e| {
        use crate::local::LocalUrlError as E;
        // For `https://remote`, the remote host is the real problem.
        let e = match e {
            E::Scheme { .. } => match v.split_once("://") {
                Some((_, rest)) => match LocalUrl::parse(&format!("http://{rest}"), port) {
                    Err(n @ E::NotLoopback { .. }) => n,
                    _ => e,
                },
                None => e,
            },
            e => e,
        };
        let msg = match e {
            E::Empty(_) => "Enter an address, like http://127.0.0.1:8080.".to_string(),
            E::NotLoopback { host, .. } => format!(
                "`{host}` isn't this computer. Lily is local-only: use 127.0.0.1, ::1 or localhost."
            ),
            E::Scheme { scheme, .. } => {
                format!("Use http:// (got {scheme}://); the server runs on this computer.")
            }
            E::Userinfo { .. } => "Remove the user name / password from the address.".into(),
            E::Port { port, .. } => format!("`{port}` isn't a valid port (1–65535)."),
            E::Malformed { .. } => {
                "That doesn't look like an address; try http://127.0.0.1:8080.".into()
            }
        };
        FieldError::new(key, msg)
    })
}

fn gguf_file(key: &str, p: &Path) -> Result<(), FieldError> {
    let e = |m: &str| Err(FieldError::new(key, m));
    if !p.exists() {
        return e("File not found.");
    }
    if !p.is_file() {
        return e("That's a folder; pick the .gguf file inside it.");
    }
    match std::fs::File::open(p) {
        Err(_) => e("Can't read this file (check its permissions)."),
        Ok(_) if !crate::gguf::is_gguf(p) => e("This isn't a GGUF model file."),
        Ok(_) => Ok(()),
    }
}

/// Validates every field of `cfg`. Empty = all good.
pub fn validate_all(cfg: &LlmConfig) -> Vec<FieldError> {
    validate_all_with(cfg, &ValidateCtx::new())
}

/// [`validate_all`] with a model context (`backend` defaults to `cfg.backend`).
pub fn validate_all_with(cfg: &LlmConfig, ctx: &ValidateCtx) -> Vec<FieldError> {
    let ctx = ValidateCtx {
        backend: Some(ctx.backend.unwrap_or(cfg.backend)),
        ..ctx.clone()
    };
    let fields: [(&str, String); 12] = [
        ("backend", cfg.backend.as_str().into()),
        ("gguf_path", cfg.gguf_path.clone()),
        ("server_url", cfg.server_url.clone()),
        ("ollama_host", cfg.ollama_host.clone()),
        ("ollama_model", cfg.ollama_model.clone()),
        ("temperature", cfg.temperature.to_string()),
        ("ctx_size", cfg.ctx_size.to_string()),
        ("chat_template", cfg.chat_template.clone()),
        ("llama_server_bin", cfg.llama_server_bin.clone()),
        ("modelfile_path", cfg.modelfile_path.clone()),
        ("threads", cfg.threads.to_string()),
        ("scan_paths", cfg.scan_paths.join("\n")),
    ];
    fields
        .iter()
        .filter_map(|(k, v)| field(k, v, &ctx).err())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(k: &str, v: &str) {
        assert_eq!(field(k, v, &ValidateCtx::new()), Ok(()), "{k}={v}");
    }
    fn bad(k: &str, v: &str, needle: &str) {
        let e = field(k, v, &ValidateCtx::new()).expect_err(&format!("{k}={v} should fail"));
        assert_eq!(e.key, k);
        assert!(e.message.contains(needle), "{k}={v}: {}", e.message);
    }

    #[test]
    fn temperature() {
        ok("temperature", "0.8");
        ok("temperature", "0");
        ok("temperature", " 2.0 ");
        bad("temperature", "2.5", "between 0 and 2");
        bad("temperature", "-0.1", "between");
        bad("temperature", "warm", "Enter a number");
        bad("temperature", "NaN", "between");
    }

    #[test]
    fn ctx_size() {
        ok("ctx_size", "4096");
        bad("ctx_size", "100", "between 256 and 131072");
        bad("ctx_size", "4k", "whole number");
        let c = ValidateCtx::new().with_model_ctx(Some(2048));
        assert!(field("ctx_size", "2048", &c).is_ok());
        let e = field("ctx_size", "4096", &c).unwrap_err();
        assert!(e.message.contains("only supports 2048"), "{}", e.message);
    }

    #[test]
    fn threads_backend_template_model() {
        ok("threads", "0");
        ok("threads", "4");
        bad("threads", "-1", "automatic");
        bad("threads", "1000", "up to 256");
        ok("backend", "llama-server");
        ok("backend", "Ollama");
        bad("backend", "openai", "llama-server");
        ok("chat_template", "chatml");
        ok("chat_template", "deepseek3");
        bad("chat_template", "", "chatml");
        bad("chat_template", "{% jinja %}", "common ones");
        ok("ollama_model", "lily:latest");
        bad("ollama_model", "lily bot", "no spaces");
        bad("nonsense_key", "x", "Unknown setting");
        ok("system_prompt", "anything at all\nmulti-line");
    }

    #[test]
    fn urls_must_be_loopback() {
        ok("server_url", "http://127.0.0.1:8080");
        ok("ollama_host", "localhost:11434");
        bad("server_url", "http://192.168.1.4:8080", "local-only");
        bad("ollama_host", "https://ollama.com", "isn't this computer");
        bad("server_url", "https://127.0.0.1:8080", "Use http://");
        bad("server_url", "", "Enter an address");
        bad("server_url", "http://127.0.0.1:70000", "valid port");
    }

    #[test]
    fn gguf_path_checks() {
        let dir = std::env::temp_dir().join(format!("lillith-setup-val-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("m.gguf");
        let notg = dir.join("x.gguf");
        std::fs::write(&good, b"GGUF\x03\0\0\0").unwrap();
        std::fs::write(&notg, b"PK\x03\x04").unwrap();
        ok("gguf_path", good.to_str().unwrap());
        bad("gguf_path", notg.to_str().unwrap(), "isn't a GGUF");
        bad(
            "gguf_path",
            dir.join("missing.gguf").to_str().unwrap(),
            "not found",
        );
        bad("gguf_path", dir.to_str().unwrap(), "folder");
        bad("gguf_path", "", "Pick a .gguf");
        assert!(field(
            "gguf_path",
            "",
            &ValidateCtx::new().with_backend(Backend::Ollama)
        )
        .is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn validate_all_collects() {
        // Defaults: only the missing model is flagged for llama-server.
        let errs = validate_all(&LlmConfig::default());
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert_eq!(errs[0].key, "gguf_path");
        let cfg = LlmConfig {
            backend: Backend::Ollama,
            temperature: 3.0,
            ctx_size: 8192,
            ollama_host: "http://10.0.0.1:11434".into(),
            ..Default::default()
        };
        let errs = validate_all_with(&cfg, &ValidateCtx::new().with_model_ctx(Some(4096)));
        let keys: Vec<_> = errs.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["ollama_host", "temperature", "ctx_size"],
            "{errs:?}"
        );
    }
}
