# Seasonal outfit clips: working recipe (owner: feat/seasonal-outfits worker, Halloween + Christmas)

Kept updated as I go. Scripts: `/workspace/lillith/seasonal_refs/kit/` (copy them into your own scratch dir and
`sed -i 's#wt-seasonal#<your-wt>#g; s#scratch-seasonal#<your-scratch>#g'`). Style refs: `/workspace/lillith/seasonal_refs/`
(`halloween_outfit_ref.jpg` = approved on-model outfit still; `halloween_*_peak.jpg` = pose stills).

## 0. Setup
- `git -C /workspace/lillith/repo fetch origin && git -C /workspace/lillith/repo worktree add -b <branch> /workspace/lillith/<wt> origin/main`
- `tools/video-pipeline/zoom_tease.py` is on main. fringeqa lives outside the repo: `/workspace/lillith/scratch-fringeqa/fringeqa.py`.
- Disk: stop under 8 GB (`df -BG /`). `diskwatch.sh` pauses only seasonal `zoom_tease.py` jobs (pattern matches clip prefixes) — edit P for your prefixes. `touch <scratch>/STOPWATCH` ends it.

## 1. Stills (GenerateImage, 16:9 -> 1280x720)
1. Outfit ref: reference `assets/generated/reference/lillith_canonical_ko.png`; describe identity explicitly
   (blonde twin-tails + purple ribbons, blue-purple glowing horns, purple eyes, cyan tribal tattoos, EXACTLY ONE black spade tail),
   the outfit (micro bikini / pin-up costume, opaque cups fully covering chest and groin, neon red trims + glowing red circuit runes),
   "zoomed-out full-body, hat/horn tip to boot soles in frame with white margin, plain pure white background, no shadow, no text".
2. Pose peak stills: reference ONLY the approved outfit ref (keeps outfit + face consistent). Same full-body wording.
- Pitfalls: without "zoomed-out ... white margin below the boots" ~half the stills crop at the thighs (unusable: baseline needs feet).
  Watch for a duplicated tail (got 2 tails once) -> reject. Figure ~700 px tall in 720 px matches scale 0.54.

## 2. Video (GenerateVideo I2V, 720p, first = last = peak still)
- 15 s takes often get a mid-clip camera push-in that cuts off the legs -> unusable (check with `vcheck.py take.mp4 strip.png`:
  bbox height must stay ~constant and bottom must stay at the feet; also eyeball the strip).
- Better: 2 x 8 s segments, each first=last=the same still (193 frames each at 24 fps), different action per segment, then chain:
  `ffmpeg -i s1.mp4 -i s2.mp4 -filter_complex "[1:v]trim=start_frame=1,setpts=PTS-STARTPTS[b];[0:v][b]concat=n=2:v=1[v]" -map "[v]" -c:v libx264 -crf 8 -pix_fmt yuv444p chain.mp4`
  (drop s2's first frame: it duplicates s1's last). 385 frames -> zoom_tease `--stop 385` -> 192 output frames = 16.0 s.
- Prompt template: "Static wide shot on a tripod: the camera never moves, never zooms, never pushes in; her entire body from <top> to
  boot soles stays visible in every frame, plain white background. <action: chest caress/squeeze over the top, lip bite, slow lip
  lick, fingertip on lips, themed prop lick, blown kiss, wink, giggle, hip pop>. Bedroom eyes looking into the viewer's eyes.
  <Outfit> stays fully in place and opaque. Ends back in the starting pose."
- Some prompts get blocked by content safety: do NOT reword to sneak past; tone the action down or keep another take.
- "Oops" moments (strap slips, she catches it): every frame of that segment must be checked; reject the take if anything shows.
- Long tool calls get interrupted by steering; the mp4 still lands in the bot's attachments dir — check `ls -lt` there and match the
  first frame against your stills (mean abs diff ~1 = match).
- Raw takes go to `/workspace/lillith/assets/source/video/<clip>/<clip>_<seg>.mp4` (outside git).

## 3. Clip build: zoom_tease.py (matte once at full 720p, eased baked zoom, premultiplied INTER_AREA downscale)
`zt.sh <clip> <video> [args]` (memory gate >=2500 MB avail, log_<clip>.txt, appends "<clip> rc=N" to queue.log). Run with nohup.
Standing: `zt.sh halloween_hat_tip_wink <chain.mp4> --stop 385 --zmax 2.3 --focus-src X,Y --zoom 14,40,58,84,104,130,148,174 --snap --facing front`
- `--zoom` = groups of 4 output-frame keyframes (ease-in start, hold start, hold end, ease-out end) per cycle; 2 cycles here.
  First/last frames must be z=1 (normal framing: 400x400, anchor (200,392), rows >= 393 clear).
