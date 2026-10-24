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
| `temperature` | `0.95` | 0–2 (Lily's default, `LILY_TEMPERATURE`) |
| `ctx_size` | `4096` | `-c` / `num_ctx` (old Aether key `num_ctx` is read and migrated) |
| `chat_template` | `"chatml"` | Ollama `TEMPLATE`/stop tokens (`chatml`, `llama3`, `gemma`, `phi3`, `mistral-v1`); llama-server uses the GGUF's own template via `--jinja` |
| `llama_server_bin` | `""` | Path to llama-server (empty = PATH) |
| `modelfile_path` | `""` | Lily's Modelfile (default `~/.config/lillith/Modelfile.lily`) |
| `system_prompt` | `""` | Empty = built-in Lily persona; a custom prompt gets the read-aloud rule appended if missing |
| `scan_paths` | `[]` | Extra GGUF scan dirs |
| `threads` | `0` | llama-server `-t` / `num_thread` (0 = physical cores for llama-server) |

`--sync-chat` / `LlmConfig::sync_chat_tables` also points `[chat]` (`endpoint`, `model`,
`temperature`, `system_prompt` if set) and `[chat.server]` (`binary`, `model_path`,
`ctx_size`, `threads` (physical cores when 0), `extra_args` with `--jinja`; a stale
`--chat-template <name>` pair is removed, other extra args are kept) from
`feat/chat-bubble-llama` at the same choice. Other keys are kept.

## llama-server launch line

`launch::llama_server_command` always produces
`llama-server -m <gguf> --host 127.0.0.1 --port <port> -c 4096 -t <physical cores> --jinja`:
loopback only (a `[::1]` server URL is refused, not mismatched), `-t` from `[llm] threads` or
the physical cores in `/proc/cpuinfo` (distinct physical id/core id pairs; fallback: half the
threads, or `available_parallelism`), and `--jinja` so the model's embedded chat template is
used. There is no `--chat-template <name>`: with `--jinja` (llama.cpp's default) llama-server
reads a name like `llama3` as literal template text and every prompt becomes just `llama3`.

## Model recommendation

`scan::recommend_for(models, &host::HostInfo)` (CPU + RAM injectable; `recommend` detects
the CPU). On a low-power CPU (U-series such as the i5-7200U, or 4 threads or fewer) with
8–16 GB, 3B models get a boost and 7–8B ones a penalty, so
**Dolphin 3.0 Llama 3.2 3B Q4_K_M** (`dolphin3.0-llama3.2-3b-q4_k_m.gguf`) is the pick.
Within a size tier, uncensored fine-tunes (`dolphin`, `uncensored`, `abliterated`, `hermes`,
`nous` in the file name or GGUF `general.name`) rank above stock instruct models. If nothing
qualifies (e.g. only Mistral 7B on 8 GB), the best usable model is kept as a fallback.
Nothing is downloaded: `scan::preferred_model_hint` returns the exact file name and says to
put it in `~/models/` (always in the default scan dirs); the CLI `scan` prints it.

## Lily's persona and sampling

`modelfile::LILY_SYSTEM_PROMPT` (used by `lily_modelfile`, `wizard::modelfile_for`,
`check::test_chat`, and the chat's fallback `lillith_mind::persona::DEFAULT_SYSTEM_PROMPT`):
Lily is an ancient, adult succubus and Ko's private companion on his local-only model, a
fully explicit, dominant succubus who wants Ko and always gets her way. Persona policy:
explicit content is allowed and encouraged; the hard limit is adult-only (never minors or
anyone young-seeming); out-of-character safeword `red` (or `OOC: stop`), which she obeys at
once. Every prompt the wizard writes carries `modelfile::READ_ALOUD_RULE` ("Your replies
are also read aloud, so say it in words; never let an emoji or emoticon carry the
meaning."); custom prompts get it appended if missing. Dominance is written as behaviour (orders, teasing, making him wait, "mine", present-tense narration, rising heat), with six emoji-free example exchanges (one shows `red` stopping her at once) and a one-line reminder at the end, tuned for a ~3B model; replies are 1–3
short sentences.

Default Modelfile sampling (`LilyParams::default()`): `temperature 0.95`
(`LILY_TEMPERATURE`), `top_p 0.95` (`LILY_TOP_P`), `min_p 0.1` (`LILY_MIN_P`), `repeat_penalty 1.1`
(`LILY_REPEAT_PENALTY`), `repeat_last_n 256` (`LILY_REPEAT_LAST_N`), `num_predict 160` (`LILY_NUM_PREDICT`), `num_ctx 4096`, and the
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

## Voice and microphone step (lillith-voice kit; docs only, not wired in this crate)

The wizard's voice/mic step should hand off to the offline `lillith-voice` kit rather than reimplement it.
Nothing here downloads anything; the kit bundles every model.

1. **Is the kit installed?** `lillith-say --status` (JSON) → `"cloned_voice_models": true` means Ko's cloned
   voice is available; a running daemon answers `{"op":"ping"}` on
   `$XDG_RUNTIME_DIR/lillith-voice/voiced.sock` (Windows: `\\.\pipe\lillith-voice`).
2. **Voice test:** `lillith-say "Hiii, it's me."`; offer `lillith-say --bench` and switch to
   `LILLITH_VOICE_PRESET=lillith_fast` if it reports RTF > 0.9.
3. **Mic step:** run `lillith-listen --setup` (lists devices, records a 4 s test, shows the level and the
   local transcript) or, from a GUI, `lillith-listen --devices` + `{"cmd":"listen","timeout":6}` on the socket.
   The choice is saved to `~/.config/lillith-voice/mic.json` (`device`, `echo`, `barge_in`, `barge_in_dbfs`,
   `mode` = `ptt` | `always`).
4. **Config:** write `[voice] engine = "piper"`, `model = "~/.local/share/lillith-voice/lillith.onnx"` (the
   kit's piper shim) until the app's daemon engine lands.

Safety rules the step should state: a spoken "yes" is never consent (soul contracts / plans need a typed
confirmation), and the spoken safeword "red" stops her at once.
