# Installing Lillith on Windows

Windows 10 or 11, 64-bit. No admin rights needed.

## 1. Install

1. Run **`Lillith-<version>-setup.exe`**. The installer isn't code-signed
   yet, so SmartScreen may say "Windows protected your PC". Click
   **More info ▸ Run anyway**.
2. Pick the options you want:
   - **Desktop shortcut** (on by default; untick it if you don't want one).
   - **Start Lillith when I sign in** (off by default). This puts a
     shortcut in your Startup folder.
3. **App icon**: pick one of the 15 icons for the shortcuts (or keep the
   current one; new installs get *Phantomcore Red*). Change it later in
   Lillith Settings ▸ **Icon** (or right-click her ▸ **Icon…**). The icon
   inside `lillith.exe` itself (taskbar, Alt-Tab) stays Phantomcore Red.
4. The default install folder is `%LOCALAPPDATA%\Programs\Lillith`.

Start menu ▸ **Lillith** gets three shortcuts: *Lillith*, *Lillith Settings*
and *Uninstall Lillith*.

Prefer no installer? Unzip `Lillith-<version>-windows-x64.zip` anywhere
and run `lillith.exe`. Keep the `assets` folder next to it (and
`lillith-admin.exe` — she needs it for elevated commands).

### Elevated (admin) commands

Settings ▸ Access ▸ **WINDOWS · ADMIN**. When she proposes an elevated
cmd / PowerShell command, the Soul Contract card shows **ELEVATED
(admin)** with the exact command and working folder. Your **Yes** is the
only approval (voice never confirms). The first Yes of a session asks
Windows for one UAC prompt; after that the helper stays until you quit
or it idles out (the safeword doesn't close it). Set UAC to
**per-command** if you want a prompt every time.

## 2. Her brain (optional, fully local)

Lillith animates and reacts without any model. To **chat** with her, she
uses a GGUF model run by llama.cpp's `llama-server` **on your PC**. She
only ever talks to `127.0.0.1`. Nothing is downloaded by the installer or
the app, and nothing is sent anywhere.

You provide two things:

- **llama-server.exe**: from a llama.cpp Windows release (for example the
  `llama-…-bin-win-cpu-x64.zip` or a CUDA/Vulkan build). Unzip the
  whole folder, because the DLLs must stay next to `llama-server.exe`.
  Good places are `%LOCALAPPDATA%\Programs\llama.cpp\` or `C:\llama.cpp\`,
  since Lillith looks there automatically.
- **A `.gguf` model file** that you already have.

On the installer's **"Lily's brain"** page, browse to both and click
Next. You can also skip the page and do it later in one of two ways:

- Start menu ▸ **Lillith Settings** ▸ *Brain*, which can also scan for
  GGUF files on your disks, or
- from a terminal:

  ```bat
  "%LOCALAPPDATA%\Programs\Lillith\lillith.exe" setup --gguf "D:\models\lily.gguf" --llama-server "C:\llama.cpp\llama-server.exe"
  ```

When chat is set up, Lillith starts llama-server herself (hidden, on
127.0.0.1) when she starts, and she stops it when she quits.

## 3. Using her

- She sits on your taskbar and on top of open windows, and she follows
  windows that move.
- **Drag** her with the left mouse button. When you let go she falls back
  down.
- Clicks on the empty space around her go through to the window
  underneath.
- **Right-click** her for the menu: modes, chat, settings, icon, quit.
- **Voice:** with `[voice] engine = "auto"` (the default) she speaks
  with Windows' built-in offline voices (SAPI). Use `engine = "none"` or
  `enabled = false` to keep her quiet.
- **Her own voice (Ko's clone):** when the setup was built with the voice payload
  (`VOICE_PAYLOAD=… windows/build.sh`), the section *Lillith's voice* installs it to
  `%LOCALAPPDATA%\Lillith\voice\` (embedded Python + wheels + models + ffmpeg, no downloads, no admin)
  and starts it hidden at sign-in. It talks to the app over a local named pipe
  `\\.\pipe\lillith-voice` (current user only, remote clients rejected), never TCP or `0.0.0.0`.
  Mic: Start menu ▸ Lillith ▸ *Lillith voice - microphone setup*. Log:
  `%LOCALAPPDATA%\Lillith\voice\logs\voiced.log`. See the kit's `windows/INSTALL-windows.md`.
  *Tested under wine; not yet on real Windows.*

## Files

| What | Where |
|---|---|
| Program | `%LOCALAPPDATA%\Programs\Lillith\` |
| Settings | `%APPDATA%\lillith\config.toml` (right-click ▸ settings, or Notepad) |
| Chat history | `%LOCALAPPDATA%\lillith\chat_history.json` |
| Voice (optional) | `%LOCALAPPDATA%\Lillith\voice\` (program, models, `logs\voiced.log`), `%APPDATA%\lillith-voice\` (mic settings) |
| Log | `%LOCALAPPDATA%\lillith\lillith.log`. When started from a terminal she logs to the terminal instead; set `RUST_LOG=lillith=debug` for more detail |

## Uninstall

Use Start menu ▸ *Uninstall Lillith*, or Settings ▸ Apps. The uninstaller
removes the program, its shortcuts and its autostart entry. It then asks
whether to delete your settings, chat history and log as well (the answer
defaults to **No**). Your GGUF models and llama.cpp are never touched.

Silent: `"%LOCALAPPDATA%\Programs\Lillith\Uninstall Lillith.exe" /S`
(this keeps your settings).

## Troubleshooting

- **She doesn't appear:** look in `%LOCALAPPDATA%\lillith\lillith.log`.
  "manifest" errors mean the `assets` folder is missing next to
  `lillith.exe`.
- **Chat says the server is missing:** check that the paths in
  `[chat.server]` in config.toml point at a real `llama-server.exe` and a
  `.gguf` file. Also check that the llama.cpp DLLs are next to the exe.
- **Port 8080 is already taken:** change `endpoint` under `[chat]` to
  another `http://127.0.0.1:PORT`. Only loopback addresses are accepted.
