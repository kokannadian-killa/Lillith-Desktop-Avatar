# Pose Builder: import your own videos

Settings → **Pose Builder** (`lillith-settings --tab import`) turns your own
video into poses that sit next to the built-in ones.

## Flow

1. **Pick a video.** Drag it onto the drop zone, click **Browse…**, or paste a
   path. `.mp4 .gif .webm .mov`, any size. The file is never loaded into the
   app; only its path goes to the pipeline.
2. **Start import.** A Soul Contract card shows the exact command. Only a
   mouse click on **Yes** runs it (the keyboard never approves; Esc / No
   denies). Either choice is written to `audit.log` (`CONFIRM` / `DENY`,
   note `pose builder import <file>`).
   **Advanced** (optional): the cut-out method (`--matte auto|isnet|key|alpha`)
   and a cap on the number of clips (`--max-candidates`, 0 = no limit).
3. **Progress.** The pipeline runs as a child process. Its `pct` drives the
   bar (it covers the whole job and only rises); the stage is shown as text
   only, since it can jump back (qa → matte). **Cancel** sends SIGTERM
   (Linux) or `CTRL_BREAK_EVENT` to the child's process group (Windows; the
   child is started with `CREATE_NEW_PROCESS_GROUP`, and TerminateProcess is
   never used). The pipeline deletes what that run created and exits 130;
   the app then removes the staging folder. On Linux a run still alive after
   5 s is SIGKILLed with its process group; on Windows the break is re-sent.
   Closing the window mid-run does the same. The `done` event's peak memory
   (`peak_rss_mb`) is written to the log.
4. **Clips.** Each proposed clip gets a card with its preview image. **▶ Play**
   decodes the MP4 in the app (no outside player). The card also shows the QA
   verdict (white edge, fringe), an editable name, and **Approve** /
   **Reject**. A clip that failed QA gets a red **QA FAIL** badge; Approve
   stays available, and its approval card opens with a clear
   `⚠ QA FAILED` warning. You decide.
   - **Approve** writes files, so it gets its own mouse-only Soul Contract
     card. The candidate folder is then copied into `user-clips/<name>/`.
     The `<stem>.*` files are renamed to `<name>.*`; `name`, `sheet` and
     `video.file` in the JSON are updated; `_preview.png` and `qa/` are left
     out. The clip is added to `user-clips/user_manifest.toml`, and the
     staged folder is deleted.
   - **Reject** deletes that clip's staged folder.
   - Once every card is decided, the job's staging folder is removed.
   - If the pipeline fails partway, the clips it already announced stay on
     screen and can still be approved.

## Folders

| What | Linux | Windows |
|---|---|---|
| Staging (temporary) | `~/.local/share/lillith/import-staging/<job>/` | `%LOCALAPPDATA%\lillith\import-staging\<job>\` |
| Your clips | `~/.local/share/lillith/user-clips/` | `%LOCALAPPDATA%\lillith\user-clips\` |

## Add-only

Your clips **never replace** a built-in clip or one of your earlier clips.

- A name that's already taken, by a built-in clip, a user clip, or a stray
  folder, gets a `_2`, `_3`, … suffix.
- Files are created with `create_new`, so nothing existing is ever opened
  for writing.
- When clips are loaded (overlay `Stage::load`, the panel's Poses and the
  Settings Poses list), `lillith_anim::user_clips::load_with_user_clips`
  merges `user_manifest.toml` into the built-in set the same way: a user
  entry whose name is taken is skipped. A broken user manifest is logged
  and ignored, so the built-in set always loads.
- User clips carry the tag `user`.

## ffmpeg / ffprobe

The app looks for each tool in this order, without running anything:

1. `$LILLITH_FFMPEG` / `$LILLITH_FFPROBE` (an explicit override),
2. **bundled** next to the program: `<exe dir>/bin/ffmpeg(.exe)`, then
   `<exe dir>/ffmpeg(.exe)`. The Windows release zip ships
   `bin\ffmpeg.exe` and `bin\ffprobe.exe`. On Linux a `bin/ffmpeg` next to
   the program is used if present.
3. `PATH`.

Only a **bundled** `bin/` copy is passed to the pipeline (`--ffmpeg <path>`
/ `--ffprobe <path>`). Otherwise the pipeline does its own lookup
(`LILLITH_FFMPEG` / `LILLITH_FFPROBE`, ffprobe next to ffmpeg, then PATH).
The app still checks the same places first, so a missing ffmpeg gets a clear
message instead of a failed run. No ffmpeg binary is committed to the repo; the release packaging puts them in
`bin/`.

`install.sh` checks for `ffmpeg` and `ffprobe` (or a bundled `bin/`).

- **Arch / Garuda:** if they're missing, it prints `sudo pacman -S --needed ffmpeg`
  and asks `[y/N]`. The default is **No**, also with no terminal, and it never
  installs silently.
- **Other distros:** it says which package to install.

If Python, ffmpeg/ffprobe or the pipeline script is missing, the tab says
exactly what to install instead of starting.

## The pipeline contract

`tools/video-pipeline/user_import.py` is built separately. This is the
contract the app speaks:

```
python3 tools/video-pipeline/user_import.py --input <file> --out <staging_dir> \
    --json-progress --max-ram-mb 1500 --threads 2 \
    [--matte auto|isnet|key|alpha] [--max-candidates N] \
    [--ffmpeg <bin/ffmpeg> --ffprobe <bin/ffprobe>]   # bundled copies only
```

stdout is one JSON object per line:

- `{"event":"progress","stage","pct","msg"}`
- `{"event":"candidate","id","mp4","preview_png","frames","fps","qa":{"pass","white_edge","fringe"},"suggested_name"}`
- `{"event":"done","candidates","peak_rss_mb"}`
- `{"event":"error","msg"}`

Other lines are shown in the log and are never fatal. Candidate paths
(absolute, or relative to the staging folder) must resolve inside the
staging folder, and anything else is ignored.

Each candidate is a folder `<out>/user_<slug>_<NN>/` holding `<stem>.json`,
`<stem>.mp4`, `<stem>_sheet.png`, `<stem>_preview.png` and `qa/`. `qa.pass`
means white edge OK and fringe PASS or WARN. Failing candidates are still
emitted. The clip JSON's `frame_count` / `frame_width` / `frame_height` /
`fps` / `loop` / `anchor` / `body_anchor_x` / `baseline_y` / `tags`
become the manifest entry, the same way `tools/import-manifest` maps them.
The `baked_motion` tag mutes the engine's jiggle for the clip, and an
optional `engine_springs` adds `disable_springs`.

Exit codes: 0 means ok, 1 means error, 130 means cancelled.

Script lookup:

1. `$LILLITH_VIDEO_IMPORT`
2. `<exe dir>/tools/…`
3. `<exe dir>/../share/lillith/tools/…`
4. `./tools/…`
5. the source tree

Python is `$LILLITH_PYTHON`, else `python3` / `python` on PATH.

## Android (feasibility note, no code)

Not planned for the first version:

- The Android build has no Settings window. Its UI is the Java shell
  under `android/`.
- Running CPython with ffmpeg and the matting models on-device would mean
  bundling a Python runtime (Chaquopy or similar) and a mobile ffmpeg build.
  It would also be heavy on RAM and battery, given the pipeline's ~1.5 GB
  budget.

A workable path would be to import on the desktop and sync `user-clips/` to
the phone. The clip format (MP4 plus `user_manifest.toml`) already works
across platforms, and the Android loader would only need the same add-only
merge.
