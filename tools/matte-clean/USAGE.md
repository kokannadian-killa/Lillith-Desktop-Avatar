# matte-clean: removing the white fringe / background leaks from clips

Ko: "remove all the backgrounds and the white while she moves". The white is baked into the clip art:
semi-transparent edge pixels (motion blur, hair, tail, gaps between limbs) whose straight colour still holds the
white keying backdrop. The renderer composites correctly (see "Renderer" below), so the fix is in the assets.

Tools (all Python, no cargo):

| what | path |
|---|---|
| cleaner (in place, idempotent) | `tools/matte-clean/matte_clean.py` |
| clips left alone (audited only) | `tools/matte-clean/locked.txt` |
| QA check `white-edge` | `tools/video-pipeline/qa_all.py --white-edge` (metric: `tools/video-pipeline/white_edge.py`) |
| MP4 repack (called by the cleaner) | `tools/video-pipeline/pack_mp4.py` |

## Lead rules (read first)

1. **Keep all poses.** No clip is removed or renamed. Only in-place re-mattes (or additions). The cleaner never
   changes `frame_count`, `fps`, `frame_durations_ms`, `anchor`, `baseline_y`, `frame_points`, `loop` or the sheet
   size (it asserts this per clip). Outfit and art stay locked, no crossfades.
2. **Base is main `ff1ddb4` (180 clips).** After asset changes run the import check (below) and keep the
   manifest diff additive or in place (normally `manifest.toml` / `manifest.json` do not change at all).
3. **Asset-only helpers skip cargo entirely.** The lead runs the single combined Rust gate.
4. **Sheets and MP4s go through Git LFS as real objects**, never broken pointers (checks below).
5. **Commit on your own local branch off this commit and report the SHA.** Do not push.

Intentional white in event clips (icons, start-menu panel, incognito fade) is protected automatically: `event_*`
clips with `base_clip`/`base_frames` only get the cleaned base body where they still equal their base frame;
events without a base are skipped (`--events-all` overrides). Use `--skip a,b` for anything else to leave alone.

## Helper recipe for a letter range (example: `g*`)

```bash
cd /workspace/lillith/wt-nowhite            # or your own worktree, see step 0
# 0. own branch + worktree off the tools commit (local branch fix/no-white-fringe, SHA in the lead message; replace <me>)
git -C /workspace/lillith/repo worktree add -b fix/no-white-fringe-<me> /workspace/lillith/wt-nowhite-<me> fix/no-white-fringe
cd /workspace/lillith/wt-nowhite-<me> && git lfs install --local && git lfs pull --include "assets/generated/g*"

# 1. before: audit (writes nothing)
python3 tools/video-pipeline/qa_all.py --white-edge --mp4 assets/generated/g* | tee /workspace/verify/nowhite/qa_before_g.log

# 2. clean in place: sheet re-matte + bbox refresh + MP4 repack (same crf/bleed), originals backed up once to
#    /workspace/verify/nowhite/orig/ (override with MC_BACKUP=...). Safe to re-run: already-clean clips are skipped.
MC_JOBS=3 python3 tools/matte-clean/matte_clean.py assets/generated/g* --report /workspace/verify/nowhite/report_g.json

# 3. after: must print OK for every clip and exit 0
python3 tools/video-pipeline/qa_all.py --white-edge --mp4 assets/generated/g* | tee /workspace/verify/nowhite/qa_after_g.log

# 4. import check (prebuilt binary, no cargo; ~2 min, rewrites manifest.toml which should come out unchanged)
/workspace/lillith/bin/import-manifest --input assets/generated/manifest.json --tag canonical --check
git diff --stat -- assets/generated/manifest.toml assets/generated/manifest.json   # expect nothing

# 5. LFS sanity: the STAGED blob of every changed .png/.mp4 must be an LFS pointer (the file on disk stays real)
git add assets/generated/g*
git lfs status                      # changed sheets/MP4s listed under "Objects to be committed" as LFS
git diff --cached --name-only | grep -E '\.(png|mp4)$' | while read f; do
  git cat-file -p ":$f" | head -c 40 | grep -q '^version https://git-lfs' || echo "NOT LFS: $f"; done   # expect no output
git lfs fsck --pointers             # expect "Git LFS fsck OK"

# 6. commit (only assets/generated/<range>/... : <clip>_sheet.png, <clip>.mp4, <clip>.json)
git commit -m "assets: matte-clean white edge fringe, clips g* (tools/matte-clean)"
git rev-parse --short HEAD          # report this SHA + the TOTAL lines from steps 2 and 3
```

What changes per clip: `<clip>_sheet.png` (re-matted), `<clip>.mp4` (repacked from the sheet), `<clip>.json`
(`bbox_per_frame`, the `video` block from pack_mp4, and a `matte_clean` stamp with the sheet sha256). Nothing else.

Locked clips (`locked.txt`: cleavage_beckon, chest_press_tease, arch_trace, blend_drag_held_to_perch_sit,
perch_sit) are being edited on the perch-poses branch. They are audited but not written. After that branch lands,
re-run on top of its SHA with `--include-locked`:

```bash
python3 tools/matte-clean/matte_clean.py --include-locked assets/generated/{cleavage_beckon,chest_press_tease,arch_trace,blend_drag_held_to_perch_sit,perch_sit}
```

Raw takes in `assets/source/video/` are pipeline inputs only (read by `tools/video-pipeline/ingest.sh` /
`build_clip.py` / `build_idle_v2.py` via `LILLITH_SRC_VIDEO`); the engine never loads them (it plays
`assets/generated/<clip>/<clip>.mp4` or the sheet). Do not modify them.

## Tool versions / re-runs

`matte_clean` stamps `version` in each clip JSON. Version 2 added the opaque-gap pass (white backdrop seen between
arm and waist / legs / tail with alpha left at 255, which the v1 metric could not see) and caps the edge erosion at
2 px. A clip cleaned by an older version is re-cleaned **from its original** (`$MC_BACKUP/<clip>_sheet.png`,
checked by sha256), never on top of the earlier output. If you ran v1 (commit 379900d), update your branch to the
latest `fix/no-white-fringe` and re-run the same command.

## Options

```
matte_clean.py DIR|GLOB... [--dry-run] [--no-mp4] [--force] [--include-locked] [--events-all]
               [--skip a,b] [--jobs N] [--report out.json]
qa_all.py --white-edge [--mp4] [--max N] DIR|GLOB...
```

`white-edge` check: a light edge pixel has alpha > 0, lies within 2.5 px of a clear pixel, has min(RGB) >= 205 and
max-min <= 20, and is more than 24 levels lighter than the nearest solid interior pixel (so real white art does
not count). A frame fails with more than 6 (`--max`). Event/overlay clips are checked on the main body only.

## Renderer (why the fix is in the assets)

* PNG pages: `crates/lillith-anim/src/render.rs` `pixmap_from_rgba` premultiplies the straight RGBA on load;
  tiny-skia then samples bilinear and composites source-over in premultiplied space. Correct for straight PNGs.
* MP4: `pack_mp4.py` stores straight colour (black where alpha = 0) + alpha as grey; `crates/lillith-anim/src/video.rs`
  `unpack_frame` snaps alpha <= 3 to 0 and >= 251 to 255 and premultiplies. Correct.
* So there is no straight/premultiplied mismatch; the light fringe is white baked into the edge colour of the art.
  Details: /workspace/verify/nowhite/RENDERER.md.
