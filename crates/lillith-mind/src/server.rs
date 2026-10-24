//! Optional self-managed llama.cpp `llama-server` (`[chat.server]`).
//!
//! When both `binary` and `model_path` are configured and nothing already
//! answers at the chat endpoint, the app starts
//! `llama-server -m <gguf> --host <host> --port <port> -c <ctx> …` and kills
//! it again on exit ([`LlamaServer`] kills the child on drop).

use lillith_core::config::{ChatConfig, LlamaServerConfig};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Server spawn errors.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("llama-server binary not found: {0}")]
    MissingBinary(PathBuf),
    #[error("model file not found: {0}")]
    MissingModel(PathBuf),
    #[error("endpoint {0} is not a local http://host:port URL")]
    BadEndpoint(String),
    #[error("refusing to start llama-server on a non-local endpoint: {0}")]
    NotLocal(#[from] lillith_setup::LocalUrlError),
    #[error("spawn: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("llama-server exited early ({0})")]
    Exited(String),
    #[error("llama-server not ready after {0:?}")]
    NotReady(Duration),
    #[error("refusing llama-server extra arg {0:?}: it would change the host, port, model, key or download source")]
    ForbiddenArg(String),
    #[error(
        "refusing to bind llama-server to {0:?}: only 127.0.0.1 / localhost / ::1 are allowed"
    )]
    BindHost(String),
}

/// `extra_args` that could rebind the server, swap the model, pull a model
/// from the network, change the API key or serve files. These are fixed by
/// the app; a matching extra arg (`--host`, `--host=…`, …) is refused.
pub const FORBIDDEN_EXTRA_ARGS: &[&str] = &[
    "--host",
    "--port",
    "-m",
    "--model",
    "-mu",
    "--model-url",
    "-hf",
    "-hfr",
    "--hf-repo",
    "-hff",
    "--hf-file",
    "-hft",
    "--hf-token",
    "-dr",
    "--docker-repo",
    "--rpc",
    "--api-key",
    "--api-key-file",
    "--path",
];

/// Errors on the first forbidden extra arg: a flag from
/// [`FORBIDDEN_EXTRA_ARGS`] (bare or `flag=value`), or any other Hugging
/// Face / URL download flag (`-hf…`, `--hf-…`, `…-url…`, e.g. the draft,
/// vocoder and mmproj variants).
pub fn check_extra_args(extra: &[String]) -> Result<(), ServerError> {
    for a in extra {
        let t = a.trim();
        let flag = t.split_once('=').map_or(t, |(f, _)| f);
        let forbidden = FORBIDDEN_EXTRA_ARGS.contains(&flag)
            || (flag.starts_with('-') && !flag.starts_with("--") && flag.starts_with("-hf"))
            || flag == "-mmu"
            || flag.starts_with("--hf-")
            || (flag.starts_with('-') && flag.contains("-url"));
        if forbidden {
            return Err(ServerError::ForbiddenArg(a.clone()));
        }
    }
    Ok(())
}

/// The literal address llama-server binds to: always `127.0.0.1`, for an
/// endpoint host of `127.0.0.1`, `localhost` or `::1`; anything else errors.
pub fn bind_host(endpoint_host: &str) -> Result<&'static str, ServerError> {
    match endpoint_host.trim().to_ascii_lowercase().as_str() {
        "127.0.0.1" | "localhost" | "::1" => Ok("127.0.0.1"),
        other => Err(ServerError::BindHost(other.to_string())),
    }
}

/// `args` with every value after `--api-key` masked, for logs.
pub fn masked_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut hide_next = false;
    for a in args {
        if hide_next {
            out.push("***".to_string());
            hide_next = false;
        } else if let Some((flag, _)) = a
            .split_once('=')
            .filter(|(f, _)| f.starts_with("--api-key"))
        {
            out.push(format!("{flag}=***"));
        } else {
            hide_next = a == "--api-key" || a == "--api-key-file";
            out.push(a.clone());
        }
    }
    out
}

/// Env var llama-server reads the API key from (`--api-key`'s env form), so
/// the key never appears in the process's argv (`ps`, /proc/<pid>/cmdline).
pub const API_KEY_ENV: &str = "LLAMA_API_KEY";

