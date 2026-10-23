//! # lillith-mind
//!
//! The memory/LLM half of the bridge (PLAN.md §3.3, M7):
//!
//! - [`memory`]: rusqlite port of `lillith_memory_sqlite.py` (same schema,
//!   DB at `~/.local/share/lillith/lillith_memory.db`), regex fact
//!   extraction, warmth → greeting style.
//! - [`llm`]: Ollama and OpenAI-compatible `llama-server` clients over
//!   `reqwest`, with SSE streaming for the latter.
//! - [`chat`]: the click-to-chat conversation (history, persona, graceful
//!   in-character fallbacks); [`persona`]: her system prompt and lines.
//! - [`server`]: optional self-spawned `llama-server` (`[chat.server]`).
//! - [`fstools`]: the scoped local file/tool API (Phase 2) — an allowlist,
//!   a tool-request parser, and a planner that routes mutations through the
//!   Soul Contract.
//! - [`tools`]: read-only system tools (summary, disk, top procs, …) with
//!   an approval gate for anything else (README-DK model).

pub mod chat;
pub mod fstools;
pub mod knowledge;
pub mod llm;
pub mod memory;
pub mod persona;
pub mod server;
pub mod tools;
