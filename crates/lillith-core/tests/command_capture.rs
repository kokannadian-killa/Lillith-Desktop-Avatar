//! Regression tests for bounded command-output capture: output is capped
//! while it is read (not buffered whole and cut afterwards), truncation is
//! reported truthfully, UTF-8 stays valid, and timeouts / stops still work.

use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lillith_core::access::commands::{drain_capped, run_captured, CappedBuf};

// stop_all() is process-wide; serialize every test in this binary.
static GATE: Mutex<()> = Mutex::new(());

fn gate() -> std::sync::MutexGuard<'static, ()> {
    GATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A reader that yields `left` bytes of `b'a'` without ever allocating them.
struct Endless {
    left: u64,
}

impl Read for Endless {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = (buf.len() as u64).min(self.left) as usize;
        buf[..n].fill(b'a');
        self.left -= n as u64;
        Ok(n)
    }
}

#[test]
fn buffer_never_grows_past_the_cap() {
    let _g = gate();
    let total = 64 * 1024 * 1024; // 64 MiB through a 4 KiB cap
    let b = drain_capped(Endless { left: total }, 4096);
    assert_eq!(b.kept.len(), 4096);
    assert!(b.kept.capacity() <= 64 * 1024, "{}", b.kept.capacity());
    assert_eq!(b.total, total);
    assert_eq!(b.dropped(), total - 4096);

    let mut c = CappedBuf::new(3);
    c.push(b"ab");
    c.push(b"cdef");
    assert_eq!(
        (c.kept.as_slice(), c.total, c.dropped()),
        (&b"abc"[..], 6, 3)
    );
}

#[test]
fn huge_output_finishes_and_reports_exact_drop() {
    let _g = gate();
    let t = Instant::now();
    let out = run_captured(
        "head -c 5000000 /dev/zero | tr '\\0' a",
        None,
        Duration::from_secs(30),
        1000,
    )
    .unwrap();
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    assert!(out.ends_with("[exit 0]"), "{out}");
    assert!(out.contains("…[4999000 more bytes]"), "{out}");
    assert!(out.len() < 1100, "{}", out.len());
    assert_eq!(out.matches('a').count(), 1000);
}

#[test]
fn both_streams_flooding_do_not_deadlock() {
    let _g = gate();
    let out = run_captured(
        "head -c 3000000 /dev/zero >&2; head -c 3000000 /dev/zero; echo end >&2",
        None,
        Duration::from_secs(30),
        512,
    )
    .unwrap();
    assert!(out.ends_with("[exit 0]"), "{out}");
    assert!(
        out.contains(&format!("…[{} more bytes]", 6_000_004 - 512)),
        "{out}"
    );
}

#[test]
fn multibyte_cut_stays_valid_utf8() {
    let _g = gate();
    // "é\n" is 3 bytes; a 100-byte cap lands right after a lead byte.
    let out = run_captured(
        "yes \"$(printf '\\303\\251')\" | head -c 3000",
        None,
        Duration::from_secs(10),
        100,
    )
    .unwrap();
    assert!(!out.contains('\u{FFFD}'), "{out}");
    assert_eq!(out.matches('é').count(), 33, "{out}");
    assert!(out.contains("…[2901 more bytes]"), "{out}");
}

#[test]
fn small_output_is_unchanged() {
    let _g = gate();
    let out = run_captured(
        "printf 'hi ✓\\n'; printf 'warn\\n' >&2",
        None,
        Duration::from_secs(5),
        4096,
    )
    .unwrap();
    assert_eq!(out, "hi ✓\nwarn\n[exit 0]");
}

#[test]
fn timeout_still_fires_on_a_chatty_command() {
    let _g = gate();
    let t = Instant::now();
    let out = run_captured("yes", None, Duration::from_millis(500), 256).unwrap();
    assert!(out.contains("[timed out after 0s]"), "{out}");
    assert!(out.contains("more bytes]"), "{out}");
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
}

#[test]
fn hush_spares_and_stop_all_kills_a_chatty_command() {
    let _g = gate();
    use lillith_core::access::kill_switch;
    let t = std::thread::spawn(|| {
        run_captured(
            "yes | head -c 2000000; sleep 1; echo done",
            None,
            Duration::from_secs(30),
            64,
        )
    });
    std::thread::sleep(Duration::from_millis(300));
    kill_switch::hush();
    let out = t.join().unwrap().unwrap();
    assert!(out.ends_with("[exit 0]"), "{out}");

    let t = std::thread::spawn(|| run_captured("yes", None, Duration::from_secs(30), 64));
    std::thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    kill_switch::stop_all();
    let out = t.join().unwrap().unwrap();
    // Under fully scratch XDG dirs this binary joins a (scratch) stop
    // session, where a global stop also disarms dispatch until Re-arm. Re-arm
    // so the other tests here can still run commands (no-op when not joined).
    let _ = kill_switch::rearm();
    assert!(lillith_core::access::commands::was_stopped(&out), "{out}");
    assert!(out.contains("more bytes]"), "{out}");
    assert!(started.elapsed() < Duration::from_secs(5));
}
