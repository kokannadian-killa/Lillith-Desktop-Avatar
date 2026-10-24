# Chatting with Lillith (local llama.cpp)

Click Lillith and a small chat bubble opens beside her. She answers through a
**local** [llama.cpp](https://github.com/ggml-org/llama.cpp) `llama-server` running a
GGUF model. Nothing leaves your machine. While her reply streams in, she plays her
speaking animation (`talk_loop` / `talk_loop_flirty`), and her lips follow the text
as it arrives.

- **Open:** single left click on her (after the double-click window, ~0.35 s), or
  `Y` / `Enter` while she has focus. Double clicks and drags don't open it, so
  dragging her around still works as before.
- **Close:** `Esc`, the `×` button, a click anywhere outside the bubble, or another
  click on her.
- **Type** in the box at the bottom and press `Enter`. `/clear` forgets the
  conversation.
- **Server down?** She tells you in character ("my brain's napping, is
  llama-server running?") and the app keeps running. Try again once the server is up.

## 1. Get llama-server

Prebuilt binaries are on the [llama.cpp releases page](https://github.com/ggml-org/llama.cpp/releases)
(look for the newest `bNNNNN` build).

| OS | Download / install | Binary |
|----|--------------------|--------|
| **Linux** (x64, CPU) | `llama-bNNNNN-bin-ubuntu-x64.tar.gz` (or `-vulkan-x64` for most GPUs). Extract it anywhere, e.g. `~/opt/llama.cpp/`. Arch: `yay -S llama.cpp` (or `-vulkan`/`-cuda`). | `llama-server` (keep the `.so` files next to it; Lillith sets `LD_LIBRARY_PATH` to that folder when she starts it) |
| **Windows** | `llama-bNNNNN-bin-win-cpu-x64.zip`, `-vulkan-x64.zip` (AMD/Intel/NVIDIA) or `-cuda-12.4-x64.zip` plus `cudart-llama-bin-win-cuda-12.4-x64.zip` (NVIDIA). Or run `winget install llama.cpp`. | `llama-server.exe` |
| **macOS** | `brew install llama.cpp`, or `llama-bNNNNN-bin-macos-arm64.tar.gz` (Apple Silicon, Metal is on by default) | `llama-server` (Homebrew: `/opt/homebrew/bin/llama-server`) |

You can also build it from source: `cmake -B build && cmake --build build --config Release -t llama-server`.

## 2. Get a GGUF model

Any instruct/chat GGUF works. Q4_K_M quantizations are a good size/quality trade-off:

| Model | Size | Notes |
|-------|------|-------|
| [Qwen2.5-0.5B-Instruct Q4_K_M](https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q4_k_m.gguf) | ~490 MB | Tiny and fast on any CPU. Good for testing, but it keeps character poorly. |
| [Qwen2.5-3B-Instruct Q4_K_M](https://huggingface.co/Qwen/Qwen2.5-3B-Instruct-GGUF) | ~2 GB | Good default on a laptop |
| [Llama-3.2-3B-Instruct Q4_K_M](https://huggingface.co/bartowski/Llama-3.2-3B-Instruct-GGUF) | ~2 GB | Playful, keeps the persona well |
| 7–8B instruct models (Qwen2.5-7B, Llama-3.1-8B) Q4_K_M | 4.5–5 GB | Best personality if you have a GPU or 16 GB of RAM |

```sh
mkdir -p ~/models && cd ~/models
curl -LO https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q4_k_m.gguf
```

(On Windows, download it in the browser or use `curl.exe -LO …` in PowerShell.)

## 3a. Run the server yourself

```sh
llama-server -m ~/models/qwen2.5-0.5b-instruct-q4_k_m.gguf --host 127.0.0.1 --port 8080 -c 4096
# GPU builds: add -ngl 99 to offload all layers
```

Check it with `curl http://127.0.0.1:8080/health`, which should print `{"status":"ok"}`. The
defaults already point to `http://127.0.0.1:8080`, so no config is needed.

## 3b. …or let Lillith start it

Set both paths under `[chat.server]`. Starting a program goes through the
Soul Contract, so she asks first: when you chat and nothing answers at
`endpoint`, the bubble shows a "Restart local model server?" card. Only a
click on its **Yes** starts `llama-server` (audited `CONFIRM`; No, Esc, no
answer within 120 s, or no card surface leave her text-only for 10 minutes).
The app then owns the server and stops it when she quits, including on
Ctrl-C or SIGTERM. Closing the bubble with the card up counts as
`CANCELLED` (written by the bubble, or by the app if the bubble was killed;
a card left open by a crash is closed the next time the app starts).

`lillith chat --spawn-server` asks too: the same card in a small window when
a desktop is available, otherwise `Start local model server? [y/N]` on your
terminal (read from the terminal itself, never from piped stdin; only `y` /
`yes` starts it, audited `CONFIRM_TTY`, anything else is `DENY_TTY`). With
neither (e.g. under a service manager) it is `NO_UI` and the chat stays
text-only. Keys typed before the question are discarded, so a stray `y`
can't answer it. One card starts at most one server (`USED`; a refused or
failed start is `SPAWN_REFUSED` / `SPAWN_FAILED`). See [MEMORY.md](MEMORY.md) ("Desktop wiring") for the full table.

```toml
# ~/.config/lillith/config.toml  (or any file passed with --config)
[chat.server]
binary = "/home/me/opt/llama.cpp/llama-server"      # Windows: 'C:\llama\llama-server.exe'
model_path = "/home/me/models/qwen2.5-0.5b-instruct-q4_k_m.gguf"
```

## Config keys (`[chat]`)

| Key | Default | Meaning |
|-----|---------|---------|
| `enabled` | `true` | Master switch for the bubble |
| `open_on_click` | `true` | A single left click opens the bubble (`Y`/`Enter` always do) |
| `endpoint` | `"http://127.0.0.1:8080"` | Base URL of the OpenAI-compatible server (`/v1/chat/completions` is appended). Other compatible servers work too (LM Studio `:1234`, Ollama `:11434`, …) |
| `model` | `"local"` | `model` field of the request (llama-server ignores it; other servers need the real name) |
| `system_prompt` | `""` | Empty means `[llm] system_prompt` if set, else her built-in persona `lillith_setup::modelfile::LILY_SYSTEM_PROMPT` (re-exported as `lillith_mind::persona::DEFAULT_SYSTEM_PROMPT`): a fully explicit, dominant adult succubus who wants Ko and always gets her way. Persona policy: explicit content is allowed and encouraged on Ko's private, local-only model. Hard limit: adult-only (never minors or anyone young-seeming). Out-of-character safeword: `red` (or `OOC: stop`), which she obeys at once |
| `temperature` | `0.95` | Sampling temperature (same as the wizard's `LILY_TEMPERATURE`); requests also send `top_p 0.95`, `min_p 0.1`, `repeat_penalty 1.1`, `repeat_last_n 256` |
| `max_tokens` | `160` | Reply length cap (same as the wizard's `LILY_NUM_PREDICT`) |
| `history_turns` | `12` | Past user/assistant turns sent with each message |
| `timeout_s` | `120` | Whole-request timeout (connecting times out after 3 s) |
| `save_history` | `true` | Keep the chat in `~/.local/share/lillith/chat_history.json` (last 200 messages) |
| `api_key` | `""` | Sent as `Authorization: Bearer …` (llama-server `--api-key`; also passed when spawning) |
| `overlay_speech` | `true` | Draw her reply in a small speech bubble over her head on the overlay. The overlay window grows upward by about 64 px at scale 0.75 (it scales with `--scale`); her feet stay put and the bubble area is click-through |
| `talk_jiggle` | `true` | In baked-jiggle mode (`baked_jiggle = true` / `--baked-jiggle`), draw `talk_loop_jiggle` / `talk_loop_flirty_jiggle` while she talks. Their `disable_springs` (chest/pigtails/hip) apply. `false` keeps the clean talk clips with engine springs |

`[chat.server]` (optional, self-spawned server):

| Key | Default | Meaning |
|-----|---------|---------|
| `binary` | `""` | Path to `llama-server`; empty means don't spawn |
| `model_path` | `""` | Path to the `.gguf`; empty means don't spawn |
| `ctx_size` | `4096` | `-c` |
| `threads` | `0` | `-t` (0 leaves it to llama-server) |
| `gpu_layers` | unset | `-ngl` |
| `extra_args` | `[]` | Extra arguments (e.g. `["--jinja"]`). Allowlist only: `-c`/`--ctx-size`, `-t`/`--threads`, `-ngl`, `--jinja`, sampling flags (`--temp`, `--top-k`, `--top-p`, `--min-p`, …), `-b`/`-ub`, `--mlock`, `--no-mmap`, `--flash-attn`. Anything else is refused (nothing starts) and logged as `ARG_REFUSED` in the audit log |
| `startup_timeout_s` | `90` | How long to wait for `/health` before warning |

The host and port are taken from `endpoint`.

`[anim]` (pacing, shared with all animation):

| Key | Default | Meaning |
|-----|---------|---------|
| `talk_speed` | `0.62` | Lip-flap pace; 1.0 = the original fast pace. At 0.62, mouth shapes change about 38% less often and the end smile holds about 1.5 s (it scales with the speed). |
| `playback_speed` | `0.7` | Global clip playback speed: every clip's frame timing (fps / `frame_durations_ms`) is stretched by 1/speed. Physics, breathing and idle timers stay real-time. Range 0.1–4. |

`lillith --no-llm` keeps the bubble but answers with an "I'm switched off" line,
and doesn't spawn the server.

## Terminal chat / smoke test

```sh
lillith chat "Hi Lillith!"                  # one message, streamed to stdout
lillith chat                                # prompt loop (/quit to exit)
lillith chat --spawn-server "Hi"            # ask to start [chat.server] first (card or [y/N]), stop it after
lillith chat --endpoint http://127.0.0.1:9000 --no-history "Hi"
```

Live test of the client code against a real server:

```sh
LILLITH_LLAMA_URL=http://127.0.0.1:8080 cargo test -p lillith-mind --test llama_live -- --nocapture
# or let the test spawn the server:
LILLITH_LLAMA_BIN=/path/llama-server LILLITH_LLAMA_GGUF=/path/model.gguf \
  cargo test -p lillith-mind --test llama_live -- --nocapture
```

## How it works

```
 overlay process (lillith)                    bubble process (lillith chat-bubble)
 ┌──────────────────────────┐  spawn --x --y  ┌──────────────────────────────┐
 │ click → ClickGate (0.35s)├────────────────▶│ eframe window (300×380,      │
 │ ChatHook → chat::Launcher│                 │ undecorated, on top)         │
 │                          │ stdout JSON     │ ChatSession → llama-server   │
 │ EventFeed ◀── chat.start/token/end ────────┤ /v1/chat/completions (SSE)   │
 │ Talk: talk_loop + lipflap│                 └──────────────────────────────┘
 │ ServerGuard (llama-server child, optional)
 └──────────────────────────┘
```

- The bubble is a separate process because the overlay already owns this process's
  windowing event loop (layer-shell or winit). It prints one JSON line per event
  (`{"event":"chat.token","detail":"…"}`), and the overlay feeds those into the same
  event queue as the system monitor.
- **Lip-flap** (`crates/lillith-anim/src/talk.rs`): every streamed text piece queues
  mouth shapes. Vowels open the mouth, consonants half-open it, punctuation closes it,
  and `~`, emoji or `<3` make her smile. Her lips move at an unhurried pace (`[anim] talk_speed`) and never fall more
  than 4 s behind the text. `talk_loop` has a baked mouth shape per frame (`mouth_sequence`, imported from
  `lip_flap.baked_sequence`). The body keeps playing, and whenever the current frame has
  the wrong mouth, the nearest frame (within ±3) with the right one is drawn instead.
  Replies alternate between `talk_loop` and `talk_loop_flirty`. At the end she holds a
  smile for about 1.5 s and goes back to idle. Without a talk clip she just stays idle.
- **True lip-sync (mouth patches):** when the talk clip has `mouth_patches` (imported
  from `lip_flap.shapes`: `talk_loop/mouth/mouth_<shape>.png` + `centre_in_patch`), the
  body plays on untouched and the shape's patch is pasted every frame. It goes at that frame's
  `frame_points.mouth_point`, rotated by `mouth_angle_deg[frame]`, and scaled/mirrored
  with the sprite. Shapes then change once per syllable, each lasting the art's
  `recommended_rate.mouth_shape_change_ms` (mean of 200–260 ms = 230 ms at
  `talk_speed = 0.62`, scaled by 0.62/`talk_speed`). Jiggle siblings use their own
  points/tilt and fall back to the clean clip's patches. If the patch images don't
  load, the overlay logs a warning and uses the baked-frame picking above.
- **Overlay speech bubble** (`crates/lillith-overlay/src/speech.rs`): the reply is
  revealed at about 12 characters/s × `talk_speed`/0.62, faster when the stream is more than 4 s
  ahead. The lip-flap is fed from the *revealed* text, so mouth and words stay together.
  The bubble shows at most 3 word-wrapped lines (the newest, auto-scrolling), stays left of
  the chat window, holds 3.5 s after the last word, then fades out over 0.6 s. It is
  drawn after the click mask is computed, so clicks there pass through to the desktop.
  When a reply starts, any small reaction she was playing stops.
- **Emoji:** text is drawn with `ab_glyph` and a per-character fallback chain:
  Ubuntu Light → bundled Noto Emoji (monochrome, current Unicode, SIL OFL 1.1:
  `crates/lillith-anim/fonts/`) → egui's emoji-icon font → system DejaVu Sans if
  present. The chat window uses the same Noto Emoji ahead of egui's older copy.
  Invisible emoji modifiers (U+FE0F, ZWJ, skin tones) are stripped before drawing.
  Before this they showed as boxes, and egui's built-in emoji font lacks 🥰 🥺 🫶 🤭 and others.

### Known limits

- **Wayland:** normal windows can't position themselves, so the compositor decides
  where the bubble appears (KWin usually centres it). With X11/XWayland it opens next
  to her head. A layer-shell bubble would fix this later.
- Closing on a click outside relies on focus-out events. Window managers with
  focus-stealing prevention may need a click into the bubble first.
