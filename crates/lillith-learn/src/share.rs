//! "Share this page with Lillith" (P6, docs/SELF_LEARNING.md §3.1).
//!
//! Ko sends a page he is looking at over the local socket
//! `<runtime dir>/share.sock` (dir 0700, socket 0600, the peer must be the
//! same user: `SO_PEERCRED` / `getpeereid`). No TCP port, nothing is
//! fetched: the page's URL, title and visible text arrive in the request.
//! Browser profiles, history, sessions and caches are never read (the
//! socket is the only input; [`crate::paths::denied`] refuses those paths
//! for every other source too).
//!
//! Request: one JSON line `{"url": …, "title": …, "text": …}` (≤ 512 KiB).
//! Reply: one line, `ok <note id>` or `refused: <why>`.
//!
//! - Only `http(s)://` URLs; tracking parameters stripped; the URL is
//!   stored on the note (`source: { type: shared, ref: <url> }`, `url:`).
//! - Refused: hosts on `[learn] block_hosts` and the built-in denylist
//!   (banks and payment sites, password managers, webmail, login / auth /
//!   checkout pages).
//! - Injection text is dropped: every line the §7 ingest filter or the
//!   worker action filter rejects (role / instruction patterns, persona or
//!   safeword words, "run this" lines) is cut before anything is stored;
//!   a page with the safeword, or nothing left, is refused whole.
//! - Audit: `LEARN\tshare\t<host>\tnote=<id>` (no page text), and
//!   `LEARN\tshare-refused\t<host>\tnote=-`.
//!
//! The socket only runs with `[learn] share_socket = true` (off by default)
//! and learning + memory on.

use crate::online::url::Url;
use crate::store::{Note, NoteStore, WebSource};
use lillith_core::config::Config;
use std::path::{Path, PathBuf};

/// The socket's file name under the runtime dir.
pub const SOCKET_NAME: &str = "share.sock";
/// Largest request accepted (bytes, the JSON line).
pub const MAX_REQUEST: usize = 512 * 1024;
/// Topic shared pages are filed under.
pub const SHARE_TOPIC: &str = "web";
/// Confidence of a shared page note (Ko chose it, but it is still web text).
pub const SHARE_CONFIDENCE: f64 = 0.4;

/// Hosts never accepted (a part of the host name: `bank` matches
/// `mybank.example`). Banks and payments, password managers, webmail.
const DENY_HOST_WORDS: &[&str] = &[
    "bank",
    "paypal",
    "venmo",
    "interac",
    "1password",
    "bitwarden",
    "lastpass",
    "dashlane",
    "keepass",
    "nordpass",
];
/// Whole hosts never accepted (webmail; their subdomains too).
const DENY_HOSTS: &[&str] = &[
    "mail.google.com",
    "outlook.live.com",
    "outlook.office.com",
    "outlook.office365.com",
    "mail.proton.me",
    "mail.yahoo.com",
    "mail.aol.com",
    "mail.zoho.com",
    "fastmail.com",
    "tuta.com",
    "tutanota.com",
];
/// Host labels for login / auth front doors (`accounts.example.org`).
const DENY_HOST_LABELS: &[&str] = &["accounts", "login", "signin", "auth", "sso", "id", "idp"];
/// Path segments of login / auth / payment pages.
const DENY_PATH_WORDS: &[&str] = &[
    "login",
    "log-in",
    "signin",
    "sign-in",
    "signup",
    "sign-up",
    "logon",
    "auth",
    "oauth",
    "oauth2",
    "sso",
    "saml",
    "password",
    "reset-password",
    "2fa",
    "mfa",
    "checkout",
    "payment",
    "billing",
    "compose",
];

/// One page Ko shared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedPage {
    pub url: String,
    pub title: String,
    pub text: String,
}

impl SharedPage {
    /// Parses the request line.
    pub fn parse(line: &[u8]) -> Result<Self, String> {
        if line.len() > MAX_REQUEST {
            return Err(format!("over {} KiB", MAX_REQUEST / 1024));
        }
        let v: serde_json::Value =
            serde_json::from_slice(line).map_err(|_| "not a share request".to_string())?;
        let s = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string()
        };
        Ok(Self {
            url: s("url"),
            title: s("title"),
            text: s("text"),
        })
    }

    /// The request line for this page (clients, tests).
    pub fn to_line(&self) -> String {
        let mut l = serde_json::json!({
            "url": self.url,
            "title": self.title,
            "text": self.text,
        })
        .to_string();
        l.push('\n');
        l
    }
}

