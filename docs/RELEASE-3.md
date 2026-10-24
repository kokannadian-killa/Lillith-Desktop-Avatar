# Release #3 notes (draft, `ci/batch12`)

Draft notes for the next test build. They cover what changed since release #2
(`test-build-d88ed30`). There's no release zip for this build yet, so the
**Assets** line is left for whoever publishes it. Same format as the release #2
notes on GitHub.

**Assets:** `Lillith-main-linux.zip` (one all-in-one download: programs,
all art, art kit, offline voice kit), its `.sha256`, and `INSTALL.md`;
sizes and checksums are filled in when the build is published. **INSTALL.md** covers Linux, Windows
(build from source) and Android/Termux.

## New in release #3

- **Security: only a mouse click signs a Soul Contract.** Before, Tab then
  Enter (or Space) could press the focused **YES** plate, because egui counts
  a keyboard press on a focused button as a click. Now the Yes plate can't
  take keyboard focus and only a real primary mouse click on it counts. Tab,
  Enter, Space, voice and typed text never say yes. **No** still works from
  the keyboard, and `Esc` still means No.
- **Security: the self-started llama-server is locked to this computer.**
  - It always listens on `127.0.0.1` (the endpoint must be `127.0.0.1`,
    `localhost` or `::1`).
  - `[chat.server] extra_args` can no longer change the host, port, model or
    API key, download a model (`-hf…`, `--model-url`, `--docker-repo`, any
    `…-url` flag), reach other machines (`--rpc`) or serve files (`--path`).
    Only an allowlist passes: the normal tuning flags, `--jinja`,
    `--chat-template <built-in name>`, `-np`/`--parallel`,
    `-cb`/`--cont-batching` and `-mg`/`--main-gpu`. Anything else (including
    file flags such as `--lora`, `--mmproj`, a template file) stops the start,
    is logged as `ARG_REFUSED` and is named in a notice in the settings
    window. See "Migrating `extra_args`" in CHAT.md.
  - `lillith chat --spawn-server` no longer takes a typed `y`: only a click
    on the card's Yes starts the server. With no desktop it refuses and
    prints the command to start llama-server yourself.
  - The API key is handed over in the `LLAMA_API_KEY` environment variable,
    not on the command line, and the startup log masks it. Stray `LLAMA_ARG_*`
    variables are cleared so nothing outside the config can steer the server.
- **One all-in-one download.** `Lillith-main-linux.zip` has the ready-made
  programs, all her art and pose videos, the art kit and the offline voice
  kit. `bash install.sh` installs with no downloads and no build: it only
  lists missing pacman libraries and asks. It finds your model at
  `~/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf`, then `~/models/*.gguf`.
  The model itself is never in the zip and never downloaded.

