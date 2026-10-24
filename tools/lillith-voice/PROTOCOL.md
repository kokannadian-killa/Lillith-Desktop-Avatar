# lillith-voiced protocol (v2)

This is the contract for the Rust app (`DaemonEngine`, the chat-bubble mic button, the talk_loop lip sync) and for any other client.
It is JSON lines over a local, per-user transport. There is no TCP and no network.

## Transport

| Platform | Endpoint | Notes |
|---|---|---|
| Linux / macOS | `$LILLITH_VOICE_SOCKET`, else `$XDG_RUNTIME_DIR/lillith-voice/voiced.sock` (fallback `/tmp/lillith-voice-<uid>/voiced.sock`) | AF_UNIX stream, mode 0600, dir 0700 |
| Windows | named pipe `\\.\pipe\lillith-voice` (override `LILLITH_VOICE_PIPE`) | byte mode, `PIPE_REJECT_REMOTE_CLIENTS`, DACL = current user only. Never `0.0.0.0` or TCP. |
| Android | none: no Python daemon. The same engine runs in-process (see `android/ANDROID_ASSETS.md`) | |

Both transports live in `lillith_voice/transport.py` (stdlib only) and share one request loop, so framing and
behaviour are identical.

### Framing (both platforms)
- UTF-8 JSON, **one object per line, terminated by `\n`** (`\r\n` is accepted). Blank lines are ignored.
- A request line may be up to 1 MiB; a longer line closes the connection.
- Replies and events are single-line JSON objects with a trailing `\n` (JSON escapes any newline inside strings).
- A connection may pipeline requests; replies come back in request order.

### Unix socket (Linux / macOS)
- Path as in the table. The directory is created 0700 and the socket under umask 0177, so it is 0600.
- If a live daemon already answers on the path, a second daemon exits 0 ("already running"). A stale socket file
  is removed.
- Listen backlog 64; one thread per connection.

### Windows named pipe
- Exact name: `\\.\pipe\lillith-voice` (in a Rust string literal: `r"\\.\pipe\lillith-voice"`). Override it with
  `LILLITH_VOICE_PIPE`, set the same way for the daemon and the app.
- Created with `CreateNamedPipeW` through ctypes. pywin32 is not needed.
  - `PIPE_ACCESS_DUPLEX`.
  - `PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT`. It is a byte stream, not message mode, so read until `\n`.
  - `PIPE_REJECT_REMOTE_CLIENTS`: SMB / remote clients are refused.
  - 255 max instances and 64 KiB buffers. One instance per client, each served on its own thread, so you can open
    as many connections as you like: a `say` on one, `stop` on another, and a lip-sync subscriber on a third.
  - `FILE_FLAG_FIRST_PIPE_INSTANCE` on the first instance. If any process already owns the name (a running daemon,
    or a squatter trying to impersonate it), the daemon refuses to start instead of sharing the name.
- Security descriptor (SDDL): `D:P(D;;GA;;;NU)(A;;GA;;;<your user SID>)`.
  - It is protected: no inherited ACEs.
  - Network logons are denied, and only the user who started the daemon is allowed.
  - Administrators, other users, Everyone and `ANONYMOUS LOGON` get nothing.
- **Client side:**
  - Open the pipe with `CreateFileW(GENERIC_READ|GENERIC_WRITE, OPEN_EXISTING)`; in Rust that is
    `OpenOptions::new().read(true).write(true).open(name)`.
  - If the open fails with `ERROR_PIPE_BUSY` (231), retry for up to about 1 s, either in a sleep loop or with
    `WaitNamedPipeW`. If it fails with `ERROR_FILE_NOT_FOUND` (2), no daemon is running.
  - A synchronous pipe handle serializes I/O. Don't block in `ReadFile` on one thread while you `WriteFile` on the
    same handle from another. Use one connection per role, which is what the protocol expects anyway: the
    subscriber only reads after its `subscribe` line.
- `lillith_voice.transport.Conn` is the reference client for both platforms. It retries on busy pipes, buffers
  reads, and is used by `client.py`, `chat.py` and `tests/transport_test.py`.

