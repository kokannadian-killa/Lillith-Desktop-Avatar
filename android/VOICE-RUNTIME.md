# Lillith's voice on Android (speech output only)

Lillith speaks her chat replies in Ko's cloned voice, fully on the phone. This is **speech output
only**. There is no microphone, no speech-to-text, and the APK has no `RECORD_AUDIO`, no
`INTERNET` and no `ACCESS_NETWORK_STATE` permission. `build-apk.sh` fails the build if any of
them shows up in `aapt dump permissions`.

Everything is local and offline. The models are the Pocket TTS files from the voice kit, shipped
inside the APK (`assets/voice/pocket/`). `AssetInstaller` copies them out and checks their SHA256
on first run. Nothing is downloaded at runtime and nothing is sent anywhere.

## How it works

| Piece | File | Ported from (desktop kit `tools/lillith-voice/lillith_voice/`) |
|---|---|---|
| Text cleanup + sentence split | `voice/SpeechText.java` | `core.clean_text`, `core.split_sentences`, `pocket.prepare/split`, `speaker.pause_after`, the per-line seed in `core.Voice.render` |
| Number/abbreviation normalizer | `voice/TextNorm.java` (+ `voice/PyRe.java`, which gives Python `re` semantics on `java.util.regex`) | `textnorm.normalize` |
| SentencePiece tokenizer (unigram + byte fallback, no native lib) | `voice/SpTokenizer.java` | sentencepiece 0.2.2 `Encode` on `pocket/tokenizer.model` |
| Pocket TTS on ONNX Runtime | `voice/PocketTts.java` | `pocket.PocketVoice` (`voice_state` primed once and cached, `stream`) |
| "polish" chain | `voice/Dsp.java` | `dsp.polish` (trim, HPF/EQ + STFT de-esser, loudness, limiter, fades), `crossfade_concat`, `fade`, `gap` |
| Sentence pipeline | `voice/PocketEngine.java` | `speaker.Speaker._play`: render sentence N+1 while N plays, reuse the first sentence's gain, natural pauses |
| Queue + hard stop | `voice/SpeechQueue.java` | `speaker.Speaker.stop` (generation counter) |
| Typed safeword | `voice/Safeword.java` | `lillith-core` `kill_switch::is_safeword` OR `stt._SAFE_RE` |
| Speaker output | `AudioTrackSink.java` | AudioTrack, `MODE_STREAM`, 24 kHz mono float, ~200 ms buffer |
| App entry point | `LillithVoice.java`, `VoiceFallback.java` | |

The Java classes in `voice/` have no Android imports, so they are tested on a plain JVM.

**Playback.** As on the desktop, each sentence is rendered in full (polish needs the whole
sentence for trim and loudness), then played while the next one renders. The first sentence is
kept short (120 chars) so audio starts sooner. The AudioTrack streams, so each sentence starts
playing as soon as it is ready.

**Safeword.** In the chat box, "red" or "OOC: stop" works even while she is thinking. It cuts
the audio within one 20 ms slice and flushes the AudioTrack. It drops every queued line, plus the
reply that was being generated when it arrived (the ticket taken at request time no longer
matches). It only stops her talking: the local AI keeps running. Typed matching is the desktop
chat rule OR the voice kit's regex. The kit's `fuzzy_red` is not used, because it is there for
whisper mishearings ("Read.", "Rad.", "Right.") that typed text can't have.

**Fallback.** If `AssetInstaller.clonedVoiceReady()` is false, or the model fails to load, replies
go to the `VoiceFallback` registered with `LillithVoice.get(ctx).setFallback(...)`, and the reason
is logged under tag `lillith_voice`. While the clone is loading (a few seconds after the first
chat), replies also go to the fallback. If no fallback is registered, that is logged once and the
reply is shown but not spoken.

**Sampling.** Temperature is 0.7 and the seed is 1234, the desktop "lillith" preset. The RNG is
`java.util.Random`, not numpy PCG64. With the same seed, a line is therefore a different take
than on the desktop, but still a stable one: the same line sounds the same every time on the
phone. At temperature 0 the output matches the desktop sample for sample (see the smoke test).

## ONNX Runtime and telemetry

- `com.microsoft.onnxruntime:onnxruntime-android:1.31.0` (AAR) is the same version as the desktop
  kit's `onnxruntime` wheel. `build-apk.sh` takes it from `ORT_AAR`, or fetches it once at build
  time from Maven Central into `$OUT/dl`, and checks its SHA256.
- Packaged: `jni/arm64-v8a/libonnxruntime.so`, `libonnxruntime4j_jni.so`, and the `ai.onnxruntime`
  Java API.
- **Telemetry.** ORT 1.31's Android AAR contains a 1DS telemetry uploader:
  - the classes `ai.onnxruntime.telemetry.*` and `TelemetryInitializer` (a ContentProvider);
  - in its manifest, `INTERNET` and `ACCESS_NETWORK_STATE`.

  Here is how it is kept out:
  1. The AAR manifest is never merged, so there is no provider and no permissions.
  2. The telemetry classes are removed before dexing, and the build fails if any remain.
  3. `OrtEnvironment.setTelemetry(false)` is called.
  4. The app has no INTERNET permission.

  Neither native library imports `socket`, `connect` or `getaddrinfo`. The native library's only
  path to the network is the stripped Java HTTP client.
