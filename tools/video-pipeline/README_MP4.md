# New pose → MP4 in one command

```sh
# venv with numpy, Pillow, scipy, opencv, onnxruntime (+ the isnet-anime model in $U2NET_HOME); ffmpeg with libx264
tools/video-pipeline/new_pose.py <clip> --video ~/Downloads/<clip>.mp4 \
    --start 1 --stop 71 --step 2 --fps 12 --mode loop --scale 0.54 --anchor-x 200 \
    --finalize keyframed --allure
```

This runs ingest → `build_clip.py` → `matte_v2.py` → `finalize_keyframed_loop.py` → copy into
`assets/generated/<clip>/` → register in `manifest.json` → `pack_mp4.py` → `import-manifest --check`.
After that, review `<clip>_preview.gif` and commit `assets/generated/<clip>/` along with `manifest.*`.

- `--from STEP` / `--to STEP` re-run part of the chain. For example, `--from pack` after you hand-fix
  the sheet. The steps are `ingest build matte finalize copy register pack import`.
- `--finalize matte` takes the matte as is (once-clips, no mid keyframe). `--finalize build` skips the
  matte, which gives quick drafts with the old white key.
- `--build-arg=--snap` (repeatable) passes extra options to `build_clip.py`, e.g. `--center body_trend`
  or `--durations-from-old`.
- `--dry-run` prints the commands without running them.
- A pose without `--allure` lands in the menu under its name-based category. Add it to a list in
  `crates/lillith-anim/src/catalog.rs` if the category matters.

## Take template (Grok Imagine I2V)

| Field | Value |
|---|---|
| Input still | canonical idle still (`assets/generated_v2/_stills/`), first frame = last frame for loops |
| Background | plain white, full body in frame, feet visible, no camera move/zoom |
| Length | 3 s at 24 fps (→ 36 frames at step 2 / 12 fps) |
| Content | clothed, non-explicit, playful. Reject back views, deep bows and sexualized framing |
| Name | `<clip>_v3_<take>.mp4`, snake_case clip name |

## Format reference

`<clip>.mp4` is 800×400 H.264 (High, 4:2:0, full-range BT.709, crf 16, no B-frames). The left half is
straight colour and the right half is alpha as grey. Timing comes from the JSON (`fps`, `loop`,
`frame_durations_ms`), never from the MP4. Details and measured quality are in
`docs/VIDEO_PIPELINE.md` → *MP4 pose source*.
