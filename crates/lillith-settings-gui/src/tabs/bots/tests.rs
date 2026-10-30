use super::*;
use crate::tabs::tests::shared;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lillith-botstab-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A tab with no model (tests never touch a real server), its audit log
/// and Paimon pointed at `dir`.
fn tab_in(dir: &std::path::Path) -> BotsTab {
    let s = shared();
    let mut t = BotsTab::new(&s);
    t.model = None;
    t.audit = Some(dir.join("audit.log"));
    t.journal = dir.join("journal");
    t.roster_path = dir.join("bots.toml");
    for b in &mut t.roster {
        b.folders = vec![dir.join("work").display().to_string()];
    }
    t.saved = t.roster.clone();
    t
}

fn wait_planned(t: &mut BotsTab, i: usize) {
    let t0 = std::time::Instant::now();
    while t.runs[i].thinking.is_some() && t0.elapsed() < Duration::from_secs(20) {
        t.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    t.poll();
}

fn frame(ctx: &egui::Context, tab: &mut BotsTab, s: &mut Shared, events: Vec<egui::Event>) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1100.0, 760.0),
        )),
        events,
        ..Default::default()
    };
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| tab.ui(ui, s));
    });
}

fn key(k: egui::Key) -> egui::Event {
    egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

const PAIMON: usize = 2;

#[test]
fn tab_lists_six_bots_idle() {
    let d = tmp("list");
    let t = tab_in(&d);
    assert_eq!(t.id(), "bots");
    assert_eq!(t.roster().len(), 6);
    assert!((0..6).all(|i| t.status(i) == BotStatus::Idle));
    assert_eq!(t.pending(), 0);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn job_plans_then_waits_for_the_card() {
    let d = tmp("plan");
    let mut t = tab_in(&d);
    t.runs[PAIMON].input = "Ko's demo page".into();
    t.submit(PAIMON);
    assert_eq!(t.status(PAIMON), BotStatus::Working);
    wait_planned(&mut t, PAIMON);
    assert_eq!(t.status(PAIMON), BotStatus::NeedsApproval);
    assert_eq!(t.runs[PAIMON].queue.len(), 2);
    assert!(!d.join("work").exists(), "planning wrote nothing");
    let (action, detail) = t.card(PAIMON).expect("a card");
    assert!(action.needs_confirmation());
    assert!(detail.starts_with("Paimon (Web Page Builder) proposes step 1 of 2"));
    let log = std::fs::read_to_string(d.join("audit.log")).unwrap_or_default();
    assert!(log.contains("READ") && log.contains("bot=paimon"), "{log}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn keyboard_never_approves_and_escape_denies() {
    let d = tmp("keys");
    let mut t = tab_in(&d);
    t.selected = PAIMON;
    t.runs[PAIMON].input = "page".into();
    t.submit(PAIMON);
    wait_planned(&mut t, PAIMON);
    let mut s = shared();
    let ctx = egui::Context::default();
    lillith_theme::egui_theme::install(&ctx);
    for _ in 0..3 {
        frame(&ctx, &mut t, &mut s, vec![]);
    }
    assert!(ctx
        .read_response(egui::Id::new(CARD_ID).with("yes"))
        .is_some());
    for k in [
        egui::Key::Enter,
        egui::Key::Space,
        egui::Key::Tab,
        egui::Key::Enter,
    ] {
        frame(&ctx, &mut t, &mut s, vec![key(k)]);
    }
    assert!(t.runs[PAIMON].exec.is_none(), "keys must never run a step");
    assert_eq!(t.runs[PAIMON].queue.len(), 2);
    frame(&ctx, &mut t, &mut s, vec![key(egui::Key::Escape)]);
    assert_eq!(t.runs[PAIMON].queue.len(), 1);
    assert!(t.runs[PAIMON].exec.is_none());
    let log = std::fs::read_to_string(d.join("audit.log")).unwrap_or_default();
    assert!(log.contains("DENY") && !log.contains("CONFIRM"), "{log}");
    assert!(!d.join("work").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn approved_step_writes_and_lists_the_file() {
    let d = tmp("approve");
    let mut t = tab_in(&d);
    t.runs[PAIMON].input = "my page".into();
    t.submit(PAIMON);
    wait_planned(&mut t, PAIMON);
    let ctx = egui::Context::default();
    for _ in 0..2 {
        t.approve(PAIMON, &ctx);
        let t0 = std::time::Instant::now();
        while t.runs[PAIMON].exec.is_some() && t0.elapsed() < Duration::from_secs(20) {
            t.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    assert_eq!(t.status(PAIMON), BotStatus::Done);
    let files = &t.runs[PAIMON].files;
    assert_eq!(files.len(), 2, "{files:?}");
    assert!(files
        .iter()
        .all(|f| f.is_file() && f.starts_with(d.join("work"))));
    let log = std::fs::read_to_string(d.join("audit.log")).unwrap();
    assert_eq!(log.matches("CONFIRM").count(), 2, "{log}");
    assert!(
        log.contains("BOT_STEP") && log.contains("bot=paimon"),
        "{log}"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn stop_drops_the_plan_and_kill_switch_stops_everyone() {
    let d = tmp("stop");
    let mut t = tab_in(&d);
    t.runs[PAIMON].input = "page".into();
    t.submit(PAIMON);
    wait_planned(&mut t, PAIMON);
    t.stop(PAIMON);
    assert!(t.runs[PAIMON].queue.is_empty());
    assert_eq!(t.status(PAIMON), BotStatus::Stopped);
    let log = std::fs::read_to_string(d.join("audit.log")).unwrap();
    assert!(log.contains("CANCELLED"), "{log}");
    // The kill switch moving (simulated: no real stop_all inside the test
    // binary, other tests run commands) stops every busy bot.
    t.runs[1].input = "a rust tool called x".into();
    t.submit(1);
    wait_planned(&mut t, 1);
    assert_eq!(t.status(1), BotStatus::NeedsApproval);
    t.kill_epoch = t.kill_epoch.wrapping_add(1);
    let ctx = egui::Context::default();
    let mut s = shared();
    t.tick(&ctx, &mut s);
    assert_eq!(t.status(1), BotStatus::Stopped);
    assert!(t.runs[1].queue.is_empty());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn disabled_bot_takes_no_job() {
    let d = tmp("off");
    let mut t = tab_in(&d);
    t.roster[0].enabled = false;
    t.runs[0].input = "tidy".into();
    t.submit(0);
    assert!(t.runs[0].thinking.is_none());
    assert!(t.runs[0].chat.last().unwrap().1.contains("is off"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn rename_saves_to_bots_toml() {
    let d = tmp("save");
    let mut t = tab_in(&d);
    let mut s = shared();
    t.roster[3].name = "Ba'al Zebub".into();
    assert_eq!(t.pending(), 1);
    t.save(&mut s).unwrap();
    assert_eq!(t.pending(), 0);
    let back = bots::load_roster(&d.join("bots.toml")).unwrap();
    assert_eq!(back[3].name, "Ba'al Zebub");
    t.roster[0].name = "x".into();
    t.revert(&mut s);
    assert_eq!(t.roster[0].name, "Mammon");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn deep_link_picks_a_bot_or_view() {
    let d = tmp("deep");
    let mut t = tab_in(&d);
    t.deep_link("naamah");
    assert_eq!(t.selected, 5);
    t.deep_link("Andras");
    assert_eq!(t.selected, 4);
    t.deep_link("new");
    assert_eq!(t.view, View::NewBot);
    let _ = std::fs::remove_dir_all(&d);
}

struct FakeDream;
impl DreamStatusSource for FakeDream {
    fn status(&self) -> Option<DreamStatus> {
        Some(DreamStatus {
            session: "d-1".into(),
            state: "running".into(),
            minutes_left: 42,
            jobs_done: 3,
            jobs_total: 5,
            note: String::new(),
            ..Default::default()
        })
    }
}

#[test]
fn dream_hook_shows_the_session() {
    let d = tmp("dream");
    let mut t = tab_in(&d);
    assert!(t.dream_line().starts_with("No dream session"));
    t.set_dream_source(Box::new(FakeDream));
    assert_eq!(
        t.dream_line(),
        "Session d-1: running · 42 min left · 3/5 jobs"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn draws_every_view_headless() {
    let d = tmp("draw");
    let mut t = tab_in(&d);
    let mut s = shared();
    let ctx = egui::Context::default();
    lillith_theme::egui_theme::install(&ctx);
    for v in [View::Chat, View::Settings, View::NewBot] {
        t.view = v;
        for _ in 0..2 {
            frame(&ctx, &mut t, &mut s, vec![]);
        }
    }
    let _ = std::fs::remove_dir_all(&d);
}

struct ItemsDream(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
impl DreamStatusSource for ItemsDream {
    fn status(&self) -> Option<DreamStatus> {
        let gone = self.0.lock().unwrap().clone();
        Some(DreamStatus {
            session: "s-9".into(),
            state: "running".into(),
            minutes_left: 12,
            note: "3 pages visited, 2 saved".into(),
            deadline_local: Some("7:30 PM MDT".into()),
            digest: Some("Rust lifetimes, in short.".into()),
            items: ["a1", "b2"]
                .into_iter()
                .filter(|i| !gone.iter().any(|g| g == i))
                .map(|i| crate::bots::DreamItem {
                    id: i.into(),
                    title: format!("note {i}"),
                    host: "doc.rust-lang.org".into(),
                })
                .collect(),
            ..Default::default()
        })
    }
    fn forget(&mut self, id: &str) -> Result<String, String> {
        self.0.lock().unwrap().push(id.into());
        Ok(format!("note {id}"))
    }
}

fn click(at: egui::Pos2) -> Vec<egui::Event> {
    let b = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    vec![egui::Event::PointerMoved(at), b(true), b(false)]
}

fn key_tap(k: egui::Key) -> Vec<egui::Event> {
    let e = |pressed| egui::Event::Key {
        key: k,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    vec![e(true), e(false)]
}

#[test]
fn dream_items_show_and_forget_is_mouse_only() {
    let d = tmp("dreamitems");
    let mut t = tab_in(&d);
    let gone = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    t.set_dream_source(Box::new(ItemsDream(gone.clone())));
    assert_eq!(
        t.dream_line(),
        "Session s-9: running · 12 min left · 3 pages visited, 2 saved"
    );
    let mut s = shared();
    let ctx = egui::Context::default();
    lillith_theme::egui_theme::install(&ctx);
    for _ in 0..2 {
        frame(&ctx, &mut t, &mut s, vec![]);
    }
    assert_eq!(t.dream_hits.len(), 2, "both saved items are listed");
    // Keyboard never forgets: Tab around and press Enter / Space.
    for _ in 0..40 {
        frame(&ctx, &mut t, &mut s, key_tap(egui::Key::Tab));
        frame(&ctx, &mut t, &mut s, key_tap(egui::Key::Enter));
        frame(&ctx, &mut t, &mut s, key_tap(egui::Key::Space));
    }
    assert!(gone.lock().unwrap().is_empty(), "a key forgot a note");
    // A mouse click on the newest item's Forget does (newest is listed first).
    let (id, r) = t.dream_hits[0].clone();
    assert_eq!(id, "b2");
    frame(&ctx, &mut t, &mut s, click(r.center()));
    frame(&ctx, &mut t, &mut s, vec![]);
    assert_eq!(*gone.lock().unwrap(), ["b2"]);
    assert_eq!(t.dream_hits.len(), 1, "the forgotten item drops out");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn live_status_maps_the_learn_status() {
    use lillith_learn::workers::orchestrator as o;
    let mut st = o::DreamStatus {
        state: o::DreamState::Running,
        session: Some("s-1".into()),
        deadline: Some("2026-10-30T22:00:00Z".into()),
        deadline_local: Some("4:00 PM MDT".into()),
        where_now: Some("lifetimes — doc.rust-lang.org".into()),
        visited: vec!["https://doc.rust-lang.org/a".into()],
        saved_count: 1,
        items: vec![o::DreamItem {
            id: "n1".into(),
            title: "Lifetimes".into(),
            host: "doc.rust-lang.org".into(),
            path: String::new(),
        }],
        digest: Some("short digest".into()),
        ..Default::default()
    };
    // 2026-10-30T21:30:00Z = 1793395800.
    let v = crate::bots::from_learn(&st, 1_793_395_800).unwrap();
    assert_eq!(v.minutes_left, 30);
    assert_eq!(v.deadline_local.as_deref(), Some("4:00 PM MDT"));
    assert_eq!(v.items.len(), 1);
    assert_eq!(v.items[0].id, "n1");
    assert_eq!(v.digest.as_deref(), Some("short digest"));
    assert_eq!(
        dream_text(Some(v)),
        "Session s-1: running · 30 min left · 1 page visited, 1 saved, now: lifetimes — doc.rust-lang.org"
    );
    st.state = o::DreamState::Idle;
    assert!(crate::bots::from_learn(&st, 0).is_none());
    assert_eq!(crate::bots::minutes_until("garbage", 0), 0);
    assert_eq!(crate::bots::minutes_until("1970-01-01T01:00:00Z", 0), 60);
}
