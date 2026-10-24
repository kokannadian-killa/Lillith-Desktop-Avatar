# Release #3 notes (draft, `ci/batch6` at `3e2748d`)

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

- The white-fringe cleanup is done for the six perch clips only. Other clips
  can still show a thin light edge (see *Coming next*).
- Voice kit:
  - The models aren't in any release yet (see above).
  - On Windows, only the named-pipe transport has been tested, and only
    under wine. Audio, the mic and the full daemon haven't run on real
    Windows, and no Windows installer bundles the kit.
  - On Android, voice is documented as a plan only.
- Windows is build-from-source (see INSTALL.md). The Android APK was not
  rebuilt; the older, untested one is on `test-build-f8cd236`.
- Muting from the right-click menu isn't remembered after a restart.

## Coming next (pending branches, not in this build)

- **No white fringe** (`fix/no-white-fringe-ev`, `fix/no-white-fringe-gm`,
  `fix/no-white-fringe-nr`). These re-matte the rest of her clips to remove
  white edges:
  - the `event_*` clips (keeping their icons, panels and effects)
  - clips `g*`–`m*`, with a stricter pass that checks the frames as the
    app decodes them
  - clips `n*`–`r*`, including filling interior gaps on six of them
- **Memory** (`feat/memory-graph`). A local learning memory: one SQLite file
  (default `~/.local/share/lillith/memory.db`) with no network code.
  - She remembers what you tell her (people, preferences, habits), app use
    and errors and how they were fixed.
  - Unused memories fade over time.
  - A small relevant slice goes into each chat turn.
  - "Forget …" requests are handled locally.
  - Messages containing the safeword are never stored.
  - It adds a `[memory]` config table, on by default; `enabled = false`
    turns it off.

## Gate

_To be filled in for the published build._ (Release #2 was Rust 1.85 fmt,
clippy `-D warnings`, build and tests, plus the headless smoke test and green
CI.)
