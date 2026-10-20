//! Runtime detection: llama.cpp binaries, Ollama, and whether their local
//! HTTP APIs answer. Every probe is loopback-only ([`crate::local`]), has a
//! short timeout and never panics when nothing is installed.

use crate::http;
use crate::local::LocalUrl;
use crate::paths;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const DEFAULT_LLAMA_SERVER_URL: &str = "http://127.0.0.1:8080";
pub const DEFAULT_OLLAMA_HOST: &str = "http://127.0.0.1:11434";

#[derive(Debug, Clone)]
pub struct DetectOptions {
    /// Extra directories (or llama.cpp release roots) to search for binaries.
    pub extra_bin_dirs: Vec<PathBuf>,
    pub llama_server_url: String,
    pub ollama_host: String,
    /// Connect / read timeout per probe.
    pub timeout: Duration,
    /// Run `<bin> --version` (killed after `timeout * 3`).
    pub probe_versions: bool,
}

impl Default for DetectOptions {
    fn default() -> Self {
        Self {
            extra_bin_dirs: Vec::new(),
            llama_server_url: DEFAULT_LLAMA_SERVER_URL.into(),
            // OLLAMA_HOST is honoured only if it is loopback; otherwise the
            // probe reports the refusal instead of connecting.
            ollama_host: std::env::var("OLLAMA_HOST")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_OLLAMA_HOST.into()),
            timeout: Duration::from_millis(800),
            probe_versions: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BinaryInfo {
    pub path: PathBuf,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct OllamaModel {
    pub name: String,
    pub size_bytes: u64,
    pub family: Option<String>,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct OllamaApi {
    pub url: String,
    pub reachable: bool,
    pub version: Option<String>,
    pub models: Vec<OllamaModel>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LlamaServerApi {
    pub url: String,
    pub reachable: bool,
    /// `/health` status (`ok`, `loading model`, ...).
    pub health: Option<String>,
    /// Model ids from `/v1/models`.
    pub models: Vec<String>,
    pub error: Option<String>,
}

/// Everything the wizard needs to pick a backend.
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeReport {
    pub llama_server: Option<BinaryInfo>,
    pub llama_cli: Option<BinaryInfo>,
    pub ollama: Option<BinaryInfo>,
    pub llama_server_api: LlamaServerApi,
    pub ollama_api: OllamaApi,
}

impl RuntimeReport {
    /// Suggested backend: a running server wins, then installed binaries.
    pub fn suggested_backend(&self) -> Option<crate::config::Backend> {
        use crate::config::Backend;
        if self.llama_server_api.reachable {
            Some(Backend::LlamaServer)
        } else if self.ollama_api.reachable {
            Some(Backend::Ollama)
        } else if self.llama_server.is_some() {
            Some(Backend::LlamaServer)
        } else if self.ollama.is_some() {
            Some(Backend::Ollama)
        } else {
            None
        }
    }
}

/// Runs all probes.
pub fn detect(opts: &DetectOptions) -> RuntimeReport {
    let dirs = search_dirs(&opts.extra_bin_dirs);
    let find = |name: &str| {
        find_binary(name, &dirs).map(|path| BinaryInfo {
            version: opts.probe_versions.then(|| binary_version(&path, opts.timeout * 3)).flatten(),
            path,
        })
    };
    RuntimeReport {
        llama_server: find("llama-server"),
        llama_cli: find("llama-cli"),
        ollama: find("ollama"),
        llama_server_api: probe_llama_server(&opts.llama_server_url, opts.timeout),
        ollama_api: probe_ollama(&opts.ollama_host, opts.timeout),
    }
}

/// `$PATH` + common install locations (+ `extra`), in search order.
pub fn search_dirs(extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = extra.to_vec();
    if let Some(h) = paths::home_dir() {
        for d in ["llama.cpp", "AI/llama.cpp", "src/llama.cpp", "tools/llama.cpp", ".local/share/llama.cpp"] {
            roots.push(h.join(d));
        }
    }
    roots.push(PathBuf::from("/opt/llama.cpp"));
    let mut dirs = Vec::new();
    if let Some(p) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    if let Some(h) = paths::home_dir() {
        dirs.push(h.join(".local/bin"));
        dirs.push(h.join("bin"));
    }
    dirs.extend(["/usr/local/bin", "/usr/bin", "/opt/ollama/bin", "/opt/llama.cpp/bin"].map(PathBuf::from));
    // llama.cpp roots: the root, its build dirs, and one level of release
    // sub-folders (e.g. `llama-b4567/`).
    for r in roots {
        let mut cands = vec![r.clone()];
        if let Ok(rd) = std::fs::read_dir(&r) {
            let mut subs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            subs.sort();
            subs.reverse(); // newest release name first (llama-b1234 sorting)
            cands.extend(subs);
        }
        for c in cands {
            dirs.push(c.join("build/bin"));
            dirs.push(c.join("bin"));
            dirs.push(c.clone());
        }
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}

/// First executable `name` in `dirs`.
pub fn find_binary(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(name)).find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// First non-empty line of `<bin> --version` (stdout or stderr), killed
/// after `timeout`. Local process only.
pub fn binary_version(bin: &Path, timeout: Duration) -> Option<String> {
    let mut child = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // `ollama --version` also asks the server for its version: pin it to
        // loopback so a remote OLLAMA_HOST in the environment is never contacted.
        .env("OLLAMA_HOST", "127.0.0.1:11434")
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    text.lines()
        .map(str::trim)
        .find(|l| l.contains("version"))
        .or_else(|| text.lines().map(str::trim).find(|l| !l.is_empty()))
        .map(|l| l.chars().take(160).collect())
}

/// Probes llama-server's `/health` and `/v1/models` (loopback only).
pub fn probe_llama_server(url: &str, timeout: Duration) -> LlamaServerApi {
    let mut r = LlamaServerApi { url: url.into(), ..Default::default() };
    let base = match LocalUrl::parse(url, 8080) {
        Ok(b) => b,
        Err(e) => {
            r.error = Some(e.to_string());
            return r;
        }
    };
    r.url = base.base();
    match http::get(&base, "/health", timeout) {
        Ok(resp) => {
            r.reachable = true;
            let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap_or_default();
            r.health = v
                .get("status")
                .and_then(|s| s.as_str())
                .map(str::to_string)
                .or_else(|| Some(format!("HTTP {}", resp.status)));
            if resp.status != 200 {
                r.error = Some(format!("/health answered HTTP {}", resp.status));
            }
        }
        Err(e) => {
            r.error = Some(e.to_string());
            return r;
        }
    }
    if let Ok(resp) = http::get(&base, "/v1/models", timeout) {
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap_or_default();
        if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
            r.models = arr.iter().filter_map(|m| m.get("id")?.as_str().map(str::to_string)).collect();
        }
    }
    r
}

/// Probes Ollama's `/api/version` and `/api/tags` (loopback only).
pub fn probe_ollama(host: &str, timeout: Duration) -> OllamaApi {
    let mut r = OllamaApi { url: host.into(), ..Default::default() };
    let base = match LocalUrl::parse(host, 11434) {
        Ok(b) => b,
        Err(e) => {
            r.error = Some(e.to_string());
            return r;
        }
    };
    r.url = base.base();
    match http::get(&base, "/api/tags", timeout) {
        Ok(resp) if resp.status == 200 => {
            r.reachable = true;
            r.models = parse_ollama_tags(&resp.body);
        }
        Ok(resp) => {
            r.reachable = true;
            r.error = Some(format!("/api/tags answered HTTP {}", resp.status));
        }
        Err(e) => {
            r.error = Some(e.to_string());
            return r;
        }
    }
    if let Ok(resp) = http::get(&base, "/api/version", timeout) {
        let v: serde_json::Value = serde_json::from_str(&resp.body).unwrap_or_default();
        r.version = v.get("version").and_then(|s| s.as_str()).map(str::to_string);
    }
    r
}

/// Parses an Ollama `/api/tags` body.
pub fn parse_ollama_tags(body: &str) -> Vec<OllamaModel> {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let s = |m: &serde_json::Value, k: &str| m.get(k).and_then(|x| x.as_str()).map(str::to_string);
    v.get("models")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .map(|m| {
                    let d = m.get("details").cloned().unwrap_or_default();
                    OllamaModel {
                        name: s(m, "name").or_else(|| s(m, "model")).unwrap_or_default(),
                        size_bytes: m.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
                        family: s(&d, "family"),
                        parameter_size: s(&d, "parameter_size"),
                        quantization: s(&d, "quantization_level"),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A detection running on a background thread (for a GUI: call
/// [`DetectHandle::poll`] each frame). `Send`, so it can live in app state
/// that moves between threads.
#[derive(Debug)]
pub struct DetectHandle {
    rx: std::sync::mpsc::Receiver<RuntimeReport>,
    result: Option<RuntimeReport>,
    taken: bool,
}

impl DetectHandle {
    /// Takes the report once it is ready (`Some` exactly once).
    pub fn try_recv(&mut self) -> Option<RuntimeReport> {
        if let Some(r) = self.result.take() {
            self.taken = true;
            return Some(r);
        }
        if self.taken {
            return None;
        }
        match self.rx.try_recv() {
            Ok(r) => {
                self.taken = true;
                Some(r)
            }
            Err(_) => None,
        }
    }

    /// The report if ready, kept in the handle (repeatable, for per-frame UI).
    pub fn poll(&mut self) -> Option<&RuntimeReport> {
        if self.result.is_none() && !self.taken {
            if let Ok(r) = self.rx.try_recv() {
                self.result = Some(r);
            }
        }
        self.result.as_ref()
    }

    /// True once the report has arrived (or was taken).
    pub fn is_done(&mut self) -> bool {
        self.taken || self.poll().is_some()
    }

    /// Blocks until the report is ready (`None` if the worker died).
    pub fn wait(mut self) -> Option<RuntimeReport> {
        self.result.take().or_else(|| if self.taken { None } else { self.rx.recv().ok() })
    }
}

/// Runs [`detect`] on a new thread and returns at once.
pub fn spawn(opts: DetectOptions) -> DetectHandle {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("lillith-detect".into())
        .spawn(move || {
            let _ = tx.send(detect(&opts));
        })
        .expect("spawn detect thread");
    DetectHandle { rx, result: None, taken: false }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::fake_server;

    const TAGS: &str = r#"{"models":[{"name":"lily:latest","model":"lily:latest","size":2019393189,"details":{"family":"llama","parameter_size":"3.2B","quantization_level":"Q4_K_M"}}]}"#;

    #[test]
    fn ollama_probe_lists_models() {
        let port = fake_server(vec![("/api/tags", 200, TAGS), ("/api/version", 200, r#"{"version":"0.12.3"}"#)]);
        let r = probe_ollama(&format!("127.0.0.1:{port}"), Duration::from_secs(2));
        assert!(r.reachable, "{r:?}");
        assert_eq!(r.version.as_deref(), Some("0.12.3"));
        assert_eq!(r.models.len(), 1);
        assert_eq!(r.models[0].name, "lily:latest");
        assert_eq!(r.models[0].quantization.as_deref(), Some("Q4_K_M"));
    }

    #[test]
    fn llama_server_probe() {
        let port = fake_server(vec![
            ("/health", 200, r#"{"status":"ok"}"#),
            ("/v1/models", 200, r#"{"object":"list","data":[{"id":"qwen2.5-0.5b-instruct-q4_k_m.gguf"}]}"#),
        ]);
        let r = probe_llama_server(&format!("http://localhost:{port}"), Duration::from_secs(2));
        assert!(r.reachable);
        assert_eq!(r.health.as_deref(), Some("ok"));
        assert_eq!(r.models, vec!["qwen2.5-0.5b-instruct-q4_k_m.gguf".to_string()]);
    }

    #[test]
    fn nothing_running_is_not_a_panic() {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let r = probe_llama_server(&format!("http://127.0.0.1:{port}"), Duration::from_millis(200));
        assert!(!r.reachable && r.error.is_some());
        let o = probe_ollama(&format!("http://127.0.0.1:{port}"), Duration::from_millis(200));
        assert!(!o.reachable && o.error.is_some());
    }

    #[test]
    fn remote_hosts_are_refused_without_connecting() {
        let r = probe_ollama("http://192.168.1.50:11434", Duration::from_millis(200));
        assert!(!r.reachable);
        assert!(r.error.unwrap().contains("local-only"));
        let r = probe_llama_server("http://example.com:8080", Duration::from_millis(200));
        assert!(!r.reachable);
        assert!(r.error.unwrap().contains("not on this computer"));
    }

    #[test]
    fn finds_binaries_in_extra_dirs() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("lillith-setup-bin-{}", std::process::id()));
        let rel = root.join("llama-b9999");
        std::fs::create_dir_all(&rel).unwrap();
        let bin = rel.join("llama-server");
        std::fs::write(&bin, "#!/bin/sh\necho 'version: 9999 (test)' >&2\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let dirs = search_dirs(std::slice::from_ref(&root));
        let found = find_binary("llama-server", &dirs);
        let ver = found.as_deref().and_then(|p| binary_version(p, Duration::from_secs(2)));
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(found, Some(bin));
        assert_eq!(ver.as_deref(), Some("version: 9999 (test)"));
        assert!(find_binary("definitely-not-a-binary-xyz", &dirs).is_none());
    }

    #[test]
    fn spawn_is_send_and_polls() {
        fn assert_send<T: Send>() {}
        assert_send::<DetectHandle>();
        assert_send::<RuntimeReport>();
        let port = fake_server(vec![("/health", 200, r#"{"status":"ok"}"#)]);
        let opts = DetectOptions {
            llama_server_url: format!("http://127.0.0.1:{port}"),
            ollama_host: "http://127.0.0.1:9".into(),
            timeout: Duration::from_millis(500),
            probe_versions: false,
            ..Default::default()
        };
        let mut h = spawn(opts);
        let start = Instant::now();
        while h.poll().is_none() {
            assert!(start.elapsed() < Duration::from_secs(10), "detect never finished");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(h.poll().unwrap().llama_server_api.reachable);
        assert!(h.is_done());
        let r = h.try_recv().unwrap();
        assert_eq!(r.llama_server_api.health.as_deref(), Some("ok"));
        assert!(h.try_recv().is_none());
    }
}
