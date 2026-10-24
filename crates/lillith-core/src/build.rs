//! Local, offline project scaffolding / build / run for the settings-GUI
//! **Build** tab (Phase 3 of the companion plan — a Replit-style "vibe
//! coding" surface, but entirely local).
//!
//! Everything here uses tools that are already installed (`cargo`, `python3`,
//! `fish`) and never touches the network: cargo runs are `--offline` with
//! `CARGO_NET_OFFLINE=true`, and the Python/fish scaffolds just write plain
//! files. There is **no cloud and no download step**.
//!
//! This module only *does the work*; it never decides *whether* to. Each
//! scaffold/build/run is filed as a [`crate::soul_contract::PendingAction`]
//! ([`ScaffoldProject`]/[`BuildProject`]/[`RunProject`]) and only runs after
//! an explicit Soul Contract **Yes** (the Build tab's Yes/No buttons, the
//! same gate as `/contract yes` in chat). The caller is also responsible for
//! confining the project directory to the projects root with
//! [`resolve_project_dir`] before filing anything.
//!
//! [`ScaffoldProject`]: crate::soul_contract::PendingAction::ScaffoldProject
//! [`BuildProject`]: crate::soul_contract::PendingAction::BuildProject
//! [`RunProject`]: crate::soul_contract::PendingAction::RunProject

use std::path::{Component, Path, PathBuf};

/// The languages the Build tab can scaffold, build and run. Each maps to an
/// already-installed, offline toolchain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// `cargo` (`cargo new`/`build`/`run`, always `--offline`).
    Rust,
    /// `python3` (plain files; compile check via `py_compile`).
    Python,
    /// `fish` (plain script; syntax check via `fish -n`).
    Fish,
}

impl Language {
    /// All languages, in the order the tab offers them.
    pub const ALL: [Language; 3] = [Language::Rust, Language::Python, Language::Fish];

    /// The lowercase config / wire token (`rust`, `python`, `fish`).
    pub fn as_str(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Python => "python",
            Language::Fish => "fish",
        }
    }

    /// The label shown on the tab.
    pub fn label(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::Python => "Python",
            Language::Fish => "fish",
        }
    }

    /// Parses a token like `rust`/`rs`/`python`/`py`/`fish` (case-insensitive).
    pub fn parse(s: &str) -> Option<Language> {
        match s.trim().to_ascii_lowercase().as_str() {
            "rust" | "rs" | "cargo" => Some(Language::Rust),
            "python" | "python3" | "py" => Some(Language::Python),
            "fish" => Some(Language::Fish),
            _ => None,
        }
    }

    /// The templates offered for **New project**. The first is the default.
    pub fn templates(self) -> &'static [&'static str] {
        match self {
            Language::Rust => &["bin", "lib"],
            Language::Python => &["script", "package"],
            Language::Fish => &["script"],
        }
    }

    /// The default template (first of [`Language::templates`]).
    pub fn default_template(self) -> &'static str {
        self.templates()[0]
    }

    /// Whether `template` is one this language offers.
    pub fn has_template(self, template: &str) -> bool {
        self.templates().contains(&template)
    }
}

/// Why a project directory could not be resolved or scaffolded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BuildError {
    #[error("give the project a name")]
    EmptyName,
    #[error("the project folder must sit inside the projects root, not {0}")]
    OutsideRoot(PathBuf),
    #[error("unknown template {0:?} for {1}")]
    BadTemplate(String, &'static str),
    #[error("{0} already exists and is not empty — pick another name")]
    NotEmpty(PathBuf),
}

/// Lexically normalises a path (no filesystem access): expands a leading `~`,
/// anchors a relative path at `$HOME`, and resolves `.`/`..`. This is the same
/// confinement normalisation the file tools use, kept here so `lillith-core`
/// stays free of `lillith-mind`.
pub fn normalize(path: &Path) -> PathBuf {
    let expanded: PathBuf = match path.to_str() {
        Some("~") => std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_path_buf()),
        Some(s) if s.starts_with("~/") => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(&s[2..]),
            None => path.to_path_buf(),
        },
        _ => path.to_path_buf(),
    };
    let anchored = if expanded.is_absolute() {
        expanded
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        home.join(expanded)
    } else {
        expanded
    };
    let mut out = PathBuf::new();
    for comp in anchored.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(c) => out.push(c),
        }
    }
    out
}

