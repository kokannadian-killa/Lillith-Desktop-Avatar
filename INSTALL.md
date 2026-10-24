# Installing Lillith

**Downloaded the main zip** (GitHub's **Code ▸ Download ZIP**,
`Lillith-Desktop-Avatar-main.zip`)? Use the one-step installer. See
[From the main zip](#from-the-main-zip-github-download-zip) right below.

How to install and start Lillith from the release zip. The zip ships **Linux
binaries only**. On Windows you build from source with Rust 1.85. On Android you chat
with her model in Termux (plus an experimental APK, see below).

Everything runs on your own machine. Lillith only talks to `127.0.0.1`
(loopback), and nothing is downloaded while she runs. You get the model file
yourself.

- [From the main zip (GitHub "Download ZIP")](#from-the-main-zip-github-download-zip)
- [Linux (Garuda / Arch, KDE Plasma 6 Wayland)](#linux-garuda--arch-kde-plasma-6-wayland)
- [Windows (build from source)](#windows-build-from-source)
- [Android (Termux chat, experimental APK)](#android-termux-chat-experimental-apk)
- [Configuration](#configuration)
- [Privacy: what she keeps on your machine](#privacy-what-she-keeps-on-your-machine)
- [The safeword (kill switch)](#the-safeword-kill-switch)
- [Troubleshooting](#troubleshooting)

---

## From the main zip (GitHub "Download ZIP")

The main zip is the source code. It has no ready-made program, and on GitHub
its art (PNG sheets, MP4 poses) is only small **Git LFS placeholders** unless
the repo setting *Settings ▸ General ▸ Archives ▸ Include Git LFS objects in
archives* is on. The installer handles both cases.

**Linux (fish, bash or zsh):** paste only these lines, not the prompt in front of them:

```fish
cd ~/Downloads
unzip -o Lillith-Desktop-Avatar-main.zip
bash ~/Downloads/Lillith-Desktop-Avatar-main/install.sh
```

Run it with `bash` (it isn't a fish script), from any folder. Steps:

1. **Art.** If the art is placeholders, it asks, then downloads about
   530 MB from your GitHub repo and checks every file's sha256. The repo is
   private, so it needs your GitHub login once. It offers to install
   `github-cli` and run `gh auth login --web` (a browser window; log in and
   type the code it shows). No token is saved by the installer.
2. **Packages** with `sudo pacman -S --needed` (Rust, build tools, ffmpeg,
   Wayland/X11 libraries). It asks first.
3. **Build** (first time about 10–20 minutes on an i5 laptop).
4. **Install** for your user: `~/.local/bin/lillith`, her animations in
   `~/.local/share/lillith/generated/`, the app-menu entry, a desktop icon,
   the KWin bridge and the KDE blur fix.
5. **Brain.** It uses `~/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf` if it's
   there (else it searches your home folder), finds `llama-server`, and writes
   `~/.config/lillith/config.toml` (127.0.0.1:8080, `-c 4096`, `--jinja`).
   Lillith then starts llama-server herself. If you already have it running
   on 127.0.0.1:8080 she uses that one.

Other model: `bash install.sh --gguf /path/to/model.gguf`. All options:
`bash install.sh --help`. Remove her: `bash install.sh --uninstall`.

**Windows:** double-click `install.bat` in the unzipped folder. It downloads
the art the same way (GitHub CLI login), offers to install Rust and the C++
Build Tools with `winget`, builds her, installs to
`%LOCALAPPDATA%\Programs\Lillith` with Start-menu and desktop shortcuts, and
runs `lillith setup` with the `.gguf` it finds in `%USERPROFILE%\AI\models`,
`%USERPROFILE%\models` or `Downloads`. It hasn't been run on a real Windows PC yet.

**Android:** the main zip has no APK. See [Android](#android-termux-chat-experimental-apk).

**Not in any zip:** the `.gguf` model and llama.cpp (you install those
yourself), and the voice kit's models (`lillith-voice-kit.tar.gz`, see below).

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
unzip Lillith-Desktop-Avatar-<sha>.zip      # <sha> = the build's commit, e.g. 46ed853
cd Lillith-Desktop-Avatar-<sha>
chmod +x bin/lillith bin/lillith-settings bin/lillith-setup-wizard run.sh run.fish kwin-blur-exclude.sh   # if unzip dropped the executable bits
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
   Q4_K_M** (`dolphin3.0-llama3.2-3b-q4_k_m.gguf`). Download the `.gguf` file
   from the model's Hugging Face page in your browser. Lillith never downloads
   anything. Then put it in `~/models/`:

   ```fish
   mkdir -p ~/models
   mv ~/Downloads/*.gguf ~/models/
   ```

2. **Run the first-run setup.** It points her at the model and at
   `llama-server`, and it writes `[llm]`, `[chat]` and `[chat.server]` to
   `~/.config/lillith/config.toml` (loopback only, `--jinja` included):

   ```fish
   if command -q llama-server
       ./bin/lillith setup --gguf ~/models/<model>.gguf --llama-server (command -s llama-server)
   else
       echo "llama-server not found: install llama.cpp first (step 1)"
   end
   ```

   If `llama-server` isn't on your `PATH`, give its full path instead,
   e.g. `--llama-server ~/llama.cpp/build/bin/llama-server`.

   If you'd rather use the GUI, run `./bin/lillith-settings` and open the
   **Brain** tab. That's the setup wizard, and it can also scan your disks
   for GGUF files (`~/models`, `~/AI/models`, `~/Downloads`,
   `~/.local/share/lillith/models`, `~/.cache/llama.cpp` and the LM Studio /
   Ollama folders). On a low-power CPU with 8–16 GB of RAM it recommends
   `dolphin3.0-llama3.2-3b-q4_k_m.gguf` and ranks uncensored models above
   stock ones of the same size. A Mistral 7B GGUF still works as a fallback if
   it's the only one you have, but it's slow there. After setup, Lillith
   starts `llama-server` herself (hidden, on 127.0.0.1) and stops it when she
   quits.

3. **Or start llama-server by hand** on 127.0.0.1:8080, with the same flags
   the setup writes (`-t` = your **physical** cores, 2 on an i5-7200U; `lscpu`
   shows "Core(s) per socket", while `nproc` counts threads):

   ```fish
   llama-server -m ~/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf --host 127.0.0.1 --port 8080 -c 4096 -t 2 --jinja
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

### 6. Optional: her cloned voice and the mic (offline voice kit)

Without the kit she speaks with piper, espeak-ng or spd-say if one is
installed, or stays silent. The **voice kit** gives her Ko's cloned voice,
lip-sync from the audio and a push-to-talk mic in the chat bubble. It runs a
local daemon (`lillith-voiced`) that she finds on her own (`[voice] engine =
"auto"` tries the daemon first). It talks only over a Unix socket, and its
network guard blocks everything that isn't loopback.

The kit's source is in the repo at `tools/lillith-voice/`. The **models and
Python wheels are not in the repo or the app zip**. They come in the separate
full kit tarball `lillith-voice-kit.tar.gz` (about 731 MiB). That tarball
isn't published as a release asset yet, so get it from Ko. With it, in a git
checkout of the repo:

```fish
tar xzf lillith-voice-kit.tar.gz lillith-voice-kit/models lillith-voice-kit/wheels
mv lillith-voice-kit/models lillith-voice-kit/wheels tools/lillith-voice/
cd tools/lillith-voice
bash install.sh --systemd      # offline: ./models + ./wheels only, no sudo; --systemd also starts the daemon
```

What the installer does:
- It never downloads anything by default. Without the wheels it stops. You
  can opt in to `--online-pip` (PyPI, at install time only) and `--pacman`
  (installs `python ffmpeg rubberband espeak-ng pipewire …` with sudo).
- It puts `lillith-say`, `lillith-listen`, `lillith-chat` and
  `lillith-voiced` in `~/.local/bin`.
- It adds fish shortcuts (`say`, `hush`, `lil`, `lil-talk`, `hear`, …). Open a
  new fish shell to get them.

Check that it works:

```fish
lillith-say --status
lillith-say "Hiii, it's me."
hear --setup                    # pick and test the microphone
```

- If the cloned-voice files (`models/pocket/`) are missing, she speaks with
  the `p1` voice and the log says
  `using fallback voice p1 (cloned 'lillith' voice unavailable)`.
- Without `models/stt/`, the mic is off.
- Saying **"red"** into the mic is the safeword too. A spoken "yes" never
  approves a Soul Contract.
- Model list, sizes, sha256 and tests: `tools/lillith-voice/INSTALL.md`.

### 7. Autostart

To start her with your desktop session, set `autostart = true` in
`~/.config/lillith/config.toml`, or use the setting in her right-click menu or
Settings. Either one writes `~/.config/autostart/lillith.desktop`.

### Full install (menu entry, `~/.local/bin`)

From the unzipped release folder, run
`./source/linux/install.sh --assets assets/generated` (it builds from
`source/`, so it needs Rust). It asks a few questions. Undo it with
`./source/linux/install.sh --uninstall`. In a git checkout of the repo the
script is `./linux/install.sh` (the zip keeps the repo's files under `source/`).

---

## Windows (build from source)

The release zip has no Windows build, so you build it.

**Option A: cross-build from Linux or WSL** (makes the installer and a
portable zip). Run it in a git checkout of the repo with the art pulled
(`git lfs pull`): the zip's `source/` folder has the script at
`source/windows/build.sh`, but not the `assets/` it packs.

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
and `-t` is half the thread count (all of them below 4 threads) unless you set
`[llm] threads`. Lily's sampler settings
(temperature 0.95, top_p 0.95, min_p 0.1, repeat_penalty 1.1 over the last 256
tokens, 160 reply tokens) are sent with every chat request, so they aren't on
the command line. If you use Ollama instead (`backend = "ollama"`), it must be
on `http://127.0.0.1:11434`. The **kill switch** on Windows is the same safeword
(see below), and it also shuts down the elevated `lillith-admin` helper.
**Voice on Windows:** she uses the built-in Windows (SAPI) voice. The offline
voice kit's Windows notes are in `tools/lillith-voice/windows/INSTALL-windows.md`.
Only its named-pipe transport has been tested, and only under wine. It has
not run on real Windows, and no Windows installer bundles it yet.
More detail: `source/windows/INSTALL-windows.md` in the release zip
([`windows/INSTALL-windows.md`](windows/INSTALL-windows.md) in the repo).

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
   repeat_penalty 1.1 over the last 256 tokens, 160 reply tokens and a 4096 context.
   Set `-t` to the phone's big (performance) cores; 4 is a safe start:

   ```sh
   llama-server -m ~/models/<model>.gguf --host 127.0.0.1 --port 8080 --jinja \
       -c 4096 -t 4 --temp 0.95 --top-p 0.95 --min-p 0.1 --repeat-penalty 1.1 --repeat-last-n 256 -n 160
   ```

   **System prompt:** Lily's persona is `LILY_SYSTEM_PROMPT` in
   `source/crates/lillith-setup/src/modelfile.rs` (also saved as `setup/persona.txt` in the zip, which includes the
   source). llama-server has no command-line flag for a system prompt. Paste
   the text into the system message field in the web UI's settings.
5. **Chat:** open `http://127.0.0.1:8080` in the phone's browser (llama-server's
   built-in web UI), or chat in the terminal with
   `llama-cli -m ~/models/<model>.gguf --jinja -c 4096 -t 4 --temp 0.95 --top-p 0.95 --min-p 0.1 --repeat-penalty 1.1 --repeat-last-n 256 -n 160`.
   `llama-cli --help` lists its system-prompt option.

Keep `--host 127.0.0.1` so nothing else on the network can reach the server.

**Voice on Android:** the cloned voice in the APK is only a plan so far
(`tools/lillith-voice/android/INSTALL-android.md`, untested), and no APK has
it.

---

## Configuration

Her settings live in `~/.config/lillith/config.toml` (Windows:
`%APPDATA%\lillith\config.toml`). Every key is optional. **`assets/config.example.toml`**
lists every table and key with its real default, and a test keeps it in sync
with the code. Copy it, then uncomment what you want to change:

```fish
mkdir -p ~/.config/lillith
test -f ~/.config/lillith/config.toml; or cp assets/config.example.toml ~/.config/lillith/config.toml
```

On Windows (cmd), from the source folder:

```bat
if not exist "%APPDATA%\lillith" mkdir "%APPDATA%\lillith"
if not exist "%APPDATA%\lillith\config.toml" copy assets\config.example.toml "%APPDATA%\lillith\config.toml"
```

The keys people change most are `mode`, `scale`, `[overlay] auto_fix_kwin`
(the KDE blur fix at start-up), `[voice] enabled` (the **Speak out loud**
toggle), `[behavior.tease]` (the "come closer" tease) and `[mind.access]`
(what she may do on your machine, always behind the Soul Contract).
**Memory** is on by default (`[memory] enabled = true`). Set
`enabled = false` to turn it off. The other `[memory]` keys (`db_path`,
`half_life_days`, `top_k`, `max_context_tokens`, `prune_below`,
`max_nodes`, `max_events`, `max_errors`) are in the example file; see
*Privacy* below.
For the voice kit, `[voice] daemon_socket`, `preset` and `[voice.stt]` (the
push-to-talk mic) are optional; the defaults find the local daemon.
`lillith setup` (and the setup wizard) writes `[llm]`, `[chat]` and, for
llama-server, `[chat.server]` (model path, ctx size, threads, `extra_args = ["--jinja"]`).

---

## Privacy: what she keeps on your machine

Nothing leaves your machine: no cloud, no telemetry, and the model server is
on 127.0.0.1 only. She does keep these files locally:

| what | Linux | Windows |
|---|---|---|
| settings | `~/.config/lillith/config.toml` | `%APPDATA%\lillith\config.toml` |
| learning memory (SQLite) | `~/.local/share/lillith/memory.db` | `%LOCALAPPDATA%\lillith\memory.db` |
| saved chat history | `~/.local/share/lillith/chat_history.json` | `%LOCALAPPDATA%\lillith\chat_history.json` |
| audit log (Soul Contract + forgets) | `~/.local/share/lillith/audit.log` | `%LOCALAPPDATA%\lillith\audit.log` |
| app log | stderr only (not saved) | `%LOCALAPPDATA%\lillith\lillith.log` when not started from a terminal |

On Linux, `$XDG_DATA_HOME` (if set) replaces `~/.local/share`.

**Memory.** She remembers what you tell her (people, likes, habits), which
apps you use, and errors and their fixes. A short note from that goes into
each chat turn, and unused memories fade over time.
- On Linux the memory folder is owner-only (0700) and the file is 0600
  (Windows uses the default per-user permissions). The safeword
  is never stored.
- **Turn it off** with `[memory] enabled = false`. Then nothing is read or
  written, and no file is created.
- **Forget from the chat:**
  - "forget about <topic>" and "forget everything" ask first; answer "yes".
  - "forget the last 30 minutes" or "the last hour" deletes right away.
    Longer windows (days, a week, months) ask for "yes" first.
  - "forget the last thing I said" drops only your last message and her
    reply from the chat history (and the last 5 minutes from memory).
  - Forgotten rows are deleted, the file is vacuumed, and the matching chat
    history lines go too.
- To wipe it completely, quit her and delete `memory.db` (and
  `chat_history.json` if you want).
- The Android app and the Termux chat don't use this memory.

**Logs** never go to stdout.
- Linux: logs go to stderr and aren't saved unless you redirect them, e.g.
  `env RUST_LOG=info,lillith=debug ./run.fish 2> ~/lillith.log`.
- Windows: started from a terminal, logs show in that console. Started any
  other way (shortcut, autostart), they go to
  `%LOCALAPPDATA%\lillith\lillith.log`, which starts over once it passes
  4 MB.

**Voice kit** (optional): it only uses a local Unix socket or named pipe. Its
network guard blocks any connection and refuses any listening socket that
isn't loopback.

## The safeword (kill switch)

To stop everything she's doing, type the safeword in her chat: **`red`** on
its own, or a message that starts with **`OOC: stop`**. With the voice kit's
mic you can also say "red". It works right away, even in the middle of a
reply. Her voice stops, and the rest of the reply it cut off is dropped: it
isn't shown or spoken. It kills every running command's process tree, and on
Windows it also shuts down the elevated admin helper.

Every file change, command and program launch is a **Soul Contract** plan
that you approve with Yes or No. Nothing runs before you approve it, and every
action goes into the audit log (`~/.local/share/lillith/audit.log`, Windows
`%LOCALAPPDATA%\lillith\audit.log`). To undo
her last file-organize batch, run `lillith undo-organize`.

## Art kit (optional)

The zip's `art-kit/` folder holds the tools that made her clips (`gen-kit`,
`video-pipeline`, `slice-sheets`, `import-manifest`) and the source I2V takes.
Lillith doesn't need it to run. See `art-kit/README.md`. No model weights are
included; get any you need yourself.

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
- **No voice.** Tick **Speak out loud (speakers)** in the right-click menu
  (`--no-voice` starts her with the speakers off). Then either start the
  voice kit's daemon (`systemctl --user start lillith-voice`, check with
  `lillith-say --status`) or install `espeak-ng` or piper.
- **Debug log:** logs go to stderr, so capture them with
  `env RUST_LOG=info,lillith=debug ./run.fish 2>&1 | tee ~/lillith.log`.
- **She forgot something / remembers too much.** See *Privacy* above:
  `[memory] enabled = false`, a "forget …" message, or delete `memory.db`.
