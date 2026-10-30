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

- `{"event":"progress","stage","pct","msg","ts","elapsed_s"}`
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

Python is `$LILLITH_PYTHON`, else the interpreter bundled next to the
program (`<exe dir>\python\python.exe` from the Windows setup, which ships
numpy, scipy, opencv-python-headless, Pillow and onnxruntime;
`<exe dir>/python/bin/python3` on Linux if present), else `python3` /
`python` on PATH. The importer also needs `tools/matte-clean/` next to
`tools/video-pipeline/` (it imports `matte_clean`).

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

---

<!-- Merged in ci/batch30: the Pose Builder (app) guide above comes from feat/video-import; the engine reference below comes from feat/video-import-pipeline, unchanged except headings moved one level down. -->

## Video import: user videos → Lillith pose candidates

`tools/video-pipeline/user_import.py` is the engine behind "add my own video". The GUI spawns it, reads JSON
lines from stdout, shows the candidates, and copies the ones the user keeps into the clip library. The engine
does **no** library writes, no network access, no downloads, and writes nothing outside `--out`.

```
python3 tools/video-pipeline/user_import.py --input <file> --out <staging_dir> --json-progress [--max-ram-mb 1500]
```

### Flags

| flag | default | meaning |
|---|---|---|
| `--input` | required | `.mp4 .m4v .gif .webm .mov`, any size or length (streamed, never loaded whole) |
| `--out` | required | staging dir, created if missing. Everything goes here. |
| `--json-progress` | off | JSON lines on stdout (the GUI contract). Without it, stdout gets human-readable progress lines instead. |
| `--max-ram-mb` | 1500 | cap for the importer **plus** its ffmpeg child (150 MB of it is reserved for ffmpeg) |
| `--ffmpeg` | `LILLITH_FFMPEG`, then PATH | ffmpeg binary **or its folder**, e.g. `C:\Lillith\bin\ffmpeg.exe` or `C:\Lillith\bin` |
| `--ffprobe` | `LILLITH_FFPROBE`, then next to ffmpeg, then PATH | ffprobe binary or its folder |
| `--matte` | `auto` | `auto`, `alpha` (use the file's own alpha), `isnet` (local model), `key` (plain-backdrop key) |
| `--model` | see [Model](#model): `$U2NET_HOME`, the installer's `models/rembg`, `~/.u2net`, `~/.rembg/models/isnet-anime` | isnet model file (or its folder); never downloaded |
| `--zmax` | 1.8 | peak cleavage zoom; `1` turns the zoom off |
| `--min-seconds` / `--max-seconds` | 1.5 / 8 | movement length limits |
| `--max-candidates` | 0 (no limit) | stop after N candidates (useful for long videos) |
| `--threads` | 2 | onnxruntime threads for isnet |

**ffmpeg lookup order:** flag → env (`LILLITH_FFMPEG` / `LILLITH_FFPROBE`) → for ffprobe, `ffprobe(.exe)` beside
the chosen ffmpeg → PATH. A folder argument looks for `ffmpeg.exe` then `ffmpeg` inside it. Each binary is
checked with `-version`. The Windows zip ships `bin\ffmpeg.exe` (and `bin\ffprobe.exe`), so the GUI should pass
`--ffmpeg <install>\bin\ffmpeg.exe` and can leave `--ffprobe` unset. Paths with spaces are fine: every
subprocess uses an argument list, never a shell.

### stdout: JSON lines (one object per line, nothing else)

```json
{"event":"progress","stage":"matte","pct":37,"msg":"movement 2/4: frame 31/58","ts":1793212345.678,"elapsed_s":64.21}
{"event":"candidate","id":"user_dance_02","mp4":"/abs/staging/user_dance_02/user_dance_02.mp4",
 "preview_png":"/abs/staging/user_dance_02/user_dance_02_preview.png","frames":58,"fps":12,
 "qa":{"pass":true,"white_edge":"OK","fringe":"PASS"},"suggested_name":"user_dance_02"}
{"event":"done","candidates":4,"peak_rss_mb":612}
{"event":"error","msg":"ffmpeg not found. Install FFmpeg ... or pass --ffmpeg/--ffprobe ..."}
```

* `stage` ∈ `probe decode split matte clean zoom encode qa`. `pct` is an int 0–100 for the **whole job** and
  never goes backwards (stages interleave per movement, so the stage can go back from `qa` to `matte`).
  Progress lines are throttled to about two per second. `ts` is the wall clock (unix seconds, float) and
  `elapsed_s` the seconds since the importer started (monotonic clock, float); both were added in a
  backwards-compatible way, so readers that only know `stage`/`pct`/`msg` keep working. Per-frame speed is the
  `elapsed_s` difference between two `matte` lines divided by the frame difference in their `msg`.
* A `candidate` is emitted as soon as its movement is finished, before the rest of the video is done.
  `id == suggested_name ==` the candidate's folder name (`user_<input-slug>_<NN>`, bumped if it already exists).
* `qa.pass` = `qa_all.py --white-edge --mp4` printed OK **and** fringeqa gave PASS or WARN. `white_edge` is
  `OK`/`FAIL`, and `fringe` is `PASS`/`WARN`/`FAIL`. A failing candidate is still produced; the GUI decides
  whether to show it.
* `done` carries an extra field, `peak_rss_mb`: the peak RSS of the whole process tree (the importer **plus** its
  ffmpeg and QA children), sampled every 0.25 s by a background thread (psutil if installed, else
  `/proc/<pid>/task/*/children` on Linux, a Toolhelp32 snapshot + `GetProcessMemoryInfo` on Windows).
* Exactly one terminal line: `done` or `error`. On cancel there is none.

### Exit codes and cancel

| code | meaning |
|---|---|
| 0 | success (`done` printed) |
| 1 | error (`error` printed): missing ffmpeg, unreadable or unsupported input, RAM cap too low or exceeded, … |
| 130 | cancelled |

Cancel with **SIGTERM** (or SIGINT) on Linux/macOS. On Windows, spawn the importer with
`CREATE_NEW_PROCESS_GROUP` and send `CTRL_BREAK_EVENT` to it (`TerminateProcess` cannot run cleanup). The
importer kills its ffmpeg children and exits within about 1 s:

* if `--out` did not exist before the run, it is removed entirely;
* otherwise only what this run created inside it (its candidate folders and `.user_import_work`) is removed,
  and other files in `--out` are left alone.

On an error exit, candidates already announced are kept; only the work dir and the half-finished movement are removed.

### Candidate folder

```
<out>/user_<slug>_<NN>/
  user_<slug>_<NN>.json         same schema as shipped clips (frame_count, fps 12, anchor [200,392], baseline_y 392,
                                cleavage_zoom block, video block, bbox_per_frame, …) plus a "user_import" block;
                                loop: false; tags include baked_motion, user_import (+ cleavage_zoom when zoomed)
  user_<slug>_<NN>.mp4          alpha-packed H.264, 800x400 (colour | alpha as grey), exactly like pack_mp4.py
  user_<slug>_<NN>_sheet.png    RGBA strip, 400 px per frame
  user_<slug>_<NN>_preview.png  one frame on grey, for the picker
  qa/                           only if a QA tool wrote images (fringe crops)
```

To keep a candidate, the GUI copies the folder into the clip library under the user's chosen name, renaming the
three `<name>.*` files and updating `name`, `sheet` and `video.file` in the JSON. A shipped clip folder holds
only `json`, `mp4` and `_sheet.png`, so drop `_preview.png` and `qa/`.

### What it does

1. **probe**: ffprobe (file/pipe protocols only). Handles rotation metadata. Detects alpha (`yuva*`, `rgba`, GIF
   palettes, WebM `alpha_mode`; VP8/VP9 alpha is decoded with libvpx).
2. **decode/split**: first streaming pass at 12 fps and 64 px wide grey. Cuts on scene changes (frame difference
   far above the recent median, or a histogram jump), then in the middle of motion holds of ≥ 0.4 s
   (pose changes) when both sides stay ≥ `--min-seconds`. Pieces longer than `--max-seconds` are cut at the
   calmest frame; short leftovers are merged and anything under 0.5 s is dropped.
3. **matte**: second streaming pass at 720 px high (540 px if `--max-ram-mb` < 700), about 1.85× the
   on-canvas scale, i.e. the "matte at 2×" step.
   * `alpha`: the file's own alpha. Opaque frames from GIF/WebM/MOV fall back to the next mode.
   * `isnet`: `matte_v2.py` functions (isnet-anime ONNX on onnxruntime, then `raw_alpha`, `colour`,
     `drop_specks`). Used by `auto` only when the model file exists, onnxruntime imports, and the cap is ≥ 1100 MB.
     If RAM runs over the cap mid-run it downgrades to `key`.
   * `key`: white backdrops (border median ≥ 242) use `key_white` fed into matte_v2's `raw_alpha`/`colour`.
     Other plain backdrops are flood-filled from the border colour and un-mixed against it.
   Each matted frame is cropped and spilled to `<out>/.user_import_work/` as a PNG, so RAM does not grow with
   video length.
4. **zoom**: `zoom_tease.py` math (ported; that script is a CLI with side effects at import). Uses
   one sampling grid and **one scale per movement** (no pumping), the lowest point of the movement on baseline
   392, and a premultiplied INTER_AREA downscale.
   * **Scale fit:** `0.54 × 720 / H` (the shipped clips' scale) is the maximum. It is only reduced when the
     movement's union bounding box would not fit the 400×400 canvas at normal framing: taller than 386 px
     (`fit_height`) or wider than the canvas minus 4 px side margins (`fit_width`). x comes from
     `build_clip.body_x` of the first frame and is moved the least needed so the whole bbox is on the canvas
     (`shifted`; a lying pose is not centred on its body point). The result is in the JSON as
     `user_import.centering` (`fixed`/`fit_height`/`fit_width`/`shifted`), `scale` and `scale_max`. Standing
     poses keep 0.54; a full-length lying pose comes out at about 0.31, so she looks smaller in it.
   * **Zoom aim:** the slow cleavage zoom has keyframes at 15/40/60/85 % of the movement (smoothstep ease-in,
     hold, ease-out), peaks at `--zmax`, and brings the chest toward (200,195). The chest is found on the matte
     (median over up to 5 frames of the movement): the mask is opened by 3 % of the figure height to drop thin
     limbs and hair; for an upright figure it is the widest torso row 18–45 % down, at the body point
     (`upright_widest_band`); for a lying figure (wider than 1.25× its height) the head end is the outer quarter
     that reaches highest and the chest is 30 % of the width in from it, centred on the body there
     (`lying_head_left`/`lying_head_right`). The rule and the source point are in `cleavage_zoom.focus_rule` /
     `focus_src_xy`. Movements under 12 frames are not zoomed.
5. **clean**: `tools/matte-clean/matte_clean.clean_cell` (v6), then `cellfix.post` (in-memory port of the
   seasonal holefill → edgefix → **pocketfill** → holefill). **holefill is the last image step.** Rows below 392
   are zeroed. *pocketfill* (after the seasonal `holefill2.py`) compares with the cell **before** matte-clean:
   a transparent hole enclosed by the figure (≤ 400 px, ≥ 3 px inside) that was ≥ 90 % solid there was punched
   by the clean pass (a highlight, a belly button, a gloss spot) and gets the raw pixels back at alpha 255.
   Real gaps are left alone: holes that were already clear before cleaning (arm/body, hair/neck), holes ringed
   mostly by blonde hair, and holes whose raw colour is backdrop white unless they sit in bright skin (ring mean
   min ≥ 170, the same highlight rule as the white-edge QA). Counts are in `user_import.stats`
   (`pocketfill_px`, `pocket_skipped_px`).
6. **encode**: sheet, JSON and mp4 with pack_mp4's settings (libx264 veryslow, tune animation, crf 16,
   high@4.0, full-range bt709, no B-frames, faststart). The mp4 is decoded back and checked.
7. **qa**: `qa_all.py --white-edge --mp4 <candidate>` and `tools/video-pipeline/fringeqa.py` (FQA_ALL=1, output
   in `<candidate>/qa`).

### RAM

The importer holds one frame at a time, plus one movement's metadata and a few canvases. ffmpeg runs with one
thread and one filter thread. RSS is checked every frame (`/proc/self/status` on Linux,
`GetProcessMemoryInfo` on Windows), and a background thread samples the whole process tree (importer + ffmpeg +
QA children) every 0.25 s. If the importer's RSS goes over `--max-ram-mb − 150`, or the tree goes over
`--max-ram-mb`, isnet downgrades to the key and,
if that is still over, the run ends with `error: memory cap exceeded`. A cap smaller than the importer's start-up RSS + 150 MB (about 250 MB) is refused at start. The measured
peak in key mode is about 160 MB. isnet needs the cap to be ≥ 1100 MB to be picked by `auto`.

### Model

The isnet-anime model (176 MB, sha256 `f15622d8…1a6e99`) is **not** in the repo and is never fetched. The
installers bundle it: Arch `lillith-bin` puts it in `/opt/lillith/models/rembg/`, the Windows setup in
`<install folder>\models\rembg\`. `tools/video-pipeline/matte_model.py` is the one place that resolves it
(paths.py, matte_v2, key_white, build_clip, new_pose and user_import all use it) and sets `U2NET_HOME` to that
folder for any child process. Lookup order, first file with the exact size (176,069,933 bytes) wins, so a
half-finished copy is never used:

1. `--model <file>` (or a folder holding `isnet-anime.onnx`)
2. `$U2NET_HOME/isnet-anime.onnx` (`%U2NET_HOME%` on Windows)
3. the installer's copy, `<install>/models/rembg/isnet-anime.onnx`, found relative to the script
   (`tools/video-pipeline/../../models/rembg`)
4. `/opt/lillith/models/rembg/isnet-anime.onnx` (Linux only)
5. `<home>/.u2net/isnet-anime.onnx` (rembg's own folder)
6. `<home>/.rembg/models/isnet-anime/isnet-anime.onnx`, where home is Python's `Path.home()`: `~` on
   Linux/macOS, `%USERPROFILE%` on Windows (e.g. `C:\Users\Ko\.rembg\models\isnet-anime\isnet-anime.onnx`)

rembg is no longer needed: the mask is computed with onnxruntime directly, the same way rembg's isnet-anime
session does it. `python3 tools/video-pipeline/matte_model.py [--json] [--verify]` prints what is missing
(packages or model) and how to fix it; `user_import.py --preflight [--matte M] [--model F]` prints the same as
one JSON line, and `build_clip.py`, `matte_v2.py` and `new_pose.py` exit 2 with that message instead of a
traceback.

With `--matte isnet` and no file found, the run fails with an `error` that lists every path it tried. With
`auto`, it falls back to the backdrop key (plain or solid backdrops only) and the log line says which paths were
tried.

### Known limits (v1)

* Without the isnet model (a source checkout, or a damaged install), busy backgrounds give poor mattes. The installers ship it.
* isnet is slow on a laptop i5 (about 1–3 s per frame at 720 px), so a 30 s video takes minutes. Progress keeps
  moving, and `--max-candidates` bounds the work.
* There is no temporal median smoothing of the matte (skipped to keep memory flat), so mattes may flicker
  slightly more than on hand-made clips.
* The zoom aim is a shape heuristic (widest torso band upright, 30 % in from the head end lying), not a
  body-part detector. Unusual poses (handstand, curled up, two people) can aim off the chest.
* One scale per movement, fitted to that movement: a wide (lying) movement is smaller on screen than a standing
  one, so switching between them changes her size.
* White-edge QA can flag opaque white **art** as pockets: eye whites and skin highlights next to dark lines
  (tattoo, fishnet) look like enclosed backdrop to `white_edge.pocket_mask` when their ring is not bright. These
  pixels are unchanged from the source (raw alpha 255), so a candidate can show `white_edge: FAIL` with a clean
  matte; check the frames it names before throwing it away.
* Segmentation is motion-based. A long continuous dance becomes several `--max-seconds` pieces cut at the calmest frame.
* Candidates are one-shot (`loop: false`). Loop closing is not attempted.
* Audio is ignored.

### Tests

```
python3 -m pytest -q -p no:cacheprovider tools/video-pipeline/tests
```

The tests synthesise tiny 160×120 videos (a crude figure that waves, holds, bounces, then a hard cut to a
second scene) and run the real importer at `--work-height 160` (a hidden test flag). They cover the JSON schema
and monotonic progress, splitting into ≥ 3 movements, the clip format (800×400 mp4, sheet size, nothing below
the baseline, zoom curve), the RAM cap (too low → error; the child tree's `ru_maxrss` stays under the cap),
the `ts`/`elapsed_s` progress fields, the model lookup order (incl. a Windows home), pocketfill (an enclosed
punched highlight is restored, an open or raw-clear gap is not), the scale fit for a wide figure, the process-tree
RSS sampler (a child holding 220 MB is counted), cancel → exit 130 within 1.5 s with cleanup (new and pre-existing `--out`), missing ffmpeg, `--ffmpeg`/`--ffprobe`
as files and folders under a path with spaces, bad input, and writes confined to `--out` (empty TMPDIR, tool
and input folders unchanged). Runtime is about 1 min on one core. Requires ffmpeg with libx264.
