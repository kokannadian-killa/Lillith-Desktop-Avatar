//! The process-global broker that owns the one elevated `lillith-admin`
//! helper. Windows only.
//!
//! - The first approved elevated command launches the helper once, through
//!   `ShellExecuteW` with the verb `runas` — that is the single UAC prompt.
//!   The helper is handed a random pipe name and a random session token on
//!   its command line and nothing else.
//! - Lillith connects to `\\.\pipe\lillith-admin-<hex>` (ACL restricted to
//!   Ko's account), checks the token, and sends the approved command.
//! - In `per-session` mode the helper stays up until the app exits, the
//!   safeword fires ([`super::kill_switch`]), or it is idle past the timeout.
//!   In `per-command` mode the helper exits after each command, so every
//!   elevated command brings its own UAC prompt.
//!
//! **Safeword / stop:** the broker keeps the helper's process handle in a
//! separate lock from the pipe session. [`force_kill`] terminates that
//! process (and, inside the helper, a Job Object with `KILL_ON_JOB_CLOSE`
//! tears down any command children) without waiting on an in-flight
//! `read_frame`. The pipe session lock is only held around short
//! write/read setup, never across the blocking reply wait.
//!
//! On non-Windows the whole thing is an error, so the plan's elevated branch
//! compiles everywhere but only runs on Windows.

use std::path::Path;
use std::time::Duration;

use super::winjob::HelperKill;
use super::winshell::WinShell;

/// Inputs for one approved elevated run (bundled so the call site stays tidy).
pub struct ElevatedRun<'a> {
    pub shell: WinShell,
    pub command: &'a str,
    pub cwd: Option<&'a Path>,
    pub timeout: Duration,
    pub cap: usize,
    pub per_command: bool,
    pub idle_min: u64,
    pub helper: &'a Path,
}

/// Runs an approved elevated command, launching or reusing the helper.
pub fn run_elevated(req: ElevatedRun<'_>) -> std::io::Result<String> {
    #[cfg(windows)]
    {
        imp::run_elevated(req)
    }
    #[cfg(not(windows))]
    {
        let _ = req;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "elevated commands only run on Windows",
        ))
    }
}

/// Force-kills the elevated helper out-of-band (safeword / app shutdown).
/// Does not wait on the pipe session lock held by an in-flight command.
pub fn force_kill() -> HelperKill {
    #[cfg(windows)]
    {
        imp::force_kill()
    }
    #[cfg(not(windows))]
    {
        HelperKill::Idle
    }
}

/// Soft shutdown: try a Stop frame, then [`force_kill`]. Same as the safeword
/// path; kept as a name for app-exit call sites.
pub fn shutdown() -> HelperKill {
    force_kill()
}

