# Lillith: Rust desktop companion & system monitor: build plan

> **Changelog**
> - 2026-10-19, 04:4x MDT: folded in Grok history pass 2. Pinned the CPU monitor numbers (§3.2), confirmed the OpenVINO/PyQt6 backend variant (§2, Q4), added the Mote movement verbs to §3.1, and extended the out-of-scope list and open questions. The ~85 Library downloads matched `source/` by hash (no new files); see `source/DOWNLOADS_MAPPING.csv`.
> - 2026-10-19, 04:3x MDT: added §R (repo = README only).


Based on `inventory.md`, `FEATURES.md`, `grok_history.md` (pass 1) and the 223 user attachments. Items marked **(guess)** or **(verify)** are not confirmed by the sources.
Scope: the desktop companion (avatar, animation, interactions), the system state/action monitor, and local LLM chat with voice and memory. Excluded: Tor/anon bots, MAC/kill-switch, security-audit tooling, Demon OS distro work, and NSFW/unmoderated modes.


## R. Repository status (added 2026-10-19, UTC-6)

**Repo:** `kokannadian-killa/Lillith-Desktop-Avatar`, private, default branch `main`.
- Cloned read-only to `/workspace/lillith/repo`. Nothing was pushed.
- **Content:** one commit, `afb24cc` "Initial commit", 2026-10-19 04:31 MDT, by kokannadian-killa. One file, `README.md`, with 2 lines: title + "Interactive desktop avatar and system state and action moniter". No language detected, no Cargo.toml, no `.rs`, no assets, no CI, no licence, no `.gitignore`.
- **This confirms the earlier "no Rust code exists" finding** and does not conflict with it. The repo is an empty home for the port. Every step in §3.3 and §5 starts from zero, and M0 should **bootstrap the workspace (§1.2) inside this repo**.

