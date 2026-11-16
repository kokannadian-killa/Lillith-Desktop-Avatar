//! Dream learning (P7, docs/SELF_LEARNING.md §11/§12) for the app.
//!
//! Every surface only **proposes**: the right-click "Dream learning…"
//! item, `/dream [duration]` in chat and `lillith dream [duration]` all open
//! the same Soul Contract card (`lillith contract-card`, a separate
//! process) and the session starts on a mouse click on its Yes, nothing
//! else (a typed or spoken yes is refused by `propose_session`).
//!
//! - [`register`]: the process's [`lillith_core::dream`] hooks (overlay,
//!   console chat). A session runs on its own thread with the global
//!   [`DreamControl`]; "End dream early", `/dream end`, `/dream stop`, the
//!   kill switch and Ctrl-C hard-stop it.
//! - [`register_bubble`]: the chat-bubble process forwards `/dream …` to the
//!   overlay as `chat.dream_*` events (see `chat::pump`); its status comes
//!   from the status file.
//! - [`cli`]: `lillith dream [duration]` in the foreground.
//!
//! The parent does every GET over Tor; helper workers are this binary as
//! `lillith learn-worker` (no network, seccomp no-exec, bubblewrap).

use lillith_core::config::Config;
use lillith_core::dream::{self as hooks, Dispatch, Hooks};
use lillith_learn::online::{CardProcessConsent, TorFetcher};
use lillith_learn::workers::dream::{duration_range, ClickSessionCard};
use lillith_learn::workers::orchestrator::{global, power_probe, Announce};
use lillith_learn::workers::process::WorkerCommand;
use lillith_learn::workers::{
    propose_session, read_status_file, CuriosityQueue, DreamEvent, DreamReport, DreamRun, NoteSink,
    Session, Supervisor,
};
use std::sync::Arc;

/// Most recent requests read from memory for the curiosity queue.
const CURIOSITY_READ: usize = 64;

/// The worker subcommand (dispatched before the async runtime starts: the
/// seccomp filter is per thread).
pub const WORKER_ARG: &str = "learn-worker";

/// The session length for `minutes` (`None`: the default, inside the
/// `[learn.dream]` range).
pub fn minutes_for(cfg: &Config, minutes: Option<u64>) -> u64 {
    let (lo, hi) = duration_range(cfg);
    minutes.unwrap_or_else(|| hooks::default_minutes(lo, hi))
}

/// Registers this process's dream hooks. `announce` gets her lines
/// (speech in the overlay, the terminal in console chat).
pub fn register(cfg: &Config, announce: Announce) {
    let cfg = Arc::new(cfg.clone());
    hooks::set_hooks(Hooks {
        open: Arc::new(move |m| open(&cfg, m, announce.clone())),
        end_early: Arc::new(|| ended(global().end_early())),
        stop: Arc::new(|| ended(global().stop())),
        status: Arc::new(|| global().status().line()),
    });
}

fn ended(asked: bool) -> Dispatch {
    if asked {
        Dispatch::Ending
    } else {
        Dispatch::NotRunning
    }
}

/// Opens the card on a thread (one dream at a time).
fn open(cfg: &Arc<Config>, minutes: Option<u64>, announce: Announce) -> Dispatch {
    if !global().try_propose() {
        return Dispatch::Busy;
    }
    let cfg = cfg.clone();
    let spawned = std::thread::Builder::new()
        .name("lillith-dream".into())
        .spawn(move || match run_session(&cfg, minutes, announce) {
            Ok(r) => tracing::info!(session = %r.session, reason = r.reason.word(), "dream over"),
            Err(e) => tracing::info!("no dream: {e}"),
        });
    match spawned {
        Ok(_) => Dispatch::CardOpening,
        Err(e) => {
            global().proposal_refused(&format!("no thread: {e}"));
            Dispatch::Status(format!("I couldn't start the dream card ({e})."))
        }
    }
}