/// What the socket checks before anything is stored.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub block_hosts: Vec<String>,
}

impl Policy {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            block_hosts: cfg
                .learn
                .block_hosts
                .iter()
                .map(|h| h.trim().trim_start_matches("*.").to_ascii_lowercase())
                .filter(|h| !h.is_empty())
                .collect(),
        }
    }

    /// The cleaned URL, or why it is refused.
    pub fn vet(&self, url: &str) -> Result<Url, String> {
        let u = Url::parse(url)
            .map_err(|_| "only http:// and https:// pages can be shared".to_string())?
            .strip_tracking();
        let host = u.host().to_ascii_lowercase();
        let under = |b: &str| host == b || host.ends_with(&format!(".{b}"));
        if self.block_hosts.iter().any(|b| under(b)) {
            return Err(format!("{host} is on [learn] block_hosts"));
        }
        if let Some(why) = denied_page(&host, u.path_and_query()) {
            return Err(format!("{host}: {why} pages are never kept"));
        }
        Ok(u)
    }
}

/// The built-in denylist's reason for a page, if any.
pub fn denied_page(host: &str, path_and_query: &str) -> Option<&'static str> {
    let host = host.to_ascii_lowercase();
    if DENY_HOSTS
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    {
        return Some("webmail");
    }
    if DENY_HOST_WORDS.iter().any(|w| host.contains(w)) {
        return Some("banking, payment or password-manager");
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() > 2 && DENY_HOST_LABELS.contains(&labels[0]) {
        return Some("login");
    }
    let path = path_and_query
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if path
        .split(['/', '.', '_'])
        .any(|seg| DENY_PATH_WORDS.contains(&seg))
    {
        return Some("login, account or payment");
    }
    None
}

/// `text` without the lines the ingest filters reject, and how many were
/// cut. Blank-line runs collapse to one.
pub fn drop_injection(text: &str) -> (String, usize) {
    let mut out = String::new();
    let mut cut = 0;
    let mut blank = true;
    for line in text.lines() {
        let l = line.trim_end();
        if l.trim().is_empty() {
            if !blank {
                out.push('\n');
            }
            blank = true;
            continue;
        }
        let item = crate::extract::Item {
            kind: "web".into(),
            title: String::new(),
            body: l.to_string(),
            links: Vec::new(),
        };
        if !crate::extract::allowed(&item)
            || crate::workers::validate::asks_for_action(l).is_some()
            || lillith_memory::mentions_safeword(l)
        {
            cut += 1;
            continue;
        }
        out.push_str(l);
        out.push('\n');
        blank = false;
    }
    (out.trim().to_string(), cut)
}

fn cap(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// What happened to one shared page.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Saved {
        id: String,
        rel: String,
        url: String,
        cut_lines: usize,
    },
    Refused(String),
}

