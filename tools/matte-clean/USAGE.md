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
#    /workspace/verify/nowhite/orig/<worktree>/ (override with MC_BACKUP=...; also recoverable from LFS). Safe to re-run: already-clean clips are skipped.
MC_JOBS=3 python3 tools/matte-clean/matte_clean.py assets/generated/g* --report /workspace/verify/nowhite/report_g.json

# 3. after: must print OK for every clip and exit 0
python3 tools/video-pipeline/qa_all.py --white-edge --mp4 assets/generated/g* | tee /workspace/verify/nowhite/qa_after_g.log

# 4. import check (prebuilt binary, no cargo; ~2 min; --check never writes and fails if manifest.toml
#    is not what the import renders: then run it once without --check to regenerate)
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
2 px. Version 3 despills (recolours from the interior, alpha kept) whatever is still light after that erosion,
so single frames no longer fail by a few pixels. Version 4 adds: (a) enclosed backdrop pockets
(near-white neutral regions >= 12 px inside the body: hair/neck, arm/torso, thigh/tail) go fully clear, flood-limited
to near-white neutral pixels, so helpers no longer hand-fix them; (b) a soft-edge despill: soft edge pixels whose
colour is the nearest interior colour pushed toward white (including the alpha 251-254 band the MP4 path snaps to
opaque) are recoloured to the interior colour with alpha kept. Version 5 adds a pale-rim despill matching the
app-decoded fringe QA (`/workspace/lillith/scratch-fringeqa/fringeqa.py`): pixels in the 3 px edge ring, any alpha,
light and near-neutral (min >= 170, sat <= 40) and >= 32 lighter than the interior are recoloured to the interior.
Version 6 (current) adds the **protect guard** (`protect.py`), fixing a v1-v5 bug where the island / leak passes
deleted detached body parts (tail segments, boot tips, hands, glow and FX blobs) and punched pinholes into white
skin highlights and the shoulder tattoo: every output cell is compared with its original and any >= 20 px body
blob that lost over half its area (or fell apart into specks), any new enclosed hole in non-backdrop pixels, and
any solid non-pale pixel whose alpha was cut deeper than 3 px inside the body is restored from the original
(colour despilled). Enclosed near-white backdrop pockets still go clear; a near-white blob whose 1-3 px ring has a
mean min >= 170 is a skin highlight, not backdrop, and is kept. The report's `protect_bad_frames` must be 0.

### Regression check (run on every cleaned clip before merging)

```
python3 tools/matte-clean/regress.py --base 379900d assets/generated/*/          # check only, exit 1 on failures
python3 tools/matte-clean/regress.py --base 379900d --fix assets/generated/*/    # repair failures in place (+ MP4)
#   --alt-base REV --alt-clips a,b   compare those clips against another original (e.g. re-shot perch clips)
#   --fix-any                        with --fix, also repair damage below the flag threshold
```

Per frame vs the original sheet (`git show BASE:... | git lfs smudge`): (a) the number of >= 20 px opaque blobs
(alpha > 128) must not drop, except where original parts merely merged into one blob (reported as
`comp_merged`), and no original body blob may keep < 50 % of its area; (b) a frame is flagged with > 30 new hole
pixels (alpha < 128 now, opaque before, enclosed by the body, not near-white neutral backdrop) or > 30 deep-loss
pixels. `--fix` restores those pixels from the original, keeps every other fringe fix, refreshes
`bbox_per_frame`, repacks the MP4 and stamps `matte_protect` in the JSON (frame data never changes).
A clip cleaned by an older version is re-cleaned **from its original** (`$MC_BACKUP/<clip>_sheet.png` or the repo's
Git LFS object store, located by the sha256 stamped in the JSON), never on top of the earlier output. If you ran an older version (v1 379900d, v2 11807e1), update your branch to the
latest `fix/no-white-fringe` and re-run the same command (add `--force` only to redo v4 output).

## Options

```
matte_clean.py DIR|GLOB... [--dry-run] [--no-mp4] [--force] [--include-locked] [--events-all]
               [--skip a,b] [--jobs N] [--report out.json]
qa_all.py --white-edge [--mp4] [--max N] DIR|GLOB...
```

`white-edge` check: a light edge pixel has alpha > 0, lies within 2.5 px of a clear pixel, has min(RGB) >= 205 and
max-min <= 20, and is more than 24 levels lighter than the nearest solid interior pixel (so real white art does
not count). A frame fails with more than 6 (`--max`), or with any enclosed backdrop pocket on the sheet (`white_edge.pocket_mask`,
not checked on overlay clips). The soft-edge lift count (`soft`) is reported but not gated (H.264 chroma adds some on
MP4 frames). Overlay clips are checked on the main body only; for `event_*` clips drawn over a `base_clip`, pixels
where the original event differs from its original base frame (icons, panels, FX; dilated 2 px) are excluded, so
intentional white effects never fail.

## Renderer (why the fix is in the assets)

* PNG pages: `crates/lillith-anim/src/render.rs` `pixmap_from_rgba` premultiplies the straight RGBA on load;
  tiny-skia then samples bilinear and composites source-over in premultiplied space. Correct for straight PNGs.
* MP4: `pack_mp4.py` stores straight colour (black where alpha = 0) + alpha as grey; `crates/lillith-anim/src/video.rs`
  `unpack_frame` snaps alpha <= 3 to 0 and >= 251 to 255 and premultiplies. Correct.
* So there is no straight/premultiplied mismatch; the light fringe is white baked into the edge colour of the art.
  Details: /workspace/verify/nowhite/RENDERER.md.
