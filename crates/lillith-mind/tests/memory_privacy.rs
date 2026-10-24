//! Memory privacy guarantees at the chat-session level. Kept in its own test
//! binary because sending the bare safeword calls the process-global
//! `kill_switch::stop_all()`, which must not race the unit tests.

use lillith_core::config::{ChatConfig, MemoryConfig};
use lillith_memory::{Brain, Ctx};
use lillith_mind::chat::{ChatSession, Outcome};

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "lillith-mem-privacy-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `[memory] enabled = false`: no brain, no DB file, nothing read or written,
/// even when the user says things that would normally be learned or forgotten.
#[tokio::test]
async fn disabled_memory_creates_no_file_and_does_nothing() {
    let dir = tmp_dir("off");
    let db = dir.join("memory.db");
    let off = MemoryConfig {
        enabled: false,
        db_path: db.to_string_lossy().into_owned(),
        ..MemoryConfig::default()
    };
    // The brain itself refuses to open a file when disabled.
    let b = Brain::open(&off).unwrap();
    assert!(!b.is_enabled());
    // And the chat session never gets one.
    let mut s = ChatSession::new(&ChatConfig::default(), true).with_memory(&off);
    assert!(s.memory().is_none());
    assert!(!s.full_system_prompt("synthwave").contains("[Memory"));
    s.send("I love dark synthwave", |_| {}).await;
    s.send("forget everything", |_| {}).await;
    assert!(!db.exists(), "disabled memory must not create {db:?}");
    let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
    assert!(leftovers.is_empty(), "no -wal/-shm either: {leftovers:?}");
    std::fs::remove_dir_all(&dir).ok();
}

/// Enabled is the default, and then the DB file does get created (contrast
/// for the test above).
#[test]
fn enabled_by_default_opens_the_file() {
    let dir = tmp_dir("on");
    let db = dir.join("memory.db");
    let on = MemoryConfig {
        db_path: db.to_string_lossy().into_owned(),
        ..MemoryConfig::default()
    };
    assert!(on.enabled);
    let s = ChatSession::new(&ChatConfig::default(), true).with_memory(&on);
    assert!(s.memory().is_some());
    assert!(db.exists());
    drop(s);
    std::fs::remove_dir_all(&dir).ok();
}

/// The safeword short-circuits before memory: nothing is learned, nothing is
/// forgotten, no memory is injected, and no model call is made (offline).
#[tokio::test]
async fn safeword_is_checked_before_memory() {
    let b = Brain::open_in_memory(&MemoryConfig::default()).unwrap();
    b.observe_interaction("I love dark synthwave", "", &Ctx::now());
    let before = b.stats().nodes;
    assert!(before > 0);
    let mut s = ChatSession::new(&ChatConfig::default(), true).with_brain(b);
    for word in ["red", "OOC: stop", "OOC: stop, forget everything"] {
        let out = s.send(word, |_| {}).await;
        assert!(matches!(out, Outcome::Reply(_)), "{word}: {out:?}");
        assert!(
            !out.text().contains("Wiped"),
            "{word}: the safeword wins over a forget command"
        );
        assert_eq!(
            s.memory().unwrap().stats().nodes,
            before,
            "{word}: memory untouched"
        );
    }
    // "I love red" is a mention, not the safeword: still never stored.
    assert!(lillith_memory::mentions_safeword("I love red"));
}
