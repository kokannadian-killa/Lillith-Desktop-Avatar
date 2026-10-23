//! Lillith's scoped local file/tool API (Phase 2 of the companion plan).
//!
//! This is the bridge between a tool request the GGUF mind writes into a
//! reply and a [`PendingAction`] the Soul Contract can run. It owns two
//! things:
//!
//! - [`Allowlist`] — which folders she may touch. By default: the config dir,
//!   the knowledge and skills folders, plus every `[mind] allow_paths` root
//!   (e.g. `~/Projects`). A built-in deny list keeps `/etc` and the usual
//!   secrets dirs (`~/.ssh`, `~/.gnupg`, `~/.aws`, …) out of reach unless a
//!   matching `[mind] unlock_paths` root re-opens them. There is no network
//!   tool at all.
//! - [`MindTools::plan`] — takes a parsed [`ToolRequest`], confines its path
//!   to the allowlist, and returns a [`Planned`]: a read she may run straight
//!   away (audited, no popup) or a mutation that needs a Soul Contract Yes.
//!
//! The model asks for a tool with a fenced ` ```tool ` block; see
//! [`parse_tool_request`] for the exact format.

use lillith_core::config::{expand_tilde, MindConfig};
use lillith_core::soul_contract::PendingAction;
use std::path::{Component, Path, PathBuf};

/// A tool the model asked for, parsed out of a reply. Paths here are raw
/// (as the model wrote them); [`MindTools::plan`] normalises and vets them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRequest {
    /// List a folder's entries.
    List { path: String },
    /// Read a file.
    Read { path: String },
    /// Search a file or folder for a substring.
    Search { path: String, query: String },
    /// Write a file (needs a Yes).
    Write { path: String, body: String },
    /// Run a shell command (needs a Yes, and `allow_shell`).
    Shell { command: String },
}

/// The outcome of planning a [`ToolRequest`] against the allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planned {
    /// A read she may run now (via `SoulContract::run_now`): no popup, audited.
    Read(PendingAction),
    /// A mutation that must wait for a Soul Contract Yes.
    Confirm(PendingAction),
}

/// Why a tool request was refused before it ever reached the gate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolError {
    #[error("{} is outside Lillith's allowed folders", .0.display())]
    OutsideAllowlist(PathBuf),
    #[error("shell is turned off (set [mind] allow_shell = true to enable it)")]
    ShellDisabled,
    #[error("the tool request was empty")]
    Empty,
}

/// The set of folders Lillith's tool API may touch, plus the shell switch and
/// read cap.
#[derive(Debug, Clone)]
pub struct Allowlist {
    allow: Vec<PathBuf>,
    deny: Vec<PathBuf>,
    unlock: Vec<PathBuf>,
    allow_shell: bool,
    max_read_bytes: usize,
}

impl Allowlist {
    /// Builds the allowlist from `[mind]`: allowed roots from
    /// [`MindConfig::resolved_allow_roots`], the built-in deny roots, and any
    /// `unlock_paths` that re-open them.
    pub fn from_mind(cfg: &MindConfig) -> Self {
        Self {
            allow: cfg.resolved_allow_roots(),
            deny: default_deny_roots(),
            unlock: cfg.resolved_unlock_roots(),
            allow_shell: cfg.allow_shell,
            max_read_bytes: cfg.read_cap(),
        }
    }

    /// Builds an allowlist directly (tests).
    pub fn new(
        allow: Vec<PathBuf>,
        deny: Vec<PathBuf>,
        unlock: Vec<PathBuf>,
        allow_shell: bool,
        max_read_bytes: usize,
    ) -> Self {
        Self {
            allow,
            deny,
            unlock,
            allow_shell,
            max_read_bytes: max_read_bytes.max(1),
        }
    }

    pub fn allow_shell(&self) -> bool {
        self.allow_shell
    }

    pub fn max_read_bytes(&self) -> usize {
        self.max_read_bytes
    }

    pub fn allow_roots(&self) -> &[PathBuf] {
        &self.allow
    }

