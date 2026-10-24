//! Installed programs on Windows: Start Menu shortcuts (`.lnk` under
//! ProgramData and AppData) plus the `App Paths` and `Uninstall` registry
//! keys (HKLM, HKLM WOW6432Node, HKCU), read through `reg.exe query /s`.
//! The parsers are pure and tested on every platform; gathering them is
//! Windows-only.
//!
//! [`super::programs::desktop_apps`] uses this on Windows, so `programs`,
//! `installed` and `open` work there the same way as on Linux.

use std::path::{Path, PathBuf};

/// One installed program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinProgram {
    /// Display name (`7-Zip 23.01 (x64)`, `Notepad++`).
    pub name: String,
    /// What starts it: an `.exe` or a `.lnk`, when known.
    pub target: Option<PathBuf>,
    /// `start-menu`, `app-paths` or `uninstall`.
    pub source: &'static str,
    /// Registry key or shortcut file it came from.
    pub origin: String,
}

/// One key from `reg.exe query` output: its full path and its values
/// (`name`, `type`, `data`). The default value is named `(Default)` (or the
/// localised equivalent in parentheses).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RegKey {
    pub path: String,
    pub values: Vec<(String, String, String)>,
}

impl RegKey {
    /// A value by name (case-insensitive).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(n, _, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, _, d)| d.as_str())
    }

    /// The unnamed default value.
    pub fn default_value(&self) -> Option<&str> {
        self.values
            .iter()
            .find(|(n, _, _)| n.starts_with('(') && n.ends_with(')'))
            .map(|(_, _, d)| d.as_str())
    }

    /// The last path segment (`chrome.exe`, `{GUID}`, `7-Zip`).
    pub fn leaf(&self) -> &str {
        self.path.rsplit('\\').next().unwrap_or(&self.path)
    }
}

/// Parses `reg.exe query <key> /s` output:
///
/// ```text
/// HKEY_LOCAL_MACHINE\SOFTWARE\...\Uninstall\7-Zip
///     DisplayName    REG_SZ    7-Zip 23.01 (x64)
///     NoModify    REG_DWORD    0x1
/// ```
pub fn parse_reg_query(text: &str) -> Vec<RegKey> {
    let mut out: Vec<RegKey> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with("End of search") {
            continue;
        }
        if line.starts_with("HKEY_") {
            out.push(RegKey {
                path: line.trim().to_string(),
                values: Vec::new(),
            });
            continue;
        }
        let Some(key) = out.last_mut() else { continue };
        let body = line.trim_start();
        // name<4 spaces>REG_TYPE[<4 spaces>data]
        let Some(tpos) = body.find("    REG_") else {
            continue;
        };
        let name = body[..tpos].trim().to_string();
        let rest = &body[tpos + 4..];
        let (ty, data) = match rest.find("    ") {
            Some(p) => (rest[..p].trim(), rest[p + 4..].to_string()),
            None => (rest.trim(), String::new()),
        };
        key.values.push((name, ty.to_string(), data));
    }
    out
}

/// An `.exe` path out of a registry value: quotes, `,0` icon indexes and
/// trailing arguments removed. `None` unless it names an `.exe`.
pub fn exe_from_value(v: &str) -> Option<PathBuf> {
    let v = v.trim();
    let path = if let Some(rest) = v.strip_prefix('"') {
        rest.split('"').next().unwrap_or("")
    } else {
        let lower = v.to_ascii_lowercase();
        match lower.find(".exe") {
            Some(p) => &v[..p + 4],
            None => v.split(',').next().unwrap_or(""),
        }
    };
    let path = path.trim();
    path.to_ascii_lowercase()
        .ends_with(".exe")
        .then(|| PathBuf::from(path))
}

/// The last segment of a Windows path (either separator), on any platform.
pub fn win_file_name(p: &str) -> &str {
    p.rsplit(['\\', '/']).next().unwrap_or(p)
}

/// Programs from `Uninstall` keys: those with a `DisplayName` that are not
/// system components or updates.
pub fn from_uninstall(keys: &[RegKey]) -> Vec<WinProgram> {
    keys.iter()
        .filter_map(|k| {
            let name = k.get("DisplayName")?.trim();
            if name.is_empty()
                || k.get("SystemComponent")
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case("0x1"))
                || k.get("ParentKeyName").is_some()
                || k.get("ReleaseType")
                    .is_some_and(|v| v.contains("Update") || v.contains("Hotfix"))
            {
                return None;
            }
            let target = k.get("DisplayIcon").and_then(exe_from_value).filter(|p| {
                let f = win_file_name(&p.to_string_lossy()).to_ascii_lowercase();
                !(f.starts_with("unins") || f.contains("uninstall"))
            });
            Some(WinProgram {
                name: name.to_string(),
                target,
                source: "uninstall",
                origin: k.path.clone(),
            })
        })
        .collect()
}

