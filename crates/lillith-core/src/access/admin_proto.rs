//! The wire protocol between Lillith (not elevated) and `lillith-admin`
//! (elevated through one UAC prompt), over a local named pipe
//! `\\.\pipe\lillith-admin-<random>`. No TCP, no network.
//!
//! - Frame: `u32` little-endian length, then that many bytes (at most
//!   [`MAX_FRAME`]).
//! - Payload: a list of fields, each a `u32` LE length plus UTF-8 bytes. The
//!   first two are always [`MAGIC`] and the session token. The helper drops
//!   the connection and exits on any frame whose token does not match the
//!   one on its command line (compared in constant time).
//! - Requests: `run <id> <shell> <cwd> <timeout_s> <cap> <command>` and
//!   `stop`. Replies: `ok <id> <output>` / `err <id> <message>`.
//!
//! The helper only runs what arrives here, and Lillith only sends a command
//! after Ko approved it on the Soul Contract card.

use std::io::{self, Read, Write};

use super::winshell::WinShell;

/// Protocol tag (first field of every payload).
pub const MAGIC: &str = "LILLITH-ADMIN/1";
/// Largest frame accepted either way (commands and capped output fit easily).
pub const MAX_FRAME: usize = 1 << 20;
/// Pipe names always start with this.
pub const PIPE_PREFIX: &str = r"\\.\pipe\lillith-admin-";

/// A request to the helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Run {
        id: u64,
        shell: WinShell,
        cwd: String,
        timeout_s: u64,
        cap: usize,
        command: String,
    },
    /// Exit now (safeword, app exit, per-command mode).
    Stop,
}

/// The helper's answer to one `Run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Ok { id: u64, output: String },
    Err { id: u64, message: String },
}

/// Why a frame was rejected.
#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("frame of {0} bytes is too big")]
    TooBig(usize),
    #[error("malformed frame: {0}")]
    Malformed(&'static str),
    #[error("wrong session token")]
    BadToken,
}