/// Takes one page: vets it, cuts injection text and stores the note (the
/// knowledge lock held). Audited either way.
pub fn accept(
    cfg: &Config,
    policy: &Policy,
    page: &SharedPage,
    audit_log: Option<&Path>,
) -> Outcome {
    let host_of = |u: &str| {
        Url::parse(u)
            .map(|u| u.host().to_string())
            .unwrap_or_else(|_| "-".into())
    };
    let refuse = |why: String| {
        crate::audit::record(audit_log, "share-refused", &host_of(&page.url), "-");
        Outcome::Refused(why)
    };
    let url = match policy.vet(&page.url) {
        Ok(u) => u,
        Err(e) => return refuse(e),
    };
    if lillith_memory::mentions_safeword(&page.title) || lillith_memory::is_safeword(&page.text) {
        return refuse("text with the safeword is never stored".into());
    }
    let (body, cut) = drop_injection(&crate::paths::drop_secret_lines(&page.text));
    let body = cap(&body, crate::MAX_NOTE_BYTES);
    let title: String = {
        let t = page.title.trim();
        let t = if t.is_empty() || !title_ok(t) {
            format!("{}{}", url.host(), url.path_and_query())
        } else {
            t.to_string()
        };
        t.chars().take(120).collect()
    };
    if body.trim().is_empty() {
        return refuse("no text left to keep".into());
    }
    let whole = crate::extract::Item {
        kind: "web".into(),
        title: title.clone(),
        body: body.clone(),
        links: Vec::new(),
    };
    if !crate::extract::allowed(&whole) {
        return refuse("dropped by the ingest filter".into());
    }
    let Some(dir) = cfg.mind.resolved_knowledge_dir() else {
        return refuse("no knowledge folder".into());
    };
    let mut topics = cfg.learn.topics.clone();
    if !topics.iter().any(|t| t == SHARE_TOPIC) {
        topics.push(SHARE_TOPIC.into());
    }
    let store = match NoteStore::open(&dir, &topics, false) {
        Ok(s) => s,
        Err(e) => return refuse(format!("knowledge folder: {e}")),
    };
    let lock = match store.lock() {
        Ok(Some(l)) => l,
        Ok(None) => return refuse("the learner is writing; share it again in a moment".into()),
        Err(e) => return refuse(format!("lock: {e}")),
    };
    let now = crate::now_s();
    let u = url.to_string();
    let note = Note {
        id: crate::learner::new_id(&u, &title, now),
        title,
        topic: SHARE_TOPIC.into(),
        kind: "web".into(),
        source_type: "shared".into(),
        source_ref: u.clone(),
        source_hash: crate::fnv1a(page.text.as_bytes()),
        created: now,
        updated: now,
        confidence: SHARE_CONFIDENCE,
        quiz_asked: 0,
        quiz_passed: 0,
        links: Vec::new(),
        origin: "learned".into(),
        body,
        web: Some(WebSource {
            url: u.clone(),
            trip_id: String::new(),
        }),
    };
    match store.write(&lock, &note) {
        Ok(path) => {
            crate::audit::record(audit_log, "share", url.host(), &note.id);
            Outcome::Saved {
                id: note.id,
                rel: store.rel(&path),
                url: u,
                cut_lines: cut,
            }
        }
        Err(e) => refuse(format!("write: {e}")),
    }
}

fn title_ok(t: &str) -> bool {
    let item = crate::extract::Item {
        kind: "web".into(),
        title: t.to_string(),
        body: String::new(),
        links: Vec::new(),
    };
    crate::extract::allowed(&item)
        && crate::workers::validate::asks_for_action(t).is_none()
        && !lillith_memory::mentions_safeword(t)
}

/// `<runtime dir>/share.sock`.
pub fn socket_path() -> Option<PathBuf> {
    lillith_core::llm_gate::runtime_dir().map(|d| d.join(SOCKET_NAME))
}

/// Whether the socket should run for `cfg`.
pub fn should_serve(cfg: &Config) -> bool {
    crate::should_run(cfg) && cfg.learn.share_socket
}

#[cfg(unix)]
pub use unix::{send, ShareServer};

