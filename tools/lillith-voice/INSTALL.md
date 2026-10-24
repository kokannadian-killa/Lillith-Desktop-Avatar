# Installing the Lillith voice kit from the repo source

This folder (`tools/lillith-voice/`) is the **source** of the offline voice kit (Python, protocol v2:
`lillith-voiced`, `lillith-say`, `lillith-listen`, `lillith-chat`). It contains **no model weights, no voice
samples and no wheels**. Those are big binaries, and they're shipped separately in the **full kit tarball**
`lillith-voice-kit.tar.gz` (≈731 MiB: this code plus `models/` plus `wheels/`). The `-nomodels` tarball equals this folder.

Nothing in the kit downloads anything at runtime. The daemon listens only on a local unix socket
(`$XDG_RUNTIME_DIR/lillith-voice/voiced.sock`, override `LILLITH_VOICE_SOCKET`; socket 0600, dir 0700) or, on Windows, the local named pipe
`\\.\pipe\lillith-voice`. `lillith_voice/netguard.py` blocks every non-loopback connection and DNS lookup in
every kit process. There is no telemetry.

## 1. Where the models go

`install.sh` looks for the models in this order and uses the first directory that has both Kokoro files:

1. `tools/lillith-voice/models/` (next to `install.sh`; git-ignored), or
2. `~/.local/share/lillith-voice/models/` (`$LILLITH_VOICE_HOME/models`, default
   `${XDG_DATA_HOME:-~/.local/share}/lillith-voice/models`). That's where the installer copies them and where
   the daemon reads them at runtime.

Runtime overrides: `LILLITH_VOICE_MODELS` (whole models dir), `LILLITH_POCKET_DIR` (cloned voice),
`LILLITH_STT_DIR` (speech-to-text). Layout and files (from the kit's `models/SHA256SUMS`, which the installer
checks when present):