/// Writes one length-prefixed frame.
pub fn write_frame(w: &mut impl Write, payload: &[u8]) -> Result<(), ProtoError> {
    if payload.len() > MAX_FRAME {
        return Err(ProtoError::TooBig(payload.len()));
    }
    w.write_all(&(payload.len() as u32).to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()?;
    Ok(())
}

/// Reads one length-prefixed frame (refusing oversize lengths before
/// allocating).
pub fn read_frame(r: &mut impl Read) -> Result<Vec<u8>, ProtoError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_le_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err(ProtoError::TooBig(n));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

fn pack(fields: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for f in fields {
        out.extend_from_slice(&(f.len() as u32).to_le_bytes());
        out.extend_from_slice(f.as_bytes());
    }
    out
}

fn unpack(mut b: &[u8]) -> Result<Vec<String>, ProtoError> {
    let mut out = Vec::new();
    while !b.is_empty() {
        if b.len() < 4 {
            return Err(ProtoError::Malformed("truncated field length"));
        }
        let n = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
        b = &b[4..];
        if n > b.len() {
            return Err(ProtoError::Malformed("field runs past the frame"));
        }
        let s = std::str::from_utf8(&b[..n]).map_err(|_| ProtoError::Malformed("not UTF-8"))?;
        out.push(s.to_string());
        b = &b[n..];
    }
    Ok(out)
}

/// Constant-time string comparison (token checks).
pub fn token_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl Request {
    /// The payload for this request under `token`.
    pub fn encode(&self, token: &str) -> Vec<u8> {
        match self {
            Request::Run {
                id,
                shell,
                cwd,
                timeout_s,
                cap,
                command,
            } => pack(&[
                MAGIC,
                token,
                "run",
                &id.to_string(),
                shell.as_str(),
                cwd,
                &timeout_s.to_string(),
                &cap.to_string(),
                command,
            ]),
            Request::Stop => pack(&[MAGIC, token, "stop"]),
        }
    }

    /// Decodes a payload, checking the magic and that the token is `token`.
    pub fn decode(payload: &[u8], token: &str) -> Result<Self, ProtoError> {
        let f = unpack(payload)?;
        if f.first().map(String::as_str) != Some(MAGIC) {
            return Err(ProtoError::Malformed("bad magic"));
        }
        if !f.get(1).is_some_and(|t| token_eq(t, token)) || token.is_empty() {
            return Err(ProtoError::BadToken);
        }
        let num = |i: usize| -> Result<u64, ProtoError> {
            f.get(i)
                .and_then(|s| s.parse().ok())
                .ok_or(ProtoError::Malformed("bad number"))
        };
        match f.get(2).map(String::as_str) {
            Some("stop") if f.len() == 3 => Ok(Request::Stop),
            Some("run") if f.len() == 9 => Ok(Request::Run {
                id: num(3)?,
                shell: WinShell::parse(&f[4]).ok_or(ProtoError::Malformed("bad shell"))?,
                cwd: f[5].clone(),
                timeout_s: num(6)?,
                cap: num(7)? as usize,
                command: f[8].clone(),
            }),
            _ => Err(ProtoError::Malformed("unknown request")),
        }
    }
}

impl Reply {
    pub fn encode(&self, token: &str) -> Vec<u8> {
        match self {
            Reply::Ok { id, output } => pack(&[MAGIC, token, "ok", &id.to_string(), output]),
            Reply::Err { id, message } => pack(&[MAGIC, token, "err", &id.to_string(), message]),
        }
    }

    pub fn decode(payload: &[u8], token: &str) -> Result<Self, ProtoError> {
        let f = unpack(payload)?;
        if f.first().map(String::as_str) != Some(MAGIC) {
            return Err(ProtoError::Malformed("bad magic"));
        }
        if !f.get(1).is_some_and(|t| token_eq(t, token)) || token.is_empty() {
            return Err(ProtoError::BadToken);
        }
        if f.len() != 5 {
            return Err(ProtoError::Malformed("bad reply"));
        }
        let id = f[3].parse().map_err(|_| ProtoError::Malformed("bad id"))?;
        match f[2].as_str() {
            "ok" => Ok(Reply::Ok {
                id,
                output: f[4].clone(),
            }),
            "err" => Ok(Reply::Err {
                id,
                message: f[4].clone(),
            }),
            _ => Err(ProtoError::Malformed("unknown reply")),
        }
    }
}

/// Whether `name` is one of our pipe names (`PIPE_PREFIX` + 16-64 hex).
pub fn valid_pipe_name(name: &str) -> bool {
    name.strip_prefix(PIPE_PREFIX)
        .is_some_and(|r| (16..=64).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Whether `t` looks like a session token (64 hex chars).
pub fn valid_token(t: &str) -> bool {
    t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// How the helper was started (its command line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperArgs {
    pub pipe: String,
    pub token: String,
    /// Exit after this long without a request.
    pub idle_min: u64,
    /// Exit after the first command.
    pub per_command: bool,
    /// Lillith's pid: the helper exits when it is gone and only accepts a
    /// client with this pid.
    pub parent_pid: u32,
    /// String SID allowed on the pipe (Ko's account).
    pub allow_sid: String,
}

impl HelperArgs {
    /// The arguments Lillith passes (in order).
    pub fn to_args(&self) -> Vec<String> {
        vec![
            "--pipe".into(),
            self.pipe.clone(),
            "--token".into(),
            self.token.clone(),
            "--idle-min".into(),
            self.idle_min.to_string(),
            "--mode".into(),
            if self.per_command {
                "per-command"
            } else {
                "per-session"
            }
            .into(),
            "--parent-pid".into(),
            self.parent_pid.to_string(),
            "--allow-sid".into(),
            self.allow_sid.clone(),
        ]
    }

    /// Parses the helper's arguments (without argv[0]); every value is
    /// validated so a hand-started helper cannot be pointed anywhere odd.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut it = args.iter();
        let (mut pipe, mut token, mut idle, mut mode, mut ppid, mut sid) =
            (None, None, None, None, None, None);
        while let Some(k) = it.next() {
            let v = it.next().ok_or_else(|| format!("{k} needs a value"))?;
            match k.as_str() {
                "--pipe" => pipe = Some(v.clone()),
                "--token" => token = Some(v.clone()),
                "--idle-min" => idle = Some(v.parse::<u64>().map_err(|_| "bad --idle-min")?),
                "--mode" => mode = Some(v.clone()),
                "--parent-pid" => ppid = Some(v.parse::<u32>().map_err(|_| "bad --parent-pid")?),
                "--allow-sid" => sid = Some(v.clone()),
                _ => return Err(format!("unknown argument {k}")),
            }
        }
        let pipe = pipe.ok_or("missing --pipe")?;
        if !valid_pipe_name(&pipe) {
            return Err("bad --pipe name".into());
        }
        let token = token.ok_or("missing --token")?;
        if !valid_token(&token) {
            return Err("bad --token".into());
        }
        let sid = sid.ok_or("missing --allow-sid")?;
        if !(sid.starts_with("S-1-") && sid[4..].bytes().all(|b| b.is_ascii_digit() || b == b'-')) {
            return Err("bad --allow-sid".into());
        }
        let per_command = match mode.as_deref() {
            None | Some("per-session") => false,
            Some("per-command") => true,
            Some(_) => return Err("bad --mode".into()),
        };
        Ok(Self {
            pipe,
            token,
            idle_min: idle.unwrap_or(15).max(1),
            per_command,
            parent_pid: ppid.ok_or("missing --parent-pid")?,
            allow_sid: sid,
        })
    }
}

/// The pipe's security descriptor (SDDL): protected DACL granting full
/// access only to `sid` (Ko's account) — not Everyone, not the network
/// (`NU` is denied explicitly).
pub fn pipe_sddl(sid: &str) -> String {
    format!("D:P(D;;GA;;;NU)(A;;GA;;;{sid})")
}

/// `n` random bytes as lowercase hex (OS randomness).
pub fn random_hex(n: usize) -> io::Result<String> {
    let mut buf = vec![0u8; n];
    fill_random(&mut buf)?;
    let mut out = String::with_capacity(n * 2);
    for b in buf {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    Ok(out)
}

#[cfg(windows)]
fn fill_random(buf: &mut [u8]) -> io::Result<()> {
    #[link(name = "advapi32")]
    extern "system" {
        #[link_name = "SystemFunction036"]
        fn rtl_gen_random(buf: *mut u8, len: u32) -> u8;
    }
    // SAFETY: `buf` is valid for `len` writable bytes.
    let ok = unsafe { rtl_gen_random(buf.as_mut_ptr(), buf.len() as u32) };
    if ok == 0 {
        return Err(io::Error::other("RtlGenRandom failed"));
    }
    Ok(())
}

#[cfg(not(windows))]
fn fill_random(buf: &mut [u8]) -> io::Result<()> {
    std::fs::File::open("/dev/urandom")?.read_exact(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok() -> String {
        "ab".repeat(32)
    }

    #[test]
    fn frames_round_trip_and_reject_oversize() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello").unwrap();
        write_frame(&mut buf, b"").unwrap();
        let mut r = &buf[..];
        assert_eq!(read_frame(&mut r).unwrap(), b"hello");
        assert_eq!(read_frame(&mut r).unwrap(), b"");
        assert!(matches!(read_frame(&mut r), Err(ProtoError::Io(_))));
        let big = ((MAX_FRAME + 1) as u32).to_le_bytes();
        assert!(matches!(
            read_frame(&mut &big[..]),
            Err(ProtoError::TooBig(_))
        ));
        assert!(write_frame(&mut Vec::new(), &vec![0; MAX_FRAME + 1]).is_err());
        // Truncated body.
        let mut t = 10u32.to_le_bytes().to_vec();
        t.extend_from_slice(b"abc");
        assert!(read_frame(&mut &t[..]).is_err());
    }

    #[test]
    fn requests_round_trip_with_the_right_token_only() {
        let req = Request::Run {
            id: 7,
            shell: WinShell::PowerShell,
            cwd: r"C:\Users\Ko".into(),
            timeout_s: 30,
            cap: 16384,
            command: "Get-Service | ? Status -eq 'Running'\nüñí".into(),
        };
        let p = req.encode(&tok());
        assert_eq!(Request::decode(&p, &tok()).unwrap(), req);
        assert!(matches!(
            Request::decode(&p, &"cd".repeat(32)),
            Err(ProtoError::BadToken)
        ));
        assert!(matches!(Request::decode(&p, ""), Err(ProtoError::BadToken)));
        assert_eq!(
            Request::decode(&Request::Stop.encode(&tok()), &tok()).unwrap(),
            Request::Stop
        );
        // Garbage and truncation are rejected, not panics.
        assert!(Request::decode(b"\xff\xff\xff\xff", &tok()).is_err());
        assert!(Request::decode(&p[..p.len() - 3], &tok()).is_err());
        assert!(Request::decode(&pack(&["NOPE", &tok(), "stop"]), &tok()).is_err());
        assert!(Request::decode(&pack(&[MAGIC, &tok(), "rm"]), &tok()).is_err());
    }

    #[test]
    fn replies_round_trip() {
        for r in [
            Reply::Ok {
                id: 1,
                output: "done\n[exit 0]".into(),
            },
            Reply::Err {
                id: 2,
                message: "nope".into(),
            },
        ] {
            assert_eq!(Reply::decode(&r.encode(&tok()), &tok()).unwrap(), r);
            assert!(Reply::decode(&r.encode(&tok()), &"00".repeat(32)).is_err());
        }
    }

    #[test]
    fn token_compare_is_exact() {
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abc", "abd"));
        assert!(!token_eq("abc", "abcd"));
    }

    #[test]
    fn helper_args_round_trip_and_validate() {
        let a = HelperArgs {
            pipe: format!("{PIPE_PREFIX}{}", "0f".repeat(16)),
            token: tok(),
            idle_min: 15,
            per_command: true,
            parent_pid: 4242,
            allow_sid: "S-1-5-21-1-2-3-1001".into(),
        };
        assert_eq!(HelperArgs::parse(&a.to_args()).unwrap(), a);
        let bad = |k: &str, v: &str| {
            let mut args = a.to_args();
            let i = args.iter().position(|x| x == k).unwrap();
            args[i + 1] = v.into();
            HelperArgs::parse(&args).is_err()
        };
        assert!(bad("--pipe", r"\\.\pipe\other"));
        assert!(bad("--pipe", &format!("{PIPE_PREFIX}zz")));
        assert!(bad("--token", "short"));
        assert!(bad("--mode", "forever"));
        assert!(bad("--allow-sid", "S-1-5;(A;;GA;;;WD)"));
        assert!(HelperArgs::parse(&["--evil".into(), "1".into()]).is_err());
    }

    #[test]
    fn sddl_is_user_only_and_random_is_hex() {
        let s = pipe_sddl("S-1-5-21-9");
        assert!(s.starts_with("D:P") && s.contains("(A;;GA;;;S-1-5-21-9)"));
        assert!(!s.contains(";WD)") && s.contains("(D;;GA;;;NU)"));
        let a = random_hex(32).unwrap();
        assert!(valid_token(&a));
        assert_ne!(a, random_hex(32).unwrap());
    }
}
