//! Installed programs: `.desktop` apps and `PATH` binaries. Lists them, checks
//! whether something is installed, and builds the argv to open an app
//! (optionally with files). Launching itself goes through the Soul Contract
//! ([`PendingAction::Launch`](crate::soul_contract::PendingAction::Launch)).

use std::path::{Path, PathBuf};

/// One application from a `.desktop` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopApp {
    /// Desktop id (`org.kde.dolphin`).
    pub id: String,
    /// Display name (`Dolphin`).
    pub name: String,
    /// Generic name (`File Manager`), if any.
    pub generic: String,
    /// The `Exec=` line.
    pub exec: String,
    pub file: PathBuf,
    pub terminal: bool,
}

/// Folders holding `.desktop` files, user first.
pub fn application_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|x| !x.is_empty())
        .map(PathBuf::from)
        .or_else(|| super::home().map(|h| h.join(".local/share")));
    if let Some(d) = data_home {
        v.push(d.join("applications"));
        v.push(d.join("flatpak/exports/share/applications"));
    }
    let dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for d in dirs.split(':').filter(|d| !d.is_empty()) {
        v.push(PathBuf::from(d).join("applications"));
    }
    v.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    v.dedup();
    v
}

/// Parses the `[Desktop Entry]` group of one `.desktop` file. Hidden /
/// `NoDisplay` entries and non-applications return `None`.
pub fn parse_desktop(file: &Path, id: &str) -> Option<DesktopApp> {
    let text = std::fs::read_to_string(file).ok()?;
    let mut in_main = false;
    let (mut name, mut generic, mut exec) = (String::new(), String::new(), String::new());
    let (mut terminal, mut hidden, mut is_app) = (false, false, false);
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_main = l == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "Name" => name = v.to_string(),
            "GenericName" => generic = v.to_string(),
            "Exec" => exec = v.to_string(),
            "Terminal" => terminal = v.eq_ignore_ascii_case("true"),
            "NoDisplay" | "Hidden" => hidden |= v.eq_ignore_ascii_case("true"),
            "Type" => is_app = v == "Application",
            _ => {}
        }
    }
    (is_app && !hidden && !exec.is_empty() && !name.is_empty()).then(|| DesktopApp {
        id: id.to_string(),
        name,
        generic,
        exec,
        file: file.to_path_buf(),
        terminal,
    })
}

