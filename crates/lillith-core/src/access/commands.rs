//! CLI commands for finishing tasks: a conservative classifier, a runner with
//! a timeout and captured output, and the command log.
//!
//! [`classify`] sorts a command line into:
//! - [`CommandClass::Safe`]: every part is a known read-only command (ls, cat,
//!   grep, du, df, `pacman -Q`, `git status`, …) with no redirection or
//!   substitution. Runs straight away.
//! - [`CommandClass::NeedsApproval`]: anything else (writes, installs, config
//!   changes, `sudo`/`pkexec`, `kill`, `>` redirection, `$(…)`, unknown
//!   programs, scripts). Shown on an approval card with the exact command.
//! - [`CommandClass::Removal`]: deletes files (`rm`, `rmdir`, `shred`,
//!   `unlink`, `find -delete`, …). Never run as a command; deletion goes
//!   through the delete Soul Contract with a full file listing.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How a command line may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandClass {
    /// Read-only: runs without asking.
    Safe,
    /// Needs an explicit Yes on a card showing the exact command (the reason).
    NeedsApproval(String),
    /// Deletes files: refused as a command (use the delete contract).
    Removal(String),
}

impl CommandClass {
    pub fn is_safe(&self) -> bool {
        matches!(self, CommandClass::Safe)
    }
}

/// Read-only programs (any arguments, subject to the per-program checks below).
const SAFE: &[&str] = &[
    "ls",
    "cat",
    "head",
    "tail",
    "wc",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "fd",
    "find",
    "du",
    "df",
    "file",
    "stat",
    "which",
    "whereis",
    "uname",
    "date",
    "uptime",
    "free",
    "ps",
    "pgrep",
    "lsblk",
    "findmnt",
    "id",
    "whoami",
    "hostname",
    "pwd",
    "echo",
    "printf",
    "sort",
    "uniq",
    "cut",
    "tr",
    "nl",
    "tree",
    "md5sum",
    "sha1sum",
    "sha256sum",
    "basename",
    "dirname",
    "realpath",
    "readlink",
    "lscpu",
    "lspci",
    "lsusb",
    "nproc",
    "sensors",
    "ffprobe",
    "pdfinfo",
    "identify",
    "exiftool",
    "mediainfo",
    "less",
    "column",
    "diff",
    "cmp",
    "jq",
    "true",
    "false",
    "test",
    "locale",
    "xdg-user-dir",
    "fc-list",
    "glxinfo",
    "vainfo",
    "inxi",
    "fastfetch",
    "neofetch",
    "awk",
    "seq",
];

/// Programs that delete files.
const REMOVERS: &[&str] = &[
    "rm",
    "rmdir",
    "unlink",
    "shred",
    "srm",
    "wipe",
    "trash",
    "trash-put",
    "trash-rm",
    "trash-empty",
    "gio",
];

/// Wrappers whose next word is the real program.
const WRAPPERS: &[&str] = &[
    "sudo",
    "doas",
    "pkexec",
    "su",
    "env",
    "nice",
    "nohup",
    "timeout",
    "command",
    "exec",
    "xargs",
    "busybox",
    "stdbuf",
    "ionice",
    "time",
    "run0",
    "flatpak-spawn",
];