/// Programs from `App Paths` keys (`chrome.exe` → its full path).
pub fn from_app_paths(keys: &[RegKey]) -> Vec<WinProgram> {
    keys.iter()
        .filter(|k| k.leaf().to_ascii_lowercase().ends_with(".exe"))
        .filter_map(|k| {
            let target = k.default_value().and_then(exe_from_value)?;
            let leaf = k.leaf();
            Some(WinProgram {
                name: leaf[..leaf.len() - 4].to_string(),
                target: Some(target),
                source: "app-paths",
                origin: k.path.clone(),
            })
        })
        .collect()
}

/// Start Menu shortcuts under `dirs` (recursively); uninstallers, help and
/// web links are skipped.
pub fn scan_start_menu(dirs: &[PathBuf]) -> Vec<WinProgram> {
    let mut out = Vec::new();
    for d in dirs {
        let mut stack = vec![d.clone()];
        let mut seen = 0usize;
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                seen += 1;
                if seen > 20_000 {
                    break;
                }
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let is_lnk = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk"));
                if !is_lnk {
                    continue;
                }
                let name = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let low = name.to_ascii_lowercase();
                if name.is_empty()
                    || low.starts_with("uninstall")
                    || low.contains("uninstall ")
                    || low.ends_with(" help")
                    || low.ends_with(" website")
                    || low.ends_with(" readme")
                {
                    continue;
                }
                out.push(WinProgram {
                    name,
                    target: Some(p.clone()),
                    source: "start-menu",
                    origin: p.display().to_string(),
                });
            }
        }
    }
    out
}