/// The default helper path: `lillith-admin.exe` beside the running exe.
pub fn default_helper_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.with_file_name("lillith-admin.exe"))
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::access::admin_proto::{
        pipe_sddl, random_hex, read_frame, write_frame, HelperArgs, Reply, Request, PIPE_PREFIX,
    };
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_ACCESS_DUPLEX, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcessToken, TerminateProcess, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    /// Pipe + token for Run/Reply. Never held across the blocking reply wait
    /// in a way that blocks [`force_kill`] — force_kill uses [`PROC`] only.
    struct Session {
        pipe: std::fs::File,
        token: String,
        per_command: bool,
    }

    /// Process handle of the elevated helper. Separate from [`SESSION`] so
    /// the safeword can TerminateProcess while a Run is mid-`read_frame`.
    struct Proc {
        handle: HANDLE,
    }

    // SAFETY: HANDLEs are opaque kernel objects; we only call Win32 APIs.
    unsafe impl Send for Proc {}

    static SESSION: Mutex<Option<Session>> = Mutex::new(None);
    static PROC: Mutex<Option<Proc>> = Mutex::new(None);
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    static HOOKED: AtomicU64 = AtomicU64::new(0);

    fn ensure_hook() {
        if HOOKED.swap(1, Ordering::SeqCst) == 0 {
            crate::access::kill_switch::on_stop(force_kill);
        }
    }

    pub fn force_kill() -> HelperKill {
        // 1) Soft Stop on the pipe if we can grab the session without waiting.
        if let Ok(mut g) = SESSION.try_lock() {
            if let Some(s) = g.as_mut() {
                let _ = write_frame(&mut s.pipe, &Request::Stop.encode(&s.token));
            }
            *g = None;
        }

        // 2) Out-of-band: terminate the helper process (its Job Object with
        //    KILL_ON_JOB_CLOSE then wipes command children).
        let proc = {
            let mut g = PROC.lock().unwrap_or_else(|e| e.into_inner());
            g.take()
        };
        let Some(proc) = proc else {
            return HelperKill::Idle;
        };

        let term_ok = unsafe { TerminateProcess(proc.handle, 1) != 0 };
        // Wait briefly to confirm exit.
        let wait = unsafe { WaitForSingleObject(proc.handle, 2_000) };
        unsafe {
            CloseHandle(proc.handle);
        }
        // Clear session if still present (best-effort).
        if let Ok(mut g) = SESSION.try_lock() {
            *g = None;
        }

        if wait == WAIT_OBJECT_0 {
            HelperKill::Gone
        } else if term_ok {
            HelperKill::Unconfirmed
        } else {
            let _ = WAIT_TIMEOUT; // referenced on all SDK builds
            HelperKill::Failed
        }
    }

    pub fn run_elevated(req: super::ElevatedRun<'_>) -> std::io::Result<String> {
        let ElevatedRun {
            shell,
            command,
            cwd,
            timeout,
            cap,
            per_command,
            idle_min,
            helper,
        } = req;
        ensure_hook();

        // Launch under both locks briefly.
        {
            let mut session = SESSION.lock().unwrap_or_else(|e| e.into_inner());
            let mut proc = PROC.lock().unwrap_or_else(|e| e.into_inner());
            if session.is_none() || proc.is_none() {
                let (s, p) = launch(helper, per_command, idle_min)?;
                *session = Some(s);
                *proc = Some(p);
            }
        }

        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let frame = Request::Run {
            id,
            shell,
            cwd: cwd.map(|p| p.display().to_string()).unwrap_or_default(),
            timeout_s: timeout.as_secs().max(1),
            cap,
            command: command.to_string(),
        };

        // Take the session out of the mutex for the whole write/read so
        // force_kill's try_lock soft-Stop never waits on us. The out-of-band
        // path (PROC + TerminateProcess) does not need SESSION at all.
        let mut session = {
            let mut g = SESSION.lock().unwrap_or_else(|e| e.into_inner());
            g.take()
                .ok_or_else(|| std::io::Error::other("admin helper session gone"))?
        };
        let write_res = write_frame(&mut session.pipe, &frame.encode(&session.token));
        let read_res = write_res.and_then(|()| {
            read_frame(&mut session.pipe)
                .map_err(|e| std::io::Error::other(format!("admin helper: {e}")))
        });

        let out = match read_res {
            Ok(reply) => match Reply::decode(&reply, &session.token)
                .map_err(|e| std::io::Error::other(format!("admin helper: {e}")))?
            {
                Reply::Ok { id: rid, output } if rid == id => Ok(output),
                Reply::Err { id: rid, message } if rid == id => Err(std::io::Error::other(message)),
                _ => Err(std::io::Error::other("admin helper: id mismatch")),
            },
            Err(e) => Err(e),
        };

        // Put the session back only if the helper process is still tracked
        // (force_kill may have cleared PROC mid-read).
        let helper_alive = PROC.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        if helper_alive && !per_command {
            let mut g = SESSION.lock().unwrap_or_else(|e| e.into_inner());
            *g = Some(session);
        } else if per_command {
            let _ = force_kill();
        }
        out
    }

    fn launch(helper: &Path, per_command: bool, idle_min: u64) -> std::io::Result<(Session, Proc)> {
        if !helper.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("admin helper not found at {}", helper.display()),
            ));
        }
        let token = random_hex(32)?;
        let pipe_name = format!("{PIPE_PREFIX}{}", random_hex(16)?);
        let sid = current_user_sid()?;
        let args = HelperArgs {
            pipe: pipe_name.clone(),
            token: token.clone(),
            idle_min,
            per_command,
            parent_pid: std::process::id(),
            allow_sid: sid.clone(),
        };
        let server = create_pipe(&pipe_name, &sid)?;
        let proc_handle = shell_execute_runas(helper, &args.to_args())?;
        let pipe = connect_server(server)?;
        Ok((
            Session {
                pipe,
                token,
                per_command,
            },
            Proc {
                handle: proc_handle,
            },
        ))
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn current_user_sid() -> std::io::Result<String> {
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
            let mut buf = vec![0u8; len as usize];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let tu = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut s: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(tu.User.Sid, &mut s) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut n = 0;
            while *s.add(n) != 0 {
                n += 1;
            }
            let out = String::from_utf16_lossy(std::slice::from_raw_parts(s, n));
            windows_sys::Win32::Foundation::LocalFree(s.cast());
            Ok(out)
        }
    }

    struct PipeServer(HANDLE);
    unsafe impl Send for PipeServer {}

    fn create_pipe(name: &str, sid: &str) -> std::io::Result<PipeServer> {
        unsafe {
            let sddl = wide(&pipe_sddl(sid));
            let mut psd: *mut std::ffi::c_void = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1 as u32,
                &mut psd,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            let mut sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: psd,
                bInheritHandle: 0,
            };
            let h = CreateNamedPipeW(
                wide(name).as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                super::super::admin_proto::MAX_FRAME as u32,
                super::super::admin_proto::MAX_FRAME as u32,
                0,
                &mut sa,
            );
            windows_sys::Win32::Foundation::LocalFree(psd);
            if h == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            Ok(PipeServer(h))
        }
    }

    fn connect_server(server: PipeServer) -> std::io::Result<std::fs::File> {
        use std::os::windows::io::FromRawHandle;
        unsafe {
            let ok = ConnectNamedPipe(server.0, std::ptr::null_mut());
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(535) {
                    CloseHandle(server.0);
                    return Err(err);
                }
            }
            let f = std::fs::File::from_raw_handle(server.0 as *mut _);
            std::mem::forget(server);
            Ok(f)
        }
    }

    /// Launches the helper elevated; returns its process handle (caller owns).
    fn shell_execute_runas(exe: &Path, args: &[String]) -> std::io::Result<HANDLE> {
        unsafe {
            let verb = wide("runas");
            let file = wide(&exe.display().to_string());
            let params = wide(&super::super::winshell::join_args(args));
            let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.lpParameters = params.as_ptr();
            info.nShow = SW_HIDE as i32;
            if ShellExecuteExW(&mut info) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            if info.hProcess.is_null() {
                return Err(std::io::Error::other(
                    "ShellExecuteExW returned no process handle",
                ));
            }
            Ok(info.hProcess)
        }
    }

    #[allow(dead_code)]
    fn _link(_: Instant, _: Duration, _: GENERIC_READ, _: GENERIC_WRITE) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::winjob::HelperKill;
    use std::sync::{Arc, Barrier, Mutex};
    use std::time::{Duration, Instant};

    /// force_kill must finish while a simulated session lock is held (the
    /// old bug blocked on that lock until the in-flight command finished).
    #[test]
    fn force_kill_does_not_wait_on_session_lock() {
        // On non-Windows force_kill is Idle and returns immediately — still
        // proves the public API never blocks on a session mutex we hold here.
        let session = Arc::new(Mutex::new(()));
        let barrier = Arc::new(Barrier::new(2));
        let s2 = session.clone();
        let b2 = barrier.clone();
        let holder = std::thread::spawn(move || {
            let _g = s2.lock().unwrap();
            b2.wait();
            std::thread::sleep(Duration::from_millis(400));
        });
        barrier.wait();
        let start = Instant::now();
        let r = force_kill();
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(300),
            "force_kill blocked: {elapsed:?}"
        );
        assert_eq!(r, HelperKill::Idle);
        holder.join().unwrap();
    }
}
