//! Helper-worker sessions come only from a clicked dream-session card, and
//! every stop works on its own: the process-wide kill switch, End early and
//! the plain stop. The safeword ("red", "OOC: stop") is not a stop: it only
//! hushes her voice (Ko, 2026-10-24: "Just the voice"). Its own test
//! binary: `stop_all()` bumps a process-wide epoch, so the tests here take
//! one lock and every session is made after the previous kill switch.

use lillith_core::card_process::CardAnswer;
use lillith_core::config::Config;
use lillith_learn::workers::{
    propose_session, DreamRefused, Session, SessionAnswer, SessionCardRequest, SessionConsent,
    StopReason,
};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let d = std::env::temp_dir().join(format!(
            "lillith-workers-dream-{name}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
    fn log(&self) -> PathBuf {
        self.0.join("audit.log")
    }
    fn audit(&self) -> String {
        std::fs::read_to_string(self.log()).unwrap_or_default()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn on() -> Config {
    let mut c = Config::default();
    c.learn.online = true;
    c
}

/// A card UI that answers with a fixed answer (and can run a hook while
/// open, e.g. fire the kill switch).
struct Card {
    answer: SessionAnswer,
    hook: Box<dyn Fn()>,
    seen: RefCell<Vec<SessionCardRequest>>,
}

impl Card {
    fn new(answer: SessionAnswer) -> Self {
        Self {
            answer,
            hook: Box::new(|| {}),
            seen: RefCell::default(),
        }
    }
    fn click(external_ticked: bool) -> Self {
        Self::new(SessionAnswer::Click {
            answer: CardAnswer::Yes,
            external_ticked,
        })
    }
}

impl SessionConsent for Card {
    fn ask(&self, req: &SessionCardRequest, stop: &dyn Fn() -> bool) -> SessionAnswer {
        self.seen.borrow_mut().push(req.clone());
        (self.hook)();
        if stop() {
            return SessionAnswer::Click {
                answer: CardAnswer::Cancelled,
                external_ticked: false,
            };
        }
        self.answer.clone()
    }
}

fn clicked(d: &Dir) -> Session {
    let a = propose_session(&on(), 60, &Card::click(false), Some(d.log())).unwrap();
    Session::from_card(a).unwrap()
}

#[test]
fn a_session_exists_only_after_a_click_with_its_lines_first() {
    let _g = lock();
    let d = Dir::new("click");
    // The online gate first: nothing shown, nothing written.
    let c = Card::click(false);
    assert!(matches!(
        propose_session(&Config::default(), 60, &c, Some(d.log())),
        Err(DreamRefused::Off(_))
    ));
    assert!(c.seen.borrow().is_empty() && d.audit().is_empty());
    // 30 min–8 h, never clamped.
    for m in [29, 481, 0] {
        let c = Card::click(false);
        assert!(
            matches!(
                propose_session(&on(), m, &c, Some(d.log())),
                Err(DreamRefused::Duration { .. })
            ),
            "{m}"
        );
        assert!(c.seen.borrow().is_empty());
    }
    // Typed / spoken yes and every non-Yes click: no session.
    for ans in [
        SessionAnswer::Typed("yes".into()),
        SessionAnswer::Spoken("yes".into()),
        SessionAnswer::Click {
            answer: CardAnswer::No,
            external_ticked: true,
        },
        SessionAnswer::Click {
            answer: CardAnswer::TimedOut,
            external_ticked: false,
        },
        SessionAnswer::Click {
            answer: CardAnswer::NoUi,
            external_ticked: false,
        },
    ] {
        let r = propose_session(&on(), 60, &Card::new(ans.clone()), Some(d.log()));
        assert!(matches!(r, Err(DreamRefused::NotApproved(_))), "{ans:?}");
    }
    // The click.
    let c = Card::click(false);
    let a = propose_session(&on(), 30, &c, Some(d.log())).unwrap();
    let req = c.seen.borrow()[0].clone();
    assert_eq!(req.text.title, "START DREAM SESSION");
    assert!(req.text.detail.contains("End early"), "{}", req.text.detail);
    // The card shows the end in local time with a zone label; the request
    // field, the action and the audit keep the UTC stamp.
    assert!(req.deadline_utc.ends_with('Z'), "{}", req.deadline_utc);
    let local = lillith_core::localtime::label_for_utc_stamp(&req.deadline_utc).unwrap();
    assert!(
        req.text.summary.ends_with(&format!("ends {local}")),
        "{}",
        req.text.summary
    );
    assert!(
        req.text
            .detail
            .contains(&format!("ends: {local} ({})", req.deadline_utc)),
        "{}",
        req.text.detail
    );
    assert!(!req.external_offered);
    let card = a.card().to_string();
    let s = Session::from_card(a).unwrap();
    assert_eq!(s.card(), card);
    assert_eq!(s.stop_reason(), None);
    assert!(!s.external_allowed());
    let log = d.audit();
    let at = |needle: &str| {
        log.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing: {log}"))
    };
    let proposed = at(&format!("\tPROPOSED\tdream_session\tcard={card}"));
    let confirm = at(&format!("\tCONFIRM\tdream_session\tcard={card}"));
    let start = at("\tDREAM\tstart\t");
    assert!(proposed < confirm && confirm < start, "{log}");
    assert!(log.contains("\tDENY\tdream_session\t") && log.contains("\tNO_UI\tdream_session\t"));
    s.end_early();
    s.finish().unwrap();
    assert!(d
        .audit()
        .contains(&format!("\tDREAM\tend\tend_early\tcard={card}")));
}

#[test]
fn an_unwritable_audit_log_starts_nothing() {
    let _g = lock();
    let d = Dir::new("nolog");
    let c = Card::click(false);
    assert!(matches!(
        propose_session(&on(), 60, &c, None),
        Err(DreamRefused::Audit(_))
    ));
    assert!(c.seen.borrow().is_empty(), "no card without PROPOSED");
    // PROPOSED and CONFIRM fine, then the log breaks before the start line.
    let a = propose_session(&on(), 60, &Card::click(false), Some(d.log())).unwrap();
    std::fs::remove_file(d.log()).unwrap();
    std::fs::create_dir_all(d.log()).unwrap();
    assert!(matches!(Session::from_card(a), Err(DreamRefused::Audit(_))));
}

#[test]
fn the_external_tick_comes_from_the_card_only_when_offered() {
    let _g = lock();
    let d = Dir::new("ext");
    // Not offered: a tick is ignored.
    let a = propose_session(&on(), 60, &Card::click(true), Some(d.log())).unwrap();
    assert!(!a.external());
    assert!(!Session::from_card(a).unwrap().external_allowed());
    // Offered and unticked (the default): off.
    let mut cfg = on();
    cfg.learn.dream.external_bots = true;
    let c = Card::click(false);
    let a = propose_session(&cfg, 60, &c, Some(d.log())).unwrap();
    assert!(c.seen.borrow()[0].external_offered);
    assert!(!a.external());
    // Offered and ticked: on, until any stop (the safeword is not one).
    let a = propose_session(&cfg, 60, &Card::click(true), Some(d.log())).unwrap();
    assert!(a.external());
    let s = Session::from_card(a).unwrap();
    assert!(s.external_allowed());
    assert!(s.observe_chat("red"));
    assert!(s.external_allowed(), "the safeword only hushes her voice");
    s.stop();
    assert!(!s.external_allowed(), "a stop turns external off");
}

#[test]
fn end_early_works_on_its_own() {
    let _g = lock();
    let d = Dir::new("end");
    let s = clicked(&d);
    s.end_early();
    assert_eq!(s.stop_reason(), Some(StopReason::EndEarly));
    // No safeword, no kill switch involved: a fresh session is unaffected.
    assert_eq!(clicked(&d).stop_reason(), None);
}

#[test]
fn the_plain_stop_works_on_its_own() {
    let _g = lock();
    let d = Dir::new("stop");
    let s = clicked(&d);
    s.stop();
    assert_eq!(s.stop_reason(), Some(StopReason::Stop));
    assert_eq!(clicked(&d).stop_reason(), None);
}

#[test]
fn the_safeword_hushes_her_voice_but_stops_no_session() {
    use lillith_core::access::kill_switch;
    let _g = lock();
    let d = Dir::new("sw");
    for word in ["red", "OOC: stop"] {
        assert!(kill_switch::is_safeword(word));
        let s = clicked(&d);
        assert_eq!(s.stop_reason(), None);
        // What the chat layer does on the safeword: hush (her talking and
        // queued lines stop) — never stop_all.
        let (hush0, kill0) = (kill_switch::hush_epoch(), kill_switch::epoch());
        assert!(s.observe_chat(word), "{word}");
        kill_switch::hush();
        assert!(
            kill_switch::hushed_since(hush0),
            "{word}: her voice is hushed"
        );
        assert!(!kill_switch::stopped_since(kill0), "{word}: nothing killed");
        assert_eq!(s.stop_reason(), None, "{word}: the session runs on");
        // A dream card opened right after the safeword still works.
        assert_eq!(clicked(&d).stop_reason(), None);
        // The hard stops still work on that same session.
        s.end_early();
        assert_eq!(s.stop_reason(), Some(StopReason::EndEarly), "{word}");
    }
    let s = clicked(&d);
    assert!(!s.observe_chat("reduce the heat"));
    assert_eq!(s.stop_reason(), None);
    assert!(!d.audit().contains("\tDREAM\tend\tsafeword"));
}

#[test]
fn the_kill_switch_works_on_its_own() {
    let _g = lock();
    let d = Dir::new("ks");
    // A running session: reported as the kill switch.
    let s = clicked(&d);
    assert_eq!(s.stop_reason(), None);
    lillith_core::access::kill_switch::stop_all();
    assert_eq!(s.stop_reason(), Some(StopReason::KillSwitch));
    // An approval clicked before the kill switch can't start afterwards.
    let a = propose_session(&on(), 60, &Card::click(false), Some(d.log())).unwrap();
    lillith_core::access::kill_switch::stop_all();
    let s = Session::from_card(a).unwrap();
    assert_eq!(s.stop_reason(), Some(StopReason::KillSwitch));
    // Fired while the card is open: the card closes, no approval.
    let mut c = Card::click(false);
    c.hook = Box::new(|| {
        lillith_core::access::kill_switch::stop_all();
    });
    assert_eq!(
        propose_session(&on(), 60, &c, Some(d.log())).unwrap_err(),
        DreamRefused::Stopped(StopReason::KillSwitch)
    );
    assert!(d.audit().contains("\tCANCELLED\tdream_session\t"));
    // A session clicked after the kill switch runs normally.
    assert_eq!(clicked(&d).stop_reason(), None);
}

#[test]
fn the_card_end_time_and_the_session_deadline_share_one_reference() {
    // Review N3-1: the deadline is fixed when the card appears (the time it
    // shows), so a slow click can't push the real end past the shown one.
    let _g = lock();
    let d = Dir::new("deadline");
    let mut c = Card::click(false);
    c.hook = Box::new(|| std::thread::sleep(std::time::Duration::from_millis(1200)));
    let a = propose_session(&on(), 30, &c, Some(d.log())).unwrap();
    let shown = c.seen.borrow()[0].deadline_utc.clone();
    assert_eq!(a.deadline_utc(), shown, "the approval keeps the card's end");
    assert!(d.audit().contains(&format!("deadline={shown}")));
    let s = Session::from_card(a).unwrap();
    let left = s.remaining();
    let len = std::time::Duration::from_secs(30 * 60);
    assert!(
        left <= len - std::time::Duration::from_millis(1100),
        "the slow click came out of the session, not past the shown end: {left:?}"
    );
    assert!(left > len - std::time::Duration::from_secs(30), "{left:?}");
}