/// `(host, port)` from an `http://host:port[/…]` endpoint (port defaults
/// to 80; llama-server itself defaults to 8080).
pub fn host_port(endpoint: &str) -> Option<(String, u16)> {
    let rest = endpoint.trim().strip_prefix("http://")?;
    let authority = rest.split('/').next()?;
    if authority.is_empty() {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => (h, p.parse().ok()?),
        _ => (authority, 80),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((host.to_string(), port))
}

/// The command line for `cfg` (exposed for tests and logging). The API key
/// is not on it: [`LlamaServer::spawn`] passes it in [`API_KEY_ENV`].
/// Callers must run [`check_extra_args`] and [`bind_host`] first.
pub fn command_args(server: &LlamaServerConfig, host: &str, port: u16) -> Vec<String> {
    let mut a = vec![
        "-m".to_string(),
        server.model_path.clone(),
        "--host".into(),
        host.to_string(),
        "--port".into(),
        port.to_string(),
        "-c".into(),
        server.ctx_size.to_string(),
    ];
    if server.threads > 0 {
        a.extend(["-t".into(), server.threads.to_string()]);
    }
    if let Some(n) = server.gpu_layers {
        a.extend(["-ngl".into(), n.to_string()]);
    }
    a.extend(server.extra_args.iter().cloned());
    a
}

/// A running llama-server child; killed on drop.
#[derive(Debug)]
pub struct LlamaServer {
    child: Option<Child>,
    pub host: String,
    pub port: u16,
}

impl LlamaServer {
    /// Spawns llama-server for `cfg.server`, listening where `cfg.endpoint`
    /// points. Does not wait for the model to load (see [`Self::wait_ready`]).
    pub fn spawn(cfg: &ChatConfig) -> Result<Self, ServerError> {
        // Loopback only: never bind the model to a network interface.
        lillith_setup::local::ensure_local(&cfg.endpoint, 80)?;
        let s = &cfg.server;
        let (host, port) = host_port(&cfg.endpoint)
            .ok_or_else(|| ServerError::BadEndpoint(cfg.endpoint.clone()))?;
        let host = bind_host(&host)?.to_string();
        check_extra_args(&s.extra_args)?;
        let bin = PathBuf::from(s.binary.trim());
        let model = PathBuf::from(s.model_path.trim());
        if !bin.exists() && which(&bin).is_none() {
            return Err(ServerError::MissingBinary(bin));
        }
        if !model.exists() {
            return Err(ServerError::MissingModel(model));
        }
        let args = command_args(s, &host, port);
        let shown = masked_args(&args);
        tracing::info!(bin = %bin.display(), args = ?shown, api_key = !cfg.api_key.is_empty(), "starting llama-server");
        let mut cmd = Command::new(&bin);
        // Nothing from our environment may steer the server (llama.cpp reads
        // LLAMA_ARG_* for every flag, e.g. a model URL or HF repo to fetch).
        for (k, _) in std::env::vars_os() {
            let k = k.to_string_lossy();
            if k.starts_with("LLAMA_ARG_") || k == API_KEY_ENV || k == "HF_TOKEN" {
                cmd.env_remove(&*k);
            }
        }
        if !cfg.api_key.is_empty() {
            cmd.env(API_KEY_ENV, &cfg.api_key);
        }
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Prebuilt Linux releases ship their .so files next to the binary.
        if let Some(dir) = bin.parent().filter(|d| !d.as_os_str().is_empty()) {
            #[cfg(target_os = "linux")]
            {
                let mut ld = dir.as_os_str().to_owned();
                if let Some(old) = std::env::var_os("LD_LIBRARY_PATH") {
                    ld.push(":");
                    ld.push(old);
                }
                cmd.env("LD_LIBRARY_PATH", ld);
            }
            let _ = dir;
        }
        // Windows: no console window for the server.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let child = cmd.spawn()?;
        Ok(Self {
            child: Some(child),
            host,
            port,
        })
    }

    /// Polls `GET /health` until the model is loaded, the child dies or
    /// `timeout` passes.
    pub async fn wait_ready(
        &mut self,
        endpoint: &str,
        timeout: Duration,
    ) -> Result<(), ServerError> {
        let client = crate::llm::LlmClient::with_timeout(
            crate::llm::Flavor::OpenAiCompat,
            endpoint,
            "",
            Duration::from_secs(3),
        );
        let t0 = Instant::now();
        loop {
            if let Some(c) = &mut self.child {
                if let Ok(Some(status)) = c.try_wait() {
                    self.child = None;
                    return Err(ServerError::Exited(status.to_string()));
                }
            }
            if let Ok(true) = client.health().await {
                tracing::info!(after_s = t0.elapsed().as_secs_f32(), "llama-server ready");
                return Ok(());
            }
            if t0.elapsed() >= timeout {
                return Err(ServerError::NotReady(timeout));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Kills the server (idempotent).
    pub fn shutdown(&mut self) {
        if let Some(mut c) = self.child.take() {
            tracing::info!(pid = c.id(), "stopping llama-server");
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for LlamaServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Is something already answering `/health` at `endpoint`?
pub async fn already_running(endpoint: &str) -> bool {
    let c = crate::llm::LlmClient::with_timeout(
        crate::llm::Flavor::OpenAiCompat,
        endpoint,
        "",
        Duration::from_secs(2),
    );
    c.health().await.is_ok()
}

fn which(bin: &std::path::Path) -> Option<PathBuf> {
    if bin.components().count() != 1 {
        return None;
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join(bin))
            .find(|p| p.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_parsing() {
        assert_eq!(
            host_port("http://127.0.0.1:8080"),
            Some(("127.0.0.1".into(), 8080))
        );
        assert_eq!(
            host_port("http://localhost:9000/v1"),
            Some(("localhost".into(), 9000))
        );
        assert_eq!(host_port("http://box"), Some(("box".into(), 80)));
        assert_eq!(host_port("http://[::1]:8081/"), Some(("::1".into(), 8081)));
        assert_eq!(host_port("https://remote:443"), None);
        assert_eq!(host_port("http://"), None);
    }

    #[test]
    fn args_and_missing_files() {
        let s = LlamaServerConfig {
            binary: "/nonexistent/llama-server".into(),
            model_path: "/m/q.gguf".into(),
            threads: 4,
            gpu_layers: Some(99),
            extra_args: vec!["--jinja".into()],
            ..LlamaServerConfig::default()
        };
        let a = command_args(&s, "127.0.0.1", 8080);
        assert_eq!(
            a,
            [
                "-m",
                "/m/q.gguf",
                "--host",
                "127.0.0.1",
                "--port",
                "8080",
                "-c",
                "4096",
                "-t",
                "4",
                "-ngl",
                "99",
                "--jinja"
            ]
        );
        let cfg = ChatConfig {
            server: s,
            ..ChatConfig::default()
        };
        assert!(matches!(
            LlamaServer::spawn(&cfg),
            Err(ServerError::MissingBinary(_))
        ));
    }

    #[test]
    fn api_key_never_in_argv_and_masked_in_logs() {
        let s = LlamaServerConfig {
            model_path: "/m/q.gguf".into(),
            ..LlamaServerConfig::default()
        };
        let a = command_args(&s, "127.0.0.1", 8080);
        assert!(!a.iter().any(|x| x.contains("--api-key")), "{a:?}");
        let raw: Vec<String> = [
            "-m",
            "x",
            "--api-key",
            "s3cret",
            "--api-key=s3cret2",
            "--jinja",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let m = masked_args(&raw);
        assert_eq!(
            m,
            ["-m", "x", "--api-key", "***", "--api-key=***", "--jinja"]
        );
        assert!(!format!("{m:?}").contains("s3cret"));
    }

    #[test]
    fn extra_args_cannot_override_host_model_key_or_fetch() {
        for bad in [
            "--host",
            "--host=0.0.0.0",
            "--port",
            "--port=1",
            "-m",
            "--model",
            "-mu",
            "--model-url",
            "-hf",
            "-hfr",
            "--hf-repo",
            "-hff",
            "--hf-file",
            "-hft",
            "--hf-token",
            "-dr",
            "--docker-repo",
            "--rpc",
            "--api-key",
            "--api-key-file",
            "--path",
            " --host",
            "--hf-repo-draft",
            "-hfd",
            "--mmproj-url",
            "-mmu",
            "--model-url=http://x",
        ] {
            assert!(
                matches!(
                    check_extra_args(&["--jinja".into(), bad.into()]),
                    Err(ServerError::ForbiddenArg(_))
                ),
                "{bad}"
            );
        }
        let ok: Vec<String> = [
            "--jinja",
            "-ngl",
            "99",
            "-t",
            "8",
            "-c",
            "8192",
            "--flash-attn",
            "--no-mmap",
            "--mlock",
            "-b",
            "512",
            "-ub",
            "256",
            "--temp",
            "0.7",
            "--chat-template",
            "llama3",
            "-np",
            "1",
            "--cont-batching",
            "-mg",
            "0",
            "--mlock",
            "--min-p",
            "0.05",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert!(check_extra_args(&ok).is_ok());
        // And spawn refuses before touching the binary.
        let cfg = ChatConfig {
            server: LlamaServerConfig {
                extra_args: vec!["--host".into(), "0.0.0.0".into()],
                ..LlamaServerConfig::default()
            },
            ..ChatConfig::default()
        };
        assert!(matches!(
            LlamaServer::spawn(&cfg),
            Err(ServerError::ForbiddenArg(_))
        ));
    }

    #[test]
    fn bind_host_is_always_literal_loopback() {
        for h in ["127.0.0.1", "localhost", "LOCALHOST", "::1"] {
            assert_eq!(bind_host(h).unwrap(), "127.0.0.1", "{h}");
        }
        for h in ["0.0.0.0", "127.0.0.2", "::", "box", "192.168.1.5", ""] {
            assert!(matches!(bind_host(h), Err(ServerError::BindHost(_))), "{h}");
        }
    }

    #[test]
    fn spawn_refuses_non_local_endpoint() {
        for e in [
            "http://0.0.0.0:8080",
            "http://192.168.1.5:8080",
            "https://127.0.0.1:8080",
        ] {
            let cfg = ChatConfig {
                endpoint: e.into(),
                ..ChatConfig::default()
            };
            assert!(
                matches!(LlamaServer::spawn(&cfg), Err(ServerError::NotLocal(_))),
                "{e}"
            );
        }
    }
}
