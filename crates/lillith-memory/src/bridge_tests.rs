use super::*;
use std::sync::mpsc::channel;

const DAY: f64 = 86_400.0;
/// 2026-10-19 00:00:00 UTC, a Monday.
const MON: f64 = 1_792_368_000.0;

fn focus(class: &str) -> Event {
    Event::with_detail("window.focused", class)
}

fn pred(subject: &str, confidence: f64) -> Prediction {
    Prediction {
        kind: "app".into(),
        subject: subject.into(),
        confidence,
        support: 4.0,
    }
}

fn calm(now: f64) -> Moment {
    Moment {
        now,
        safeword: false,
        busy: false,
    }
}

#[test]
fn app_names_are_sanitized_to_a_short_id() {
    assert_eq!(app_name("org.kde.konsole").as_deref(), Some("konsole"));
    assert_eq!(app_name("Firefox").as_deref(), Some("firefox"));
    assert_eq!(app_name("C:\\Games\\Steam.exe").as_deref(), Some("steam"));
    assert_eq!(app_name("My secret doc - Writer"), None);
    assert_eq!(app_name("  "), None);
    assert_eq!(app_name(&"x".repeat(80)).map(|s| s.len()), Some(32));
}

#[test]
fn focus_events_keep_only_the_app_name_and_debounce() {
    let mut f = EventFilter::new(20);
    let e = f.admit(&focus("org.mozilla.firefox"), MON).unwrap();
    assert_eq!(e.name, "window.opened.app");
    assert_eq!(e.detail.as_deref(), Some("firefox"));
    // Alt-tabbing back within 10 minutes is the same use.
    assert!(f.admit(&focus("firefox"), MON + 5.0).is_none());
    assert!(f.admit(&focus("konsole"), MON + 6.0).is_some());
    assert!(f.admit(&focus("firefox"), MON + 300.0).is_none());
    assert!(f
        .admit(&focus("firefox"), MON + FOCUS_REPEAT_S + 1.0)
        .is_some());
}

#[test]
fn private_and_captioned_events_are_never_admitted() {
    let mut f = EventFilter::new(20);
    // window.opened carries the window caption: never stored.
    assert!(f
        .admit(
            &Event::with_detail("window.opened", "Bank — statement.pdf"),
            MON
        )
        .is_none());
    assert!(f.admit(&Event::new("window.private"), MON).is_none());
    assert!(f
        .admit(&Event::with_detail("notify.message", "hi from Sam"), MON)
        .is_none());
    assert!(f.admit(&focus("red"), MON).is_none(), "safeword");
    assert!(f
        .admit(&Event::with_detail("chat.error", "llm down"), MON)
        .is_none());
}

#[test]
fn errors_are_reduced_to_kind_and_app_and_deduped() {
    let mut f = EventFilter::new(20);
    let e = f
        .admit(
            &Event::with_detail("app.crash", "steam segfault at /home/ko/x"),
            MON,
        )
        .unwrap();
    assert_eq!(e.name, "app.crash");
    assert_eq!(e.detail.as_deref(), Some("app.crash: steam"));
    assert!(f
        .admit(&Event::with_detail("app.crash", "steam again"), MON + 10.0)
        .is_none());
    let e = f.admit(&Event::new("journal.error"), MON + 11.0).unwrap();
    assert_eq!(e.detail.as_deref(), Some("journal.error"));
    assert!(f
        .admit(
            &Event::with_detail("app.crash", "steam"),
            MON + ERROR_REPEAT_S + 1.0
        )
        .is_some());
}

#[test]
fn quantumsec_events_are_admitted() {
    let mut f = EventFilter::new(20);
    let e = f
        .admit(
            &Event::with_detail("quantumsec.status.elevated", "cpu warm"),
            MON,
        )
        .unwrap();
    assert_eq!(e.name, "quantumsec.status.elevated");
    // Debounce same name within 30s.
    assert!(f
        .admit(
            &Event::with_detail("quantumsec.status.elevated", "again"),
            MON + 5.0
        )
        .is_none());
    assert!(f
        .admit(
            &Event::with_detail("quantumsec.lockdown.proposed", "sim"),
            MON + 6.0
        )
        .is_some());
}

