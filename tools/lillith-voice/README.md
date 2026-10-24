# lillith-voice-kit: Lillith's fully offline voice

**Lillith speaks in Ko's voice.** The sample Ko provided (`models/pocket/voices/lillith_ref_original.mp3`) is cloned
with **Pocket TTS** (Kyutai, ONNX export, CPU-only), and that clone is the default everywhere: `lillith-say`,
the daemon, `lillith-chat`, the piper shim, and the Rust app through the socket protocol.
Packaged for **Ko's HP Spectre x360 (i5-7200U, 8 GB, no GPU, Garuda/Arch, KDE Plasma 6, fish)**.

**Hard rule: fully local.** After install nothing downloads anything or sends telemetry. Every model is in the
tarball and lands in `~/.local/share/lillith-voice/models/`. Speech recognition (mic) is local too.

| | |
|---|---|
| **Default voice** | **`lillith`**: Ko's cloned voice (Pocket TTS fp32, temperature 0.7), then a gentle polish chain (HPF 90 Hz, −1.5 dB @ 250 Hz, +1.5 dB @ 3 kHz, −3 dB shelf @ 9 kHz, de-esser, −17 LUFS, −1.5 dBTP). Natural pauses at punctuation and crossfaded sentences. 24 kHz. |
| Faster clone | `lillith_fast`: the same voice with the small int8 flow head (`flow_lm_flow_int8`). A modest speed-up; try it if `lillith-say --bench` shows RTF > 0.9. |
| Fallback | `p1`: Kokoro `af_bella` at 0.95, +1 st formant shift, per-line F0 → 290 Hz, HPF 90, −2 dB @ 300, +5 dB @ 9k, treble +2 @ 12k, `aecho 0.8:0.85:28\|47:0.12\|0.07`, two-pass loudnorm −16 LUFS / −1 dBTP / LRA 11, 44.1 kHz. Used automatically if the Pocket models are missing. |
| Alternates | `bella` (old default, VB1), `bella85_heart15`, `bella80_nicole20`, `bella75_sky25` (Kokoro) |
| Commands | `lillith-say`, `lillith-voiced` (daemon), `lillith-chat` (llama-server bridge, voice loop), `lillith-listen` (mic / STT), `lillith-piper` (piper-compatible shim) |
| Fish | `say`, `say-fast`, `say-p1`, `say-bella`, `say-heart`, `say-nicole`, `say-sky`, `hush`, `lil`, `lil-talk`, `hear`, `lillith-voice-start/stop` |
| Protocol | [`PROTOCOL.md`](PROTOCOL.md): JSON lines over a unix socket / named pipe; say, stop, listen, STT, lip-sync events |

Text gotchas are handled for every preset: tildes are stripped (`hiii~`), giggles become `Hehe.`, `mmm`/`hmm`
are spoken as sounds (Kokoro gets the `ˈmmmm` phonemes), and numbers, money, times, `omg`, `u/ur`, `w/` and
similar are expanded.

### Why Pocket TTS (comparison)

Speaker similarity to Ko's held-out reference (higher is better; "ceiling" = another clip of Ko herself):

| engine | resemblyzer | ECAPA | RTF (loaded build box) | est. i5-7200U RTF | RAM | license |
|---|---|---|---|---|---|---|
| Ko herself (held-out clip, ceiling) | 0.936 | 0.849 | | | | |
| **Pocket TTS fp32 (this kit, `lillith`)** | **0.910** | **0.696** | 2.5–3.2 under load 20–40; 0.69 one-shot when quieter | ~0.6–1.0 | 0.86 GB peak (measured, one-shot) | models CC-BY-4.0, ONNX wrapper Apache-2.0 |
| Pocket TTS int8 flow head (`lillith_fast`) | not re-scored (only the 37 MB flow head is int8) | | ~0.85–0.95× fp32 time | ~0.55–0.95 | ~0.85 GB | same |
| Pocket TTS int8 transformer (`flow_lm_main_int8`, evaluated, **not used**) | 0.899 | 0.672 | ~0.7× fp32 time | ~0.5–0.8 | 0.98 GB peak (measured) | same; unreliable: 2 of 6 test renders stopped early or said the wrong thing |
| Kokoro + OpenVoice tone colour (evaluated, dropped) | 0.860 | 0.439 | 0.76 | ~1.2 | ~2 GB (torch, estimate) | Apache-2.0 / MIT |
| `p1` (Kokoro bella, F0 290) | 0.701 | 0.080 | 0.39 | ~0.5 | 0.53 GB peak (measured) | Apache-2.0 |
| `bella` (VB1) | 0.686 | 0.020 | 0.25 | ~0.4 | ~0.5 GB (same engine as p1) | Apache-2.0 |
| Chatterbox / F5-TTS / XTTS-v2 | not scored: OOM-killed or killed on this box | | published CPU RTF ≈ 3.3 / 11 / 0.7 (GPU-class CPUs) | too slow | 3–6 GB (published) | MIT / CC-BY-NC / CPML (non-commercial) |

