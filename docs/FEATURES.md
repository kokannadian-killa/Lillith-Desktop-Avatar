# Lillith: deduplicated feature list

> **Changelog**
> - 2026-10-19, 13:5x MDT: **M4 system monitor** landed on `scaffold/rust-workspace` (`lillith-sense`). §4 Repo column now says IMPL / STUB for the rows it covers; detection sources updated to what the Rust code actually reads.
> - 2026-10-19, 04:4x MDT: folded in **Grok history pass 2** (`grok_history_pass2.md`). Corrected the CPU monitor, added new movement/pose-sheet/installer/branding rows, expanded the out-of-scope list (CAPABILITIES 60-tool set). All ~85 Library downloads in `~/Downloads` were already in `source/` (hash-checked, no new files); see `source/DOWNLOADS_MAPPING.csv`.
> - 2026-10-19, 04:3x MDT: added the Repo column (the repo holds only a README).

Built from every attached file (see `inventory.md`), `grok_history.md` (pass 1, GH#n) and `grok_history_pass2.md` (pass 2, P2-A..P2-O).

**Status key**
- **PY**: prototyped in Python and present in the attachments (the "works?" note says whether it runs)
- **PY-missing**: a README says it was built in Python, but the code was not attached
- **SPEC**: only described (README, Grok chat, notes)
- **ART**: art exists (pose still / sheet / reference)
- **MISSING**: neither code nor art
- **WEB**: browser demo only (Grok preview / TSX)

**Source abbreviations:** README-SP = `LILLITH_README_spring_physics_edition.md`; README-DK = `README_DesKToPD_project_lillith.md`; CAP = `CAPABILITIES_aether_lillith.md`; INV = `aether_companion_full_inventory_notes.md`; GH#n = conversation n in `grok_history.md`; v01–v12, mod_*, aether_*, lillith_* = files in `source/`.

**Repo column:** status in `github.com/kokannadian-killa/Lillith-Desktop-Avatar` (private, branch `main`), checked 2026-10-19 at 04:31 MDT.
- The repo holds one commit, `afb24cc` "Initial commit", made 04:31 MDT, about when it was shared. Its only file is a 2-line `README.md`: "Interactive desktop avatar and system state and action moniter".
- Rows read "—" until code lands, then IMPL / STUB (M4 filled in the §4 system-monitor rows; earlier milestones are tracked in PLAN.md §5).
- The repo is the intended home for the Rust port in PLAN.md and has no code yet.

Out of scope and deliberately excluded (not planned, even where code or docs exist): the Tor/anon bot pool (CAPABILITIES.md lists 55 "anon" tools among 60), Tor routing, MAC randomizer/"MAC mask", network kill switch (rfkill/nmcli), trace wiper (RAM tmpdir/shred); port/secret scanning, the CAI security scan/panel, offensive "forge" tools, "30 drones", and pentesting/exploit tooling (P2-L, P2-O and the skipped chats); the ~97 KB `grok-chat.txt` jailbreak/18+ transcript and any "uncensored"/"unrestricted"/moderation-bypass persona prompts (P2-1c). These are noted for awareness only; their code was not reviewed and none feed the plan.

## 1. Avatar & animation

| Feature | Sources | Status | Repo |
|---|---|---|---|
| Transparent, frameless, always-on-top avatar window | v01–v12, aether_avatar_window, README-SP, GH#1/#11/#12 | PY. Tk works on X11. pygame is not truly transparent. Wayland is unsolved | — |
| Base idle pose (sitting / rock-on) | pose_map (sitting.png), CreationImagesNeeded, character_fullbody_reference | ART + PY | — |
| Micro "jiggle": random rotate/shift/scale per frame | v01–v12 | PY (works, looks cheap) | — |
| Multi-body spring-damper physics (chest, pigtails, root, angle, scale) with tunable stiffness/damping, sub-stepping, velocity clamp | README-SP | PY-missing | — |
| Micro-movement image variants with hysteresis (no flicker) | README-SP | PY-missing (205–221 images not attached) | — |
| Breathing cycle (inhale 1.2–1.6 s / hold / exhale / settle) + heartbeat opacity pulse | README-SP, phys_breathing | SPEC (PY-missing) | — |
| Idle transitions every 8–12 s: hip sway, curious lean-forward, shoulder roll, hips shift, soft reset | phys_idle_transitions, prompt_refined_idle_transitions, physics_ref_A–D | ART (keyframe refs) + SPEC | — |
| Pose cycling of idle poses every 14–18 s | v01+, config_A/B | PY (works) | — |
| Random idle reactions every 8–28 s (tail wag, blink, glitch, hacking) | v01+, mod_reactions | PY (works) | — |
| Walk cycle / patrol along the taskbar, sprite flip for return | aether_avatar_window, INV (sprite_sheet walk_left/right), GH#1/#5, sheets | PY (procedural) + ART (walk sheets, left-run 4×3) | — |
| Running | pose 24, left-run sheet, alt sheet row 1 | ART | — |
| Spell/orb/beam/aura cast animations | 3 alpha sheets | ART only (no feature wired) | — |
| Sword slash, jump/flip/spin, lounging | alt-style sheet | ART (different style) | — |
| Multi-frame sequences engine (knife_ready → knife_swing → knife; volume frames) | README-SP | PY-missing | — |
| Procedural gesture library: float, bounce, wave, stab, point_right, think, tada, dance, salute, nod, shrug, facepalm, gif playback | aether_animator | PY (works with any single image) | — |
| Knife-tap loop on window edge with sparks + fading crack | phys_knife_tap, anim_core_knife_tap, pose 28, physics_ref_D | ART + SPEC | — |
| Tail physics (S-curve figure-8, heart-tip glow/flick) | phys_tail, phys_tail_heart | SPEC | — |
| Horn nebula glow, tattoo glow + particles, pigtail bounce, facial micro-expressions (blink 4–6 s, wink, tongue peek) | phys_* | SPEC (I2V prompts) | — |
| Particle system (cyan, magenta, purple, gold, red, soft) | README-SP | PY-missing | — |
| Speech bubbles (mood-aware lines) | aether_avatar_window (SpeechBubble), README-SP, LillithStage | PY | — |
| Drop shadow, default avatar generation | aether_image_loader | PY | — |
| Upload your own avatar photo → background removal / segmentation | aether_onboarding, aether_body_segmenter (rembg) | PY | — |
| "Learned" gestures (feedback scoring, mutation, blending) | aether_movement_learner | PY (optional) | — |
| Video-reel playback (rock/stage/city reels) | LillithStage.tsx, motion-manifest, 80 frames | WEB + ART (frames unordered) | — |
| Mood system: Playful, Happy, Mischievous, Alert, Sleepy (+ warm/excited in web) | README-SP, LillithStage | PY-missing / WEB | — |
| Mischief meter (grows when ignored → playful interruptions) | README-SP | PY-missing | — |
| Sleep after inactivity; wake on mouse proximity (the "most alive" moment) | README-SP, GH#1, pose 34 | PY-missing + ART | — |

## 2. Interactions (mouse/keyboard)

| Feature | Sources | Status | Repo |
|---|---|---|---|
| Left click → knife/rock-on + bounce + particles | v01+, README-SP, pose 28 | PY | — |
| Double click → knife (v01) / hacking (README-SP) | v01, README-SP, GH#4 | PY. The two sources disagree: decide | — |
| Right click → cleaning pose; GH#4: right-click → **pose menu** | v01+, README-SP, GH#4 | PY / WEB | — |
| Middle click → help | v01+ | PY | — |
| Hover enter → sit & swing; sustained → peek with mouse parallax | v01+ | PY | — |
| Drag to move (with drag pose) | v01+, README-SP | PY | M1: left-drag plays `drag_held`, position saved to `[overlay]` in config.toml (X11 verified; Wayland needs on-machine test) |
| Tap to pet (GH#1); tap empty glass to walk there; arrow keys nudge | GH#1 | WEB | — |
| Secret sequence Left → Double → Right within ~3 s | README-SP | PY-missing | — |
| First-interaction & daily greeting (style from warmth) | README-SP, lillith_memory | PY (memory part) | — |
| Key map: T tail, K hack, H help, R random, C cycle, M mute, S sleep, W wifi, V vpn, B battery, A alert, E error, F firewall, P private, 0 running, 1 htop, 2 task manager, 3 settings, 4 tools, 5 screenshot, 6 calendar, 7 clipboard, 8 volume, 9 brightness, D diagnostics, N bluetooth, U cloud sync, J bookmark, X compress, Z restart, Y/Enter chat, I voice, Q/Esc quit, (A/S opacity in v09) | README-SP, v01–v12 | PY-missing (subset PY). Note: A is used for both alert and opacity | — |
| Opacity control / fade on hover | v09 | PY (broken file) | — |

## 3. Movement modes

| Feature | Sources | Status | Repo |
|---|---|---|---|
| Wander / patrol (walk the desk or taskbar) | GH#1, aether_avatar_window, INV | PY (X11) / WEB | — |
| Follow pointer | GH#1 | WEB | — |
| Stay / park | GH#1 | WEB | — |
| Perch on window title bars; drop-to-sit on title bar after drag; snap-to-window | GH#1, INV, CAP (avatar_window) | PY-missing (needs window geometry) | — |
| Window prank / hang from window / peek from window or folder | GH#1, poses 08/45/49 | ART + WEB | — |
| Slows / sleeps when idle | GH#1, README-SP | SPEC | — |
| Climb window side; sit on ledge; drag a window to "ride" it; wander/follow/sit/sleep; speaker-based footstep sounds | P2-M ("Mote" Garuda build) | SPEC (same avatar lineage) | — |
| Pet-on-tap; pick up and throw | P2-M | SPEC | — |

## 4. System monitor events → pose mappings

Each row gives the event, the pose (clean name from pose_map / file in `assets/poses/`), the Linux data source to use in Rust, and status. "PY" means a Python prototype detects it; otherwise the pose only plays on a key press.

| Event / state | Pose (assets/poses) | Detection source (Rust) | Status | Repo |
|---|---|---|---|---|
| CPU high: psutil.cpu_percent(interval=0.5), threshold 72%, every 5.5 s; 60% → `system_high` (2100 ms) else `hacking` (1800 ms), skipped while already reacting | busy (39) / alert (51) / hacking | `/proc/stat` deltas (2 s poll), ≥ 72 % for 10 s, re-arm < 62 % for 10 s | **PY** (psutil; exact logic in mod_system_monitor/system_monitor.py, P2-§2) | IMPL (M4) `cpu.high` |
| RAM high | busy (39) / htop (29) | `/proc/meminfo` MemTotal − MemAvailable ≥ 85 % for 10 s, re-arm < 80 % | SPEC (INV system_event_watcher) | IMPL (M4) `ram.high` |
| System monitor view / "how's my system" | htop (29), diagnostics (37) | sysinfo (CPU, RAM, disk, load, uptime, temps, top procs) | PY-missing (README-SP agent tools) | STUB (M4): `lillith status` one-line snapshot (CPU, RAM, load, battery, backlight, net, VPN, drives); panel view M6 |
| Battery low (15%) | battery_low (12) | `/sys/class/power_supply` (system batteries only; peripheral `scope=Device` ignored; no battery = silent) | PY-missing | IMPL (M4) `battery.low` |
| Wi-Fi strength / connection check | wifi (40) | NetworkManager D-Bus | ART + key only | — |
| Connection lost / restored | connection_lost (01) | `/proc/net/route` + `ipv6_route` default route on an interface with carrier (`/sys/class/net/*/operstate`), 4 s debounce | SPEC (INV, GH#1) | IMPL (M4) `net.lost` / `net.restored` |
| Hotspot active (n devices) | hotspot (30) | NetworkManager | ART | — |
| VPN connected | vpn (22, guess) | `/sys/class/net` tun/tap/WireGuard/PPP interface comes up | ART + key | IMPL (M4) `vpn.up` |
| Bluetooth paired | bluetooth (05) | BlueZ D-Bus (bluer) | ART | — |
| Master volume change | volume (26) + volume frames | PipeWire/Pulse (libpulse or `pactl subscribe`) | ART + key | — |
| Screen brightness change | brightness (18) | `/sys/class/backlight/*` polled (≥ 2 points) | ART | IMPL (M4) `brightness.changed` |
| Firewall blocked IP | firewall (31) | journald (firewalld/ufw/nftables log) | ART. Detection unspecced | — |
| Safely eject USB / drive mounted | drive_eject (48) | `/sys/block` removable / USB-bus disks with media appear / vanish | ART | IMPL (M4) `usb.added` / `usb.removed` |
| Printing job | printing (20) | CUPS (D-Bus notifier / `lpstat` poll) | ART | — |
| Creating archive / compress | compress (44) | inotify on Downloads/home for .zip/.tar*, or process watch (zip/tar/ark) | ART | IMPL (M4) `archive.created` (inotify on ~/Downloads, incl. `.part` renames) |
| Cloud sync | cloud_sync (21) | process/inotify (rclone/nextcloud/syncthing). Guess | ART | — |
| Recycle bin / file deleted | recycle (15) | inotify on ~/.local/share/Trash | ART | IMPL (M4) `trash.added` |
| Private/incognito session | private (38) | window title contains "Private Browsing"/"Incognito" (KWin script) | ART | STUB: receiver + mapping done (`window.private`); needs the KWin script installed (M5) |
| Email / system update notification; dismiss | dismiss_notif (32) | org.freedesktop.Notifications monitor (zbus) | ART | IMPL (M4) `notify.received` (summary only, body never read) |
| System update available / restart needed | restart (unresolved), dismiss_notif | pacman checkupdates / notifications | ART | IMPL (M4) `update.available` from Discover/pamac/octopi/garuda notifications |
| Calendar meeting reminder | calendar (02) | notifications from KOrganizer/Merkuro, or .ics. Guess | ART | IMPL (M4) `calendar.reminder` from KOrganizer/Merkuro/kalendarac notifications |
| Clock / scheduled task | scheduler (10) | timers (own reminders, Pomodoro) | ART | — |
| Screenshot taken | screenshot (07) | ~/Pictures/Screenshots inotify or Spectacle D-Bus | ART + key | IMPL (M4) `screenshot.taken` |
| Clipboard copy | clipboard (25) | wl-clipboard-rs (watch) | ART + key | — |
| Cut / crop | crop (03) | clipboard / app focus. Guess | ART | — |
| File dragging / multi-select | drag_drop (23), multi_select (19) | not observable on Wayland. Own-window drag only | ART | — |
| Right-click / desktop shortcut menu | shortcut_menu (27) | own right-click | ART | — |
| Start menu opened | start_menu (36) | KWin script (plasmashell launcher window active) | ART | — |
| Search / magnifier | magnify (35) | KRunner/zoom activation (KWin). Guess | ART | — |
| Window opened (folder/browser/terminal/media/app → point/wave/salute/dance/nod + line) | peeking_folder (49), peeking (45), hacking (09), code_inspect (13) | KWin script → D-Bus | **PY** (wmctrl, X11) | STUB: `org.lillith.Bridge` receiver maps `window.opened.*`; script install/verify M5 |
| Window closed → "stab" knife | knife (28) | KWin script | **PY** (X11) | STUB: receiver maps `window.closed`; M5 |
| Window minimized | minimize (08) | KWin script | ART + WEB | STUB: receiver maps `window.minimized`; M5 |
| Browser history / bookmark | browser_history (00), bookmark (06) | window title heuristics. Guess | ART | — |
| Code editor focused / long work session → stretch nudge | code_inspect (13) | KWin active window + timer | PY-missing (README-SP) | — |
| User typing | (bounce) | not observable globally on Wayland; only in own chat bar | PY (X11 design) | — |
| Idle → sit → sleep; return → peek / wake | sitting (42), sleep (34), peeking (45) | ext-idle-notify-v1 (Wayland) / MIT-SCREEN-SAVER (X11) → `user.activity` (keeps her awake), `user.returned` after ≥ 120 s away; drowsy/asleep stay the brain's own timers | PY-missing | IMPL (M4) — needs Plasma 6 smoke test |
| Loading / busy app start | loading (16), running (24) | process spawn (sysinfo diff) | ART | — |
| Waiting for input / awaiting approval | waiting (11) | own confirmation dialogs | ART + SPEC (README-DK) | — |
| Error (app crash / journal error / LLM error) | error (43) | journald, own errors | PY (aether gesture_map on_error) | — |
| LLM thinking / done | think → tada; waiting | own | PY (aether) | — |
| Help request | help (46) | own | PY | — |
| Diagnostics / tools / settings / task manager | diagnostics (37), tools (50), 14/41/47 unresolved | own panels | ART | — |
| Dual-tail ready | dual_tail (04, guess) | key T | ART | — |
| Extra art without mapped events: cable management, file unlock, browser navigation, system log reading, activate shortcut, pretends to type, window snap (split-screen), loading complete, clipboard stack, icon drag (tail), help request (confused), sitting on icon, selfie | reference sheets 1–3 | various (KWin, journald) | ART (on reference sheets only; not cropped) | — |

## 5. AI chat / LLM / voice / memory

| Feature | Sources | Status | Repo |
|---|---|---|---|
| Local LLM chat via Ollama (llama3.2, phi3:mini, mistral, dolphin3 3B gguf Modelfile) | README-SP, README-DK, Modelfile, GH#6 | PY-missing (ollama_client not attached) | — |
| llama.cpp `llama-server` OpenAI-compatible endpoint :8080, chatml | GH#6 | SPEC (Rust snippet) | — |
| OpenVINO on Intel HD 620 (optimum-intel) streaming | lillith_os_v2 | PY (UI-thread bug) | — |
| Rust CLI chat on llama.cpp (`lillith model.gguf`, /quit) | GH#2 | SPEC (code not on box) | — |
| Axum local server + SQLite history (`lillith-local`) | GH#6 | SPEC (snippets) | — |
| Chat bar above avatar (Y/Enter), Esc closes; reply in bubble + voice | README-SP | PY-missing | — |
| Chat commands: my name is / call me / who am I / how do you feel / clear memory / remember that… / what do you know about me / status / system status | README-SP, lillith_memory | PY (memory side) | — |
| Persistent memory: profile, preferences, moods, events, conversation, facts; regex fact extraction | lillith_memory_sqlite | **PY (complete)** | — |
| Relationship warmth 0–1 (+chat, drift when absent) → greeting style reserved/neutral/warm/affectionate | lillith_memory_sqlite, README-SP | **PY** | — |
| Session count, last interaction, emotional continuity | lillith_memory_sqlite | **PY** | — |
| Live LLM context: name, facts, moods, events, time, active window, mood | README-SP, memory.build_context_summary | PY (partial) | — |
| Proactive comments (idle, mischievous, sleepy, alert, time-of-day) with cooldown | README-SP | PY-missing | — |
| Agent tools: system summary, disk, active window, top processes, network, uptime, temps, load | README-SP, CAP system_monitor | PY-missing | — |
| OpenVINO/optimum-intel inference on Intel HD 620 (PyQt6 frameless window, temp 0.7, max 150 tok, "SCAN DESKTOP" via `spectacle -f -b -n -o`) | lillith_os_v2.py (P2-§2) | PY (needs verify) | — |
| Tool approval gating (safe/risky/danger) + waiting pose | README-DK, CAP | PY-missing | — |
| Vision: screenshot + vision LLM (moondream/llava) "What do you see?" | lillith_os_v2 (Spectacle), CAP screenshot_analyzer, GH#3 | PY (capture only) | — |
| TTS: Piper → espeak-ng → festival → spd-say → silent | lillith_tts_engine, aether_tts | **PY (complete)** | — |
| STT push-to-talk: arecord/ffmpeg + whisper / whisper.cpp | lillith_voice_input, aether_stt | **PY (complete)** | — |
| Sound effects (19 cues) via pygame/paplay, mute | README_SOUNDS, README-SP | PY-missing. No sound files attached | — |
| Session logs JSONL + transcript | README-DK | PY-missing | — |

## 6. UI panels / tray / menus

| Feature | Sources | Status | Repo |
|---|---|---|---|
| System tray: Show/Hide, Random Reaction, Hacking Mode, Mute, Talk to Lillith, Status, Voice Listen, Quit | mod_tray, v04+, README-SP | PY (pystray) | — |
| Right-click context menu: upload image, settings, hide, toggle topmost | aether_avatar_window, GH#4 (pose menu) | PY | — |
| Settings panel (avatar image, patrol, HID reactions, pose map) | aether_avatar_settings | PY (CTk) | — |
| First-run onboarding: avatar upload / model scanner / Modelfile editor | aether_onboarding, aether_main_v1/v2 | PY (partial) | — |
| Side panel with commands (help, tidy desk, focus 1, scan), Poses tab, pulse meters | GH#4 | WEB | — |
| Fish-style terminal (`lillith@garuda`): event/pose/speed/sway/breath commands | GH#5 | WEB | — |
| Stage/reel player with sway/bounce sliders + event buttons with lines and moods | LillithStage.tsx | WEB | — |
| Knife-tab main window (10 tabs), brain terminal thought stream, floating orb, toolbar | README-DK, aether_main | PY-missing (out of core scope) | — |
| Global hotkeys Ctrl+Alt+A/O/V | README-DK | PY-missing (pynput, poor on Wayland) | — |
| Plans shown first, Approve button | GH#3 | WEB/SPEC | — |
| config.ini / config.json with all tunables | README-SP, config_A/B, mod_config | PY | — |

## 7. OS integration

| Feature | Sources | Status | Repo |
|---|---|---|---|
| Target: Garuda (Arch), KDE Plasma 6 **Wayland**, HP Spectre x360 i5-7200U / Intel HD 620, fish shell | lillith_os_v2, GH#5/#12, README-SP | info | — |
| Always-on-top on Wayland (layer-shell overlay; KWin "Keep above" rule fallback) | GH#12, README-SP troubleshooting | SPEC | M1: `zwlr_layer_shell_v1` Overlay layer + input-region click-through (compiles; needs on-machine smoke test); X11/XWayland fallback verified |
| X11 always-on-top via wmctrl | v12, GH#11 | PY (X11 only) | — |
| Window list/events via wmctrl/xdotool | aether_event_watcher | PY (X11 only) | — |
| Autostart (.desktop in ~/.config/autostart), install via pacman | install_arch_autostart, README-SP | PY | — |
| Windows build (PyInstaller --onefile, startup folder) | pyinstaller_windows_notes, README-DK | SPEC | — |
| Windows installer `Lillith-The-Master-Setup.iss` (Inno Setup: AppName "Lillith The Master" v1.0, copies all to {app}, launches tools\launch.bat) | P2-1c | SPEC (.iss not retrieved) | — |
| Android/Termux companion (chat over LAN) | README-DK | SPEC (out of scope) | — |
| Demon OS (Arch-based distro concept, dock, ISO bench) | GH#3/#10 | WEB (out of scope for companion; branding only) | — |

## 8. Assets & branding

| Item | Sources | Status | Repo |
|---|---|---|---|
| 52 system-event pose stills | assets/poses (+ pose_map names) | ART (need bg removal) | — |
| 6 alpha sprite sheets 3×10 (walk, hips idle, casts) | assets/sheets | ART (ready, needs smart slicing) | — |
| Left-run 4×3 sheet; alt-style action sheet | assets/sheets | ART (fake checkerboard) | — |
| 3 labelled pose sheets + new-pose grid + full-body ref | assets/reference | ART (fake checkerboard) | — |
| 4 physics keyframe references (idle transitions, knife tap) | assets/reference | ART | — |
| 80 video frames (640×960, black bg) | assets/frames_640x960 | ART (unordered) | — |
| 221-image micro-variant set | README-SP | MISSING | — |
| Sheet 3: 10 new desktop poses (Volume Check, Low Power, Download Bar, Wi-Fi Boost, Screenshot, Folder Dive, File Search, Notifications, Fully Charged, Restart); sheets 1/2 label set | P2-D, P2-F | ART-adjacent (labels confirmed; match to `assets/poses/`) | — |
| imagine_images/ and browsed_files/ project folders (generated art + browsed refs) | P2-1c | MISSING (folders not opened) | — |
| Sound effects | README_SOUNDS | MISSING | — |
| Chroma-key #00ff00 regeneration prompts | GH#9 | SPEC | — |
| 10 I2V motion ideas (peek tilt, tail-drag, screen clean, sit-and-swing, help-to-loading, key unlock, knife crack, rock-on sway, sleep yawn, running alert) | P2-§2 | SPEC | — |
| I2V prompt library (knife/tail/horn/tattoo/breath/face/idle) | phys_*, prompt_* | SPEC | — |
| Demon OS palette ink #120810, rose #e23d8a, ice #5fd0e6, paper #f8eef4; "Lillith 2029" theme #120014 / C026D3 (P2-B PASS: og.jpg 1200×630); "Lilith" void #07060b/ember #e23d8c (P2-A, card FAIL); Aether charcoal #12110f/ember #c46a45 (P2-C PASS); Garuda violet/cyan/dragon red | GH#8, route_root.tsx, README-DK, P2-A/B/C | SPEC | — |
| Name: Lillith (also "Lilith", "Nyx", "Sable", "Aether", "Cyber-Demon") | all | decide (open question) | — |
