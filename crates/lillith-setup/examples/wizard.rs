//! Terminal front-end for the setup wizard (usable before the GUI panel).
//!
//! ```text
//! cargo run -p lillith-setup --example wizard -- scan [--dir D]... [--bin-dir D]... [--json]
//! cargo run -p lillith-setup --example wizard -- detect [--json]
//! cargo run -p lillith-setup --example wizard -- modelfile [--gguf P] [--out FILE]
//! cargo run -p lillith-setup --example wizard -- write-config [--gguf P] [--backend llama-server|ollama]
//!                                                  [--config FILE] [--modelfile FILE] [--sync-chat]
//! cargo run -p lillith-setup --example wizard -- launch-cmd [--config FILE]
//! cargo run -p lillith-setup --example wizard -- check-modelfile FILE
//! cargo run -p lillith-setup --example wizard -- ollama-create [--config FILE] [--modelfile FILE]
//! cargo run -p lillith-setup --example wizard -- check [--config FILE] [--server-url U] [PROMPT]
//! ```
//! Without `--gguf`, the top-ranked scanned model is used. Local-only: the
//! only network traffic is loopback health checks.

use lillith_setup::*;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

struct Args {
    cmd: String,
    positional: Vec<String>,
    dirs: Vec<PathBuf>,
    bin_dirs: Vec<PathBuf>,
    gguf: Option<String>,
    out: Option<PathBuf>,
    config: Option<PathBuf>,
    modelfile: Option<PathBuf>,
    backend: Option<String>,
    server_url: Option<String>,
    ollama_host: Option<String>,
    sync_chat: bool,
    json: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let mut a = Args {
        cmd: String::new(), positional: vec![], dirs: vec![], bin_dirs: vec![], gguf: None, out: None,
        config: None, modelfile: None, backend: None, server_url: None, ollama_host: None, sync_chat: false, json: false,
    };
    while let Some(x) = it.next() {
        let mut val = |n: &str| it.next().ok_or(format!("{n} needs a value"));
        match x.as_str() {
            "--dir" => a.dirs.push(paths::expand_tilde(&val("--dir")?)),
            "--bin-dir" => a.bin_dirs.push(paths::expand_tilde(&val("--bin-dir")?)),
            "--gguf" => a.gguf = Some(val("--gguf")?),
            "--out" => a.out = Some(paths::expand_tilde(&val("--out")?)),
            "--config" => a.config = Some(paths::expand_tilde(&val("--config")?)),
            "--modelfile" => a.modelfile = Some(paths::expand_tilde(&val("--modelfile")?)),
            "--backend" => a.backend = Some(val("--backend")?),
            "--server-url" => a.server_url = Some(val("--server-url")?),
            "--ollama-host" => a.ollama_host = Some(val("--ollama-host")?),
            "--sync-chat" => a.sync_chat = true,
            "--json" => a.json = true,
            "-h" | "--help" => a.cmd = "help".into(),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s if a.cmd.is_empty() => a.cmd = s.into(),
            s => a.positional.push(s.into()),
        }
    }
    if a.cmd.is_empty() {
        a.cmd = "scan".into();
    }
    Ok(a)
}

fn gb(b: u64) -> String {
    format!("{:.2} GB", b as f64 / 1024f64.powi(3))
}

fn scan_opts(a: &Args, cfg: Option<&LlmConfig>) -> ScanOptions {
    let mut o = ScanOptions::default();
    let mut extra: Vec<PathBuf> = a.dirs.clone();
    if let Some(c) = cfg {
        extra.extend(c.scan_paths.iter().map(|p| paths::expand_tilde(p)));
    }
    extra.retain(|d| !o.dirs.contains(d));
    o.dirs.splice(0..0, extra);
    o
}

fn detect_opts(a: &Args) -> DetectOptions {
    let mut o = DetectOptions { extra_bin_dirs: a.bin_dirs.clone(), ..Default::default() };
    if let Some(u) = &a.server_url { o.llama_server_url = u.clone(); }
    if let Some(h) = &a.ollama_host { o.ollama_host = h.clone(); }
    o
}

fn print_detect(r: &RuntimeReport) {
    println!("== Runtimes ==");
    let bin = |n: &str, b: &Option<detect::BinaryInfo>| match b {
        Some(b) => println!("  {n:<13} {}  [{}]", b.path.display(), b.version.as_deref().unwrap_or("version ?")),
        None => println!("  {n:<13} not found"),
    };
    bin("llama-server", &r.llama_server);
    bin("llama-cli", &r.llama_cli);
    bin("ollama", &r.ollama);
    let s = &r.llama_server_api;
    if s.reachable {
        println!("  llama-server API {}  health={} models={:?}", s.url, s.health.as_deref().unwrap_or("?"), s.models);
    } else {
        println!("  llama-server API {}  not answering ({})", s.url, s.error.as_deref().unwrap_or("-"));
    }
    let o = &r.ollama_api;
    if o.reachable {
        println!("  ollama API       {}  version={} models:", o.url, o.version.as_deref().unwrap_or("?"));
        for m in &o.models {
            println!("    - {} ({}, {} {})", m.name, gb(m.size_bytes), m.parameter_size.as_deref().unwrap_or("?"), m.quantization.as_deref().unwrap_or("?"));
        }
    } else {
        println!("  ollama API       {}  not answering ({})", o.url, o.error.as_deref().unwrap_or("-"));
    }
    println!(
        "  suggested backend: {}",
        r.suggested_backend().map(|b| b.as_str()).unwrap_or("none installed (llama-server recommended)")
    );
}