    /// Whether `path` may be touched: inside an allowed root, and not inside a
    /// deny root (unless an unlock root re-opens it). Operates on the
    /// lexically-normalised path, so `..` cannot climb out of an allowed root.
    pub fn is_allowed(&self, path: &Path) -> bool {
        let p = normalize(path);
        let under_allow = self.allow.iter().any(|r| p.starts_with(r));
        if !under_allow {
            return false;
        }
        let under_deny = self.deny.iter().any(|r| p.starts_with(r));
        let under_unlock = self.unlock.iter().any(|r| p.starts_with(r));
        !under_deny || under_unlock
    }
}

/// The scoped tool API ChatSession drives: an [`Allowlist`] plus the audit
/// log path mutations are recorded against.
#[derive(Debug, Clone)]
pub struct MindTools {
    allow: Allowlist,
    audit_log: Option<PathBuf>,
}

impl MindTools {
    /// Builds from `[mind]` (allowlist + audit log path).
    pub fn from_mind(cfg: &MindConfig) -> Self {
        Self {
            allow: Allowlist::from_mind(cfg),
            audit_log: cfg.audit_log_path(),
        }
    }

    /// Builds directly (tests).
    pub fn new(allow: Allowlist, audit_log: Option<PathBuf>) -> Self {
        Self { allow, audit_log }
    }

    pub fn allowlist(&self) -> &Allowlist {
        &self.allow
    }

    pub fn audit_log(&self) -> Option<&Path> {
        self.audit_log.as_deref()
    }

    /// Confines a request to the allowlist and turns it into a [`Planned`]
    /// action. Reads come back as [`Planned::Read`] (run now, no popup);
    /// writes and shell runs as [`Planned::Confirm`] (need a Yes).
    pub fn plan(&self, req: ToolRequest) -> Result<Planned, ToolError> {
        match req {
            ToolRequest::List { path } => {
                let p = self.vet(&path)?;
                Ok(Planned::Read(PendingAction::ListDir { path: p }))
            }
            ToolRequest::Read { path } => {
                let p = self.vet(&path)?;
                Ok(Planned::Read(PendingAction::ReadFile {
                    path: p,
                    max_bytes: self.allow.max_read_bytes,
                }))
            }
            ToolRequest::Search { path, query } => {
                let p = self.vet(&path)?;
                Ok(Planned::Read(PendingAction::SearchFiles {
                    path: p,
                    query,
                    max_bytes: self.allow.max_read_bytes,
                }))
            }
            ToolRequest::Write { path, body } => {
                let p = self.vet(&path)?;
                Ok(Planned::Confirm(PendingAction::WriteFile { path: p, body }))
            }
            ToolRequest::Shell { command } => {
                if !self.allow.allow_shell {
                    return Err(ToolError::ShellDisabled);
                }
                if command.trim().is_empty() {
                    return Err(ToolError::Empty);
                }
                Ok(Planned::Confirm(PendingAction::RunShell { command }))
            }
        }
    }

    /// Normalises a path and checks it against the allowlist.
    fn vet(&self, raw: &str) -> Result<PathBuf, ToolError> {
        let p = normalize(Path::new(raw));
        if self.allow.is_allowed(&p) {
            Ok(p)
        } else {
            Err(ToolError::OutsideAllowlist(p))
        }
    }
}

/// The built-in deny roots: `/etc` and the usual per-user secrets folders.
/// `unlock_paths` can re-open any of these.
fn default_deny_roots() -> Vec<PathBuf> {
    let mut v = vec![PathBuf::from("/etc")];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for sub in [
            ".ssh",
            ".gnupg",
            ".aws",
            ".config/gcloud",
            ".password-store",
            ".docker",
            ".kube",
        ] {
            v.push(home.join(sub));
        }
    }
    v
}

