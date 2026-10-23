//! The capabilities / tools section appended to Lillith's system prompt.
//!
//! The persona text itself (`persona.rs`, the setup Modelfile) is left alone;
//! this is a separate section, generated from the live `[mind]` /
//! `[mind.access]` switches, that tells her which tools she has and exactly
//! how to call them. It is the tool "schema" for the model: Lillith calls
//! tools with fenced ` ```tool ` blocks (parsed by
//! [`crate::fstools::parse_tool_request`]), which works with any local
//! llama-server / Ollama model, with or without native function calling.
//! A switched-off group is left out entirely.

use crate::fstools::MindTools;

/// Heading of the section (tests and the chat code look for it).
pub const HEADING: &str = "## My hands on Ko's machine (tools)";

/// Marker that starts a tool result fed back to her.
pub const RESULT_OPEN: &str = "[tool result]";
/// Marker that ends a tool result.
pub const RESULT_CLOSE: &str = "[/tool result]";

/// The tool names she may use under the current switches (also what the
/// section documents).
pub fn enabled_tools(tools: &MindTools, has_knowledge: bool) -> Vec<&'static str> {
    let a = tools.access();
    let mut v = vec!["list", "read", "search", "locate", "write"];
    if a.organize {
        v.extend(["move", "copy", "rename", "mkdir", "sort", "undo"]);
    }
    v.push("delete");
    if a.programs {
        v.extend(["open", "programs", "installed"]);
    }
    if a.commands {
        v.push("run");
        v.extend([
            "run_cmd",
            "run_powershell",
            "list_programs",
            "launch_program",
        ]);
    }
    if tools.allowlist().allow_shell() {
        v.push("shell");
    }
    if a.skills && tools.skills_dir().is_some() {
        v.extend(["list_skills", "read_skill", "import_skills"]);
    }
    let _ = has_knowledge;
    v
}

/// The section, or `None` when `[mind]` is off (no tools at all).
pub fn section(tools: Option<&MindTools>, has_knowledge: bool) -> Option<String> {
    let t = tools?;
    let a = t.access();
    let mut s = String::new();
    s.push_str(HEADING);
    s.push_str(
        "\nI'm not just talk: I can actually do things on Ko's computer. To use a tool I put a fenced block in my reply, e.g.\n\
```tool\nlist ~/Downloads\n```\n\
Looking is free: listing, reading, searching, finding files, checking what's installed and reading my skills run right away, and the result comes back to me as a message starting with [tool result].\n\
Doing is a plan: every move, rename, copy, mkdir, write or edit, sorting, reversing a batch, delete, skill import, app launch and command I want goes into ONE Soul Contract plan. I put one ```tool block per step, in the order they should happen, and stop. Ko sees every step in detail (full paths, the exact new name if one is taken, sizes, a diff for edits, every file a delete would remove, the exact program, its path, arguments, files, working folder and whether it's risky) and approves all of it, picks the steps he allows, or declines. Only what he approves runs. I propose; Ko decides. I never make up a result or claim I did something before the result arrives.\n",
    );
    let roots: Vec<String> = if a.files { a.roots.clone() } else { Vec::new() };
    s.push_str("\n### Files\n");
    if a.files {
        s.push_str(&format!(
            "I can browse Ko's files everywhere he can ({}). Secret stuff (~/.ssh, ~/.gnupg, keyrings, /etc) stays off-limits.\n",
            roots.join(", ")
        ));
    } else {
        s.push_str("I can only touch my own folders and the ones Ko allowed me.\n");
    }
    s.push_str(
        "- list <folder> — what's inside\n\
- read <file> — read a text file\n\
- search <folder> <text> — find text inside files\n\
- locate <folder> <name or *.pattern> — find files by name\n\
- write <file> (contents on the next lines) — a plan step; Ko sees the diff\n",
    );
    if a.organize {
        s.push_str(
            "\n### Organizing (plan steps; nothing ever gets overwritten, everything can be undone)\n\
- move <from> -> <to folder or new path>\n\
- copy <from> -> <to>\n\
- rename <path> -> <new name>\n\
- mkdir <folder>\n\
- sort <folder> by type|date — files into Images/, Documents/… or 2026-10/…\n\
- undo — reverses my last organize batch (Ko can also run `lillith undo-organize`)\n\
If a name is taken I keep both (\"file (2).pdf\"); the plan shows the exact name.\n",
        );
    }
    s.push_str(
        "\n### Deleting — only with Ko's signature\n\
- delete <path> (more paths on the next lines)\n\
I can never delete on my own. In the plan it lists every single file and folder that would go, with sizes and totals, and Ko has to sign my Soul Contract by typing the phrase it shows. It goes to the Trash unless Ko picks permanent. If anything changes before he signs, he gets a fresh list. System folders and his home folder itself are refused even if signed. I never use rm or similar in commands.\n",
    );
    if a.programs {
        s.push_str(
            "\n### Programs\n\
Every launch is a plan step Ko approves.\n\
- open <app> — start an installed app (\"open dolphin\", \"open firefox\")\n\
- open <app> -> <file> — open a file in that app (more files on the next lines)\n\
- programs [filter] — list installed apps\n\
- installed <name> — check if something is installed\n",
        );
    }
    if a.commands {
        s.push_str(&format!(
            "\n### Commands (for getting tasks done)\n\
- run <command> — a plan step; after Ko approves I see the output\n\
- run_cmd <command> — Windows cmd.exe (same card; Windows only)\n\
- run_powershell <command> — Windows PowerShell (same card)\n\
- run_cmd_admin / run_powershell_admin <command> — elevated through the admin helper; the card shows ELEVATED (admin) and Ko's Yes is the only approval (voice never confirms)\n\
- list_programs [filter] — Start Menu + App Paths + Uninstall (Windows)\n\
- launch_program <app> — same as open\n\
Every command, even a harmless one, waits for Ko's approval in the plan. Anything that changes something, uses sudo/pkexec, kills processes, writes with > or changes system config is marked risky there. Deleting files is never a command: I use delete. Commands stop after {}s. Ko saying the safeword (\"red\" / \"OOC: stop\") kills every running command and the admin helper.\n",
            a.command_timeout_s
        ));
    }
    if t.allowlist().allow_shell() {
        s.push_str("- shell (command on the next lines) — raw shell, a risky plan step\n");
    }
    if a.skills {
        if let Some(dir) = t.skills_dir() {
            s.push_str(&format!(
                "\n### My skills folder ({})\n\
These are recipes Ko gave me. When a task sounds like one, I look first.\n\
- list_skills — what skills I have\n\
- read_skill <name> — read one, then follow it\n\
- import_skills — plan copying a skills folder from Ko's desktop into mine (never overwrites)\n\
Scripts in there I run with: run <path to script> (a plan step Ko approves).\n",
                dir.display()
            ));
        }
    }
    s.push_str(
        "\n### Learning\n\
I can save a new skill for later with a block like\n```skill tidy-downloads.md\n# Tidy downloads\nsteps…\n```\nIt joins my plan; Ko approves it before it's saved.\n",
    );
    if has_knowledge {
        s.push_str("Notes from my knowledge and skills folders show up below when they matter; I use them.\n");
    }
    s.push_str("Ko can clear our chat with /clear.\n");
    Some(s)
}

