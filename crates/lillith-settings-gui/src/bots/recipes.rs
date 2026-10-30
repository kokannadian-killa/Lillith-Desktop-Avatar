//! Built-in recipes: how each bot plans a job without the model (and the
//! only planner for Mammon, Andras and Naamah). Planning never changes
//! anything: it reads (audited by the caller) and returns [`Plan`] steps
//! that still each need a Soul Contract Yes.

use super::{BotDef, BotKind, Plan, Step};
use lillith_core::access::organize::{suggest_sort, SortBy};
use lillith_core::build::Language;
use std::path::{Path, PathBuf};

/// Plans `job` for `bot` with its built-in recipe.
pub fn plan(bot: &BotDef, job: &str) -> Result<Plan, String> {
    let folders = bot.resolved_folders();
    let base = folders
        .first()
        .cloned()
        .ok_or_else(|| "I have no allowed folder yet: add one in my settings.".to_string())?;
    match bot.kind {
        BotKind::FileOrganizer => organize(&folders, &base, job),
        BotKind::ProgramBuilder => program(&base, job),
        BotKind::WebPageBuilder => Ok(web_page(&base, job)),
        BotKind::OsBuilder => Ok(os_respin(&base, job)),
        BotKind::RedTeam => red_team(&folders, &base, job),
        BotKind::PoseBuilder => poses(&folders, &base, job),
    }
}

/// Lower-case words of the job.
fn words(job: &str) -> Vec<String> {
    job.split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

/// A file-name-safe slug of `s` (max 40 chars), or `fallback`.
pub fn slug(s: &str, fallback: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        fallback.into()
    } else {
        out
    }
}

/// Paths mentioned in the job (`~/x`, `/x` or relative words with a `/`),
/// resolved and confined to `folders`.
fn mentioned_paths(folders: &[PathBuf], base: &Path, job: &str) -> Vec<PathBuf> {
    job.split_whitespace()
        .map(|w| w.trim_matches(|c: char| "\"'`,;()".contains(c)))
        .filter(|w| w.contains('/') || w.contains('\\'))
        .filter_map(|w| {
            let p = super::expand_home(w).unwrap_or_else(|| base.join(w));
            super::confine(folders, &p).ok()
        })
        .collect()
}

/// The name after "called" / "named", else the words after the verb.
fn name_in(job: &str, fallback: &str) -> String {
    let w: Vec<&str> = job.split_whitespace().collect();
    if let Some(i) = w
        .iter()
        .position(|x| matches!(x.to_lowercase().as_str(), "called" | "named"))
    {
        if let Some(n) = w.get(i + 1) {
            return slug(n, fallback);
        }
    }
    slug(job, fallback)
}

// ------------------------------------------------------------- Mammon

fn organize(folders: &[PathBuf], base: &Path, job: &str) -> Result<Plan, String> {
    let w = words(job);
    let by = if w
        .iter()
        .any(|x| x.starts_with("month") || x.starts_with("date"))
    {
        SortBy::Date
    } else {
        SortBy::Type
    };
    let dir = mentioned_paths(folders, base, job)
        .into_iter()
        .find(|p| p.is_dir())
        .unwrap_or_else(|| base.to_path_buf());
    if !dir.is_dir() {
        return Err(format!("{} doesn't exist yet.", dir.display()));
    }
    let plan = suggest_sort(&dir, by).map_err(|e| format!("{}: {e}", dir.display()))?;
    if plan.is_empty() {
        return Ok(Plan {
            say: format!("{} is already tidy; nothing to move.", dir.display()),
            ..Default::default()
        });
    }
    let n = plan.len();
    Ok(Plan {
        say: format!(
            "I'll sort {} by {} ({n} moves). Nothing is overwritten and `lillith undo-organize` puts it all back.",
            dir.display(),
            if by == SortBy::Date { "month" } else { "type" }
        ),
        steps: vec![Step::Organize { plan }],
        notes: Vec::new(),
    })
}

// ------------------------------------------------------------- Belphegor

