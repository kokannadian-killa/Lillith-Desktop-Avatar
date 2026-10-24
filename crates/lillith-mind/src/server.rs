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
    #[error("refusing llama-server extra arg {0:?}: only context, threads, GPU layers, --jinja, sampling, batch, --mlock, --no-mmap and --flash-attn flags are allowed")]
    ForbiddenArg(String),
    #[error(
        "refusing to bind llama-server to {0:?}: only 127.0.0.1 / localhost / ::1 are allowed"
    )]
    BindHost(String),
}

/// The only llama-server flags `extra_args` may carry: context size,
/// threads, GPU layers, the chat template switch, sampling, batch sizes and
/// memory mapping. Everything else (host, port, model, downloads, API key,
/// file serving, and any flag llama.cpp adds later) is refused.
/// `(flag, takes_value)`.
pub const ALLOWED_EXTRA_ARGS: &[(&str, bool)] = &[
    ("-c", true),
    ("--ctx-size", true),
    ("-t", true),
    ("--threads", true),
    ("-ngl", true),
    ("--n-gpu-layers", true),
    ("--gpu-layers", true),
    ("--jinja", false),
    ("-b", true),
    ("--batch-size", true),
    ("-ub", true),
    ("--ubatch-size", true),
    ("--mlock", false),
    ("--no-mmap", false),
    ("-fa", false),
    ("--flash-attn", false),
    // Sampling.
    ("--temp", true),
    ("--top-k", true),
    ("--top-p", true),
    ("--min-p", true),
    ("--typical", true),
    ("--top-nsigma", true),
    ("--repeat-penalty", true),
    ("--repeat-last-n", true),
    ("--presence-penalty", true),
    ("--frequency-penalty", true),
    ("--mirostat", true),
    ("--mirostat-lr", true),
    ("--mirostat-ent", true),
    ("--dynatemp-range", true),
    ("--dynatemp-exp", true),
    ("--xtc-probability", true),
    ("--xtc-threshold", true),
    ("--dry-multiplier", true),
    ("--dry-base", true),
    ("--dry-allowed-length", true),
    ("--dry-penalty-last-n", true),
    ("-s", true),
    ("--seed", true),
];

/// `--flash-attn` / `-fa` may carry `on`, `off` or `auto` (newer
/// llama.cpp) or stand alone (older builds).
const FLASH_ATTN_VALUES: &[&str] = &["on", "off", "auto"];

/// A flag value: a short plain token (a number, `auto`, `all`, …), never a
/// path, URL, or anything with spaces.
fn plain_value(v: &str) -> bool {
    // A leading `-` only for a negative number (`--seed -1`), so the next
    // flag can't be swallowed as a value.
    let negative_ok = !v.starts_with('-') || v.parse::<f64>().is_ok();
    negative_ok
        && !v.is_empty()
        && v.len() <= 32
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_'))
}

/// Errors on the first extra arg that is not on [`ALLOWED_EXTRA_ARGS`]
/// (bare or `flag=value`), a value flag without a plain value, or a stray
/// positional arg.
pub fn check_extra_args(extra: &[String]) -> Result<(), ServerError> {
    let mut it = extra.iter().peekable();
    while let Some(a) = it.next() {
        let refuse = || Err(ServerError::ForbiddenArg(a.clone()));
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) => (f, Some(v)),
            None => (a.as_str(), None),
        };
        let Some(&(_, takes_value)) = ALLOWED_EXTRA_ARGS.iter().find(|(f, _)| *f == flag) else {
            return refuse();
        };
        let flash = flag == "-fa" || flag == "--flash-attn";
        match inline {
            Some(v) if takes_value && plain_value(v) => {}
            Some(v) if flash && FLASH_ATTN_VALUES.contains(&v) => {}
            Some(_) => return refuse(),
            None if takes_value => match it.next() {
                Some(v) if plain_value(v) => {}
                _ => return refuse(),
            },
            None if flash => {
                if it
                    .peek()
                    .is_some_and(|v| FLASH_ATTN_VALUES.contains(&v.as_str()))
                {
                    it.next();
                }
            }
            None => {}
        }
    }
    Ok(())
}

