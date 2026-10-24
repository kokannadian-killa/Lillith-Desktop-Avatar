//! Integration-style tests of [`Brain`] on in-memory / temp-file DBs.

use crate::*;
use lillith_core::event::Event;
use std::path::PathBuf;

const DAY: f64 = 86_400.0;
/// 2026-10-19 00:00:00 UTC, a Monday.
const MON: f64 = 1_792_368_000.0;

fn cfg() -> MemoryConfig {
    MemoryConfig::default()
}

fn brain() -> Brain {
    Brain::open_in_memory(&cfg()).unwrap()
}

fn at(t: f64) -> Ctx {
    Ctx::at(t).with_utc_offset(0)
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lillith-mem-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn labels(m: &[Memory]) -> Vec<String> {
    m.iter().map(|m| m.label.clone()).collect()
}

#[test]
fn monday_constant_is_a_monday() {
    assert_eq!(habits::local_hour_weekday(MON, 0), (0, 0));
}

#[test]
fn brain_is_send() {
    fn send<T: Send>() {}
    send::<Brain>();
}

#[test]
fn opt_out_means_no_file_no_reads_no_writes() {
    let d = tmp("optout");
    let path = d.join("memory.db");
    let off = MemoryConfig::disabled();
    let b = Brain::open_path(&path, &off).unwrap();
    assert!(!b.is_enabled());
    assert_eq!(
        b.observe_interaction("I love dark synthwave", "ok", &at(MON)),
        Observed::Disabled
    );
    assert_eq!(
        b.observe_event(&Event::new("window.opened.browser"), &at(MON)),
        Observed::Disabled
    );
    assert!(b.retrieve("synthwave", &at(MON), 5).unwrap().is_empty());
    assert!(b.context_block("synthwave", &at(MON)).is_none());
    assert!(b.predict(&at(MON), 3).is_empty());
    assert_eq!(b.record_error("boom", "x", &at(MON)).unwrap(), None);
    assert_eq!(b.forget(&ForgetCmd::All, &at(MON)).unwrap(), 0);
    assert_eq!(b.stats(), Stats::default());
    assert!(!path.exists(), "a disabled brain never creates the file");
    // Brain::open with the default-path config also stays file-less.
    assert!(!Brain::open(&off).unwrap().is_enabled());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn safeword_short_circuits_and_stores_nothing() {
    let b = brain();
    for msg in [
        "red",
        "RED!",
        "OOC: stop",
        "ooc stop, I love cats",
        "ok wait... ooc: stop. my cat is Nyx",
        "I love red wine",
    ] {
        assert_eq!(
            b.observe_interaction(msg, "reply", &at(MON)),
            Observed::Stop,
            "{msg}"
        );
    }
    assert_eq!(b.stats(), Stats::default(), "nothing at all was written");
    assert!(b.context_block("red", &at(MON)).is_none());
    assert!(b.retrieve("ooc: stop", &at(MON), 5).unwrap().is_empty());
    assert_eq!(
        b.observe_event(&Event::with_detail("notify", "red"), &at(MON)),
        Observed::Stop
    );
    assert_eq!(b.record_error("red", "chat", &at(MON)).unwrap(), None);
    assert!(is_safeword("red") && !is_safeword("I love red wine"));
}

#[test]
fn learns_preferences_and_retrieves_them() {
    let b = brain();
    let o = b.observe_interaction("I love dark synthwave", "Mm, noted.", &at(MON));
    assert!(matches!(o, Observed::Learned { facts: 1, .. }), "{o:?}");
    let m = b
        .retrieve("play me some synthwave", &at(MON + 60.0), 5)
        .unwrap();
    assert_eq!(m[0].label, "Ko likes dark synthwave");
    assert_eq!(m[0].kind, Kind::Preference);
    let block = b
        .context_block("synthwave tonight?", &at(MON + 60.0))
        .unwrap();
    assert!(block.starts_with("[Memory"));
    assert!(block.contains("Ko likes dark synthwave"));
}

#[test]
fn attributes_update_in_place() {
    let b = brain();
    b.observe_interaction("my cat is Nyx", "", &at(MON));
    b.observe_interaction("my cat is Luna now", "", &at(MON + 10.0));
    let m = b.retrieve("how is my cat", &at(MON + 20.0), 5).unwrap();
    let cats: Vec<_> = m
        .iter()
        .filter(|m| m.label.starts_with("Ko's cat"))
        .collect();
    assert_eq!(cats.len(), 1, "{:?}", labels(&m));
    assert_eq!(cats[0].label, "Ko's cat: luna now");
}

#[test]
fn ranking_relevance_then_strength_and_recency() {
    let b = brain();
    let old = at(MON - 20.0 * DAY);
    b.upsert_node(
        Kind::Preference,
        "likes:metal",
        "Ko likes metal music",
        "",
        &old,
    )
    .unwrap();
    b.upsert_node(
        Kind::Preference,
        "likes:synth",
        "Ko likes synth music",
        "",
        &at(MON),
    )
    .unwrap();
    // Same lexical relevance: the fresh, strong one wins.
    let m = b.retrieve("music", &at(MON), 5).unwrap();
    assert_eq!(m[0].label, "Ko likes synth music", "{:?}", labels(&m));
    // A precise match beats strength.
    let m = b.retrieve("metal", &at(MON), 5).unwrap();
    assert_eq!(m[0].label, "Ko likes metal music", "{:?}", labels(&m));
    assert!(m[0].score > m.get(1).map(|x| x.score).unwrap_or(0.0));
    // App context boosts memories about the focused app.
    b.upsert_node(
        Kind::Entity,
        "app:krita",
        "krita (app)",
        "drawing",
        &at(MON),
    )
    .unwrap();
    b.upsert_node(Kind::Entity, "app:gimp", "gimp (app)", "drawing", &at(MON))
        .unwrap();
    let m = b.retrieve("drawing", &at(MON).with_app("gimp"), 2).unwrap();
    assert_eq!(m[0].label, "gimp (app)");
}

#[test]
fn neighbor_expansion_pulls_in_linked_memories() {
    let b = brain();
    let c = at(MON);
    let nyx = b
        .upsert_node(Kind::Entity, "nyx", "Nyx", "Ko's black cat", &c)
        .unwrap()
        .unwrap();
    let vet = b
        .upsert_node(Kind::Event, "vet", "Vet appointment on Friday", "", &c)
        .unwrap()
        .unwrap();
    b.link(nyx, vet, "has", &c).unwrap();
    b.upsert_node(Kind::Event, "dentist", "Dentist appointment", "", &c)
        .unwrap();
    let m = b.retrieve("how is nyx doing", &c, 5).unwrap();
    assert_eq!(m[0].label, "Nyx");
    let v = m
        .iter()
        .find(|m| m.label.starts_with("Vet"))
        .expect("neighbor");
    assert!(v.via_neighbor);
    assert!(m.iter().all(|m| m.label != "Dentist appointment"));
}

#[test]
fn context_block_respects_token_budget() {
    let mut small = cfg();
    small.max_context_tokens = 40;
    small.top_k = 50;
    let b = Brain::open_in_memory(&small).unwrap();
    for i in 0..30 {
        b.upsert_node(
            Kind::Preference,
            &format!("likes:coffee{i}"),
            &format!("Ko likes coffee variety number {i} with a fairly long description attached"),
            "",
            &at(MON),
        )
        .unwrap();
    }
    let block = b.context_block("coffee", &at(MON)).unwrap();
    assert!(block.len() <= 40 * 4, "{} chars", block.len());
    assert!(block.lines().count() >= 2);
    // Default budget (~300 tokens) with default top_k.
    let b = brain();
    for i in 0..30 {
        b.upsert_node(
            Kind::Preference,
            &format!("k{i}"),
            &format!("Ko likes coffee {i}"),
            "",
            &at(MON),
        )
        .unwrap();
    }
    let block = b.context_block("coffee", &at(MON)).unwrap();
    assert!(block.len() <= 300 * 4);
    assert_eq!(block.lines().count(), 1 + cfg().top_k);
    assert!(
        brain().context_block("coffee", &at(MON)).is_none(),
        "empty brain = no block"
    );
}

#[test]
fn decay_prunes_weak_memories_and_caps_nodes() {
    let b = brain();
    b.observe_interaction("I love lava lamps", "", &at(MON));
    let s0 = b
        .strength_of(Kind::Preference, "likes:lava lamps", &at(MON))
        .unwrap();
    let s1 = b
        .strength_of(Kind::Preference, "likes:lava lamps", &at(MON + 14.0 * DAY))
        .unwrap();
    assert!((s1 - s0 / 2.0).abs() < 1e-9, "one half-life halves it");
    b.upsert_node(
        Kind::Preference,
        "likes:fresh",
        "Ko likes fresh things",
        "",
        &at(MON + 200.0 * DAY),
    )
    .unwrap();
    let pruned = b.maintain(&at(MON + 200.0 * DAY)).unwrap();
    assert!(pruned >= 2, "old pref + its entity pruned ({pruned})");
    assert!(b
        .strength_of(Kind::Preference, "likes:lava lamps", &at(MON))
        .is_none());
    assert!(b
        .strength_of(Kind::Preference, "likes:fresh", &at(MON + 200.0 * DAY))
        .is_some());
    assert!(
        b.strength_of(Kind::Entity, "ko", &at(MON)).is_some(),
        "Ko is never pruned"
    );

    let mut capped = cfg();
    capped.max_nodes = 5;
    let b = Brain::open_in_memory(&capped).unwrap();
    for i in 0..12 {
        b.upsert_node(
            Kind::Entity,
            &format!("e{i}"),
            &format!("thing {i}"),
            "",
            &at(MON + i as f64 * DAY),
        )
        .unwrap();
    }
    b.maintain(&at(MON + 12.0 * DAY)).unwrap();
    assert_eq!(b.stats().nodes, 5);
    assert!(
        b.strength_of(Kind::Entity, "e11", &at(MON)).is_some(),
        "strongest kept"
    );
    assert!(
        b.strength_of(Kind::Entity, "e0", &at(MON)).is_none(),
        "weakest dropped"
    );
}

#[test]
fn reuse_reinforces() {
    let b = brain();
    b.observe_interaction("I love dark synthwave", "", &at(MON));
    let later = at(MON + 7.0 * DAY);
    let before = b
        .strength_of(Kind::Preference, "likes:dark synthwave", &later)
        .unwrap();
    b.retrieve("synthwave please", &later, 3).unwrap();
    let after_retrieve = b
        .strength_of(Kind::Preference, "likes:dark synthwave", &later)
        .unwrap();
    assert!(after_retrieve > before);
    let o = b.observe_interaction(
        "synthwave please",
        "Putting on some dark synthwave for you.",
        &later,
    );
    assert!(
        matches!(o, Observed::Learned { reinforced: r, .. } if r >= 1),
        "{o:?}"
    );
    let after_use = b
        .strength_of(Kind::Preference, "likes:dark synthwave", &later)
        .unwrap();
    assert!(after_use > after_retrieve);
}

#[test]
fn change_of_heart_weakens_the_opposite() {
    let b = brain();
    b.observe_interaction("I love pineapple", "", &at(MON));
    let like = b
        .strength_of(Kind::Preference, "likes:pineapple", &at(MON))
        .unwrap();
    b.observe_interaction("I hate pineapple", "", &at(MON + 1.0));
    let like2 = b
        .strength_of(Kind::Preference, "likes:pineapple", &at(MON + 1.0))
        .unwrap();
    assert!(like2 < like * 0.5);
    let m = b.retrieve("pineapple", &at(MON + 2.0), 3).unwrap();
    assert_eq!(m[0].label, "Ko dislikes pineapple", "{:?}", labels(&m));
}

#[test]
fn forget_all_deletes_rows_and_vacuums_the_file() {
    let d = tmp("forgetall");
    let path = d.join("memory.db");
    let b = Brain::open_path(&path, &cfg()).unwrap();
    for i in 0..40 {
        b.observe_interaction(
            &format!("I love zanzibarite crystals number {i}"),
            "",
            &at(MON + i as f64),
        );
    }
    b.record_error("zanzibarite crash", "test", &at(MON))
        .unwrap();
    assert!(b.stats().nodes > 0);
    let n = b.forget(&ForgetCmd::All, &at(MON + 100.0)).unwrap();
    assert!(n > 0);
    assert_eq!(b.stats(), Stats::default());
    assert!(b
        .retrieve("zanzibarite", &at(MON + 101.0), 5)
        .unwrap()
        .is_empty());
    drop(b);
    let mut bytes = std::fs::read(&path).unwrap();
    if let Ok(w) = std::fs::read(d.join("memory.db-wal")) {
        bytes.extend(w);
    }
    let hay = String::from_utf8_lossy(&bytes);
    assert!(
        !hay.contains("zanzibarite"),
        "deleted text must not linger on disk"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn forget_topic_only_removes_that_topic() {
    let b = brain();
    b.observe_interaction("I hate pineapple", "", &at(MON));
    b.observe_interaction("I love dark synthwave", "", &at(MON));
    b.log_event("request", "pineapple pizza", &at(MON)).unwrap();
    let n = b
        .forget(
            &parse_forget("forget about pineapple").unwrap(),
            &at(MON + 5.0),
        )
        .unwrap();
    assert!(n >= 3, "entity + preference + event ({n})");
    assert!(b
        .retrieve("pineapple", &at(MON + 6.0), 5)
        .unwrap()
        .iter()
        .all(|m| !m.label.contains("pineapple")));
    assert!(b
        .strength_of(Kind::Preference, "dislikes:pineapple", &at(MON))
        .is_none());
    assert!(b
        .strength_of(Kind::Preference, "likes:dark synthwave", &at(MON))
        .is_some());
}

#[test]
fn forget_last_minutes_keeps_older_memories() {
    let b = brain();
    b.observe_interaction("I love old school jazz", "", &at(MON));
    b.observe_interaction("I love new shiny trance", "", &at(MON + 3600.0));
    let n = b
        .forget(
            &parse_forget("forget the last 30 minutes").unwrap(),
            &at(MON + 3700.0),
        )
        .unwrap();
    assert!(n >= 2);
    assert!(b
        .strength_of(Kind::Preference, "likes:new shiny trance", &at(MON))
        .is_none());
    assert!(b
        .strength_of(Kind::Preference, "likes:old school jazz", &at(MON))
        .is_some());
}

#[test]
fn forget_command_itself_is_not_learned() {
    let b = brain();
    assert_eq!(
        b.observe_interaction("forget about my love for cats", "", &at(MON)),
        Observed::Ignored
    );
    assert_eq!(b.stats().nodes, 0);
}

#[test]
fn error_memory_recalls_fixes_and_counts_recurrences() {
    let b = brain();
    let e1 = "llama-server exited with code 137 (pid 4242) loading /models/a.gguf";
    let e2 = "llama-server exited with code 137 (pid 777) loading /models/b.gguf";
    let r = b
        .record_error(e1, "chat.server", &at(MON))
        .unwrap()
        .unwrap();
    assert_eq!((r.count, r.known_fix.clone()), (1, None));
    b.record_fix(
        e1,
        "lower ctx_size to 2048",
        FixOutcome::Failed,
        &at(MON + 1.0),
    )
    .unwrap();
    b.record_fix(e1, "set threads = 2", FixOutcome::Worked, &at(MON + 2.0))
        .unwrap();
    let r = b
        .record_error(e2, "chat.server", &at(MON + 3.0))
        .unwrap()
        .unwrap();
    assert_eq!(r.count, 2, "same signature despite different pid/path");
    assert_eq!(r.known_fix.as_deref(), Some("set threads = 2"));
    let k = b.lookup_error(e2).unwrap();
    assert_eq!(k.failed_fixes, vec!["lower ctx_size to 2048".to_string()]);
    assert_eq!((k.successes, k.failures, k.recurrences), (1, 0, 2));
    // The error is retrievable with its fix.
    let block = b
        .context_block("llama-server exited again", &at(MON + 4.0))
        .unwrap();
    assert!(block.contains("fix: set threads = 2"), "{block}");
    // A fix that then fails more than it works is no longer trusted.
    b.record_fix(e1, "set threads = 2", FixOutcome::Failed, &at(MON + 5.0))
        .unwrap();
    b.record_fix(e1, "set threads = 2", FixOutcome::Failed, &at(MON + 6.0))
        .unwrap();
    assert_eq!(b.known_fix(e1), None);
    assert!(b.lookup_error("something else entirely").is_none());
    // app.crash events land in the same memory.
    let o = b.observe_event(
        &Event::with_detail("app.crash", "krita segfault at 0x1234"),
        &at(MON),
    );
    assert!(matches!(o, Observed::Learned { .. }));
    assert_eq!(
        b.lookup_error("krita segfault at 0xffff")
            .unwrap()
            .recurrences,
        1
    );
}

#[test]
fn predicts_habits_from_events() {
    let b = brain();
    // Steam every weekday evening at 20:xx, VS Code every morning.
    for d in 0..5 {
        let day = MON + d as f64 * DAY;
        b.observe_event(&Event::new("window.opened.steam"), &at(day + 20.0 * 3600.0));
        b.observe_event(&Event::new("window.opened.steam"), &at(day + 20.5 * 3600.0));
        b.observe_interaction("open vscode", "", &at(day + 9.0 * 3600.0));
    }
    let next_monday_evening = at(MON + 7.0 * DAY + 20.25 * 3600.0);
    let p = b.predict(&next_monday_evening, 3);
    assert_eq!(p[0].subject, "steam", "{p:?}");
    assert_eq!(p[0].describe(), "open steam");
    assert!(p[0].confidence > 0.5);
    let morning = b.predict(&at(MON + 7.0 * DAY + 9.0 * 3600.0), 3);
    assert!(morning.iter().any(|p| p.subject == "vscode"), "{morning:?}");
    assert!(morning.iter().all(|p| p.subject != "steam"));
    // Strong patterns become habit nodes and show up in the context block.
    assert!(b.refresh_habits(&next_monday_evening).unwrap() >= 1);
    let block = b
        .context_block("bored, any ideas?", &next_monday_evening)
        .unwrap();
    assert!(
        block.contains("Around now Ko often wants to open steam"),
        "{block}"
    );
    // Private windows are never logged.
    let before = b.stats().events;
    assert_eq!(
        b.observe_event(&Event::new("window.private"), &next_monday_evening),
        Observed::Ignored
    );
    assert_eq!(b.stats().events, before);
}

#[test]
fn events_are_bounded() {
    let mut c = cfg();
    c.max_events = 10;
    let b = Brain::open_in_memory(&c).unwrap();
    for i in 0..50 {
        b.log_event("app", "x", &at(MON + i as f64)).unwrap();
    }
    assert_eq!(b.stats().events, 10);
}

#[test]
fn file_db_migrates_once_and_persists() {
    let d = tmp("persist");
    let path = d.join("sub").join("memory.db");
    {
        let b = Brain::open_path(&path, &cfg()).unwrap();
        b.observe_interaction("I love dark synthwave", "", &at(MON));
        assert_eq!(b.path(), Some(path.as_path()));
    }
    let b = Brain::open_path(&path, &cfg()).unwrap();
    assert!(b
        .strength_of(Kind::Preference, "likes:dark synthwave", &at(MON))
        .is_some());
    let c = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(schema::version(&c).unwrap(), schema::LATEST);
    let mode: String = c
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600, "memory file is private");
    }
    drop(b);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn custom_extractor_hook() {
    struct Always;
    impl Extractor for Always {
        fn extract(&self, _: &str, _: &str) -> Vec<Fact> {
            vec![Fact::Attribute {
                attr: "mood".into(),
                value: "sleepy".into(),
            }]
        }
    }
    let b = brain().with_extractor(Box::new(Always));
    b.observe_interaction("whatever", "", &at(MON));
    assert!(b.strength_of(Kind::Entity, "attr:mood", &at(MON)).is_some());
}
