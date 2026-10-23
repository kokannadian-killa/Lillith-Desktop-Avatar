//! Windows command lines: `cmd.exe` and Windows PowerShell, with captured
//! output, an exit code and a timeout (through [`super::commands::run_child`],
//! so the safeword kills them too).
//!
//! The command text Ko approves is exactly what runs:
//! - `cmd`: `cmd.exe /D /S /C "<command>"`. `/S` strips just the outer quotes
//!   we add, so the command is passed through verbatim (raw, no re-quoting).
//! - `powershell`: `powershell.exe -NoLogo -NoProfile -NonInteractive
//!   -EncodedCommand <base64>`. The base64 is the UTF-16LE of a fixed UTF-8
//!   output prefix plus exactly the approved command. `-EncodedCommand` is
//!   how `-Command` text is passed without any command-line quoting, so no
//!   argument can break out of it.
//!
//! Building, quoting and risk notes are pure and tested on every platform;
//! running is Windows-only.

use std::path::Path;
use std::time::Duration;

/// `CREATE_NO_WINDOW`: no console window flashes up for a captured run.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Which Windows shell runs a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WinShell {
    Cmd,
    PowerShell,
}

impl WinShell {
    /// `cmd` / `powershell` (tool names, wire protocol).
    pub fn as_str(self) -> &'static str {
        match self {
            WinShell::Cmd => "cmd",
            WinShell::PowerShell => "powershell",
        }
    }

    /// The program shown on the card.
    pub fn program(self) -> &'static str {
        match self {
            WinShell::Cmd => "cmd.exe",
            WinShell::PowerShell => "powershell.exe",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "cmd" | "cmd.exe" | "run_cmd" => Some(WinShell::Cmd),
            "powershell" | "powershell.exe" | "ps" | "pwsh" | "run_powershell" => {
                Some(WinShell::PowerShell)
            }
            _ => None,
        }
    }
}

/// One argument to the shell program: quoted normally, or passed raw
/// (`cmd /S /C "…"` must not be re-quoted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    Plain(String),
    Raw(String),
}

/// What PowerShell runs before the approved command: UTF-8 output, no
/// progress bars on the captured stream.
pub const PS_PREFIX: &str = "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;$ProgressPreference='SilentlyContinue';";

/// The program and its arguments for `command` in `shell`.
pub fn invocation(shell: WinShell, command: &str) -> (&'static str, Vec<Arg>) {
    match shell {
        WinShell::Cmd => (
            "cmd.exe",
            vec![
                Arg::Plain("/D".into()),
                Arg::Plain("/S".into()),
                Arg::Plain("/C".into()),
                Arg::Raw(format!("\"{command}\"")),
            ],
        ),
        WinShell::PowerShell => (
            "powershell.exe",
            vec![
                Arg::Plain("-NoLogo".into()),
                Arg::Plain("-NoProfile".into()),
                Arg::Plain("-NonInteractive".into()),
                Arg::Plain("-EncodedCommand".into()),
                Arg::Plain(encode_powershell(&format!("{PS_PREFIX}\n{command}"))),
            ],
        ),
    }
}

/// The full command line as Windows will see it (for the card and logs).
pub fn command_line(shell: WinShell, command: &str) -> String {
    let (prog, args) = invocation(shell, command);
    let mut s = prog.to_string();
    for a in args {
        s.push(' ');
        match a {
            Arg::Plain(p) => s.push_str(&quote_arg(&p)),
            Arg::Raw(r) => s.push_str(&r),
        }
    }
    s
}

/// How the card describes the invocation (the PowerShell base64 is not
/// readable, so it says what it encodes).
pub fn describe(shell: WinShell) -> &'static str {
    match shell {
        WinShell::Cmd => "cmd.exe /D /S /C \"<the command above, verbatim>\"",
        WinShell::PowerShell => {
            "powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand <UTF-16LE base64 of the command above, after a UTF-8 output line>"
        }
    }
}