/// Card → session → orchestration, blocking. The caller has already
/// claimed the proposal ([`DreamControl::try_propose`]); every refusal
/// hands it back.
///
/// [`DreamControl::try_propose`]: lillith_learn::workers::DreamControl::try_propose
pub fn run_session(
    cfg: &Config,
    minutes: Option<u64>,
    announce: Announce,
) -> Result<DreamReport, String> {
    let control = global().clone();
    let refuse = |why: String| {
        control.proposal_refused(&why);
        Err(why)
    };
    let minutes = minutes_for(cfg, minutes);
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return refuse(format!("can't find my own binary: {e}")),
    };
    let fetcher = TorFetcher::from_config(&cfg.learn);
    // Opened on this thread; read-only for the identity denylist, then the
    // sink's index for dream notes.
    let brain = if cfg.memory.enabled {
        lillith_memory::Brain::open(&cfg.memory)
            .map_err(|e| tracing::warn!("dream: memory unavailable ({e}); notes only"))
            .ok()
    } else {
        None
    };
    let names = brain
        .as_ref()
        .map(lillith_memory::Brain::person_names)
        .unwrap_or_default();
    let worker = WorkerCommand::sandboxed(exe, vec![WORKER_ARG.into()]);
    let mut sup = match Supervisor::new(cfg, &fetcher, &names, worker) {
        Ok(s) => s,
        Err(e) => return refuse(format!("helpers are off: {e:?}")),
    };
    let Some(card) = CardProcessConsent::current_exe() else {
        return refuse("no card program".into());
    };
    let audit = cfg.mind.audit_log_path();
    let approval = match propose_session(cfg, minutes, &ClickSessionCard(&card), audit.clone()) {
        Ok(a) => a,
        Err(e) => return refuse(e.to_string()),
    };
    let session = match Session::from_card(approval) {
        Ok(s) => s,
        Err(e) => return refuse(e.to_string()),
    };
    // Curiosity first (what Ko asked about lately; names / blocklist
    // terms / personal topics never enter), then the `[learn] topics`
    // rotation. Mentions only pick fixed public pages.
    let deny = lillith_learn::online::IdentityDenylist::for_requests(
        &names,
        cfg.learn.dream.blocklist_terms.terms(),
    );
    let mentions = brain
        .as_ref()
        .map(|b| b.recent_requests(&lillith_memory::Ctx::now(), CURIOSITY_READ))
        .unwrap_or_default();
    let mut planner = CuriosityQueue::from_mentions(mentions, &deny).planner(cfg);
    let kdir = cfg.mind.resolved_knowledge_dir();
    let mut sink = NoteSink::new(cfg, kdir.as_deref(), brain, audit.clone());
    hooks::set_active(true);
    let report = DreamRun::new(cfg, &mut sup, &mut planner, &mut sink, control, announce)
        .with_audit_log(audit)
        .with_conditions(power_probe(cfg))
        .run(session);
    hooks::set_active(false);
    if cfg.learn.dream.summarize && !report.saved.is_empty() {
        digest(cfg, &report, global());
    }
    Ok(report)
}

/// The optional after-session digest from the local model (loopback only,
/// never waits for chat, no model means no digest).
fn digest(cfg: &Config, report: &DreamReport, control: &lillith_learn::workers::DreamControl) {
    let Some(gate) = lillith_learn::LlmGate::shared() else {
        return;
    };
    let mut sum = match lillith_learn::LlmSummarizer::new(&cfg.chat, gate) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("dream digest off: {e:?}");
            return;
        }
    };
    let epoch = lillith_core::access::kill_switch::epoch();
    let stop = || lillith_core::access::kill_switch::stopped_since(epoch);
    match lillith_learn::workers::digest::summarize_session(&report.saved, &mut sum, &stop) {
        Ok(text) => control.set_digest(text),
        Err(why) => tracing::debug!("no dream digest: {why:?}"),
    }
}

