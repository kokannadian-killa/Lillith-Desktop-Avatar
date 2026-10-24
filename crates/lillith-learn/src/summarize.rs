//! The one model call of a step, behind the shared [`LlmGate`].
//!
//! [`LlmSummarizer`] only ever `try_lock`s the gate (never waits), refuses
//! when chat asked for the model a moment ago, caps the request
//! (`max_tokens` ≤ [`MAX_TOKENS`], temperature [`TEMPERATURE`], 60 s
//! timeout), and drops its HTTP stream as soon as chat signals `llm.want`
//! or the step is stopped (safeword, activity): dropping the request
//! future closes the connection, so llama-server's slot frees at once. It
//! talks only to the loopback endpoint (`local_base`) and never starts or
//! restarts llama-server.

use crate::{LlmGate, MAX_TOKENS, REQUEST_TIMEOUT_S, TEMPERATURE};
use lillith_mind::llm::{ChatOptions, Flavor, LlmClient, Message};
use std::future::Future;
use std::time::Duration;

/// A finished reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub text: String,
    /// Rough tokens spent (prompt + reply, ~4 chars/token).
    pub tokens: u32,
}

/// Why no reply came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SumError {
    /// Chat holds the model (or just asked for it): try later.
    Busy,
    /// Chat asked for the model mid-reply; the stream was dropped.
    Preempted,
    /// The step was stopped (safeword / activity / quit).
    Stopped,
    /// The server isn't there (never started by the learner).
    Unavailable(String),
    /// Anything else (counts as a failed try).
    Failed(String),
}

/// Something that answers the step's request.
pub trait Summarizer {
    /// Answers `messages`; `stop` is polled while waiting.
    fn summarize(
        &mut self,
        messages: &[Message],
        stop: &dyn Fn() -> bool,
    ) -> Result<Summary, SumError>;
}

/// How often the gate / stop flag are checked while a reply streams.
pub const POLL: Duration = Duration::from_millis(25);
/// Chat asked this recently: don't start.
const RECENT_WANT: Duration = Duration::from_secs(5);

/// Rough token count of `s`.
pub fn est_tokens(s: &str) -> u32 {
    (s.len() as u32).div_ceil(4)
}

/// Runs `fut` until it finishes, chat preempts the `guard`, or `stop()`
/// says so; the future is dropped (its connection closed) on the latter
/// two.
pub async fn run_preemptible<F: Future>(
    fut: F,
    guard: &crate::GateGuard,
    stop: &dyn Fn() -> bool,
) -> Result<F::Output, SumError> {
    tokio::pin!(fut);
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            out = &mut fut => return Ok(out),
            _ = tick.tick() => {
                if guard.preempted() {
                    return Err(SumError::Preempted);
                }
                if stop() {
                    return Err(SumError::Stopped);
                }
            }
        }
    }
}

/// The real model call (loopback llama-server / Ollama).
///
/// Each call gets its own client and current-thread runtime, dropped when
/// the call ends: dropping the runtime drops the HTTP connection task, so a
/// preempted or stopped request closes its socket at once (llama-server
/// stops generating) instead of lingering until the next step.
#[derive(Debug)]
pub struct LlmSummarizer {
    chat: lillith_core::config::ChatConfig,
    gate: LlmGate,
    opts: ChatOptions,
}

impl LlmSummarizer {
    /// A summarizer for the chat endpoint. Refuses a non-loopback endpoint.
    /// Never starts or restarts the model server.
    pub fn new(chat: &lillith_core::config::ChatConfig, gate: LlmGate) -> Result<Self, SumError> {
        lillith_mind::llm::local_base(&chat.endpoint)
            .map_err(|e| SumError::Unavailable(e.to_string()))?;
        Ok(Self {
            chat: chat.clone(),
            gate,
            opts: Self::options(),
        })
    }

    fn client(&self) -> LlmClient {
        let mut client = LlmClient::with_timeout(
            Flavor::OpenAiCompat,
            self.chat.endpoint.clone(),
            self.chat.model.clone(),
            Duration::from_secs(REQUEST_TIMEOUT_S),
        );
        if !self.chat.api_key.is_empty() {
            client.api_key = Some(self.chat.api_key.clone());
        }
        client
    }

    /// The capped learner options.
    pub fn options() -> ChatOptions {
        ChatOptions {
            temperature: TEMPERATURE,
            max_tokens: MAX_TOKENS,
            ..ChatOptions::default()
        }
    }
}

impl Summarizer for LlmSummarizer {
    fn summarize(
        &mut self,
        messages: &[Message],
        stop: &dyn Fn() -> bool,
    ) -> Result<Summary, SumError> {
        if self.gate.wanted_within(RECENT_WANT) {
            return Err(SumError::Busy);
        }
        let guard = match self.gate.try_lock() {
            Ok(Some(g)) => g,
            Ok(None) => return Err(SumError::Busy),
            Err(e) => return Err(SumError::Failed(format!("llm gate: {e}"))),
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| SumError::Failed(e.to_string()))?;
        let client = self.client();
        let opts = self.opts;
        let out = rt.block_on(async {
            run_preemptible(client.chat_stream(messages, opts, |_| {}), &guard, stop).await
        });
        // Close the connection before chat gets the lock.
        drop(client);
        drop(rt);
        drop(guard);
        let out = out?;
        match out {
            Ok(text) => {
                let prompt: usize = messages.iter().map(|m| m.content.len()).sum();
                Ok(Summary {
                    tokens: est_tokens(&text) + (prompt as u32).div_ceil(4),
                    text,
                })
            }
            Err(e) if e.is_down() => Err(SumError::Unavailable(e.to_string())),
            Err(e) => Err(SumError::Failed(e.to_string())),
        }
    }
}

