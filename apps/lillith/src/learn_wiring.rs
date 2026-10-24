//! Wires the idle learner (`lillith-learn`, P1) into the overlay next to
//! the memory thread, and the `lillith learn` command.

use lillith_core::config::Config;
use lillith_core::Event;
use lillith_learn::learner::Outcome;
use lillith_learn::{LearnHandle, Learner, LlmGate, LlmSummarizer};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// Starts the learner when `[learn] enabled` and `[memory] enabled` and the
/// model is allowed (`--no-llm` off, `[chat] enabled`). It opens its own
/// memory connection on its own thread. `None`: nothing started.
pub fn start(cfg: &Config, llm_allowed: bool) -> Option<Arc<LearnHandle>> {
    let h = LearnHandle::spawn(cfg, llm_allowed && cfg.chat.enabled).map(Arc::new);
    tracing::info!(
        on = h.is_some(),
        learn = cfg.learn.enabled,
        memory = cfg.memory.enabled,
        "idle learner"
    );
    h
}

/// A sender that forwards everything to `feed` and shows each event to the
/// learner on the way (activity → pause, `chat.safeword` → stop + cooldown;
/// never blocks). No learner: `feed` itself, no thread.
pub fn tee(feed: Sender<Event>, learner: Option<&Arc<LearnHandle>>) -> Sender<Event> {
    let Some(l) = learner else {
        return feed;
    };
    let (tx, rx) = std::sync::mpsc::channel::<Event>();
    let l = l.clone();
    let out = feed.clone();
    let spawned = std::thread::Builder::new()
        .name("lillith-learn-tee".into())
        .spawn(move || {
            for ev in rx {
                l.observe(&ev);
                if out.send(ev).is_err() {
                    return;
                }
            }
        });
    match spawned {
        Ok(_) => tx,
        Err(e) => {
            tracing::warn!("learn tee: {e}; the learner will not see desktop events");
            feed
        }
    }
}

/// `lillith learn --once [--dry-run]` / `lillith learn --reindex`.
pub fn cli(cfg: &Config, once: bool, dry_run: bool, reindex: bool) -> anyhow::Result<()> {
    if !cfg.memory.enabled {
        println!("memory is off ([memory] enabled = false): nothing to do");
        return Ok(());
    }
    let brain = lillith_memory::Brain::open(&cfg.memory)?;
    if reindex {
        if !cfg.learn.enabled {
            println!("learning is off ([learn] enabled = false): nothing to do");
            return Ok(());
        }
        let mut l = Learner::new(cfg, brain);
        let n = l.reindex()?;
        println!(
            "reindexed {n} notes from {}",
            cfg.mind
                .resolved_knowledge_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_else(|| "-".into())
        );
        return Ok(());
    }
    if !once {
        println!("use `lillith learn --once [--dry-run]` or `lillith learn --reindex`");
        return Ok(());
    }
    if !cfg.learn.enabled {
        println!("learning is off ([learn] enabled = false): nothing to do");
        return Ok(());
    }
    let gate = LlmGate::shared()
        .ok_or_else(|| anyhow::anyhow!("no runtime/data dir for the model lock"))?;
    let mut sum =
        LlmSummarizer::new(&cfg.chat, gate).map_err(|e| anyhow::anyhow!("model: {e:?}"))?;
    let _ = lillith_learn::hygiene::lower_current_thread();
    let epoch = lillith_core::access::kill_switch::epoch();
    let mut l = Learner::new(cfg, brain).dry_run(dry_run);
    let rep = l.step(&mut sum, &|| {
        lillith_core::access::kill_switch::stopped_since(epoch)
    });
    println!(
        "source: {}#{}",
        rep.source.as_deref().unwrap_or("-"),
        rep.chunk
    );
    println!("outcome: {:?} (~{} tokens)", rep.outcome, rep.tokens);
    if dry_run && !rep.chunk_preview.is_empty() {
        println!("chunk: {}…", rep.chunk_preview.replace('\n', " "));
    }
    for it in &rep.items {
        println!("  [{}] {}", it.kind, it.title);
    }
    for w in &rep.written {
        let verb = if rep.outcome == Outcome::DryRun {
            "would"
        } else {
            "did"
        };
        println!("  {verb} {} {}/{} ({})", w.op, w.topic, w.id, w.title);
    }
    Ok(())
}