fn program(base: &Path, job: &str) -> Result<Plan, String> {
    let w = words(job);
    let language = w
        .iter()
        .find_map(|x| Language::parse(x))
        .unwrap_or(Language::Rust);
    let name = name_in(job, "new-project");
    let dir = lillith_core::build::resolve_project_dir(base, &name).map_err(|e| e.to_string())?;
    Ok(Plan {
        say: format!(
            "A {} project `{name}` in {}: scaffold it, then build it (offline).",
            language.label(),
            base.display()
        ),
        steps: vec![
            Step::Scaffold {
                dir: dir.clone(),
                language,
                template: language.default_template().into(),
            },
            Step::Build { dir, language },
        ],
        notes: Vec::new(),
    })
}

// ------------------------------------------------------------- Paimon

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn web_page(base: &Path, job: &str) -> Plan {
    let title: String = job.trim().chars().take(70).collect();
    let dir = base.join(name_in(job, "site"));
    let html = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\">\n  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n  <title>{t}</title>\n  <link rel=\"stylesheet\" href=\"style.css\">\n</head>\n<body>\n  <header><h1>{t}</h1></header>\n  <main>\n    <section>\n      <h2>About</h2>\n      <p>Built locally by Paimon. Edit this text.</p>\n    </section>\n  </main>\n  <footer><p>Made on this machine. Nothing is published.</p></footer>\n</body>\n</html>\n",
        t = html_escape(&title)
    );
    let css = "body { margin: 0; font-family: system-ui, sans-serif; background: #0c080b; color: #f3e4e6; }\nheader { padding: 2rem; background: #5c000b; }\nh1 { margin: 0; color: #ff6b78; }\nmain { max-width: 60rem; margin: 2rem auto; padding: 0 1rem; }\nfooter { padding: 1rem 2rem; color: #c9a3a8; font-size: .9rem; }\n";
    Plan {
        say: format!(
            "A static page in {} (index.html + style.css). Open it in your browser when it's written.",
            dir.display()
        ),
        steps: vec![
            Step::WriteFile {
                path: dir.join("index.html"),
                body: html,
            },
            Step::WriteFile {
                path: dir.join("style.css"),
                body: css.into(),
            },
        ],
        notes: Vec::new(),
    }
}

// ------------------------------------------------------------- Baal

/// Base package list of a DemonOS / Phantomcore (Arch) respin.
const BASE_PACKAGES: &[&str] = &[
    "base",
    "linux",
    "linux-firmware",
    "mkinitcpio",
    "mkinitcpio-archiso",
    "syslinux",
    "grub",
    "efibootmgr",
    "networkmanager",
    "plasma-meta",
    "sddm",
    "konsole",
    "dolphin",
    "firefox",
    "pipewire",
    "pipewire-pulse",
    "wireplumber",
    "git",
    "base-devel",
    "fish",
    "vim",
];

fn os_respin(base: &Path, job: &str) -> Plan {
    let dir = base.join(name_in(job, "demonos-respin"));
    let mut pk =
        String::from("# DemonOS / Phantomcore respin package list (archiso packages.x86_64)\n");
    for p in BASE_PACKAGES {
        pk.push_str(p);
        pk.push('\n');
    }
    let build = String::from(
        "#!/bin/sh\n# Builds the ISO with archiso. Baal never runs this by himself:\n# run it yourself, or ask Baal and approve the card.\nset -eu\ncd \"$(dirname \"$0\")\"\n# needs: sudo pacman -S archiso   (install it yourself first)\nsudo mkarchiso -v -w /tmp/demonos-work -o ./out .\n"
    );
    let readme = format!(
        "# {name}\n\nRespin profile written by Baal for: {job}\n\n- `packages.x86_64`: packages on the ISO (edit freely).\n- `airootfs/etc/hostname`: the live system's hostname.\n- `build.sh`: builds the ISO with archiso (needs root; review before running).\n\nCopy the rest of the profile from `/usr/share/archiso/configs/releng/` once archiso is installed.\n",
        name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("respin"),
        job = job.trim()
    );
    Plan {
        say: format!(
            "A respin profile in {}: package list, hostname, README and a build script. I only write files; building the ISO is a separate step you approve.",
            dir.display()
        ),
        steps: vec![
            Step::WriteFile {
                path: dir.join("packages.x86_64"),
                body: pk,
            },
            Step::WriteFile {
                path: dir.join("airootfs/etc/hostname"),
                body: "demonos\n".into(),
            },
            Step::WriteFile {
                path: dir.join("README.md"),
                body: readme,
            },
            Step::WriteFile {
                path: dir.join("build.sh"),
                body: build,
            },
        ],
        notes: Vec::new(),
    }
}

