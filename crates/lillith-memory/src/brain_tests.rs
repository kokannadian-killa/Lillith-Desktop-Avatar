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
    assert_eq!(b.forget(&ForgetCmd::All, &at(MON)).unwrap().total(), 0);
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
    let r = b.forget(&ForgetCmd::All, &at(MON + 100.0)).unwrap();
    assert!(r.total() > 0);
    assert!(r.scrubbed, "{r:?}");
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
        .unwrap()
        .total();
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
        .unwrap()
        .total();
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

/// The DB + WAL bytes, lowercased (for "nothing lingers on disk" checks).
fn file_text(path: &std::path::Path) -> String {
    let mut bytes = std::fs::read(path).unwrap();
    let wal = path.with_file_name(format!(
        "{}-wal",
        path.file_name().unwrap().to_string_lossy()
    ));
    if let Ok(w) = std::fs::read(wal) {
        bytes.extend(w);
    }
    String::from_utf8_lossy(&bytes).to_lowercase()
}

/// Rows + FTS hits for `word` in a closed DB file.
fn residue(path: &std::path::Path, word: &str) -> i64 {
    let c = rusqlite::Connection::open(path).unwrap();
    let pat = format!("%{word}%");
    let q = |sql: &str| -> i64 { c.query_row(sql, [&pat], |r| r.get(0)).unwrap() };
    q("SELECT count(*) FROM nodes WHERE lower(label || ' ' || detail || ' ' || key) LIKE ?1")
        + q("SELECT count(*) FROM events WHERE lower(subject) LIKE ?1")
        + q("SELECT count(*) FROM errors WHERE lower(sample || context || coalesce(fix,'') || failed_fixes) LIKE ?1")
        + c.query_row(
            "SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH ?1",
            [format!("\"{word}\"")],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
}

#[test]
fn forget_my_ex_leaves_no_sarah_in_rows_fts_or_file_bytes() {
    let d = tmp("forget-ex");
    let path = d.join("memory.db");
    let b = Brain::open_path(&path, &cfg()).unwrap();
    b.observe_interaction("My ex is Sarah", "ok", &at(MON));
    b.observe_interaction("I love dark synthwave", "ok", &at(MON + 1.0));
    b.observe_interaction(
        "I love text editors and the next song",
        "ok",
        &at(MON + 2.0),
    );
    assert!(b.strength_of(Kind::Entity, "attr:ex", &at(MON)).is_some());
    let cmd = parse_forget("forget about my ex").unwrap();
    assert_eq!(cmd, ForgetCmd::Topic("my ex".into()));
    let preview = b.forget_preview(&cmd, &at(MON + 5.0)).unwrap();
    assert!(preview.nodes >= 1, "{preview:?}");
    assert!(
        b.strength_of(Kind::Entity, "attr:ex", &at(MON)).is_some(),
        "preview deletes nothing"
    );
    let r = b.forget(&cmd, &at(MON + 5.0)).unwrap();
    assert!(r.nodes >= 1 && r.scrubbed, "{r:?}");
    assert!(b.strength_of(Kind::Entity, "attr:ex", &at(MON)).is_none());
    // Whole words only: "text"/"next" are not "ex"; "my"/"about" match nothing.
    assert!(b
        .strength_of(Kind::Preference, "likes:dark synthwave", &at(MON))
        .is_some());
    assert!(b
        .retrieve("text editors", &at(MON + 6.0), 5)
        .unwrap()
        .iter()
        .any(|m| m.label.contains("text editors")));
    drop(b);
    assert_eq!(residue(&path, "sarah"), 0);
    assert!(
        !file_text(&path).contains("sarah"),
        "no 'sarah' in the db/wal bytes"
    );
    assert!(file_text(&path).contains("synthwave"), "the rest survives");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn forget_unknown_topic_reports_zero() {
    let b = brain();
    b.observe_interaction("I love dark synthwave", "", &at(MON));
    let before = b.stats();
    let r = b
        .forget(
            &parse_forget("forget about dragons").unwrap(),
            &at(MON + 1.0),
        )
        .unwrap();
    assert_eq!(r.total(), 0, "{r:?}");
    assert_eq!(b.stats().nodes, before.nodes);
    // Only stopwords: nothing to match, nothing deleted.
    let r = b
        .forget(&ForgetCmd::Topic("about my the it".into()), &at(MON + 2.0))
        .unwrap();
    assert_eq!(r.total(), 0);
    assert_eq!(b.stats().nodes, before.nodes);
}

#[test]
fn forget_last_minutes_removes_rows_updated_recently_and_their_bytes() {
    let d = tmp("forget-updated");
    let path = d.join("memory.db");
    let b = Brain::open_path(&path, &cfg()).unwrap();
    b.observe_interaction("I love old school jazz", "", &at(MON));
    b.observe_interaction("my favorite band is Ghost", "", &at(MON));
    b.record_error("zorblax daemon crashed", "test", &at(MON))
        .unwrap();
    // An hour later the old rows are rewritten with new content.
    b.observe_interaction("my favorite band is Quuxtronic", "", &at(MON + 3600.0));
    b.record_fix(
        "zorblax daemon crashed",
        "flurbify the cache",
        FixOutcome::Worked,
        &at(MON + 3600.0),
    )
    .unwrap();
    let r = b
        .forget(
            &parse_forget("forget the last 30 minutes").unwrap(),
            &at(MON + 3700.0),
        )
        .unwrap();
    assert!(r.nodes >= 1 && r.errors == 1 && r.scrubbed, "{r:?}");
    assert!(b
        .strength_of(Kind::Entity, "attr:favorite band", &at(MON))
        .is_none());
    assert!(b.lookup_error("zorblax daemon crashed").is_none());
    assert!(b
        .strength_of(Kind::Preference, "likes:old school jazz", &at(MON))
        .is_some());
    drop(b);
    for w in ["quuxtronic", "flurbify"] {
        assert_eq!(residue(&path, w), 0, "{w}");
        assert!(!file_text(&path).contains(w), "{w} lingers on disk");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn forget_reports_when_another_reader_keeps_the_wal() {
    let d = tmp("forget-busy");
    let path = d.join("memory.db");
    let b = Brain::open_path(&path, &cfg()).unwrap();
    b.observe_interaction("I love glimmerwick candles", "", &at(MON));
    let reader = rusqlite::Connection::open(&path).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let _: i64 = reader
        .query_row("SELECT count(*) FROM nodes", [], |r| r.get(0))
        .unwrap();
    let r = b.forget(&ForgetCmd::All, &at(MON + 1.0)).unwrap();
    assert!(r.total() > 0);
    assert_eq!(b.stats(), Stats::default(), "rows are deleted regardless");
    assert!(
        !r.scrubbed && r.note.is_some(),
        "honest about the WAL: {r:?}"
    );
    reader.execute_batch("COMMIT").unwrap();
    drop(reader);
    assert!(b.vacuum().unwrap(), "scrubs once the reader is gone");
    drop(b);
    assert!(!file_text(&path).contains("glimmerwick"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn writes_wait_for_another_process_holding_the_lock() {
    let d = tmp("busy-writer");
    let path = d.join("memory.db");
    let b = Brain::open_path(&path, &cfg()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let p2 = path.clone();
    let h = std::thread::spawn(move || {
        let other = rusqlite::Connection::open(&p2).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        other.execute_batch("COMMIT").unwrap();
    });
    rx.recv().unwrap();
    assert!(matches!(
        b.observe_interaction("I love quiet rain", "", &at(MON)),
        Observed::Learned { .. }
    ));
    h.join().unwrap();
    assert!(b
        .strength_of(Kind::Preference, "likes:quiet rain", &at(MON))
        .is_some());
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn db_file_and_folder_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let d = tmp("perms");
    let dir = d.join("nested").join("lillith");
    let path = dir.join("memory.db");
    let b = Brain::open_path(&path, &cfg()).unwrap();
    b.observe_interaction("I love tea", "", &at(MON));
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&path), 0o600);
    // An existing loose file is tightened on open.
    drop(b);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let _b = Brain::open_path(&path, &cfg()).unwrap();
    assert_eq!(mode(&path), 0o600);
    // A folder that cannot be created is an error, not a silent default.
    let blocker = d.join("file");
    std::fs::write(&blocker, b"x").unwrap();
    assert!(Brain::open_path(&blocker.join("memory.db"), &cfg()).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn record_fix_never_stores_the_safeword_and_does_not_recurse() {
    let b = brain();
    b.record_fix("red", "restart", FixOutcome::Worked, &at(MON))
        .unwrap();
    b.record_fix("boom", "OOC: stop", FixOutcome::Worked, &at(MON))
        .unwrap();
    assert_eq!(b.stats(), Stats::default());
    // A fix for an unseen error records it once.
    b.record_fix("wibble exploded", "restart", FixOutcome::Worked, &at(MON))
        .unwrap();
    assert_eq!(b.stats().errors, 1);
    assert_eq!(b.known_fix("wibble exploded").as_deref(), Some("restart"));
}

#[test]
fn errors_table_is_capped() {
    let mut c = cfg();
    c.max_errors = 3;
    let b = Brain::open_in_memory(&c).unwrap();
    for (i, e) in ["alpha", "bravo", "charlie", "delta", "echo"]
        .iter()
        .enumerate()
    {
        b.record_error(&format!("{e} module failed"), "t", &at(MON + i as f64))
            .unwrap();
    }
    assert_eq!(b.stats().errors, 3);
    assert!(b.lookup_error("alpha module failed").is_none());
    assert!(b.lookup_error("echo module failed").is_some());
    let error_nodes = b
        .retrieve("module failed", &at(MON + 10.0), 10)
        .unwrap()
        .iter()
        .filter(|m| m.kind == Kind::Error)
        .count();
    assert!(error_nodes <= 3);
}

// ----- re-review regressions (reviews/memory-graph-rereview-probe.rs) ------

#[test]
fn forget_my_new_job_keeps_new_wave_music() {
    // probe_multiword: one shared word ("new") is not enough.
    let b = brain();
    b.observe_interaction("I love new wave music", "", &at(MON));
    b.observe_interaction("my job is barista", "", &at(MON));
    let cmd = parse_forget("forget about my new job").unwrap();
    let preview = b.forget_preview(&cmd, &at(MON + 5.0)).unwrap();
    assert!(
        !preview.examples.iter().any(|e| e.contains("wave")),
        "{preview:?}"
    );
    b.forget(&cmd, &at(MON + 5.0)).unwrap();
    assert!(b
        .strength_of(Kind::Preference, "likes:new wave music", &at(MON))
        .is_some());
    // "my job" (all of its one content word) still finds the job.
    let r = b
        .forget(&parse_forget("forget my job").unwrap(), &at(MON + 6.0))
        .unwrap();
    assert!(r.nodes >= 1, "{r:?}");
    assert!(b.strength_of(Kind::Entity, "attr:job", &at(MON)).is_none());
    assert!(b
        .strength_of(Kind::Preference, "likes:new wave music", &at(MON))
        .is_some());
}

#[test]
fn forget_the_news_keeps_new_wave_music() {
    // probe_plural: "news" is not the plural of "new".
    let b = brain();
    b.observe_interaction("I love new wave music", "", &at(MON));
    let r = b
        .forget(
            &parse_forget("forget about the news").unwrap(),
            &at(MON + 5.0),
        )
        .unwrap();
    assert_eq!(r.total(), 0, "{r:?}");
    assert!(b
        .strength_of(Kind::Preference, "likes:new wave music", &at(MON))
        .is_some());
}

#[test]
fn forget_topic_matches_exact_plural_of_the_same_word() {
    let b = brain();
    b.observe_interaction("I love cats", "", &at(MON));
    b.observe_interaction("I love catalog shopping", "", &at(MON));
    let r = b
        .forget(&parse_forget("forget about cat").unwrap(), &at(MON + 5.0))
        .unwrap();
    assert!(r.nodes >= 1, "{r:?}");
    assert!(b
        .strength_of(Kind::Preference, "likes:cats", &at(MON))
        .is_none());
    assert!(b
        .strength_of(Kind::Preference, "likes:catalog shopping", &at(MON))
        .is_some());
}

#[test]
fn forget_job_expands_the_value_only_as_a_whole_phrase() {
    // probe_value_expansion: job = "google engineer" must not take the
    // "google chrome" app event with it.
    let b = brain();
    b.observe_interaction("my job is google engineer", "", &at(MON));
    b.log_event("app", "google chrome", &at(MON)).unwrap();
    b.log_event("app", "firefox", &at(MON)).unwrap();
    b.log_event("note", "lunch with a google engineer", &at(MON))
        .unwrap();
    let ev0 = b.stats().events;
    let r = b
        .forget(
            &parse_forget("forget about my job").unwrap(),
            &at(MON + 5.0),
        )
        .unwrap();
    assert!(r.nodes >= 1, "{r:?}");
    // The whole-phrase event (and the chat's own "job" request event) go;
    // the browser and firefox stay.
    assert!(r.events >= 1, "{r:?}");
    assert_eq!(b.stats().events, ev0 - r.events as u64);
    let lunch = b
        .forget_preview(&ForgetCmd::Topic("lunch".into()), &at(MON + 6.0))
        .unwrap();
    assert_eq!(lunch.events, 0, "{lunch:?}");
    let ff = b
        .forget_preview(&ForgetCmd::Topic("firefox".into()), &at(MON + 6.0))
        .unwrap();
    assert_eq!(ff.events, 1, "{ff:?}");
    let chrome = b
        .forget_preview(&ForgetCmd::Topic("google chrome".into()), &at(MON + 6.0))
        .unwrap();
    assert_eq!(chrome.events, 1, "google chrome survives: {chrome:?}");
}

#[test]
fn forget_preview_carries_a_few_short_examples() {
    let b = brain();
    b.observe_interaction("my job is barista", "", &at(MON));
    for i in 0..10 {
        b.log_event("note", &format!("job shift {i}"), &at(MON))
            .unwrap();
    }
    let cmd = parse_forget("forget my job").unwrap();
    let p = b.forget_preview(&cmd, &at(MON + 5.0)).unwrap();
    assert!(p.total() >= 11, "{p:?}");
    assert!(!p.examples.is_empty() && p.examples.len() <= FORGET_EXAMPLES);
    assert!(p.examples.iter().all(|e| e.chars().count() <= 60));
    assert!(p.examples[0].contains("barista"), "{:?}", p.examples);
    assert!(b.strength_of(Kind::Entity, "attr:job", &at(MON)).is_some());
}

#[test]
fn huge_time_window_needs_confirmation_and_is_reported_honestly() {
    // probe_lastsecs_huge: "the last 1000 months" is forget-everything, so
    // it is never run without a yes (the chat layer asks first).
    let cmd = parse_forget("forget the last 1000 months").unwrap();
    assert_eq!(cmd, ForgetCmd::LastSecs(2_592_000_000));
    assert!(cmd.needs_confirm());
    assert!(!parse_forget("forget the last 30 minutes")
        .unwrap()
        .needs_confirm());
    let b = brain();
    b.observe_interaction("I love tea", "", &at(MON - 400.0 * DAY));
    let p = b.forget_preview(&cmd, &at(MON)).unwrap();
    assert!(p.nodes >= 1, "{p:?}");
    assert!(b.stats().nodes > 1, "preview deletes nothing");
}

#[test]
fn reinforcing_inside_the_window_does_not_make_an_old_memory_new() {
    // probe_reinforce_does_not_bump_updated_at.
    let b = brain();
    b.observe_interaction("I love tea", "", &at(MON));
    b.observe_interaction("I love tea", "", &at(MON + 3600.0));
    b.forget(
        &parse_forget("forget the last 30 minutes").unwrap(),
        &at(MON + 3700.0),
    )
    .unwrap();
    assert!(b
        .strength_of(Kind::Preference, "likes:tea", &at(MON))
        .is_some());
}

#[test]
fn forget_writes_a_private_content_free_audit_line() {
    let d = tmp("forget-audit");
    let path = d.join("memory.db");
    let b = Brain::open_path(&path, &cfg()).unwrap();
    assert_eq!(b.audit_log(), Some(d.join("audit.log").as_path()));
    b.observe_interaction("My ex is Sarah", "ok", &at(MON));
    b.forget(&parse_forget("forget about my ex").unwrap(), &at(MON + 5.0))
        .unwrap();
    b.forget(&ForgetCmd::LastTurn, &at(MON + 6.0)).unwrap();
    let log = std::fs::read_to_string(d.join("audit.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert!(lines[0].starts_with("2026-10-19T00:00:05Z\tFORGET\tmemory:topic\tnodes="));
    assert!(lines[1].contains("memory:last_turn"));
    let low = log.to_lowercase();
    assert!(!low.contains("sarah") && !low.contains(" ex"), "{log}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(d.join("audit.log"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // In-memory brains have no audit file.
    assert_eq!(brain().audit_log(), None);
    let _ = std::fs::remove_dir_all(&d);
}
