//! "Test it" step of the wizard: ping the configured local server and run a
//! short chat round trip. Also re-exported as `lillith_setup::test`.
//!
//! Loopback-only like the rest of the crate: every URL comes from the
//! validated `[llm]` config ([`LlmConfig::server`] / [`LlmConfig::ollama`])
//! and goes through [`crate::http`], which only connects to a
//! [`crate::local::LocalUrl`]. Nothing is pulled: Ollama's `/api/chat`
//! answers 404 for a missing model instead of downloading it, and that is
//! reported as [`CheckError::ModelMissing`].

use crate::config::{Backend, ConfigError, LlmConfig};
use crate::http::{self, HttpError};
use crate::local::LocalUrl;
use crate::modelfile::{
    LILY_MIN_P, LILY_REPEAT_LAST_N, LILY_REPEAT_PENALTY, LILY_SYSTEM_PROMPT, LILY_TOP_P,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;

/// Result of [`ping_server`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServerHealth {
    pub backend: Backend,
    /// Base URL that was checked.
    pub url: String,
    /// True when the server is up and the configured model can answer.
    pub ready: bool,
    /// Short human status: `ok`, `loading model`, `model "lily" not created yet`, ...
    pub status: String,
    /// Model name (llama-server: GGUF file name / alias; Ollama: the
    /// configured `ollama_model` if installed).
    pub model: Option<String>,
    /// Context size the server runs with (llama-server `n_ctx`, Ollama `num_ctx`
    /// or the model's context length).
    pub ctx: Option<u64>,
    /// Server version if it reports one (Ollama).
    pub version: Option<String>,
    /// Models the server offers.
    pub models: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("can't reach the server: {0}")]
    Http(#[from] HttpError),
    #[error("the server is still loading the model, try again in a moment")]
    Loading,
    #[error("model `{model}` isn't available locally (have: {available:?}); create it from the Modelfile first")]
    ModelMissing {
        model: String,
        available: Vec<String>,
    },
    #[error("server answered HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("unexpected reply: {0}")]
    BadResponse(String),
}

/// Timeouts for the probes.
const PING_TIMEOUT: Duration = Duration::from_millis(1500);

/// Pings the server for `cfg.backend` (llama-server: `/health` + `/props`;
/// Ollama: `/api/version` + `/api/tags` + `/api/show`). Connection failures
/// are `Err`; a server that is up but not ready is `Ok` with `ready = false`.
pub fn ping_server(cfg: &LlmConfig) -> Result<ServerHealth, CheckError> {
    match cfg.backend {
        Backend::LlamaServer => ping_llama_server(cfg),
        Backend::Ollama => ping_ollama(cfg),
    }
}

/// llama-server health: `/health`, then `/props` (model + `n_ctx`).
pub fn ping_llama_server(cfg: &LlmConfig) -> Result<ServerHealth, CheckError> {
    let base = cfg.server()?;
    let mut h = ServerHealth {
        backend: Backend::LlamaServer,
        url: base.base(),
        ready: false,
        status: String::new(),
        model: None,
        ctx: None,
        version: None,
        models: Vec::new(),
    };
    let r = http::get(&base, "/health", PING_TIMEOUT)?;
    let v: Value = serde_json::from_str(&r.body).unwrap_or_default();
    match r.status {
        200 => {
            h.ready = true;
            h.status = v
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("ok")
                .to_string();
        }
        503 => {
            h.status = "loading model".into();
            return Ok(h);
        }
        s => {
            return Err(CheckError::Status {
                status: s,
                body: truncate(&r.body),
            })
        }
    }
    if let Ok(r) = http::get(&base, "/props", PING_TIMEOUT) {
        let v: Value = serde_json::from_str(&r.body).unwrap_or_default();
        h.ctx = v
            .pointer("/default_generation_settings/n_ctx")
            .or_else(|| v.get("n_ctx"))
            .and_then(Value::as_u64);
        h.model = v
            .get("model_alias")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                v.get("model_path")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
            })
            .map(file_name);
    }
    if let Ok(r) = http::get(&base, "/v1/models", PING_TIMEOUT) {
        let v: Value = serde_json::from_str(&r.body).unwrap_or_default();
        h.models = v
            .get("data")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|m| m.get("id")?.as_str().map(file_name))
                    .collect()
            })
            .unwrap_or_default();
    }
    if h.model.is_none() {
        h.model = h.models.first().cloned();
    }
    Ok(h)
}

