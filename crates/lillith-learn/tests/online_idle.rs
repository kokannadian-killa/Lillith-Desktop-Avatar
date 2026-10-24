//! The idle learner never starts an online trip, even with
//! `[learn] online = true` (its own test binary so the process-wide trip
//! counter is not shared with the trip tests).

use lillith_core::config::Config;
use lillith_learn::readers::DocRoots;
use lillith_learn::{online, FakeSummarizer, Learner};
use lillith_memory::{Brain, MemoryConfig};

#[test]
fn idle_path_starts_no_online_trip() {
    let d = std::env::temp_dir().join(format!("lillith-online-idle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("docs")).unwrap();
    std::fs::write(
        d.join("docs/hooks.txt"),
        "Pacman runs hook files before and after a transaction.\n",
    )
    .unwrap();
    let mut cfg = Config::default();
    cfg.learn.online = true;
    assert_eq!(online::gate(&cfg), Ok(()), "online is fully allowed here");
    let brain = Brain::open_in_memory(&MemoryConfig::default()).unwrap();
    let mut l = Learner::new(&cfg, brain)
        .with_docs(DocRoots {
            doc: vec![d.join("docs")],
            ..DocRoots::default()
        })
        .with_knowledge_dir(Some(d.join("knowledge")))
        .with_audit_log(Some(d.join("audit.log")));
    let reply = "{\"kind\":\"fact\",\"title\":\"Pacman hooks\",\"body\":\"Pacman runs hook files around a transaction.\",\"links\":[]}";
    let mut sum = FakeSummarizer::with(&[reply, reply, reply]);
    for _ in 0..3 {
        let _ = l.step(&mut sum, &|| false);
    }
    assert_eq!(
        online::trips_started(),
        0,
        "the idle path must never start a trip"
    );
    let log = std::fs::read_to_string(d.join("audit.log")).unwrap_or_default();
    assert!(!log.contains("online_research"), "{log}");
    let _ = std::fs::remove_dir_all(&d);

    // And no idle-path module even knows the trip engine exists.
    for (name, src) in [
        ("learner", include_str!("../src/learner.rs")),
        ("runner", include_str!("../src/runner.rs")),
        ("scheduler", include_str!("../src/scheduler.rs")),
        ("readers", include_str!("../src/readers.rs")),
    ] {
        for bad in [
            "online::",
            "OnlineResearch",
            "Fetcher",
            "TorFetcher",
            "TripRequest",
        ] {
            assert!(!src.contains(bad), "{name} mentions {bad}");
        }
    }
}