/// Confines a user-typed project name/path to `projects_root` and returns the
/// absolute project directory.
///
/// A bare name (`hello`) is taken relative to the root; an absolute or `~`
/// path is normalised and must still land inside the root. `..` can never
/// climb out (it is resolved lexically first). The root itself is rejected —
/// a project needs its own folder.
pub fn resolve_project_dir(projects_root: &Path, name: &str) -> Result<PathBuf, BuildError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(BuildError::EmptyName);
    }
    let root = normalize(projects_root);
    let candidate = if name.starts_with('/') || name.starts_with('~') {
        normalize(Path::new(name))
    } else {
        normalize(&root.join(name))
    };
    if candidate == root || !candidate.starts_with(&root) {
        return Err(BuildError::OutsideRoot(candidate));
    }
    Ok(candidate)
}

/// The project's short name (its last path component), used for crate names,
/// greetings and so on.
pub fn project_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".into())
}

// ---------------------------------------------------------------------------
// Human-readable summaries (shown on the Soul Contract Yes/No prompt).
// ---------------------------------------------------------------------------

/// One-line summary of a scaffold action.
pub fn scaffold_summary(dir: &Path, lang: Language, template: &str) -> String {
    format!(
        "New {} project ({template}) in {}",
        lang.label(),
        dir.display()
    )
}

/// One-line summary of a build action.
pub fn build_summary(dir: &Path, lang: Language) -> String {
    format!("Build {} project {}", lang.label(), dir.display())
}

/// One-line summary of a run action.
pub fn run_summary(dir: &Path, lang: Language) -> String {
    format!("Run {} project {}", lang.label(), dir.display())
}

/// A preview of the command(s) a build/run/scaffold would execute, for the UI.
pub fn preview(dir: &Path, lang: Language, action: Action, template: &str) -> String {
    let name = project_name(dir);
    match (action, lang) {
        (Action::Scaffold, Language::Rust) => {
            let kind = if template == "lib" { " --lib" } else { "" };
            format!("cargo new --offline --vcs none{kind} {}", dir.display())
        }
        (Action::Scaffold, Language::Python) => {
            format!("write {template} files into {}", dir.display())
        }
        (Action::Scaffold, Language::Fish) => format!("write main.fish into {}", dir.display()),
        (Action::Build, Language::Rust) => "cargo build --offline".into(),
        (Action::Build, Language::Python) => "python3 -m py_compile main.py".into(),
        (Action::Build, Language::Fish) => "fish -n main.fish".into(),
        (Action::Run, Language::Rust) => "cargo run --offline".into(),
        (Action::Run, Language::Python) => "python3 main.py".into(),
        (Action::Run, Language::Fish) => format!("fish main.fish  # {name}"),
    }
}

/// Which step of the build lifecycle a preview/command is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Scaffold,
    Build,
    Run,
}

// ---------------------------------------------------------------------------
// Doing the work (only reached after a Soul Contract Yes).
// ---------------------------------------------------------------------------

/// Scaffolds a new project into `dir`. Rust shells out to `cargo new`
/// (offline); Python and fish write plain files directly. Returns a short
/// report of what happened.
pub fn scaffold(dir: &Path, lang: Language, template: &str) -> std::io::Result<String> {
    if !lang.has_template(template) {
        return Err(io_err(format!(
            "unknown template {template:?} for {}",
            lang.label()
        )));
    }
    if dir_has_entries(dir)? {
        return Err(io_err(format!(
            "{} already exists and is not empty",
            dir.display()
        )));
    }
    match lang {
        Language::Rust => scaffold_rust(dir, template),
        Language::Python => scaffold_python(dir, template),
        Language::Fish => scaffold_fish(dir),
    }
}

/// Receives each output line (stdout or stderr) as soon as the command
/// prints it. See [`build_streaming`] / [`run_streaming`].
pub type LineSink<'a> = &'a mut dyn FnMut(&str);

/// Builds (compiles / syntax-checks) the project in `dir`, offline.
pub fn build(dir: &Path, lang: Language) -> std::io::Result<String> {
    build_impl(dir, lang, None)
}

/// [`build`], but every stdout/stderr line also goes to `sink` while the
/// command runs. The returned text is the same full log.
pub fn build_streaming(dir: &Path, lang: Language, sink: LineSink<'_>) -> std::io::Result<String> {
    build_impl(dir, lang, Some(sink))
}

fn build_impl(dir: &Path, lang: Language, sink: Option<LineSink<'_>>) -> std::io::Result<String> {
    match lang {
        Language::Rust => run_cmd(
            "cargo",
            &["build", "--offline"],
            dir,
            &[("CARGO_NET_OFFLINE", "true")],
            sink,
        ),
        Language::Python => run_cmd("python3", &["-m", "py_compile", "main.py"], dir, &[], sink),
        Language::Fish => run_cmd("fish", &["-n", "main.fish"], dir, &[], sink),
    }
}

