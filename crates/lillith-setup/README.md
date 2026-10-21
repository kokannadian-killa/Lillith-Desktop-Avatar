# lillith-setup

UI-agnostic core of Lillith's first-run LLM setup wizard (PLAN.md §5.2, M6):
runtime detection, GGUF scanner + picker data, Ollama Modelfile customizer,
and the `[llm]` table of `~/.config/lillith/config.toml`.

## Local-only guarantee

Lily's brain runs on this computer; chats and models never leave the local disk.
Enforced in code (see `src/local.rs`):

- The only network I/O is a std-only HTTP/1.0 GET client used for health and
  model-list probes. It only accepts loopback URLs (`127.0.0.0/8`, `::1`,
  `localhost` → 127.0.0.1 with no DNS lookup). No TLS, no proxies, no redirects.
- `server_url` and `ollama_host` are validated on read **and** write; a remote host
  is a clear error (`... is not on this computer. Lillith is local-only ...`) and
  nothing is written.
- No HTTP-library dependency, no telemetry, no update checks, no model downloads,
  no `ollama pull`. `ollama create` is refused unless `FROM`/`ADAPTER` are existing
  local files (a registry name would make Ollama download it), and the child's
  `OLLAMA_HOST` is pinned to loopback. The llama-server command always binds loopback.

Dependencies: `serde`, `serde_json`, `thiserror`, `toml_edit`.

## CLI (before the GUI lands)

```sh
cargo run -p lillith-setup --example wizard -- scan                 # detect + scan + ranking + launch line
cargo run -p lillith-setup --example wizard -- scan --json
cargo run -p lillith-setup --example wizard -- detect [--bin-dir DIR]
cargo run -p lillith-setup --example wizard -- modelfile [--gguf P] [--out FILE]
cargo run -p lillith-setup --example wizard -- write-config [--gguf P] [--backend llama-server|ollama] \
      [--config FILE] [--modelfile FILE] [--sync-chat]
cargo run -p lillith-setup --example wizard -- launch-cmd
cargo run -p lillith-setup --example wizard -- check-modelfile FILE
cargo run -p lillith-setup --example wizard -- ollama-create        # only if ollama is installed
```

Extra scan dirs: `--dir DIR` (repeatable) or `[llm] scan_paths`.

## `[llm]` keys

| Key | Default | Meaning |
|---|---|---|
| `backend` | `"llama-server"` | `"llama-server"` or `"ollama"` |
| `gguf_path` | `""` | Chosen `.gguf` (absolute path) |
| `server_url` | `"http://127.0.0.1:8080"` | llama-server base URL, loopback only (same form as `[chat] endpoint`) |
| `ollama_host` | `"http://127.0.0.1:11434"` | Ollama base URL, loopback only |
| `ollama_model` | `"lily"` | Name for `ollama create` |
| `temperature` | `0.85` | 0–2 (Lily's default, `LILY_TEMPERATURE`) |
| `ctx_size` | `4096` | `-c` / `num_ctx` (old Aether key `num_ctx` is read and migrated) |
| `chat_template` | `"chatml"` | llama-server `--chat-template` (`chatml`, `llama3`, `gemma`, `phi3`, `mistral-v1`) |
| `llama_server_bin` | `""` | Path to llama-server (empty = PATH) |
| `modelfile_path` | `""` | Lily's Modelfile (default `~/.config/lillith/Modelfile.lily`) |
| `system_prompt` | `""` | Empty = built-in Lily persona |
| `scan_paths` | `[]` | Extra GGUF scan dirs |
| `threads` | `0` | llama-server `-t` / `num_thread` (0 = auto) |

`--sync-chat` / `LlmConfig::sync_chat_tables` also points `[chat]` (`endpoint`, `model`,
`temperature`, `system_prompt` if set) and `[chat.server]` (`binary`, `model_path`,
`ctx_size`, `threads`, `extra_args = ["--chat-template", …]`) from
`feat/chat-bubble-llama` at the same choice. Other keys are kept.

## Lily's persona and sampling

`modelfile::LILY_SYSTEM_PROMPT` (used by `lily_modelfile`, `wizard::modelfile_for` and
`check::test_chat` when `[llm] system_prompt` is empty): Lily is an ancient, adult succubus
and Ko's private companion on his local-only model: seductive, teasing and thirsty, and
explicit with the user when he leads there. Hard limits stay in the prompt: consenting
adults only, never anyone young-seeming, nothing non-consensual, and she stops when told
to. Short bullet rules plus two example lines, tuned for a ~3B model; replies are 1–3 short
sentences.

Default Modelfile sampling (`LilyParams::default()`): `temperature 0.85`
(`LILY_TEMPERATURE`), `top_p 0.9` (`LILY_TOP_P`), `repeat_penalty 1.1`
(`LILY_REPEAT_PENALTY`), `num_predict 160` (`LILY_NUM_PREDICT`), `num_ctx 4096`, and the
chat template's stop tokens (ChatML: `<|im_start|>`, `<|im_end|>`).

## GUI helpers (additive, for the settings panel)

- `detect::spawn(DetectOptions) -> DetectHandle` (`poll() -> Option<&RuntimeReport>`, `try_recv()`, `is_done()`, `wait()`); `Send`.
- `scan::scan_with_progress(&ScanOptions, &CancelToken, FnMut(ScanProgress)) -> ScanReport`;
  `scan::spawn_scan(ScanOptions) -> ScanHandle` (`progress()`, `try_recv()`, `cancel()`). `scan()` is unchanged.
- `check` (alias `test`): `ping_server(&LlmConfig) -> Result<ServerHealth, CheckError>`,
  `test_chat(&LlmConfig, prompt) -> Result<String, CheckError>`, `test_chat_with(.., &ChatTestOptions)`.
  llama-server `/health` + `/props` + `/v1/chat/completions`; Ollama `/api/version` + `/api/tags` + `/api/show` + `/api/chat` (stream=false). Loopback only.
- `validate::field(key, value, &ValidateCtx) -> Result<(), FieldError{key, message}>`, `validate_all(&LlmConfig)`, `validate_all_with(.., &ValidateCtx)`.
- `http::post_json` (loopback-only, request body capped at 256 KB).
- Example: `wizard check [--config F] [--server-url U] [PROMPT]`.