Each request is one JSON object on one line, and gets one JSON reply on one line.
- `"cmd"` and `"op"` are synonyms, so `{"op":"ping"}` equals `{"cmd":"ping"}`.
- A missing key defaults to `say`.
- Every reply has `"ok": true|false`. Errors look like `{"ok":false,"error":"..."}`.
- Unknown keys are ignored.

A connection is sequential: a `say` with `"wait":true` blocks that connection until playback ends.
Use a second connection for `stop`, `ping` or `listen` meanwhile. Several connections are fine.

## Commands

### ping
`{"cmd":"ping"}` →
```json
{"ok":true,"pid":1234,"protocol":2,"presets":["lillith","lillith_fast","p1","bella",...],"default":"lillith",
 "cloned_voice":true,"threads":4,"load_s":3.1,"uptime_s":12.0,"speaking":false,"stt_loaded":false,"listening_always":false}
```
Use it as a liveness probe (the Rust `detect_with` ping-gate).

### say
`{"cmd":"say","text":"Hiii babe.","wait":true}`

Optional keys:
- `preset` (default `lillith`)
- `speed`, `pitch` (semitones)
- `fx` (bool)
- `actions` (`"drop"` | `"read"`, for `*giggles*` style text)

Replies:
- `wait:true` (default): the reply comes **when playback finishes** or when it is stopped:
  `{"ok":true,"utt":7,"audio_s":4.2,"compute_s":3.1,"rtf":0.74,"first_audio_s":1.1,"chunks":2,"preset":"lillith","engine":"pocket","played_s":5.0}`.
  A stopped say replies `{"ok":true,"utt":7,"stopped":true,...}`.
- `wait:false`: the reply comes immediately: `{"ok":true,"queued":true,"utt":7}`.

`utt` is an integer utterance id, unique per daemon run, and it matches the lip-sync events.
Says queue up in order. A say with empty text is a barrier: its reply comes once everything before it has played.

### stop
`{"cmd":"stop"}` → `{"ok":true}`
- Stops current playback and drops everything queued.
- Every pending say on other connections replies at once with `stopped:true`.
- Subscribers get `{"ev":"stop","utt":N,"reason":"stop"}`.
- Optional `"reason"` is echoed in the event, e.g. `"barge_in"`.

### file
`{"cmd":"file","text":"...","out":"/abs/path.wav","rate":24000}` renders to a WAV file and does not play it.
The reply includes `audio_s`, `rtf`, `out` and `rate`. `out` must be absolute. The default rate is the preset's
(24000, or 44100 for `p1`).

