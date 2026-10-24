//! `lillith-admin`: the elevated helper.
//!
//! Started once (or once per elevated command) by Lillith through
//! `ShellExecuteW` with the verb `runas`. Its only job is to accept
//! commands over a local named pipe ACL'd to Ko's account, check the
//! session token on every frame, run them via `cmd` / PowerShell with
//! captured output, and exit on Stop, idle timeout, or when Lillith dies.
//!
//! Stop is handled on a dedicated reader thread so an in-flight command
//! cannot delay it: the reader calls `ExitProcess`, which closes every
//! handle — including Job Objects created with `KILL_ON_JOB_CLOSE` for
//! command children — so the whole tree dies with the helper.
//!
//! Nothing here decides what is allowed: Lillith only sends a command
//! after Ko approved it on the Soul Contract card. There is no network
//! listener — only `\\.\pipe\lillith-admin-<hex>`.

#![cfg_attr(windows, windows_subsystem = "windows")]

use lillith_core::access::admin_proto::HelperArgs;

#[cfg(windows)]
use lillith_core::access::admin_proto::{read_frame, write_frame, Reply, Request};
#[cfg(windows)]
use lillith_core::access::winshell;
#[cfg(windows)]
use std::io::{Read, Write};
#[cfg(windows)]
use std::path::Path;
#[cfg(windows)]
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = match HelperArgs::parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("lillith-admin: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(cfg) {
        eprintln!("lillith-admin: {e}");
        std::process::exit(1);
    }
}

fn run(cfg: HelperArgs) -> Result<(), String> {
    #[cfg(windows)]
    {
        win::serve(cfg)
    }
    #[cfg(not(windows))]
    {
        let _ = cfg;
        Err("lillith-admin only runs on Windows".into())
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::os::windows::io::{FromRawHandle, IntoRawHandle};
    use std::sync::{mpsc, Arc, Mutex};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
        WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };

    enum Msg {
        Run {
            id: u64,
            shell: winshell::WinShell,
            cwd: String,
            timeout_s: u64,
            cap: usize,
            command: String,
        },
        /// Pipe died or parent gone — main should exit.
        Bye,
    }

    pub fn serve(cfg: HelperArgs) -> Result<(), String> {
        let pipe = Arc::new(Mutex::new(connect_pipe(&cfg.pipe)?));
        let idle = Duration::from_secs(cfg.idle_min.saturating_mul(60).max(60));
        let parent = open_parent(cfg.parent_pid);
        let token = cfg.token.clone();
        let (tx, rx) = mpsc::channel::<Msg>();

        // Reader thread: always blocked in read_frame. Stop → ExitProcess so
        // an in-flight command on the main thread cannot delay the kill.
        let pipe_r = pipe.clone();
        let token_r = token.clone();
        std::thread::spawn(move || {
            loop {
                let frame = {
                    let mut p = pipe_r.lock().unwrap_or_else(|e| e.into_inner());
                    read_frame(&mut *p)
                };
                let frame = match frame {
                    Ok(f) => f,
                    Err(_) => {
                        let _ = tx.send(Msg::Bye);
                        break;
                    }
                };
                match Request::decode(&frame, &token_r) {
                    Ok(Request::Stop) => {
                        // Immediate: closes Job handles → KILL_ON_JOB_CLOSE
                        // wipes command children.
                        std::process::exit(0);
                    }
                    Ok(Request::Run {
                        id,
                        shell,
                        cwd,
                        timeout_s,
                        cap,
                        command,
                    }) => {
                        if tx
                            .send(Msg::Run {
                                id,
                                shell,
                                cwd,
                                timeout_s,
                                cap,
                                command,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(lillith_core::access::admin_proto::ProtoError::BadToken) => {
                        std::process::exit(1);
                    }
                    Err(_) => continue,
                }
            }
        });

        let mut last = Instant::now();
        loop {
            if parent_dead(parent) {
                break;
            }
            if last.elapsed() >= idle {
                break;
            }
            let msg = match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(m) => m,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            match msg {
                Msg::Bye => break,
                Msg::Run {
                    id,
                    shell,
                    cwd,
                    timeout_s,
                    cap,
                    command,
                } => {
                    last = Instant::now();
                    let cwd = (!cwd.is_empty()).then(|| Path::new(&cwd).to_path_buf());
                    let out = winshell::run(
                        shell,
                        &command,
                        cwd.as_deref(),
                        Duration::from_secs(timeout_s.max(1)),
                        cap.max(1),
                    );
                    let reply = match out {
                        Ok(o) => Reply::Ok { id, output: o },
                        Err(e) => Reply::Err {
                            id,
                            message: e.to_string(),
                        },
                    };
                    {
                        let mut p = pipe.lock().unwrap_or_else(|e| e.into_inner());
                        if write_frame(&mut *p, &reply.encode(&token)).is_err() {
                            break;
                        }
                    }
                    if cfg.per_command {
                        break;
                    }
                }
            }
        }
        if let Some(h) = parent {
            unsafe {
                CloseHandle(h);
            }
        }
        Ok(())
    }

    fn connect_pipe(name: &str) -> Result<std::fs::File, String> {
        let w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            unsafe {
                let h = CreateFileW(
                    w.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    std::ptr::null_mut(),
                );
                if h != INVALID_HANDLE_VALUE {
                    return Ok(std::fs::File::from_raw_handle(h as *mut _));
                }
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) {
                    let _ = WaitNamedPipeW(w.as_ptr(), 1000);
                } else if Instant::now() >= deadline {
                    return Err(format!("could not connect to {name}: {err}"));
                } else {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            if Instant::now() >= deadline {
                return Err(format!("timed out connecting to {name}"));
            }
        }
    }

    fn open_parent(pid: u32) -> Option<HANDLE> {
        unsafe {
            let h = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            );
            if h.is_null() {
                None
            } else {
                Some(h)
            }
        }
    }

    fn parent_dead(h: Option<HANDLE>) -> bool {
        let Some(h) = h else {
            return true;
        };
        unsafe { WaitForSingleObject(h, 0) == WAIT_OBJECT_0 }
    }

    #[allow(dead_code)]
    fn _link(_: WAIT_TIMEOUT, f: std::fs::File) {
        let _ = f.into_raw_handle();
    }
}
