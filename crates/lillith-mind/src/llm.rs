//! Local LLM clients.
//!
//! Two wire protocols: Ollama (`/api/chat`, default `http://127.0.0.1:11434`)
//! and the OpenAI-compatible `/v1/chat/completions` that llama.cpp's
//! `llama-server` speaks (default `http://127.0.0.1:8080`). The chat bubble
//! uses [`LlmClient::chat_stream`] against llama-server: the reply arrives
//! as Server-Sent Events (`data: {json}` lines, ending with `data: [DONE]`),
//! parsed incrementally by [`SseParser`].
//!
//! Local-only: every request URL is rebuilt from
//! [`lillith_setup::LocalUrl::parse`] (the same validator the setup wizard
//! uses) right before it is sent, so only `http://` to 127.0.0.0/8, `::1`
//! or `localhost` is ever contacted, by IP literal (no DNS). Anything else
//! fails with [`LlmError::NotLocal`] without touching the network. Proxies
//! from the environment and HTTP redirects are disabled for the same reason.

use futures_util::StreamExt;
use lillith_setup::{LocalUrl, LocalUrlError};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// A chat message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn system(c: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: c.into(),
        }
    }
    pub fn user(c: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: c.into(),
        }
    }
    pub fn assistant(c: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: c.into(),
        }
    }
}

/// Errors from an LLM backend.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// Nothing listens at the endpoint (connection refused / DNS / connect
    /// timeout): the server is down or still starting.
    #[error("server unreachable: {0}")]
    Unreachable(String),
    /// The server answered but the request took longer than the timeout.
    #[error("timed out: {0}")]
    Timeout(String),
    /// Non-2xx answer (e.g. 503 while llama-server is still loading).
    #[error("server returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("http: {0}")]
    Http(reqwest::Error),
    #[error("unexpected response: {0}")]
    Response(String),
    /// The configured endpoint is not a loopback `http://` URL; nothing
    /// was sent.
    #[error("refusing non-local LLM endpoint: {0}")]
    NotLocal(#[from] LocalUrlError),
}

/// Port assumed when an endpoint has none (plain `http://`, like reqwest).
const DEFAULT_HTTP_PORT: u16 = 80;

/// Validates `endpoint` as a loopback `http://` URL and returns the base
/// to connect to, with the host replaced by its loopback IP literal (so no
/// DNS lookup can redirect it) and without a trailing `/`.
pub fn local_base(endpoint: &str) -> Result<String, LlmError> {
    let l = LocalUrl::parse(endpoint, DEFAULT_HTTP_PORT)?;
    let host = match l.ip {
        std::net::IpAddr::V4(v4) => v4.to_string(),
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
    };
    Ok(format!("http://{host}:{}{}", l.port, l.base_path))
}

impl From<reqwest::Error> for LlmError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_connect() {
            LlmError::Unreachable(e.to_string())
        } else if e.is_timeout() {
            LlmError::Timeout(e.to_string())
        } else {
            LlmError::Http(e)
        }
    }
}

impl LlmError {
    /// `true` when the server is not (yet) answering at all; the UI shows
    /// an in-character "my brain is napping" line instead of an error.
    pub fn is_down(&self) -> bool {
        match self {
            LlmError::Unreachable(_) => true,
            // llama-server answers 503 "Loading model" while it starts.
            LlmError::Status { status, .. } => *status == 503,
            _ => false,
        }
    }
}

/// Which wire protocol to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Ollama,
    OpenAiCompat,
}

/// Sampling options for one request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChatOptions {
    pub temperature: f32,
    pub max_tokens: u32,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            temperature: lillith_setup::modelfile::LILY_TEMPERATURE,
            max_tokens: 320,
        }
    }
}

/// Client for both backends (streaming for the OpenAI-compatible one).
#[derive(Debug, Clone)]
pub struct LlmClient {
    pub flavor: Flavor,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    http: reqwest::Client,
}

impl LlmClient {
    pub fn new(flavor: Flavor, base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self::with_timeout(flavor, base_url, model, Duration::from_secs(120))
    }

