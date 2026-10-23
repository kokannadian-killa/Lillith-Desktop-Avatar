# Audio-driven lip-sync (`talk_loop`)

Lillith's mouth can follow her real voice instead of the chat text. The
voice daemon (`voiced`, owned by the voice crate) publishes a loudness
envelope and optional visemes for every audio chunk it plays; the
avatar maps them to the four mouth shapes of the speaking clip
(`closed`, `mid`, `open`, `smile`).

Code: `crates/lillith-anim/src/lipsync.rs` (`AudioLipSync`, `TalkMouth`).
It is pure (no I/O, no clock): the app reads the socket and passes each
line plus a local monotonic time in ms.

## Transport (local only)

| OS            | Endpoint                                            |
|---------------|-----------------------------------------------------|
| Linux / macOS | Unix socket `$XDG_RUNTIME_DIR/lillith-voice/voiced.sock` (`lipsync::SOCKET_REL_PATH`) |
| Windows       | Named pipe `\\.\pipe\lillith-voice` (`lipsync::WINDOWS_PIPE`) |

Never TCP, never off-box. Same JSON-lines framing on both: one UTF-8
JSON object per `\n`-terminated line.

After connecting, the app writes `lipsync::SUBSCRIBE_LINE` + `\n`:

```json
{"op":"subscribe","topics":["lipsync"]}
```

## Events

`utt` = utterance id (string), `chunk` = chunk index (non-negative int).
Times `t_ms` / `dur_ms` are relative to the chunk's `chunk_start`.

| `ev`          | Fields | When |
|---------------|--------|------|
| `envelope`    | `utt, chunk, fps, pts: [[t_ms, rms0to1], ...]` | before the chunk plays; rms normalised per utterance |
| `visemes`     | `utt, chunk, items: [[t_ms, dur_ms, "closed\|mid\|open\|smile"], ...]` | optional, before the chunk plays |
| `chunk_start` | `utt, chunk, mono_ms` | audio actually reaches the output (after device-latency compensation) |
| `chunk_end`   | `utt, chunk` | chunk finished playing |
| `utt_end`     | `utt` | last chunk sent: smile 1.5 s once it has played |
| `stop`        | `utt` | barge-in / interrupt: close the mouth now, no smile |

`chunk_start` is mapped to the **local** time the line was received; the
sender's `mono_ms` is stored (`last_sender_mono_ms()`) for diagnostics but
never compared with the local clock (monotonic clocks differ per process).

## Mapping

- **Envelope only** (with ±0.02 hysteresis):
  `rms < 0.08` → closed, `0.08 – 0.35` → mid, `> 0.35` → open.
  Closed→mid needs ≥ 0.10, mid→closed below 0.06, mid→open above 0.37,
  open→mid below 0.33. Past the last envelope point the chunk is silent.
- **Visemes present** for the chunk: the viseme wins; the envelope gates it
  (`rms < 0.05` → closed). Gaps between visemes are closed.
- **Minimum hold**: a shape stays at least 85 ms (the token lip-flap's
  `FLAP_S`) before changing, so fast envelopes don't chatter.
- Between chunks (speaking, nothing playing): closed.
- **`utt_end`**: when nothing is playing any more, smile for 1.5 s
  (`talk::END_SMILE_S`), then `tick` returns `None`.
- **`stop`**: the next `tick` returns `closed` (no hold), then `None`; the
  utterance's pending chunks and any later events for it are discarded.

## Robustness

- Missing visemes, envelope after `chunk_start`, chunks out of order,
  duplicate `chunk_start`, `chunk_end` before `chunk_start`: all handled.
- A chunk whose `chunk_start` hasn't arrived 2 s after its first data is
  dropped (`dropped_late()` counts them); a late start for it is ignored.
- A chunk without `chunk_end` stops 200 ms after its data runs out.
- Speaking but no chunk/event for 3 s and no `utt_end` (daemon died):
  treated as ended (smile, then idle).
- A new `utt` id supersedes the current one; events for stopped, finished
  or superseded utterances are ignored.
- Blank lines, lines without `ev` (acks), unknown `ev`s, unknown fields
  and unknown viseme names are ignored. Bad JSON or a known event with
  missing/mistyped fields returns `Err(LipSyncError)`; the state is
  unchanged. Never panics.

## Wiring in the app

```rust
use lillith_anim::lipsync::{TalkMouth, SUBSCRIBE_LINE};

// Reader thread: connect, write SUBSCRIBE_LINE + "\n", forward each line
// over a channel. Reconnect with back-off; on disconnect call
// talk.audio.reset().

// Frame loop (same monotonic clock for feed and tick):
let now_ms = start_instant.elapsed().as_millis() as u64;
while let Ok(line) = rx.try_recv() {
    if let Err(e) = talk.audio.feed_line(&line, now_ms) {
        tracing::debug!(%e, "lipsync line");
    }
}
// Audio shape when the voice is playing, else the token lip-flap.
let mouth = talk.tick(dt, now_ms);
```

`TalkMouth::tick` always advances the token `LipFlap` too, so text-driven
flap resumes seamlessly when audio stops (voice off, daemon not running).
The resulting `Mouth` goes through the existing `MouthMap::frame_for` /
`Animator::set_mouth` path unchanged.