// ------------------------------------------------------------- Andras

/// Whether the job points at something other than this machine (a URL,
/// a public IP or host name). Andras refuses those.
pub fn targets_others(job: &str) -> Option<String> {
    for raw in job.split_whitespace() {
        let w = raw
            .trim_matches(|c: char| "\"'`,;()<>[]".contains(c))
            .to_lowercase();
        let host = if let Some(i) = w.find("://") {
            w[i + 3..]
                .split(['/', ':', '@'])
                .next()
                .unwrap_or("")
                .to_string()
        } else {
            w.split(':').next().unwrap_or("").to_string()
        };
        let local =
            matches!(host.as_str(), "localhost" | "::1" | "127.0.0.1") || host.starts_with("127.");
        if w.contains("://") && !local {
            return Some(raw.to_string());
        }
        // A bare IPv4 that isn't loopback.
        let parts: Vec<&str> = host.split('.').collect();
        if parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok()) && !local {
            return Some(raw.to_string());
        }
        // host.tld style names (not files with a path or an extension we know).
        let tlds = [
            ".com", ".net", ".org", ".io", ".gov", ".edu", ".co", ".ru", ".cn",
        ];
        if !w.contains('/') && tlds.iter().any(|t| host.ends_with(t)) {
            return Some(raw.to_string());
        }
    }
    None
}

/// Names that usually hold secrets (reported by name only, never read).
const SECRET_NAMES: &[&str] = &[
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    ".env",
    "credentials",
    ".netrc",
    ".pgpass",
    ".git-credentials",
];
const SECRET_EXT: &[&str] = &["pem", "key", "p12", "pfx", "kdbx"];
const MAX_WALK: usize = 20_000;

/// Read-only audit of `folders` (bounded walk, symlinks not followed).
pub fn audit(folders: &[PathBuf]) -> Vec<String> {
    let mut findings = Vec::new();
    let mut seen = 0usize;
    for root in folders {
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                seen += 1;
                if seen > MAX_WALK {
                    findings.push(format!("(stopped after {MAX_WALK} entries)"));
                    return findings;
                }
                let p = e.path();
                let Ok(md) = std::fs::symlink_metadata(&p) else {
                    continue;
                };
                let name = e.file_name().to_string_lossy().to_lowercase();
                if md.is_dir() {
                    if depth < 6 && !matches!(name.as_str(), ".git" | "target" | "node_modules") {
                        stack.push((p.clone(), depth + 1));
                    }
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let m = md.permissions().mode();
                        if m & 0o002 != 0 && m & 0o1000 == 0 {
                            findings.push(format!("world-writable folder: {}", p.display()));
                        }
                    }
                    continue;
                }
                if !md.is_file() {
                    continue;
                }
                let ext = p
                    .extension()
                    .and_then(|x| x.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if SECRET_NAMES.contains(&name.as_str()) || SECRET_EXT.contains(&ext.as_str()) {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let m = md.permissions().mode();
                        if m & 0o077 != 0 {
                            findings.push(format!(
                                "secret-looking file readable by others (mode {:o}): {}",
                                m & 0o777,
                                p.display()
                            ));
                        } else {
                            findings
                                .push(format!("secret-looking file (private): {}", p.display()));
                        }
                    }
                    #[cfg(not(unix))]
                    findings.push(format!("secret-looking file: {}", p.display()));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let m = md.permissions().mode();
                    if m & 0o002 != 0 {
                        findings.push(format!("world-writable file: {}", p.display()));
                    }
                    if m & 0o4000 != 0 {
                        findings.push(format!("setuid file: {}", p.display()));
                    }
                }
            }
        }
    }
    findings
}