- `--focus-src` = her chest centre in SOURCE px (read it off the still). `--snap` for planted feet; omit for seated/floating poses
  (seated: lowest opaque px is the baseline). Lying poses: `--scale 0.38 --center-bbox`.
- Output: `$LILLITH_V2/<clip>/` = /workspace/lillith/assets/generated_v2/<clip>/. Cache: /tmp/zoom_tease_cache (delete after).
- At 2.3x the 720p source is slightly upsampled (~0.8 src px per canvas px); 1.85 at normal framing. Say so in reports.

## 4. Into the repo + clean + QA (`proc.sh <clip>`)
1. `python3 tools/video-pipeline/new_pose.py <clip> --from copy --to pack --allure --repo $WT` (copies v2 -> assets/generated/<clip>/,
   packs the 800x400 colour|alpha MP4, adds the manifest.json entry). NOTE: it rewrites manifest.json — never edit manifest.json
   while a new_pose is running.
2. `CC_REPO=$WT MC_BACKUP=<scratch>/mc_orig MC_JOBS=1 python3 tools/matte-clean/matte_clean.py assets/generated/<clip>`
3. `fixall.py <clip>`: drops detached specks <=40 px, zeros fringeqa white mask and white_edge light-edge mask, repacks, repeats
   on MP4-decoded frames, repacks.
4. `qa.sh <clip> mc`: matte-clean again, then `qa_all.py --white-edge --mp4`, fringeqa summary (FQA_ALL=1), `popcheck.py`.
- If fringeqa FAILs or white-edge FAILs: `fixloop.sh <clip>` (fixall + qa mc, up to 3 rounds; usually passes in 1-2).
- Pass bar: qa_all OK, fringeqa WARN/PASS (0 FAIL), normal-framing centroid jump <=8 px, floating specks 0, rows>=393 = 0.
  All-frames centroid is larger during the zoom (expected). fringeqa treats the row-392 crop line as an edge; bottom-crop and
  side-crop cut edges are opaque.

## 5. Tags (manifest.json entry + per-clip json)
- import-manifest passes free-form `tags` through to manifest.toml. Seasonal clips get:
  `tags += ["allure","baked_motion","cleavage_zoom","seasonal","season:<id>","outfit:<id>"]` and json fields
  `"season": "<id>"`, `"outfit": "<id>"` (extra json fields are ignored by import-manifest, harmless).
- Season ids: halloween, xmas, valentines, spring, summer, newyear. Clip names `<id>_<pose>`.
- seasons.toml / outfit menu entries: owned by feat/seasonal-outfits (all 6 seasons have slots). Your thumbnail goes to
  `assets/generated/outfits/<id>.png` (128x128 RGBA, transparent, normal-framing head-to-chest crop of a clip frame).

