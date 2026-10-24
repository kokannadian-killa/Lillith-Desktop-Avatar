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
        std::iter::once(&self.program).chain(&self.args).map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")
    }

    /// The process to spawn, with an environment scrubbed of anything that
    /// could make the child reach the network on its own: llama.cpp's
    /// `LLAMA_ARG_*` overrides (e.g. `LLAMA_ARG_HF_REPO` / `LLAMA_ARG_MODEL_URL`
    /// would make llama-server download a model, `LLAMA_ARG_HOST` could
    /// re-bind it), `HF_*` hub settings, proxy variables and any inherited
    /// `OLLAMA_HOST` (callers pin it to loopback explicitly).
    pub fn to_command(&self) -> Command {
        let mut c = Command::new(&self.program);
        c.args(&self.args);
        for (k, _) in std::env::vars_os() {
            if let Some(name) = k.to_str() {
                if is_scrubbed_env(name) {
                    c.env_remove(&k);
                }
            }
        }
        for k in SCRUBBED_ENV {
            c.env_remove(k);
        }
        c
    }
}

/// Environment variables always removed from launched children.
const SCRUBBED_ENV: &[&str] = &[
    "OLLAMA_HOST",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "LLAMA_ARG_HF_REPO",
    "LLAMA_ARG_HF_FILE",
    "LLAMA_ARG_MODEL_URL",
    "LLAMA_ARG_HOST",
    "LLAMA_ARG_PORT",
    "HF_TOKEN",
    "HF_ENDPOINT",
];