/// Local listening TCP ports (Linux `/proc/net/tcp*`, read-only).
pub fn listening_ports() -> Vec<String> {
    let mut out = Vec::new();
    for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(t) = std::fs::read_to_string(f) else {
            continue;
        };
        for l in t.lines().skip(1) {
            let c: Vec<&str> = l.split_whitespace().collect();
            if c.len() > 3 && c[3] == "0A" {
                if let Some((addr, port)) = c[1].split_once(':') {
                    let port = u16::from_str_radix(port, 16).unwrap_or(0);
                    let any = addr.chars().all(|ch| ch == '0');
                    let loopback = addr == "0100007F" || addr.ends_with("01000000");
                    let scope = if any {
                        "all interfaces"
                    } else if loopback {
                        "loopback"
                    } else {
                        "one interface"
                    };
                    let line = format!("listening on port {port} ({scope})");
                    if !out.contains(&line) {
                        out.push(line);
                    }
                }
            }
        }
    }
    out
}

fn red_team(folders: &[PathBuf], base: &Path, job: &str) -> Result<Plan, String> {
    if let Some(t) = targets_others(job) {
        return Err(format!(
            "I only audit your own machine and projects, so I won't go near `{t}`. Point me at a folder of yours instead."
        ));
    }
    let mut findings = audit(folders);
    findings.extend(listening_ports());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut report = format!(
        "# Andras audit report\n\nJob: {}\nScanned (read-only): {}\n\n## Findings\n\n",
        job.trim(),
        folders
            .iter()
            .map(|f| f.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    if findings.is_empty() {
        report.push_str("Nothing notable found.\n");
    }
    for f in &findings {
        report.push_str(&format!("- {f}\n"));
    }
    report
        .push_str("\nNo file contents were read; secret-looking files are listed by name only.\n");
    Ok(Plan {
        say: format!(
            "Scan done: {} finding(s). I'd like to save the report as a file.",
            findings.len()
        ),
        steps: vec![Step::WriteFile {
            path: base.join(format!("andras-report-{stamp}.md")),
            body: report,
        }],
        notes: findings,
    })
}

// ------------------------------------------------------------- Naamah

fn poses(folders: &[PathBuf], base: &Path, job: &str) -> Result<Plan, String> {
    use crate::video_import as vi;
    let mut videos: Vec<PathBuf> = mentioned_paths(folders, base, job)
        .into_iter()
        .filter(|p| p.is_file() && vi::accepted(p))
        .collect();
    if videos.is_empty() {
        let mut found: Vec<PathBuf> = std::fs::read_dir(base)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_file() && vi::accepted(p))
                    .collect()
            })
            .unwrap_or_default();
        found.sort();
        let w = words(job);
        // "all" imports every video in the folder; otherwise the newest-named few.
        let cap = if w.iter().any(|x| x == "all") { 20 } else { 3 };
        videos = found.into_iter().take(cap).collect();
    }
    if videos.is_empty() {
        return Err(format!(
            "I found no videos (mp4, gif, webm, mov) in {}. Put them there or name the file.",
            base.display()
        ));
    }
    let python = vi::find_python();
    let script = vi::find_script();
    if let Some(m) = vi::missing_tools(
        python.as_deref(),
        vi::find_ffmpeg().as_deref(),
        vi::find_ffprobe().as_deref(),
        script.as_deref(),
    ) {
        return Err(m);
    }
    let (python, script) = (python.unwrap_or_default(), script.unwrap_or_default());
    let root = vi::staging_root().ok_or("no data folder for the import staging area")?;
    let mut steps = Vec::new();
    for (i, v) in videos.iter().enumerate() {
        let out = root.join(format!("{}-naamah-{i}", vi::new_job_id()));
        let cmd = vi::ImportCmd {
            python: python.clone(),
            script: script.clone(),
            input: v.clone(),
            out: out.clone(),
            max_ram_mb: vi::DEFAULT_MAX_RAM_MB,
            ffmpeg: vi::bundled_tool("ffmpeg"),
            ffprobe: vi::bundled_tool("ffprobe"),
            matte: vi::Matte::Auto,
            max_candidates: 0,
        };
        let q = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "'\\''"));
        steps.push(Step::Run {
            command: format!("mkdir -p {} && {}", q(&out), cmd.display()),
            cwd: None,
            timeout_s: 3600,
        });
    }
    Ok(Plan {
        say: format!(
            "{} video(s) to turn into pose candidates. Each run is its own card; then approve the poses you like in Pose Builder.",
            videos.len()
        ),
        steps,
        notes: videos.iter().map(|v| format!("video: {}", v.display())).collect(),
    })
}