/// The chat bubble (its own process): `/dream …` is forwarded to the
/// overlay, which owns the dream; status comes from the status file.
pub fn register_bubble(emit: lillith_panel::chat_bubble::Emit) {
    let running = || bubble_busy(read_status_file().as_ref());
    let (e1, e2, e3) = (emit.clone(), emit.clone(), emit);
    hooks::set_hooks(Hooks {
        open: Arc::new(move |m| {
            if running() {
                return Dispatch::Busy;
            }
            let detail = m.map(|m| m.to_string());
            e1(hooks::OPEN_EVENT, detail.as_deref());
            Dispatch::CardOpening
        }),
        end_early: Arc::new(move || {
            if !running() {
                return Dispatch::NotRunning;
            }
            e2(hooks::END_EARLY_EVENT, None);
            Dispatch::Ending
        }),
        stop: Arc::new(move || {
            if !running() {
                return Dispatch::NotRunning;
            }
            e3(hooks::STOP_EVENT, None);
            Dispatch::Ending
        }),
        status: Arc::new(|| {
            read_status_file()
                .map(|s| s.line())
                .unwrap_or_else(|| "I'm not dreaming.".into())
        }),
    });
}

/// Whether the bubble treats the overlay's dream as live (a card up or a
/// session running / ending). The status file reader already turns a
/// crashed owner's leftover live state into `Ended`
/// ([`lillith_learn::workers::read_status_file_at`]), so a stale file never
/// blocks a new card; an owner it can't check stays live (fail closed).
fn bubble_busy(s: Option<&lillith_learn::workers::DreamStatus>) -> bool {
    s.is_some_and(|s| {
        matches!(
            s.state,
            lillith_learn::workers::DreamState::Running
                | lillith_learn::workers::DreamState::Proposed
                | lillith_learn::workers::DreamState::Ending
        )
    })
}

/// In the overlay: a `chat.dream_*` event from the bubble. `true` when it
/// was one (handled here, never forwarded).
pub fn on_bubble_event(name: &str, detail: Option<&str>) -> bool {
    let d = match name {
        hooks::OPEN_EVENT => hooks::open_card(detail.and_then(|d| d.trim().parse().ok())),
        hooks::END_EARLY_EVENT => hooks::end_early(),
        hooks::STOP_EVENT => hooks::stop(),
        _ => return false,
    };
    tracing::info!(event = name, result = ?d, "dream (from the chat bubble)");
    true
}

/// Her lines as overlay speech (`chat.start` / `chat.token` / `chat.end`
/// on the feed) plus the log. Only the start, where she's going and the
/// summary are spoken; the rest is in the log, the audit log and
/// [`lillith_learn::workers::dream_status`].
pub fn speech_announce(feed: std::sync::mpsc::Sender<lillith_core::Event>) -> Announce {
    let feed = std::sync::Mutex::new(feed);
    Arc::new(move |e: &DreamEvent| {
        let line = e.line();
        tracing::info!(event = e.name(), "{line}");
        let spoken = matches!(
            e,
            DreamEvent::Started { .. } | DreamEvent::Going { .. } | DreamEvent::Ended { .. }
        );
        if !spoken {
            return;
        }
        if let Ok(f) = feed.lock() {
            let _ = f.send(lillith_core::Event::new("chat.start"));
            let _ = f.send(lillith_core::Event::with_detail("chat.token", line));
            let _ = f.send(lillith_core::Event::new("chat.end"));
        }
    })
}

/// Her lines on the terminal.
pub fn print_announce() -> Announce {
    Arc::new(|e: &DreamEvent| println!("[dream] {}", e.line()))
}