/// Ollama health: `/api/version`, `/api/tags` (is `ollama_model` installed?),
/// `/api/show` (context size).
pub fn ping_ollama(cfg: &LlmConfig) -> Result<ServerHealth, CheckError> {
    let base = cfg.ollama()?;
    let mut h = ServerHealth {
        backend: Backend::Ollama,
        url: base.base(),
        ready: false,
        status: String::new(),
        model: None,
        ctx: None,
        version: None,
        models: Vec::new(),
    };
    let r = http::get(&base, "/api/version", PING_TIMEOUT)?;
    if r.status == 200 {
        let v: Value = serde_json::from_str(&r.body).unwrap_or_default();
        h.version = v.get("version").and_then(Value::as_str).map(str::to_string);
    }
    let r = http::get(&base, "/api/tags", PING_TIMEOUT)?;
    if r.status != 200 {
        return Err(CheckError::Status {
            status: r.status,
            body: truncate(&r.body),
        });
    }
    h.models = crate::detect::parse_ollama_tags(&r.body)
        .into_iter()
        .map(|m| m.name)
        .collect();
    let Some(name) = find_ollama_model(&h.models, &cfg.ollama_model) else {
        h.status = format!("model \"{}\" not created yet", cfg.ollama_model);
        return Ok(h);
    };
    h.model = Some(name.clone());
    h.ready = true;
    h.status = "ok".into();
    let body = json!({ "model": name }).to_string();
    if let Ok(r) = http::post_json(
        &base,
        "/api/show",
        &body,
        PING_TIMEOUT,
        Duration::from_secs(5),
    ) {
        let v: Value = serde_json::from_str(&r.body).unwrap_or_default();
        h.ctx =
            v.get("parameters")
                .and_then(Value::as_str)
                .and_then(|p| {
                    p.lines().find_map(|l| {
                        let mut it = l.split_whitespace();
                        (it.next() == Some("num_ctx"))
                            .then(|| it.next()?.parse().ok())
                            .flatten()
                    })
                })
                .or_else(|| {
                    v.get("model_info")?.as_object()?.iter().find_map(|(k, v)| {
                        k.ends_with(".context_length").then(|| v.as_u64()).flatten()
                    })
                });
    }
    Ok(h)
}

/// `lily` matches `lily:latest`; an explicit tag must match exactly.
fn find_ollama_model(models: &[String], want: &str) -> Option<String> {
    let want = want.trim();
    if want.is_empty() {
        return None;
    }
    models
        .iter()
        .find(|m| *m == want || (!want.contains(':') && m.as_str() == format!("{want}:latest")))
        .cloned()
}

/// Options for [`test_chat_with`].
#[derive(Debug, Clone, PartialEq)]
pub struct ChatTestOptions {
    pub connect_timeout: Duration,
    /// Per-read timeout for the reply (first load of a model on a laptop CPU
    /// can take a while).
    pub timeout: Duration,
    pub max_tokens: u32,
    /// `None`: llama-server gets `cfg.system_prompt` or Lily's built-in
    /// persona; Ollama uses the Modelfile's SYSTEM unless `cfg.system_prompt`
    /// is set.
    pub system_prompt: Option<String>,
}

impl Default for ChatTestOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(2),
            timeout: Duration::from_secs(90),
            max_tokens: 64,
            system_prompt: None,
        }
    }
}

/// llama-server `/v1/chat/completions` body with Lily's sampling
/// (`top_p`, `min_p`, `repeat_penalty`, `repeat_last_n`).
fn llama_chat_body(temperature: f32, system: &str, prompt: &str, max_tokens: u32) -> Value {
    json!({
        "model": "local",
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": prompt },
        ],
        "max_tokens": max_tokens,
        "temperature": temperature,
        "top_p": LILY_TOP_P,
        "min_p": LILY_MIN_P,
        "repeat_penalty": LILY_REPEAT_PENALTY,
        "repeat_last_n": LILY_REPEAT_LAST_N,
        "stream": false,
    })
}

/// Ollama `/api/chat` body; sampling goes in `options`.
fn ollama_chat_body(cfg: &LlmConfig, messages: Vec<Value>, max_tokens: u32) -> Value {
    json!({
        "model": cfg.ollama_model,
        "messages": messages,
        "stream": false,
        "options": {
            "temperature": cfg.temperature,
            "top_p": LILY_TOP_P,
            "min_p": LILY_MIN_P,
            "repeat_penalty": LILY_REPEAT_PENALTY,
            "repeat_last_n": LILY_REPEAT_LAST_N,
            "num_predict": max_tokens,
            "num_ctx": cfg.ctx_size,
        },
    })
}

/// One short, non-streaming chat turn with the configured local model;
/// returns the reply text.
pub fn test_chat(cfg: &LlmConfig, prompt: &str) -> Result<String, CheckError> {
    test_chat_with(cfg, prompt, &ChatTestOptions::default())
}