/// A canned summarizer (tests, `--dry-run` without a model).
#[derive(Debug, Default)]
pub struct FakeSummarizer {
    pub replies: std::collections::VecDeque<Result<String, SumError>>,
    pub calls: usize,
}

impl FakeSummarizer {
    pub fn with(replies: &[&str]) -> Self {
        Self {
            replies: replies.iter().map(|r| Ok(r.to_string())).collect(),
            calls: 0,
        }
    }
}

impl Summarizer for FakeSummarizer {
    fn summarize(
        &mut self,
        messages: &[Message],
        stop: &dyn Fn() -> bool,
    ) -> Result<Summary, SumError> {
        self.calls += 1;
        if stop() {
            return Err(SumError::Stopped);
        }
        let text = self
            .replies
            .pop_front()
            .unwrap_or_else(|| Err(SumError::Unavailable("no reply".into())))?;
        let prompt: usize = messages.iter().map(|m| m.content.len()).sum();
        Ok(Summary {
            tokens: est_tokens(&text) + (prompt as u32).div_ceil(4),
            text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lillith-sum-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn options_are_capped() {
        let o = LlmSummarizer::options();
        assert!(o.max_tokens <= 256);
        assert_eq!(o.temperature, 0.2);
    }

    #[test]
    fn non_loopback_endpoint_is_refused_without_connecting() {
        let chat = lillith_core::config::ChatConfig {
            endpoint: "http://example.com:8080".into(),
            ..Default::default()
        };
        let g = LlmGate::at(&tmp("refuse"));
        assert!(matches!(
            LlmSummarizer::new(&chat, g),
            Err(SumError::Unavailable(_))
        ));
    }

    #[test]
    fn preempted_within_200ms_of_want() {
        let d = tmp("pre");
        let gate = LlmGate::at(&d);
        let guard = gate.try_lock().unwrap().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let g2 = gate.clone();
        let signalled = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            g2.signal_want().unwrap();
            Instant::now()
        });
        let res = rt.block_on(async {
            run_preemptible(tokio::time::sleep(Duration::from_secs(30)), &guard, &|| {
                false
            })
            .await
        });
        let done = Instant::now();
        let at = signalled.join().unwrap();
        assert_eq!(res, Err(SumError::Preempted));
        assert!(
            done.duration_since(at) <= Duration::from_millis(200),
            "{:?}",
            done.duration_since(at)
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn stop_flag_ends_the_wait() {
        let d = tmp("stop");
        let gate = LlmGate::at(&d);
        let guard = gate.try_lock().unwrap().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let t0 = Instant::now();
        let res = rt.block_on(async {
            run_preemptible(tokio::time::sleep(Duration::from_secs(30)), &guard, &|| {
                t0.elapsed() > Duration::from_millis(50)
            })
            .await
        });
        assert_eq!(res, Err(SumError::Stopped));
        assert!(t0.elapsed() < Duration::from_millis(300));
        let _ = std::fs::remove_dir_all(d);
    }

    /// End to end against a slow loopback "llama-server": chat's want drops
    /// the learner's HTTP stream (the server sees the connection close)
    /// within 200 ms.
    #[test]
    fn http_stream_is_dropped_when_chat_wants_the_model() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let closed = Arc::new(AtomicBool::new(false));
        let c2 = closed.clone();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = s.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n",
            );
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(20) {
                let ev = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n";
                let chunk = format!("{:x}\r\n{ev}\r\n", ev.len());
                if s.write_all(chunk.as_bytes())
                    .and_then(|_| s.flush())
                    .is_err()
                {
                    c2.store(true, Ordering::SeqCst);
                    return Instant::now();
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Instant::now()
        });
        let d = tmp("http");
        let gate = LlmGate::at(&d);
        let chat = lillith_core::config::ChatConfig {
            endpoint: format!("http://127.0.0.1:{port}"),
            ..Default::default()
        };
        let mut sum = LlmSummarizer::new(&chat, gate.clone()).unwrap();
        let g2 = gate.clone();
        let signalled = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            g2.signal_want().unwrap();
            Instant::now()
        });
        let res = sum.summarize(&[Message::user("hi")], &|| false);
        let returned = Instant::now();
        let at = signalled.join().unwrap();
        assert_eq!(res, Err(SumError::Preempted));
        assert!(returned.duration_since(at) <= Duration::from_millis(200));
        // The lock is free for chat right away.
        assert!(gate.try_lock().unwrap().is_some());
        let server_saw_close = server.join().unwrap();
        assert!(closed.load(Ordering::SeqCst), "connection was dropped");
        assert!(server_saw_close.duration_since(at) <= Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn busy_when_chat_holds_or_just_wanted_the_model() {
        let d = tmp("busy");
        let gate = LlmGate::at(&d);
        let chat = lillith_core::config::ChatConfig {
            endpoint: "http://127.0.0.1:9".into(),
            ..Default::default()
        };
        let mut sum = LlmSummarizer::new(&chat, gate.clone()).unwrap();
        let held = gate.try_lock().unwrap().unwrap();
        assert_eq!(
            sum.summarize(&[Message::user("x")], &|| false),
            Err(SumError::Busy)
        );
        drop(held);
        gate.signal_want().unwrap();
        assert_eq!(
            sum.summarize(&[Message::user("x")], &|| false),
            Err(SumError::Busy)
        );
        let _ = std::fs::remove_dir_all(d);
    }
}