/// Merges sources (first wins per lower-cased name; Start Menu first since a
/// shortcut is what Ko clicks), sorted by name.
pub fn merge(groups: Vec<Vec<WinProgram>>) -> Vec<WinProgram> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<WinProgram> = Vec::new();
    for g in groups {
        for p in g {
            let k = p.name.to_lowercase();
            if seen.insert(k) {
                out.push(p);
            } else if let Some(prev) = out
                .iter_mut()
                .find(|x| x.name.eq_ignore_ascii_case(&p.name) && x.target.is_none())
            {
                prev.target = p.target;
            }
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

/// The Start Menu `Programs` folders (all users, then this user) from the
/// environment (`ProgramData`, `APPDATA`).
pub fn start_menu_dirs(env: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for (var, sub) in [
        ("ProgramData", r"Microsoft\Windows\Start Menu\Programs"),
        ("APPDATA", r"Microsoft\Windows\Start Menu\Programs"),
    ] {
        if let Some(base) = env(var).filter(|s| !s.is_empty()) {
            v.push(Path::new(&base).join(sub));
        }
    }
    v
}

/// The registry keys `reg.exe` is asked about: `(key, is_app_paths)`.
pub const REG_KEYS: &[(&str, bool)] = &[
    (
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths",
        true,
    ),
    (
        r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths",
        true,
    ),
    (
        r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        false,
    ),
    (
        r"HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        false,
    ),
    (
        r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        false,
    ),
];

/// Every installed program on this Windows machine.
#[cfg(windows)]
pub fn installed() -> Vec<WinProgram> {
    use std::os::windows::process::CommandExt;
    let start = scan_start_menu(&start_menu_dirs(|k| std::env::var(k).ok()));
    let (mut app_paths, mut uninstall) = (Vec::new(), Vec::new());
    for (key, is_app) in REG_KEYS {
        let mut c = std::process::Command::new("reg.exe");
        c.args(["query", key, "/s"]);
        crate::launch_audit::record_command("win_programs", &c);
        let out = c
            .creation_flags(super::winshell::CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let Ok(out) = out else { continue };
        let keys = parse_reg_query(&String::from_utf8_lossy(&out.stdout));
        if *is_app {
            app_paths.extend(from_app_paths(&keys));
        } else {
            uninstall.extend(from_uninstall(&keys));
        }
    }
    merge(vec![start, app_paths, uninstall])
}

/// Not Windows: nothing.
#[cfg(not(windows))]
pub fn installed() -> Vec<WinProgram> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::super::testutil::tmp;
    use super::*;

    const UNINSTALL: &str = "\r
HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\7-Zip\r
    DisplayName    REG_SZ    7-Zip 23.01 (x64)\r
    DisplayVersion    REG_SZ    23.01\r
    DisplayIcon    REG_SZ    C:\\Program Files\\7-Zip\\7zFM.exe\r
    UninstallString    REG_SZ    \"C:\\Program Files\\7-Zip\\Uninstall.exe\"\r
    NoModify    REG_DWORD    0x1\r
\r
HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{GUID-1}\r
    DisplayName    REG_SZ    Microsoft Visual C++ Runtime\r
    SystemComponent    REG_DWORD    0x1\r
\r
HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\KB123\r
    DisplayName    REG_SZ    Security Update KB123\r
    ParentKeyName    REG_SZ    OperatingSystem\r
\r
HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Notepad++\r
    DisplayName    REG_SZ    Notepad++ (64-bit x64)\r
    DisplayIcon    REG_SZ    \"C:\\Program Files\\Notepad++\\notepad++.exe\",0\r
    EmptyVal    REG_SZ\r
\r
HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Tool\r
    DisplayName    REG_SZ    Some Tool\r
    DisplayIcon    REG_SZ    C:\\Tool\\uninst.exe,0\r
\r
End of search: 14 match(es) found.\r
";

    const APP_PATHS: &str = "
HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\chrome.exe
    (Default)    REG_SZ    C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe
    Path    REG_SZ    C:\\Program Files\\Google\\Chrome\\Application

HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\wordpad.exe
    (Default)    REG_EXPAND_SZ    \"%ProgramFiles%\\Windows NT\\Accessories\\WORDPAD.EXE\"

HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\Empty.exe
";

    #[test]
    fn reg_query_output_parses() {
        let keys = parse_reg_query(UNINSTALL);
        assert_eq!(keys.len(), 5);
        assert_eq!(keys[0].leaf(), "7-Zip");
        assert_eq!(keys[0].get("displayname"), Some("7-Zip 23.01 (x64)"));
        assert_eq!(keys[0].get("NoModify"), Some("0x1"));
        assert_eq!(
            keys[3].values.last().unwrap(),
            &("EmptyVal".into(), "REG_SZ".into(), String::new())
        );
    }

    #[test]
    fn uninstall_keys_become_programs() {
        let p = from_uninstall(&parse_reg_query(UNINSTALL));
        let names: Vec<&str> = p.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(
            names,
            ["7-Zip 23.01 (x64)", "Notepad++ (64-bit x64)", "Some Tool"]
        );
        assert_eq!(
            p[0].target.as_deref(),
            Some(Path::new(r"C:\Program Files\7-Zip\7zFM.exe"))
        );
        assert_eq!(
            p[1].target.as_deref(),
            Some(Path::new(r"C:\Program Files\Notepad++\notepad++.exe"))
        );
        assert_eq!(p[2].target, None, "uninstaller icon is not a launcher");
    }

    #[test]
    fn app_paths_become_programs() {
        let p = from_app_paths(&parse_reg_query(APP_PATHS));
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].name, "chrome");
        assert_eq!(
            p[1].target.as_deref(),
            Some(Path::new(
                r"%ProgramFiles%\Windows NT\Accessories\WORDPAD.EXE"
            ))
        );
    }

    #[test]
    fn exe_values_are_cleaned() {
        assert_eq!(
            exe_from_value(r#""C:\A B\x.exe" --flag"#),
            Some(PathBuf::from(r"C:\A B\x.exe"))
        );
        assert_eq!(
            exe_from_value(r"C:\A\x.EXE,1"),
            Some(PathBuf::from(r"C:\A\x.EXE"))
        );
        assert_eq!(exe_from_value(r"C:\A\icon.ico"), None);
    }

    #[test]
    fn start_menu_shortcuts_are_found_and_merged() {
        let d = tmp("win-start");
        std::fs::create_dir_all(d.join("7-Zip")).unwrap();
        for f in [
            "Notepad++.lnk",
            "7-Zip/7-Zip File Manager.lnk",
            "7-Zip/Uninstall 7-Zip.lnk",
            "7-Zip/7-Zip Help.lnk",
            "desktop.ini",
        ] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let sm = scan_start_menu(&[d.clone()]);
        let mut names: Vec<&str> = sm.iter().map(|x| x.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["7-Zip File Manager", "Notepad++"]);
        let all = merge(vec![
            sm.clone(),
            from_app_paths(&parse_reg_query(APP_PATHS)),
            from_uninstall(&parse_reg_query(UNINSTALL)),
        ]);
        let names: Vec<&str> = all.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "7-Zip 23.01 (x64)",
                "7-Zip File Manager",
                "chrome",
                "Notepad++",
                "Notepad++ (64-bit x64)",
                "Some Tool",
                "wordpad"
            ]
        );
        assert_eq!(
            all.iter().find(|p| p.name == "Notepad++").unwrap().source,
            "start-menu"
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn start_menu_dirs_come_from_env() {
        let v = start_menu_dirs(|k| match k {
            "ProgramData" => Some(r"C:\ProgramData".into()),
            "APPDATA" => Some(r"C:\Users\Ko\AppData\Roaming".into()),
            _ => None,
        });
        assert_eq!(v.len(), 2);
        assert!(v[1].to_string_lossy().contains("Roaming"));
    }
}
