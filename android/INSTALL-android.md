# Lillith on Android

Lillith floats over your other apps, plays the same clips as on the desktop,
reacts to taps, falls to the bottom of the screen when you drop her, and can
chat using a model that runs **entirely on your phone**.

Everything Android-specific lives in this `android/` folder:

| Path | What it is |
|---|---|
| `app/AndroidManifest.xml` | Package `com.kokannadian.lillith`, minSdk 26 (Android 8), targetSdk 34 |
| `app/src/…/SetupActivity.java` | First-run setup: overlay permission, notification, pick a GGUF model, size, start/hide |
| `app/src/…/OverlayService.java` | Foreground service that draws her in a `TYPE_APPLICATION_OVERLAY` window, forwards touches, shows her speech bubble |
| `app/src/…/ChatActivity.java`, `LocalLlm.java` | Chat with Lily via on-device llama.cpp |
| `app/src/…/MenuActivity.java` | Long-press menu (chat, kiss, wave, dance, settings, hide) |
| `rust/` (`lillith-android` crate) | The desktop brain (`lillith-core`), animator and clips (`lillith-anim`) driven over JNI; gravity, drag, tap/long-press, LRU page cache. The JNI glue is behind `cfg(target_os = "android")`; the rest builds and is tested on Linux as part of the workspace |
| `llm/` | `liblillith_llm.so`: a ~200-line C JNI shim over llama.cpp (pinned tag `b11829`), CPU-only |
| `build-apk.sh` | One-shot build of the debug APK (no Gradle needed) |

## Install (Ko)

1. Copy `lillith-debug-arm64.apk` to the phone (or `adb install -r lillith-debug-arm64.apk`).
   Opening the file on the phone asks to allow installs from that app (Files/Chrome): allow it once.
   It's a debug build signed with a debug key, so Play Protect may warn: choose **Install anyway**.
2. Open **Lillith**. The setup screen walks you through:
   1. **Float over other apps**: tap the button, find Lillith, turn on *Allow display over other apps*, come back.
   2. **Notification** (Android 13+): allow it. Android requires a small "Lillith is on screen" notification
      while she's out. It also has **Chat** and **Hide** buttons.
   3. **Local chat model (optional)**: first copy a `.gguf` file onto the phone yourself (USB, file manager).
      Tap **Pick GGUF file…** and choose it. It's copied into Lillith's private storage, which needs free space
      equal to the file size. A 1–4B model at Q4_K_M (0.7–2.5 GB) suits most phones; 7–8B needs 8 GB+ RAM.
      You can skip this: she still plays, she just can't chat.
   4. **Size**: 100% = the desktop default (0.55 × the 400 px clips, in dp, so the same apparent size).
   5. **Start Lillith**. The first start unpacks her clips (~270 MB, takes a few seconds).
3. Using her:
   - **Tap** her: the same reactions as a desktop click (double tap etc. go through the same brain).
   - **Hold** (½ s): the menu.
   - **Drag** her anywhere. **Let go** and she falls to the bottom of the screen with a soft bounce.
   - She idles, cycles flirty poses, wanders along the bottom, and falls asleep when left alone, like on desktop.
   - **Hide**: notification → *Hide*, or menu → *Hide Lillith*. Settings: open the app again.

Arm64 phones only (nearly every phone from 2017 on). Android 8.0+.

## Privacy: fully local

- The APK has **no `INTERNET` permission**, so Android blocks every socket the app could open, loopback
  included. Check with `aapt2 dump permissions lillith-debug-arm64.apk`.
- The chat model is llama.cpp **inside the app process** (JNI). There is no server, not even on loopback,
  which is why INTERNET isn't needed.
- No model downloads, no telemetry, no analytics, no update checks. The only model is the GGUF you pick.
- `allowBackup="false"`: chats and the model aren't sent to Google backup.

## Build it yourself

Needs Linux x86_64 and about 4 GB for the toolchain:

```bash
# Rust target (rustup toolchain 1.85.1, same as the desktop gate)
rustup target add aarch64-linux-android
# Android SDK command-line tools + JDK 17, then:
sdkmanager "platforms;android-34" "build-tools;34.0.0" "ndk;27.2.12479018" "cmake;3.22.1"

export ANDROID_HOME=/path/to/sdk JAVA_HOME=/path/to/jdk-17
export CARGO_TARGET_DIR=/path/to/target-android      # optional
export LLAMA_CPP_DIR=/path/to/llama.cpp              # optional; else cloned at tag b11829 at build time
android/build-apk.sh
# -> $CARGO_TARGET_DIR/apk/lillith-debug-arm64.apk
```

What the script does: `cargo build --release --target aarch64-linux-android -p lillith-android` →
CMake/NDK builds the llama.cpp shim → copies `assets/generated/manifest.toml` + every `*_sheet.png`
(art and manifest unchanged) → `javac` + `d8` → `aapt2 link` → `zipalign` → `apksigner` with a generated
debug key (`$OUT/debug.keystore`, not committed). The only network access is the optional one-time
llama.cpp source clone, at build time.

The Linux gate is unaffected: `lillith-android` is a workspace member whose JNI module only compiles for
Android, so `cargo fmt --check`, `build --locked`, `test --locked` and `clippy -D warnings` cover its engine
and tests on Linux.

## Known limits

- The overlay window is a rectangle around her. Touches on transparent pixels inside it are ignored, but
  Android doesn't pass them through to the app underneath.
- No window perching (Android has no window list for other apps); she lands on the bottom of the screen.
- No sound or voice yet. The desktop sensors (CPU, Bluetooth, notifications…) aren't wired on Android.
- Chat replies arrive all at once (no token streaming yet) and appear in her bubble when she's on screen.
