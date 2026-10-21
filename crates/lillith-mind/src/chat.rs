//! A chat conversation with Lillith over a local llama-server.
//!
//! [`ChatSession`] keeps the history, builds the request (system prompt +
//! the last `history_turns` turns + the new message), streams the reply and
//! turns every failure into an in-character line so the bubble never shows
//! a raw error. The history can be persisted as JSON between bubble openings.

use crate::llm::{ChatOptions, Flavor, LlmClient, LlmError, Message};
use crate::persona;
use lillith_core::config::ChatConfig;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How a turn ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The model answered (the full reply).
    Reply(String),
    /// The model could not answer; `line` is the in-character fallback
    /// (already streamed to the callback), `error` the technical reason.
    Fallback { line: String, error: String },
}

impl Outcome {
    pub fn text(&self) -> &str {
        match self {
            Outcome::Reply(t) => t,
            Outcome::Fallback { line, .. } => line,
        }
    }
}

/// One conversation.
#[derive(Debug)]
pub struct ChatSession {
    client: Option<LlmClient>,
    system_prompt: String,
    opts: ChatOptions,
    history_turns: usize,
    history: Vec<Message>,
    save_path: Option<PathBuf>,
    salt: usize,
}

/// Most messages kept on disk.
const MAX_SAVED: usize = 200;

impl ChatSession {
    /// Session for `cfg`. `offline` (= `--no-llm`) answers every message
    /// with [`persona::OFFLINE_LINE`] without touching the network.
    pub fn new(cfg: &ChatConfig, offline: bool) -> Self {
        if !offline {
            // Checked here (config read) and again before every request.
            if let Err(e) = crate::llm::local_base(&cfg.endpoint) {
                tracing::warn!(endpoint = %cfg.endpoint, "[chat] endpoint refused: {e}");
            }
        }
        let client = (!offline).then(|| {
            let mut c = LlmClient::with_timeout(
                Flavor::OpenAiCompat,
                cfg.endpoint.clone(),
                cfg.model.clone(),
                Duration::from_secs_f32(cfg.timeout_s.max(1.0)),
            );
            if !cfg.api_key.is_empty() {
                c.api_key = Some(cfg.api_key.clone());
            }
            c
        });
        let system_prompt = if cfg.system_prompt.trim().is_empty() {
            persona::DEFAULT_SYSTEM_PROMPT.to_string()
        } else {
            cfg.system_prompt.clone()
        };
        Self {
            client,
            system_prompt,
            opts: ChatOptions {
                temperature: cfg.temperature,
                max_tokens: cfg.max_tokens.max(1),
            },
            history_turns: cfg.history_turns,
            history: Vec::new(),
            save_path: None,
            salt: std::process::id() as usize,
        }
    }

    /// Loads (if present) and keeps saving the history at `path`.
    pub fn with_history_file(mut self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        if let Ok(s) = std::fs::read_to_string(&path) {
            match serde_json::from_str::<Vec<Message>>(&s) {
                Ok(h) => self.history = h,
                Err(e) => tracing::warn!("{}: {e}; starting a fresh chat", path.display()),
            }
        }
        self.save_path = Some(path);
        self
    }

    pub fn history(&self) -> &[Message] {
        &self.history
    }

    pub fn clear(&mut self) {
        self.history.clear();
        self.save();
    }

    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// The messages sent for a new `user` text.
    pub fn request_messages(&self, user: &str) -> Vec<Message> {
        let keep = self.history_turns.saturating_mul(2);
        let start = self.history.len().saturating_sub(keep);
        let mut m = Vec::with_capacity(keep + 2);
        m.push(Message::system(self.system_prompt.clone()));
        m.extend(
            self.history[start..]
                .iter()
                .filter(|h| h.role == "user" || h.role == "assistant")
                .cloned(),
        );
        m.push(Message::user(user));
        m
    }

