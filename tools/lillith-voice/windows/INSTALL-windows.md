# Lillith's voice on Windows (Ko's cloned voice, fully offline)

> **Status: tested under wine, not yet on real Windows.** The build box is Linux. The complete payload this
> folder builds (`build-voice-win.sh`) was run under **wine 10 with the network off** (`unshare -rn`, loopback only):
> the embeddable Python imports onnxruntime 1.31 / sherpa-onnx 2.0, the daemon comes up on `\\.\pipe\lillith-voice`
> and answers `ping`, `say` works with `lillith` (cloned voice) and `p1` (bundled ffmpeg + rubberband), STT
> transcribes a wav, the safeword "red" is recognised, stop from a 2nd connection, second-start refusal,
> `--stop` and `--start-app` all pass, and `tests/transport_test.py` passes 16/16.
> Not tested: real Windows, `winsound` playback into real speakers, a real microphone. (Wine 10's own
> `ucrtbase.dll` lacks `crealf`, which numpy needs, so the wine test used Microsoft's native `ucrtbase.dll`;
> real Windows 10/11 ships it, so it is **not** bundled.)

## What the installer bundles (no downloads at install or run time)

`windows/installer.nsi` (section **Lillith's voice**, on by default) puts everything under
`%LOCALAPPDATA%\Lillith\voice\` (per-user, no admin, no UAC prompt):

| part | what | approx. size |
|---|---|---:|
| `python\` | **Python 3.13.17 embeddable** (python.org, sigstore-verified), `python313._pth` = `python313.zip`, `.`, `Lib\site-packages`, `..\lib`, `import site`; app-local **MSVC runtime** `msvcp140*.dll` (Microsoft-signed, from conda-forge `vc14_runtime`: onnxruntime, sentencepiece and espeak-ng need it, and the app-local copy avoids the admin-only vc_redist) | 188 MB with site-packages |
| `python\Lib\site-packages\` | 20 win_amd64 wheels pinned in `voice-win.lock.json` (url + sha256), installed offline at *build* time: onnxruntime (CPU only), sherpa-onnx, numpy, soundfile (libsndfile), sounddevice (PortAudio), kokoro-onnx, phonemizer, espeakng-loader, sentencepiece … The named pipe needs no pywin32: it is stdlib ctypes | (above) |
| `lib\lillith_voice\` | this kit's Python package | <1 MB |
| `models\` | pocket-tts + Ko's voice (`pocket\voices\lillith.npy`), Kokoro (`p1`/bella), whisper + VAD (`stt\`), checked against `models\SHA256SUMS` at build time | 941 MB |
| `bin\ffmpeg.exe` | static ffmpeg 8.1 with rubberband (BtbN win64-gpl): the `p1` preset and `--speed/--pitch`. Without it `p1` falls back to a numpy approximation | 158 MB |
| `tests\`, `licenses\`, `*.cmd`, `SHA256SUMS`, `PAYLOAD_ID` | self-tests, third-party licenses, launchers, the payload's own checksums | small |

Total ~1.3 GB unpacked (~790 MB zipped). Build it (Linux, network at build time only):

```sh
tools/lillith-voice/windows/build-voice-win.sh --kit-tarball lillith-voice-kit-<sha>.tar.xz --out <dir>
# -> <dir>/voice/ and <dir>/lillith-voice-win-<id>.zip (+ .sha256)
VOICE_PAYLOAD=<dir>/lillith-voice-win-<id>.zip windows/build.sh     # app installer + portable zip with voice
```

Every download is pinned by sha256 in `voice-win.lock.json` and cached (`CACHE=`); a mismatch stops the build.
The launchers are checked to contain no download commands.

## How it runs

- **Autostart:** `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\LillithVoice` =
  `"%LOCALAPPDATA%\Lillith\voice\python\pythonw.exe" -m lillith_voice.winlaunch` (hidden, no console). A second
  start exits at once (the pipe is created with `FILE_FLAG_FIRST_PIPE_INSTANCE`). The installer also starts it
  right away. Turn it off in Task Manager ▸ Startup apps, or untick the section.
- If **Start Lillith when I sign in** is ticked too, that Startup shortcut goes through
  `winlaunch --start-app "<install dir>\lillith.exe"`: it waits until the voice answers (≤ 60 s, starting it if the
  Run entry is off), then starts Lillith. That way she does not fall back to SAPI because she started first.
- The app finds the daemon on `\\.\pipe\lillith-voice` with no configuration (`[voice] engine = "auto"`); override
  with `LILLITH_VOICE_PIPE` on both sides. The daemon finds its models next to its own `python\`, so the
  portable copy (`Lillith\voice\` in the portable zip, start with `Start Lillith with voice.cmd`) works from any folder.
- **Log:** `%LOCALAPPDATA%\Lillith\voice\logs\voiced.log` (rotated at 2 MB).
- **Settings:** `%APPDATA%\lillith-voice\` (`mic.json`, `presets.json`); your own cloned voices:
  `%LOCALAPPDATA%\Lillith\voice\voices\` (kept on upgrade; removed on uninstall only if you choose to delete settings).
- **Upgrade / uninstall** stop it with `python.exe -m lillith_voice.winlaunch --stop` (a `shutdown` over the
  pipe), never a taskkill on python, and remove the Run entry and the bundled files.
- `netguard` runs first in every process and blocks every non-loopback socket; onnxruntime uses the CPU provider only.

## Transport: a local named pipe, never TCP

- `\\.\pipe\lillith-voice`, created through stdlib ctypes (`CreateNamedPipeW`): byte mode,
  `PIPE_REJECT_REMOTE_CLIENTS`, `FILE_FLAG_FIRST_PIPE_INSTANCE`, one instance per client, protected DACL
  `D:P(D;;GA;;;NU)(A;;GA;;;<your SID>)` (only the current user; network logons denied).
- Nothing binds `0.0.0.0`, `127.0.0.1` or any TCP/UDP port.
- Same JSON-lines protocol as Linux: see [`../PROTOCOL.md`](../PROTOCOL.md) ("Windows named pipe").

## Audio

- Playback: built-in `winsound` (no extra DLLs) unless `LILLITH_PLAYER` is set.
- Mic: `sounddevice` (PortAudio, WASAPI). Start menu ▸ Lillith ▸ *Lillith voice - microphone setup*
  (`lillith-listen.cmd --setup`) picks the device and writes `%APPDATA%\lillith-voice\mic.json`.
- Without the voice the app uses Windows' SAPI voice and says so. It doesn't sound like Ko.

## Check after install (cmd, no network needed)

```bat
cd /d %LOCALAPPDATA%\Lillith\voice
lillith-voiced.cmd --ping
lillith-say.cmd "Hiii, it's me, on Windows."
lillith-say.cmd --preset p1 "And this is the backup voice."
python\python.exe tests\win_smoke.py %TEMP%\lillith-smoke     &rem full self-test on a private pipe: ALL PASSED
python\python.exe tests\transport_test.py                      &rem pipe transport only: ALL PASSED
```
