//! The cheap static check of practice code (docs §6), run **before** any
//! card, compile or run. The sandbox is the real boundary; this only keeps
//! obviously networked / escaping code from ever reaching a card.
//!
//! The doc's list — `std::net`, `socket`, `Command`, `unsafe`, `extern`,
//! `include!` — is matched on **identifier tokens** of the whole source
//! (code, strings and comments alike), case-insensitively, after `r#` raw
//! identifiers are unwrapped. That makes the usual dodges fail:
//! `::std::net`, `std :: net`, `use std::{net}`, `use std::net as n`,
//! `use std::*` + `TcpStream`, `extern crate std as s`, `m!(std, net)`
//! (a macro pasting paths), `r#unsafe`, `__import__("so" + "cket")`.
//! Non-ASCII anywhere is refused too (Python folds full-width letters
//! into ASCII identifiers, NFKC). Each language adds its own bypasses
//! (Rust: other escape hatches; Python: process / dynamic-import / FFI
//! modules and builtins; fish: only builtins may run).

use std::fmt;

/// Largest practice source.
pub const MAX_SOURCE_BYTES: usize = 64 * 1024;

/// A practice language and its §6 toolchain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    /// `rustc --edition 2021 -C opt-level=0 main.rs` (single file, no
    /// cargo, no crates), then `./main`.
    Rust,
    /// `python3 -I -S main.py`.
    Python,
    /// `fish -n` (check), then `fish`.
    Fish,
}

impl Lang {
    pub fn word(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Python => "python",
            Lang::Fish => "fish",
        }
    }

    /// The source file name inside `/work`.
    pub fn file(self) -> &'static str {
        match self {
            Lang::Rust => "main.rs",
            Lang::Python => "main.py",
            Lang::Fish => "main.fish",
        }
    }
}

/// Why a source was rejected (the offending token, never the source).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    pub token: String,
    pub why: &'static str,
}

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.why, self.token)
    }
}

/// docs §6, for every language: the doc's own list as identifier tokens
/// (`std::net` → `net`, `include!` → `include`), plus the networking types
/// a glob import would reach without naming `net`.
const DOC_TOKENS: &[&str] = &[
    "net",
    "socket",
    "command",
    "unsafe",
    "extern",
    "include",
    "include_str",
    "include_bytes",
    "tcpstream",
    "tcplistener",
    "udpsocket",
    "unixstream",
    "unixlistener",
    "unixdatagram",
    "socketaddr",
    "toSocketAddrs",
];

/// Rust escape hatches beyond the doc's list.
const RUST_TOKENS: &[&str] = &[
    "process",
    "os",
    "ffi",
    "libc",
    "asm",
    "global_asm",
    "naked_asm",
    "link",
    "link_name",
    "no_mangle",
    "export_name",
    "link_section",
    "env",
    "option_env",
    "proc_macro",
    "feature",
    "crate_type",
    "no_std",
    "no_main",
    "syscall",
];

/// Python modules and builtins that reach the network, other programs,
/// FFI or dynamic imports / evaluation.
const PYTHON_TOKENS: &[&str] = &[
    "subprocess",
    "os",
    "sys",
    "posix",
    "pty",
    "ctypes",
    "cffi",
    "importlib",
    "__import__",
    "__builtins__",
    "builtins",
    "__loader__",
    "__spec__",
    "__subclasses__",
    "__globals__",
    "__code__",
    "getattr",
    "setattr",
    "globals",
    "locals",
    "vars",
    "eval",
    "exec",
    "compile",
    "breakpoint",
    "urllib",
    "http",
    "ftplib",
    "smtplib",
    "imaplib",
    "poplib",
    "telnetlib",
    "ssl",
    "select",
    "selectors",
    "asyncio",
    "multiprocessing",
    "signal",
    "shutil",
    "requests",
    "pickle",
    "marshal",
    "runpy",
    "code",
    "codecs",
    "webbrowser",
];

/// fish: the only commands a practice script may run (all builtins or
/// keywords). Anything else in command position is refused.
const FISH_COMMANDS: &[&str] = &[
    "echo", "printf", "set", "string", "math", "for", "in", "while", "if", "else", "end",
    "function", "return", "test", "switch", "case", "not", "and", "or", "begin", "count",
    "contains", "break", "continue", "true", "false", "random", "status", "argparse", "[",
];