Pocket is the only engine that sounds like Ko and still runs on a 2-core laptop CPU without torch.
Check your own laptop with `lillith-say --bench`.

**Licence and consent.** Pocket TTS weights are CC-BY-4.0 (Kyutai; ONNX export by KevinAHM). Attribution:
"Pocket TTS by Kyutai Labs, CC-BY-4.0". The cloned voice comes from a sample Ko provided for this purpose.
Only clone voices you have permission to use (`lillith-say --clone` asks for nothing else and stays local).

---

## 1. Install (from fish is fine)

```fish
tar xzf lillith-voice-kit.tar.gz      # full kit: code + models + offline wheels
cd lillith-voice-kit
bash install.sh                        # offline: models + wheels from the tarball, no network, no sudo
bash install.sh --systemd              # same, plus enable+start the background daemon (recommended)
bash install.sh --pacman               # opt-in: also sudo pacman -S the system packages listed below
```

What it does:

1. **Checks the model files first** and verifies them against `models/SHA256SUMS`. It never downloads them.
   If they're missing it prints exactly where to put them (`lillith-voice-kit/models/` or
   `~/.local/share/lillith-voice/models/`).
   - `models/pocket/` (504 MiB): Ko's clone. `flow_lm_main.onnx` 289 MiB (+ `_int8` 73 MiB), `flow_lm_flow` 37 MiB
     (+ `_int8`), `mimi_decoder` 40 MiB, `mimi_encoder` 38 MiB (only for `--clone`), `text_conditioner` 16 MiB,
     `tokenizer.model`, `bundle.json`, and `voices/lillith.npy` (Ko's precomputed voice embedding) with her
     reference clip.
   - `models/stt/` (100 MiB): whisper tiny.en int8 (encoder + decoder + tokens) and `silero_vad.onnx`.
   - `models/kokoro-v1.0.onnx` (311 MiB) + `voices-v1.0.bin` (27 MiB): the `p1` / bella presets.
2. **With `--pacman` (opt-in; default is check-only) installs these pacman packages:** `python ffmpeg rubberband espeak-ng pipewire pipewire-audio pipewire-pulse`.
   Arch's `extra/ffmpeg` (9.0.2) is linked against `librubberband`, and `pw-play` comes from
   `pipewire-audio`. The installer checks that the ffmpeg `rubberband` filter really exists.
3. **Creates a venv** at `~/.local/share/lillith-voice/venv` and installs pinned packages
   (`requirements.lock`) **from `./wheels` with `--no-index`**. No PyPI access is needed. The wheelhouse covers
   CPython 3.13 and 3.14 on x86_64. Use `--wheels DIR` for a different wheelhouse, or the opt-in `--online-pip`
   to fetch the pins from PyPI (install time only). With neither wheels nor `--online-pip` it stops instead of
   going online (the git copy in the Lillith repo has no `wheels/`: see `INSTALL.md`).
   - Arch ships Python 3.14. `kokoro-onnx 0.6.1` declares `Requires-Python <3.14`, but it's pure Python and
     works on 3.14 (tested with CPython 3.14.9 and these exact pins). On 3.14 the installer installs it with
     `--no-deps --ignore-requires-python`.
4. **Copies the code** to `~/.local/share/lillith-voice/lib`, the launchers to `~/.local/bin`, the fish file to
   `~/.config/fish/conf.d/lillith-voice.fish`, and the user unit to `~/.config/systemd/user/lillith-voice.service`.
5. **Runs a self-test:** it renders a WAV in Ko's voice offline, prints the RTF, then transcribes it with the
   local whisper (speech-to-text round trip).

Other flags: `--no-pacman` (no-op, the default), `--piper-shim` (see §6), `--python /usr/bin/python3.13`, `-y`.
Uninstall with `bash uninstall.sh`. It removes only what `install.sh` added to `$HOME`.

---

## 2. Usage

```fish
lillith-say "Oh my god, hiii! Like, there you are?"      # speak (via daemon if running, else one-shot)
echo "piped text works too" | lillith-say
lillith-say -p lillith_fast "same voice, faster"          # presets: lillith (default) lillith_fast p1 bella ...
lillith-say -p bella80_nicole20 "whispery one"             # aliases: ko/clone/default, fast, p, heart, nicole, sky, vb1..vb4
lillith-say --speed 1.05 --pitch 3 "custom"                # override preset speed / semitones
lillith-say -o line.wav "save instead of playing"          # WAV at the preset rate (24 kHz; p1 44.1 kHz), --rate 48000 to resample
lillith-say --raw "pcm" | pw-play --raw --rate 24000 --channels 1 --format s16 -   # raw s16le to stdout
lillith-say --no-fx "raw engine output, no polish"
lillith-say --list                                         # presets (engine, default marked)
lillith-say --bench                                        # RTF on THIS machine; tells you if lillith_fast is better
lillith-say --clone ref.wav myvoice                        # make a new cloned voice from a WAV you may use (local)
lillith-say --status                                       # daemon / rubberband / player info
lillith-say --stop        # or fish: hush                  # cut her off mid-sentence
lillith-say -v "..."                                       # JSON timing (RTF, first-audio latency) on stderr
```

- **Daemon vs one-shot.** If `lillith-voiced` is running, `lillith-say` talks to it over a **unix socket**
  (`$XDG_RUNTIME_DIR/lillith-voice/voiced.sock`, mode 600). The models stay loaded and Ko's voice state is
  cached, so audio starts after the first sentence chunk renders (about 1–2 s). Without the daemon each call
  loads the models (1.5–3.5 s) first.
  Use `--no-daemon` to force in-process or `--daemon` to require the daemon.
- **Sentence pipelining.** Text is split into sentences. Sentence *n+1* renders while sentence *n* plays, so
  long replies start fast and keep flowing as long as RTF < 1.
- **Text cleanup.** For LLM output it drops ```` ```code``` ````, URLs (spoken as "link"), emoji, markdown, and
  `<think>…</think>`. `*giggles*`-style actions are dropped by default; use `--actions read` to speak them.
- **Players.** Tried in order: `pw-play` → `paplay` → `aplay` → `ffplay`. Override with
  `LILLITH_PLAYER="pw-play --volume 0.8 {}"` (`{}` = wav path) or `LILLITH_PLAYER=none`.
- **Custom presets.** Create `~/.config/lillith-voice/presets.json`:
  ```json
  {"default": "bella80_nicole20",
   "brat": {"mix": {"af_bella": 0.7, "af_heart": 0.3}, "speed": 1.05, "pitch": 2.5}}
  ```
  Any `Preset` field can be set: `engine voice mix speed pitch highpass eq echo lufs true_peak temperature int8`.
  An entry with `mix` and no `engine` is a Kokoro preset. You can also set `LILLITH_VOICE_PRESET=lillith_fast`
  in the environment. If the Pocket models are missing, `lillith` presets fall back to `p1` with a warning
  (`LILLITH_VOICE_FALLBACK` to change that).

### Daemon (systemd --user)

```fish
systemctl --user enable --now lillith-voice     # done by install.sh --systemd
systemctl --user status lillith-voice
journalctl --user -u lillith-voice -f
systemctl --user edit lillith-voice              # e.g. Environment=LILLITH_THREADS=2
```

The unit uses `RestrictAddressFamilies=AF_UNIX`, so the daemon **can't open an IPv4/IPv6 socket at all**.
It only gets its own socket and PipeWire's. It runs with `Nice=5` and `LILLITH_THREADS=4` (all 4 logical CPUs
of the i5). Without systemd, run `lillith-voiced &` or the fish function `lillith-voice-start`.