/// Runs the project in `dir`, offline.
pub fn run(dir: &Path, lang: Language) -> std::io::Result<String> {
    run_impl(dir, lang, None)
}

/// [`run`], but every stdout/stderr line also goes to `sink` while the
/// program runs. The returned text is the same full log.
pub fn run_streaming(dir: &Path, lang: Language, sink: LineSink<'_>) -> std::io::Result<String> {
    run_impl(dir, lang, Some(sink))
}

fn run_impl(dir: &Path, lang: Language, sink: Option<LineSink<'_>>) -> std::io::Result<String> {
    match lang {
        Language::Rust => run_cmd(
            "cargo",
            &["run", "--offline"],
            dir,
            &[("CARGO_NET_OFFLINE", "true")],
            sink,
        ),
        // Unbuffered so print() output streams to the log as it happens.
        Language::Python => run_cmd(
            "python3",
            &["main.py"],
            dir,
            &[("PYTHONUNBUFFERED", "1")],
            sink,
        ),
        Language::Fish => run_cmd("fish", &["main.fish"], dir, &[], sink),
    }
}

fn scaffold_rust(dir: &Path, template: &str) -> std::io::Result<String> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut args: Vec<&str> = vec!["new", "--offline", "--vcs", "none"];
    if template == "lib" {
        args.push("--lib");
    }
    let dir_str = dir.to_string_lossy().to_string();
    args.push(&dir_str);
    let cwd = dir.parent().unwrap_or(Path::new("."));
    run_cmd("cargo", &args, cwd, &[("CARGO_NET_OFFLINE", "true")], None)
}

fn scaffold_python(dir: &Path, template: &str) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    let name = project_name(dir);
    let mut written = Vec::new();
    let main = format!(
        "def main() -> None:\n    print(\"Hello from {name}\")\n\n\nif __name__ == \"__main__\":\n    main()\n"
    );
    write_file(dir, "main.py", &main, false, &mut written)?;
    write_file(
        dir,
        "README.md",
        &format!("# {name}\n\nA local Python project scaffolded by Lillith's Build tab.\n\nRun: `python3 main.py`\n"),
        false,
        &mut written,
    )?;
    if template == "package" {
        let pkg = sanitize_module(&name);
        let pkg_dir = dir.join(&pkg);
        std::fs::create_dir_all(&pkg_dir)?;
        let rel_init = format!("{pkg}/__init__.py");
        write_file(
            dir,
            &rel_init,
            &format!("def greet() -> str:\n    return \"Hello from {name}\"\n"),
            false,
            &mut written,
        )?;
        write_file(
            dir,
            "pyproject.toml",
            &format!(
                "[project]\nname = \"{pkg}\"\nversion = \"0.1.0\"\ndescription = \"Local project scaffolded by Lillith\"\nrequires-python = \">=3.9\"\n"
            ),
            false,
            &mut written,
        )?;
        // Make main.py use the package so `python3 main.py` exercises it.
        let main_pkg = format!(
            "from {pkg} import greet\n\n\ndef main() -> None:\n    print(greet())\n\n\nif __name__ == \"__main__\":\n    main()\n"
        );
        std::fs::write(dir.join("main.py"), main_pkg)?;
    }
    Ok(scaffold_report(dir, &written))
}

fn scaffold_fish(dir: &Path) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    let name = project_name(dir);
    let mut written = Vec::new();
    let main = format!(
        "#!/usr/bin/env fish\n\nfunction main\n    echo \"Hello from {name}\"\nend\n\nmain $argv\n"
    );
    write_file(dir, "main.fish", &main, true, &mut written)?;
    write_file(
        dir,
        "README.md",
        &format!("# {name}\n\nA local fish project scaffolded by Lillith's Build tab.\n\nRun: `fish main.fish`\n"),
        false,
        &mut written,
    )?;
    Ok(scaffold_report(dir, &written))
}

fn scaffold_report(dir: &Path, written: &[String]) -> String {
    let mut s = format!("Scaffolded {}:\n", dir.display());
    for w in written {
        s.push_str("  + ");
        s.push_str(w);
        s.push('\n');
    }
    s.push_str("[ok]");
    s
}