    /// Like [`LlmClient::new`] with a whole-request `timeout` (the connect
    /// timeout is 3 s so a missing server is reported quickly).
    pub fn with_timeout(
        flavor: Flavor,
        base_url: impl Into<String>,
        model: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        let base_url = base_url.into();
        if let Err(e) = local_base(&base_url) {
            tracing::warn!("LLM endpoint refused, chat stays offline: {e}");
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(timeout)
            // Never via an env proxy, never follow a redirect off the box.
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            flavor,
            base_url,
            model: model.into(),
            api_key: None,
            http,
        }
    }

    /// The validated loopback base, re-checked before every request
    /// (`base_url` is a public field and may change after construction).
    fn base(&self) -> Result<String, LlmError> {
        local_base(&self.base_url)
    }

    fn post(&self, url: String) -> reqwest::RequestBuilder {
        let r = self.http.post(url);
        match &self.api_key {
            Some(k) if !k.is_empty() => r.bearer_auth(k),
            _ => r,
        }
    }

    /// Request body for the OpenAI-compatible endpoint.
    pub fn openai_body(
        &self,
        messages: &[Message],
        opts: ChatOptions,
        stream: bool,
    ) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "messages": messages,
            "temperature": opts.temperature,
            "max_tokens": opts.max_tokens,
            "stream": stream,
        })
    }

    /// Sends a chat and returns the full reply text.
    pub async fn chat(&self, messages: &[Message]) -> Result<String, LlmError> {
        let base = self.base()?;
        let (url, body) = match self.flavor {
            Flavor::Ollama => (
                format!("{base}/api/chat"),
                serde_json::json!({ "model": self.model, "messages": messages, "stream": false }),
            ),
            Flavor::OpenAiCompat => (
                format!("{base}/v1/chat/completions"),
                self.openai_body(messages, ChatOptions::default(), false),
            ),
        };
        let resp = self.post(url).json(&body).send().await?;
        let resp = check_status(resp).await?;
        let v: serde_json::Value = resp.json().await?;
        let text = match self.flavor {
            Flavor::Ollama => v.pointer("/message/content"),
            Flavor::OpenAiCompat => v.pointer("/choices/0/message/content"),
        };
        text.and_then(|t| t.as_str())
            .map(str::to_owned)
            .ok_or_else(|| LlmError::Response(v.to_string()))
    }

    /// Streams a reply from `/v1/chat/completions` (`"stream": true`),
    /// calling `on_token` with every content delta as it arrives. Returns
    /// the whole reply. (Ollama is answered in one piece.)
    pub async fn chat_stream<F: FnMut(&str)>(
        &self,
        messages: &[Message],
        opts: ChatOptions,
        mut on_token: F,
    ) -> Result<String, LlmError> {
        if self.flavor == Flavor::Ollama {
            let all = self.chat(messages).await?;
            on_token(&all);
            return Ok(all);
        }
        let url = format!("{}/v1/chat/completions", self.base()?);
        let resp = self
            .post(url)
            .header("Accept", "text/event-stream")
            .json(&self.openai_body(messages, opts, true))
            .send()
            .await?;
        let resp = check_status(resp).await?;
        let mut parser = SseParser::default();
        let mut out = String::new();
        let mut stream = resp.bytes_stream();
        'read: while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            for ev in parser.push(&chunk) {
                match ev {
                    SseEvent::Delta(t) => {
                        out.push_str(&t);
                        on_token(&t);
                    }
                    SseEvent::Done => break 'read,
                    SseEvent::Error(e) => return Err(LlmError::Response(e)),
                }
            }
        }
        Ok(out)
    }

    /// `GET /health` (llama-server): `Ok(true)` once the model is loaded,
    /// `Ok(false)` while it is still loading (503).
    pub async fn health(&self) -> Result<bool, LlmError> {
        let r = self
            .http
            .get(format!("{}/health", self.base()?))
            .send()
            .await?;
        Ok(r.status().is_success())
    }
}

