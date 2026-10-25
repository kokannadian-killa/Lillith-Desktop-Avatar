//! Glue for a wizard UI: turn a picked model into a config and a Modelfile.

use crate::config::{Backend, LlmConfig};
use crate::modelfile::{lily_modelfile, LilyParams, Modelfile, LILY_SYSTEM_PROMPT};
use crate::scan::ModelInfo;

/// Context the wizard proposes: 4096, capped by the model's training context.
pub fn suggested_ctx(m: &ModelInfo) -> u32 {
    m.context_length
        .map(|c| c.min(4096) as u32)
        .unwrap_or(4096)
        .max(512)
}

/// `[llm]` settings for a picked model (other fields default).
pub fn config_for_model(m: &ModelInfo, backend: Backend) -> LlmConfig {
    LlmConfig {
        backend,
        gguf_path: m.path.display().to_string(),
        ctx_size: suggested_ctx(m),
        chat_template: m.chat_template.llama_server_name().into(),
        ..LlmConfig::default()
    }
}

/// Lily's Modelfile for a picked model, using `cfg`'s temperature, context
/// and system prompt (empty = built-in persona).
pub fn modelfile_for(m: &ModelInfo, cfg: &LlmConfig) -> Modelfile {
    let p = LilyParams {
        template: m.chat_template,
        system_prompt: if cfg.system_prompt.trim().is_empty() {
            LILY_SYSTEM_PROMPT.into()
        } else {
            cfg.system_prompt.clone()
        },
        temperature: cfg.temperature,
        num_ctx: cfg.ctx_size,
        num_thread: cfg.threads,
        ..LilyParams::default()
    };
    lily_modelfile(&m.path.display().to_string(), &p)
}