/// [`check_extra_args`], and on a refusal an `ARG_REFUSED` line in the
/// audit log `audit` (binary `binary`; the refused arg with any secret
/// masked).
pub fn check_extra_args_audited(
    extra: &[String],
    binary: &str,
    audit: Option<PathBuf>,
) -> Result<(), ServerError> {
    let r = check_extra_args(extra);
    if let Err(ServerError::ForbiddenArg(arg)) = &r {
        let shown = masked_args(std::slice::from_ref(arg)).join(" ");
        lillith_core::soul_contract::SoulContract::new(audit).record_arg_refused(
            binary,
            &shown,
            "refused llama-server extra arg (not on the allowlist); nothing started",
        );
    }
    r
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

/// The full command line [`LlamaServer::spawn`] would run for `cfg`, for the
/// Soul Contract card and its audit log: the resolved binary, then every
/// arg (the API key is never on it, see [`API_KEY_ENV`]). `None` when the
/// endpoint has no host/port or is not loopback.
pub fn display_argv(cfg: &ChatConfig) -> Option<Vec<String>> {
    let (host, port) = host_port(&cfg.endpoint)?;
    // What `spawn` binds (always 127.0.0.1); a non-loopback host has no card.
    let host = bind_host(&host).ok()?;
    let bin = lillith_setup::launch::resolve_program(&cfg.server.binary);
    let mut argv = vec![bin.display().to_string()];
    // The key itself goes in the environment, never on the command line.
    argv.extend(masked_args(&command_args(&cfg.server, host, port)));
    Some(argv)
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
        Self::spawn_audited(cfg, lillith_core::config::audit_log_path())
    }

    /// [`Self::spawn`], writing a refused extra arg to the audit log `audit`.
    pub fn spawn_audited(cfg: &ChatConfig, audit: Option<PathBuf>) -> Result<Self, ServerError> {
        // Loopback only: never bind the model to a network interface.
        lillith_setup::local::ensure_local(&cfg.endpoint, 80)?;
        let s = &cfg.server;
        let (host, port) = host_port(&cfg.endpoint)
            .ok_or_else(|| ServerError::BadEndpoint(cfg.endpoint.clone()))?;
        let host = bind_host(&host)?.to_string();
        // The same absolute path the card showed (`display_argv`).
        let bin = lillith_setup::launch::resolve_program(&s.binary);
        check_extra_args_audited(&s.extra_args, &bin.display().to_string(), audit)?;
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
        lillith_setup::launch::scrub_model_env(&mut cmd);
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

    /// The child is still alive.
    pub fn is_running(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|c| matches!(c.try_wait(), Ok(None)))
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
    fn extra_args_are_an_allowlist() {
        let refused = |args: &[&str]| {
            let v: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            matches!(check_extra_args(&v), Err(ServerError::ForbiddenArg(_)))
        };
        for bad in [
            // The old denylist: still refused.
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
            // Not on the allowlist: refused now too.
            "--chat-template",
            "-np",
            "--cont-batching",
            "-mg",
            "--lora",
            "--mmproj",
            "--log-file",
            "--slot-save-path",
            "--webui",
            "--some-future-flag",
            "positional",
            "",
            "--JINJA",
            "--jinja=1",
            "--mlock=yes",
            "--flash-attn=maybe",
        ] {
            assert!(refused(&["--jinja", bad]), "{bad:?}");
        }
        // Values must be plain tokens and must be there.
        for bad in [
            &["-c"][..],
            &["-c", "--host"],
            &["--temp", "-x"],
            &["-t", "/etc/passwd"],
            &["--seed", "http://x"],
            &["-ngl", "99 --host"],
            &["--ctx-size=../m.gguf"],
            &["-b", "512", "0.0.0.0"],
        ] {
            assert!(refused(bad), "{bad:?}");
        }
        let ok: Vec<String> = [
            "--jinja",
            "-ngl",
            "99",
            "--n-gpu-layers=auto",
            "-t",
            "8",
            "--threads",
            "4",
            "-c",
            "8192",
            "--ctx-size=4096",
            "--flash-attn",
            "--no-mmap",
            "-fa",
            "on",
            "--flash-attn=auto",
            "--mlock",
            "-b",
            "512",
            "-ub",
            "256",
            "--temp",
            "0.7",
            "--top-k",
            "40",
            "--top-p",
            "0.9",
            "--min-p",
            "0.05",
            "--repeat-penalty",
            "1.1",
            "--seed",
            "-1",
            "--mlock",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert!(check_extra_args(&ok).is_ok(), "{:?}", check_extra_args(&ok));
        assert!(check_extra_args(&[]).is_ok());
        // And spawn refuses before touching the binary, with an audit line.
        let dir = std::env::temp_dir().join(format!(
            "lillith-extra-args-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        let log = dir.join("audit.log");
        let cfg = ChatConfig {
            server: LlamaServerConfig {
                binary: "/nonexistent/llama-server".into(),
                extra_args: vec!["--jinja".into(), "--api-key=s3cret".into()],
                ..LlamaServerConfig::default()
            },
            ..ChatConfig::default()
        };
        assert!(matches!(
            LlamaServer::spawn_audited(&cfg, Some(log.clone())),
            Err(ServerError::ForbiddenArg(_))
        ));
        let text = std::fs::read_to_string(&log).expect("audit line written");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 1, "{text}");
        let cols: Vec<&str> = lines[0].split('\t').collect();
        assert_eq!(cols[1], "ARG_REFUSED");
        assert_eq!(cols[2], "start_model_server");
        assert_eq!(cols[3], "/nonexistent/llama-server");
        assert_eq!(cols[4], "no-card");
        assert!(cols[5].contains("--api-key=***"), "{text}");
        assert!(!text.contains("s3cret"), "{text}");
        // An accepted list writes nothing.
        let _ = std::fs::remove_file(&log);
        assert!(check_extra_args_audited(&ok, "x", Some(log.clone())).is_ok());
        assert!(!log.exists());
        let _ = std::fs::remove_dir_all(&dir);
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
