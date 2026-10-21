//! Commands the wizard can show or run: the llama-server launch line and
//! `ollama create lily -f <Modelfile>`.
//!
//! Both are local-only: llama-server is always bound to a loopback host,
//! and `ollama create` is refused unless `FROM` (and any `ADAPTER`) is an
//! existing local file, because a registry name in `FROM` would make Ollama
//! download it. Nothing here pulls, updates or uploads.

use crate::config::LlmConfig;
use crate::local::LocalUrl;
use crate::modelfile::{Instruction, Modelfile};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LaunchCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl LaunchCommand {
    /// Shell-quoted line (valid in fish, bash and zsh).
    pub fn to_shell_string(&self) -> String {
        std::iter::once(&self.program)
            .chain(&self.args)
            .map(|a| shell_quote(a))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn to_command(&self) -> Command {
        let mut c = Command::new(&self.program);
        c.args(&self.args);
        c
    }
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c))
    {
        s.to_string()
    } else {
        // Single quotes; `'` and `\` escaped the way fish requires (bash
        // treats `\'` differently, so close/reopen instead, which both accept).
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),
    #[error("no GGUF chosen yet ([llm] gguf_path is empty)")]
    NoModel,
    #[error("{0}")]
    NotLocalModel(String),
    #[error("modelfile: {0}")]
    Modelfile(#[from] crate::modelfile::ModelfileError),
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("`ollama create` failed ({status}): {output}")]
    Failed { status: String, output: String },
}

/// `llama-server -m <gguf> --host 127.0.0.1 --port 8080 -c 4096 --chat-template chatml`
/// for `cfg` (host/port from the validated loopback `server_url`).
pub fn llama_server_command(
    cfg: &LlmConfig,
    bin: Option<&Path>,
) -> Result<LaunchCommand, LaunchError> {
    if cfg.gguf_path.trim().is_empty() {
        return Err(LaunchError::NoModel);
    }
    let url: LocalUrl = cfg.server()?;
    let program = bin
        .map(|b| b.display().to_string())
        .or_else(|| (!cfg.llama_server_bin.is_empty()).then(|| cfg.llama_server_bin.clone()))
        .unwrap_or_else(|| "llama-server".into());
    let host = url.ip.to_string();
    let mut args = vec![
        "-m".into(),
        cfg.gguf_path.clone(),
        "--host".into(),
        host,
        "--port".into(),
        url.port.to_string(),
        "-c".into(),
        cfg.ctx_size.to_string(),
        "--chat-template".into(),
        cfg.chat_template.clone(),
    ];
    if cfg.threads > 0 {
        args.push("-t".into());
        args.push(cfg.threads.to_string());
    }
    Ok(LaunchCommand { program, args })
}

/// Checks that a Modelfile only references local files (so `ollama create`
/// cannot download anything). Relative paths resolve against `base_dir`.
pub fn ensure_local_modelfile(m: &Modelfile, base_dir: &Path) -> Result<(), LaunchError> {
    let from = m
        .from()
        .ok_or_else(|| LaunchError::NotLocalModel("Modelfile has no FROM".into()))?;
    let check = |what: &str, p: &str| -> Result<(), LaunchError> {
        let path = crate::paths::expand_tilde(p);
        let path = if path.is_absolute() {
            path
        } else {
            base_dir.join(path)
        };
        if path.is_file() {
            Ok(())
        } else {
            Err(LaunchError::NotLocalModel(format!(
                "{what} `{p}` is not an existing local file; refusing to run `ollama create` \
                 because Ollama would try to download it (Lillith is local-only)"
            )))
        }
    };
    check("FROM", from)?;
    for i in &m.instructions {
        if let Instruction::Adapter(a) = i {
            check("ADAPTER", a)?;
        }
    }
    Ok(())
}

