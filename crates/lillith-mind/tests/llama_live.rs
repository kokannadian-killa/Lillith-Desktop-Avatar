//! Live round-trip against a real llama-server. Skipped unless
//! `LILLITH_LLAMA_URL` is set, e.g.
//!
//! ```sh
//! llama-server -m qwen2.5-0.5b-instruct-q4_k_m.gguf --port 8080 &
//! LILLITH_LLAMA_URL=http://127.0.0.1:8080 cargo test -p lillith-mind --test llama_live -- --nocapture
//! ```
//!
//! With `LILLITH_LLAMA_BIN` + `LILLITH_LLAMA_GGUF` set instead, the test
//! spawns the server itself through [`LlamaServer`] (the `[chat.server]` path)
//! on port 18089 and checks it is gone afterwards.

use lillith_core::config::{ChatConfig, LlamaServerConfig};
use lillith_mind::chat::{ChatSession, Outcome};
use lillith_mind::server::LlamaServer;
use std::time::Duration;

async fn round_trip(endpoint: &str) {
    let cfg = ChatConfig {
        endpoint: endpoint.into(),
        max_tokens: 60,
        ..ChatConfig::default()
    };
    let mut s = ChatSession::new(&cfg, false);
    let mut tokens = 0usize;
    let mut streamed = String::new();
    let out = s
        .send("Hi Lillith! Introduce yourself in one sentence.", |t| {
            tokens += 1;
            streamed.push_str(t);
        })
        .await;
    println!("reply ({tokens} deltas): {}", out.text());
    match out {
        Outcome::Reply(r) => {
            assert!(tokens > 1, "reply was streamed in pieces");
            assert_eq!(r, streamed.trim());
        }
        Outcome::Fallback { error, .. } => panic!("no reply: {error}"),
    }
    // Second turn carries the history.
    let out2 = s.send("What did I just ask you?", |_| {}).await;
    println!("reply 2: {}", out2.text());
    assert!(matches!(out2, Outcome::Reply(_)));
    assert_eq!(s.history().len(), 4);
}

#[tokio::test]
async fn live_llama_server_round_trip() {
    if let Ok(url) = std::env::var("LILLITH_LLAMA_URL") {
        round_trip(&url).await;
        return;
    }
    let (Ok(bin), Ok(gguf)) = (
        std::env::var("LILLITH_LLAMA_BIN"),
        std::env::var("LILLITH_LLAMA_GGUF"),
    ) else {
        eprintln!("skipped: set LILLITH_LLAMA_URL or LILLITH_LLAMA_BIN + LILLITH_LLAMA_GGUF");
        return;
    };
    let endpoint = "http://127.0.0.1:18089";
    let cfg = ChatConfig {
        endpoint: endpoint.into(),
        server: LlamaServerConfig {
            binary: bin,
            model_path: gguf,
            ctx_size: 2048,
            ..LlamaServerConfig::default()
        },
        ..ChatConfig::default()
    };
    let mut server = LlamaServer::spawn(&cfg).expect("spawn llama-server");
    server
        .wait_ready(endpoint, Duration::from_secs(120))
        .await
        .expect("llama-server ready");
    round_trip(endpoint).await;
    server.shutdown();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !lillith_mind::server::already_running(endpoint).await,
        "server stopped"
    );
}
