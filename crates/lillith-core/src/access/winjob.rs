//! Windows Job Objects used to kill a whole process tree at once.
//!
//! A job with [`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`] means closing (or
//! terminating) the job handle kills every process still in it — including
//! grandchildren. The elevated helper owns one job for its command children;
//! non-elevated `cmd` / PowerShell runs get one per invocation.
//!
//! Off Windows this module is empty stubs so call sites compile everywhere.

#![allow(dead_code)]

/// A job that kills its members when dropped or [`Job::terminate`]d.
pub struct Job {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

// SAFETY: the handle is used only through Win32 APIs that accept a HANDLE
// from any thread; we never share mutable Rust state across threads.
#[cfg(windows)]
unsafe impl Send for Job {}

impl Job {
    /// Creates a job with `KILL_ON_JOB_CLOSE` set.
    pub fn new_kill_on_close() -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
            use windows_sys::Win32::System::JobObjects::{
                CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            };
            unsafe {
                let h: HANDLE = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() || h == INVALID_HANDLE_VALUE {
                    return Err(std::io::Error::last_os_error());
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    let e = std::io::Error::last_os_error();
                    CloseHandle(h);
                    return Err(e);
                }
                Ok(Job { handle: h })
            }
        }
        #[cfg(not(windows))]
        {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "job objects are Windows-only",
            ))
        }
    }

    /// Puts an already-started process into this job by pid.
    pub fn assign_pid(&self, pid: u32) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
            use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
            use windows_sys::Win32::System::Threading::{
                OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
            };
            unsafe {
                let proc: HANDLE = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if proc.is_null() {
                    return Err(std::io::Error::last_os_error());
                }
                let ok = AssignProcessToJobObject(self.handle, proc);
                let err = if ok == 0 {
                    Some(std::io::Error::last_os_error())
                } else {
                    None
                };
                CloseHandle(proc);
                match err {
                    Some(e) => Err(e),
                    None => Ok(()),
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = pid;
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "job objects are Windows-only",
            ))
        }
    }

    /// Kills every process still in the job.
    pub fn terminate(&self) {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::JobObjects::TerminateJobObject;
            unsafe {
                let _ = TerminateJobObject(self.handle, 1);
            }
        }
    }

    #[cfg(windows)]
    pub fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.handle
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            // KILL_ON_JOB_CLOSE: closing the last handle kills members.
            use windows_sys::Win32::Foundation::CloseHandle;
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

/// Outcome of trying to kill the elevated helper (cfg-independent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperKill {
    /// No helper was tracked as alive.
    Idle,
    /// Kill was issued and the process wait confirmed it exited.
    Gone,
    /// Kill was issued but we could not confirm exit in time.
    Unconfirmed,
    /// Kill failed to start (no handle / TerminateProcess error).
    Failed,
}

impl HelperKill {
    /// Full stop (Ctrl-C / SIGTERM) reply. Never claims the helper is gone unless
    /// [`HelperKill::Gone`].
    pub fn chat_line(self) -> &'static str {
        match self {
            HelperKill::Idle => "Stopped. Every running command is dead.",
            HelperKill::Gone => {
                "Stopped. Every running command is dead and the admin helper is gone."
            }
            HelperKill::Unconfirmed => {
                "Stopped. Every running command is dead; stopping the admin helper…"
            }
            HelperKill::Failed => {
                "Stopped. Every running command is dead; could not stop the admin helper."
            }
        }
    }
}

/// Builds the stop reply line from a helper-kill outcome.
pub fn stop_chat_line(helper: HelperKill) -> String {
    helper.chat_line().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_chat_line_is_truthful() {
        assert_eq!(
            stop_chat_line(HelperKill::Idle),
            "Stopped. Every running command is dead."
        );
        assert_eq!(
            stop_chat_line(HelperKill::Gone),
            "Stopped. Every running command is dead and the admin helper is gone."
        );
        assert_eq!(
            stop_chat_line(HelperKill::Unconfirmed),
            "Stopped. Every running command is dead; stopping the admin helper…"
        );
        assert_eq!(
            stop_chat_line(HelperKill::Failed),
            "Stopped. Every running command is dead; could not stop the admin helper."
        );
        // Never say "gone" unless confirmed.
        for k in [
            HelperKill::Idle,
            HelperKill::Unconfirmed,
            HelperKill::Failed,
        ] {
            assert!(!stop_chat_line(k).contains("helper is gone"), "{k:?}");
        }
    }
}