fn is_scrubbed_env(name: &str) -> bool {
    let up = name.to_ascii_uppercase();
    up.starts_with("LLAMA_ARG_") || up.starts_with("HF_") || up.ends_with("_PROXY") || SCRUBBED_ENV.contains(&name)
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c)) {
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
pub fn llama_server_command(cfg: &LlmConfig, bin: Option<&Path>) -> Result<LaunchCommand, LaunchError> {
    if cfg.gguf_path.trim().is_empty() {
        return Err(LaunchError::NoModel);
    }
    // `-m` must be a local file path, never a URL / hub reference.
    if cfg.gguf_path.contains("://") || cfg.gguf_path.trim_start().starts_with('-') {
        return Err(LaunchError::NotLocalModel(format!(
            "gguf_path `{}` is not a local file path (Lillith is local-only)",
            cfg.gguf_path
        )));
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
/// cannot download anything). Every `FROM` and `ADAPTER` line is checked
/// (Ollama uses the *last* `FROM`, so checking only the first would let a
/// second `FROM llama3` slip through): it must look like a path
/// ([`crate::modelfile::check_local_model_ref`]) **and** be an existing
/// regular file. Relative paths resolve against `base_dir` (the
/// Modelfile's folder, as Ollama does).
pub fn ensure_local_modelfile(m: &Modelfile, base_dir: &Path) -> Result<(), LaunchError> {
    if m.from().is_none() {
        return Err(LaunchError::NotLocalModel("Modelfile has no FROM".into()));
    }
    let check = |what: &str, p: &str| -> Result<(), LaunchError> {
        let refuse = |why: String| {
            LaunchError::NotLocalModel(format!(
                "{what} `{p}`: {why}; refusing to run `ollama create` \
                 because Ollama would try to download it (Lillith is local-only)"
            ))
        };
        crate::modelfile::check_local_model_ref(p).map_err(refuse)?;
        let path = crate::paths::expand_tilde(p.trim());
        let path = if path.is_absolute() { path } else { base_dir.join(path) };
        if path.is_file() {
            Ok(())
        } else {
            Err(refuse("not an existing local file".into()))
        }
    };
    for i in &m.instructions {
        match i {
            Instruction::From(f) => check("FROM", f)?,
            Instruction::Adapter(a) => check("ADAPTER", a)?,
            _ => {}
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
    let src = std::fs::read_to_string(modelfile_path).map_err(|e| LaunchError::Io(modelfile_path.into(), e))?;
    let m = Modelfile::parse(&src)?;
    m.validate()?;
    ensure_local_modelfile(&m, modelfile_path.parent().unwrap_or(Path::new(".")))?;
    if model_name.trim().is_empty() || model_name.starts_with('-') || model_name.chars().any(char::is_whitespace) {
        return Err(LaunchError::NotLocalModel(format!("bad Ollama model name `{model_name}`")));
    }
    cfg.ollama()?; // host must be loopback
    Ok(LaunchCommand {
        program: ollama_bin.display().to_string(),
        args: vec!["create".into(), model_name.into(), "-f".into(), modelfile_path.display().to_string()],
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
        .env("OLLAMA_HOST", host.host_port())
        .stdin(Stdio::null())
        .output()
        .map_err(|e| LaunchError::Io(ollama_bin.into(), e))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(text)
    } else {
        Err(LaunchError::Failed { status: out.status.to_string(), output: text })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llama_server_line() {
        let cfg = LlmConfig { gguf_path: "/workspace/models/qwen2.5-0.5b-instruct-q4_k_m.gguf".into(), ..Default::default() };
        let c = llama_server_command(&cfg, None).unwrap();
        assert_eq!(
            c.to_shell_string(),
            "llama-server -m /workspace/models/qwen2.5-0.5b-instruct-q4_k_m.gguf --host 127.0.0.1 --port 8080 -c 4096 --chat-template chatml"
        );
        let cfg = LlmConfig { gguf_path: "/home/ko/My Models/it's.gguf".into(), server_url: "http://localhost:9090".into(), ..Default::default() };
        let s = llama_server_command(&cfg, Some(Path::new("/opt/llama.cpp/bin/llama-server"))).unwrap().to_shell_string();
        assert!(s.contains("'/home/ko/My Models/it'\"'\"'s.gguf'"), "{s}");
        assert!(s.contains("--host 127.0.0.1 --port 9090"));
    }

    #[test]
    fn llama_server_refuses_remote_and_missing_model() {
        let cfg = LlmConfig { gguf_path: "/m.gguf".into(), server_url: "http://0.0.0.0:8080".into(), ..Default::default() };
        assert!(matches!(llama_server_command(&cfg, None), Err(LaunchError::Config(_))));
        assert!(matches!(llama_server_command(&LlmConfig::default(), None), Err(LaunchError::NoModel)));
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
        let remote = LlmConfig { ollama_host: "http://192.168.0.9:11434".into(), ..Default::default() };
        assert!(ollama_create_command(Path::new("/usr/bin/ollama"), "lily", &mf, &remote).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lillith-setup-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn every_from_and_adapter_must_be_a_local_file() {
        let dir = tmpdir("mf2");
        std::fs::write(dir.join("m.gguf"), b"GGUF").unwrap();
        std::fs::create_dir_all(dir.join("adir")).unwrap();
        let ok = |src: &str| ensure_local_modelfile(&Modelfile::parse(src).unwrap(), &dir);
        assert!(ok("FROM ./m.gguf\n").is_ok());
        assert!(ok("FROM m.gguf\n").is_ok());
        assert!(ok(&format!("FROM {}\n", dir.join("m.gguf").display())).is_ok());
        assert!(ok("FROM ./m.gguf\nADAPTER ./m.gguf\n").is_ok());
        for src in [
            "FROM ./m.gguf\nFROM llama3\n",          // second FROM wins in Ollama
            "FROM llama3.2:3b\n",
            "FROM dolphin3\n",
            "FROM hf.co/bartowski/Dolphin3.0-Llama3.2-3B-GGUF:Q4_K_M\n",
            "FROM registry.ollama.ai/library/llama3:latest\n",
            "FROM https://example.com/m.gguf\n",
            "FROM ./missing.gguf\n",                  // missing path = registry lookup in Ollama
            "FROM missing.gguf\n",
            "FROM ./*.gguf\n",
            "FROM ./adir\n",
            "FROM ./m.gguf\nADAPTER lora-from-hub\n",
            "FROM ./m.gguf\nADAPTER ./missing-lora.gguf\n",
            "# no from\nSYSTEM hi\n",
        ] {
            let e = ok(src).expect_err(src);
            assert!(e.to_string().contains("local-only") || e.to_string().contains("no FROM"), "{src}: {e}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ollama_create_validates_params_and_name() {
        let dir = tmpdir("mf3");
        std::fs::write(dir.join("m.gguf"), b"GGUF").unwrap();
        let mf = dir.join("Modelfile");
        let cfg = LlmConfig::default();
        let bin = Path::new("/usr/bin/ollama");
        std::fs::write(&mf, "FROM ./m.gguf\nPARAMETER temprature 0.7\n").unwrap();
        assert!(matches!(ollama_create_command(bin, "lily", &mf, &cfg), Err(LaunchError::Modelfile(_))));
        std::fs::write(&mf, "FROM ./m.gguf\nPARAMETER num_ctx lots\n").unwrap();
        assert!(matches!(ollama_create_command(bin, "lily", &mf, &cfg), Err(LaunchError::Modelfile(_))));
        std::fs::write(&mf, "FROM ./m.gguf\nPARAMETER num_ctx 4096\n").unwrap();
        assert!(ollama_create_command(bin, "lily", &mf, &cfg).is_ok());
        for name in ["", "--help", "two words"] {
            assert!(ollama_create_command(bin, name, &mf, &cfg).is_err(), "{name:?}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ollama_host_is_bracketed_for_ipv6() {
        let cfg = LlmConfig { ollama_host: "http://[::1]:11500".into(), ..Default::default() };
        assert_eq!(cfg.ollama().unwrap().host_port(), "[::1]:11500");
    }

    #[test]
    fn children_get_a_scrubbed_environment() {
        std::env::set_var("LLAMA_ARG_HF_REPO", "bartowski/whatever-GGUF");
        std::env::set_var("LLAMA_ARG_MODEL_URL", "https://example.com/m.gguf");
        std::env::set_var("HF_ENDPOINT_TEST_X", "https://example.com");
        let cfg = LlmConfig { gguf_path: "/m.gguf".into(), ..Default::default() };
        let cmd = llama_server_command(&cfg, None).unwrap().to_command();
        std::env::remove_var("LLAMA_ARG_HF_REPO");
        std::env::remove_var("LLAMA_ARG_MODEL_URL");
        std::env::remove_var("HF_ENDPOINT_TEST_X");
        let removed: Vec<String> = cmd
            .get_envs()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        for k in ["LLAMA_ARG_HF_REPO", "LLAMA_ARG_MODEL_URL", "HF_ENDPOINT_TEST_X", "OLLAMA_HOST", "HTTPS_PROXY", "http_proxy"] {
            assert!(removed.iter().any(|r| r == k), "{k} not scrubbed: {removed:?}");
        }
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(!args.iter().any(|a| a.starts_with("-hf") || a.contains("url") || a.contains("http")), "{args:?}");
    }

    #[test]
    fn llama_server_refuses_url_models() {
        for p in ["https://huggingface.co/x/y.gguf", "--hf-repo"] {
            let cfg = LlmConfig { gguf_path: p.into(), ..Default::default() };
            assert!(matches!(llama_server_command(&cfg, None), Err(LaunchError::NotLocalModel(_))), "{p}");
        }
    }
}
