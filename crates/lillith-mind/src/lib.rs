//! # lillith-mind
//!
//! The memory/LLM half of the bridge (PLAN.md §3.3, M7):
//!
//! - [`memory`]: rusqlite port of `lillith_memory_sqlite.py` (same schema,
//!   DB at `~/.local/share/lillith/lillith_memory.db`), regex fact
//!   extraction, warmth → greeting style.
//! - [`llm`]: `LlmBackend` trait with Ollama and OpenAI-compatible
//!   `llama-server` implementations over `reqwest` (streaming).
//! - [`tools`]: read-only system tools (summary, disk, top procs, …) with
//!   an approval gate for anything else (README-DK model).

pub mod llm;
pub mod memory;
pub mod tools;