/// `-EncodedCommand` payload: base64 of the UTF-16LE text.
pub fn encode_powershell(script: &str) -> String {
    let bytes: Vec<u8> = script
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    base64(&bytes)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes standard base64 (tests and the helper's sanity checks).
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = B64.iter().position(|&x| x == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Quotes one argument the way `CommandLineToArgvW` / the MSVC runtime
/// splits it back (backslashes before a quote are doubled).
pub fn quote_arg(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '\n', '\u{b}', '"']) {
        return a.to_string();
    }
    let mut s = String::from('"');
    let mut slashes = 0usize;
    for c in a.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                s.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                s.push('"');
                slashes = 0;
            }
            _ => {
                s.extend(std::iter::repeat_n('\\', slashes));
                s.push(c);
                slashes = 0;
            }
        }
    }
    s.extend(std::iter::repeat_n('\\', slashes * 2));
    s.push('"');
    s
}

/// Joins arguments into one Windows command line.
pub fn join_args<S: AsRef<str>>(args: &[S]) -> String {
    args.iter()
        .map(|a| quote_arg(a.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Why a Windows command is risky, for the card. Every command is shown on
/// the card anyway; this just says what stands out. `Err` = it deletes files,
/// which never goes through a command (the delete contract lists them and
/// needs the typed phrase).
pub fn assess(shell: WinShell, command: &str) -> Result<Option<String>, String> {
    let lower = command.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| c.is_whitespace() || matches!(c, '&' | '|' | ';' | '(' | ')' | '{' | '}'))
        .filter(|w| !w.is_empty())
        .collect();
    let first_of_each = segment_heads(&lower);
    let removers: &[&str] = match shell {
        WinShell::Cmd => &["del", "erase", "rd", "rmdir"],
        WinShell::PowerShell => &[
            "remove-item",
            "rm",
            "ri",
            "del",
            "erase",
            "rd",
            "rmdir",
            "clear-recyclebin",
        ],
    };
    for h in &first_of_each {
        let h = h.trim_start_matches('@');
        if removers.contains(&h) {
            return Err(format!(
                "`{h}` deletes files — ask me to delete instead so you can sign the list"
            ));
        }
    }
    if words
        .iter()
        .any(|w| matches!(*w, "format" | "diskpart" | "format-volume" | "clear-disk"))
        || lower.contains("cipher /w")
    {
        return Ok(Some("can format or wipe a disk".into()));
    }
    let mut why = Vec::new();
    if words.iter().any(|w| {
        matches!(
            *w,
            "reg" | "set-itemproperty" | "new-itemproperty" | "remove-itemproperty"
        )
    }) {
        why.push("may edit the registry");
    }
    if words.iter().any(|w| {
        matches!(
            *w,
            "sc" | "sc.exe" | "net" | "net.exe" | "stop-service" | "set-service" | "bcdedit"
        )
    }) {
        why.push("changes services or system settings");
    }
    if words.iter().any(|w| {
        matches!(
            *w,
            "taskkill" | "stop-process" | "shutdown" | "restart-computer"
        )
    }) {
        why.push("stops processes or the computer");
    }
    if lower.contains('>') {
        why.push("redirects output into a file (could overwrite)");
    }
    if words.iter().any(|w| {
        matches!(
            *w,
            "invoke-expression"
                | "iex"
                | "start-process"
                | "invoke-webrequest"
                | "iwr"
                | "curl"
                | "wget"
        )
    }) {
        why.push("starts other programs or fetches from the network");
    }
    Ok((!why.is_empty()).then(|| why.join("; ")))
}

/// The first word of every `&`, `&&`, `||`, `|`, `;` or newline segment.
fn segment_heads(lower: &str) -> Vec<String> {
    lower
        .split(['&', '|', ';', '\n'])
        .filter_map(|seg| seg.split_whitespace().next().map(str::to_string))
        .collect()
}

/// Runs `command` in `shell` in `cwd` (home if `None`) with captured output,
/// a timeout and an output cap. Windows only; elsewhere it is an error.
pub fn run(
    shell: WinShell,
    command: &str,
    cwd: Option<&Path>,
    timeout: Duration,
    cap: usize,
) -> std::io::Result<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let (prog, args) = invocation(shell, command);
        let mut c = std::process::Command::new(prog);
        for a in args {
            match a {
                Arg::Plain(p) => {
                    c.arg(p);
                }
                Arg::Raw(r) => {
                    c.raw_arg(r);
                }
            }
        }
        if let Some(d) = cwd.map(Path::to_path_buf).or_else(super::home) {
            if d.is_dir() {
                c.current_dir(d);
            }
        }
        super::commands::run_child(c, timeout, cap)
    }
    #[cfg(not(windows))]
    {
        let _ = (shell, command, cwd, timeout, cap);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "cmd / PowerShell commands only run on Windows",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_is_passed_verbatim_inside_s_quotes() {
        let (p, a) = invocation(WinShell::Cmd, r#"dir "C:\Program Files" & echo "x y""#);
        assert_eq!(p, "cmd.exe");
        assert_eq!(
            a,
            vec![
                Arg::Plain("/D".into()),
                Arg::Plain("/S".into()),
                Arg::Plain("/C".into()),
                Arg::Raw(r#""dir "C:\Program Files" & echo "x y"""#.into()),
            ]
        );
        assert_eq!(
            command_line(WinShell::Cmd, "ver"),
            r#"cmd.exe /D /S /C "ver""#
        );
    }

    #[test]
    fn powershell_is_encoded_utf16le_and_round_trips() {
        let cmd = "Get-ChildItem 'C:\\Users\\Ko' | Select -First 3; \"ünï\"";
        let (p, a) = invocation(WinShell::PowerShell, cmd);
        assert_eq!(p, "powershell.exe");
        assert_eq!(a[3], Arg::Plain("-EncodedCommand".into()));
        let Arg::Plain(b64) = &a[4] else { panic!() };
        let bytes = base64_decode(b64).unwrap();
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let text = String::from_utf16(&units).unwrap();
        assert_eq!(text, format!("{PS_PREFIX}\n{cmd}"));
        // Nothing in the encoded argument needs quoting.
        assert_eq!(quote_arg(b64), *b64);
    }

    #[test]
    fn base64_matches_known_vectors() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o);
            assert_eq!(base64_decode(o).unwrap(), i.as_bytes());
        }
        // PowerShell docs example: "dir" → ZABpAHIA
        assert_eq!(encode_powershell("dir"), "ZABpAHIA");
    }

    #[test]
    fn args_quote_like_commandlinetoargvw() {
        assert_eq!(quote_arg("plain"), "plain");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg("a b"), "\"a b\"");
        assert_eq!(quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_arg(r"C:\dir with space\"), r#""C:\dir with space\\""#);
        assert_eq!(quote_arg(r"C:\no\space"), r"C:\no\space");
        assert_eq!(
            join_args(&["--pipe", r"\\.\pipe\x", "a b"]),
            r#"--pipe \\.\pipe\x "a b""#
        );
    }

    #[test]
    fn shells_parse_and_risks_are_named() {
        assert_eq!(WinShell::parse("PowerShell"), Some(WinShell::PowerShell));
        assert_eq!(WinShell::parse("cmd.exe"), Some(WinShell::Cmd));
        assert_eq!(WinShell::parse("bash"), None);
        assert_eq!(assess(WinShell::Cmd, "dir C:\\"), Ok(None));
        assert_eq!(assess(WinShell::PowerShell, "Get-Process"), Ok(None));
        assert!(assess(WinShell::Cmd, "del /q C:\\x").is_err());
        assert!(assess(WinShell::Cmd, "dir & rd /s /q C:\\x").is_err());
        assert!(assess(WinShell::PowerShell, "Remove-Item -Recurse C:\\x").is_err());
        assert!(assess(WinShell::Cmd, "echo hi > a.txt").unwrap().is_some());
        assert!(assess(WinShell::Cmd, "reg add HKCU\\X").unwrap().is_some());
        assert!(assess(WinShell::PowerShell, "Stop-Process -Name x")
            .unwrap()
            .is_some());
        assert!(assess(WinShell::Cmd, "format D:").unwrap().is_some());
    }

    #[cfg(not(windows))]
    #[test]
    fn running_is_windows_only() {
        let e = run(WinShell::Cmd, "ver", None, Duration::from_secs(1), 100).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::Unsupported);
    }
}