#[test]
fn events_per_minute_are_capped() {
    let mut f = EventFilter::new(3);
    let admitted = (0..10)
        .filter(|i| {
            f.admit(&focus(&format!("app{i}")), MON + *i as f64)
                .is_some()
        })
        .count();
    assert_eq!(admitted, 3);
    assert!(f.admit(&focus("late"), MON + 61.0).is_some());
}

#[test]
fn proactive_needs_confidence_and_respects_cooldown_and_cap() {
    let cfg = MemoryConfig {
        proactive_min_confidence: 0.6,
        proactive_cooldown_min: 60.0,
        proactive_max_per_hour: 2,
        ..MemoryConfig::default()
    };
    let mut p = Proactive::from_config(&cfg);
    let none = |_: &str, _: &str| false;
    assert_eq!(
        p.choose(&[pred("steam", 0.4)], calm(MON), none),
        Err(Quiet::NothingConfident)
    );
    assert_eq!(
        p.choose(&[pred("steam", 0.9)], calm(MON), none)
            .unwrap()
            .subject,
        "steam"
    );
    // Same subject inside the cooldown: skipped, the next one is picked.
    let two = [pred("steam", 0.9), pred("krita", 0.7)];
    assert_eq!(
        p.choose(&two, calm(MON + 60.0), none).unwrap().subject,
        "krita"
    );
    // Two in the last hour: capped.
    assert_eq!(
        p.choose(&[pred("blender", 0.9)], calm(MON + 120.0), none),
        Err(Quiet::HourlyCap)
    );
    // Still capped until the first one is an hour old.
    assert_eq!(
        p.choose(&[pred("steam", 0.9)], calm(MON + 3000.0), none),
        Err(Quiet::HourlyCap)
    );
    assert_eq!(
        p.choose(&[pred("steam", 0.9)], calm(MON + 3601.0), none)
            .unwrap()
            .subject,
        "steam"
    );
}

#[test]
fn proactive_is_silent_during_safeword_busy_recent_use_or_off() {
    let cfg = MemoryConfig::default();
    let mut p = Proactive::from_config(&cfg);
    let preds = [pred("steam", 0.95)];
    let none = |_: &str, _: &str| false;
    let mut m = calm(MON);
    m.safeword = true;
    assert_eq!(p.choose(&preds, m, none), Err(Quiet::Safeword));
    let mut m = calm(MON);
    m.busy = true;
    assert_eq!(p.choose(&preds, m, none), Err(Quiet::Busy));
    assert_eq!(
        p.choose(&preds, calm(MON), |_, s| s == "steam"),
        Err(Quiet::NothingConfident)
    );
    let mut off = cfg.clone();
    off.proactive = false;
    assert_eq!(
        Proactive::from_config(&off).choose(&preds, calm(MON), none),
        Err(Quiet::Off)
    );
    let mut disabled = cfg;
    disabled.enabled = false;
    assert_eq!(
        Proactive::from_config(&disabled).choose(&preds, calm(MON), none),
        Err(Quiet::Off)
    );
}

#[test]
fn suggestion_lines_are_short_and_name_the_subject() {
    for salt in 0..3 {
        let l = suggestion_line(&pred("steam", 0.9), salt);
        assert!(l.contains("steam") && l.len() < 90, "{l}");
    }
    let l = suggestion_line(&pred("evil\n{summary}", 0.9), 0);
    assert!(!l.contains('\n') && !l.contains('{'), "{l}");
}

// ----- the thread -----------------------------------------------------------

struct Rig {
    bridge: MemoryBridge,
    clock: Arc<Mutex<f64>>,
    epoch: Arc<AtomicU64>,
    said: Receiver<String>,
}

fn rig(cfg: MemoryConfig, brain: Option<Brain>) -> Rig {
    let clock = Arc::new(Mutex::new(MON));
    let epoch = Arc::new(AtomicU64::new(0));
    let (stx, said) = channel();
    let stx = Mutex::new(stx);
    let c = clock.clone();
    let e = epoch.clone();
    let mut opts = BridgeOptions::new(
        &cfg,
        Box::new(move |l| {
            let _ = stx.lock().unwrap().send(l);
        }),
    );
    opts.clock = Arc::new(move || *c.lock().unwrap());
    opts.stop_epoch = Arc::new(move || e.load(Ordering::Relaxed));
    let c2 = cfg.clone();
    opts.open = Box::new(move || brain.or_else(|| Brain::open_in_memory(&c2).ok()));
    Rig {
        bridge: MemoryBridge::spawn_with(opts),
        clock,
        epoch,
        said,
    }
}

