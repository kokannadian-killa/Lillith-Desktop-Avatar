# Android assets for Lillith's voice (Ko's clone) — list only, nothing copied

**Nothing in this file is copied into `android/assets/`.** The models are large, and the box has limited disk.
This is the exact list the APK build needs, with source paths, sizes and SHA-256, so the Android build can
copy (or symlink) them at build time. `link_assets.sh` in this folder creates **symlinks only** if you ask it to.

Source of truth (both hold the same bytes):
- the kit: `/workspace/lillith/lillith-voice-kit/models/` (also inside `lillith-voice-kit.tar.gz`)
- an installed laptop: `~/.local/share/lillith-voice/models/`

Verify with `cd models && sha256sum -c SHA256SUMS --ignore-missing`.

## 1. TTS: Pocket TTS ONNX (english_2026-04), Ko's cloned voice

| asset (under `models/`) | bytes | MiB | SHA-256 | needed? |
|---|---:|---:|---|---|
| `pocket/text_conditioner.onnx` | 16,388,344 | 15.6 | `4ecee995fb69f85c7a7493d11f7b5ee15d9950facc7ab3f5c9c49ef1e03847bb` | yes |
| `pocket/flow_lm_main.onnx` (fp32) | 302,742,149 | 288.7 | `6d18315e2c33ca3e3aa4a4e3dca22f56d007fd823127e24948b37695bf54190f` | **yes (recommended)** |
| `pocket/flow_lm_main_int8.onnx` | 76,341,079 | 72.8 | `f9bd8106b79a0192c1c43399ab938fb24900a95c1c599870d75a884e99000116` | experimental small build only (see warning) |
| `pocket/flow_lm_flow.onnx` (fp32) | 39,097,095 | 37.3 | `085d239f68897e28fb06e95c743738ad8b8c092ee6dc55f5491313e81ff08062` | yes |
| `pocket/flow_lm_flow_int8.onnx` | 9,962,530 | 9.5 | `3dd781ee5abee9e195320bf0106bebd6372a852b3b36352524ee78b40554635d` | optional (safe on its own: saves 28 MB, 6/6 test renders correct) |
| `pocket/mimi_decoder.onnx` (fp32) | 41,471,926 | 39.6 | `86f038caa02a96a0ff9c25526a0ff43a4906c418197ed72d3e30f720ac7ce802` | yes (keep fp32; int8 decoder hurts similarity most) |
| `pocket/mimi_encoder.onnx` | 39,768,446 | 37.9 | `853e2ca623b8782d94c3745ec6133bfdff7ce33d9b11128bd29ea03f28d76e3d` | **no**: only for cloning a new voice; Ko's voice is precomputed |
| `pocket/tokenizer.model` (SentencePiece) | 59,339 | 0.06 | `d461765ae179566678c93091c5fa6f2984c31bbe990bf1aa62d92c64d91bc3f6` | yes |
| `pocket/bundle.json` (graph I/O + KV-state manifest) | 24,381 | 0.02 | `bab643150f437f37df080a710520ff39ed9ebd9a339f8ebdc739f7eddfc28b3f` | yes |
| `pocket/bos_before_voice.npy` | 4,224 | 0.004 | `f46edf4f7007b7ba4ea58831f49d003e59e167b4641c44bb3addfe9231a780b1` | yes |
| `pocket/voices/lillith.npy` (Ko's voice embedding, float32 (1,204,1024)) | 835,712 | 0.8 | `4c2ee51a893ddc8d2636b97e080e56c4881942288255c7f32e4cf6d766887e77` | yes |

**Warning, tested:** don't run *both* `flow_lm_main` and `flow_lm_flow` as int8. The errors compound and
she loops ("so, so, so…"). int8 `flow_lm_main` alone is **not reliable either**: in a 6-render test 2 renders
stopped early or said the wrong words ("Yeah." instead of the line), and one e2e run produced gibberish. int8
`flow_lm_flow` alone was 6/6 correct. So ship fp32 `flow_lm_main`; use the int8 transformer only if the APK
must be small, and then check each chunk's duration against its text length and re-render (fp32 or a new
seed) when it is too short.

Licence: Pocket TTS weights CC-BY-4.0 (Kyutai Labs); ONNX export by KevinAHM (Apache-2.0 wrapper). Put
"Pocket TTS by Kyutai Labs, CC-BY-4.0" in the app's About/licences screen.

## 2. STT: whisper tiny.en int8 + silero VAD (for the mic)

| asset | bytes | MiB | SHA-256 |
|---|---:|---:|---|
| `stt/silero_vad.onnx` | 643,854 | 0.6 | `9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6` |
| `stt/tiny.en-encoder.int8.onnx` | 12,937,772 | 12.3 | `0ce578b827c94a961aacb8fa14b02f096504b337e5c94be37c36238cbe3e8bc6` |
| `stt/tiny.en-decoder.int8.onnx` | 89,853,865 | 85.7 | `06c0e6ff6348d427e51839219d1c886c18cfdf411e629e33f5e1679bff9c1527` |
| `stt/tiny.en-tokens.txt` | 835,554 | 0.8 | `306cd27f03c1a714eca7108e03d66b7dc042abe8c258b44c199a7ed9838dd930` |