Socket protocol (v2): see **[PROTOCOL.md](PROTOCOL.md)**. In short, one JSON object per line, one reply per line,
`cmd` and `op` are synonyms:
```
{"cmd":"say","text":"...","preset":"lillith","wait":true}   -> {"ok":true,"utt":7,"audio_s":4.2,"rtf":0.74,"first_audio_s":1.1,...}
{"cmd":"file","text":"...","out":"/abs/path.wav"}           {"cmd":"stop"}  {"op":"ping"}  {"cmd":"shutdown"}
{"cmd":"listen","action":"start"|"stop"}  {"cmd":"listen","timeout":15}  {"cmd":"listen_mode","on":true}  {"cmd":"stt","wav":"/abs.wav"}
{"op":"subscribe","topics":["lipsync","stt"]}   -> events: envelope (30 fps mouth level), chunk_start, chunk_end, utt_end, stop,
                                                   transcript, listening, barge_in, safeword
```

## 2b. Microphone: talking to her (local speech-to-text)

Everything runs on the laptop: silero VAD finds where you start and stop talking, and whisper tiny.en (int8,
~100 MB, via sherpa-onnx) transcribes. No audio leaves the machine.

```fish
lillith-listen --setup          # first run: pick the mic, 4 s test, shows the level and what it heard
                                # saves ~/.config/lillith-voice/mic.json (device, echo, barge_in, mode)
lillith-listen                  # say one thing, prints the transcript (VAD ends it when you stop)
lillith-listen --ptt            # push-to-talk: Enter to start, Enter to stop
lillith-listen --wav file.wav   # transcribe a file
lillith-listen --devices        # list capture devices
lillith-chat --ptt              # voice chat: Enter, talk, Enter -> llama-server -> she answers out loud (fish: lil-talk)
lillith-chat --mic              # always listening: just talk; she answers; talking over her interrupts her
```

