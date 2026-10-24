# Lillith's voice on Android (Ko's cloned voice, in the APK)

> **Status: documented, not tested.** No Android build or device was available for the voice. Asset list,
> sizes and hashes are exact; the port is a plan.

- **No `INTERNET` permission.** Every model is inside the APK (see [`ANDROID_ASSETS.md`](ANDROID_ASSETS.md)):
  Pocket TTS graphs + Ko's voice embedding, whisper tiny.en + silero VAD. Nothing is fetched at runtime.
- **No daemon, no sockets.** The engine runs in-process: Kotlin → ONNX Runtime (`onnxruntime-android` AAR,
  `arm64-v8a`) for Pocket TTS, and sherpa-onnx (JNI) or plain ORT for STT. The JSON protocol in
  `PROTOCOL.md` maps to in-process calls (say / stop / listen / lip-sync callbacks).
- **Recommended flavour: "quality"** = fp32 `flow_lm_main` + `flow_lm_flow` (fp32, or its int8 version to
  save 28 MB) + fp32 `mimi_decoder` (≈401 MB of TTS models, APK ≈ 530–545 MB with STT). The "small" flavour
  (int8 `flow_lm_main`, APK ≈ 305–320 MB) is experimental: in testing it sometimes stopped early or said the
  wrong words, so it needs a duration check and re-render. Never ship both main and flow as int8 (she loops).
- **Mic:** `RECORD_AUDIO` runtime permission, `AudioRecord` 16 kHz mono → silero VAD → whisper. Echo safety and
  barge-in as on desktop (mic muted while she talks; loud speech over her stops her).
- **Fallback:** Android `TextToSpeech` with an offline system voice if the models are missing or the phone is
  too slow (check RTF on first run; a mid-range arm64 phone should manage fp32 at around real time, but this
  is an estimate).
- **Build:** `android/link_assets.sh <app>/src/main/assets quality` symlinks the models into the assets dir
  (never copies). Store `.onnx` uncompressed (`noCompress`).
