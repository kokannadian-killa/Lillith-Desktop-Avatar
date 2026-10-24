//! Test driver for `tools/lillith-voice/tests/close_test.py`: the real
//! `voice_mic::Mic` (the chat bubble's talk button) against a real voice
//! daemon, driven over stdin.
//!
//! stdin: `toggle` | `hush` | `stop` | `drop` | `state` | `quit`
//! (`quit` returns from `main`, so a live `Mic` is dropped on the way out).
//! stdout: `<ms> KIND detail` lines. Needs `LILLITH_VOICE_SOCKET`.
use lillith_panel::voice_mic::Mic;
use lillith_voice::daemon::DaemonClient;
use std::io::{BufRead, Write};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

fn main() {
    let sock = std::env::var("LILLITH_VOICE_SOCKET").expect("LILLITH_VOICE_SOCKET");
    let client = DaemonClient::new(sock).expect("client");
    let t0 = Instant::now();
    let ms = move || t0.elapsed().as_millis();
    let out = |s: String| {
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, "{s}");
        let _ = o.flush();
    };
    let mut mic = Some(Mic::spawn(client, Duration::from_secs(30), || {}));
    let (tx, rx) = channel::<String>();
    std::thread::spawn(move || {
        for l in std::io::stdin().lock().lines() {
            let Ok(l) = l else { break };
            if tx.send(l.trim().to_string()).is_err() {
                break;
            }
        }
    });
    let mut last = None;
    loop {
        if let Some(m) = mic.as_mut() {
            for e in m.poll() {
                out(format!("{} EV {e:?}", ms()));
            }
            let cur = (m.ready, m.state, m.can_start());
            if last != Some(cur) {
                out(format!(
                    "{} STATE ready={} state={:?} can_start={}",
                    ms(),
                    cur.0,
                    cur.1,
                    cur.2
                ));
                last = Some(cur);
            }
        }
        while let Ok(c) = rx.try_recv() {
            match (c.as_str(), mic.as_mut()) {
                ("toggle", Some(m)) => {
                    let r = m.toggle();
                    out(format!("{} TOGGLE started={r} state={:?}", ms(), m.state));
                }
                ("hush", Some(m)) => {
                    m.hush("safeword");
                    out(format!("{} HUSH state={:?}", ms(), m.state));
                }
                ("stop", Some(m)) => {
                    m.stop();
                    out(format!("{} STOP state={:?}", ms(), m.state));
                }
                ("state", Some(m)) => {
                    out(format!("{} STATE? {m:?} can_start={}", ms(), m.can_start()));
                }
                ("drop", _) => {
                    let t = Instant::now();
                    mic = None;
                    out(format!("{} DROPPED in {}ms", ms(), t.elapsed().as_millis()));
                }
                ("quit", _) => {
                    out(format!("{} QUIT", ms()));
                    return;
                }
                _ => out(format!("{} ?? {c}", ms())),
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