/// Lexically normalises a path: expands a leading `~`, makes it absolute
/// (relative paths are taken against `$HOME`), and resolves `.`/`..` without
/// touching the filesystem (so it works for files that don't exist yet and
/// cannot be fooled by a path that doesn't exist on disk).
pub fn normalize(path: &Path) -> PathBuf {
    // Expand a leading tilde against HOME.
    let expanded: PathBuf = match path.to_str() {
        Some(s) if s == "~" || s.starts_with("~/") => {
            expand_tilde(s).unwrap_or_else(|| path.to_path_buf())
        }
        _ => path.to_path_buf(),
    };
    // Anchor relative paths at HOME so there is always a root to confine.
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

// ---------------------------------------------------------------------------
// Parsing tool requests out of a model reply.
// ---------------------------------------------------------------------------

/// Parses the first tool request out of `reply`.
///
/// She asks for a tool with a fenced block whose info string is `tool`:
///
/// ````text
/// ```tool
/// list ~/Projects
/// ```
/// ```tool
/// read ~/Projects/notes.md
/// ```
/// ```tool
/// search ~/Projects TODO
/// ```
/// ```tool
/// write ~/Projects/hello.txt
/// line one
/// line two
/// ```
/// ```tool
/// shell
/// cargo build --locked
/// ```
/// ````
///
/// The first line inside the block is the command (`list`/`read`/`search`/
/// `write`/`shell` plus its path/args); for `write` and `shell` the remaining
/// lines are the file body / the command. The command may also be written on
/// the fence line itself (`` ```tool read ~/x `` ). A JSON body such as
/// `{"op":"read","path":"~/x"}` is accepted too. Returns `None` if there is no
/// well-formed tool block.
pub fn parse_tool_request(reply: &str) -> Option<ToolRequest> {
    for (info, body) in fenced_blocks(reply) {
        let rest = info.strip_prefix("tool")?;
        // The info string's first token must be exactly "tool".
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            continue;
        }
        if let Some(req) = parse_tool_body(rest.trim(), &body) {
            return Some(req);
        }
    }
    None
}

/// Parses one tool block. `inline` is anything after `tool` on the fence line;
/// `body` is the fenced content.
fn parse_tool_body(inline: &str, body: &str) -> Option<ToolRequest> {
    // JSON form: the body is a single `{...}` object.
    let trimmed = body.trim();
    if inline.is_empty() && trimmed.starts_with('{') {
        if let Some(req) = parse_tool_json(trimmed) {
            return Some(req);
        }
    }
    // Line form: command from the inline text, else the first body line.
    let (command_line, content): (String, String) = if inline.is_empty() {
        let mut lines = body.lines();
        let first = lines.next()?.trim().to_string();
        let rest: Vec<&str> = lines.collect();
        (first, rest.join("\n"))
    } else {
        (inline.to_string(), body.to_string())
    };
    let command_line = command_line.trim();
    if command_line.is_empty() {
        return None;
    }
    let mut parts = command_line.splitn(2, char::is_whitespace);
    let op = parts.next()?.to_ascii_lowercase();
    let arg = parts
        .next()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    match op.as_str() {
        "list" | "ls" | "dir" => (!arg.is_empty()).then_some(ToolRequest::List { path: arg }),
        "read" | "cat" => (!arg.is_empty()).then_some(ToolRequest::Read { path: arg }),
        "search" | "grep" | "find" => {
            let mut a = arg.splitn(2, char::is_whitespace);
            let path = a.next().unwrap_or_default().trim().to_string();
            let query = a.next().unwrap_or_default().trim().to_string();
            (!path.is_empty() && !query.is_empty()).then_some(ToolRequest::Search { path, query })
        }
        "write" => {
            let content = content.trim_end_matches('\n').to_string();
            (!arg.is_empty()).then_some(ToolRequest::Write {
                path: arg,
                body: content,
            })
        }
        "shell" | "sh" | "run" => {
            // Shell command: inline tail if present, else the body.
            let command = if arg.is_empty() {
                content.trim().to_string()
            } else {
                arg
            };
            (!command.is_empty()).then_some(ToolRequest::Shell { command })
        }
        _ => None,
    }
}