- **Capture:** `pw-record` → `parec` → `arecord` (or `LILLITH_MIC_CMD`, `LILLITH_MIC_DEVICE`), 16 kHz mono.
- **Echo safety:** while she is talking the mic is *not* transcribed (`"echo":"mute"`, default), so laptop
  speakers can't make her answer herself. A 250 ms hangover covers the speaker tail.
- **Barge-in:** if you talk loudly over her (> −32 dBFS for 300 ms, tune `barge_in_dbfs` in `mic.json`) she
  stops at once, and what you said (kept in a 0.5 s pre-roll) is transcribed. Push-to-talk always interrupts her.
  With headphones you can set `"echo":"open"` for VAD-based barge-in.
- **Safety:** a spoken safeword (`red`, or `out of character, stop`) stops her immediately and is never sent to
  the LLM. A spoken "yes"/"I agree" is **never** treated as consent for a soul contract, plan or confirmation;
  `lillith-chat` asks for a typed confirmation instead. Every transcript carries `"consent_ok": false`.
- **Rust app:** the chat-bubble mic button uses `{"cmd":"listen","action":"start"}` / `"stop"` (push-to-talk)
  over the same socket, and subscribes to `stt` events. See PROTOCOL.md.

---

## 3. How it stays offline

- **No runtime downloads by design.** `kokoro-onnx` loads two local files. `espeak-ng` phonemization uses the
  library and data bundled in the `espeakng-loader` wheel, with the system `espeak-ng` as fallback. onnxruntime
  on Linux has no telemetry.
- **`netguard.py`** is imported first by every entry point. It sets `HF_HUB_OFFLINE=1`,
  `TRANSFORMERS_OFFLINE=1`, `HF_HUB_DISABLE_TELEMETRY=1`, `DO_NOT_TRACK=1`, `PIP_NO_INDEX=1`, and
  `ORT_DISABLE_TELEMETRY=1`. It also patches `socket.connect/connect_ex/sendto/getaddrinfo` so that
  **only AF_UNIX and loopback (127.0.0.1/::1) are allowed**. Anything else raises `NetworkBlocked`.
- **ffmpeg** gets only pipes, with `-protocol_whitelist pipe,file`.
- **`lillith-chat`** refuses any URL that isn't loopback and ignores `http_proxy`-style variables.
- **systemd** `RestrictAddressFamilies=AF_UNIX` on the daemon, as above.
- **Tested in a network namespace with no network at all.** Both the install and the full e2e suite ran
  under `unshare -rn`, with only `lo` present (§7).

---

## 4. Hooking it to llama-server (127.0.0.1:8080)

Start llama-server the same way the app's setup wizard does: loopback only, `--jinja` always, 4k context,
`-t` = physical cores (2 on the i5-7200U):
```fish
llama-server -m ~/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf --jinja --host 127.0.0.1 --port 8080 -c 4096 -t 2
```
Never `--host 0.0.0.0`. `lillith-chat` refuses any non-loopback `--url`.