## 6. Commit + push in batches (`BRANCH=<your-branch> EXTRA="assets/generated/outfits/<id>.png" batch2.sh "<clips>" "<msg>"`)
- SET BRANCH (defaults to feat/seasonal-outfits!) and edit WT/target paths in the kit copy. EXTRA = extra paths to git add.
- Builds a filtered manifest (HEAD's clips + the batch) in `assets/generated/.batch_manifest.json`, runs
  `import-manifest --input <that> --output assets/generated/manifest.toml --tag canonical --check`, stages the filtered json into
  the index as manifest.json (working manifest.json untouched, so unfinished clips never get committed by accident), commits,
  checks LFS, `git push origin HEAD:refs/heads/<branch>` (no force, never main).
- Needs the importer binary: build once with your own target dir, gated on <2 other cargo builds and >=8 GB:
  `CARGO_TARGET_DIR=/workspace/lillith/target-<you> CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only
   CARGO_PROFILE_TEST_DEBUG=line-tables-only RUSTFLAGS="--remap-path-prefix=$WT=<wt-name>" cargo build -j 2 -p import-manifest`
  then edit batch2.sh's binary path. Someone may wipe target dirs; rebuild if rc=127. Delete it when done.
- Pitfalls: (a) never hand-filter manifest.json in place (I dropped two entries that way once); (b) a commit whose manifest
  lists a clip whose dir isn't committed is broken — batch2.sh's index method avoids this; (c) check `git lfs ls-files` counts
  2 files per clip (sheet + mp4).

## 7. Sheet + cleanup
- Sheet: `sheet.py` (edit CL) -> /workspace/lillith/chat_shots/seasonal_<id>_sheet.png (peak-zoom frame per clip).
- After each season: delete /tmp/zoom_tease_cache/<clip>*, assets/generated_v2/<clip>, chain/raw temp renders, mc_orig.

## 7b. Helper scripts added (kit/)
- `postwatch.sh`: nohup it; polls queue.log for `<clip> rc=0`, runs proc.sh then fixloop.sh (if FAIL or specks) SEQUENTIALLY
  (new_pose rewrites manifest.json, so never run two in parallel). Logs proc_<clip>.txt / fix_<clip>.txt, post_done.log.
- `tags_seasonal.py <clip>...`: adds tags seasonal, season:<id>, outfit:<id>, allure, baked_motion, cleavage_zoom and json
  fields season/outfit/allure to manifest.json + per-clip json. Run right before batch2.sh.
- `thumb.py <outfit id> <clip> [frame]`: 128x128 RGBA transparent thumbnail -> assets/generated/outfits/<id>.png.
- `sheet.py <season> <clip>...`: f0 + peak-zoom frame per clip -> chat_shots/seasonal_<season>_sheet.png.
- zoom_tease prints `canvas_compliant False` for every zoom clip (zoomed frames overflow the sides by design) — expected.

## 8. Status / learnings log
- Halloween takes: hat 15 s single take pushed in (rejected); hat s2 (8 s) also pushed in -> regenerated with
  "Locked-off ... stays the same small size in frame with white space around her" = stable. Use that wording.
- Cape "swirl" prompt produced a big semi-transparent red magic-ring VFX -> rejected (would matte as junk). Add
  "no visual effects, no magic trails, no particles".
- Oops segment check: crop the chest region of EVERY frame into numbered contact sheets (8x5 tiles of 220x180) and look at
  all of them; mine (cape s2) kept cups covering on all 193 frames.
- zoom_tease matte is CPU-bound: ~15 s/frame when 4+ workers run at once (load ~10 on 8 cores) -> ~45 min per clip. Run at most
  2 of your own at a time.
- Chain of 2x193 frames decodes as 384 frames -> use --stop 384, zoom keys 14,40,58,84,104,130,148,174.
- Christmas: ref + 4 peak stills in seasonal_refs (xmas_*); all 8 segments passed vcheck first try with the
  'Locked-off ... same small size in frame ... no visual effects' wording.
- Catalog: lillith-anim catalog::category_of only knows allure clips by name list; seasonal clips need GUI wiring (docs/SEASONS.md).
- popcheck.py (kit) now counts pieces touching the top/left/right canvas edge separately as "crop-edge body pieces":
  during the zoom the frame edge slices off the hat tip / tail tip, which used to show up as "floating specks" that fixloop
  can't (and mustn't) remove. Use `speckloc.py <clip>` to list remaining specks (frame, size, position, gap) before deciding.
- OOM: a zoom_tease got OOM-killed (rc=137) with ~6 bots' matte jobs running. Run at most 2 of yours; re-run the clip
  (its /tmp/zoom_tease_cache/<clip>_a is reused).

## 9. Holefill (REQUIRED after matte-clean/fixall, before push) — added 14:50
- matte-clean/fixall delete white specular highlights -> enclosed transparent holes inside the body (bikini gloss, hair, cape, boots).
- `python3 kit/holefill.py <clip> 80` (env HF_WT=worktree, HF_REV=review dir; `--dry` to just count) inpaints pure-black opaque px and
  fills enclosed alpha holes (<=80 px, >=5 px inside the filled body) with TELEA + alpha 255, then repacks the mp4.
- Then QA WITHOUT matte-clean: `kit/hfpost.sh <clip>` (= holefill + `qa.sh <clip>`; qa.sh/popcheck honour QA_WT for other worktrees).
- If mp4 white-edge FAILs afterwards (pumpkin: 1 bad frame): `fixall.py <clip>` then holefill again then qa -> passed.
- Verified on the Halloween clips: fills land on gloss/hair/cape/boot only, none in real background gaps.
- Delete each clip's intermediates (generated_v2/<clip>, /tmp/zoom_tease_cache/<clip>_a, mc_orig/<clip>*) as soon as it is pushed.