    /// Sends `user`, streaming the reply through `on_token`. Failures are
    /// streamed too, as one in-character line, and are not stored in the
    /// history (so the next try resends the same context).
    pub async fn send<F: FnMut(&str)>(&mut self, user: &str, mut on_token: F) -> Outcome {
        let user = user.trim();
        let messages = self.request_messages(user);
        self.salt = self.salt.wrapping_add(1);
        let result = match &self.client {
            None => {
                on_token(persona::OFFLINE_LINE);
                return Outcome::Fallback {
                    line: persona::OFFLINE_LINE.into(),
                    error: "chat disabled (--no-llm)".into(),
                };
            }
            Some(c) => c.chat_stream(&messages, self.opts, &mut on_token).await,
        };
        match result {
            Ok(reply) if !reply.trim().is_empty() => {
                let reply = reply.trim().to_string();
                self.history.push(Message::user(user));
                self.history.push(Message::assistant(reply.clone()));
                self.save();
                Outcome::Reply(reply)
            }
            Ok(_) => self.fallback(LlmError::Response("empty reply".into()), &mut on_token),
            Err(e) => self.fallback(e, &mut on_token),
        }
    }

    fn fallback<F: FnMut(&str)>(&self, e: LlmError, on_token: &mut F) -> Outcome {
        let lines = match &e {
            LlmError::Status { status: 503, .. } => persona::SERVER_LOADING_LINES,
            e if e.is_down() => persona::SERVER_DOWN_LINES,
            _ => persona::SERVER_ERROR_LINES,
        };
        let line = persona::pick(lines, self.salt).to_string();
        tracing::warn!("chat: {e}");
        on_token(&line);
        Outcome::Fallback {
            line,
            error: e.to_string(),
        }
    }

    fn save(&self) {
        let Some(path) = &self.save_path else { return };
        let start = self.history.len().saturating_sub(MAX_SAVED);
        if let Err(e) = write_json(path, &self.history[start..]) {
            tracing::warn!("could not save chat history to {}: {e}", path.display());
        }
    }
}

fn write_json(path: &Path, h: &[Message]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(h)?)?;
    std::fs::rename(tmp, path)
}

