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
        scrub_model_env(&mut c);
        c
    }
}

/// Env var prefixes that would change what a model program does behind the
/// card's back: llama.cpp reads every `LLAMA_ARG_*` as a flag (host, port,
/// api key, model URL…), and `HF_*` points it at Hugging Face downloads.
pub const SCRUBBED_ENV_PREFIXES: [&str; 2] = ["LLAMA_ARG_", "HF_"];

/// Removes every inherited env var starting with [`SCRUBBED_ENV_PREFIXES`]
/// from `cmd`, so the program runs exactly the argv the card showed.
pub fn scrub_model_env(cmd: &mut Command) {
    for (k, _) in std::env::vars_os() {
        if k.to_str()
            .is_some_and(|k| SCRUBBED_ENV_PREFIXES.iter().any(|p| k.starts_with(p)))
        {
            cmd.env_remove(k);
        }
    }
}

/// The program a launch really runs, as an absolute path: a path (more
/// than one component, or an existing file) is made absolute; a bare name
/// is looked up in `PATH` (`.exe` too on Windows). Unresolvable names come
/// back unchanged (the spawn then fails as "missing"). The card shows and
/// the spawn uses this same path, so `PATH` can't swap the binary between
/// the click and the start.
pub fn resolve_program(program: &str) -> PathBuf {
    let p = Path::new(program.trim());
    if p.as_os_str().is_empty() {
        return p.to_path_buf();
    }
    if p.components().count() > 1 || p.is_absolute() || p.is_file() {
        return std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    }
    let found = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|d| {
            let c = d.join(p);
            if c.is_file() {
                return Some(c);
            }
            #[cfg(windows)]
            {
                let e = d.join(format!("{}.exe", p.display()));
                if e.is_file() {
                    return Some(e);
                }
            }
            None
        })
    });
    match found {
        Some(f) => std::path::absolute(&f).unwrap_or(f),
        None => p.to_path_buf(),
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
    #[error("llama-server is only started on 127.0.0.1; set server_url to http://127.0.0.1:PORT (got {0})")]
    Host(String),
    #[error("{0}")]
    NotLocalModel(String),
    #[error("modelfile: {0}")]
    Modelfile(#[from] crate::modelfile::ModelfileError),
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("`ollama create` failed ({status}): {output}")]
    Failed { status: String, output: String },
}

/// The llama-server line for `cfg`, e.g.
/// `llama-server -m <gguf> --host 127.0.0.1 --port 8080 -c 4096 -t 2 --jinja`.
///
/// Always `--host 127.0.0.1` (loopback only; a `[::1]` server URL is refused
/// rather than silently mismatched) and always `--jinja`, so the model's own
/// embedded chat template is used. No `--chat-template <name>`: with
/// `--jinja` (llama.cpp's default) a name like `chatml` is taken as literal
/// template text and every prompt collapses to the word "chatml".
/// `-t` is `[llm] threads`, or this machine's physical cores when 0.
pub fn llama_server_command(
    cfg: &LlmConfig,
    bin: Option<&Path>,
) -> Result<LaunchCommand, LaunchError> {
    llama_server_command_with(cfg, bin, crate::host::physical_cores())
}

/// [`llama_server_command`] with the physical core count injected (tests, GUI).
pub fn llama_server_command_with(
    cfg: &LlmConfig,
    bin: Option<&Path>,
    physical_cores: u32,
) -> Result<LaunchCommand, LaunchError> {
    if cfg.gguf_path.trim().is_empty() {
        return Err(LaunchError::NoModel);
    }
    let url: LocalUrl = cfg.server()?;
    if url.ip != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) {
        return Err(LaunchError::Host(url.ip.to_string()));
    }
    let program = bin
        .map(|b| b.display().to_string())
        .or_else(|| (!cfg.llama_server_bin.is_empty()).then(|| cfg.llama_server_bin.clone()))
        .unwrap_or_else(|| "llama-server".into());
    let ctx = if cfg.ctx_size == 0 {
        4096
    } else {
        cfg.ctx_size
    };
    let args = vec![
        "-m".into(),
        cfg.gguf_path.clone(),
        "--host".into(),
        LOOPBACK_HOST.into(),
        "--port".into(),
        url.port.to_string(),
        "-c".into(),
        ctx.to_string(),
        "-t".into(),
        effective_threads(cfg.threads, physical_cores).to_string(),
        "--jinja".into(),
    ];
    Ok(LaunchCommand { program, args })
}

/// The only address the wizard ever binds llama-server to.
pub const LOOPBACK_HOST: &str = "127.0.0.1";