/// Every visible installed app, first definition of an id wins, sorted by name.
/// On Windows: Start Menu shortcuts plus the App Paths / Uninstall registry
/// keys (see [`super::winprograms`]).
pub fn desktop_apps() -> Vec<DesktopApp> {
    #[cfg(windows)]
    {
        super::winprograms::installed()
            .into_iter()
            .filter_map(|p| {
                let t = p.target.clone()?;
                Some(DesktopApp {
                    id: p.name.clone(),
                    name: p.name,
                    generic: p.source.to_string(),
                    exec: t.display().to_string(),
                    file: t,
                    terminal: false,
                })
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        apps_in(&application_dirs())
    }
}

/// [`desktop_apps`] over explicit folders (tests).
pub fn apps_in(dirs: &[PathBuf]) -> Vec<DesktopApp> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for d in dirs {
        let mut stack = vec![(d.clone(), String::new())];
        while let Some((dir, prefix)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if p.is_dir() {
                    stack.push((p, format!("{prefix}{name}-")));
                } else if let Some(stem) = name.strip_suffix(".desktop") {
                    let id = format!("{prefix}{stem}");
                    if seen.insert(id.clone()) {
                        if let Some(a) = parse_desktop(&p, &id) {
                            out.push(a);
                        }
                    }
                }
            }
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// The best app for `query`: exact id / name, then the last id segment
/// (`dolphin` → `org.kde.dolphin`), then generic name, then a prefix match.
pub fn find_app<'a>(apps: &'a [DesktopApp], query: &str) -> Option<&'a DesktopApp> {
    let q = query.trim().trim_end_matches(".desktop").to_lowercase();
    if q.is_empty() {
        return None;
    }
    let exec0 = |a: &DesktopApp| {
        split_exec(&a.exec)
            .first()
            .map(|s| {
                Path::new(s)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_lowercase())
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    };
    let tests: [&dyn Fn(&DesktopApp) -> bool; 5] = [
        &|a| a.id.to_lowercase() == q || a.name.to_lowercase() == q,
        &|a| a.id.to_lowercase().rsplit('.').next() == Some(q.as_str()),
        &|a| exec0(a) == q,
        &|a| a.generic.to_lowercase() == q,
        &|a| a.name.to_lowercase().starts_with(&q),
    ];
    tests.iter().find_map(|t| apps.iter().find(|a| t(a)))
}

/// `name` on `PATH`, if it is an executable file. On Windows the `PATHEXT`
/// extensions (`.exe`, `.cmd`, …) are tried too.
pub fn which(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || (cfg!(windows) && name.contains('\\')) {
        let p = PathBuf::from(name);
        return is_exec(&p).then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        let pe = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        std::iter::once(String::new())
            .chain(pe.split(';').filter(|e| !e.is_empty()).map(str::to_string))
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&path)
        .flat_map(|d| exts.iter().map(move |e| d.join(format!("{name}{e}"))))
        .find(|p| is_exec(p))
}

#[cfg(unix)]
fn is_exec(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}
#[cfg(not(unix))]
fn is_exec(p: &Path) -> bool {
    p.is_file()
}

/// A one-paragraph answer to "is `name` installed?".
pub fn installed_report(name: &str) -> String {
    let apps = desktop_apps();
    let app = find_app(&apps, name);
    let bin = which(name.trim());
    match (app, bin) {
        (None, None) => format!("{name}: not installed (no app entry, not on PATH)"),
        (a, b) => {
            let mut s = format!("{name}: installed");
            if let Some(a) = a {
                s.push_str(&format!("\n  app: {} ({}) — Exec={}", a.name, a.id, a.exec));
            }
            if let Some(b) = b {
                s.push_str(&format!("\n  command: {}", b.display()));
            }
            s
        }
    }
}

/// The installed apps as `Name (id)` lines (optionally filtered).
pub fn apps_report(filter: &str) -> String {
    let f = filter.trim().to_lowercase();
    let rows: Vec<String> = desktop_apps()
        .into_iter()
        .filter(|a| {
            f.is_empty()
                || a.name.to_lowercase().contains(&f)
                || a.id.to_lowercase().contains(&f)
                || a.generic.to_lowercase().contains(&f)
        })
        .map(|a| {
            if a.generic.is_empty() {
                format!("{} ({})", a.name, a.id)
            } else {
                format!("{} — {} ({})", a.name, a.generic, a.id)
            }
        })
        .collect();
    if rows.is_empty() {
        "no matching apps".into()
    } else {
        format!("{} apps:\n{}", rows.len(), rows.join("\n"))
    }
}

/// Splits an `Exec=` value into words, honouring double quotes and `\`.
pub fn split_exec(exec: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut have = false;
    let mut it = exec.chars();
    while let Some(c) = it.next() {
        match c {
            '"' => {
                in_q = !in_q;
                have = true;
            }
            '\\' => {
                if let Some(n) = it.next() {
                    cur.push(n);
                    have = true;
                }
            }
            c if c.is_whitespace() && !in_q => {
                if have {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            c => {
                cur.push(c);
                have = true;
            }
        }
    }
    if have {
        out.push(cur);
    }
    out
}

/// The argv that opens `app` with `files`: `%f`/`%u` take the first file
/// (the rest are appended), `%F`/`%U` take them all, other field codes are
/// dropped. Files are appended when the Exec line has no file code.
pub fn launch_argv(app: &DesktopApp, files: &[PathBuf]) -> Vec<String> {
    let fs: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
    if cfg!(windows) {
        // A Windows entry's `exec` is the .exe / .lnk path itself (backslashes
        // are separators there, not escapes).
        let mut v = vec![app.exec.clone()];
        v.extend(fs);
        return v;
    }
    let mut out = Vec::new();
    let mut used = false;
    for w in split_exec(&app.exec) {
        match w.as_str() {
            "%f" | "%u" => {
                if let Some(f) = fs.first() {
                    out.push(f.clone());
                }
                used = true;
            }
            "%F" | "%U" => {
                out.extend(fs.iter().cloned());
                used = true;
            }
            "%i" | "%c" | "%k" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => {}
            _ => out.push(w.replace("%%", "%")),
        }
    }
    if !used {
        out.extend(fs);
    } else if fs.len() > 1 && split_exec(&app.exec).iter().any(|w| w == "%f" || w == "%u") {
        out.extend(fs[1..].iter().cloned());
    }
    if app.terminal {
        let term = ["konsole", "kitty", "alacritty", "foot", "xterm"]
            .into_iter()
            .find(|t| which(t).is_some());
        if let Some(t) = term {
            let mut v = vec![t.to_string(), "-e".to_string()];
            v.extend(out);
            return v;
        }
    }
    out
}

/// Starts `argv` detached (own process group, no stdin/stdout), returning its
/// pid. Lillith does not wait for it.
pub fn spawn_detached(argv: &[String]) -> std::io::Result<u32> {
    let (cmd, args) = argv
        .split_first()
        .ok_or_else(|| std::io::Error::other("nothing to launch"))?;
    #[cfg(windows)]
    {
        // Shortcuts, documents and anything not an .exe open through the
        // shell (ShellExecuteW "open"), exactly like a double-click.
        let lower = cmd.to_ascii_lowercase();
        if !lower.ends_with(".exe")
            && !lower.ends_with(".com")
            && !lower.ends_with(".bat")
            && !lower.ends_with(".cmd")
            && (lower.contains('\\') || lower.contains('.'))
        {
            return shell_open(cmd, &super::winshell::join_args(args)).map(|()| 0);
        }
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        let child = std::process::Command::new(cmd)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .spawn()?;
        Ok(child.id())
    }
    #[cfg(not(windows))]
    {
        spawn_unix(cmd, args)
    }
}

/// Opens `file` with `params` through the Windows shell (`open` verb).
#[cfg(windows)]
pub fn shell_open(file: &str, params: &str) -> std::io::Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let w = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let (verb, f, p) = (w("open"), w(file), w(params));
    // SAFETY: every pointer is a NUL-terminated UTF-16 buffer that outlives
    // the call; a null HWND and directory are allowed.
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            f.as_ptr(),
            if params.is_empty() {
                std::ptr::null()
            } else {
                p.as_ptr()
            },
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if (r as isize) <= 32 {
        return Err(std::io::Error::other(format!(
            "Windows could not open {file} (ShellExecute code {})",
            r as isize
        )));
    }
    Ok(())
}

#[cfg(not(windows))]
fn spawn_unix(cmd: &str, args: &[String]) -> std::io::Result<u32> {
    let mut c = std::process::Command::new(cmd);
    c.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    let child = c.spawn()?;
    Ok(child.id())
}

#[cfg(test)]
mod tests {
    use super::super::testutil::tmp;
    use super::*;

    fn write_app(dir: &Path, id: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(format!("{id}.desktop")), body).unwrap();
    }

    #[test]
    fn finds_apps_and_builds_argv_with_files() {
        let d = tmp("prog-apps");
        write_app(
            &d,
            "org.kde.kate",
            "[Desktop Entry]\nType=Application\nName=Kate\nGenericName=Text Editor\nExec=kate -b %U\n[Desktop Action new]\nExec=kate -n\n",
        );
        write_app(
            &d,
            "hidden",
            "[Desktop Entry]\nType=Application\nName=Hid\nExec=hid\nNoDisplay=true\n",
        );
        write_app(&d, "gimp", "[Desktop Entry]\nType=Application\nName=GNU Image Manipulation Program\nExec=gimp-2.10 %f\n");
        let apps = apps_in(&[d.clone()]);
        assert_eq!(apps.len(), 2);
        let kate = find_app(&apps, "kate").unwrap();
        assert_eq!(kate.id, "org.kde.kate");
        assert_eq!(find_app(&apps, "text editor").unwrap().id, "org.kde.kate");
        assert!(find_app(&apps, "hid").is_none());
        let f = vec![PathBuf::from("/home/ko/a b.txt"), PathBuf::from("/x/c.md")];
        assert_eq!(
            launch_argv(kate, &f),
            vec!["kate", "-b", "/home/ko/a b.txt", "/x/c.md"]
        );
        let gimp = find_app(&apps, "gimp-2.10").unwrap();
        assert_eq!(
            launch_argv(gimp, &f[..1]),
            vec!["gimp-2.10", "/home/ko/a b.txt"]
        );
        assert_eq!(launch_argv(gimp, &[]), vec!["gimp-2.10"]);
    }

    #[test]
    fn exec_splitting_honours_quotes() {
        assert_eq!(
            split_exec(r#"sh -c "echo hi" %u"#),
            vec!["sh", "-c", "echo hi", "%u"]
        );
    }

    #[test]
    fn which_finds_sh() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-real-program-xyz").is_none());
    }
}
