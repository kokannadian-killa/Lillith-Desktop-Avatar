# Lillith's voice on Windows (Ko's cloned voice, fully offline)

> **Status: partly tested.** The build box is Linux.
> - The named-pipe transport (`lillith_voice/transport.py`, stdlib ctypes) passes `tests/transport_test.py` under
>   **wine 10 + the official Python 3.13 embeddable build**. That covers framing, concurrent clients, stop from a
>   2nd connection, a lip-sync subscriber, the DACL read back from the live pipe, and the first-instance guard.
> - It has **not** run on real Windows yet.
> - The audio paths (`winsound` playback, `sounddevice` mic) and the full daemon with models are untested on Windows.
> - Linux is the tested platform.

## What the installer bundles (no downloads at install or run time)

The Windows installer (`windows/installer.nsi` in the app repo) puts everything under
`%LOCALAPPDATA%\Lillith\voice\`:

| part | what | approx. size |
|---|---|---:|
| `python\` | **Python 3.13 embeddable** zip (python.org `python-3.13.x-embed-amd64.zip`), with `python313._pth` edited to add `Lib\site-packages` and `..\lib` | 11 MB |
| `python\Lib\site-packages\` | the pins in `requirements.lock`, **win_amd64 / py3 wheels** pre-installed at *build* time (`pip install --no-index --find-links wheels-win -r requirements.lock --target ...`), plus `sounddevice` (mic + playback via PortAudio, bundled DLL). The named pipe needs **no** pywin32: it is stdlib ctypes | ~120 MB |
| `lib\lillith_voice\` | this kit's Python package | <1 MB |
| `models\pocket\`, `models\stt\` | the same files and SHA-256 as `models/SHA256SUMS` (see `android/ANDROID_ASSETS.md` for the table). Kokoro (`kokoro-v1.0.onnx`, `voices-v1.0.bin`) only if the `p1`/bella presets are wanted | 604 MB (+338 MB Kokoro) |
| `ffmpeg.exe` | optional; only for the `p1` preset and `--speed/--pitch` (rubberband). The default `lillith` preset's polish is pure numpy | 80–130 MB |

The packager fetches the win_amd64 wheels **once on the build machine** (`pip download --only-binary=:all:
--platform win_amd64 --python-version 3.13 -r requirements.lock -d wheels-win`); the user's machine never
touches the network. `kokoro-onnx` and `espeakng-loader` are only needed for the Kokoro presets.

## Transport: a local named pipe, never TCP

- The daemon (`python -m lillith_voice.daemon`) listens on **`\\.\pipe\lillith-voice`** (override with
  `LILLITH_VOICE_PIPE`).
- It is created through stdlib ctypes (`CreateNamedPipeW`), with no pywin32:
  - byte mode;
  - `PIPE_REJECT_REMOTE_CLIENTS`;
  - `FILE_FLAG_FIRST_PIPE_INSTANCE`, so the daemon won't start if the name is already taken (already running, or
    squatted);
  - one instance per client, so concurrent connections work;
  - a protected DACL `D:P(D;;GA;;;NU)(A;;GA;;;<your SID>)`, which allows **only the current user** and denies network
    logons.
- Nothing binds `0.0.0.0`, `127.0.0.1` or any TCP/UDP port.
- Same JSON-lines protocol as Linux: see [`../PROTOCOL.md`](../PROTOCOL.md) ("Windows named pipe"). The Rust app
  opens the pipe with `std::fs::OpenOptions` (read + write) and writes one JSON object per line.
  - It should retry for about 1 s on `ERROR_PIPE_BUSY` (231).
  - It should use separate connections for say, stop and subscribe.
- Self-test on the Windows machine (no models needed):
  run `%LOCALAPPDATA%\Lillith\voice\python\python.exe tests\transport_test.py` from an unpacked copy of this kit. It
  should print `ALL PASSED`.
- `netguard` still runs first in every process and blocks every non-loopback socket.

Start it at logon from the app (spawn hidden: `pythonw.exe -m lillith_voice.daemon --threads 4`) or with a
Startup shortcut. `windows\lillith-say.cmd "text"`, `lillith-listen.cmd` and `lillith-voiced.cmd` (in this
folder) wrap `python -m lillith_voice.cli` / `.listen` / `.daemon`. (The embeddable Python ignores
`PYTHONPATH`, so the `._pth` edit above is what makes `lib\` importable.)

## Audio

- Playback: built-in `winsound` (no extra DLLs), or `sounddevice` when installed (needed for gap-free
  streaming and barge-in timing). `LILLITH_PLAYER` works as on Linux.
- Mic: `sounddevice` (PortAudio, WASAPI). `lillith-listen --setup` picks the device and writes
  `%APPDATA%\lillith-voice\mic.json`.
- The Windows app's existing SAPI voice (`[voice] engine = "auto"`) stays as the fallback when the kit is
  not installed. It doesn't sound like Ko.

## Check after install (on the Windows machine)

```bat
%LOCALAPPDATA%\Lillith\voice\python\python.exe -m lillith_voice.cli --status
%LOCALAPPDATA%\Lillith\voice\python\python.exe -m lillith_voice.cli --bench
%LOCALAPPDATA%\Lillith\voice\python\python.exe -m lillith_voice.cli "Hiii, it's me, on Windows."
```
