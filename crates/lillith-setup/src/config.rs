//! The `[llm]` table of `~/.config/lillith/config.toml`.
//!
//! Read and written with `toml_edit`, so every other table, key, comment and
//! the formatting of the file are preserved. `server_url` and `ollama_host`
//! are validated as loopback on read *and* write ([`crate::local`]): a
//! remote host is an error, never silently used.

use crate::detect::{DEFAULT_LLAMA_SERVER_URL, DEFAULT_OLLAMA_HOST};
use crate::local::{LocalUrl, LocalUrlError};
use crate::template::ChatTemplate;
use serde::Serialize;
use std::path::{Path, PathBuf};
use toml_edit::{value, Array, DocumentMut, Item, Table};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    LlamaServer,
    Ollama,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::LlamaServer => "llama-server",
            Backend::Ollama => "ollama",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "llama-server" | "llama.cpp" | "llamacpp" | "llama-cpp" => Some(Backend::LlamaServer),
            "ollama" => Some(Backend::Ollama),
            _ => None,
        }
    }
}

/// The `[llm]` settings. Unset keys take these defaults.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LlmConfig {
    pub backend: Backend,
    /// Absolute path of the chosen `.gguf` (empty = not chosen yet).
    pub gguf_path: String,
    /// llama-server base URL (loopback only); `/v1/chat/completions` is appended
    /// by the chat client, like `[chat] endpoint`.
    pub server_url: String,
    /// Ollama base URL (loopback only).
    pub ollama_host: String,
    /// Model name created in Ollama from the Modelfile.
    pub ollama_model: String,
    pub temperature: f32,
    pub ctx_size: u32,
    /// llama-server `--chat-template` name (`chatml`, `llama3`, ...).
    pub chat_template: String,
    /// Path to llama-server (empty = find on PATH).
    pub llama_server_bin: String,
    /// Where Lily's Modelfile lives (empty = default path).
    pub modelfile_path: String,
    /// Empty = built-in Lily persona.
    pub system_prompt: String,
    /// Extra dirs for the GGUF scanner.
    pub scan_paths: Vec<String>,
    /// CPU threads for llama-server `-t` (0 = auto).
    pub threads: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            backend: Backend::LlamaServer,
            gguf_path: String::new(),
            server_url: DEFAULT_LLAMA_SERVER_URL.into(),
            ollama_host: DEFAULT_OLLAMA_HOST.into(),
            ollama_model: "lily".into(),
            temperature: 0.8,
            ctx_size: 4096,
            chat_template: "chatml".into(),
            llama_server_bin: String::new(),
            modelfile_path: String::new(),
            system_prompt: String::new(),
            scan_paths: Vec::new(),
            threads: 0,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: TOML parse error: {source}")]
    Parse { path: PathBuf, source: toml_edit::TomlError },
    #[error("[llm] {key}: {source}")]
    NotLocal { key: &'static str, source: LocalUrlError },
    #[error("[llm] {key}: {msg}")]
    Invalid { key: &'static str, msg: String },
}

impl LlmConfig {
    /// Checks the local-only rule and value ranges.
    pub fn validate(&self) -> Result<(), ConfigError> {
        LocalUrl::parse(&self.server_url, 8080)
            .map_err(|source| ConfigError::NotLocal { key: "server_url", source })?;
        LocalUrl::parse(&self.ollama_host, 11434)
            .map_err(|source| ConfigError::NotLocal { key: "ollama_host", source })?;
        if !(0.0..=2.0).contains(&self.temperature) {
            return Err(ConfigError::Invalid { key: "temperature", msg: format!("{} not in 0..=2", self.temperature) });
        }
        if !(256..=131072).contains(&self.ctx_size) {
            return Err(ConfigError::Invalid { key: "ctx_size", msg: format!("{} not in 256..=131072", self.ctx_size) });
        }
        if ChatTemplate::from_name(&self.chat_template).is_none() && self.chat_template.trim().is_empty() {
            return Err(ConfigError::Invalid { key: "chat_template", msg: "empty".into() });
        }
        Ok(())
    }

    /// Validated llama-server URL.
    pub fn server(&self) -> Result<LocalUrl, ConfigError> {
        LocalUrl::parse(&self.server_url, 8080).map_err(|source| ConfigError::NotLocal { key: "server_url", source })
    }

    /// Validated Ollama URL.
    pub fn ollama(&self) -> Result<LocalUrl, ConfigError> {
        LocalUrl::parse(&self.ollama_host, 11434).map_err(|source| ConfigError::NotLocal { key: "ollama_host", source })
    }

    /// Reads `[llm]` from a parsed document (missing table = defaults).
    pub fn from_document(doc: &DocumentMut) -> Result<Self, ConfigError> {
        let mut c = Self::default();
        let Some(t) = doc.get("llm").and_then(Item::as_table_like) else {
            return Ok(c);
        };
        let s = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let n = |k: &str| t.get(k).and_then(|v| v.as_integer().map(|i| i as f64).or_else(|| v.as_float()));
        if let Some(b) = s("backend") {
            c.backend = Backend::parse(&b).ok_or(ConfigError::Invalid {
                key: "backend",
                msg: format!("`{b}` (use \"llama-server\" or \"ollama\")"),
            })?;
        }
        if let Some(v) = s("gguf_path") { c.gguf_path = v; }
        if let Some(v) = s("server_url") { c.server_url = v; }
        if let Some(v) = s("ollama_host") { c.ollama_host = v; }
        if let Some(v) = s("ollama_model") { c.ollama_model = v; }
        if let Some(v) = n("temperature") { c.temperature = v as f32; }
        // `num_ctx` is the old Aether key; `ctx_size` wins.
        if let Some(v) = n("ctx_size").or_else(|| n("num_ctx")) { c.ctx_size = v.max(0.0) as u32; }
        if let Some(v) = s("chat_template") { c.chat_template = v; }
        if let Some(v) = s("llama_server_bin") { c.llama_server_bin = v; }
        if let Some(v) = s("modelfile_path") { c.modelfile_path = v; }
        if let Some(v) = s("system_prompt") { c.system_prompt = v; }
        if let Some(v) = n("threads") { c.threads = v.max(0.0) as u32; }
        if let Some(a) = t.get("scan_paths").and_then(|v| v.as_array()) {
            c.scan_paths = a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        }
        c.validate()?;
        Ok(c)
    }

    /// Writes into `doc`'s `[llm]` table, keeping unknown keys, other tables
    /// and comments. Refuses (doc untouched) if validation fails.
    pub fn write_to_document(&self, doc: &mut DocumentMut) -> Result<(), ConfigError> {
        self.validate()?;
        if !doc.contains_key("llm") || doc["llm"].as_table().is_none() {
            doc["llm"] = Item::Table(Table::new());
        }
        let t = doc["llm"].as_table_mut().expect("llm is a table");
        set(t, "backend", value(self.backend.as_str()));
        set(t, "gguf_path", value(self.gguf_path.clone()));
        set(t, "server_url", value(self.server_url.clone()));
        set(t, "ollama_host", value(self.ollama_host.clone()));
        set(t, "ollama_model", value(self.ollama_model.clone()));
        set(t, "temperature", value(round3(self.temperature)));
        set(t, "ctx_size", value(self.ctx_size as i64));
        set(t, "chat_template", value(self.chat_template.clone()));
        set(t, "llama_server_bin", value(self.llama_server_bin.clone()));
        set(t, "modelfile_path", value(self.modelfile_path.clone()));
        set(t, "system_prompt", value(self.system_prompt.clone()));
        set(t, "threads", value(self.threads as i64));
        let mut a = Array::new();
        for p in &self.scan_paths {
            a.push(p.as_str());
        }
        set(t, "scan_paths", value(a));
        t.remove("num_ctx"); // migrated to ctx_size
        Ok(())
    }

    /// Optional bridge for `feat/chat-bubble-llama`: points `[chat]` and
    /// `[chat.server]` at this choice (endpoint, model, temperature,
    /// model_path, ctx_size, `--chat-template`). Other `[chat]` keys are kept.
    pub fn sync_chat_tables(&self, doc: &mut DocumentMut) -> Result<(), ConfigError> {
        self.validate()?;
        if doc.get("chat").and_then(Item::as_table).is_none() {
            doc["chat"] = Item::Table(Table::new());
        }
        let chat = doc["chat"].as_table_mut().expect("chat is a table");
        match self.backend {
            Backend::LlamaServer => {
                set(chat, "endpoint", value(self.server()?.base()));
                set(chat, "model", value("local"));
            }
            Backend::Ollama => {
                // Ollama's OpenAI-compatible API lives under the same host.
                set(chat, "endpoint", value(self.ollama()?.base()));
                set(chat, "model", value(self.ollama_model.clone()));
            }
        }
        set(chat, "temperature", value(round3(self.temperature)));
        if !self.system_prompt.trim().is_empty() {
            set(chat, "system_prompt", value(self.system_prompt.clone()));
        }
        if self.backend == Backend::LlamaServer && !self.gguf_path.is_empty() {
            if chat.get("server").and_then(Item::as_table).is_none() {
                chat["server"] = Item::Table(Table::new());
            }
            let srv = chat["server"].as_table_mut().expect("server is a table");
            if !self.llama_server_bin.is_empty() {
                set(srv, "binary", value(self.llama_server_bin.clone()));
            }
            set(srv, "model_path", value(self.gguf_path.clone()));
            set(srv, "ctx_size", value(self.ctx_size as i64));
            if self.threads > 0 {
                set(srv, "threads", value(self.threads as i64));
            }
            let mut a = Array::new();
            a.push("--chat-template");
            a.push(self.chat_template.as_str());
            set(srv, "extra_args", value(a));
        }
        Ok(())
    }
}

/// Sets `key`, keeping the existing key's decor (comments) if present.
fn set(t: &mut Table, key: &str, mut v: Item) {
    if let (Some(old), Some(new)) = (t.get(key).and_then(Item::as_value), v.as_value_mut()) {
        *new.decor_mut() = old.decor().clone();
    }
    // Replace in place so the key (and comments above it) stays put.
    match t.get_mut(key) {
        Some(slot) => *slot = v,
        None => {
            t.insert(key, v);
        }
    }
}

fn round3(v: f32) -> f64 {
    ((v as f64) * 1000.0).round() / 1000.0
}

/// Loads the document at `path` (missing file = empty document).
pub fn load_document(path: &Path) -> Result<DocumentMut, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(s) => s.parse::<DocumentMut>().map_err(|source| ConfigError::Parse { path: path.into(), source }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(source) => Err(ConfigError::Io { path: path.into(), source }),
    }
}