/// `lillith dream [DURATION]`: the card, then the session in the
/// foreground. Ctrl-C pulls the kill switch (the session and its workers
/// stop) before exiting.
pub fn cli(cfg: &Config, duration: Option<&str>) -> anyhow::Result<()> {
    let minutes = match duration {
        None => None,
        Some(d) => Some(
            hooks::parse_duration(d)
                .ok_or_else(|| anyhow::anyhow!("can't read {d:?}: try 2h, 90m or 1h30m"))?,
        ),
    };
    let (lo, hi) = duration_range(cfg);
    let m = minutes_for(cfg, minutes);
    println!("Opening the dream card ({m} min; {lo}–{hi} allowed). Click Yes on it to start — typing yes won't.");
    if !global().try_propose() {
        anyhow::bail!("a dream is already running or its card is up");
    }
    let report = run_session(cfg, Some(m), print_announce()).map_err(|e| anyhow::anyhow!(e))?;
    println!(
        "{} jobs, {} pages, {} notes saved, {} quarantined.",
        report.jobs,
        report.visited.len(),
        report.saved.len(),
        report.quarantined
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_length_is_in_range_and_explicit_ones_pass_through() {
        let cfg = Config::default();
        let (lo, hi) = duration_range(&cfg);
        let d = minutes_for(&cfg, None);
        assert!((30..=480).contains(&d) && (lo..=hi).contains(&d), "{d}");
        assert_eq!(minutes_for(&cfg, Some(90)), 90);
    }

    #[test]
    fn bubble_events_are_handled_not_forwarded() {
        assert!(!on_bubble_event("chat.token", Some("hi")));
        // No hooks in this test process's overlay role: NotWired, handled.
        hooks::clear_hooks();
        assert!(on_bubble_event(hooks::END_EARLY_EVENT, None));
    }

    /// After an overlay crash mid-dream its `dream-status.json` still says
    /// `running`: the bubble must not answer `/dream` with "busy" (nor the
    /// Bots tab show it running), while a live session still is busy.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_crashed_overlays_status_file_does_not_block_a_new_card() {
        use lillith_learn::workers::{read_status_file_at, DreamControl, DreamState};
        let dir = std::env::temp_dir().join(format!("lillith-dw-stale-{}", std::process::id()));
        let file = dir.join("dream-status.json");
        let c = DreamControl::with_status_file(Some(file.clone()));
        assert!(c.try_propose());
        assert!(bubble_busy(read_status_file_at(&file).as_ref()), "card up");
        // A live owner (this process) with a running state: busy.
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        v["state"] = "running".into();
        std::fs::write(&file, v.to_string()).unwrap();
        assert!(bubble_busy(read_status_file_at(&file).as_ref()), "live");
        // The owner died (a reaped child's pid): not busy, not running.
        let mut ch = std::process::Command::new("true").spawn().unwrap();
        let dead = ch.id();
        ch.wait().unwrap();
        v["owner_pid"] = dead.into();
        std::fs::write(&file, v.to_string()).unwrap();
        let st = read_status_file_at(&file).unwrap();
        assert_eq!(st.state, DreamState::Ended);
        assert!(!bubble_busy(Some(&st)));
        // No owner recorded and the deadline still ahead: fail closed.
        v.as_object_mut().unwrap().remove("owner_pid");
        v["deadline"] = "2999-01-01T00:00:00Z".into();
        std::fs::write(&file, v.to_string()).unwrap();
        assert!(bubble_busy(read_status_file_at(&file).as_ref()), "unknown");
        assert!(!bubble_busy(None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn speech_is_a_chat_stream_for_the_main_lines() {
        let (tx, rx) = std::sync::mpsc::channel();
        let a = speech_announce(tx);
        a(&DreamEvent::Visited {
            url: "https://a.org/".into(),
        });
        assert!(rx.try_recv().is_err(), "visits are not spoken");
        a(&DreamEvent::Ended {
            reason: lillith_learn::workers::StopReason::EndEarly,
            summary: "Back from dreaming.".into(),
        });
        let got: Vec<String> = rx.try_iter().map(|e| e.name).collect();
        assert_eq!(got, ["chat.start", "chat.token", "chat.end"]);
    }
}