/// `$XDG_DATA_HOME/lillith/chat_history.json`, else
/// `~/.local/share/lillith/chat_history.json`.
pub fn default_history_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("lillith/chat_history.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A one-shot fake llama-server: answers each connection with `reply`
    /// (raw HTTP) and hands back the request it got.
    async fn fake_server(
        replies: Vec<String>,
    ) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            for reply in replies {
                let (mut s, _) = l.accept().await.unwrap();
                let mut req = Vec::new();
                let mut buf = [0u8; 8192];
                // Read headers + body (Content-Length).
                loop {
                    let n = s.read(&mut buf).await.unwrap();
                    req.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&req).to_string();
                    if let Some(h) = text.find("\r\n\r\n") {
                        let len = text[..h]
                            .lines()
                            .find_map(|l| {
                                let l = l.to_ascii_lowercase();
                                l.strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if req.len() >= h + 4 + len {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                tx.send(String::from_utf8_lossy(&req).to_string()).unwrap();
                s.write_all(reply.as_bytes()).await.unwrap();
                s.shutdown().await.ok();
            }
        });
        (url, rx)
    }

    fn sse_reply(parts: &[&str]) -> String {
        let mut body = String::new();
        for p in parts {
            body.push_str(&format!(
                "data: {}\n\n",
                serde_json::json!({"choices":[{"delta":{"content":p},"finish_reason":null}]})
            ));
        }
        body.push_str("data: [DONE]\n\n");
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn cfg(url: &str) -> ChatConfig {
        ChatConfig {
            endpoint: url.into(),
            history_turns: 1,
            ..ChatConfig::default()
        }
    }

    #[tokio::test]
    async fn streams_and_keeps_history() {
        let (url, mut reqs) = fake_server(vec![
            sse_reply(&["Hey", " cutie", "~"]),
            sse_reply(&["Again", "?"]),
            sse_reply(&["Third"]),
        ])
        .await;
        let mut s = ChatSession::new(&cfg(&url), false);
        let mut toks = Vec::new();
        let out = s.send("hi", |t| toks.push(t.to_string())).await;
        assert_eq!(out, Outcome::Reply("Hey cutie~".into()));
        assert_eq!(toks, ["Hey", " cutie", "~"]);
        let r1 = reqs.recv().await.unwrap();
        assert!(r1.starts_with("POST /v1/chat/completions"));
        assert!(r1.contains("\"stream\":true"));
        assert!(r1.contains("You are Lillith"));
        s.send("again", |_| {}).await;
        s.send("third", |_| {}).await;
        // history_turns = 1: only the last turn goes along.
        let _ = reqs.recv().await.unwrap();
        let r3 = reqs.recv().await.unwrap();
        assert!(r3.contains("\"again\"") && r3.contains("Again?"));
        assert!(!r3.contains("\"hi\""));
        assert_eq!(s.history().len(), 6);
    }

    #[tokio::test]
    async fn server_down_is_in_character() {
        // Bind and drop: nothing listens there any more.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let mut s = ChatSession::new(&cfg(&format!("http://127.0.0.1:{port}")), false);
        let mut shown = String::new();
        let out = s.send("hello?", |t| shown.push_str(t)).await;
        match out {
            Outcome::Fallback { line, error } => {
                assert!(
                    persona::SERVER_DOWN_LINES.contains(&line.as_str()),
                    "{line} / {error}"
                );
                assert_eq!(shown, line);
            }
            o => panic!("expected fallback, got {o:?}"),
        }
        assert!(s.history().is_empty(), "failed turns are not stored");
    }

    #[tokio::test]
    async fn loading_503_and_offline() {
        let body = "{\"error\":{\"code\":503,\"message\":\"Loading model\"}}";
        let (url, _r) = fake_server(vec![format!(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )])
        .await;
        let mut s = ChatSession::new(&cfg(&url), false);
        let out = s.send("hi", |_| {}).await;
        assert!(
            persona::SERVER_LOADING_LINES.contains(&out.text()),
            "{out:?}"
        );

        let mut off = ChatSession::new(&ChatConfig::default(), true);
        assert_eq!(off.send("hi", |_| {}).await.text(), persona::OFFLINE_LINE);
    }

    #[tokio::test]
    async fn history_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("lillith-chat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("h.json");
        let (url, _r) = fake_server(vec![sse_reply(&["Saved"])]).await;
        let mut s = ChatSession::new(&cfg(&url), false).with_history_file(&path);
        s.send("remember me", |_| {}).await;
        let s2 = ChatSession::new(&cfg(&url), false).with_history_file(&path);
        assert_eq!(s2.history(), s.history());
        assert_eq!(s2.history()[1].content, "Saved");
        let custom = ChatConfig {
            system_prompt: "Be brief.".into(),
            ..ChatConfig::default()
        };
        assert_eq!(
            ChatSession::new(&custom, true).request_messages("x")[0].content,
            "Be brief."
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn non_local_endpoint_is_refused_in_character() {
        for e in [
            "http://203.0.113.7:8080",
            "https://127.0.0.1:8080",
            "http://localhost.evil.com:8080",
        ] {
            let mut s = ChatSession::new(&cfg(e), false);
            let mut shown = String::new();
            match s.send("hi", |t| shown.push_str(t)).await {
                Outcome::Fallback { line, error } => {
                    assert!(error.contains("non-local"), "{e}: {error}");
                    assert!(persona::SERVER_ERROR_LINES.contains(&line.as_str()));
                    assert_eq!(shown, line);
                }
                o => panic!("{e}: expected refusal, got {o:?}"),
            }
            assert!(s.history().is_empty());
        }
    }
}
