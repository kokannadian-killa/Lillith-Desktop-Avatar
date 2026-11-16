use super::*;
use crate::{Ctx, ForgetCmd, MemoryConfig};
use lillith_core::Event;

const DAY: f64 = 86_400.0;
/// A Monday, 00:00 UTC.
const MON: f64 = 1_792_368_000.0;
const NOW: f64 = MON + 100.0 * DAY + 12.0 * 3600.0;

fn open(evenings: &[(f64, &str)]) -> Brain {
    let b = Brain::open_in_memory(&MemoryConfig::default()).unwrap();
    for (t, app) in evenings {
        b.observe_event(&Event::with_detail("window.opened.app", *app), &Ctx::at(*t));
    }
    b
}

fn count(b: &Brain, sql: &str) -> i64 {
    b.conn
        .as_ref()
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

/// Steam every evening 70–79 days ago and in the last 10 days, firefox
/// once long ago.
fn history() -> Vec<(f64, &'static str)> {
    let mut v = Vec::new();
    for d in (21..31).chain(90..100) {
        v.push((MON + d as f64 * DAY + 20.0 * 3600.0, "steam"));
    }
    v.push((MON + 22.0 * DAY + 9.0 * 3600.0, "firefox"));
    v
}

#[test]
fn old_events_become_day_digests_and_recent_ones_and_predictions_stay() {
    let b = open(&history());
    let ctx = Ctx::at(NOW);
    let preds = b.predict(&ctx, 5);
    let events = count(&b, "SELECT count(*) FROM events");
    let others = count(
        &b,
        "SELECT count(*) FROM nodes WHERE key NOT LIKE 'digest:%'",
    );
    let r = b.compact(NOW, &|| false).unwrap();
    assert_eq!(r.days, 10, "{r:?}");
    assert_eq!(r.rows_merged, 11);
    assert!(!r.stopped);
    assert_eq!(count(&b, "SELECT count(*) FROM events"), events - 11);
    assert_eq!(
        count(
            &b,
            "SELECT count(*) FROM events WHERE ts < 1792368000 + 61 * 86400"
        ),
        0
    );
    assert_eq!(
        count(
            &b,
            "SELECT count(*) FROM nodes WHERE key NOT LIKE 'digest:%'"
        ),
        others,
        "nothing but digests is added or removed"
    );
    let detail: String = b
        .conn
        .as_ref()
        .unwrap()
        .query_row(
            "SELECT detail FROM nodes WHERE kind = 'event' AND key = ?1",
            [format!("digest:day:{}", ymd((MON / DAY) as i64 + 22))],
            |r| r.get(0),
        )
        .unwrap();
    assert!(detail.contains("steam (app) ×1"), "{detail}");
    assert!(detail.contains("firefox (app) ×1"), "{detail}");
    assert!(
        count(
            &b,
            "SELECT count(*) FROM edges e JOIN nodes n ON n.id = e.dst
             WHERE e.relation = 'digest_of' AND n.key = 'app:steam'"
        ) >= 10
    );
    // The 60-day habit window never saw those events: same predictions.
    assert_eq!(b.predict(&ctx, 5), preds);
    // A second run has nothing left to do.
    assert_eq!(b.compact(NOW, &|| false).unwrap().rows_merged, 0);
}

#[test]
fn digests_are_searchable_memories_and_forget_reaches_them() {
    let b = open(&history());
    b.compact(NOW, &|| false).unwrap();
    let hits = count(
        &b,
        "SELECT count(*) FROM nodes_fts WHERE nodes_fts MATCH 'firefox'",
    );
    assert!(hits >= 1);
    b.forget(&ForgetCmd::Topic("firefox".into()), &Ctx::at(NOW))
        .unwrap();
    assert_eq!(
        count(
            &b,
            "SELECT count(*) FROM nodes WHERE key LIKE 'digest:%' AND detail LIKE '%firefox%'"
        ),
        0,
        "forgetting a topic deletes the digests that mention it"
    );
    assert!(count(&b, "SELECT count(*) FROM nodes WHERE key LIKE 'digest:%'") >= 9);
}

#[test]
fn a_stop_leaves_finished_days_done_and_the_rest_untouched() {
    let b = open(&history());
    let events = count(&b, "SELECT count(*) FROM events");
    let r = b.compact(NOW, &|| true).unwrap();
    assert!(r.stopped);
    assert_eq!(r.rows_merged, 0);
    assert_eq!(count(&b, "SELECT count(*) FROM events"), events);
    // Stop after three days: three digested, the rest wait.
    let n = std::cell::Cell::new(0);
    let r = b
        .compact(NOW, &|| {
            n.set(n.get() + 1);
            n.get() > 3
        })
        .unwrap();
    assert!(r.stopped);
    assert_eq!(r.days, 3);
    let r = b.compact(NOW, &|| false).unwrap();
    assert_eq!(r.days, 7, "resumes where it stopped");
}

#[test]
fn memory_off_compacts_nothing() {
    let cfg = MemoryConfig {
        enabled: false,
        ..MemoryConfig::default()
    };
    let b = Brain::open_in_memory(&cfg).unwrap();
    assert_eq!(b.compact(NOW, &|| false).unwrap(), CompactReport::default());
}

#[test]
fn the_audit_line_has_counts_only() {
    let r = CompactReport {
        days: 2,
        rows_merged: 7,
        bytes_saved: 4096,
        stopped: false,
    };
    assert_eq!(
        audit_line(0.0, &r),
        "1970-01-01T00:00:00Z\tCOMPACT\trows_merged=7\tbytes_saved=4096\n"
    );
    let d = std::env::temp_dir().join(format!("lillith-compact-{}", std::process::id()));
    let log = d.join("audit.log");
    record(&log, 0.0, &r).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(&log).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600);
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn digest_text_lists_the_biggest_first_and_counts_the_rest() {
    let mut m = BTreeMap::new();
    m.insert(("app".to_string(), "steam".to_string()), 3);
    m.insert(("app".to_string(), "kate".to_string()), 5);
    assert_eq!(
        digest_detail("2026-09-01", &m),
        "2026-09-01: 8 events. kate (app) ×5, steam (app) ×3"
    );
    for i in 0..(MAX_SUBJECTS + 2) {
        m.insert(("app".to_string(), format!("a{i:02}")), 1);
    }
    // 28 subjects, 24 listed: the 4 smallest are "others".
    assert!(digest_detail("d", &m).ends_with("others ×4"));
}