/// Splits on `|`, `||`, `&&`, `;`, `&` and newlines (outside quotes). Returns
/// `None` if quoting is unbalanced.
fn segments(cmd: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut sq, mut dq) = (false, false);
    let mut it = cmd.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\'' if !dq => {
                sq = !sq;
                cur.push(c);
            }
            '"' if !sq => {
                dq = !dq;
                cur.push(c);
            }
            '\\' if !sq => {
                cur.push(c);
                if let Some(n) = it.next() {
                    cur.push(n);
                }
            }
            '|' | '&' | ';' | '\n' if !sq && !dq => {
                if matches!(it.peek(), Some('|') | Some('&')) && c != ';' && c != '\n' {
                    it.next();
                }
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    if sq || dq {
        return None;
    }
    out.push(cur);
    Some(
        out.into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

/// Shell words of the first command in `cmd` (quotes removed).
pub fn split_words(cmd: &str) -> Vec<String> {
    match segments(cmd) {
        Some(segs) => segs.first().map(|s| words(s)).unwrap_or_default(),
        None => words(cmd),
    }
}

/// Shell words of one segment (quotes removed).
fn words(seg: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut sq, mut dq, mut have) = (false, false, false);
    let mut it = seg.chars();
    while let Some(c) = it.next() {
        match c {
            '\'' if !dq => {
                sq = !sq;
                have = true;
            }
            '"' if !sq => {
                dq = !dq;
                have = true;
            }
            '\\' if !sq => {
                if let Some(n) = it.next() {
                    cur.push(n);
                    have = true;
                }
            }
            c if c.is_whitespace() && !sq && !dq => {
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

fn prog_name(w: &str) -> String {
    Path::new(w)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| w.to_string())
}

/// Classifies `cmd`. `deny` are folders that make a command need approval when
/// any argument points into them (secrets dirs).
pub fn classify(cmd: &str, deny: &[PathBuf]) -> CommandClass {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return CommandClass::NeedsApproval("empty command".into());
    }
    let Some(segs) = segments(cmd) else {
        return CommandClass::NeedsApproval("unbalanced quotes".into());
    };
    // Removal anywhere wins: it is never a command.
    for seg in &segs {
        let w = words(seg);
        if let Some(r) = removal_in(&w) {
            return CommandClass::Removal(r);
        }
    }
    if cmd.contains("$(") || cmd.contains('`') || cmd.contains("<(") || cmd.contains(">(") {
        return CommandClass::NeedsApproval("uses command substitution".into());
    }
    let mut reasons: Vec<String> = Vec::new();
    for seg in &segs {
        let quiet = seg
            .replace("2>/dev/null", "")
            .replace("&>/dev/null", "")
            .replace(">/dev/null", "")
            .replace("2>&1", "");
        if quiet.contains('>') {
            reasons.push("redirects output into a file (could overwrite)".into());
        }
        let w = words(seg);
        let Some(first) = w.first() else { continue };
        let p = prog_name(first);
        if first.contains('=') && !first.starts_with('-') {
            reasons.push(format!("sets environment variable {first}"));
            continue;
        }
        if ["sudo", "doas", "pkexec", "su", "run0"].contains(&p.as_str()) {
            reasons.push(format!("uses {p} (root rights)"));
            continue;
        }
        if ["kill", "pkill", "killall", "xkill"].contains(&p.as_str()) {
            reasons.push(format!("{p} stops processes"));
            continue;
        }
        if p.starts_with("mkfs")
            || ["dd", "wipefs", "fdisk", "parted", "sfdisk", "mkswap"].contains(&p.as_str())
        {
            reasons.push(format!("{p} can format or overwrite a disk"));
            continue;
        }
        if [
            "systemctl",
            "chmod",
            "chown",
            "chgrp",
            "usermod",
            "passwd",
            "mount",
            "umount",
            "modprobe",
            "sysctl",
            "grub-mkconfig",
            "mkinitcpio",
            "update-grub",
        ]
        .contains(&p.as_str())
        {
            reasons.push(format!("{p} changes system configuration"));
            continue;
        }
        if let Some(r) = unsafe_args(&p, &w[1..]) {
            reasons.push(r);
            continue;
        }
        if !is_safe_program(&p, &w[1..]) {
            reasons.push(format!("{p} may change things"));
            continue;
        }
        for a in &w[1..] {
            if touches_deny(a, deny) {
                reasons.push(format!("reads a protected folder ({a})"));
            }
        }
    }
    if reasons.is_empty() {
        CommandClass::Safe
    } else {
        reasons.dedup();
        CommandClass::NeedsApproval(reasons.join("; "))
    }
}

fn removal_in(w: &[String]) -> Option<String> {
    let mut i = 0;
    while i < w.len() {
        let p = prog_name(&w[i]);
        if REMOVERS.contains(&p.as_str()) {
            if p == "gio" && !w.get(i + 1).is_some_and(|s| s == "trash" || s == "remove") {
                return None;
            }
            return Some(format!(
                "`{p}` deletes files — ask me to delete instead so you can sign the list"
            ));
        }
        if p == "find" && w[i + 1..].iter().any(|a| a == "-delete") {
            return Some("`find -delete` deletes files — ask me to delete instead".into());
        }
        if (p == "git" && w.get(i + 1).is_some_and(|s| s == "clean"))
            || (p == "rsync"
                && w.iter()
                    .any(|a| a.starts_with("--delete") || a == "--remove-source-files"))
        {
            return Some(format!(
                "`{p}` would delete files — ask me to delete instead"
            ));
        }
        if WRAPPERS.contains(&p.as_str()) {
            // Skip the wrapper's own options / VAR=val, look at what it runs.
            i += 1;
            while i < w.len()
                && (w[i].starts_with('-') || w[i].contains('=') || w[i].parse::<f64>().is_ok())
            {
                i += 1;
            }
            continue;
        }
        if ["sh", "bash", "zsh", "fish", "dash"].contains(&p.as_str()) {
            if let Some(pos) = w.iter().position(|a| a == "-c") {
                if let Some(inner) = w.get(pos + 1) {
                    if let CommandClass::Removal(r) = classify(inner, &[]) {
                        return Some(r);
                    }
                }
            }
        }
        return None;
    }
    None
}

fn unsafe_args(p: &str, args: &[String]) -> Option<String> {
    match p {
        "find" => args
            .iter()
            .find(|a| {
                [
                    "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprint0", "-fprintf", "-fls",
                ]
                .contains(&a.as_str())
            })
            .map(|a| format!("find {a} runs or writes")),
        "awk"
            if args
                .iter()
                .any(|a| a.contains("system") || a.contains('>') || a.contains('|')) =>
        {
            Some("awk script may run commands or write".into())
        }
        "sort" if args.iter().any(|a| a == "-o" || a.starts_with("--output")) => {
            Some("sort -o writes a file".into())
        }
        "fd" if args
            .iter()
            .any(|a| a == "-x" || a == "-X" || a.starts_with("--exec")) =>
        {
            Some("fd --exec runs commands".into())
        }
        "less"
            if args
                .iter()
                .any(|a| a.starts_with("-o") || a.starts_with("--log-file")) =>
        {
            Some("less -o writes a file".into())
        }
        _ => None,
    }
}

fn is_safe_program(p: &str, args: &[String]) -> bool {
    if SAFE.contains(&p) {
        return true;
    }
    let first = args.first().map(String::as_str).unwrap_or("");
    match p {
        "pacman" => {
            (first.starts_with("-Q")
                || first == "-Ss"
                || first == "-Si"
                || first == "-Fl"
                || first == "-Fs")
                && !first.contains('y')
                && !first.contains('u')
        }
        "paru" | "yay" => first == "-Ss" || first == "-Si" || first.starts_with("-Q"),
        "git" => [
            "status",
            "log",
            "diff",
            "show",
            "blame",
            "ls-files",
            "rev-parse",
        ]
        .contains(&first),
        "flatpak" => ["list", "info", "search"].contains(&first),
        "systemctl" => false,
        "xdg-mime" => first == "query",
        "kreadconfig5" | "kreadconfig6" => true,
        "journalctl" => !args
            .iter()
            .any(|a| a.starts_with("--vacuum") || a == "--rotate" || a.starts_with("--flush")),
        "ip" => ["a", "addr", "route", "r", "link", "-br"].contains(&first) && args.len() <= 3,
        "command" => first == "-v",
        "type" => true,
        _ => false,
    }
}

fn touches_deny(arg: &str, deny: &[PathBuf]) -> bool {
    if !(arg.starts_with('/') || arg.starts_with('~') || arg.contains("..") || arg.starts_with('.'))
    {
        return false;
    }
    let p = super::normalize(Path::new(arg));
    deny.iter().any(|d| p.starts_with(d))
}

/// Runs `cmd` through `sh -c` in `cwd` (home if `None`), with no stdin, a
/// `timeout` and output capped at `cap` bytes. Returns the combined output and
/// an `[exit N]` line (`[timed out after Ns]` if it was stopped).
pub fn run_captured(
    cmd: &str,
    cwd: Option<&Path>,
    timeout: Duration,
    cap: usize,
) -> std::io::Result<String> {
    let mut c = std::process::Command::new("sh");
    c.arg("-c").arg(cmd);
    if let Some(d) = cwd.map(Path::to_path_buf).or_else(super::home) {
        if d.is_dir() {
            c.current_dir(d);
        }
    }
    run_child(c, timeout, cap)
}

/// The last line of a command's output when the safeword killed it.
pub const STOPPED_LINE: &str = "[stopped by the safeword]";

/// Whether a command result says the safeword stopped it (audited as
/// `RESULT-STOPPED`, not `RESULT-OK`).
pub fn was_stopped(output: &str) -> bool {
    output.trim_end().ends_with(STOPPED_LINE)
}

/// Runs a prepared command with no stdin and captured stdout/stderr, a
/// `timeout` and output capped at `cap` bytes. The whole child tree is
/// killed on timeout or when the safeword fires
/// ([`super::kill_switch::stop_all`]). Returns the combined output plus an
/// `[exit N]` / `[timed out after Ns]` / `[stopped by the safeword]` line.
pub fn run_child(
    c: std::process::Command,
    timeout: Duration,
    cap: usize,
) -> std::io::Result<String> {
    // Tests that pull the safeword hold the write side, so other tests'
    // commands are never caught by it.
    #[cfg(test)]
    let _gate = super::kill_switch::TEST_GATE
        .read()
        .unwrap_or_else(|e| e.into_inner());
    run_child_ungated(c, timeout, cap)
}

fn run_child_ungated(
    mut c: std::process::Command,
    timeout: Duration,
    cap: usize,
) -> std::io::Result<String> {
    let epoch = super::kill_switch::epoch();
    c.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(super::winshell::CREATE_NO_WINDOW);
    }
    let mut child = c.spawn()?;
    // Windows: put the child in a Job with KILL_ON_JOB_CLOSE so safeword /
    // timeout tears down the whole tree (cmd → nested processes) at once.
    #[cfg(windows)]
    let job = match super::winjob::Job::new_kill_on_close() {
        Ok(j) => {
            let _ = j.assign_pid(child.id());
            Some(j)
        }
        Err(e) => {
            tracing::warn!("could not create kill-job for command child: {e}");
            None
        }
    };
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(s) = so.as_mut() {
            let _ = s.read_to_end(&mut b);
        }
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(s) = se.as_mut() {
            let _ = s.read_to_end(&mut b);
        }
        b
    });
    let start = Instant::now();
    let mut stop: Option<&str> = None;
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break Some(st);
        }
        let why = if super::kill_switch::stopped_since(epoch) {
            Some("safeword")
        } else if start.elapsed() >= timeout {
            Some("timeout")
        } else {
            None
        };
        if let Some(w) = why {
            stop = Some(w);
            #[cfg(windows)]
            if let Some(ref j) = job {
                j.terminate();
            }
            kill_tree(&mut child);
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let out = t_out.join().unwrap_or_default();
    let err = t_err.join().unwrap_or_default();
    let mut text = String::from_utf8_lossy(&out).into_owned();
    if !err.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&err));
    }
    if text.len() > cap {
        let mut end = cap;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let more = text.len() - end;
        text.truncate(end);
        text.push_str(&format!("\n…[{more} more bytes]"));
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    match (stop, status) {
        (Some("safeword"), _) => text.push_str(STOPPED_LINE),
        (Some(_), _) => text.push_str(&format!("[timed out after {}s]", timeout.as_secs())),
        (_, Some(st)) => text.push_str(&format!(
            "[exit {}]",
            st.code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into())
        )),
        _ => {}
    }
    Ok(text)
}

