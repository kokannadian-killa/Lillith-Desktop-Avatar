//! The real process-wide safeword (kill switch) stops a helper-worker
//! session. Its own test binary: `stop_all()` bumps a process-wide epoch.

use lillith_learn::workers::{Session, StopReason};
use std::time::Duration;

#[test]
fn kill_switch_and_chat_safewords_stop_worker_sessions() {
    let s = Session::new("s-1", Duration::from_secs(600));
    assert_eq!(s.stop_reason(), None);
    lillith_core::access::kill_switch::stop_all();
    assert_eq!(s.stop_reason(), Some(StopReason::Safeword));

    for word in ["red", "OOC: stop"] {
        let s = Session::new("s-2", Duration::from_secs(600));
        assert_eq!(s.stop_reason(), None, "a new session starts after the stop");
        assert!(s.observe_chat(word), "{word}");
        assert_eq!(s.stop_reason(), Some(StopReason::Safeword), "{word}");
    }
    let s = Session::new("s-3", Duration::from_secs(600));
    assert!(!s.observe_chat("reduce the heat"));
    assert_eq!(s.stop_reason(), None);
}
