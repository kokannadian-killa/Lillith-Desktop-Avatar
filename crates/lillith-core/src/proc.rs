//! Child processes that never flash a console window on Windows.
//!
//! Lillith's GUI binaries use the `windows` subsystem, so every console
//! program they start (ffmpeg, ffprobe, Piper, whisper, llama-server,
//! `cmd.exe`, PowerShell, `reg.exe`, `taskkill`, swarm workers, ...) would
//! otherwise get its own new console window: one per clip decode, stacking
//! up on screen. Every spawn goes through [`command`] (or [`hide`] for a
//! `Command` built elsewhere), which sets `CREATE_NO_WINDOW` on Windows and
//! is a no-op everywhere else. `tests/no_raw_command.rs` fails the build on
//! a raw `Command::new` outside this module.

use std::ffi::OsStr;
use std::process::Command;

/// `CREATE_NO_WINDOW` (`0x0800_0000`): the child gets no console window.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `std::process::Command::new(program)` with [`hide`] applied.
pub fn command<S: AsRef<OsStr>>(program: S) -> Command {
    let mut c = Command::new(program);
    hide(&mut c);
    c
}

/// Sets `CREATE_NO_WINDOW` on Windows (no-op elsewhere). A later
/// `creation_flags` call replaces it, so callers that need more flags use
/// [`hide_with`].
pub fn hide(c: &mut Command) -> &mut Command {
    hide_with(c, 0)
}

/// [`hide`] plus extra Windows creation flags (e.g. `CREATE_NEW_PROCESS_GROUP`).
#[allow(unused_variables)]
pub fn hide_with(c: &mut Command, extra_flags: u32) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW | extra_flags);
    }
    c
}

#[cfg(test)]
mod tests {
    #[test]
    fn command_runs_and_keeps_the_program() {
        let c = super::command("lillith-proc-test");
        assert_eq!(c.get_program(), "lillith-proc-test");
    }

    #[test]
    fn flag_value_is_the_win32_constant() {
        assert_eq!(super::CREATE_NO_WINDOW, 0x0800_0000);
    }
}
