//! Chat template families: detection from GGUF metadata, the matching
//! llama-server `--chat-template` name, Ollama `TEMPLATE` and stop tokens.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatTemplate {
    /// `<|im_start|>role\n...<|im_end|>` — Dolphin 3.0, Qwen 2/2.5, Hermes.
    ChatMl,
    /// `<|start_header_id|>role<|end_header_id|>` — Meta Llama 3.x instruct.
    Llama3,
    /// `<start_of_turn>user` — Gemma.
    Gemma,
    /// `<|user|>...<|end|>` — Phi-3.
    Phi3,
    /// `[INST] ... [/INST]` — Mistral / Llama 2.
    Mistral,
}

impl ChatTemplate {
    pub const ALL: [ChatTemplate; 5] =
        [Self::ChatMl, Self::Llama3, Self::Gemma, Self::Phi3, Self::Mistral];

    /// Name used in config (`chat_template`) and for llama-server's
    /// `--chat-template` built-ins.
    pub fn llama_server_name(self) -> &'static str {
        match self {
            Self::ChatMl => "chatml",
            Self::Llama3 => "llama3",
            Self::Gemma => "gemma",
            Self::Phi3 => "phi3",
            Self::Mistral => "mistral-v1",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "chatml" => Some(Self::ChatMl),
            "llama3" | "llama-3" => Some(Self::Llama3),
            "gemma" => Some(Self::Gemma),
            "phi3" | "phi-3" => Some(Self::Phi3),
            "mistral" | "mistral-v1" | "llama2" => Some(Self::Mistral),
            _ => None,
        }
    }

    /// Detects the family from an embedded Jinja `tokenizer.chat_template`.
    pub fn from_jinja(t: &str) -> Option<Self> {
        if t.contains("<|im_start|>") {
            Some(Self::ChatMl)
        } else if t.contains("<|start_header_id|>") {
            Some(Self::Llama3)
        } else if t.contains("<start_of_turn>") {
            Some(Self::Gemma)
        } else if t.contains("<|user|>") && t.contains("<|end|>") {
            Some(Self::Phi3)
        } else if t.contains("[INST]") {
            Some(Self::Mistral)
        } else {
            None
        }
    }

    /// Best guess from the embedded template, then arch / name / file name.
    /// Falls back to ChatML (Dolphin and Qwen both use it).
    pub fn detect(jinja: Option<&str>, arch: Option<&str>, name_hint: &str) -> Self {
        if let Some(t) = jinja.and_then(Self::from_jinja) {
            return t;
        }
        let n = name_hint.to_ascii_lowercase();
        let a = arch.unwrap_or("").to_ascii_lowercase();
        if n.contains("dolphin") || n.contains("hermes") || a.starts_with("qwen") || n.contains("qwen") {
            Self::ChatMl
        } else if a.starts_with("gemma") || n.contains("gemma") {
            Self::Gemma
        } else if a.starts_with("phi3") || n.contains("phi-3") || n.contains("phi3") {
            Self::Phi3
        } else if n.contains("mistral") || n.contains("llama-2") || n.contains("llama2") {
            Self::Mistral
        } else if n.contains("llama-3") || n.contains("llama3") {
            Self::Llama3
        } else {
            Self::ChatMl
        }
    }

    /// Stop tokens for Ollama `PARAMETER stop`.
    pub fn stop_tokens(self) -> &'static [&'static str] {
        match self {
            Self::ChatMl => &["<|im_start|>", "<|im_end|>"],
            Self::Llama3 => &["<|start_header_id|>", "<|end_header_id|>", "<|eot_id|>"],
            Self::Gemma => &["<start_of_turn>", "<end_of_turn>"],
            Self::Phi3 => &["<|end|>", "<|user|>", "<|assistant|>"],
            Self::Mistral => &["[INST]", "[/INST]"],
        }
    }

    /// Ollama Go-template `TEMPLATE` body for this family.
    pub fn ollama_template(self) -> &'static str {
        match self {
            Self::ChatMl => "\
{{- if .System }}<|im_start|>system
{{ .System }}<|im_end|>
{{ end }}
{{- range .Messages }}<|im_start|>{{ .Role }}
{{ .Content }}<|im_end|>
{{ end }}<|im_start|>assistant
",
            Self::Llama3 => "\
{{- if .System }}<|start_header_id|>system<|end_header_id|>

{{ .System }}<|eot_id|>
{{- end }}
{{- range .Messages }}<|start_header_id|>{{ .Role }}<|end_header_id|>

{{ .Content }}<|eot_id|>
{{- end }}<|start_header_id|>assistant<|end_header_id|>

",
            Self::Gemma => "\
{{- range $i, $m := .Messages }}<start_of_turn>{{ if eq $m.Role \"assistant\" }}model{{ else }}user{{ end }}
{{ if and (eq $i 0) $.System }}{{ $.System }}

{{ end }}{{ $m.Content }}<end_of_turn>
{{ end }}<start_of_turn>model
",
            Self::Phi3 => "\
{{- if .System }}<|system|>
{{ .System }}<|end|>
{{ end }}
{{- range .Messages }}<|{{ .Role }}|>
{{ .Content }}<|end|>
{{ end }}<|assistant|>
",
            Self::Mistral => "\
{{- range $i, $m := .Messages }}
{{- if eq $m.Role \"user\" }}[INST] {{ if and (eq $i 0) $.System }}{{ $.System }}

{{ end }}{{ $m.Content }} [/INST]
{{- else }} {{ $m.Content }}</s>
{{- end }}
{{- end }}",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection() {
        assert_eq!(ChatTemplate::detect(Some("..<|im_start|>.."), Some("llama"), ""), ChatTemplate::ChatMl);
        assert_eq!(ChatTemplate::detect(None, Some("llama"), "dolphin3.0-llama3.2-3b-q4_k_m.gguf"), ChatTemplate::ChatMl);
        assert_eq!(ChatTemplate::detect(None, Some("qwen2"), "x.gguf"), ChatTemplate::ChatMl);
        assert_eq!(ChatTemplate::detect(None, Some("llama"), "Llama-3.2-3B-Instruct-Q4_K_M.gguf"), ChatTemplate::Llama3);
        assert_eq!(ChatTemplate::detect(Some("<|start_header_id|>"), None, ""), ChatTemplate::Llama3);
        assert_eq!(ChatTemplate::detect(None, Some("gemma2"), ""), ChatTemplate::Gemma);
    }

    #[test]
    fn names_round_trip() {
        for t in ChatTemplate::ALL {
            assert_eq!(ChatTemplate::from_name(t.llama_server_name()), Some(t));
        }
    }
}
