//! A failed DB step never leaves the knowledge folder out of step with the
//! index: a new note's file is removed and a merged note's file is put
//! back exactly as it was before the merge.

use lillith_core::config::Config;
use lillith_learn::learner::Outcome;
use lillith_learn::readers::DocRoots;
use lillith_learn::{FakeSummarizer, Learner};
use lillith_memory::{Brain, MemoryConfig};
use std::io::Write;
use std::path::{Path, PathBuf};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lillith-rb-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn man(d: &Path, text: &str) {
    std::fs::create_dir_all(d.join("man/man8")).unwrap();
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(text.as_bytes()).unwrap();
    std::fs::write(d.join("man/man8/pacman.8.gz"), e.finish().unwrap()).unwrap();
}

fn learner(d: &Path, brain: Brain) -> Learner {
    Learner::new(&Config::default(), brain)
        .with_docs(DocRoots {
            man: vec![d.join("man")],
            ..DocRoots::default()
        })
        .with_knowledge_dir(Some(d.join("knowledge")))
        .with_audit_log(Some(d.join("audit.log")))
}

/// Makes every `notes` write of `when` ("INSERT" or "UPDATE") fail, from a
/// second connection, the way a full disk or a locked DB would.
fn break_notes(db: &Path, when: &str) {
    let c = rusqlite::Connection::open(db).unwrap();
    c.execute_batch(&format!(
        "CREATE TRIGGER break_notes BEFORE {when} ON notes
         BEGIN SELECT RAISE(ABORT, 'injected failure'); END;"
    ))
    .unwrap();
}

fn audit_ops(d: &Path) -> Vec<String> {
    std::fs::read_to_string(d.join("audit.log"))
        .unwrap_or_default()
        .lines()
        .map(|l| l.split('\t').nth(2).unwrap_or("").to_string())
        .collect()
}

#[test]
fn failed_merge_restores_the_old_note_file() {
    let d = tmp("merge");
    let db = d.join("memory.db");
    man(
        &d,
        ".SH NAME\npacman \\- package manager utility\n.SH DESCRIPTION\nPacman manages packages on Garuda.\n",
    );
    let brain = Brain::open_path(&db, &MemoryConfig::default()).unwrap();
    let mut l = learner(&d, brain);
    let mut sum = FakeSummarizer::with(&[
        "{\"kind\":\"howto\",\"title\":\"Remove orphan packages with pacman\",\"body\":\"On Garuda, pacman -Rns $(pacman -Qtdq) removes orphans.\",\"links\":[]}",
    ]);
    let rep = l.step(&mut sum, &|| false);
    assert_eq!(rep.outcome, Outcome::Wrote, "{rep:?}");
    assert_eq!(rep.written.len(), 1);
    let row = l.brain().all_notes().pop().unwrap();
    let file = d.join("knowledge").join(&row.path);
    let original = std::fs::read(&file).unwrap();

    // The next merge's DB update fails.
    break_notes(&db, "UPDATE");
    man(
        &d,
        ".SH NAME\npacman \\- package manager utility, updated\n",
    );
    let mut sum = FakeSummarizer::with(&[
        "{\"kind\":\"howto\",\"title\":\"remove orphan packages with Pacman!\",\"body\":\"Check with pacman -Qtd first.\",\"links\":[]}",
    ]);
    let rep = l.step(&mut sum, &|| false);
    assert!(rep.written.is_empty(), "the merge must not count: {rep:?}");

    // The file holds exactly the pre-merge bytes, with no tmp file left.
    assert_eq!(std::fs::read(&file).unwrap(), original);
    let text = String::from_utf8(original).unwrap();
    assert!(!text.contains("pacman -Qtd first"));
    assert!(!file.with_extension("md.tmp").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // The index still has the one old note, and the failure was audited.
    let notes = l.brain().all_notes();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].content_hash, row.content_hash);
    assert_eq!(audit_ops(&d), ["write", "fail"]);
    // The audit lines carry ids only, never note text (old or merged).
    let log = std::fs::read_to_string(d.join("audit.log")).unwrap();
    for t in ["pacman -Qtd first", "pacman -Rns", "orphan", "Garuda"] {
        assert!(!log.contains(t), "note text in the audit log: {log}");
    }
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn failed_new_write_removes_the_file() {
    let d = tmp("write");
    let db = d.join("memory.db");
    man(&d, ".SH NAME\npacman \\- package manager utility\n");
    let brain = Brain::open_path(&db, &MemoryConfig::default()).unwrap();
    break_notes(&db, "INSERT");
    let mut l = learner(&d, brain);
    let mut sum = FakeSummarizer::with(&[
        "{\"kind\":\"fact\",\"title\":\"What pacman is\",\"body\":\"pacman is the Arch package manager.\",\"links\":[]}",
    ]);
    let rep = l.step(&mut sum, &|| false);
    assert!(rep.written.is_empty(), "{rep:?}");
    assert!(l.brain().all_notes().is_empty());
    let left: Vec<_> = std::fs::read_dir(d.join("knowledge"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_type().unwrap().is_dir())
        .flat_map(|t| std::fs::read_dir(t.path()).unwrap().flatten())
        .map(|e| e.file_name())
        .collect();
    assert!(left.is_empty(), "no note file left behind: {left:?}");
    assert_eq!(audit_ops(&d), ["fail"]);
    let _ = std::fs::remove_dir_all(d);
}