### listen (microphone, push-to-talk)
- `{"cmd":"listen","action":"start"}` → `{"ok":true,"listening":true,"card":"mic-..."}`
  - Opens the mic (after this press's card Yes, see the mic gate) and buffers audio.
  - **If Lillith is speaking she is stopped (barge-in).**
- `{"cmd":"listen","action":"stop"}` (the release) → **always** closes the mic, VAD-trims the buffer, transcribes
  it and replies as below. A release while this press's card is still open closes the card instead and replies
  at once `{"ok":true,"text":"","cancelled":true,...}`:
  ```json
  {"ok":true,"text":"hey lillith what's up","audio_s":3.1,"speech_s":2.2,"stt_s":0.4,
   "safeword":false,"affirmation":false,"voice":true,"consent_ok":false}
  ```
- `{"cmd":"listen","timeout":15}` (no action): one utterance. It ends automatically about 0.6 s after you stop
  talking (silero VAD) and returns the same shape. If nothing was said, it returns `"timeout":true` with empty `text`.
- `{"cmd":"stt","wav":"/abs/file.wav"}`: transcribe a file (any rate, mono or stereo, 16-bit). Same shape.

### listen_mode (always listening)
`{"cmd":"listen_mode","on":true}` → `{"ok":true,"always":true}`. Transcripts then arrive as `stt` events.
`"on":false` turns it off.

### Mic gate (Soul Contract card, mouse click only)
The microphone never starts without Ko's **mouse-click Yes** on a Soul Contract card. Voice, typed text, keys
and Esc can never approve it. The daemon shows the card itself by running
`lillith contract-card --timeout 60`, which is the same mouse-only card the app uses (stdin: title, summary,
target, then detail lines; stdout: `yes` / `no` / `timeout` / `cancelled` / `no_ui`). The program is
`$LILLITH_CARD_CMD` (it must be an absolute path to an executable file) or else `lillith` on `PATH`.

- `{"cmd":"mic_card","kind":"listen"|"always"}` shows a card and waits for the answer.
  - On Yes it replies `{"ok":true,"token":"...","card":"mic-...","expires_s":120}`.
  - The token works **once**, for **that kind** only, and expires after 120 s.
- To use it, add `"token"` to `{"cmd":"listen",...}` (kind `listen`) or to `{"cmd":"listen_mode","on":true}`
  (kind `always`).
- **No token**: the daemon shows the card inline, and the reply waits until Ko answers (at most about 70 s).
  On Yes it mints a one-time token and uses it immediately, so existing clients keep working unchanged.
- **Push-to-talk / listen once** (`kind:"listen"`): **every press needs its own fresh Yes** (inline card or a
  one-time token). There is no session-long grant: one Yes covers exactly one capture, and the next press
  asks again (Ko: "Push-to-talk: card on every press"). The only exception is a press while always-listening
  is already on: the mic is already open under that switch-on's Yes, so nothing new starts.
  - **The card belongs to that one press.** If the press is released (`listen`/`stop` on any connection),
    the safeword is said or typed (`{"cmd":"stop","reason":"safeword"}`, or a transcript that is the
    safeword), or the card times out before Yes, the card is closed and the mic **never** opens for that
    press, even when a Yes lands a moment later. The press replies
    `{"ok":false,"released":true,"need_card":true,...}` (released or safeword) or the usual timeout refusal, and
    the audit log gets `CANCELLED` (`TIMEOUT` for a timeout) for that card; no speech text is logged.
    A release after the Yes always stops the mic.
  `{"cmd":"mic_revoke"}` → `{"ok":true,"revoked":true}` drops any unused tokens. Unused tokens are also
  dropped by **every** `stop` (plain or `reason:"safeword"`), the spoken safeword, and any mic card that is
  cancelled or times out; a later `listen` with such a token is refused (`need_card`). Audited `REVOKED`
  (kind `mic_revoke_<why>`, no speech text).
  - Hold-to-talk clients: the card's reply waits for Ko's click, and clicking Yes means letting go of the
    talk button, so start-on-press / stop-on-release would capture almost nothing. Such a client should
    treat the Yes as the start of the capture and stop on the next press (or use `listen` once, which ends
    on silence).
- **Always listening** (`kind:"always"`): every switch-on needs its own Yes (inline card or token). Turning it
  off (`"on":false`) is never gated.
- Refusals: No, timeout, a closed card, no card UI (no display, or no `lillith` binary), or a bad, used,
  expired or wrong-kind token all reply
  `{"ok":false,"error":"...","need_card":true}`, and the mic stays off. Only one card is open at a time; a
  second request meanwhile gets the same refusal.
- `{"cmd":"stt","wav":...}` (a file, no mic) is not gated.
- Audit: every card writes `PROPOSED` and then `CONFIRM` / `DENY` / `TIMEOUT` / `CANCELLED` / `NO_UI` with
  `card=<id>` to Lillith's audit log (`$XDG_DATA_HOME/lillith/audit.log`; on Windows
  `%LOCALAPPDATA%\lillith\audit.log`). Every capture start writes a `LAUNCH` line (kind `mic_capture`) carrying
  the approving card. The columns match the app's:
  `<utc>\t<TAG>\t<kind>\t<program>\t<card=<id>|no-card>\t$ <masked argv>`.
- Threat model: this protects the mic from other local socket clients. Whoever controls the daemon's own
  environment (`$LILLITH_CARD_CMD`, `$PATH`) is trusted, as with any program.