/// fish words refused anywhere (they run or load other code).
const FISH_TOKENS: &[&str] = &[
    "eval",
    "source",
    "exec",
    "builtin",
    "fish",
    "sh",
    "bash",
    "env",
    "sudo",
    "functions",
    "fish_config",
    "funced",
    "funcsave",
    "open",
    "disown",
    "nohup",
];

/// Checks `src` for `lang`; `Ok` means it may be proposed for a card.
pub fn check(lang: Lang, src: &str) -> Result<(), Rejected> {
    let reject = |token: &str, why| {
        Err(Rejected {
            token: token.to_string(),
            why,
        })
    };
    if src.trim().is_empty() {
        return reject("", "empty source");
    }
    if src.len() > MAX_SOURCE_BYTES {
        return reject("", "source over the size cap");
    }
    if let Some(c) = src.chars().find(|c| !c.is_ascii()) {
        return reject(&c.escape_unicode().to_string(), "non-ASCII source");
    }
    if src.contains('\0') {
        return reject("\\0", "NUL byte in source");
    }
    let toks = tokens(src);
    let hit = |list: &[&str]| -> Option<String> {
        toks.iter()
            .find(|t| list.iter().any(|b| b.eq_ignore_ascii_case(t)))
            .cloned()
    };
    if let Some(t) = hit(DOC_TOKENS) {
        return reject(&t, "network, process, unsafe, FFI or include (docs §6)");
    }
    match lang {
        Lang::Rust => {
            if let Some(t) = hit(RUST_TOKENS) {
                return reject(&t, "a Rust escape hatch");
            }
            // `#![...]` crate attributes and `#[link]`-style attributes
            // beyond the token list: no crate-level attributes at all.
            if src.replace(char::is_whitespace, "").contains("#![") {
                return reject("#![", "a crate-level attribute");
            }
        }
        Lang::Python => {
            if let Some(t) = hit(PYTHON_TOKENS) {
                return reject(&t, "a Python process, network, FFI or dynamic-code path");
            }
        }
        Lang::Fish => {
            if let Some(t) = hit(FISH_TOKENS) {
                return reject(&t, "a fish word that runs other code");
            }
            if let Some(t) = fish_external_command(src) {
                return reject(&t, "a fish command that is not a builtin");
            }
            for bad in ["/dev/tcp", "/dev/udp", "$(", "`"] {
                if src.contains(bad) {
                    return reject(bad, "a fish redirection or substitution");
                }
            }
        }
    }
    Ok(())
}