**Changes to earlier assumptions:**
- The GitHub account is **kokannadian-killa**, not `killakkainerz`. That was the account seen through the connector, which still cannot see this repo (`scm_repo_not_accessible`; the repo needs granting to the Cursor GitHub App).
- The box's `gh` CLI is logged in as kokannadian-killa (scopes: repo, read:org, gist), which is how the clone worked.
- Other repos on that account that may hold reusable Rust (not reviewed, read-only listing only): `BoiTekEngine` (Rust, public), `TA_TheAttendant` (Rust, public, "HomeWideLocalAiFamilyDataCenterNetwork", possibly related to the local-LLM server in GH#6), and `LifeCraft` (Python, private, pushed 04:27 MDT today).

**Suggested first commit for M0:** a Cargo workspace per §1.2, a `.gitignore` (target/, generated atlases), `assets/source/` with the curated originals or a Git LFS pointer (the sheets and frames total tens of MB, so LFS is a **(guess)** worth confirming), `assets/rules.toml`, an expanded README, and CI running `cargo fmt --check`, `clippy` and `test`.

## 0. What we're building on (facts from the files)

- **Target machine** (from `lillith_os_v2`): HP Spectre x360, i5-7200U, Intel HD 620, **Garuda Linux, KDE Plasma 6, Wayland**, fish shell. Local model file: `/home/ko-terminal/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf` (Modelfile).
- **The best behaviour spec** is `source/LILLITH_README_spring_physics_edition.md`: springs, moods, mischief, sleep/wake, key map, sounds, memory, Ollama chat, TTS, STT, proactive talk, and system tools. Its Python implementation was not attached, so the Rust version is built from the spec.
- **Working Python modules to port 1:1:** `lillith_memory_sqlite.py`, `lillith_tts_engine.py`, `lillith_voice_input.py`, `aether_gesture_map.py`, `aether_animator.py`, `mod_reactions.py`, and the v01 interaction model.
- **Assets:** 6 alpha sheets (3×10, 1792×1008), 52 single-pose stills (grey background), a left-run 4×3 sheet and reference sheets (baked checkerboard), 4 idle-physics keyframe refs, and 80 black-background video frames.
- **Prior Rust intent** (GH#6): Axum + sqlx/SQLite + reqwest to `llama-server` :8080 (chatml), Ollama as the alternative, "egui + tray" for the window. GH#2: a llama.cpp CLI. No Rust code exists anywhere; the user's repo `Lillith-Desktop-Avatar` holds only a 2-line README (§R). Toolchain on the box: cargo/rustc 1.85.1.

## 1. Architecture

### 1.1 Option comparison: pure-Rust overlay vs Tauri

The TSX files (`LillithStage.tsx`, TanStack routes, "Lillith 2029") are a browser demo from a Grok preview app. They do not use Tauri `invoke`, but they do suggest a web-UI direction, so Tauri was weighed.

| | A. Pure Rust overlay | B. Tauri v2 (Rust core + web UI reusing LillithStage.tsx) |
|---|---|---|
| Always-on-top on KDE Wayland | **Yes**, via wlr-layer-shell *Overlay* layer (KWin implements it) | No layer-shell in Tauri. `alwaysOnTop` is not honoured for Wayland toplevels, so it needs a KWin "Keep above" rule or XWayland (`GDK_BACKEND=x11`) |
| Click-through except on her body | Yes: `wl_surface.set_input_region` = union of rects from the frame's alpha mask | Whole-window only (`set_ignore_cursor_events`). No per-pixel hit-test |
| Transparency | Native ARGB buffer | WebKitGTK transparency on Wayland is fragile |
| CPU/RAM on i5-7200U | ~300×400 sprite at 12–30 fps. Tens of MB **(guess)** | WebKitGTK process at ~100–200 MB **(guess)** plus a JS animation loop |
| Reuse | Ports the Python logic directly | Reuses the TSX look (reel player, sliders, event lines) |
| Risk | More low-level Wayland code (sctk) | Fights the compositor on the core "lives on the desktop" behaviour |

**Recommendation: hybrid with A at the core.**
- The avatar overlay is pure Rust: `smithay-client-toolkit` layer-shell plus a CPU blitter (`tiny-skia` into a `wl_shm` buffer). Move to `wgpu` only if glow/particle effects need it.
- Chat, settings and debug panels run in a separate, normal window with `egui`/`eframe`, which needs nothing special from Wayland.
- `LillithStage.tsx` is kept as a design reference. A Tauri control panel can come later if you want a web UI. The overlay should not be Tauri.
- **X11/XWayland fallback:** `winit` 0.30 with `transparent(true)`, `WindowLevel::AlwaysOnTop` and `set_cursor_hittest()` toggled by the hit-test. Selected with `--backend x11` or when `WAYLAND_DISPLAY` is unset.

### 1.2 Workspace layout

```
lillith/                          Cargo workspace
├─ crates/
│  ├─ lillith-core/     pure logic: state machine, moods, mischief, springs, rules, config (no I/O, unit-tested with fake clock)
│  ├─ lillith-anim/     atlas loader, clip player, procedural gestures (port of aether_animator), particles
│  ├─ lillith-overlay/  layer-shell window (sctk) + winit/X11 fallback, input region, drag, speech bubbles
│  ├─ lillith-sense/    monitors → Event bus (sysinfo, zbus: UPower/NM/BlueZ/UDisks/Notifications, inotify, journald, KWin bridge)
│  ├─ lillith-mind/     memory (rusqlite port), LLM client (Ollama + OpenAI-compatible llama-server), context builder, read-only tools
│  ├─ lillith-voice/    TTS chain (piper → espeak-ng → spd-say), STT (whisper.cpp CLI → whisper-rs), rodio SFX
│  ├─ lillith-panel/    egui chat bar, settings, pose browser, event injector
│  └─ lillith-tray/     ksni StatusNotifierItem
├─ apps/lillith/        main binary, tokio runtime, flags (--no-tray --no-voice --no-llm --backend x11)
├─ kwin/lillith-bridge/ KWin script (JS) → D-Bus: window added/removed/activated/minimized, geometry, cursor pos
├─ tools/slice-sheets/  offline asset pipeline
└─ assets/              generated atlases + manifest.toml + rules.toml
```

**Runtime.** The render thread owns the Wayland connection and draws at 30 Hz. Physics is sub-stepped at 120 Hz and sprite frames advance at ~12 fps (GH#1). A `tokio` runtime runs the sensors, LLM and voice. They talk over one `Event` bus into `lillith-core`, which emits `Command`s (play clip, say line, set mood, move to) back to the overlay.

### 1.3 Wayland specifics (KDE Plasma 6)

- **Overlay surface.** `zwlr_layer_shell_v1` on layer `Overlay`, anchored bottom+left. Exclusive zone −1. Keyboard interactivity `OnDemand`, so keys reach her only when she is focused (README key map) **(verify on KWin 6)**.
- **Moving her.** Layer surfaces are positioned by **margins**. Drag is pointer-grab plus margin updates. Walk and patrol animate the margins.
- **Click-through.** Precompute per-frame alpha masks as row runs at build time and set them as the input region on every frame change.
- **What Wayland hides from clients:** the global cursor, other windows' titles and geometry, and global keys. Add a small **KWin script** that pushes `workspace.windowAdded/windowRemoved/windowActivated`, minimize changes, frame geometry and polled `workspace.cursorPos` (~10 Hz while awake) to `org.lillith.Bridge` via `callDBus`. This replaces `wmctrl`/`xdotool` from `aether_event_watcher.py` and enables title-bar perching, window open/close gestures, Follow mode and proximity wake **(verify Plasma 6 KWin API names)**.
- **Global shortcuts** (chat, push-to-talk): xdg-desktop-portal GlobalShortcuts via `ashpd`, with tray items as fallback. This replaces `pynput`, which README-DK notes is limited on Wayland.
- **Screenshot** for "What do you see?": `ashpd` Screenshot portal, or `spectacle -b -n -o` as in `lillith_os_v2`.
- **Idle:** `ext-idle-notify-v1`, or `org.freedesktop.ScreenSaver` D-Bus.
- **No global typing detection on Wayland.** "User types" reactions apply only inside her own chat bar. Reading `/dev/input` would need the input group and is a privacy concern, so it is not planned.

## 2. Recommended crates (matched to the Python prototypes)

| Python behaviour / dependency | Rust crate(s) |
|---|---|
| Tk/pygame frameless topmost alpha window | `smithay-client-toolkit`, `wayland-client`. Fallback `winit` + `softbuffer` |
| PIL rotate/scale/tint/paste (apply_transform, aether_animator) | `tiny-skia`, `image`, `fast_image_resize`. Optional `wgpu` |
| Speech bubble text | `cosmic-text` (or `ab_glyph`) |
| psutil CPU/RAM/disk/procs/temps/uptime/load | `sysinfo` |
| Battery | `zbus` → UPower (event-driven). Alternative `starship-battery` |
| Wi-Fi strength, VPN, hotspot, connectivity lost/restored | `zbus` → NetworkManager |
| Bluetooth paired | `bluer` |
| Volume | `libpulse-binding` subscribe (PipeWire-pulse) or `pactl subscribe` subprocess |
| Brightness | `notify` on `/sys/class/backlight/*/actual_brightness`, or `zbus` → `org.kde.Solid.PowerManagement` |
| USB / drive eject | `zbus` → UDisks2 |
| Email/update/calendar notifications | `zbus` monitor of `org.freedesktop.Notifications` |
| Trash / screenshots / archives / downloads | `notify` (inotify) |
| Errors, firewall blocks | `systemd` crate journal reader **(guess: depends on firewalld vs ufw on this install)** |
| Clipboard | `wl-clipboard-rs` |
| pystray | `ksni` |
| Tk/CTk dialogs, chat bar, settings | `egui` / `eframe` |
| config.ini | `serde` + `toml` → `~/.config/lillith/config.toml` (same key names as README-SP), `directories` |
| sqlite3 memory | `rusqlite` (bundled) + `regex`. Use `sqlx` only if the GH#6 Axum server is kept |
| Ollama / llama-server | `reqwest` (streaming) + `serde_json` + `tokio`, or `ollama-rs` |
| Optional local API (GH#6 `lillith-local`) | `axum` 0.7, `tower-http` |
| Piper/espeak TTS | `std::process::Command` (same chain) + `rodio` playback |
| Whisper STT | v1: `whisper-cli`/whisper.cpp + `pw-record`/`arecord` subprocess. v2: `whisper-rs` + `cpal` |
| SFX | `rodio` |
| Logging / session JSONL | `tracing`, `tracing-subscriber`, `tracing-appender` |
| Spring physics | hand-written + `glam` |
| Global shortcuts / portals | `ashpd` |
| X11 fallback window list | `x11rb` |

## 3. Behaviour model (lillith-core)

### 3.1 Layers

1. **Base / locomotion state** (exclusive): `Idle{Sitting|Standing}`, `Walking{dir}`, `Running{dir}`, `Perched{window}`, `Dragged`, `Sleepy`, `Asleep`, `Hidden`. Movement modes from GH#1 / P2-M ("Mote") are **Wander** (patrol the floor), **Follow** (cursor via KWin bridge), **Stay**, plus the stretch verbs climb-window-side, sit-on-ledge, ride-a-dragged-window, pet-on-tap and pick-up/throw.
2. **Reaction layer.** One-shot pose or clip with `priority`, `duration`, `cooldown`, `interruptible`, optional line and SFX. A priority queue replaces v01's `is_reacting` flag. There is a global auto-gesture cooldown of 2.5 s (from `aether_avatar_window`).
3. **Idle life.** Breathing period 3.5–4.5 s. Blink every 4–6 s. An idle transition every 8–12 s lasting 1.2–2 s (hip sway, lean-forward, shoulder roll, hips shift, soft reset, per physics refs A–D). Random idle reactions every 12–26 s. Pose cycle every 16 s (README-SP defaults).
4. **Mood & drives.** Moods are Playful, Happy, Mischievous, Alert and Sleepy, each with a strength. The mischief meter rises while she is ignored and fires a playful interruption at a threshold. Inactivity moves her Sleepy → Asleep, and proximity or a click wakes her. Warmth and greeting style come from memory.
5. **Physics (render side).** One spring-damper per channel: root x/y, angle, scale, chest, pigtails. README-SP defaults (stiffness/damping): chest 65/9.5, pig 32/6.5, root 48/10, angle 42/9, scale 55/11. Gains: jiggle 0.6, chest_bounce 1.0, pigtail_sway 0.9. Semi-implicit Euler at 120 Hz with velocity clamping. Reactions apply **impulses** in place of the v01–v12 random jitter. With single-image poses, the chest and pigtail channels can only be applied as subtle whole-sprite squash/offset until layered art exists **(design note)**.

### 3.2 Event → reaction rules (`assets/rules.toml`, data-driven)

Poses use clean names from `lillith_pose_map.txt`. Gestures use `aether_animator` names. Lines come from LillithStage.tsx and gesture_map.py. SFX cues come from README_SOUNDS. Thresholds marked (guess) are new.

| Event | Pose / clip | Gesture | Example line | Mood | SFX |
|---|---|---|---|---|---|
| `cpu.high` (psutil parity: >72%, polled 0.5 s, re-eval 5.5 s; 60/40 split `system_high`/`hacking`; add hysteresis so it doesn't chatter) | busy → alert / hacking | bounce | "System's getting sharp… I like the heat." | Alert | alert |
| `ram.high` (>85%, guess) | htop | think | — | Alert | alert |
| `battery.low` (≤15%) | battery_low | — | "Power's low. Don't fade on me." | Alert | battery |
| `net.lost` / `net.restored` | connection_lost / wifi | facepalm / wave | "Network restored" | Alert / Happy | disconnect / soft |
| `wifi.changed` | wifi | nod | — | — | wifi |
| `vpn.up` | vpn | salute | "Tunnel's up." | Mischievous | vpn |
| `hotspot.on` | hotspot | wave | — | — | — |
| `bt.paired` | bluetooth | tada | — | Happy | soft |
| `volume.changed` | volume (frames) | — | — | — | — |
| `brightness.changed` | brightness | — | — | — | — |
| `usb.added` / `usb.removed` | drive_eject | point_right | — | — | — |
| `print.job` | printing | nod | — | — | — |
| `archive.created` | compress | tada | — | — | — |
| `trash.added` | recycle | shrug | — | — | — |
| `notify.received` | dismiss_notif | wave | (summary) | Alert | alert |
| `calendar.reminder` | calendar | point_right | (summary) | Alert | alert |
| `screenshot.taken` | screenshot | tada | — | Playful | soft |
| `clipboard.copied` | clipboard | — | — | — | — |
| `window.opened{folder,browser,terminal,media,app}` | peeking_folder / peeking / hacking / (dance) / (nod) | point_right / wave / salute / dance / nod | "Ooh, what's in there? 👀" / "Surfing the web? 🌐" / "Let's hack! 💻" / "Now we're vibing 🎵" | Playful | hover |
| `window.closed` | knife | stab | "Gotcha! 🗡️" | Mischievous | click |
| `window.minimized` | minimize | — | — | — | — |
| `window.private` (title match) | private | — | — | Mischievous | — |
| `focus.long_work` (>50 min, guess) | code_inspect | — | stretch / water nudge | — | soft |
| `idle.long` → `idle.asleep` / `user.returned` | sitting → sleep / peeking | float | "You've been quiet. I'll stay close." | Sleepy → Happy | sleep / soft |
| `journal.error` / `app.crash` | error | facepalm | "That stack trace looked rude." | Alert | error |
| `firewall.blocked` | firewall | point_right | — | Alert | alert |
| `llm.thinking` / `llm.done` | waiting / — | think / tada | "Hmm…" | — | — |
| `approval.needed` | waiting | — | — | — | — |
| Mouse L / Dbl / R / Mid / hover / hover-long / drag | knife / hacking-or-knife / cleaning-or-menu / help / hover / peeking / drag | bounce | — | Playful | click/hack/clean/help/hover/peek/drag |
| Secret L→Dbl→R ≤3 s | dual_tail + particles | dance | special line | Happy | tail |
| Keys (README-SP map, FEATURES §2) | per key | — | — | — | — |

A debug **event injector** in the egui panel and on the CLI (`lillith event battery.low`) can fire any event. It mirrors GH#5's `event alert|busy|…` and the LillithStage buttons, so every mapping can be tested without changing real hardware state.

### 3.3 Python → Rust port map

Repo baseline: the repo has nothing to port from. Every target module below is new code in `Lillith-Desktop-Avatar`, and the Python sources live only in the attachments (`/workspace/lillith/source/`).

| Python | Rust | Notes |
|---|---|---|
| v01 `CyberDemonAvatar` (bindings, hover phases, drag, timers) | `core::interaction`, `overlay::input` | Fixes these known bugs: blocking `pygame.time.wait` and the never-shown reaction image (v10–v12), `_start_all_systems` / `_setup_bindings` mismatches (v08/v09), `drag_x` uninitialised (v07), missing `import os` (mod_config) |
| `mod_reactions.py`, CreationImagesNeeded | `rules.toml` + `core::rules` | |
| `mod_system_monitor.py` | `sense::cpu` (sysinfo, 5 s, hysteresis + cooldown) | |
| `aether_event_watcher.py` (wmctrl) | `sense::kwin` (D-Bus fed by the KWin script). `x11rb` on X11 | |
| `aether_gesture_map.py` | `rules.toml [window.*]` | copy the app-category lists verbatim |
| `aether_animator.py` | `anim::gestures` | easing and tweens become spring targets/impulses |
| `aether_avatar_window.py` | `overlay::{bubble, walk, menu}` | 2 px/tick patrol becomes a margin animation |
| `aether_image_loader.py`, `aether_body_segmenter.py` | `tools/slice-sheets` | rembg stays in the offline tooling |
| `mod_tray.py` | `lillith-tray` (ksni) | |
| `mod_config.py`, config_A/B, README-SP keys | `core::config` (TOML, same keys) | |
| `lillith_memory_sqlite.py` | `mind::memory` (rusqlite + regex) | same schema. DB at `~/.local/share/lillith/lillith_memory.db` |
| `lillith_tts_engine.py`, `aether_tts.py` | `voice::tts` | same chain. espeak `en+f3 -s145 -p50` |
| `lillith_voice_input.py`, `aether_stt.py` | `voice::stt` | subprocess first. `pw-record` preferred on PipeWire (guess) |
| `lillith_os_v2.py` (PyQt6, OpenVINO/optimum-intel on HD 620, temp 0.7/150 tok, spectacle vision) | `mind::llm` trait: `Ollama`, `OpenAiCompat` (llama-server) | v1 runs the Dolphin-3 3B Q4 on CPU. OpenVINO on the HD 620 is a stretch (P2-§2) |
| GH#6 `lillith-local` Axum `/chat` | optional `axum` API on 127.0.0.1 | only if a web panel or phone client is wanted |
| `aether_movement_learner.py` | deferred | stretch goal |
| `LillithStage.tsx` | egui stage/debug panel design | |

## 4. Asset pipeline (`tools/slice-sheets`)

1. **6 alpha sheets** (1792×1008, 3×10, nominal cell 179.2×336, non-integer):
   - Tails and FX cross cell boundaries, so uniform slicing would clip them.
   - Per row, locate each body's alpha centroid near `(c+0.5)*179.2`.
   - Crop an overlapping window (~240×336) around it and remove the neighbours' connected components.
   - Record a feet anchor (lowest opaque pixel) so playback doesn't jitter.
   - Sheets A/B/C share RGB pixels. Keep the cleanest alpha.
2. **Baked-checkerboard images** (left-run 4×3, alt sheet, reference sheets, full-body ref): flood-fill the two checker greys from the borders, or use rembg/BiRefNet. The alt sheet has a header, uneven cells and grid lines, so detect the lines first, then crop each cell by its content bbox.
3. **52 pose stills** (grey grid bg, 111–302 px): background removal. Keep the UI props (popups/icons) as part of the pose. Upscale ×2 to about 260×360 (e.g. realesrgan-ncnn-vulkan, guess). Name per pose_map.
4. **80 frames** (640×960, black bg): key black → alpha with soft edges. Order them by similarity chaining (perceptual hash/SSIM) and split into loops (idle sway, wave, leg raise, enter/exit).
5. **Output:** atlases + `manifest.toml` (clip → frames, fps, loop/once, anchor, per-frame input-mask runs, tags).
6. **Art still to make** (Grok Imagine using the GH#9 #00ff00 chroma-key prompt, or I2V per `ImageToVideo_tools_and_workflow.txt`):
   - right-facing walk/run, or mirror the left-run sheet;
   - sit-down/stand-up transitions;
   - sleep loop + wake;
   - blink/wink overlays;
   - idle-transition frames from physics refs A–D;
   - knife-tap loop (`knife_ready → knife_swing → knife`);
   - perch-on-title-bar with dangling legs;
   - hang-from-window; held/drag pose;
   - 2–4 frame micro-loops for busy, alert, loading, wifi, battery_low and sleep;
   - crops of the extra labelled poses on reference sheets 1–3 (cable management, file unlock, browser navigation, system log reading, pretends to type, window snap, loading complete…);
   - the 19 SFX in README_SOUNDS (none attached).

## 5. Milestones and acceptance criteria

| # | Milestone | Acceptance criteria |
|---|---|---|
| M0 | Workspace + asset pipeline **in `Lillith-Desktop-Avatar`** (repo currently has only README) | Workspace, .gitignore, README and CI are committed to `main`. `cargo build` passes. `slice-sheets` turns the 6 alpha sheets and 52 poses into atlases + manifest. A contact sheet shows no clipped tails, and anchors keep the feet steady |
| M1 | Overlay on KDE Wayland | Transparent layer-shell overlay stays above normal windows. Clicks outside her silhouette reach the desktop. Drag works and the position persists. `--backend x11` fallback works. Idle CPU <3% on the i5-7200U (target, guess) |
| M2 | Animation engine | Clip player + springs + breathing + blink + idle transitions. Walk cycle with flip. Every manifest clip is previewable in the debug panel |
| M3 | Interaction & state machine | v01 parity (L/Dbl/R/Mid click, hover → peek, drag pose, random idle, pose cycle), README-SP keys, secret sequence, moods, mischief, sleep/wake, Wander/Stay. `lillith-core` unit tests use a fake clock |
| M4 | System monitor v1 | Real detection for CPU, RAM, battery, net lost/restored, Wi-Fi, VPN, volume, brightness, USB, notifications, trash, screenshot and idle/return. Each fires its rule with cooldown and hysteresis. The injector covers every §3.2 row |
| M5 | KWin bridge | Script installs with `kpackagetool6`. Window open/close/minimize reactions work (close → stab). She perches on the focused title bar. Follow mode and proximity wake work |
| M6 | Tray, panels, config | ksni tray: Show/Hide, Mode, Random, Mute, Talk, Status, Voice, Settings, Quit. egui settings edit `config.toml` live (springs, thresholds, size, opacity). Autostart `.desktop` + PKGBUILD |
| M7 | Memory + LLM chat | rusqlite memory passes parity tests against the Python module (same facts from the same sentences). Chat bar (Y/Enter or portal shortcut) streams from Ollama or llama-server into the bubble with the context summary. README-SP commands work. Thinking/done poses play. Offline personality lines appear when no LLM is running. Only read-only system tools in v1, with an approval dialog before anything else |
| M8 | Voice | Replies are spoken (Piper, else espeak-ng). Push-to-talk records 5 s → whisper → chat. Both can be toggled and fail gracefully |
| M9 | Proactive life & polish | Proactive comments with cooldown, warmth/greeting styles, work-session nudges, SFX, particles, optional "What do you see?" (portal + moondream), and the remaining events (print, archive, calendar, firewall) |

M0–M3 give the first demoable build: README-SP behaviour without AI. M4–M5 complete the system state/action monitor. M7–M8 complete the companion.

## 6. Open questions

1. **Canonical art style/outfit.** (a) The 1792×1008 sheets and 52 poses: fishnet sleeves, pink/black bikini. (b) The full-body ref and 80 frames: thigh-highs, magenta shorts, cyan tattoos. (c) The alt action sheet: blue bikini, pink-blonde, a different style. Pick one, and decide whether to regenerate the others to match.
2. **Name and branding.** Lillith or Lilith. Use the Demon OS palette (ink #120810, rose #e23d8a, ice #5fd0e6) or the Garuda violet/cyan for the panels?
3. **Mouse mapping.** Double-click = knife (v01, GH#4) or hacking (README-SP)? Right-click = cleaning or the pose menu (GH#4)?
4. **LLM backend.** Ollama (README-SP/DK, Modelfile) or `llama-server` (GH#6)? Is the default model `dolphin3.0-llama3.2-3b-q4_k_m.gguf` or `llama3.2:3b`? Is OpenVINO/optimum-intel on the HD 620 worth pursuing (confirmed working in `lillith_os_v2.py`, P2-§2)?
5. **Missing files / repo.** Should the Python prototypes and curated assets be committed to the repo as `legacy/` and `assets/source/` for reference? Can the repo be granted to the Cursor GitHub App? Are `TA_TheAttendant` or `BoiTekEngine` meant to be reused (e.g. for the LLM server)?
   Also: Can you supply `lillith_avatar.py` and its 205–221 image `poses/` folder, the Aether package internals, and the Rust code from GH#2/#6? They would shorten M2–M3 and M7.
6. **Window behaviour.** Should she be allowed to walk over fullscreen apps, or hide automatically during fullscreen/games? Should she stay on all virtual desktops?
7. **Event priorities.** Which system events should she react to by default, and which should be quiet indicators? Is there a do-not-disturb mode?
8. **Firewall.** Which one is in use (firewalld, ufw, or plain nftables)? This decides the `firewall.blocked` source.
9. **Desktop actions.** Should any tool change the system (kill a process, organise files)? The plan is read-only plus confirmation, following the README-DK approval model.
10. **Platforms.** Linux/Wayland only for v1, or are the Windows/Android notes in README-DK in scope later?
11. **Grok pass 2.** Fold in `grok_history_pass2.md` when it lands (unopened chats include "Build Rust Application from Grok Link" and "Rust Project Prompt Creator for Multi-OS").
12. **Installer.** Is `Lillith-The-Master-Setup.iss` (Inno Setup, Windows, `tools\launch.bat`) still wanted, given the Linux-first plan? Its contents weren't retrieved.
13. **Branding verdict.** Three brand passes exist: "Lillith 2029" (PASS, og.jpg, #C026D3 / #120014), "Lilith" (card FAILED), "Aether Companion" (PASS). Which is canonical for the Rust app?
14. **Unretrieved project content.** The `imagine_images/` and `browsed_files/` folders and the `.iss` file in the Grok "lillith" project were not opened. Should I pull them on a later pass?