- Sessions run on the CPU EP: intra-op 4 threads (at most the core count), inter-op 1, arena off,
  ORT_ENABLE_ALL. This matches the desktop.

### Pinned build-time downloads (SHA256)

| File | SHA256 |
|---|---|
| onnxruntime-android-1.31.0.aar (Maven Central; Maven sha1 09e434725ac3aea8a313f74d942c1b6e0ad5091e) | 72683787a80d0608a3121b967d8e1239aef48286826661c953211626690540a7 |
| ↳ jni/arm64-v8a/libonnxruntime.so (34,208,680 B) | 8005a741e57117b4d7b476417706c0e9af13cf1b89db47e60f0cbc56e1af91e0 |
| ↳ jni/arm64-v8a/libonnxruntime4j_jni.so | 5f2f1a99fd9f65fd6271eed825632f4ae5ada95f5cc13d66605888ccb80cdc29 |
| ↳ classes.jar | 06ae139810f5e552f64820d9a0d404620a47e30619f37051adb1017420224540 |
| onnxruntime-1.31.0.jar (desktop JVM jar, **tests only**, never packaged) | d3f903af31fe8f01300adb6b2b67297aa5bee7895909afc32e76ef29a3921149 |
| android-ndk-r27c-linux.zip (ndk;27.2.12479018) | 59c2f6dc96743b5daf5d1626684640b20a6bd2b1d85b13156b90333741bad5cc |
| build-tools_r34-linux.zip | e858c4b60069d0431051b225d384413b1643e1289b00a4825aed347f25bd510f |
| cmake-3.22.1-linux.zip | 9196644852a978012caf7a4067ba1898debf6cc204c3341562771e31080d6869 |
| platform-34-ext7_r03.zip | 16fdb74c55e59ae3ef52def135aec713508467bd56d7dabcd8c9be31fa8b20f3 |
| Temurin JDK 17.0.21+9 (linux x64 tar.gz) | 8b9b611d917e3dfd8419d3566b5949226f69b052fed7e109c1368add1dfe8711 |

The Google SDK zips also matched the sha1s in `repository2-3.xml`.

## APK size

`LILLITH_VOICE_STT` now defaults to `0`: the whisper/silero STT models are packed only with
`LILLITH_VOICE_STT=1`. The app is speaking-only and has no mic permission or STT runtime.

| Build (quality flavor, Kokoro off, debug-signed arm64) | APK size |
|---|---|
| `LILLITH_VOICE_STT=1` (previous default) | 988,303,995 B (942.5 MiB) |
| `LILLITH_VOICE_STT=0` (new default) | 884,502,828 B (843.5 MiB) |

The difference is 103,801,167 B (99.0 MiB), the four stored `assets/voice/stt/*` files. Both builds
have the same `aapt dump permissions`. Of the STT=0 size, the voice runtime itself adds about
13.0 MB: the ONNX Runtime `.so` files deflate to 12,887,720 + 35,935 B, and `classes.dex` grows
from 23,940 to 109,520 B compressed. Installed, the runtime is about +34.3 MB once the native
libraries are extracted.

## Tests (JVM, offline)

```
JAVA_HOME=<jdk17> PYTHON=<python with numpy+sentencepiece(+onnxruntime for --smoke)> \
LILLITH_VOICE_KIT=tools/lillith-voice LILLITH_VOICE_MODELS=<kit>/models \
ORT_CLASSES=<onnxruntime-1.31.0.jar or the AAR's classes.jar> android/voice-tests.sh [--smoke]
```

`app/test/gen_voice_fixtures.py` builds the fixtures from the Python kit, which is the reference.
They cover:
- hand-written cases plus seeded fuzz;
- textnorm, clean_text and split_sentences;
- tokenizer ids, including random Unicode for byte fallback;
- the full text chain down to token ids and seeds;
- the kit safeword regex;
- polish/crossfade on synthetic audio.

`VoiceTests` adds three more checks:
- the Rust `kill_switch` cases plus the typed-applicable `tests/safeword_test.py` cases;
- a queue test: a stop mid-line writes no more audio, drops the 4 queued lines, and logs it; a
  stale ticket is dropped; 50 stop/say races;
- a player-stop test.

`--smoke` renders a line through `PocketEngine.render` on the desktop ORT JVM build, at
temperature 0, and compares it with the Python `Voice.render` path.

## Desktop quirks mirrored on purpose (parity), worth fixing in the kit

- `textnorm`: `"at 5:30 today"` becomes `"at five thirtytoday"` (the time regex's trailing `\s*`
  eats the space).
- `textnorm`: `"I have 5, maybe 6"` becomes `"I have five maybe six"` (the number regex `[\d,]*`
  eats a trailing comma).
- Emoji/unassigned detection uses the JVM/Android Unicode tables. Python's `unicodedata` version
  can differ for very new code points.

## Not verified

- No real-device test: everything above ran on an x86-64 box on the JVM. On-device RTF, first-audio
  latency, memory (fp32 `flow_lm_main` is 303 MB) and AudioTrack behaviour are untested.
- The fallback voice (`VoiceFallback`, Android TextToSpeech) is not implemented here. The polish bot
  owns it.