**A. The bridge (simplest).** It streams `/v1/chat/completions` and speaks each sentence as soon as it
completes:
```fish
lillith-chat "hey you"                  # one turn
lillith-chat                            # interactive (keeps history; /quit)
lil -p bella85_heart15 "tell me a secret"
lillith-chat --url http://127.0.0.1:8080/v1/chat/completions --system "You are Lillith..." --max-tokens 200
```

**B. Any pipe.** `--stream` speaks sentence-by-sentence from stdin, and `--tee` echoes the text:
```fish
curl -sN http://127.0.0.1:8080/v1/chat/completions -H 'Content-Type: application/json' \
  -d '{"stream":true,"messages":[{"role":"user","content":"hi babe"}]}' \
  | sed -un 's/^data: //p' | grep --line-buffered -v '^\[DONE\]' \
  | jq --unbuffered -rj '.choices[0].delta.content // empty' \
  | lillith-say --stream --tee
```

**C. From code** (Python/Rust/anything). Write `{"cmd":"say","text":...,"wait":false}` lines to the unix
socket for each finished sentence, then send a final one with `"wait":true`. It returns after everything
queued before it has finished playing. `lillith_voice/chat.py` is a 100-line reference.

Tip: on a 2-core i5, llama-server and the voice compete for the same cores. Keep llama-server at `-t 2` (physical cores).
Speech for sentence 1 then renders while the LLM is still writing sentence 2. With both busy, expect
gaps between sentences (see §5).

---

## 5. Speed (real-time factor)

RTF = compute time ÷ audio duration. Lower is better, and < 1 means faster than real time.
**Measure on the laptop: `lillith-say --bench`** (renders three lines with the default preset and prints RTF,
CPU-seconds per audio-second and first-sentence time; if RTF > 0.9 it suggests `lillith_fast`). Compare with
`lillith-say --bench -p lillith_fast`.

Build-box numbers (shared 8-vCPU Xeon, **load average 20–58 from other jobs**, so they are pessimistic):

| | RTF | first audio | notes |
|---|---|---|---|
| `lillith` (Pocket fp32), one-shot, quiet moment | 0.69 | | 9.3 s line, 4 threads |
| `lillith`, daemon, under heavy load | 1.5–3.2 | 2.1–3.6 s | |
| `lillith_fast` (int8 flow head) | ~0.85–0.95× of fp32 (noisy on this loaded box) | | |
| `p1` / `bella` (Kokoro) | 0.25–0.54 | ~1 s | |
| whisper tiny.en int8 STT | 0.48 s for a 2 s utterance | | |
| model load (page-cached) | 1.4–3.4 s | | |

Published reference: Kyutai report Pocket TTS at about 6× real time on 2 cores of an Apple M4.
**Honest estimate for the i5-7200U** (2 cores / 4 threads, AVX2, thermal throttling): RTF ≈ 0.6–1.0 for
`lillith` on an idle laptop with the daemon, slightly better for `lillith_fast`. That is borderline: replies start
after 1–2 s and long replies may get short pauses between sentences, especially while llama-server generates
on the same cores. Mitigations: `LILLITH_VOICE_PRESET=lillith_fast`, keep llama-server at `-t 2` (physical cores), keep
replies short. ONNX Runtime's memory arena is off by default to save RAM (`LILLITH_ORT_ARENA=1` re-enables).
Measured daemon RSS (arena off): 0.67 GB idle after start, 0.79 GB after a `lillith` reply, 1.25 GB with every engine loaded (`lillith`, `lillith_fast`, `p1`, `bella`). That leaves plenty of room on 8 GB next to the LLM.

---

## 6. Plugging into wt-tts (`crates/lillith-voice`) without editing it

The crate's engine chain is **piper → espeak-ng → spd-say**. For Piper it runs
`piper --model M --output_raw [--length_scale X]` with the text on stdin. It plays raw s16le mono through
`aplay`/`paplay` at the `"sample_rate"` it reads from `M.json`, and checks only that `M` is a file.

**Zero-code option:** `bash install.sh --piper-shim` installs `~/.local/bin/piper` → the default preset
(Ko's clone; via the daemon if it's running) and creates a marker model with a `.json` saying 24000 Hz. Then put this in
`~/.config/lillith/config.toml`:

```toml
[voice]
engine = "piper"
model  = "~/.local/share/lillith-voice/lillith.onnx"          # marker; real model is in .../models
player = "paplay"     # or aplay
rate   = 1.0          # maps to --length_scale, then to the preset speed
```

Caveats: the shim renders the whole line (the crate caps lines at `max_chars = 240`) before audio starts, so
there's no sentence pipelining there. It also shadows a real `piper` binary, so the installer refuses to
overwrite one that isn't the shim. The preset comes from `LILLITH_VOICE_PRESET` or
`~/.config/lillith-voice/presets.json` `"default"`.

**Proper integration (Rust app):** the app's `DaemonEngine` talks to the daemon directly using
[PROTOCOL.md](PROTOCOL.md) (say / stop / ping, the mic button via `listen`, and lip sync via `subscribe`). The
daemon owns queueing, pipelining, playback, the mic and STT.

---

## 7. Tests

```fish
bash tests/e2e.sh                 # against the installed kit
NETNS=1 bash tests/e2e.sh         # same, inside an empty network namespace (proves offline); needs unshare -rn
python3 tests/transport_test.py   # transport only, no models: unix socket here, the named pipe on Windows
python3 tests/safeword_test.py    # safeword / consent classifier table (43 cases), no models
```

The suite covers:
- the safeword classifier (`tests/safeword_test.py`): a lone, short fuzzy "red" (red/read/rad/rid/rhett, "right"
  only alone) and "OOC stop" stop her; sentences ("that's right", "I read it", "stop reading") never do; voice is
  never consent
- the transport (`tests/transport_test.py`): framing, pipelining, 24 concurrent clients, a subscriber alongside
  `say`, `stop` from a second connection; socket 0600 / dir 0700 (Windows: pipe DACL, first-instance guard)
- every preset one-shot → WAV, and `lillith`/`p1`/`bella` via the daemon
- daemon playback (pipelined chunks), `--stream`, the piper shim, `--stop`, `op`/`cmd` aliases and `ping`
- lip-sync events from a subscriber (envelope frames, chunk_start/end, utt_end) and stop from a second connection
- speech-to-text with a **simulated microphone** (`tests/fake_mic.py`) and a real-time fake speaker
  (`tests/fake_player.py`): STT round trip on her own render, safeword, "yes" is never consent, VAD end of
  speech, VAD ignoring silence, push-to-talk, barge-in (stop + transcript), echo safety
- the full voice loop: mic → VAD → whisper → fake llama-server on 127.0.0.1 → reply spoken
- `lillith-chat` against the fake llama-server, refusal of non-loopback URLs, netguard blocking 1.1.1.1 and DNS
- WAV sanity: rate, −17/−16 ±3 LUFS, TP ≤ −0.5 dB, F0 in range

Real microphone and speaker hardware can't be tested on the build box; that part is simulated.

## Files

```
install.sh  uninstall.sh  requirements.lock  README.md  PROTOCOL.md
models/       pocket/ (Ko's clone)  stt/ (whisper tiny.en + silero VAD)  kokoro-v1.0.onnx  voices-v1.0.bin  SHA256SUMS
wheels/       pinned cp313+cp314 manylinux wheels (onnxruntime, sherpa-onnx, sentencepiece, numpy, ...)
lillith_voice/  netguard.py core.py pocket.py dsp.py textnorm.py speaker.py daemon.py transport.py client.py cli.py
                chat.py stt.py listen.py piper_shim.py
bin/          lillith-say lillith-voiced lillith-chat lillith-listen lillith-piper   (sh launchers)
fish/lillith-voice.fish   systemd/lillith-voice.service
windows/INSTALL-windows.md   android/INSTALL-android.md   android/ANDROID_ASSETS.md
tests/        e2e.sh bench.py check_wav.py fake_llama_server.py fake_mic.py fake_player.py lipsync_sub.py stt_test.py
              transport_test.py safeword_test.py
```

In the Lillith repo this folder (`tools/lillith-voice/`) is the source only: no `models/`, no `wheels/` (see `INSTALL.md`).
Full tarball: code + models + wheels. `-nomodels` tarball: code and docs only, no `models/` and no `wheels/` (copy both in yourself, or use `--online-pip` for the wheels).