/// The `ollama create <name> -f <modelfile>` command, after the local-file check.
pub fn ollama_create_command(
    ollama_bin: &Path,
    model_name: &str,
    modelfile_path: &Path,
    cfg: &LlmConfig,
) -> Result<LaunchCommand, LaunchError> {
    let src = std::fs::read_to_string(modelfile_path)
        .map_err(|e| LaunchError::Io(modelfile_path.into(), e))?;
    let m = Modelfile::parse(&src)?;
    ensure_local_modelfile(&m, modelfile_path.parent().unwrap_or(Path::new(".")))?;
    cfg.ollama()?; // host must be loopback
    Ok(LaunchCommand {
        program: ollama_bin.display().to_string(),
        args: vec![
            "create".into(),
            model_name.into(),
            "-f".into(),
            modelfile_path.display().to_string(),
        ],
    })
}

/// Runs `ollama create` (blocking) with `OLLAMA_HOST` pinned to the
/// validated loopback host. Returns the combined output.
pub fn ollama_create(
    ollama_bin: &Path,
    model_name: &str,
    modelfile_path: &Path,
    cfg: &LlmConfig,
) -> Result<String, LaunchError> {
    let cmd = ollama_create_command(ollama_bin, model_name, modelfile_path, cfg)?;
    let host = cfg.ollama()?;
    let out = cmd
        .to_command()
        .env("OLLAMA_HOST", format!("{}:{}", host.ip, host.port))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| LaunchError::Io(ollama_bin.into(), e))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(LaunchError::Failed {
            status: out.status.to_string(),
            output: text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llama_server_line() {
        let cfg = LlmConfig {
            gguf_path: "/workspace/models/qwen2.5-0.5b-instruct-q4_k_m.gguf".into(),
            ..Default::default()
        };
        let c = llama_server_command(&cfg, None).unwrap();
        assert_eq!(
            c.to_shell_string(),
            "llama-server -m /workspace/models/qwen2.5-0.5b-instruct-q4_k_m.gguf --host 127.0.0.1 --port 8080 -c 4096 --chat-template chatml"
        );
        let cfg = LlmConfig {
            gguf_path: "/home/ko/My Models/it's.gguf".into(),
            server_url: "http://localhost:9090".into(),
            ..Default::default()
        };
        let s = llama_server_command(&cfg, Some(Path::new("/opt/llama.cpp/bin/llama-server")))
            .unwrap()
            .to_shell_string();
        assert!(s.contains("'/home/ko/My Models/it'\"'\"'s.gguf'"), "{s}");
        assert!(s.contains("--host 127.0.0.1 --port 9090"));
    }

    #[test]
    fn llama_server_refuses_remote_and_missing_model() {
        let cfg = LlmConfig {
            gguf_path: "/m.gguf".into(),
            server_url: "http://0.0.0.0:8080".into(),
            ..Default::default()
        };
        assert!(matches!(
            llama_server_command(&cfg, None),
            Err(LaunchError::Config(_))
        ));
        assert!(matches!(
            llama_server_command(&LlmConfig::default(), None),
            Err(LaunchError::NoModel)
        ));
    }

    #[test]
    fn ollama_create_refuses_registry_names() {
        let dir = std::env::temp_dir().join(format!("lillith-setup-mf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mf = dir.join("Modelfile");
        std::fs::write(&mf, "FROM dolphin-llama3.1:8b\n").unwrap();
        let cfg = LlmConfig::default();
        let e = ollama_create_command(Path::new("/usr/bin/ollama"), "lily", &mf, &cfg).unwrap_err();
        assert!(e.to_string().contains("local-only"), "{e}");
        // A local file is fine (relative to the Modelfile).
        std::fs::write(dir.join("m.gguf"), b"GGUF").unwrap();
        std::fs::write(&mf, "FROM ./m.gguf\n").unwrap();
        let c = ollama_create_command(Path::new("/usr/bin/ollama"), "lily", &mf, &cfg).unwrap();
        assert_eq!(c.args[..3], ["create", "lily", "-f"]);
        // ...but not with a remote OLLAMA host.
        let remote = LlmConfig {
            ollama_host: "http://192.168.0.9:11434".into(),
            ..Default::default()
        };
        assert!(ollama_create_command(Path::new("/usr/bin/ollama"), "lily", &mf, &remote).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
