# Release #3 notes (draft, `main` at `3156237`)

Draft notes for the next test build. They cover what changed since release #2
(`test-build-d88ed30`). There's no release zip for this build yet, so the
**Assets** line is left for whoever publishes it. Same format as the release #2
notes on GitHub.

**Assets:** _to be filled in when the build is published_ (Linux x86_64 zip,
its `.sha256`, and `INSTALL.md`). **INSTALL.md** covers Linux, Windows
(build from source) and Android/Termux.

## New in release #3

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
    `%LOCALAPPDATA%\lillith\memory.db` on Windows. The folder is created
    owner-only (0700) and the file 0600.
  - **Turn it off** with `[memory] enabled = false` in your config. Then
    nothing is read or written, and no memory file is created.
  - **Forget from the chat:**
    - "forget about <topic>" and "forget everything" ask first. Only a yes
      ("yes", "y", "confirm", "do it") goes ahead; anything else cancels.
    - "forget the last 30 minutes" (or hour, days, week, months) deletes
      right away. So does "forget the last thing I said", which covers the
      last 5 minutes.
    - Forgotten rows are deleted, the file is vacuumed, and the matching
      lines are also removed from her saved chat history.
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
    there's no translucent back leg.
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
  - Drop her (or let her fall) onto the top of a window and she plays the
    drag-to-perch blend and settles into a seated pose on the window edge,
    instead of landing standing and then sitting.
  - If a clip has no landing blend, she lands standing as before.
  - On a wide enough window she can also lie along the top edge
    (`perch_lounge`), shifted to fit the window top. On a narrow window she
    only sits.
  - When you resize the window, she refits, or stands up if she no longer
    fits.
  - Grabbing her mid-landing, or losing the window, cancels the blend cleanly.
- **New poses.** Five new "bold allure" clips:
  `hip_sway_slow`, `dom_throne_lean`, `kneel_sit_back_beckon`,
  `taskbar_lounge_lie` and `come_hither_crawl`, in the same outfit. They're
  wired into her idle, flirty, tease, prank and banter moves, and already
  cleaned to the no-white-fringe rules.
- **Art fixes:**
  - `dom_arms_crossed_demand` no longer jumps about 60 px for one frame, so
    no more double body flash during the crossfade. Only that frame was
    re-rendered.
  - The six perch clips (`perch_sit`, `perch_lounge`, the drag-to-perch
    blend, `arch_trace`, `chest_press_tease`, `cleavage_beckon`) lost their
    white edge fringe and the white patches between hair, neck and arms. Their
    positions are unchanged.
- **Offline voice kit (source).** The Python source of the voice kit is now
  in the repo under `tools/lillith-voice/`. That covers the `lillith-voiced`
  daemon, `lillith-say` / `lillith-listen` / `lillith-chat`, fish shortcuts,
  a systemd user unit and tests. Details:
  - The installer is offline by default. It installs only from local
    `models/` and `wheels/` folders and stops instead of going online.
    Installing system packages (`--pacman`) and fetching from PyPI
    (`--online-pip`) are both opt-in.
  - There are **no model weights, voice samples or wheels in the repo**.
    They come in the separate full kit tarball (`lillith-voice-kit.tar.gz`,
    about 731 MiB), which isn't published as a release asset yet.
  - At runtime the kit uses only a local socket or named pipe. Its network
    guard blocks everything that isn't loopback.
  - See INSTALL.md › *Optional: her cloned voice and the mic*.
- **Asset pack in the repo:** `master.zip` (about 585 MB, Git LFS) holds
  everything under `assets/` (sheets, mp4 clips, manifest, configs). CI
  skips it.
- **For artists:** `tools/matte-clean` re-mattes a clip folder in place to
  remove white fringe, and `qa_all.py --white-edge` checks for it.
- **Windows build:** clippy fixes in the elevated admin helper, so Windows
  is still build-from-source.

All 180 earlier clips are kept and 5 are new (185 in total); nothing was
removed. The local AI runs on 127.0.0.1 only, with no downloads and no
telemetry.

## Known issues

- The white-fringe cleanup is done for the six perch clips and the five new
  poses only (three of those still need the touch-ups below). Other clips can still show a thin light edge (see *Coming
  next*).
- Three of the new poses still have small matte leftovers until 01405a3
  lands (see *Landing next*). `come_hither_crawl` has a grey floor shadow
  and a purple smear, `kneel_sit_back_beckon` has grey specks in the
  hair/neck gap, and `hip_sway_slow` has a few white pixels.
- Memory forget, until a97a557 lands (see *Landing next*):
  - "forget about <topic>" can remove more rows than you meant.
  - A long time window ("forget the last 2 months") deletes without
    asking.
  - "forget the last thing I said" removes the last 5 minutes rather than
    just your last message.
- Voice kit:
  - The models aren't in any release yet (see above).
  - On Windows, only the named-pipe transport has been tested, and only
    under wine. Audio, the mic and the full daemon haven't run on real
    Windows, and no Windows installer bundles the kit.
  - On Android, voice is documented as a plan only.
- Windows is build-from-source (see INSTALL.md). The Android APK was not
  rebuilt; the older, untested one is on `test-build-f8cd236`.
- Muting from the right-click menu isn't remembered after a restart.

## Landing next (on the lead's integration branch, not on `main` yet)

Both are merged into the lead's `ci/batch9`, which isn't on GitHub yet. Their
feature branches are on GitHub.

- **Precise memory forget** (`feat/memory-forget-precision`, a97a557):
  - "forget about <topic>" deletes only rows that contain the whole topic:
    every word of a one- or two-word topic, or at least two thirds of a
    longer one. Plurals match only the same word (job/jobs, never
    new/news). `job = "google engineer"` no longer deletes a "google
    chrome" event. Her chat history uses the same rule.
  - The confirmation shows how many memories and chat messages match, plus
    up to 3 examples.
  - A time window longer than an hour now asks for "yes" first; an hour or
    less still deletes right away.
  - "forget the last thing I said" drops only your last message and her
    reply from the chat history. In memory it still covers the last 5
    minutes.
  - On Windows, the data folder no longer falls back to the roaming
    `%APPDATA%`; it's `%LOCALAPPDATA%\lillith`.
  - A corrupt chat-history file is set aside as `.corrupt`. An unreadable
    one is never overwritten.
  - Each forget also adds a line to `audit.log` next to the memory file,
    with the kind of forget and the counts only.
- **Matte fixes for three new poses** (`art/slutty-poses`, 01405a3): cleans
  up `come_hither_crawl`, `kneel_sit_back_beckon` and `hip_sway_slow` (see
  *Known issues*). Their names, timing and placement are unchanged.

## Coming next (pending branches, not in this build)

- **No white fringe** (`fix/no-white-fringe-ev`, `fix/no-white-fringe-gm`,
  `fix/no-white-fringe-nr`). These re-matte the rest of her clips to remove
  white edges:
  - the `event_*` clips (keeping their icons, panels and effects)
  - clips `g*`–`m*`, with a stricter pass that checks the frames as the
    app decodes them
  - clips `n*`–`r*`, including filling interior gaps on six of them

## Gate

_To be filled in for the published build._ (Release #2 was Rust 1.85 fmt,
clippy `-D warnings`, build and tests, plus the headless smoke test and green
CI.)