impl Rig {
    fn set(&self, t: f64) {
        *self.clock.lock().unwrap() = t;
    }

    fn said(&self) -> Option<String> {
        self.said.recv_timeout(Duration::from_millis(1500)).ok()
    }

    fn quiet(&self) -> bool {
        self.said.recv_timeout(Duration::from_millis(300)).is_err()
    }
}

/// A brain that has seen Steam every evening for a week.
fn steam_evenings(cfg: &MemoryConfig) -> Brain {
    let b = Brain::open_in_memory(cfg).unwrap();
    for d in 0..5 {
        let day = MON + d as f64 * DAY;
        for h in [20.0, 20.5] {
            b.observe_event(
                &Event::with_detail("window.opened.app", "steam"),
                &Ctx::at(day + h * 3600.0),
            );
        }
    }
    b
}

const NEXT_MON_EVENING: f64 = MON + 7.0 * DAY + 20.25 * 3600.0;

#[test]
fn disabled_memory_spawns_nothing_and_writes_no_file() {
    let dir = std::env::temp_dir().join(format!("lillith-bridge-off-{}", std::process::id()));
    let cfg = MemoryConfig {
        enabled: false,
        db_path: dir.join("memory.db").to_string_lossy().into_owned(),
        ..MemoryConfig::default()
    };
    let b = MemoryBridge::spawn(&cfg, Box::new(|_| panic!("must not speak")));
    assert!(!b.is_enabled());
    b.observe(&focus("firefox"));
    b.check_now();
    let plan = b
        .report_error("llm: unreachable", "chat.llm", &SafeFix::ALL)
        .try_recv()
        .unwrap();
    assert_eq!(plan, SafeFix::ALL.to_vec(), "caller's order, untouched");
    b.report_fix("llm: unreachable", SafeFix::RetryBackoff, true);
    drop(b);
    assert!(!dir.exists());
}

#[test]
fn thread_learns_focus_events_and_suggests_once() {
    let cfg = MemoryConfig::default();
    let r = rig(cfg.clone(), Some(steam_evenings(&cfg)));
    assert!(r.bridge.is_enabled());
    r.set(NEXT_MON_EVENING);
    r.bridge.check_now();
    let line = r.said().expect("a suggestion");
    assert!(line.contains("steam"), "{line}");
    // Cooldown: asking again right away says nothing.
    r.bridge.check_now();
    assert!(r.quiet());
}

#[test]
fn no_suggestion_after_safeword_or_while_chatting_or_using_the_app() {
    let cfg = MemoryConfig::default();
    // Safeword via the process kill switch.
    let r = rig(cfg.clone(), Some(steam_evenings(&cfg)));
    r.set(NEXT_MON_EVENING);
    r.epoch.store(1, Ordering::Relaxed);
    r.bridge.check_now();
    assert!(r.quiet());
    r.set(NEXT_MON_EVENING + SAFEWORD_QUIET_S + 1.0);
    r.bridge.check_now();
    assert!(r.said().is_some(), "quiet period over");

    // Safeword seen on the feed.
    let r = rig(cfg.clone(), Some(steam_evenings(&cfg)));
    r.set(NEXT_MON_EVENING);
    r.bridge.observe(&Event::new("chat.safeword"));
    r.bridge.check_now();
    assert!(r.quiet());

    // Chatting.
    let r = rig(cfg.clone(), Some(steam_evenings(&cfg)));
    r.set(NEXT_MON_EVENING);
    r.bridge.observe(&Event::with_detail("chat.user", "hey"));
    r.bridge.check_now();
    assert!(r.quiet());
    r.set(NEXT_MON_EVENING + CHAT_QUIET_S + 1.0);
    r.bridge.check_now();
    assert!(r.said().is_some());

    // Already in Steam.
    let r = rig(cfg.clone(), Some(steam_evenings(&cfg)));
    r.set(NEXT_MON_EVENING);
    r.bridge.observe(&focus("Steam"));
    r.bridge.check_now();
    assert!(r.quiet());

    // Fullscreen / busy.
    let r = rig(cfg.clone(), Some(steam_evenings(&cfg)));
    r.set(NEXT_MON_EVENING);
    r.bridge.set_busy_for(600.0);
    r.bridge.check_now();
    assert!(r.quiet());
}