Licences: whisper MIT (OpenAI), silero-vad MIT; sherpa-onnx Apache-2.0.

## 3. Runtime libraries (native, per ABI)

- **ONNX Runtime for Android**: Maven `com.microsoft.onnxruntime:onnxruntime-android`. 1.24.2 is verified on
  Maven Central (AAR 40.8 MB for all ABIs); any newer 1.2x/1.3x release is fine (the desktop kit uses
  onnxruntime 1.31.0; the graphs to ship are ONNX opset 13–17, IR ≤ 8, checked; `mimi_encoder` is opset 18 but is not shipped). The AAR ships `arm64-v8a`, `armeabi-v7a`,
  `x86` and `x86_64`. **Ship `arm64-v8a` only** for Ko's phone (ABI split / `abiFilters 'arm64-v8a'`), add
  `x86_64` only for the emulator. Use the Java/Kotlin API (`ai.onnxruntime.OrtSession`) or link
  `libonnxruntime.so` from your own JNI/C++.
- **sherpa-onnx (STT)**: the k2-fsa/sherpa-onnx Android build (desktop kit uses 2.0.0) gives Kotlin APIs for
  `OfflineRecognizer` (whisper) and `Vad` (silero). **Pitfall:** it bundles its own `libonnxruntime.so`. Two
  different ORT copies in one APK collide. Either (a) use sherpa-onnx's ORT for Pocket as well (call the ORT
  C API from your JNI against sherpa's `libonnxruntime.so`), or (b) build sherpa-onnx against the same ORT
  version as the AAR, or (c) skip sherpa and run silero + whisper directly with ORT (silero is ~60 lines,
  whisper greedy decoding ~200).
- **SentencePiece (Pocket's text tokenizer)**: there is no official Android artifact. Options: build
  `sentencepiece` (Apache-2.0, plain C++) with the NDK into your JNI lib, or port the unigram encoder
  (the model is a 59 KB protobuf; ~150 lines of Kotlin). **No espeak-ng / G2P is needed for Pocket**: it reads
  SentencePiece tokens directly. (Only the Kokoro presets need espeak-ng data, and they are not needed on Android.)

## 4. What the Android code must port (from `lillith_voice/`)

- `pocket.py` → Kotlin/C++: text_conditioner → `flow_lm_main` (KV state per `bundle.json` manifest) →
  `flow_lm_flow` (1 LSD step) → `mimi_decoder`, 24 kHz, 12.5 latent frames/s, temperature 0.7, EOS logit > −4
  (ignored before ~1.5 frames per token), Ko's state primed once from `lillith.npy` + `bos_before_voice.npy` and
  cached. Stream 3 frames first, then 12-frame blocks to `AudioTrack` (MODE_STREAM, 24000 Hz, PCM_FLOAT).
- `textnorm.py` (tildes, "Hehe.", mmm, numbers/money/times, abbreviations) and the sentence splitter.
- `dsp.py` polish is optional on a phone speaker; at least do loudness (−17 LUFS) and the limiter.
- Fallback when the models are missing or the device is too slow: Android `TextToSpeech` (system engine,
  offline voices only). It doesn't sound like Ko.

## 5. APK size estimate (arm64-v8a only)

| build | TTS models | STT models | native libs | **APK ≈** |
|---|---:|---:|---:|---:|
| **quality (recommended)**: fp32 `flow_lm_main` | 401 MB | 104 MB | ~25–40 MB | **~530–545 MB** |
| small (experimental, see warning): int8 `flow_lm_main` | 175 MB | 104 MB | ~25–40 MB | **~305–320 MB** |
| small, no mic | 175 MB | 0 | ~20–30 MB | ~195–205 MB |

TTS sums: text_conditioner + main + flow (fp32) + mimi_decoder + tokenizer/json/bos + `lillith.npy`.
ONNX weights barely compress, so store them **uncompressed** (`androidResources { noCompress 'onnx', 'npy', 'model' }`)
so they can be memory-mapped from the APK (or copied once to `filesDir` on first run, which doubles storage
use). This is a sideloaded APK; Play's 150 MB base limit would need Play Asset Delivery instead.

RAM on the phone: roughly weights + ~60 MB KV state + ORT overhead: ~0.6 GB (quality) / ~0.35 GB (small).
Turn off the ORT CPU arena (`setCPUArenaAllocator(false)`), as the desktop kit does.

## 6. Privacy

No `INTERNET` permission: everything above is inside the APK, nothing is fetched at runtime. The mic needs
`RECORD_AUDIO` (runtime permission) and is used only while listening.