/// Writes `rel` under `dir` (optionally executable) and records it in `written`.
fn write_file(
    dir: &Path,
    rel: &str,
    body: &str,
    executable: bool,
    written: &mut Vec<String>,
) -> std::io::Result<()> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, body)?;
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&path)?.permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&path, perm)?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    written.push(rel.to_string());
    Ok(())
}

/// A conservative Python module name: lowercase, non-alphanumerics to `_`,
/// never leading with a digit.
fn sanitize_module(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "app".into()
    } else if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("app_{out}")
    } else {
        out
    }
}

/// Whether `dir` exists and contains at least one entry.
fn dir_has_entries(dir: &Path) -> std::io::Result<bool> {
    match std::fs::read_dir(dir) {
        Ok(mut rd) => Ok(rd.next().is_some()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Runs `program args...` in `cwd` with extra `envs`, capturing combined
/// stdout+stderr and appending an `[exit N]` line. A non-zero exit is returned
/// as `Ok` (the output carries the code); only a failure to launch is an
/// `Err`. No network is ever used by the commands this module builds.
///
/// With a `sink`, stdout and stderr are read line by line on two reader
/// threads and each line is handed to `sink` as it arrives (interleaved in
/// arrival order, which is also the order of the returned text).
fn run_cmd(
    program: &str,
    args: &[&str],
    cwd: &Path,
    envs: &[(&str, &str)],
    sink: Option<LineSink<'_>>,
) -> std::io::Result<String> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).current_dir(cwd);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let (mut text, status) = match sink {
        None => {
            let out = cmd.output()?;
            let mut text = String::new();
            text.push_str(&String::from_utf8_lossy(&out.stdout));
            if !out.stderr.is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&String::from_utf8_lossy(&out.stderr));
            }
            (text, out.status)
        }
        Some(sink) => stream_cmd(&mut cmd, sink)?,
    };
    let code = status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".into());
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("[exit {code}]"));
    Ok(text)
}

/// Spawns `cmd` with piped stdout/stderr and forwards each line to `sink`
/// as soon as it is read. Returns the collected text and the exit status.
fn stream_cmd(
    cmd: &mut std::process::Command,
    sink: LineSink<'_>,
) -> std::io::Result<(String, std::process::ExitStatus)> {
    use std::io::BufRead;
    use std::process::Stdio;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let mut readers = Vec::new();
    let out: Option<Box<dyn std::io::Read + Send>> = child
        .stdout
        .take()
        .map(|o| Box::new(o) as Box<dyn std::io::Read + Send>);
    let err: Option<Box<dyn std::io::Read + Send>> = child
        .stderr
        .take()
        .map(|e| Box::new(e) as Box<dyn std::io::Read + Send>);
    for pipe in [out, err].into_iter().flatten() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            let mut r = std::io::BufReader::new(pipe);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match r.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let line = String::from_utf8_lossy(&buf);
                        let line = line.trim_end_matches(['\n', '\r']).to_string();
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                }
            }
        }));
    }
    drop(tx);
    let mut text = String::new();
    // Ends once both pipes are closed (the child exited or closed them).
    for line in rx {
        sink(&line);
        text.push_str(&line);
        text.push('\n');
    }
    for r in readers {
        let _ = r.join();
    }
    let status = child.wait()?;
    Ok((text, status))
}

