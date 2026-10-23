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
//! On non-Windows the whole thing is an error, so the plan's elevated branch
//! compiles everywhere but only runs on Windows.

use std::path::Path;
use std::time::Duration;

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

/// Tells any running helper to exit (app shutdown; the safeword hook calls
/// this). No-op when none is running or off Windows.
pub fn shutdown() {
    #[cfg(windows)]
    imp::shutdown();
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
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use std::time::Instant;

    struct Helper {
        pipe: std::fs::File,
        token: String,
        per_command: bool,
    }

    static HELPER: Mutex<Option<Helper>> = Mutex::new(None);
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    pub fn shutdown() {
        let mut g = HELPER.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = g.as_mut() {
            let _ = write_frame(&mut h.pipe, &Request::Stop.encode(&h.token));
        }
        *g = None;
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
        // The safeword kills the helper too (registered once).
        crate::access::kill_switch::on_stop(shutdown);

        let mut g = HELPER.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = Some(launch(helper, per_command, idle_min)?);
        }
        let h = g.as_mut().unwrap();
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let frame = Request::Run {
            id,
            shell,
            cwd: cwd.map(|p| p.display().to_string()).unwrap_or_default(),
            timeout_s: timeout.as_secs().max(1),
            cap,
            command: command.to_string(),
        };
        write_frame(&mut h.pipe, &frame.encode(&h.token))?;
        let reply = read_frame(&mut h.pipe)
            .map_err(|e| std::io::Error::other(format!("admin helper: {e}")))?;
        let out = match Reply::decode(&reply, &h.token)
            .map_err(|e| std::io::Error::other(format!("admin helper: {e}")))?
        {
            Reply::Ok { id: rid, output } if rid == id => Ok(output),
            Reply::Err { id: rid, message } if rid == id => Err(std::io::Error::other(message)),
            _ => Err(std::io::Error::other("admin helper: id mismatch")),
        };
        if per_command {
            let _ = write_frame(&mut h.pipe, &Request::Stop.encode(&h.token));
            *g = None;
        }
        out
    }

    fn launch(helper: &Path, per_command: bool, idle_min: u64) -> std::io::Result<Helper> {
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
        // Create the pipe server first (ACL to Ko only) so the helper, which
        // connects as a client, has something to attach to and cannot be
        // pre-empted by another listener squatting the name.
        let server = create_pipe(&pipe_name, &sid)?;
        shell_execute_runas(helper, &args.to_args())?;
        let pipe = connect_server(server)?;
        let mut h = Helper {
            pipe,
            token,
            per_command,
        };
        // Handshake: the helper echoes a Stop-less ping by accepting our first
        // real request; nothing to do here beyond having the connection.
        let _ = &mut h;
        Ok(h)
    }

    // --- Win32 glue -------------------------------------------------------

    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
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
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Ko's account SID as a string (`S-1-5-21-…`).
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
            // Waits for the elevated helper to connect as a client.
            let ok = ConnectNamedPipe(server.0, std::ptr::null_mut());
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                // ERROR_PIPE_CONNECTED (535) means it connected first: fine.
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

    fn shell_execute_runas(exe: &Path, args: &[String]) -> std::io::Result<()> {
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
            if !info.hProcess.is_null() {
                CloseHandle(info.hProcess);
            }
            Ok(())
        }
    }

    // Keep otherwise-unused imports referenced on all Windows builds.
    #[allow(dead_code)]
    fn _link(_: Instant, _: GENERIC_READ, _: GENERIC_WRITE) {}
}