/// Parses a `{"op":...}` JSON tool call (no serde struct needed; small and
/// forgiving).
fn parse_tool_json(s: &str) -> Option<ToolRequest> {
    let v: serde_json::Value = serde_json::from_str(s).ok()?;
    let op = v
        .get("op")
        .or_else(|| v.get("tool"))?
        .as_str()?
        .to_ascii_lowercase();
    let path = v
        .get("path")
        .and_then(|p| p.as_str())
        .map(|s| s.to_string());
    match op.as_str() {
        "list" => path.map(|path| ToolRequest::List { path }),
        "read" => path.map(|path| ToolRequest::Read { path }),
        "search" => {
            let query = v.get("query").and_then(|q| q.as_str())?.to_string();
            path.map(|path| ToolRequest::Search { path, query })
        }
        "write" => {
            let body = v
                .get("body")
                .or_else(|| v.get("content"))
                .and_then(|b| b.as_str())
                .unwrap_or("")
                .to_string();
            path.map(|path| ToolRequest::Write { path, body })
        }
        "shell" => {
            let command = v
                .get("command")
                .or_else(|| v.get("cmd"))
                .and_then(|c| c.as_str())?
                .to_string();
            Some(ToolRequest::Shell { command })
        }
        _ => None,
    }
}

/// Yields `(info, body)` for each fenced code block in `text` (``` or ~~~).
fn fenced_blocks(text: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_start();
        let info = line
            .strip_prefix("```")
            .or_else(|| line.strip_prefix("~~~"));
        if let Some(info) = info {
            let mut body = String::new();
            let mut j = i + 1;
            let mut closed = false;
            while j < lines.len() {
                let l = lines[j].trim_start();
                if l.starts_with("```") || l.starts_with("~~~") {
                    closed = true;
                    break;
                }
                body.push_str(lines[j]);
                body.push('\n');
                j += 1;
            }
            if closed {
                blocks.push((info.trim().to_string(), body));
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowlist(allow: &[&str], deny: &[&str], unlock: &[&str], shell: bool) -> Allowlist {
        Allowlist::new(
            allow.iter().map(PathBuf::from).collect(),
            deny.iter().map(PathBuf::from).collect(),
            unlock.iter().map(PathBuf::from).collect(),
            shell,
            64 * 1024,
        )
    }

    #[test]
    fn normalize_resolves_dotdot_and_tilde() {
        assert_eq!(
            normalize(Path::new("/home/ko/Projects/../Projects/a.txt")),
            PathBuf::from("/home/ko/Projects/a.txt")
        );
        assert_eq!(
            normalize(Path::new("/home/ko/./sub/./x")),
            PathBuf::from("/home/ko/sub/x")
        );
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(
                normalize(Path::new("~/Projects/a")),
                PathBuf::from(&home).join("Projects/a")
            );
        }
    }

    #[test]
    fn allowlist_confines_reads_and_blocks_traversal() {
        let a = allowlist(
            &["/home/ko/Projects", "/home/ko/.config/lillith"],
            &["/etc", "/home/ko/.ssh"],
            &[],
            false,
        );
        assert!(a.is_allowed(Path::new("/home/ko/Projects/src/main.rs")));
        assert!(a.is_allowed(Path::new("/home/ko/.config/lillith/config.toml")));
        // Outside every allow root.
        assert!(!a.is_allowed(Path::new("/home/ko/Secrets/pw")));
        assert!(!a.is_allowed(Path::new("/etc/passwd")));
        // `..` that climbs out of an allowed root is rejected after normalising.
        assert!(!a.is_allowed(Path::new("/home/ko/Projects/../../.ssh/id_rsa")));
    }

    #[test]
    fn deny_wins_unless_unlocked() {
        // A secrets dir nested inside an allowed root is still denied…
        let a = allowlist(&["/home/ko"], &["/home/ko/.ssh"], &[], false);
        assert!(a.is_allowed(Path::new("/home/ko/notes.md")));
        assert!(!a.is_allowed(Path::new("/home/ko/.ssh/id_rsa")));
        // …unless an unlock root re-opens it.
        let b = allowlist(
            &["/home/ko"],
            &["/home/ko/.ssh"],
            &["/home/ko/.ssh/pub"],
            false,
        );
        assert!(!b.is_allowed(Path::new("/home/ko/.ssh/id_rsa")));
        assert!(b.is_allowed(Path::new("/home/ko/.ssh/pub/key.pub")));
    }

    #[test]
    fn plan_gates_writes_and_shell_but_not_reads() {
        let tools = MindTools::new(allowlist(&["/home/ko/Projects"], &[], &[], true), None);
        // Reads -> run-now.
        assert!(matches!(
            tools.plan(ToolRequest::Read {
                path: "/home/ko/Projects/a.md".into()
            }),
            Ok(Planned::Read(PendingAction::ReadFile { .. }))
        ));
        assert!(matches!(
            tools.plan(ToolRequest::List {
                path: "/home/ko/Projects".into()
            }),
            Ok(Planned::Read(PendingAction::ListDir { .. }))
        ));
        // Write -> needs confirm, and the stored path is normalised.
        match tools.plan(ToolRequest::Write {
            path: "/home/ko/Projects/sub/../out.txt".into(),
            body: "hi".into(),
        }) {
            Ok(Planned::Confirm(PendingAction::WriteFile { path, body })) => {
                assert_eq!(path, PathBuf::from("/home/ko/Projects/out.txt"));
                assert_eq!(body, "hi");
            }
            other => panic!("expected gated write, got {other:?}"),
        }
        // Shell -> needs confirm when enabled.
        assert!(matches!(
            tools.plan(ToolRequest::Shell {
                command: "ls".into()
            }),
            Ok(Planned::Confirm(PendingAction::RunShell { .. }))
        ));
    }

    #[test]
    fn plan_refuses_outside_allowlist_and_disabled_shell() {
        let tools = MindTools::new(allowlist(&["/home/ko/Projects"], &[], &[], false), None);
        assert!(matches!(
            tools.plan(ToolRequest::Read {
                path: "/etc/passwd".into()
            }),
            Err(ToolError::OutsideAllowlist(_))
        ));
        assert!(matches!(
            tools.plan(ToolRequest::Write {
                path: "/home/ko/other/x".into(),
                body: "x".into()
            }),
            Err(ToolError::OutsideAllowlist(_))
        ));
        // Shell off by default.
        assert!(matches!(
            tools.plan(ToolRequest::Shell {
                command: "rm -rf /".into()
            }),
            Err(ToolError::ShellDisabled)
        ));
    }

    #[test]
    fn parse_line_form_commands() {
        assert_eq!(
            parse_tool_request("sure~\n```tool\nlist ~/Projects\n```"),
            Some(ToolRequest::List {
                path: "~/Projects".into()
            })
        );
        assert_eq!(
            parse_tool_request("```tool\nread ~/Projects/notes.md\n```"),
            Some(ToolRequest::Read {
                path: "~/Projects/notes.md".into()
            })
        );
        assert_eq!(
            parse_tool_request("```tool\nsearch ~/Projects TODO item\n```"),
            Some(ToolRequest::Search {
                path: "~/Projects".into(),
                query: "TODO item".into()
            })
        );
        assert_eq!(
            parse_tool_request("```tool\nwrite ~/Projects/a.txt\nline one\nline two\n```"),
            Some(ToolRequest::Write {
                path: "~/Projects/a.txt".into(),
                body: "line one\nline two".into()
            })
        );
        assert_eq!(
            parse_tool_request("```tool\nshell\ncargo build --locked\n```"),
            Some(ToolRequest::Shell {
                command: "cargo build --locked".into()
            })
        );
    }

    #[test]
    fn parse_inline_and_json_and_rejects_non_tool() {
        // Command written on the fence line.
        assert_eq!(
            parse_tool_request("```tool read ~/x.md\n```"),
            Some(ToolRequest::Read {
                path: "~/x.md".into()
            })
        );
        // JSON body.
        assert_eq!(
            parse_tool_request("```tool\n{\"op\":\"read\",\"path\":\"~/x.md\"}\n```"),
            Some(ToolRequest::Read {
                path: "~/x.md".into()
            })
        );
        assert_eq!(
            parse_tool_request("```tool\n{\"op\":\"shell\",\"command\":\"ls -la\"}\n```"),
            Some(ToolRequest::Shell {
                command: "ls -la".into()
            })
        );
        // A plain code fence is not a tool, and `tooling` must not match.
        assert!(parse_tool_request("```rust\nfn main(){}\n```").is_none());
        assert!(parse_tool_request("```tooling\nread ~/x\n```").is_none());
        // Unknown op / missing arg.
        assert!(parse_tool_request("```tool\nread\n```").is_none());
        assert!(parse_tool_request("```tool\nfrobnicate ~/x\n```").is_none());
        // Unclosed fence.
        assert!(parse_tool_request("```tool\nread ~/x").is_none());
    }
}