fn io_err(msg: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn streaming_delivers_lines_before_the_command_exits() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let mut seen: Vec<(String, Duration)> = Vec::new();
        let mut sink = |l: &str| seen.push((l.to_string(), t0.elapsed()));
        let out = run_cmd(
            "sh",
            &["-c", "echo first; sleep 1; echo oops >&2; echo last"],
            &std::env::temp_dir(),
            &[],
            Some(&mut sink),
        )
        .unwrap();
        let total = t0.elapsed();
        let lines: Vec<&str> = seen.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[0], "first");
        assert!(lines.contains(&"oops"), "stderr is streamed too: {lines:?}");
        assert!(lines.contains(&"last"));
        // "first" arrived while the command was still sleeping.
        assert!(
            seen[0].1 + Duration::from_millis(500) < total,
            "first line at {:?}, finished at {total:?}",
            seen[0].1
        );
        assert!(out.starts_with("first\n"), "{out}");
        assert!(out.contains("oops") && out.ends_with("[exit 0]"), "{out}");
    }

    #[cfg(unix)]
    #[test]
    fn non_streaming_output_is_unchanged() {
        let out = run_cmd(
            "sh",
            &["-c", "echo a; echo b >&2; exit 3"],
            &std::env::temp_dir(),
            &[],
            None,
        )
        .unwrap();
        assert_eq!(out, "a\nb\n[exit 3]");
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "lillith-build-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn language_tokens_round_trip() {
        for l in Language::ALL {
            assert_eq!(Language::parse(l.as_str()), Some(l));
            assert!(l.has_template(l.default_template()));
        }
        assert_eq!(Language::parse("RS"), Some(Language::Rust));
        assert_eq!(Language::parse("py"), Some(Language::Python));
        assert_eq!(Language::parse("nope"), None);
    }

    #[test]
    fn resolve_confines_to_root() {
        let root = tmpdir("confine");
        // Bare name lands inside the root.
        let p = resolve_project_dir(&root, "hello").unwrap();
        assert_eq!(p, root.join("hello"));
        // Nested bare names are fine.
        let n = resolve_project_dir(&root, "a/b").unwrap();
        assert_eq!(n, root.join("a").join("b"));
        // `..` cannot climb out.
        assert!(matches!(
            resolve_project_dir(&root, "../evil"),
            Err(BuildError::OutsideRoot(_))
        ));
        // An absolute path outside the root is rejected.
        assert!(matches!(
            resolve_project_dir(&root, "/etc/passwd"),
            Err(BuildError::OutsideRoot(_))
        ));
        // The root itself is not a project.
        assert!(matches!(
            resolve_project_dir(&root, "."),
            Err(BuildError::OutsideRoot(_))
        ));
        assert!(matches!(
            resolve_project_dir(&root, "  "),
            Err(BuildError::EmptyName)
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scaffold_python_script_is_offline_and_runnable_shape() {
        let root = tmpdir("py");
        let dir = root.join("demo");
        let report = scaffold(&dir, Language::Python, "script").unwrap();
        assert!(report.contains("main.py"), "{report}");
        assert!(report.ends_with("[ok]"), "{report}");
        let main = std::fs::read_to_string(dir.join("main.py")).unwrap();
        assert!(main.contains("Hello from demo"), "{main}");
        assert!(dir.join("README.md").exists());
        // Scaffolding again refuses to clobber a non-empty dir.
        assert!(scaffold(&dir, Language::Python, "script").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scaffold_python_package_builds_a_module() {
        let root = tmpdir("pypkg");
        let dir = root.join("MyApp");
        scaffold(&dir, Language::Python, "package").unwrap();
        assert!(dir.join("pyproject.toml").exists());
        assert!(dir.join("myapp").join("__init__.py").exists());
        let main = std::fs::read_to_string(dir.join("main.py")).unwrap();
        assert!(main.contains("from myapp import greet"), "{main}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scaffold_fish_marks_executable() {
        let root = tmpdir("fish");
        let dir = root.join("sh");
        scaffold(&dir, Language::Fish, "script").unwrap();
        let script = dir.join("main.fish");
        assert!(script.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&script).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "fish script should be executable");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_template_is_rejected() {
        let root = tmpdir("badtpl");
        let dir = root.join("x");
        assert!(scaffold(&dir, Language::Python, "nope").is_err());
        assert!(!dir.exists(), "nothing created on a bad template");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn previews_are_offline() {
        let dir = Path::new("/home/ko/Projects/lillith-builds/demo");
        assert!(preview(dir, Language::Rust, Action::Build, "bin").contains("--offline"));
        assert!(preview(dir, Language::Rust, Action::Run, "bin").contains("--offline"));
        assert!(preview(dir, Language::Rust, Action::Scaffold, "lib").contains("--lib"));
        // No preview mentions a network/registry fetch.
        for a in [Action::Scaffold, Action::Build, Action::Run] {
            for l in Language::ALL {
                let p = preview(dir, l, a, l.default_template());
                assert!(!p.contains("http") && !p.contains("pip install"), "{p}");
            }
        }
    }

    #[test]
    fn build_and_run_fish_locally() {
        // fish is a pure-local toolchain; exercise the real build+run path.
        if which("fish").is_none() {
            return;
        }
        let root = tmpdir("fishrun");
        let dir = root.join("hi");
        scaffold(&dir, Language::Fish, "script").unwrap();
        let checked = build(&dir, Language::Fish).unwrap();
        assert!(checked.contains("[exit 0]"), "{checked}");
        let ran = run(&dir, Language::Fish).unwrap();
        assert!(ran.contains("Hello from hi"), "{ran}");
        assert!(ran.contains("[exit 0]"), "{ran}");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn which(bin: &str) -> Option<PathBuf> {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|p| p.join(bin))
                .find(|p| p.is_file())
        })
    }
}
