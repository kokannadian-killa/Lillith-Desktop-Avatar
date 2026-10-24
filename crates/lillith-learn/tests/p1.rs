//! P1 end-to-end: one step over fake installed docs, dedupe/merge,
//! invalid replies, a stopped step, dry run, v2 → v3 migrate + reindex,
//! and the "no network from the idle path" check.

use lillith_core::config::Config;
use lillith_learn::learner::Outcome;
use lillith_learn::readers::DocRoots;
use lillith_learn::{FakeSummarizer, Learner, NoteStore, SumError};
use lillith_memory::{Brain, MemoryConfig};
use std::io::Write;
use std::path::{Path, PathBuf};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lillith-p1-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn gz(text: &str) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(text.as_bytes()).unwrap();
    e.finish().unwrap()
}

fn docs(d: &Path) -> DocRoots {
    std::fs::create_dir_all(d.join("man/man8")).unwrap();
    std::fs::write(
        d.join("man/man8/pacman.8.gz"),
        gz(".SH NAME\npacman \\- package manager utility\n.SH DESCRIPTION\nPacman is a package management utility for Arch and Garuda.\n"),
    )
    .unwrap();
    DocRoots {
        man: vec![d.join("man")],
        ..DocRoots::default()
    }
}

fn learner(d: &Path, brain: Brain) -> Learner {
    let cfg = Config::default();
    Learner::new(&cfg, brain)
        .with_docs(docs(d))
        .with_knowledge_dir(Some(d.join("knowledge")))
        .with_audit_log(Some(d.join("audit.log")))
}

const REPLY: &str = "{\"kind\":\"howto\",\"title\":\"Remove orphan packages with pacman\",\"body\":\"On Garuda, pacman -Rns $(pacman -Qtdq) removes orphans; systemd is untouched.\",\"links\":[\"pacman\"]}\n\
{\"kind\":\"fact\",\"title\":\"What pacman is\",\"body\":\"pacman is the Arch package manager used by Garuda.\",\"links\":[]}";