/// [`test_chat`] with explicit timeouts / length / system prompt.
pub fn test_chat_with(
    cfg: &LlmConfig,
    prompt: &str,
    o: &ChatTestOptions,
) -> Result<String, CheckError> {
    let custom = (!cfg.system_prompt.trim().is_empty()).then(|| cfg.system_prompt.clone());
    match cfg.backend {
        Backend::LlamaServer => {
            let base = cfg.server()?;
            let system = o
                .system_prompt
                .clone()
                .or(custom)
                .unwrap_or_else(|| LILY_SYSTEM_PROMPT.into());
            let body = llama_chat_body(cfg.temperature, &system, prompt, o.max_tokens);
            let v = post(&base, "/v1/chat/completions", &body, o)?;
            v.pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .ok_or_else(|| CheckError::BadResponse(truncate(&v.to_string())))
        }
        Backend::Ollama => {
            let base = cfg.ollama()?;
            let mut messages = Vec::new();
            if let Some(s) = o.system_prompt.clone().or(custom) {
                messages.push(json!({ "role": "system", "content": s }));
            }
            messages.push(json!({ "role": "user", "content": prompt }));
            let body = ollama_chat_body(cfg, messages, o.max_tokens);
            match post(&base, "/api/chat", &body, o) {
                Err(CheckError::Status { status: 404, .. }) => {
                    let available = http::get(&base, "/api/tags", PING_TIMEOUT)
                        .map(|r| {
                            crate::detect::parse_ollama_tags(&r.body)
                                .into_iter()
                                .map(|m| m.name)
                                .collect()
                        })
                        .unwrap_or_default();
                    Err(CheckError::ModelMissing {
                        model: cfg.ollama_model.clone(),
                        available,
                    })
                }
                Err(e) => Err(e),
                Ok(v) => v
                    .pointer("/message/content")
                    .and_then(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .ok_or_else(|| CheckError::BadResponse(truncate(&v.to_string()))),
            }
        }
    }
}

fn post(
    base: &LocalUrl,
    path: &str,
    body: &Value,
    o: &ChatTestOptions,
) -> Result<Value, CheckError> {
    let r = http::post_json(base, path, &body.to_string(), o.connect_timeout, o.timeout)?;
    match r.status {
        200 => serde_json::from_str(&r.body)
            .map_err(|e| CheckError::BadResponse(format!("{e}: {}", truncate(&r.body)))),
        503 => Err(CheckError::Loading),
        s => Err(CheckError::Status {
            status: s,
            body: truncate(&r.body),
        }),
    }
}

fn file_name(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string())
}