- **The safeword now drops the queued reply.** Typing `red` or `OOC: stop`
  in her chat (or saying "red" into the mic) already stopped her voice and any
  running command. Now the reply it interrupts is gone too. Words that were
  already queued are never shown in the bubble or spoken, even during a fast
  burst of tokens. The next message you send starts a fresh reply as usual.
  (Fixes the release #2 known issue.)
- **She keeps her pose when she starts talking.** When the voice daemon is
  speaking, she switches to her talking clip only while she's just standing
  there. She never switches while you drag her or while she's teasing,
  sitting or posing. She also holds the talking clip for about a second
  between sentences, so she doesn't flicker talk → idle → talk. (Fixes the
  release #2 known issue.)
- **Learning memory (on by default, local only).** She now keeps a small
  memory of what you tell her (people, likes, habits), which apps you use,
  and which errors happened and what fixed them.
  - Each chat turn (bubble and `lillith chat`) gets a short `[Memory]` note
    of at most about 300 tokens. Memories you don't use fade by half every 14
    days, and weak ones are deleted.
  - It's one SQLite file with no network code:
    `~/.local/share/lillith/memory.db` (respects `$XDG_DATA_HOME`), or
    `%LOCALAPPDATA%\lillith\memory.db` on Windows (never the roaming
    `%APPDATA%`). On Linux the folder is created owner-only (0700) and the
    file 0600; on Windows it uses the default per-user permissions.
  - **Turn it off** with `[memory] enabled = false` in your config. Then
    nothing is read or written, and no memory file is created.
  - **Forget from the chat:**
    - "forget about <topic>" and "forget everything" ask first. Only a yes
      ("yes", "y", "confirm", "do it") goes ahead; anything else cancels.
      For a topic or a window over an hour, the confirmation shows how many
      memories and chat messages match, plus up to 3 examples. "Forget
      everything" just asks, with no counts or examples.
    - "forget about <topic>" deletes only rows that contain the whole topic:
      every word of a one- or two-word topic, or at least two thirds of a
      longer one. Plurals match only the same word (job/jobs, never
      new/news), so `job = "google engineer"` no longer deletes a "google
      chrome" event. Her chat history uses the same rule.
    - A topic word that is a whole memory key also matches it, so "forget
      about my new job" finds "Ko's job: barista" (and still never "new wave
      music").
    - She's honest about what was forgotten. If nothing in memory matched
      but chat messages did, she says "Nothing in my memory matched …" and
      how many chat messages she cleared, never "Gone".
    - "forget the last 30 minutes" (or an hour) deletes right away. A longer
      window (days, a week, months) asks for "yes" first.
    - "forget the last thing I said" drops only your last message and her
      reply from the chat history. In memory it covers the last 5 minutes.
    - Forgotten rows are deleted, the file is vacuumed, and the matching
      lines are also removed from her saved chat history. Each forget adds a
      line to `audit.log` (the same log the Soul Contract uses) next to the
      memory file, with the kind of forget and the counts only. It's the same
      `audit.log` even if you set a custom `[memory] db_path`.
  - A corrupt chat-history file is set aside as `.corrupt`. An unreadable one
    is never overwritten.
  - The safeword is never stored, nor is any message containing the word
    "red" on its own.
  - Other `[memory]` keys: `db_path`, `half_life_days`, `top_k`,
    `max_context_tokens`, `prune_below`, `max_nodes`, `max_events` and
    `max_errors`. They're all in `assets/config.example.toml`.
  - The desktop app only. The Android app has no memory.
- **No see-through double body during fast moves.**
  - When two frames barely overlap, the smooth crossfade is now shortened,
    or skipped with a hard switch. Before, she could briefly look like two
    translucent bodies.
  - Walk, run and the sultry `strut_*` walks always switch frames hard, so
    there's no translucent back leg. The same goes for the blends into or
    out of a walk, run or strut.
  - The desktop and the Android renderer both use this.
- **Logs go to stderr, never stdout.** The chat bubble and Settings windows
  use stdout to talk to the overlay, so log lines there could break them.
  Command-line output (`lillith status`, `lillith chat`, …) stays on stdout.
  On Windows, when she isn't started from a terminal, logs still go to
  `%LOCALAPPDATA%\lillith\lillith.log`.
- **Voice kit: tighter local-only checks.**
  - Its network guard now also refuses to *listen* on anything but loopback.
    It rejects `0.0.0.0`, `::` and LAN addresses; Unix sockets are
    unchanged.
  - `lillith-chat` now uses the app's sampler defaults (temperature 0.95,
    160 reply tokens).
  - Its README shows the standard `llama-server` line on 127.0.0.1:8080
    with `--jinja`.
- **Perching: she lands on windows and lounges on them.**
  - Drop her (or let her fall) onto the top of a window and she lands
    standing, then sits or lounges on the window edge.
  - The drag-to-perch landing blend stays off in this build. The engine only
    lands through a blend whose first frame matches the drag pose, and the
    current art doesn't yet; it will be re-rendered to match
    (`tools/video-pipeline/blend_start_match.py` checks this).
  - Sitting down on a window is smoother: `perch_sit` has in-between frames
    for the leg cross and uncross (96 to 102 frames), timed so each cross
    still takes a quarter of a second.
  - On a wide enough window she can also lie along the top edge
    (`perch_lounge`), shifted to fit the window top. On a narrow window she
    only sits.
  - When you resize the window, she refits, or stands up if she no longer
    fits.
- **New poses.** Five new "bold allure" clips:
  `hip_sway_slow`, `dom_throne_lean`, `kneel_sit_back_beckon`,
  `taskbar_lounge_lie` and `come_hither_crawl`, in the same outfit. They're
  wired into her idle, flirty, tease, prank and banter moves, and cleaned to
  the no-white-fringe rules. The last matte leftovers are gone too: the grey
  floor shadow and purple smear on `come_hither_crawl`, the grey specks in
  the hair/neck gap on `kneel_sit_back_beckon`, and stray white pixels on
  `hip_sway_slow`. The last pale specks in the hair/neck gaps on
  `kneel_sit_back_beckon` and the purple floor speck and faint floor haze on
  `come_hither_crawl` are now removed too, along with the grey floor-shadow
  slivers under her shin and boot, the purple smears beside her boot and the
  lavender floor-shadow residue near the floor (plus a small shadow ledge
  under her toe) on `come_hither_crawl`, whose leg and thigh gaps are now
  see-through too. Their names, timing and placement
  are unchanged.
- **Five more poses:** `leg_cross_slow_sit`, `hair_twirl_lip_bite`,
  `tail_heart_wink`, `trace_heart_on_glass` and `dom_chin_tilt_summon`, in
  the same outfit, wired into her idle, tease and banter moves. White
  backdrop pockets in the hair and neck gaps and stray specks are cleaned
  out of all five.