fn print_scan(rep: &ScanReport, recs: &[Recommendation]) {
    println!("== Searched ==");
    for d in &rep.dirs {
        println!("  [{}] {}{}", if d.exists { "x" } else { " " }, d.path.display(),
            if d.models_found > 0 { format!("  ({} found)", d.models_found) } else { String::new() });
    }
    for w in &rep.warnings {
        println!("  warning: {w}");
    }
    println!("== Models ({}) ==", rep.models.len());
    for m in &rep.models {
        println!("  {}", m.path.display());
        println!(
            "      name={:?} arch={} ctx={} quant={} params={}{} size={} est_ram={} template={}{}{}",
            m.name,
            m.architecture.as_deref().unwrap_or("?"),
            m.context_length.map(|c| c.to_string()).unwrap_or("?".into()),
            m.quant.as_deref().unwrap_or("?"),
            m.params_b.map(|p| format!("{p:.2}B")).unwrap_or("?".into()),
            if m.params_estimated { "(est)" } else { "" },
            gb(m.size_bytes),
            gb(m.est_ram_bytes),
            m.chat_template.llama_server_name(),
            if m.source == scan::ModelSource::OllamaBlob { " [ollama blob]" } else { "" },
            m.header_error.as_ref().map(|e| format!(" ERROR: {e}")).unwrap_or_default(),
        );
    }
    println!("== Recommendations (RAM {}) ==", scan::total_ram_bytes().map(gb).unwrap_or("unknown".into()));
    for (n, r) in recs.iter().enumerate() {
        println!("  {}. [{:?}] score {:>4}  {}  ({})", n + 1, r.tier, r.score, r.name, r.path.display());
        println!("       {}", r.reasons.join("; "));
    }
}

/// The model to use: `--gguf` (read now) or the top recommendation.
fn pick_model(a: &Args, cfg: Option<&LlmConfig>) -> Result<ModelInfo, String> {
    if let Some(g) = &a.gguf {
        let p = paths::expand_tilde(g);
        return ModelInfo::from_path(&p, scan::ModelSource::File).map_err(|e| format!("{}: {e}", p.display()));
    }
    let rep = scan(&scan_opts(a, cfg));
    let recs = recommend(&rep.models, scan::total_ram_bytes());
    recs.first()
        .map(|r| rep.models[r.index].clone())
        .ok_or_else(|| "no .gguf models found; pass --gguf PATH or --dir DIR".into())
}

fn config_path(a: &Args) -> Result<PathBuf, String> {
    a.config.clone().or_else(paths::default_config_path).ok_or("no $HOME; pass --config".into())
}

