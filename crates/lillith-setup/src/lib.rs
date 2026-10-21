//! # lillith-setup
//!
//! UI-agnostic core of Lillith's first-run LLM setup wizard (PLAN.md §5.2,
//! milestone M6): detect a local runtime, pick a GGUF, customise Lily's
//! Ollama Modelfile and write the `[llm]` config table. A GUI panel (egui in
//! `lillith-panel`) or the `wizard` example drives it.
//!
//! ## Local-only guarantee
//!
//! Lillith's brain runs on this computer and your data never leaves the
//! local disk. This crate enforces that in code:
//!
//! * The only network I/O is a tiny std-only HTTP GET client ([`http`]) used
//!   for health / model-list probes, and it accepts only a [`local::LocalUrl`]:
//!   `127.0.0.0/8`, `::1` or `localhost` (mapped to 127.0.0.1 without DNS).
//! * `server_url` / `ollama_host` are validated as loopback when the config
//!   is read and when it is written ([`config::LlmConfig::validate`]); a
//!   remote host is a clear error, never silently used.
//! * No HTTP library dependency, no TLS, no telemetry, no update checks, no
//!   model downloads, no `ollama pull`. [`launch::ollama_create`] refuses a
//!   Modelfile whose `FROM` / `ADAPTER` is not an existing local file (a
//!   registry name would make Ollama download it) and pins `OLLAMA_HOST` to
//!   loopback for the child process.
//! * The llama-server launch line always binds to the loopback host.
//!
//! ## Flow
//!
//! ```no_run
//! use lillith_setup::*;
//! let rt = detect::detect(&detect::DetectOptions::default());
//! let found = scan::scan(&scan::ScanOptions::default());
//! let ranked = scan::recommend(&found.models, scan::total_ram_bytes());
//! let pick = &found.models[ranked[0].index];
//! let cfg = wizard::config_for_model(pick, rt.suggested_backend().unwrap_or(config::Backend::LlamaServer));
//! let mf = wizard::modelfile_for(pick, &cfg);
//! config::save(&paths::default_config_path().unwrap(), &cfg, false).unwrap();
//! std::fs::write(paths::default_modelfile_path().unwrap(), mf.render()).unwrap();
//! ```

pub mod check;
pub mod config;
pub mod detect;
pub mod gguf;
pub mod http;
pub mod launch;
pub mod local;
pub mod modelfile;
pub mod paths;
pub mod scan;
pub mod template;
pub mod validate;
pub mod wizard;

/// Alias of [`check`] (`lillith_setup::test::ping_server`, `test::test_chat`).
pub use self::check as test;

pub use check::{ping_server, test_chat, ServerHealth};
pub use config::{Backend, LlmConfig};
pub use detect::{detect, DetectHandle, DetectOptions, RuntimeReport};
pub use local::{LocalUrl, LocalUrlError};
pub use modelfile::{lily_modelfile, LilyParams, Modelfile};
pub use scan::{
    recommend, scan, scan_with_progress, CancelToken, ModelInfo, Recommendation, ScanOptions,
    ScanProgress, ScanReport,
};
pub use template::ChatTemplate;
pub use validate::{validate_all, FieldError};
