# Installing Lillith

How to install and start Lillith from the release zip. The zip ships **Linux
binaries only**. On Windows you build from source with Rust 1.85. On Android you chat
with her model in Termux (plus an experimental APK, see below).

Everything runs on your own machine. Lillith only talks to `127.0.0.1`
(loopback), and nothing is downloaded while she runs. You get the model file
yourself.

- [Linux (Garuda / Arch, KDE Plasma 6 Wayland)](#linux-garuda--arch-kde-plasma-6-wayland)
- [Windows (build from source)](#windows-build-from-source)
- [Android (Termux chat, experimental APK)](#android-termux-chat-experimental-apk)
- [The safeword (kill switch)](#the-safeword-kill-switch)
- [Troubleshooting](#troubleshooting)

---

## Linux (Garuda / Arch, KDE Plasma 6 Wayland)

All commands are for the **fish** shell.

### 1. Install the dependencies

```fish
sudo pacman -S --needed unzip ffmpeg libxkbcommon wayland dbus libpulse systemd-libs alsa-lib \
    libx11 libxcb libxcursor libxi libxrender libglvnd mesa
```

`ffmpeg` decodes her MP4 poses. Without it she falls back to the PNG sheets.
A normal Plasma desktop already has most of the rest.

For chat you also need llama.cpp's `llama-server` (the `llama.cpp` package,
from pacman or an AUR helper you run yourself).

### 2. Unzip and start her

```fish
unzip Lillith-<version>.zip
cd Lillith-<version>
chmod +x bin/lillith bin/lillith-settings run.sh run.fish kwin-blur-exclude.sh   # if unzip dropped the executable bits
./run.fish
```

`run.fish` does the following:
- sets `LILLITH_MANIFEST` to the bundled art
- uses `~/.config/lillith/config.toml` if it exists, otherwise the bundled `config.toml` (unless you pass `--config`)
- adds `--backend wayland` in a Wayland session
- passes every other argument to `bin/lillith`

```fish
./run.fish --fix-kwin          # first exclude her from KDE force blur (the dark box), then start her
./run.fish --backend x11       # force the X11 / XWayland fallback
./run.fish status              # subcommands work too: rules, preview --list, panel …
```

To stop her, click her and press `Q` or `Esc`, or run `pkill lillith`.

### 3. Her brain: a local GGUF model on 127.0.0.1:8080

Lillith animates and reacts without a model. To **chat** with her you need a
GGUF model and `llama-server`, both on your machine.

1. **Get the model yourself.** Ko uses **Dolphin 3.0 Llama 3.2 3B
   Q4_K_M**. Download the `.gguf` file from the model's Hugging Face page in
   your browser. Lillith never downloads anything. Then put it in `~/models/`:

   ```fish
   mkdir -p ~/models
   mv ~/Downloads/*.gguf ~/models/
   ```

2. **Run the first-run setup.** It points her at the model and at
   `llama-server`, and it writes `[llm]`, `[chat]` and `[chat.server]` to
   `~/.config/lillith/config.toml` (loopback only, `--jinja` included):

   ```fish
   ./bin/lillith setup --gguf ~/models/<model>.gguf --llama-server (command -s llama-server)
   ```

   If you'd rather use the GUI, run `./bin/lillith-settings` and open the
   **Brain** tab. That's the setup wizard, and it can also scan your disks
   for GGUF files. After setup, Lillith starts `llama-server` herself
   (hidden, on 127.0.0.1) and stops it when she quits.

3. **Or start llama-server by hand** on 127.0.0.1:8080:

   ```fish
   llama-server -m ~/models/<model>.gguf --host 127.0.0.1 --port 8080 -c 4096 --jinja
   ```

   Then test it from the terminal with `./bin/lillith chat "hi"`.

**Ollama (optional).** The setup code also supports `backend = "ollama"`
(loopback `http://127.0.0.1:11434`). The **Brain** tab can write Lily's
Modelfile, whose `FROM` must be a local GGUF file, and run
`ollama create lily -f <Modelfile>`. llama-server is the default.

### 4. KDE: the dark or black box around her

KDE's force-blur effects (Better Blur DX on Garuda) can draw a dark box
around her. At start-up she excludes her own window class automatically
(`[overlay] auto_fix_kwin = true`). To do it by hand:

```fish
./bin/lillith fix-kwin --check     # report only
./bin/lillith fix-kwin             # exclude the `lillith` window class from the effect
./bin/lillith fix-kwin --revert    # undo it
```

`./run.fish --fix-kwin` (or `env LILLITH_FIX_KWIN=1 ./run.fish`) runs
`lillith fix-kwin` before it starts her.

### 5. Optional: window perching (KWin script)

```fish
kpackagetool6 --type=KWin/Script --install kwin/lillith-bridge
kwriteconfig6 --file kwinrc --group Plugins --key lillith-bridgeEnabled true
qdbus6 org.kde.KWin /KWin reconfigure
```

### 6. Autostart

To start her with your desktop session, set `autostart = true` in
`~/.config/lillith/config.toml`, or use the setting in her right-click menu or
Settings. Either one writes `~/.config/autostart/lillith.desktop`.

### Full install (menu entry, `~/.local/bin`)

From a source checkout, run `./linux/install.sh --assets Lillith-<version>.zip`.
It asks a few questions. Undo it with `./linux/install.sh --uninstall`.

---

## Windows (build from source)

The release zip has no Windows build, so you build it.

**Option A: cross-build from Linux or WSL** (makes the installer and a
portable zip):

```sh
rustup toolchain install 1.85
rustup target add x86_64-pc-windows-gnu
# needs mingw-w64 (x86_64-w64-mingw32-gcc), nsis (makensis) and python3
windows/build.sh
# → target/dist/windows/Lillith-<version>-setup.exe and Lillith-<version>-windows-x64.zip
```

**Option B: build natively on Windows** (untested on real Windows):

```bat
rustup toolchain install 1.85
cargo +1.85 build --release --locked -p lillith --no-default-features --features windows
cargo +1.85 build --release --locked -p lillith-settings-gui -p lillith-admin
```

Keep `lillith.exe`, `lillith-settings.exe` and `lillith-admin.exe` together.
She needs `lillith-admin.exe` for elevated commands. Point her at the art with
`--manifest <path>\manifest.toml`.

**Starting her:** run `Lillith-<version>-setup.exe`. It isn't signed, so
SmartScreen may block it: click **More info ▸ Run anyway**. Or unzip the
portable zip and run `lillith.exe`. The installer can add a desktop shortcut
and **Start Lillith when I sign in** (autostart).

**Brain:** get the model file yourself (Lillith never downloads anything) and
put it in `%USERPROFILE%\models\`. That's the Windows `~/models`, and the
**Brain** tab scans it (along with `%USERPROFILE%\Downloads`,
`%LOCALAPPDATA%\lillith\models` and the LM Studio / Ollama folders). On a
low-power CPU with 8–16 GB of RAM it recommends
`dolphin3.0-llama3.2-3b-q4_k_m.gguf` (Dolphin 3.0 Llama 3.2 3B Q4_K_M), and
it ranks uncensored models (Dolphin, abliterated, Hermes) above stock ones of
the same size. A Mistral 7B GGUF still works as a fallback if it's the only one
you have, but it's slow on that kind of CPU. Unzip a llama.cpp Windows release
into `C:\llama.cpp\` or `%LOCALAPPDATA%\Programs\llama.cpp\`, where the Brain
tab finds it on its own (keep the DLLs next to `llama-server.exe`). Then run:

```bat
lillith.exe setup --gguf "%USERPROFILE%\models\dolphin3.0-llama3.2-3b-q4_k_m.gguf" --llama-server "C:\llama.cpp\llama-server.exe"
```

Or use **Lillith Settings ▸ Brain**. Either way, this writes `[llm]`, `[chat]`
and `[chat.server]` to `%APPDATA%\lillith\config.toml`, and she then starts
llama-server herself, hidden and on loopback only:

```bat
llama-server.exe -m <model>.gguf --host 127.0.0.1 --port 8080 -c 4096 -t <physical cores> --jinja
```

Chat needs `--jinja` (the model's own chat template). Windows doesn't expose the
CPU name or core count to her, so there "low-power" means 4 threads or fewer,
and `-t` is half the thread count unless you set `[llm] threads`. Lily's sampler settings
(temperature 0.95, top_p 0.95, min_p 0.1, repeat_penalty 1.1 over the last 256
tokens, 160 reply tokens) are sent with every chat request, so they aren't on
the command line. If you use Ollama instead (`backend = "ollama"`), it must be
on `http://127.0.0.1:11434`. The **kill switch** on Windows is the same safeword
(see below), and it also shuts down the elevated `lillith-admin` helper.
More detail: [`windows/INSTALL-windows.md`](windows/INSTALL-windows.md)
(`source/windows/INSTALL-windows.md` in the zip).

---

## Android (Termux chat, experimental APK)

**What runs on Android today.** The Linux desktop overlay in this zip does not
run on Android. The repo has a separate Android app in `android/`. It's a
floating overlay that uses the same brain and clips over JNI, with on-device
llama.cpp chat and no `INTERNET` permission. So far it ships only as an
experimental **debug APK**:
[Lillith-f8cd236-android-arm64-debug.apk](https://github.com/kokannadian-killa/Lillith-Desktop-Avatar/releases/download/test-build-f8cd236/Lillith-f8cd236-android-arm64-debug.apk)
(from the `test-build-f8cd236` release, arm64 only). **It's untested and older
than this release**, so expect bugs and missing features. The full desktop
overlay experience isn't on Android yet. The APK isn't in this zip. For
install steps, see `source/android/INSTALL-android.md`.

**Chat with her model in Termux** (fully local, no cloud):

1. Install **Termux from F-Droid** (the Play Store build is outdated).
2. Install llama.cpp. The Termux package is `llama-cpp` (64-bit ARM / x86-64
   only):

   ```sh
   pkg update
   pkg install llama-cpp
   ```

   Or build llama.cpp yourself in Termux (`pkg install git cmake clang`, then
   `cmake -B build && cmake --build build --config Release -t llama-server`).
3. **Copy the GGUF to the phone yourself** over USB or with a file manager.
   Nothing is downloaded. Then make it reachable from Termux:

   ```sh
   termux-setup-storage                 # once, grants access to shared storage
   mkdir -p ~/models
   cp ~/storage/downloads/<model>.gguf ~/models/   # e.g. dolphin3.0-llama3.2-3b-q4_k_m.gguf
   ```

4. **Start llama-server on 127.0.0.1:8080** with Lily's sampler settings
   from `source/crates/lillith-setup/src/modelfile.rs`: temperature 0.95, top_p 0.95, min_p 0.1,
   repeat_penalty 1.1 over the last 256 tokens, 160 reply tokens and a 4096 context:

   ```sh
   llama-server -m ~/models/<model>.gguf --host 127.0.0.1 --port 8080 --jinja \
       -c 4096 --temp 0.95 --top-p 0.95 --min-p 0.1 --repeat-penalty 1.1 --repeat-last-n 256 -n 160
   ```

   **System prompt:** Lily's persona is `LILY_SYSTEM_PROMPT` in
   `source/crates/lillith-setup/src/modelfile.rs` (also saved as `setup/persona.txt` in the zip, which includes the
   source). llama-server has no command-line flag for a system prompt. Paste
   the text into the system message field in the web UI's settings.
5. **Chat:** open `http://127.0.0.1:8080` in the phone's browser (llama-server's
   built-in web UI), or chat in the terminal with
   `llama-cli -m ~/models/<model>.gguf --jinja -c 4096 --temp 0.95 --top-p 0.95 --min-p 0.1 --repeat-penalty 1.1 --repeat-last-n 256 -n 160`.
   `llama-cli --help` lists its system-prompt option.

Keep `--host 127.0.0.1` so nothing else on the network can reach the server.

---

## The safeword (kill switch)

To stop everything she's doing, type the safeword in her chat: **`red`** on
its own, or a message that starts with **`OOC: stop`**. It works right away,
even in the middle of a reply. It kills every running command's process tree,
and on Windows it also shuts down the elevated admin helper.

Every file change, command and program launch is a **Soul Contract** plan
that you approve with Yes or No. Nothing runs before you approve it, and every
action goes into the audit log (`~/.local/share/lillith/audit.log`). To undo
her last file-organize batch, run `lillith undo-organize`.

---

## Troubleshooting

- **"no manifest.toml found" / she doesn't appear.** Start her with
  `./run.fish`, or pass `--manifest <path>/manifest.toml`.
- **Dark or black box around her (KDE).** Run `./bin/lillith fix-kwin`, or
  `./run.fish --fix-kwin`. On X11, a black box means compositing is off.
- **Log says `ffmpeg unavailable: pose videos off, using PNG pages`.** Run
  `sudo pacman -S --needed ffmpeg`, or set `LILLITH_FFMPEG` to your ffmpeg
  binary.
- **Wayland trouble.** Try `./run.fish --backend x11`.
- **Chat says she's offline.** Check that llama-server answers on
  `http://127.0.0.1:8080` and that `[llm] gguf_path` points at a file in
  `~/models/`. Re-run `lillith setup`. A non-loopback URL is refused on
  purpose.
- **No voice.** Install `espeak-ng` or piper, and tick **Speak out loud
  (speakers)** in the right-click menu. `--no-voice` starts her with the
  speakers off.
- **Debug log:** `env RUST_LOG=info,lillith=debug ./run.fish 2>&1 | tee ~/lillith.log`