| file (under `models/`) | bytes | size | needed for | sha256 |
|---|---:|---:|---|---|
| `kokoro-v1.0.onnx` | 325532387 | 310.5 MiB | **required** (installer stops without it) | `7d5df8ecf7d4b1878015a32686053fd0eebe2bc377234608764cc0ef3636a6c5` |
| `pocket/bos_before_voice.npy` | 4224 | 0.0 MiB | cloned voice | `f46edf4f7007b7ba4ea58831f49d003e59e167b4641c44bb3addfe9231a780b1` |
| `pocket/bundle.json` | 24381 | 0.0 MiB | cloned voice | `bab643150f437f37df080a710520ff39ed9ebd9a339f8ebdc739f7eddfc28b3f` |
| `pocket/flow_lm_flow.onnx` | 39097095 | 37.3 MiB | cloned voice (fp32; or the _int8 one) | `085d239f68897e28fb06e95c743738ad8b8c092ee6dc55f5491313e81ff08062` |
| `pocket/flow_lm_flow_int8.onnx` | 9962530 | 9.5 MiB | cloned voice, fast option | `3dd781ee5abee9e195320bf0106bebd6372a852b3b36352524ee78b40554635d` |
| `pocket/flow_lm_main.onnx` | 302742149 | 288.7 MiB | cloned voice (fp32; or the _int8 one) | `6d18315e2c33ca3e3aa4a4e3dca22f56d007fd823127e24948b37695bf54190f` |
| `pocket/flow_lm_main_int8.onnx` | 76341079 | 72.8 MiB | cloned voice, fast option (one precision is enough) | `f9bd8106b79a0192c1c43399ab938fb24900a95c1c599870d75a884e99000116` |
| `pocket/mimi_decoder.onnx` | 41471926 | 39.6 MiB | cloned voice | `86f038caa02a96a0ff9c25526a0ff43a4906c418197ed72d3e30f720ac7ce802` |
| `pocket/mimi_encoder.onnx` | 39768446 | 37.9 MiB | optional: only for `lillith-say --clone` | `853e2ca623b8782d94c3745ec6133bfdff7ce33d9b11128bd29ea03f28d76e3d` |
| `pocket/text_conditioner.onnx` | 16388344 | 15.6 MiB | cloned voice | `4ecee995fb69f85c7a7493d11f7b5ee15d9950facc7ab3f5c9c49ef1e03847bb` |
| `pocket/tokenizer.model` | 59339 | 0.1 MiB | cloned voice | `d461765ae179566678c93091c5fa6f2984c31bbe990bf1aa62d92c64d91bc3f6` |
| `pocket/voices/lillith.npy` | 835712 | 0.8 MiB | cloned voice (Ko's voice embedding) | `4c2ee51a893ddc8d2636b97e080e56c4881942288255c7f32e4cf6d766887e77` |
| `pocket/voices/lillith_ref.wav` | 780078 | 0.7 MiB | optional: reference clip, not used at runtime | `6e03c3517e27add820fa4924b0b7c1aa74719edccc503eb3a3bc9bd5b14b0f95` |
| `pocket/voices/lillith_ref_original.mp3` | 361189 | 0.3 MiB | optional: original reference, not used at runtime | `73aafa0ee0080be3fe1a185a95a2d0352edcc6caf5f154a1f8347f2a8417c6ca` |
| `stt/silero_vad.onnx` | 643854 | 0.6 MiB | microphone (VAD) | `9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6` |
| `stt/tiny.en-decoder.int8.onnx` | 89853865 | 85.7 MiB | microphone (whisper) | `06c0e6ff6348d427e51839219d1c886c18cfdf411e629e33f5e1679bff9c1527` |
| `stt/tiny.en-encoder.int8.onnx` | 12937772 | 12.3 MiB | microphone (whisper) | `0ce578b827c94a961aacb8fa14b02f096504b337e5c94be37c36238cbe3e8bc6` |
| `stt/tiny.en-tokens.txt` | 835554 | 0.8 MiB | microphone (whisper) | `306cd27f03c1a714eca7108e03d66b7dc042abe8c258b44c199a7ed9838dd930` |
| `voices-v1.0.bin` | 28214398 | 26.9 MiB | **required** | `bca610b8308e8d99f32e6fe4197e7ec01679264efed0cac9140fe9c29f1fbf7d` |

- **Kokoro** (`kokoro-v1.0.onnx` + `voices-v1.0.bin`, 337 MiB) is required. It powers presets `p1` / `bella*`
  and is the fallback voice.
- **`pocket/`** (≈504 MiB with both precisions) is Ko's cloned voice, the default preset `lillith`. If it's
  missing, the daemon reports `cloned_voice:false` and speaks with `p1`, and the app logs
  `using fallback voice p1 (cloned 'lillith' voice unavailable)`.
- **`stt/`** (≈100 MiB) is whisper tiny.en int8 plus silero VAD for the microphone. Without it, mic / `listen` /
  `stt` are disabled.

Get them from the full kit tarball (the release asset), not from the internet:

```sh
tar xzf lillith-voice-kit.tar.gz lillith-voice-kit/models lillith-voice-kit/wheels
mv lillith-voice-kit/models lillith-voice-kit/wheels tools/lillith-voice/
```

## 2. Install (Linux)

```sh
cd tools/lillith-voice
bash install.sh                  # offline: ./models + ./wheels, no network, no sudo
bash install.sh --systemd        # also enable the systemd --user daemon
```

- Python packages come only from `./wheels` (or `--wheels DIR`) with `pip --no-index`. Without wheels the
  installer **stops**. `--online-pip` (opt-in) fetches the pins in `requirements.lock` from PyPI, at install time
  only.
- System packages (`python ffmpeg rubberband espeak-ng pipewire pipewire-audio pipewire-pulse`) are only checked
  by default. `--pacman` (opt-in) installs them with `sudo pacman -S`.
- The installer never downloads a model.

Windows: `windows/INSTALL-windows.md`. Android: `android/INSTALL-android.md`.

## 3. Tests

| test | models? | how |
|---|---|---|
| `tests/safeword_test.py`: safeword / consent classifier, 43 cases (lone fuzzy "red", "OOC stop", sentences never trigger it, voice is never consent) | no (needs numpy) | `python3 tests/safeword_test.py` (or the venv python: `~/.local/share/lillith-voice/venv/bin/python tests/safeword_test.py`) |
| `tests/transport_test.py`: framing, pipelining, 24 clients, subscriber + say, stop from a 2nd connection, socket perms | no (fake core; needs numpy) | `~/.local/share/lillith-voice/venv/bin/python tests/transport_test.py` |
| `tests/e2e.sh`: full suite against the **installed** kit (all presets, daemon, lip-sync, STT with a simulated mic, safeword, barge-in, llama-server bridge, netguard, WAV checks; also runs the two tests above) | **yes**: all of `models/` installed | `bash tests/e2e.sh [outdir]`, or `NETNS=1 bash tests/e2e.sh` inside an empty network namespace (proves offline; needs `unshare -rn`) |