async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, LlmError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    Err(LlmError::Status {
        status: status.as_u16(),
        body: body.chars().take(300).collect(),
    })
}

/// One parsed streaming event.
#[derive(Debug, Clone, PartialEq)]
pub enum SseEvent {
    /// A content delta (`choices[0].delta.content`).
    Delta(String),
    /// `data: [DONE]` or a chunk with a `finish_reason`.
    Done,
    /// An `{"error": …}` payload inside the stream.
    Error(String),
}

/// Incremental parser for the OpenAI streaming format. Bytes can arrive
/// split anywhere (mid-line, mid-UTF-8 sequence); only complete lines are
/// decoded.
#[derive(Debug, Default)]
pub struct SseParser {
    buf: Vec<u8>,
    done: bool,
}

impl SseParser {
    /// Feeds raw bytes; returns the events completed by them.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if self.done {
                continue;
            }
            if let Some(ev) = parse_line(line) {
                if ev == SseEvent::Done {
                    self.done = true;
                }
                out.push(ev);
            }
        }
        out
    }
}

fn parse_line(line: &str) -> Option<SseEvent> {
    // Comments (`: keep-alive`), `event:`/`id:` fields and blank
    // separators carry nothing we need.
    let data = line.strip_prefix("data:")?.trim_start();
    if data == "[DONE]" {
        return Some(SseEvent::Done);
    }
    let v: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return None,
    };
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| err.to_string());
        return Some(SseEvent::Error(msg));
    }
    let choice = v.pointer("/choices/0")?;
    if let Some(t) = choice.pointer("/delta/content").and_then(|t| t.as_str()) {
        if !t.is_empty() {
            return Some(SseEvent::Delta(t.to_owned()));
        }
    }
    match choice.get("finish_reason") {
        Some(serde_json::Value::String(_)) => Some(SseEvent::Done),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(t: &str) -> String {
        format!(
            "data: {}\n\n",
            serde_json::json!({"choices":[{"index":0,"delta":{"content":t},"finish_reason":null}]})
        )
    }

    #[test]
    fn parses_deltas_split_anywhere() {
        let stream = format!(
            ": ping\n\ndata: {}\n\n{}{}data: [DONE]\n\n",
            serde_json::json!({"choices":[{"delta":{"role":"assistant"}}]}),
            chunk("Hey "),
            chunk("cutie 💜"),
        );
        let bytes = stream.as_bytes();
        // Every possible split point, including inside the emoji.
        for cut in 0..bytes.len() {
            let mut p = SseParser::default();
            let mut evs = p.push(&bytes[..cut]);
            evs.extend(p.push(&bytes[cut..]));
            assert_eq!(
                evs,
                vec![
                    SseEvent::Delta("Hey ".into()),
                    SseEvent::Delta("cutie 💜".into()),
                    SseEvent::Done
                ],
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn finish_reason_and_errors() {
        let mut p = SseParser::default();
        let fin = format!(
            "data: {}\r\n",
            serde_json::json!({"choices":[{"delta":{},"finish_reason":"stop"}]})
        );
        assert_eq!(p.push(fin.as_bytes()), vec![SseEvent::Done]);
        // Nothing after Done.
        assert!(p.push(chunk("late").as_bytes()).is_empty());
        let mut p = SseParser::default();
        let err = "data: {\"error\":{\"message\":\"context full\"}}\n";
        assert_eq!(
            p.push(err.as_bytes()),
            vec![SseEvent::Error("context full".into())]
        );
    }

    #[test]
    fn body_has_sampling_options() {
        let c = LlmClient::new(Flavor::OpenAiCompat, "http://x/", "m");
        let b = c.openai_body(
            &[Message::user("hi")],
            ChatOptions {
                temperature: 0.3,
                max_tokens: 64,
            },
            true,
        );
        assert_eq!(b["stream"], true);
        assert_eq!(b["max_tokens"], 64);
        assert!((b["temperature"].as_f64().unwrap() - 0.3).abs() < 1e-6);
        assert_eq!(b["messages"][0]["role"], "user");
    }

    #[test]
    fn loopback_endpoints_are_accepted() {
        for (u, want) in [
            ("http://127.0.0.1:8080", "http://127.0.0.1:8080"),
            ("http://127.0.0.1:9090/", "http://127.0.0.1:9090"),
            ("http://localhost:8080", "http://127.0.0.1:8080"),
            ("http://LOCALHOST:11434/v1/", "http://127.0.0.1:11434/v1"),
            ("http://[::1]:8081", "http://[::1]:8081"),
            ("http://127.1.2.3", "http://127.1.2.3:80"),
        ] {
            assert_eq!(local_base(u).unwrap_or_else(|e| panic!("{u}: {e}")), want);
        }
    }

    #[test]
    fn default_endpoint_is_local_and_unchanged() {
        let d = lillith_core::config::ChatConfig::default().endpoint;
        assert_eq!(d, "http://127.0.0.1:8080");
        assert_eq!(local_base(&d).unwrap(), "http://127.0.0.1:8080");
    }

    #[test]
    fn non_loopback_hosts_are_rejected() {
        for u in [
            "http://192.168.1.20:8080",
            "http://10.0.0.5:8080",
            "http://0.0.0.0:8080",
            "http://8.8.8.8",
            "http://example.com:8080",
            "http://api.openai.com/v1",
            "http://[2001:db8::1]:8080",
        ] {
            assert!(
                matches!(
                    local_base(u),
                    Err(LlmError::NotLocal(LocalUrlError::NotLoopback { .. }))
                ),
                "{u}"
            );
        }
    }

    #[test]
    fn non_http_schemes_are_rejected() {
        for u in [
            "https://127.0.0.1:8080",
            "ftp://localhost",
            "ws://127.0.0.1:8080",
        ] {
            assert!(
                matches!(
                    local_base(u),
                    Err(LlmError::NotLocal(LocalUrlError::Scheme { .. }))
                ),
                "{u}"
            );
        }
        assert!(matches!(
            local_base("http://evil.com@127.0.0.1:8080"),
            Err(LlmError::NotLocal(LocalUrlError::Userinfo { .. }))
        ));
        assert!(matches!(local_base(""), Err(LlmError::NotLocal(_))));
    }

    #[test]
    fn look_alike_hosts_are_rejected() {
        for u in [
            "http://localhost.evil.com:8080",
            "http://127.0.0.1.nip.io:8080",
            "http://localhost@evil.com:8080",
        ] {
            assert!(matches!(local_base(u), Err(LlmError::NotLocal(_))), "{u}");
        }
    }

    /// A non-local endpoint fails with `NotLocal` before any connection is
    /// attempted (an attempt would end as `Unreachable` / `Timeout`),
    /// including when `base_url` is changed after construction.
    #[tokio::test]
    async fn requests_to_non_local_endpoints_are_never_sent() {
        let c = LlmClient::with_timeout(
            Flavor::OpenAiCompat,
            "http://10.255.255.1:8080",
            "m",
            Duration::from_secs(30),
        );
        let t0 = std::time::Instant::now();
        let r = c
            .chat_stream(&[Message::user("hi")], ChatOptions::default(), |_| {})
            .await;
        assert!(matches!(r, Err(LlmError::NotLocal(_))), "{r:?}");
        assert!(matches!(
            c.chat(&[Message::user("hi")]).await,
            Err(LlmError::NotLocal(_))
        ));
        assert!(matches!(c.health().await, Err(LlmError::NotLocal(_))));
        assert!(t0.elapsed() < Duration::from_secs(1));
        assert!(!r.unwrap_err().is_down());

        let mut c = LlmClient::new(Flavor::Ollama, "http://127.0.0.1:11434", "m");
        c.base_url = "https://example.com".into();
        assert!(matches!(
            c.chat(&[Message::user("hi")]).await,
            Err(LlmError::NotLocal(_))
        ));
    }
}