/// Reads `[llm]` from `path` (defaults if the file or table is missing).
pub fn load(path: &Path) -> Result<LlmConfig, ConfigError> {
    LlmConfig::from_document(&load_document(path)?)
}

/// Writes `[llm]` (and optionally the `[chat]` bridge) into `path`
/// atomically, creating the file and directory if needed.
pub fn save(path: &Path, cfg: &LlmConfig, sync_chat: bool) -> Result<(), ConfigError> {
    let mut doc = load_document(path)?;
    cfg.write_to_document(&mut doc)?;
    if sync_chat {
        cfg.sync_chat_tables(&mut doc)?;
    }
    let io = |source| ConfigError::Io { path: path.into(), source };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string()).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXISTING: &str = r#"# Lillith config
mode = "stay"

[overlay]
x = 120 # remembered position
y = 900

[llm]
# my comment about the model
gguf_path = "/old.gguf"
num_ctx = 2048
custom_key = "keep me"

[chat]
enabled = true
max_tokens = 320
"#;

    #[test]
    fn reads_defaults_when_missing() {
        let doc: DocumentMut = "mode = \"stay\"".parse().unwrap();
        assert_eq!(LlmConfig::from_document(&doc).unwrap(), LlmConfig::default());
    }

    #[test]
    fn reads_and_migrates_num_ctx() {
        let doc: DocumentMut = EXISTING.parse().unwrap();
        let c = LlmConfig::from_document(&doc).unwrap();
        assert_eq!(c.gguf_path, "/old.gguf");
        assert_eq!(c.ctx_size, 2048);
        assert_eq!(c.backend, Backend::LlamaServer);
    }

    #[test]
    fn write_preserves_other_tables_and_comments() {
        let mut doc: DocumentMut = EXISTING.parse().unwrap();
        let c = LlmConfig {
            gguf_path: "/home/ko-terminal/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf".into(),
            ..Default::default()
        };
        c.write_to_document(&mut doc).unwrap();
        let out = doc.to_string();
        assert!(out.starts_with("# Lillith config\nmode = \"stay\"\n"), "{out}");
        assert!(out.contains("x = 120 # remembered position"));
        assert!(out.contains("# my comment about the model"));
        assert!(out.contains("custom_key = \"keep me\""));
        assert!(out.contains("max_tokens = 320"));
        assert!(out.contains("ctx_size = 4096"));
        assert!(!out.contains("num_ctx"));
        let back = LlmConfig::from_document(&out.parse().unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn remote_hosts_rejected_on_read() {
        for (k, v) in [
            ("server_url", "http://192.168.1.10:8080"),
            ("ollama_host", "http://ollama.example.com:11434"),
            ("server_url", "https://api.openai.com"),
        ] {
            let doc: DocumentMut = format!("[llm]\n{k} = \"{v}\"\n").parse().unwrap();
            let e = LlmConfig::from_document(&doc).unwrap_err();
            assert!(matches!(e, ConfigError::NotLocal { .. }), "{k}={v}: {e}");
            assert!(e.to_string().contains(k));
        }
    }

    #[test]
    fn remote_hosts_rejected_on_write_and_doc_untouched() {
        let mut doc: DocumentMut = EXISTING.parse().unwrap();
        let before = doc.to_string();
        let c = LlmConfig { ollama_host: "10.0.0.2:11434".into(), ..Default::default() };
        assert!(matches!(c.write_to_document(&mut doc), Err(ConfigError::NotLocal { key: "ollama_host", .. })));
        assert!(matches!(c.sync_chat_tables(&mut doc), Err(ConfigError::NotLocal { .. })));
        assert_eq!(doc.to_string(), before);
        let c = LlmConfig { server_url: "http://lily.example.org".into(), ..Default::default() };
        let p = std::env::temp_dir().join(format!("lillith-setup-remote-{}.toml", std::process::id()));
        assert!(save(&p, &c, false).is_err());
        assert!(!p.exists(), "nothing written for a remote host");
    }

    #[test]
    fn sync_chat_bridge() {
        let mut doc: DocumentMut = EXISTING.parse().unwrap();
        let c = LlmConfig { gguf_path: "/m/q.gguf".into(), llama_server_bin: "/usr/bin/llama-server".into(), ..Default::default() };
        c.sync_chat_tables(&mut doc).unwrap();
        let out = doc.to_string();
        assert!(out.contains("endpoint = \"http://127.0.0.1:8080\""), "{out}");
        assert!(out.contains("enabled = true"));
        assert!(out.contains("[chat.server]"));
        assert!(out.contains("model_path = \"/m/q.gguf\""));
        assert!(out.contains("extra_args = [\"--chat-template\", \"chatml\"]"));
        let o = LlmConfig { backend: Backend::Ollama, ..Default::default() };
        let mut doc = DocumentMut::new();
        o.sync_chat_tables(&mut doc).unwrap();
        assert!(doc.to_string().contains("endpoint = \"http://127.0.0.1:11434\""));
        assert!(doc.to_string().contains("model = \"lily\""));
    }

    #[test]
    fn save_and_load_file() {
        let dir = std::env::temp_dir().join(format!("lillith-setup-cfg-{}", std::process::id()));
        let p = dir.join("lillith/config.toml");
        let c = LlmConfig { backend: Backend::Ollama, temperature: 0.65, ..Default::default() };
        save(&p, &c, false).unwrap();
        assert_eq!(load(&p).unwrap(), c);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn floats_are_written_rounded() {
        for (t, want) in [(0.8f32, "temperature = 0.8\n"), (0.7, "temperature = 0.7\n"), (0.65, "temperature = 0.65\n"), (1.0, "temperature = 1.0\n"), (0.123456, "temperature = 0.123\n")] {
            let mut doc = DocumentMut::new();
            LlmConfig { temperature: t, ..Default::default() }.write_to_document(&mut doc).unwrap();
            let out = doc.to_string();
            assert!(out.contains(want), "{t}: {out}");
            assert!(!out.contains("0000"), "{t}: {out}");
        }
        let mut doc = DocumentMut::new();
        LlmConfig { temperature: 0.8, ..Default::default() }.sync_chat_tables(&mut doc).unwrap();
        assert!(doc.to_string().contains("temperature = 0.8\n"));
    }
}