fn run(a: Args) -> Result<(), String> {
    match a.cmd.as_str() {
        "help" => {
            println!("{}", include_str!("wizard.rs").lines().skip(2).take(11).map(|l| l.trim_start_matches("//!")).collect::<Vec<_>>().join("\n"));
        }
        "detect" => {
            let r = detect(&detect_opts(&a));
            if a.json { println!("{}", serde_json::to_string_pretty(&r).unwrap()); } else { print_detect(&r); }
        }
        "scan" => {
            let cfg = config_path(&a).ok().and_then(|p| config::load(&p).ok());
            let r = detect(&detect_opts(&a));
            let rep = scan(&scan_opts(&a, cfg.as_ref()));
            let recs = recommend(&rep.models, scan::total_ram_bytes());
            if a.json {
                let v = serde_json::json!({ "runtimes": r, "scan": rep, "recommendations": recs });
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            } else {
                print_detect(&r);
                print_scan(&rep, &recs);
                if let Some(top) = recs.first() {
                    let m = &rep.models[top.index];
                    let c = wizard::config_for_model(m, r.suggested_backend().unwrap_or(Backend::LlamaServer));
                    if let Ok(l) = launch::llama_server_command(&c, r.llama_server.as_ref().map(|b| b.path.as_path())) {
                        println!("== Launch (top pick) ==\n  {}", l.to_shell_string());
                    }
                }
            }
        }
        "modelfile" => {
            let cfg = config_path(&a).ok().and_then(|p| config::load(&p).ok()).unwrap_or_default();
            let m = pick_model(&a, Some(&cfg))?;
            let c = LlmConfig { ctx_size: wizard::suggested_ctx(&m).min(cfg.ctx_size), ..cfg };
            let text = wizard::modelfile_for(&m, &c).render();
            match &a.out {
                Some(p) => {
                    if let Some(d) = p.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
                    std::fs::write(p, &text).map_err(|e| format!("{}: {e}", p.display()))?;
                    println!("wrote {}", p.display());
                }
                None => print!("{text}"),
            }
        }
        "write-config" => {
            let path = config_path(&a)?;
            let old = config::load(&path).map_err(|e| e.to_string())?;
            let m = pick_model(&a, Some(&old))?;
            let backend = match &a.backend {
                Some(b) => Backend::parse(b).ok_or(format!("bad --backend {b}"))?,
                None => old.backend,
            };
            let rt = detect(&DetectOptions { probe_versions: false, ..detect_opts(&a) });
            let mut c = LlmConfig { scan_paths: old.scan_paths.clone(), system_prompt: old.system_prompt.clone(),
                temperature: old.temperature, ollama_model: old.ollama_model.clone(), ..wizard::config_for_model(&m, backend) };
            if let Some(u) = &a.server_url { c.server_url = u.clone(); } else { c.server_url = old.server_url.clone(); }
            if let Some(h) = &a.ollama_host { c.ollama_host = h.clone(); } else { c.ollama_host = old.ollama_host.clone(); }
            c.llama_server_bin = rt.llama_server.as_ref().map(|b| b.path.display().to_string()).unwrap_or(old.llama_server_bin.clone());
            let mf_path = a.modelfile.clone().or_else(paths::default_modelfile_path).ok_or("no modelfile path")?;
            c.modelfile_path = mf_path.display().to_string();
            config::save(&path, &c, a.sync_chat).map_err(|e| e.to_string())?;
            if let Some(d) = mf_path.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
            std::fs::write(&mf_path, wizard::modelfile_for(&m, &c).render()).map_err(|e| e.to_string())?;
            println!("wrote [llm]{} to {}", if a.sync_chat { " + [chat] bridge" } else { "" }, path.display());
            println!("wrote Modelfile to {}", mf_path.display());
            println!("model: {} ({})", m.name, m.path.display());
            if let Ok(l) = launch::llama_server_command(&c, None) { println!("launch: {}", l.to_shell_string()); }
        }
        "launch-cmd" => {
            let c = config::load(&config_path(&a)?).map_err(|e| e.to_string())?;
            println!("{}", launch::llama_server_command(&c, None).map_err(|e| e.to_string())?.to_shell_string());
        }
        "check-modelfile" => {
            let p = a.positional.first().ok_or("check-modelfile FILE")?;
            let src = std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?;
            let m = Modelfile::parse(&src).map_err(|e| e.to_string())?;
            let again = Modelfile::parse(&m.render()).map_err(|e| e.to_string())?;
            println!("{} instructions, FROM={:?}, round-trip {}", m.instructions.len(), m.from(), if again == m { "OK" } else { "CHANGED" });
            print!("{}", m.render());
        }
        "ollama-create" => {
            let c = config::load(&config_path(&a)?).map_err(|e| e.to_string())?;
            let rt = detect(&DetectOptions { probe_versions: false, ..detect_opts(&a) });
            let bin = rt.ollama.ok_or("ollama is not installed")?.path;
            let mf = a.modelfile.clone()
                .or_else(|| (!c.modelfile_path.is_empty()).then(|| PathBuf::from(&c.modelfile_path)))
                .or_else(paths::default_modelfile_path).ok_or("no Modelfile")?;
            println!("{}", launch::ollama_create(&bin, &c.ollama_model, Path::new(&mf), &c).map_err(|e| e.to_string())?);
        }
        "check" => {
            let mut c = config_path(&a).ok().and_then(|p| config::load(&p).ok()).unwrap_or_default();
            if let Some(b) = &a.backend { c.backend = Backend::parse(b).ok_or(format!("bad --backend {b}"))?; }
            if let Some(u) = &a.server_url { c.server_url = u.clone(); }
            if let Some(h) = &a.ollama_host { c.ollama_host = h.clone(); }
            for e in validate_all(&c) { println!("  field {}: {}", e.key, e.message); }
            let h = check::ping_server(&c).map_err(|e| e.to_string())?;
            println!("ping: ready={} status={:?} model={:?} ctx={:?} version={:?}", h.ready, h.status, h.model, h.ctx, h.version);
            if h.ready {
                let prompt = a.positional.first().map(String::as_str).unwrap_or("Say hi to Ko in one short sentence.");
                let t = std::time::Instant::now();
                let reply = check::test_chat(&c, prompt).map_err(|e| e.to_string())?;
                println!("chat ({:.1}s): {reply}", t.elapsed().as_secs_f32());
            }
        }
        other => return Err(format!("unknown command `{other}` (try --help)")),
    }
    Ok(())
}

fn main() -> ExitCode {
    match parse_args().and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