/// Kills `child` and everything it started, then reaps it.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // Kill the whole process group we started. `-s KILL -- -<pgid>` is
        // the POSIX form; procps-ng 4.0.x mis-parses `kill -KILL -<pgid>`
        // (exit 1, and on the GitHub runner the group survived, so a
        // grandchild kept the output pipe open until it exited).
        let _ = std::process::Command::new("kill")
            .args(["-s", "KILL", "--", &format!("-{}", child.id())])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .creation_flags(super::winshell::CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Appends `<utc>\t<decision>\t<command>\t<result>` to the command log.
pub fn log_command(log: &Path, decision: &str, cmd: &str, result: &str) {
    let ts = crate::soul_contract::format_utc(std::time::SystemTime::now());
    let flat = |s: &str| s.replace(['\n', '\t'], " ");
    let line = format!("{ts}\t{decision}\t{}\t{}", flat(cmd), flat(result));
    if let Err(e) = super::append_line(log, &line) {
        tracing::warn!("could not write command log {}: {e}", log.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(c: &str) -> CommandClass {
        classify(c, &[super::super::home().unwrap().join(".ssh")])
    }

    #[test]
    fn read_only_commands_are_safe() {
        for c in [
            "ls -la ~/Downloads",
            "du -sh ~/Videos/* | sort -h | tail -5",
            "pacman -Qi firefox",
            "git status",
            "df -h && free -m",
            "grep -ri 'todo' ~/notes",
            "find ~/Pictures -name '*.png'",
            "which gimp",
            "ls ~/nope 2>/dev/null",
        ] {
            assert_eq!(class(c), CommandClass::Safe, "{c}");
        }
    }

    #[test]
    fn risky_commands_need_approval() {
        for c in [
            "sudo pacman -Syu",
            "pkexec nano /etc/fstab",
            "kill 1234",
            "killall firefox",
            "systemctl restart sddm",
            "echo hi > ~/notes.txt",
            "cat a >> b",
            "mkfs.ext4 /dev/sdb1",
            "dd if=/dev/zero of=/dev/sda",
            "chmod -R 777 ~",
            "sed -i s/a/b/ file",
            "cp a b",
            "mv a b",
            "pacman -S gimp",
            "ls $(cat x)",
            "find . -exec chmod 700 {} ;",
            "python3 ~/.local/share/lillith/skills/tidy.py",
            "bash ~/script.sh",
            "cat ~/.ssh/id_ed25519",
            "FOO=1 ls",
        ] {
            assert!(
                matches!(class(c), CommandClass::NeedsApproval(_)),
                "{c}: {:?}",
                class(c)
            );
        }
    }

    #[test]
    fn removals_are_never_commands() {
        for c in [
            "rm -rf ~/old",
            "ls | xargs rm",
            "sudo rm /etc/x",
            "find ~/tmp -name '*.log' -delete",
            "rmdir empty",
            "shred -u secret",
            "ls; rm x",
            "sh -c 'rm -f a'",
            "gio trash file",
            "git clean -fdx",
            "/usr/bin/rm a",
            "env FOO=1 rm a",
        ] {
            assert!(
                matches!(class(c), CommandClass::Removal(_)),
                "{c}: {:?}",
                class(c)
            );
        }
    }

    #[test]
    fn runner_captures_output_and_times_out() {
        let out = run_captured(
            "echo hello; echo oops >&2",
            None,
            Duration::from_secs(5),
            4096,
        )
        .unwrap();
        assert!(
            out.contains("hello") && out.contains("oops") && out.ends_with("[exit 0]"),
            "{out}"
        );
        let out = run_captured("sleep 5", None, Duration::from_millis(200), 4096).unwrap();
        assert!(out.contains("timed out"), "{out}");
        let out = run_captured("yes | head -c 10000", None, Duration::from_secs(5), 100).unwrap();
        assert!(out.contains("more bytes"), "{out}");
    }

    #[test]
    fn safeword_kills_a_running_command() {
        let _gate = super::super::kill_switch::TEST_GATE
            .write()
            .unwrap_or_else(|e| e.into_inner());
        let t = std::thread::spawn(|| {
            let mut c = std::process::Command::new("sh");
            c.arg("-c").arg("sleep 20");
            run_child_ungated(c, Duration::from_secs(30), 4096)
        });
        std::thread::sleep(Duration::from_millis(300));
        let started = Instant::now();
        super::super::kill_switch::stop_all();
        let out = t.join().unwrap().unwrap();
        assert!(out.contains("stopped by the safeword"), "{out}");
        assert!(was_stopped(&out), "{out}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn was_stopped_only_matches_the_final_marker() {
        assert!(was_stopped("partial\n[stopped by the safeword]"));
        assert!(was_stopped("[stopped by the safeword]\n"));
        assert!(!was_stopped("hi\n[exit 0]"));
        assert!(!was_stopped("echo [stopped by the safeword]\n[exit 0]"));
        assert!(!was_stopped(""));
    }
}