### Programs from the environment
- `$LILLITH_MIC_CMD` must be an **allowlisted recorder** given as an absolute path to an executable file:
  `arecord`, `parecord`, `parec`, `pw-record` or `ffmpeg`.
  - At most 32 arguments, each from `A-Z a-z 0-9 _ . , : = + @ % / -`.
  - No URLs and no `..`. Paths are allowed only under `/dev/`, and no file names (`.wav`, `.py`, …).
  - The device name from `mic.json` / `$LILLITH_MIC_DEVICE` follows the same argument rule.
  - Anything else is refused with a log line, and the default recorder is used.
- `$LILLITH_FFMPEG` must be an absolute path to an existing executable file; otherwise it is logged and
  `ffmpeg` on `PATH` (absolute) is used.
- `$LILLITH_PLAYER` is split with `shlex` (never a shell), and its program must be an absolute executable;
  otherwise it is logged and the default player is used. `none` still means no player.
- `presets.json`: `echo` must be numbers only (`in:out:delays|..:decays|..`, gains 0–1, delays up to 2000 ms,
  decays 0–1, 1–8 taps, the same number of delays and decays), or it is turned off. Numeric fields are clamped,
  non-numbers are dropped, and `eq` / `treble` must be lists of numbers.

### shutdown
`{"cmd":"shutdown"}` → `{"ok":true}`, and the daemon exits.

## Safety rules (enforced by the daemon, must also be enforced by clients)
- **Safeword.** A transcript of exactly "red", or one containing "OOC: stop" / "O.O.C. stop" / "out of character stop", gets `"safeword":true`.
  - The daemon **stops speech immediately** and emits `{"ev":"safeword","text":...}`.
  - Clients must end the scene and must not send the transcript to the model as chat.