- **Four more poses:** `lean_doorframe_smolder`, `kiss_mark_on_glass`,
  `dom_snap_kneel_order` and `finger_walk_on_glass`, in the same outfit,
  wired into her idle, tease and banter moves (the tease lists in
  `assets/config.example.toml` include them).
- **Art fixes:**
  - `dom_arms_crossed_demand` no longer jumps about 60 px for one frame, so
    no more double body flash during the crossfade. Only that frame was
    re-rendered.
  - The six perch clips (`perch_sit`, `perch_lounge`, the drag-to-perch
    blend, `arch_trace`, `chest_press_tease`, `cleavage_beckon`) lost their
    white edge fringe and the white patches between hair, neck and arms. Their
    positions are unchanged.
  - The tail tip on `perch_sit` and the drag-to-perch blend no longer floats
    loose; it's joined back to the tail.
  - `tail_tease` was regenerated with a single tail.
- **No more white fringe.** Her clips were re-matted to remove the thin
  white edge and the white backdrop showing through gaps (between hair,
  neck, arms and legs). That covers 177 of the 194 clip folders, including
  the `event_*` clips, whose icons, panels and effects are kept. A final
  pass re-cleaned about 91 clips on top of the earlier ones, with a guard so
  no body part, outline or hole was eaten. Frame counts, timing and
  placement are unchanged. The perch clips and `come_hither_crawl` keep
  their own fixes from above.
  - `dom_beckon_finger` and `smug_smirk` lost the white blobs at the neck.
  - Some light pixels are part of the art and stay: skin highlights, the
    collar, eye whites, `tail_tease`, the `soul_contract_sign` effects and
    `event_screenshot`'s deliberate flash.
- **Offline voice kit (source).** The Python source of the voice kit is now
  in the repo under `tools/lillith-voice/`. That covers the `lillith-voiced`
  daemon, `lillith-say` / `lillith-listen` / `lillith-chat`, fish shortcuts,
  a systemd user unit and tests. Details:
  - The installer is offline by default. It installs only from local
    `models/` and `wheels/` folders and stops instead of going online.
    Installing system packages (`--pacman`) and fetching from PyPI
    (`--online-pip`) are both opt-in.
  - There are **no model weights, voice samples or wheels in the repo**.
    The all-in-one `Lillith-main-linux.zip` carries them (voice models and
    Python wheels for 3.13 and 3.14), and `install.sh` sets the voice up from
    them offline.
  - At runtime the kit uses only a local socket or named pipe. Its network
    guard blocks everything that isn't loopback.
  - See INSTALL.md › *Optional: her cloned voice and the mic*.
- **Asset pack:** use the all-in-one zip. The old `master.zip` in the repo
  (Git LFS) is an outdated snapshot; CI skips it and no release zip has it.
- **For artists:** `tools/matte-clean` re-mattes a clip folder in place to
  remove white fringe (with a protect guard and `regress.py` to catch
  anything it eats), and `qa_all.py --white-edge [--mp4]` checks for it.
- **Windows build:** clippy fixes in the elevated admin helper, so Windows
  is still build-from-source.

All 180 earlier clips are kept and 14 are new (194 in total); nothing was
removed. The local AI runs on 127.0.0.1 only, with no downloads and no
telemetry.

## Known issues

- **Upgrading on Windows:** her data folder is now `%LOCALAPPDATA%\lillith`
  (never the roaming `%APPDATA%`). That's where `audit.log`, the Soul
  Contract's `plans.log` and `commands.log`, the `undo-organize` journal and
  `memory.db` now live. Files from release #2 under
  `%HOME%\.local\share\lillith` aren't moved, so an organize batch made
  with release #2 can't be undone from release #3.
- `qa_all.py --white-edge` still flags some clips. Most of what it finds is
  the keep-as-art light pixels listed above (highlights, collar, eye whites,
  effects), not backdrop fringe.
- Voice kit:
  - Voice needs Python 3.13 or 3.14 (the bundled wheels). With another
    Python the installer says so and carries on without voice.
  - Voice is Linux-only in release #3: Windows and Android voice aren't in
    scope (no Windows or Android build bundles the kit).
  - On Windows, only the named-pipe transport has been tested, and only
    under wine. Audio, the mic and the full daemon haven't run on real
    Windows, and no Windows installer bundles the kit.
  - On Android, voice is documented as a plan only.
- Windows is build-from-source (see INSTALL.md); there's no Windows
  all-in-one zip in this release. The Android APK was not
  rebuilt; the older, untested one is on `test-build-f8cd236`.
- Muting from the right-click menu isn't remembered after a restart.

## Gate

_To be filled in for the published build._ (Release #2 was Rust 1.85 fmt,
clippy `-D warnings`, build and tests, plus the headless smoke test and green
CI.)