#[cfg(unix)]
mod unix {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// The peer's uid.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn peer_uid(s: &UnixStream) -> Option<u32> {
        use std::os::fd::AsRawFd;
        let mut cred = libc::ucred {
            pid: 0,
            uid: u32::MAX,
            gid: u32::MAX,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: `cred` and `len` are valid for writes of their sizes and
        // the fd is open for the call.
        let r = unsafe {
            libc::getsockopt(
                s.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        (r == 0).then_some(cred.uid)
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    fn peer_uid(s: &UnixStream) -> Option<u32> {
        use std::os::fd::AsRawFd;
        let (mut uid, mut gid) = (u32::MAX, u32::MAX);
        // SAFETY: valid out pointers, open fd.
        let r = unsafe { libc::getpeereid(s.as_raw_fd(), &mut uid, &mut gid) };
        (r == 0).then_some(uid)
    }

    fn my_uid() -> u32 {
        // SAFETY: getuid has no preconditions.
        unsafe { libc::getuid() }
    }

    /// The running share socket. Dropping it stops the thread and removes
    /// the socket file.
    #[derive(Debug)]
    pub struct ShareServer {
        path: PathBuf,
        quit: Arc<AtomicBool>,
        join: Option<std::thread::JoinHandle<()>>,
    }

    impl ShareServer {
        /// Starts the socket for `cfg` ([`should_serve`]); `None`: off or
        /// it could not bind (logged).
        pub fn spawn(cfg: &Config) -> Option<Self> {
            if !should_serve(cfg) {
                return None;
            }
            let path = socket_path()?;
            let audit = lillith_core::config::audit_log_path();
            Self::spawn_at(cfg, &path, Policy::from_config(cfg), audit)
                .map_err(|e| tracing::warn!("share socket: {e}"))
                .ok()
        }

        /// [`spawn`](Self::spawn) at `path` with an explicit policy and
        /// audit log (tests).
        pub fn spawn_at(
            cfg: &Config,
            path: &Path,
            policy: Policy,
            audit_log: Option<PathBuf>,
        ) -> std::io::Result<Self> {
            if let Some(d) = path.parent() {
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(d)?;
                std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))?;
            }
            // A socket left by a crashed run (only a socket is replaced).
            if let Ok(m) = std::fs::symlink_metadata(path) {
                use std::os::unix::fs::FileTypeExt;
                if m.file_type().is_socket() {
                    std::fs::remove_file(path)?;
                } else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        format!("{} exists and is not a socket", path.display()),
                    ));
                }
            }
            let l = UnixListener::bind(path)?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            l.set_nonblocking(true)?;
            let quit = Arc::new(AtomicBool::new(false));
            let q = quit.clone();
            let cfg = cfg.clone();
            let join = std::thread::Builder::new()
                .name("lillith-share".into())
                .spawn(move || serve(l, &cfg, &policy, audit_log.as_deref(), &q))?;
            tracing::info!(path = %path.display(), "share socket ready");
            Ok(Self {
                path: path.to_path_buf(),
                quit,
                join: Some(join),
            })
        }

        pub fn path(&self) -> &Path {
            &self.path
        }

        /// Stops the thread and removes the socket.
        pub fn shutdown(&mut self) {
            self.quit.store(true, Ordering::SeqCst);
            if let Some(j) = self.join.take() {
                let _ = j.join();
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    impl Drop for ShareServer {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    fn serve(
        l: UnixListener,
        cfg: &Config,
        policy: &Policy,
        audit_log: Option<&Path>,
        quit: &AtomicBool,
    ) {
        while !quit.load(Ordering::SeqCst) {
            match l.accept() {
                Ok((s, _)) => {
                    if let Err(e) = handle(s, cfg, policy, audit_log) {
                        tracing::debug!("share: {e}");
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    tracing::warn!("share socket: {e}");
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
    }

    fn handle(
        s: UnixStream,
        cfg: &Config,
        policy: &Policy,
        audit_log: Option<&Path>,
    ) -> std::io::Result<()> {
        s.set_nonblocking(false)?;
        s.set_read_timeout(Some(Duration::from_secs(2)))?;
        s.set_write_timeout(Some(Duration::from_secs(2)))?;
        let mut w = s.try_clone()?;
        if peer_uid(&s) != Some(my_uid()) {
            tracing::warn!("share: refused a peer of another user");
            return w.write_all(b"refused: not the same user\n");
        }
        let mut line = Vec::new();
        BufReader::new((&s).take(MAX_REQUEST as u64 + 1)).read_until(b'\n', &mut line)?;
        let reply = match SharedPage::parse(&line) {
            Ok(p) => match accept(cfg, policy, &p, audit_log) {
                Outcome::Saved { id, .. } => format!("ok {id}\n"),
                Outcome::Refused(why) => format!("refused: {why}\n"),
            },
            Err(e) => format!("refused: {e}\n"),
        };
        w.write_all(reply.as_bytes())
    }

    /// Sends `page` to the socket at `path`; the reply line.
    pub fn send(path: &Path, page: &SharedPage) -> std::io::Result<String> {
        let mut s = UnixStream::connect(path)?;
        s.set_read_timeout(Some(Duration::from_secs(5)))?;
        s.write_all(page.to_line().as_bytes())?;
        let mut r = String::new();
        BufReader::new(s).read_line(&mut r)?;
        Ok(r.trim_end().to_string())
    }
}

/// No share socket off Unix yet (Windows would need a named pipe with a
/// same-user ACL); [`ShareServer::spawn`] says so and starts nothing.
#[cfg(not(unix))]
#[derive(Debug)]
pub struct ShareServer;

#[cfg(not(unix))]
impl ShareServer {
    pub fn spawn(cfg: &Config) -> Option<Self> {
        if should_serve(cfg) {
            tracing::info!("share socket: not available on this platform yet");
        }
        None
    }
}

#[cfg(test)]
#[path = "share_tests.rs"]
mod tests;