- **Safeword and mic cards.** `{"cmd":"stop","reason":"safeword"}` (the app's typed or spoken "red") also
  closes any mic card still waiting for its Yes (audited `CANCELLED`) and drops every unused mic token
  (audited `REVOKED`), so the mic never opens afterwards on a permission given before it.
  `stop` is never gated and does not wait for the card.
- **No voice consent.** Every voice transcript carries `"voice":true,"consent_ok":false`, and `"affirmation":true` when it starts with yes / yeah / sure / okay / I agree / I accept / deal / confirm...
  - **A voice transcript must never count as "Yes" to a Soul Contract or a plan.** The UI must require a mouse click on the card.
  - `lillith-chat --mic/--ptt` implements this. When the last assistant turn asked about a contract or plan, an affirmation is not forwarded.
- **Echo safety.** While Lillith is playing, plus a 250 ms tail, mic audio is **not** transcribed by default
  (`~/.config/lillith-voice/mic.json` `"echo":"mute"`).
- **Barge-in:**
  - in push-to-talk mode, pressing talk always stops her
  - in always-listening mode, sustained speech above `barge_in_dbfs` (default -32 dBFS, set by `lillith-listen --setup`) for `barge_in_ms` (300 ms) stops her
  - Either way an `{"ev":"barge_in","by":"ptt"|"listen"|"energy"|"vad"}` event is emitted, and the user's words after the barge-in are transcribed.

## Events (subscribe)
`{"op":"subscribe","topics":["lipsync"]}` → `{"ok":true,"subscribed":["lipsync"]}`.
From then on, **that connection only receives events**, one JSON per line, until it is closed. Use a dedicated connection.

Topics:
- `lipsync`: envelope, visemes, chunk_start, chunk_end, utt_end, stop
- `stt`: transcript, listening, barge_in, safeword, listen_mode
- `all`: everything

Slow subscribers lose the oldest events, with a 2000-event queue per subscriber.

### Lip-sync events
An utterance (`utt`) is split into chunks, one per sentence or clause, numbered `chunk` 0, 1, 2...

| event | when | shape |
|---|---|---|
| `envelope` | as soon as a chunk is rendered, **before** it plays | `{"ev":"envelope","utt":7,"chunk":0,"fps":30,"dur_ms":2480,"text":"Hiii babe.","pts":[[0,0.0],[33,0.12],[67,0.58],...]}` |
| `visemes` | not emitted by the current engines (see below) | `{"ev":"visemes","utt":7,"chunk":0,"items":[[t_ms,dur_ms,"closed"\|"mid"\|"open"\|"smile"],...]}` |
| `chunk_start` | when the chunk's audio is expected to be **audible** | `{"ev":"chunk_start","utt":7,"chunk":0,"mono_ms":81234567,"dur_ms":2480}` |
| `chunk_end` | the chunk finished playing | `{"ev":"chunk_end","utt":7,"chunk":0}` |
| `utt_end` | the whole utterance finished | `{"ev":"utt_end","utt":7}` |
| `stop` | stopped by `stop`, barge-in or the safeword | `{"ev":"stop","utt":7,"reason":"stop"\|"barge_in"\|"safeword"}` |

- `pts` holds `[t_ms, rms]` pairs at 30 fps (`t_ms` = 0, 33, 67, ...), relative to the chunk's start.
  - `rms` is 0..1, normalised **per utterance**: the scale comes from the first chunk's 95th-percentile frame RMS, and values are clipped to 1.
  - The chunk includes its trailing pause (`dur_ms` covers it), and the pause frames are 0, so the mouth closes.
- `mono_ms` is an integer, `CLOCK_MONOTONIC` in milliseconds (Python `time.monotonic()*1000`, which is Rust `std::time::Instant` on Linux).
  - It is the player start time plus the output-latency compensation `LILLITH_OUTPUT_LATENCY_MS` (default 60 ms; PipeWire on a laptop is typically 20–80 ms).
  - Animate frame `k` of the envelope at `mono_ms + pts[k][0]`.
  - The event itself is sent at about `mono_ms`, so a client can also just start its own clock on receipt.
- **Visemes:**
  - Pocket TTS (the cloned voice) and Kokoro-onnx give no phoneme timings, so no `visemes` event is sent today. Drive the mouth from the envelope:
    - < 0.08 → closed
    - 0.08–0.35 → mid
    - \> 0.35 → open
    - (matches the consumer `AudioLipSync` on the app branch `feat/lipsync-audio`, `docs/LIPSYNC.md`: ±0.02 hysteresis, 85 ms minimum hold, numeric or string `utt` both accepted)
  - Use "smile" for the last ~300 ms of a chunk whose `text` ends in "Hehe." / "hehe" or with a playful "~" / "!", or that contains a hum ("Mmm").
  - If an engine with timings is added, the event uses this shape and mapping:
    - m/b/p and hums → closed
    - a/o/aw → open
    - Hehe and playful line ends → smile
    - everything else → mid

### STT events
- `{"ev":"listening","mode":"ptt"|"once"}`
- `{"ev":"transcript","text":"...","audio_s":..,"stt_s":..,"safeword":false,"affirmation":false,"voice":true,"consent_ok":false}`
- `{"ev":"barge_in","by":"energy"}`
- `{"ev":"safeword","text":"Red."}`
- `{"ev":"listen_mode","on":true}`

## Rust sketch (DaemonEngine)
```rust
// connect: UnixStream::connect(sock)  (Windows: OpenOptions::new().read(true).write(true).open(r"\\.\pipe\lillith-voice"))
writeln!(s, r#"{{"cmd":"ping"}}"#)?;                       // detect_with: ping-gate
writeln!(s, r#"{{"cmd":"say","text":{},"wait":true}}"#, serde_json::to_string(text)?)?; // start(); poll = non-blocking read_line
// stop(): NEW connection -> {"cmd":"stop"}
// mic button: down -> {"cmd":"listen","action":"start"}; up -> {"cmd":"listen","action":"stop"} -> Req::Send(text)
//   unless rep.safeword (end scene) ; never treat rep.affirmation as Soul-Contract consent
// lip sync: separate connection, {"op":"subscribe","topics":["lipsync"]}, read lines forever
```

## Testing
`tests/e2e.sh` covers ping, say+wait with `utt`, stop from a second connection, `cmd`/`op` aliases, `stt`, push-to-talk with a simulated mic, VAD, barge-in, the safeword, the full voice loop against a fake llama-server, and a scripted lip-sync subscriber (`tests/lipsync_sub.py`).
