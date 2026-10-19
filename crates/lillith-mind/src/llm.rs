//! Local LLM clients. TODO(M7): streaming, context summary, offline lines.
//!
//! Backend choice is an open question (PLAN.md §6 Q4): Ollama
//! (`/api/chat`, default `http://127.0.0.1:11434`) or llama.cpp
//! `llama-server` (OpenAI-compatible `/v1/chat/completions`, `:8080`).

use serde::{Deserialize, Serialize};

/// A chat message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

/// Errors from an LLM backend.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("unexpected response: {0}")]
    Response(String),
}

/// Which wire protocol to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Ollama,
    OpenAiCompat,
}

/// Minimal non-streaming client for both backends.
#[derive(Debug, Clone)]
pub struct LlmClient {
    pub flavor: Flavor,
    pub base_url: String,
    pub model: String,
    http: reqwest::Client,
}

impl LlmClient {
    pub fn new(flavor: Flavor, base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            flavor,
            base_url: base_url.into(),
            model: model.into(),
            http: reqwest::Client::new(),
        }
    }

    /// Sends a chat and returns the full reply text.
    pub async fn chat(&self, messages: &[Message]) -> Result<String, LlmError> {
        let base = self.base_url.trim_end_matches('/');
        let (url, body) = match self.flavor {
            Flavor::Ollama => (
                format!("{base}/api/chat"),
                serde_json::json!({ "model": self.model, "messages": messages, "stream": false }),
            ),
            Flavor::OpenAiCompat => (
                format!("{base}/v1/chat/completions"),
                serde_json::json!({ "model": self.model, "messages": messages }),
            ),
        };
        let v: serde_json::Value = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let text = match self.flavor {
            Flavor::Ollama => v.pointer("/message/content"),
            Flavor::OpenAiCompat => v.pointer("/choices/0/message/content"),
        };
        text.and_then(|t| t.as_str())
            .map(str::to_owned)
            .ok_or_else(|| LlmError::Response(v.to_string()))
    }
}