/// Identifier tokens of `src` (`[A-Za-z0-9_]+`), with `r#` unwrapped.
fn tokens(src: &str) -> Vec<String> {
    src.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// The first word in command position that is not an allowed builtin.
fn fish_external_command(src: &str) -> Option<String> {
    for line in src.lines() {
        // Commands start a line or follow `;`, `|`, `&`, `(`, or a keyword.
        for seg in line.split([';', '|', '&', '(', ')']) {
            let mut words = seg.split_whitespace();
            loop {
                let Some(w) = words.next() else { break };
                if w.starts_with('#') {
                    break;
                }
                if !FISH_COMMANDS.contains(&w) {
                    return Some(w.to_string());
                }
                // A keyword is followed by another command; others take
                // arguments.
                let keyword = matches!(w, "not" | "and" | "or" | "begin" | "if" | "while" | "else");
                if !keyword {
                    break;
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejects(lang: Lang, src: &str) {
        assert!(check(lang, src).is_err(), "{lang:?} accepted:\n{src}");
    }

    #[test]
    fn static_check_accepts_plain_programs() {
        check(Lang::Rust, "fn main() {\n    let v: Vec<u32> = (1..=5).collect();\n    println!(\"{}\", v.iter().sum::<u32>());\n}\n").unwrap();
        check(Lang::Python, "def fib(n):\n    a, b = 0, 1\n    for _ in range(n):\n        a, b = b, a + b\n    return a\nprint(fib(10))\n").unwrap();
        check(
            Lang::Fish,
            "set -l total 0\nfor i in 1 2 3\n    set total (math $total + $i)\nend\necho $total\n",
        )
        .unwrap();
    }

    #[test]
    fn static_check_rejects_each_doc_pattern() {
        // docs §6: std::net, socket, Command, unsafe, extern, include!.
        for src in [
            "use std::net::TcpStream; fn main(){}",
            "fn main(){ let s = socket(); }",
            "use std::process::Command; fn main(){}",
            "fn main(){ unsafe { } }",
            "extern \"C\" { fn abort(); } fn main(){}",
            "fn main(){ include!(\"x.rs\"); }",
            "fn main(){ let _ = include_str!(\"/etc/passwd\"); }",
            "fn main(){ let _ = include_bytes!(\"/etc/passwd\"); }",
        ] {
            rejects(Lang::Rust, src);
        }
        for src in ["import socket\n", "x = 'socket'\n"] {
            rejects(Lang::Python, src);
        }
    }

    #[test]
    fn static_check_survives_simple_obfuscation() {
        for src in [
            // Absolute paths, spacing, braces, aliases and globs.
            "fn main(){ ::std::net::TcpStream::connect(\"1.2.3.4:80\").ok(); }",
            "fn main(){ std :: net :: TcpStream::connect(\"x\").ok(); }",
            "use std::{net}; fn main(){}",
            "use std::net as n; fn main(){ n::TcpStream::connect(\"x\").ok(); }",
            "use std::{self as s}; fn main(){ s::net::TcpStream::connect(\"x\").ok(); }",
            "extern crate std as s; fn main(){}",
            "use std::*; fn main(){ let _ = TcpStream::connect(\"x\"); }",
            "use std::io::*; fn main(){ let _ = UdpSocket::bind(\"x\"); }",
            // Macros pasting a path together.
            "macro_rules! p { ($a:ident, $b:ident) => { $a::$b::TcpStream::connect(\"x\") } }\nfn main(){ p!(std, net).ok(); }",
            // Raw identifiers and case games.
            "fn main(){ r#unsafe(); }",
            "use std::PROCESS; fn main(){}",
            // Comments still count (cheap and strict).
            "fn main(){} // std::net later",
            // Other Rust escape hatches.
            "fn main(){ core::arch::asm!(\"nop\"); }",
            "#[no_mangle] pub fn f(){} fn main(){}",
            "#![feature(rustc_private)] fn main(){}",
            "fn main(){ println!(\"{}\", env!(\"HOME\")); }",
            "use std::os::unix::fs::PermissionsExt; fn main(){}",
            "fn main(){ let _ = std::fs::read(\"x\"); } use std::ffi::CString;",
        ] {
            rejects(Lang::Rust, src);
        }
        for src in [
            "s = __import__('so' + 'cket')\n",
            "import importlib\nm = importlib.import_module('sock' + 'et')\n",
            "import subprocess\nsubprocess.run(['true'])\n",
            "import os\nos.system('true')\n",
            "from os import system as s\ns('true')\n",
            "import ctypes\n",
            "eval('1+1')\n",
            "exec('print(1)')\n",
            "getattr(__builtins__, 'ev' + 'al')('1')\n",
            "import urllib.request\n",
            "import http.client\n",
            "print(().__class__.__base__.__subclasses__())\n",
            // Full-width letters fold to `socket` in Python (NFKC).
            "import \u{ff53}ocket\n",
        ] {
            rejects(Lang::Python, src);
        }
        for src in [
            "echo hi; nc 127.0.0.1 22\n",
            "echo hi | python3 -c 'print(1)'\n",
            "eval echo hi\n",
            "source other.fish\n",
            "echo (ls /)\n",
            "if true; rm -rf /work; end\n",
            "echo hi > /dev/tcp/127.0.0.1/80\n",
            "command ls\n",
        ] {
            rejects(Lang::Fish, src);
        }
    }

    #[test]
    fn static_check_refuses_empty_huge_and_non_ascii() {
        rejects(Lang::Python, "  \n");
        rejects(Lang::Python, &"print(1)\n".repeat(MAX_SOURCE_BYTES));
        rejects(Lang::Rust, "fn main(){ let \u{0441} = 1; }");
        rejects(Lang::Python, "print(1)\0");
    }
}
