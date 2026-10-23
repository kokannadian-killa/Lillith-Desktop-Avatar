# Lillith seductive-clip kit: one new clip, fewest steps

The kit speeds up **generating** new allure clips. App playback speed is not touched.
Everything here sits on top of `tools/video-pipeline/` (build_clip → matte_v2 → finalize → pack).

## Hard limits (every still, every take)
Lillith is clearly an adult (adult face and proportions). Her outfit stays on, intact and opaque, never
see-through. No nudity, no sex acts, no fluids. Within that, push the pin-up appeal: slow sultry eye
contact, bedroom eyes, lip bites and smirks, arched-back chest-forward poses, deep neckline,
and soft natural bounce on every slow move. Reject any take that breaks a limit; don't fix it in post.

## The 5-step workflow
```sh
K=tools/gen-kit            # (= /workspace/lillith/gen_kit)
$K/make_clip.py list                       # 1. pick a recipe
$K/make_clip.py prompts lean_forward_desk  # 2. filled still + video prompts -> gen_work/out/<clip>/prompts.md
#   3. GenerateImage(still prompt, 16:9, reference_image_paths=[BASE]) -> PEAK; fit PEAK to BASE (see Framing)
#   4. GenerateVideo(image_path=BASE, last_frame_path=BASE, keyframes=[{PEAK, 3.0}], duration=6, 720p)
$K/make_clip.py build lean_forward_desk --video ~/take.mp4   # 5. clip + GIFs + automatic checks (exit 0 = pass)
```
Step 5 prints the `new_pose.py` command that ships the clip into `assets/generated/` (copy, register,
MP4 pack, import check). Run it from the repo, then commit `assets/generated/<clip>/` and `manifest.*`.
`--draft` skips matte_v2 for a 30-second look; never ship a draft.

## 1. Character lock
`prompts/character_lock.txt` is the exact look taken from the shipped stills
(`assets/generated_v2/_stills/bold_poses/`, `perch_work/perch_sit/stills/`): honey-blonde twin
pigtails with magenta ribbons, purple-to-blue gradient horns, violet eyes, cyan tribal tattoos, black
spade tail, black latex halter top with a pink heart gem, black mesh fingerless sleeves, magenta
shorts with a black belt, magenta latex thigh-highs with black straps, black lace-up platform boots.
Always pass the **BASE still as the reference image**: the reference does more for consistency than
any text. BASE = frame 1 of a shipped bold take (e.g. `ffmpeg -i assets/source/video/bend_forward_wink/bend_forward_wink_v3_c.mp4 -frames:v 1 base.png`), 1280×720, pure white.

## 2. Framing (chest-forward medium read inside a full-body cell)
The engine shows her full body in a 400×400 cell (baseline y=392, scale 0.54), so "close" means
**pose and angle**, not a crop: lean toward the camera, shoulders forward and slightly together,
back arched, chin down and eyes up. Three-quarter-to-front angle keeps the neckline readable.
- Full body, centred, horns ≥ 20 px from the top, boots at the same y as BASE (≈694 of 720).
- **Fit PEAK to BASE** before the video call: same foot line and centre x, and the person no larger
  than BASE (a lean must be shorter). GenerateImage tends to draw her ~5-10 % bigger; scale it about
  the feet (the showcase used 0.95) and flatten near-white (>247) to pure white. A larger PEAK = zoom creep.
- Nothing touches the frame edges (tail and pigtails included).

## 3. Motion vocabulary that the I2V model does well
Use one or two beats per 6 s take, each eased in and out:
- **weight shift / hip pop** → "soft, weighty settle-bounce as she lands on the hip"
- **lean in toward the viewer** → "chest sways forward and settles with a soft, natural double bounce"
- **shoulder roll** → "each roll travels through her chest with a soft, delayed jiggle and settle"
- **arch + release** (hair lift, stretch) → "drops into a soft, delayed double bounce"
- **slow hip figure-eight** → "every change of direction sends a soft jiggle up through chest and hips"
- secondary: pigtails swing in lazy arcs, ribbons flutter, tail curls slowly.
- face: slow wink, lip bite, smirk, half-lidded look. Keep eye contact the whole take.
Words that hurt: "bounce rapidly", "shake", "dance", "jump", anything with "fast"/"quick", camera words.
Physical contact beats (hand on chest/neckline) need their own pinned keyframe or hands warp.

## 4. Slow pacing
- Take: **6 s at 24 fps** (145 frames), start = end = BASE, PEAK pinned at **3.0 s**.
- Cells: frames 1..143 **step 2 → 72 frames at 12 fps** = real-time 6 s loop (shipped allure clips are
  12 fps, 36 frames from 3 s takes; 6 s gives the slower, sultrier read Ko wants).
- Long holds: a 12-15 s take with two keyframes (peak at ~4 s and ~10 s) → `_long` variant (180 frames).
- Never raise fps or step to "fix" length: the checks fail any clip that plays faster than its source.

## 5. Background for clean removal
Flat pure white #FFFFFF, no floor, **no floor shadow**, no props, no window frame (the perch/ledge is
invisible: describe her elbow/hips resting on "an invisible ledge"), no gradient or vignette, no camera
move. Pure white lets matte_v2 unmix the edge band with no white halo.

## 6. QA checklist (make_clip.py checks the starred items automatically)
- [*] 400×400 RGBA, frame count matches JSON
- [*] no alpha within 3 px of top/left/right edges, nothing below y=392
- [*] no white halo (white edge ratio ≤ 0.3 %), no isolated specks
- [*] fps 8-12, playback ≤ 1.0× source, loop ≥ 3 s, p95 body travel ≤ 6 px/frame
- [*] baseline: lowest pixel at 392 ±1 every frame (seated recipes: only ≤ 392)
- [*] loop seam within 2.5× the median frame step
- [ ] outfit intact and opaque on every frame; adult face; 5 fingers; one tail; horns stay purple-blue
- [ ] eye contact held, peak pose reached near 3 s, motion eased (look at `<clip>_check.gif`:
      gray | black | checker, white halos show up on black)
- optional: paste `prompts/videoreview_qa.txt` into videoReview with the raw take (file under /workspace)

## Failures we already hit (and the fix baked into the kit)
| Problem | Fix |
|---|---|
| white halo / grey fringe on hair and mesh | pure-white backdrop + matte_v2 (default); never ship `--draft` |
| zoom creep at the keyframe | fit PEAK to BASE; finalize_keyframed_loop scale fix |
| rushed motion | 6 s take, one or two beats, "slow, eased" language, checks reject >1.0× |
| outfit drift / see-through | character lock + BASE reference + avoid list; reject the take |
| floor shadow, props | "no floor, no shadow", invisible ledge wording |
| warped hands on body contact | pin a keyframe for the contact pose |
| seated clip snapped to the floor | `seated: true` recipes use `--finalize matte` (no ground snap); add to qa_all NO_SNAP when shipping |