fn truncate(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 300 {
        format!("{}…", s.chars().take(300).collect::<String>())
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::fake_server;

    fn llama(port: u16) -> LlmConfig {
        LlmConfig {
            server_url: format!("http://127.0.0.1:{port}"),
            ..Default::default()
        }
    }
    fn ollama(port: u16) -> LlmConfig {
        LlmConfig {
            backend: Backend::Ollama,
            ollama_host: format!("127.0.0.1:{port}"),
            ..Default::default()
        }
    }

    #[test]
    fn chat_bodies_carry_lily_sampling() {
        let b = llama_chat_body(0.95, "sys", "hi", 64);
        assert!((b["temperature"].as_f64().unwrap() - 0.95).abs() < 1e-6);
        assert!((b["top_p"].as_f64().unwrap() - 0.95).abs() < 1e-6);
        assert!((b["min_p"].as_f64().unwrap() - 0.1).abs() < 1e-6);
        assert!((b["repeat_penalty"].as_f64().unwrap() - 1.1).abs() < 1e-6);
        assert_eq!(b["repeat_last_n"], 256);
        assert_eq!(b["max_tokens"], 64);
        let cfg = LlmConfig::default();
        let o = ollama_chat_body(&cfg, vec![], 160);
        let opt = &o["options"];
        assert!((opt["temperature"].as_f64().unwrap() - 0.95).abs() < 1e-6);
        assert!((opt["top_p"].as_f64().unwrap() - 0.95).abs() < 1e-6);
        assert!((opt["min_p"].as_f64().unwrap() - 0.1).abs() < 1e-6);
        assert!((opt["repeat_penalty"].as_f64().unwrap() - 1.1).abs() < 1e-6);
        assert_eq!(opt["repeat_last_n"], 256);
        assert_eq!(opt["num_predict"], 160);
    }

    #[test]
    fn llama_server_ping_reads_props() {
        let port = fake_server(vec![
            ("/health", 200, r#"{"status":"ok"}"#),
            (
                "/props",
                200,
                r#"{"default_generation_settings":{"n_ctx":4096},"model_path":"/m/dolphin3.0-llama3.2-3b-q4_k_m.gguf"}"#,
            ),
            (
                "/v1/models",
                200,
                r#"{"data":[{"id":"/m/dolphin3.0-llama3.2-3b-q4_k_m.gguf"}]}"#,
            ),
        ]);
        let h = ping_server(&llama(port)).unwrap();
        assert!(h.ready);
        assert_eq!(h.status, "ok");
        assert_eq!(h.ctx, Some(4096));
        assert_eq!(
            h.model.as_deref(),
            Some("dolphin3.0-llama3.2-3b-q4_k_m.gguf")
        );
        assert_eq!(
            h.models,
            vec!["dolphin3.0-llama3.2-3b-q4_k_m.gguf".to_string()]
        );
    }

    #[test]
    fn llama_server_loading_is_not_ready() {
        let port = fake_server(vec![(
            "/health",
            503,
            r#"{"error":{"message":"Loading model"}}"#,
        )]);
        let h = ping_server(&llama(port)).unwrap();
        assert!(!h.ready);
        assert_eq!(h.status, "loading model");
    }

    #[test]
    fn llama_server_chat() {
        let port = fake_server(vec![(
            "POST /v1/chat/completions",
            200,
            r#"{"choices":[{"message":{"role":"assistant","content":"  Hi, cutie~ <3 "}}]}"#,
        )]);
        assert_eq!(test_chat(&llama(port), "hello").unwrap(), "Hi, cutie~ <3");
        let port = fake_server(vec![("POST /v1/chat/completions", 503, "{}")]);
        assert!(matches!(
            test_chat(&llama(port), "hello"),
            Err(CheckError::Loading)
        ));
    }

    #[test]
    fn ollama_ping_and_chat() {
        let port = fake_server(vec![
            ("/api/version", 200, r#"{"version":"0.12.3"}"#),
            (
                "/api/tags",
                200,
                r#"{"models":[{"name":"lily:latest","size":1}]}"#,
            ),
            (
                "POST /api/show",
                200,
                r#"{"parameters":"temperature 0.8\nnum_ctx 4096\nstop \"<|im_end|>\""}"#,
            ),
            (
                "POST /api/chat",
                200,
                r#"{"message":{"role":"assistant","content":"Hey~"},"done":true}"#,
            ),
        ]);
        let h = ping_server(&ollama(port)).unwrap();
        assert!(h.ready, "{h:?}");
        assert_eq!(h.model.as_deref(), Some("lily:latest"));
        assert_eq!(h.ctx, Some(4096));
        assert_eq!(h.version.as_deref(), Some("0.12.3"));
        assert_eq!(test_chat(&ollama(port), "hi").unwrap(), "Hey~");
    }

    #[test]
    fn ollama_missing_model() {
        let port = fake_server(vec![
            ("/api/version", 200, r#"{"version":"0.12.3"}"#),
            ("/api/tags", 200, r#"{"models":[{"name":"llama3.2:3b"}]}"#),
            (
                "POST /api/chat",
                404,
                r#"{"error":"model 'lily' not found"}"#,
            ),
        ]);
        let h = ping_server(&ollama(port)).unwrap();
        assert!(!h.ready);
        assert!(h.status.contains("not created"));
        match test_chat(&ollama(port), "hi") {
            Err(CheckError::ModelMissing { model, available }) => {
                assert_eq!(model, "lily");
                assert_eq!(available, vec!["llama3.2:3b".to_string()]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn remote_hosts_never_contacted() {
        let cfg = LlmConfig {
            server_url: "http://192.168.1.9:8080".into(),
            ..Default::default()
        };
        assert!(matches!(
            ping_server(&cfg),
            Err(CheckError::Config(ConfigError::NotLocal { .. }))
        ));
        assert!(matches!(test_chat(&cfg, "hi"), Err(CheckError::Config(_))));
        let cfg = LlmConfig {
            backend: Backend::Ollama,
            ollama_host: "https://ollama.com".into(),
            ..Default::default()
        };
        assert!(matches!(ping_server(&cfg), Err(CheckError::Config(_))));
        assert!(matches!(test_chat(&cfg, "hi"), Err(CheckError::Config(_))));
    }

    #[test]
    fn nothing_listening_is_an_error() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        assert!(matches!(
            ping_server(&llama(port)),
            Err(CheckError::Http(_))
        ));
    }

    #[test]
    fn test_alias_resolves() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        assert!(crate::test::ping_server(&llama(port)).is_err());
    }
}