#[test]
fn thread_runs_the_error_fix_loop() {
    let r = rig(MemoryConfig::default(), None);
    let cands = [
        SafeFix::RetryBackoff,
        SafeFix::RestartLlamaServer,
        SafeFix::TextOnly,
    ];
    let text = "llm: server unreachable";
    let first = r
        .bridge
        .report_error(text, "chat.llm", &cands)
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    assert_eq!(first, cands.to_vec());
    r.bridge.report_fix(text, SafeFix::RetryBackoff, false);
    r.bridge.report_fix(text, SafeFix::RestartLlamaServer, true);
    r.set(MON + 3600.0);
    let next = r
        .bridge
        .report_error(text, "chat.llm", &cands)
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    assert_eq!(
        next,
        vec![
            SafeFix::RestartLlamaServer,
            SafeFix::TextOnly,
            SafeFix::RetryBackoff
        ],
        "known fix first, failed one last"
    );
}

#[test]
fn observe_never_blocks_when_the_queue_is_full() {
    let cfg = MemoryConfig {
        events_per_minute: 10_000,
        ..MemoryConfig::default()
    };
    // The factory parks the thread so nothing drains the queue.
    let (gate_tx, gate_rx) = channel::<()>();
    let mut opts = BridgeOptions::new(&cfg, Box::new(|_| {}));
    let c = cfg.clone();
    opts.open = Box::new(move || {
        let _ = gate_rx.recv();
        Brain::open_in_memory(&c).ok()
    });
    let b = MemoryBridge::spawn_with(opts);
    let t = std::time::Instant::now();
    for i in 0..(QUEUE * 3) {
        b.observe(&focus(&format!("app{i}")));
    }
    assert!(t.elapsed() < Duration::from_secs(1));
    assert!(b.dropped() > 0);
    gate_tx.send(()).unwrap();
}

#[test]
fn presence_profile_is_sent_at_start_from_memory_events() {
    let cfg = MemoryConfig::default();
    let (ptx, prx) = channel::<[f32; 8]>();
    let ptx = Mutex::new(ptx);
    let mut opts = BridgeOptions::new(&cfg, Box::new(|_| {}));
    opts.clock = Arc::new(|| NEXT_MON_EVENING);
    let brain = steam_evenings(&cfg);
    opts.open = Box::new(move || Some(brain));
    opts.presence = Some(Box::new(move |p| {
        let _ = ptx.lock().unwrap().send(p);
    }));
    let b = MemoryBridge::spawn_with(opts);
    let p = prx
        .recv_timeout(Duration::from_millis(1500))
        .expect("a profile at start");
    // 10 focus events, all in one evening bucket (whatever the zone).
    assert_eq!(p.iter().sum::<f32>(), 10.0, "{p:?}");
    assert_eq!(p.iter().filter(|v| **v > 0.0).count(), 1, "{p:?}");
    // Not again before PRESENCE_EVERY_S.
    assert!(prx.recv_timeout(Duration::from_millis(300)).is_err());
    drop(b);
}

#[test]
fn presence_detail_is_what_the_overlay_parses() {
    let d = presence_detail(&[0.0, 1.0, 2.5, 0.0, 0.0, 0.0, 10.0, f32::NAN]);
    assert_eq!(d, "0,1,2.5,0,0,0,10,0");
    assert_eq!(d.split(',').count(), 8);
    assert_eq!(PRESENCE_EVENT, "lonely.presence");
}

#[test]
fn no_presence_sink_means_no_profile_work() {
    let cfg = MemoryConfig::default();
    let r = rig(cfg.clone(), Some(steam_evenings(&cfg)));
    // The rig sets no presence sink: suggestions still work as before.
    r.set(NEXT_MON_EVENING);
    r.bridge.check_now();
    assert!(r.said().is_some());
}