#[test]
fn one_step_writes_indexed_linked_audited_notes() {
    let d = tmp("step");
    let brain = Brain::open_in_memory(&MemoryConfig::default()).unwrap();
    let mut l = learner(&d, brain);
    let mut sum = FakeSummarizer::with(&[REPLY]);
    let rep = l.step(&mut sum, &|| false);
    assert_eq!(rep.outcome, Outcome::Wrote, "{rep:?}");
    assert_eq!(rep.source.as_deref(), Some("man:pacman(8)"));
    assert_eq!(rep.written.len(), 2);
    assert!(rep.written.iter().all(|w| w.op == "write"));
    // The classifier is sure about the first one (pacman, garuda, systemd).
    assert_eq!(rep.written[0].topic, "linux-garuda");
    let notes = l.brain().all_notes();
    assert_eq!(notes.len(), 2);
    for n in &notes {
        let p = d.join("knowledge").join(&n.path);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("---\nid: n-"), "{text}");
        assert!(text.len() <= lillith_learn::MAX_NOTE_BYTES);
    }
    // One LEARN line per write, never the note text.
    let audit = std::fs::read_to_string(d.join("audit.log")).unwrap();
    let lines: Vec<&str> = audit.lines().collect();
    assert_eq!(lines.len(), 2, "{audit}");
    for l in &lines {
        let cols: Vec<&str> = l.split('\t').collect();
        assert_eq!(cols.len(), 5, "{l}");
        assert_eq!(cols[1], "LEARN");
        assert_eq!(cols[2], "write");
        assert!(cols[4].starts_with("note=n-"));
    }
    assert!(!audit.contains("orphans") && !audit.contains("Arch package manager"));
    // Second step: same titles → merged, not duplicated.
    let mut sum = FakeSummarizer::with(&[
        "{\"kind\":\"howto\",\"title\":\"remove orphan packages with Pacman!\",\"body\":\"Check with pacman -Qtd first.\",\"links\":[]}",
    ]);
    // The man page has one chunk; let the picker re-read it as changed.
    std::fs::write(
        d.join("man/man8/pacman.8.gz"),
        gz(".SH NAME\npacman \\- package manager utility, updated\n"),
    )
    .unwrap();
    let rep = l.step(&mut sum, &|| false);
    assert_eq!(rep.outcome, Outcome::Wrote, "{rep:?}");
    assert_eq!(rep.written[0].op, "merge");
    assert_eq!(
        l.brain().all_notes().len(),
        2,
        "merged into the existing note"
    );
    let merged = l
        .brain()
        .all_notes()
        .into_iter()
        .find(|n| n.title.contains("orphan"))
        .unwrap();
    let text = std::fs::read_to_string(d.join("knowledge").join(&merged.path)).unwrap();
    assert!(text.contains("pacman -Qtd first") && text.contains("-Rns"));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn invalid_reply_is_discarded_and_counted() {
    let d = tmp("invalid");
    let brain = Brain::open_in_memory(&MemoryConfig::default()).unwrap();
    let mut l = learner(&d, brain);
    let mut sum = FakeSummarizer::with(&["Sure! Here you go: pacman is great"]);
    let rep = l.step(&mut sum, &|| false);
    assert_eq!(rep.outcome, Outcome::Invalid);
    assert!(l.brain().all_notes().is_empty());
    assert_eq!(l.brain().learn_job_tries("chunk", "man:pacman(8)#0"), 1);
    assert!(!d.join("audit.log").exists());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_stopped_step_keeps_nothing() {
    let d = tmp("stopped");
    let brain = Brain::open_in_memory(&MemoryConfig::default()).unwrap();
    let mut l = learner(&d, brain);
    // Stop fires after the reply (safeword mid-step): the output is dropped.
    let calls = std::cell::Cell::new(0);
    let stop = || {
        calls.set(calls.get() + 1);
        calls.get() > 1
    };
    let mut sum = FakeSummarizer::with(&[REPLY]);
    let rep = l.step(&mut sum, &stop);
    assert_eq!(rep.outcome, Outcome::Discarded(SumError::Stopped));
    assert!(l.brain().all_notes().is_empty());
    assert!(!d.join("knowledge").exists(), "nothing was written");
    assert!(!d.join("audit.log").exists());
    // Chat preempting / busy: also nothing kept and no try burned.
    let mut busy = FakeSummarizer::default();
    busy.replies.push_back(Err(SumError::Preempted));
    let rep = l.step(&mut busy, &|| false);
    assert_eq!(rep.outcome, Outcome::Discarded(SumError::Preempted));
    assert_eq!(l.brain().learn_job_tries("chunk", "man:pacman(8)#0"), 0);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn dry_run_writes_nothing() {
    let d = tmp("dry");
    let brain = Brain::open_in_memory(&MemoryConfig::default()).unwrap();
    let mut l = learner(&d, brain).dry_run(true);
    let mut sum = FakeSummarizer::with(&[REPLY]);
    let rep = l.step(&mut sum, &|| false);
    assert_eq!(rep.outcome, Outcome::DryRun);
    assert_eq!(rep.items.len(), 2);
    assert_eq!(rep.written[0].topic, "linux-garuda");
    assert!(!d.join("knowledge").exists());
    assert!(!d.join("audit.log").exists());
    assert!(l.brain().all_notes().is_empty());
    assert!(l.brain().all_sources().is_empty());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn v2_db_migrates_to_v3_and_reindex_rebuilds_from_disk() {
    let d = tmp("reindex");
    let db = d.join("memory.db");
    {
        // A v2 database from the previous release, with a memory in it.
        let c = rusqlite::Connection::open(&db).unwrap();
        for m in &lillith_memory::schema::MIGRATIONS[..2] {
            c.execute_batch(m).unwrap();
        }
        c.pragma_update(None, "user_version", 2).unwrap();
        c.execute(
            "INSERT INTO nodes(kind, key, label, detail, strength, created_at, last_seen)
             VALUES ('entity', 'pacman', 'pacman', '', 1.0, 0, 0)",
            [],
        )
        .unwrap();
    }
    let cfg = MemoryConfig::default();
    let brain = Brain::open_path(&db, &cfg).unwrap();
    {
        let c = rusqlite::Connection::open(&db).unwrap();
        let v: u32 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 3);
    }
    // Notes exist on disk (e.g. after restoring a backup) but not in the index.
    let store = NoteStore::open(&d.join("knowledge"), &["linux-garuda".into()], true).unwrap();
    let lock = store.lock().unwrap().unwrap();
    for (id, title) in [("n-aaa", "pacman orphans"), ("n-bbb", "pacman mirrors")] {
        let n = lillith_learn::Note {
            id: id.into(),
            title: title.into(),
            topic: "linux-garuda".into(),
            kind: "fact".into(),
            source_type: "man".into(),
            source_ref: "man:pacman(8)".into(),
            source_hash: "fnv1a:0".into(),
            created: 1.0,
            updated: 1.0,
            confidence: 0.5,
            quiz_asked: 0,
            quiz_passed: 0,
            links: vec!["n-aaa".into()],
            origin: "learned".into(),
            body: format!("{title}: use pacman"),
            web: None,
        };
        store.write(&lock, &n).unwrap();
    }
    drop(lock);
    let mut l = Learner::new(&Config::default(), brain)
        .with_knowledge_dir(Some(d.join("knowledge")))
        .with_audit_log(None);
    assert_eq!(l.reindex().unwrap(), 2);
    let notes = l.brain().all_notes();
    assert_eq!(notes.len(), 2);
    assert!(notes.iter().all(|n| n.topic == "linux-garuda"));
    // Rebuilding twice gives the same index.
    assert_eq!(l.reindex().unwrap(), 2);
    assert_eq!(l.brain().all_notes().len(), 2);
    assert_eq!(l.brain().similar_notes("pacman mirrors", 3).len(), 2);
    let _ = std::fs::remove_dir_all(d);
}

/// The idle path (pick → read → write → link) never starts a network
/// fetch, an install or a program: none of its modules can.
#[test]
fn no_network_install_or_process_from_the_idle_path() {
    let src = [
        include_str!("../src/learner.rs"),
        include_str!("../src/readers.rs"),
        include_str!("../src/paths.rs"),
        include_str!("../src/store.rs"),
        include_str!("../src/scheduler.rs"),
        include_str!("../src/runner.rs"),
        include_str!("../src/classify.rs"),
        include_str!("../src/dedupe.rs"),
        include_str!("../src/extract.rs"),
        include_str!("../src/audit.rs"),
        include_str!("../src/hygiene.rs"),
    ];
    for (i, s) in src.iter().enumerate() {
        // Production code only (tests may use a loopback listener / pid).
        let s = s.split("#[cfg(test)]").next().unwrap();
        for bad in [
            "process::Command",
            "std::process",
            "Command::new",
            "TcpStream",
            "UdpSocket",
            "reqwest",
            "http://",
            "https://",
            "LlamaServer",
        ] {
            assert!(!s.contains(bad), "module {i} contains {bad}");
        }
    }
    // The crate pulls no network stack of its own.
    let toml = include_str!("../Cargo.toml");
    for bad in ["reqwest", "hyper", "ureq", "curl", "arti", "tor"] {
        assert!(
            !toml.lines().any(|l| l.trim_start().starts_with(bad)),
            "Cargo.toml depends on {bad}"
        );
    }
    // A step with a fake model makes no connection at all (the fake has
    // none to make) and still learns.
    let d = tmp("nonet");
    let brain = Brain::open_in_memory(&MemoryConfig::default()).unwrap();
    let mut l = learner(&d, brain);
    let mut sum = FakeSummarizer::with(&[REPLY]);
    assert_eq!(l.step(&mut sum, &|| false).outcome, Outcome::Wrote);
    assert_eq!(sum.calls, 1, "exactly one model call per step");
    let _ = std::fs::remove_dir_all(d);
}