/// `-t`: the configured count, else `physical_cores` (at least 1).
pub fn effective_threads(configured: u32, physical_cores: u32) -> u32 {
    if configured > 0 {
        configured
    } else {
        physical_cores.max(1)
    }
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
    fn launches_drop_llama_arg_and_hf_env() {
        // Unique names: no other test reads them.
        std::env::set_var("LLAMA_ARG_LILLITH_SCRUB_TEST", "0.0.0.0");
        std::env::set_var("HF_LILLITH_SCRUB_TEST", "x");
        std::env::set_var("LILLITH_KEEP_SCRUB_TEST", "y");
        let c = LaunchCommand {
            program: "llama-server".into(),
            args: vec![],
        }
        .to_command();
        let removed: Vec<String> = c
            .get_envs()
            .filter(|(_, v)| v.is_none())
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        assert!(removed.iter().any(|k| k == "LLAMA_ARG_LILLITH_SCRUB_TEST"));
        assert!(removed.iter().any(|k| k == "HF_LILLITH_SCRUB_TEST"));
        assert!(!removed.iter().any(|k| k == "LILLITH_KEEP_SCRUB_TEST"));
    }

    #[test]
    fn programs_resolve_to_absolute_paths() {
        let rel = resolve_program("some/dir/llama-server");
        assert!(rel.is_absolute(), "{rel:?}");
        assert!(rel.ends_with("some/dir/llama-server"));
        let none = resolve_program("no-such-program-lillith-xyz");
        assert_eq!(none, PathBuf::from("no-such-program-lillith-xyz"));
        #[cfg(unix)]
        {
            let sh = resolve_program("sh");
            assert!(sh.is_absolute() && sh.is_file(), "{sh:?}");
        }
    }

    #[test]
    fn llama_server_line() {
        let cfg = LlmConfig {
            gguf_path: "/workspace/models/qwen2.5-0.5b-instruct-q4_k_m.gguf".into(),
            ..Default::default()
        };
        let c = llama_server_command_with(&cfg, None, 2).unwrap();
        assert_eq!(
            c.to_shell_string(),
            "llama-server -m /workspace/models/qwen2.5-0.5b-instruct-q4_k_m.gguf --host 127.0.0.1 --port 8080 -c 4096 -t 2 --jinja"
        );
        let cfg = LlmConfig {
            gguf_path: "/home/ko/My Models/it's.gguf".into(),
            server_url: "http://localhost:9090".into(),
            threads: 3,
            ..Default::default()
        };
        let s =
            llama_server_command_with(&cfg, Some(Path::new("/opt/llama.cpp/bin/llama-server")), 8)
                .unwrap()
                .to_shell_string();
        assert!(s.contains("'/home/ko/My Models/it'\"'\"'s.gguf'"), "{s}");
        assert!(s.contains("--host 127.0.0.1 --port 9090"));
        assert!(s.contains("-t 3 "), "configured threads win: {s}");
    }

    #[test]
    fn llama_server_always_jinja_loopback_and_physical_threads() {
        for (tmpl, url) in [
            ("chatml", "http://127.0.0.1:8080"),
            ("llama3", "http://localhost:8081/v1"),
            ("mistral-v1", "http://127.0.0.1:8082"),
        ] {
            let cfg = LlmConfig {
                gguf_path: "/m.gguf".into(),
                chat_template: tmpl.into(),
                server_url: url.into(),
                ..Default::default()
            };
            let a = llama_server_command_with(&cfg, None, 2).unwrap().args;
            assert!(a.iter().any(|x| x == "--jinja"), "{a:?}");
            let host = a.iter().position(|x| x == "--host").unwrap();
            assert_eq!(a[host + 1], "127.0.0.1");
            let c = a.iter().position(|x| x == "-c").unwrap();
            assert_eq!(a[c + 1], "4096");
            let t = a.iter().position(|x| x == "-t").unwrap();
            assert_eq!(a[t + 1], "2", "i5-7200U: 2 physical cores");
            // A template *name* under --jinja would be read as template text.
            assert!(!a.iter().any(|x| x == "--chat-template"), "{a:?}");
            assert!(!a.iter().any(|x| x == "0.0.0.0" || x == "::"));
        }
        assert_eq!(effective_threads(0, 0), 1);
        assert_eq!(effective_threads(0, 4), 4);
        assert_eq!(effective_threads(6, 4), 6);
        // The real-host wrapper detects cores and still passes -t.
        let cfg = LlmConfig {
            gguf_path: "/m.gguf".into(),
            ..Default::default()
        };
        let a = llama_server_command(&cfg, None).unwrap().args;
        let t = a.iter().position(|x| x == "-t").unwrap();
        assert!(a[t + 1].parse::<u32>().unwrap() >= 1);
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
        // Loopback but not 127.0.0.1: refused instead of a host/URL mismatch.
        let v6 = LlmConfig {
            gguf_path: "/m.gguf".into(),
            server_url: "http://[::1]:8080".into(),
            ..Default::default()
        };
        assert!(matches!(
            llama_server_command_with(&v6, None, 2),
            Err(LaunchError::Host(_))
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