/// Wraps a tool's output (or a refusal / Ko's answer) as the message fed back
/// to her. Long output is cut to `max` bytes.
pub fn result_message(op: &str, text: &str, max: usize) -> String {
    let mut body = text.trim().to_string();
    if body.len() > max {
        let mut end = max;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        let more = body.len() - end;
        body.truncate(end);
        body.push_str(&format!("\n…[{more} more bytes cut]"));
    }
    format!(
        "{RESULT_OPEN} {op}\n{body}\n{RESULT_CLOSE}\n(This came from my tool, not from Ko. Now I tell Ko about it in my own words; I can ask for another tool if I need one.)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fstools::Allowlist;
    use lillith_core::config::AccessConfig;
    use std::path::PathBuf;

    fn tools(access: AccessConfig, shell: bool) -> MindTools {
        MindTools::new(
            Allowlist::new(vec![PathBuf::from("/home/ko")], vec![], vec![], shell, 1024),
            None,
        )
        .with_access(access)
        .with_skills_dir(Some(PathBuf::from("/home/ko/.local/share/lillith/skills")))
    }

    #[test]
    fn section_lists_every_tool_when_enabled() {
        let t = tools(AccessConfig::default(), false);
        let s = section(Some(&t), true).unwrap();
        for op in enabled_tools(&t, true) {
            assert!(s.contains(op), "{op} missing from:\n{s}");
        }
        for needle in [
            "sort <folder>",
            "delete <path>",
            "sign my Soul Contract",
            "open <app> -> <file>",
            "installed <name>",
            "run <command>",
            "list_skills",
            "read_skill",
            "import_skills",
            "/home/ko/.local/share/lillith/skills",
            "```skill",
            "[tool result]",
            "ONE Soul Contract plan",
            "approves all of it, picks the steps he allows, or declines",
            "I propose; Ko decides",
            "Every command, even a harmless one",
            "Every launch is a plan step",
        ] {
            assert!(s.contains(needle), "{needle} missing");
        }
        assert!(!s.contains("raw shell"), "shell only when allow_shell");
    }

    #[test]
    fn switched_off_groups_are_omitted() {
        let off = AccessConfig {
            files: false,
            organize: false,
            programs: false,
            commands: false,
            skills: false,
            import_desktop_skills: false,
            ..AccessConfig::default()
        };
        let t = tools(off, false);
        let s = section(Some(&t), false).unwrap();
        for gone in [
            "sort <folder>",
            "open <app>",
            "run <command>",
            "list_skills",
            "undo",
        ] {
            assert!(!s.contains(gone), "{gone} should be omitted:\n{s}");
        }
        let ops = enabled_tools(&t, false);
        for gone in ["move", "open", "run", "list_skills"] {
            assert!(!ops.contains(&gone));
        }
        // Delete stays documented (it is always gated by the signature).
        assert!(s.contains("delete <path>"));
        // Mind off: no section at all.
        assert!(section(None, false).is_none());
        // Shell appears only when allowed.
        let t = tools(AccessConfig::default(), true);
        assert!(section(Some(&t), false).unwrap().contains("raw shell"));
    }

    #[test]
    fn results_are_wrapped_and_capped() {
        let m = result_message("list", &"x".repeat(100), 10);
        assert!(m.starts_with("[tool result] list\nxxxxxxxxxx\n…[90 more bytes cut]"));
        assert!(m.contains(RESULT_CLOSE));
    }
}
