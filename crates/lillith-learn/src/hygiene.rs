//! Process hygiene for the learner thread (§1): lowest CPU priority and
//! idle I/O class, so a step never competes with Ko's work or the model.

/// Lowers the *calling thread* to nice 19 and the idle I/O class (Linux),
/// or background mode (Windows). Best effort; returns what was applied.
pub fn lower_current_thread() -> &'static str {
    imp::lower()
}

#[cfg(target_os = "linux")]
mod imp {
    /// `IOPRIO_CLASS_IDLE << IOPRIO_CLASS_SHIFT` (linux/ioprio.h).
    const IOPRIO_IDLE: libc::c_long = 3 << 13;
    /// `IOPRIO_WHO_PROCESS` (a tid is a "process" for ioprio).
    const IOPRIO_WHO_PROCESS: libc::c_long = 1;

    pub fn lower() -> &'static str {
        // SAFETY: plain syscalls on the current thread id; no pointers.
        unsafe {
            let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
            let nice = libc::setpriority(libc::PRIO_PROCESS, tid, 19) == 0;
            let io = libc::syscall(
                libc::SYS_ioprio_set,
                IOPRIO_WHO_PROCESS,
                tid as libc::c_long,
                IOPRIO_IDLE,
            ) == 0;
            match (nice, io) {
                (true, true) => "nice 19 + ioprio idle",
                (true, false) => "nice 19",
                (false, true) => "ioprio idle",
                (false, false) => "unchanged",
            }
        }
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
mod imp {
    pub fn lower() -> &'static str {
        // SAFETY: setpriority on this process; no pointers.
        if unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 19) } == 0 {
            "nice 19"
        } else {
            "unchanged"
        }
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
    };

    pub fn lower() -> &'static str {
        // SAFETY: the pseudo-handle of the current thread.
        if unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) } != 0 {
            "background mode"
        } else {
            "unchanged"
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    pub fn lower() -> &'static str {
        "unchanged"
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn lowering_a_scratch_thread_does_not_fail() {
        let r = std::thread::spawn(super::lower_current_thread)
            .join()
            .unwrap();
        assert!(!r.is_empty());
    }
}
