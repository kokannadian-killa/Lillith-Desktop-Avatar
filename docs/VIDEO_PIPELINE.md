# Video pipeline (generated set v2)

The canonical art in `assets/generated/` is cut from short image-to-video takes (Grok Imagine I2V)
instead of hand-drawn stop-motion frames. Body bounce and hair/tail follow-through are baked into the
pixels, so every clip is tagged `baked_motion` and the engine damps its own chest/pigtail springs and
breathing scale for them (`AnimatorConfig::baked_motion_gain`, default 0.2).

Content rule for every take: clothed, non-explicit and playful. Takes with sexualized framing are rejected
and not committed. Examples: a deep bow, or a turn that shows her back.

## Steps

1. **Generate.** Each clip uses a 720p I2V take on a white background. The first frame, and usually the
   last too, is a canonical still, so loops close and clips start and end on the idle pose. Keyframes pin
   poses mid-clip where needed. Blends (`blend_a_to_b`) are short transition videos from the end pose of
   A to the start pose of B. Raw takes are kept in `assets/source/video/<clip>/<clip>_v3_<take>.mp4`.
2. **Ingest** with `ingest.sh <clip> <take…>`. It copies the take into the source tree, extracts frames,
   writes a contact sheet and lists loop candidates (`find_loops.py`, frame-difference based).
3. **Build a clip** with `build_clip.py --clip … --video … --start N --stop M --step 2 --fps 12
   --anchor-x X --scale S --mode loop|once --center fixed|body|body_trend [--mirror-to …]`. This step:
   - keys the white background with a flood fill combined with an `isnet-anime` (rembg) matte
     (`key_white.py`);
   - locks the horn hue to the source still (`horn_fix.py`);
   - places each frame in a 400×400 RGBA cell with the feet on baseline y=392 and anchor `[200,392]`;
   - writes `<clip>_sheet.png` (horizontal strip), `frames/` and `<clip>.json` (timing, points, source
     frame range).
4. **Clip-specific passes:**
   - `build_events_v3.py`: `event_*` clips. It puts the original code-drawn icons on the v2 body and
     tracks the fingertip or head crown frame by frame.
   - `build_transition.py`: video blends, registered in scale and offset to both end clips.
     All 12 blends now use transition videos. `build_blends.py` is the optical-flow fallback and no
     blend uses it any more.
   - `beam_overlay.py`: redraws the beam that the keyer removes.
   - `allure_fx_v2.py`: hearts and sparkles that track the hand or head, used on `blow_kiss`,
     `heart_hands`, `peace_sign_pose` and `hip_pop_wink`.
   - `build_idle_v2.py`: the `idle_a` and `idle_b` variants.
5. **QA and manifest:**
   - `qa_all.py [--fix]` checks the baseline, alpha fringes, horn hue, frame size and endpoint
     continuity.
   - `make_manifest_v2.py` writes the manifest JSON, including the tags `baked_motion` and
     `variant_of:idle`, and the `blend` and `frame_points` fields.
   - `review_sheet.py`, `blend_sheet.py`, `event_strip.py` and `all_sheet.py` make contact sheets for
     review.
6. **Import:** `cargo run -p import-manifest -- --input assets/generated/manifest.json --tag canonical`
   writes `assets/generated/manifest.toml` (deterministic; untouched when nothing changed). The same
   command with `--check` never writes: it fails if the file differs from what the import renders, then
   loads it and bounds-checks every page and frame.
   `cargo test -p lillith-anim --test generated_set` plays and renders every clip.

## Layout and environment

- Scripts live in `tools/video-pipeline/` (Python 3 with numpy, Pillow, scipy, opencv-python-headless
  and rembg; ffmpeg is also required). They build into a working directory outside the repo, then the
  result is copied to `assets/generated/`. Path overrides are documented in `paths.py`:
  - `LILLITH_V2`: working output, default `/workspace/lillith/assets/generated_v2`.
  - `LILLITH_V1`: the old stop-motion set and its `_tools`, default `…/generated_v1_backup`. It
    supplies the event icon painters (`fx.py`, `icons.py`, `build_events*.py`), old timing and points
    JSON, and the allure key art.
  - `LILLITH_SRC_VIDEO`, `LILLITH_RAW_IN` (for `ingest.sh`) and `U2NET_HOME`.
- The v1 stop-motion set is not in git. It is preserved on the build box at
  `/workspace/lillith/assets/generated_v1_backup/` (about 657 MB). The v2 set covers all 95 v1 clip
  names, adds `idle_a` and `idle_b`, and keeps `idle` as an alias of `idle_a`.
- Batch 12 (115 clips in total) adds video versions of the 18 clean clips that were only in the v1
  `manifest.json`:
  - the ten `idle_flirt_*` mannerisms, each a whole take from the idle still back to the idle still;
  - `hair_twirl` (a loop cut of the pigtail_twirl take), `sway_dance`, `cheeky_tongue_wink`,
    `victory_jump` and `over_shoulder_look`;
  - three aliases of near-duplicates (`alias_of`, same sheet): `finger_wag_tease` → `lean_tease`,
    `hands_behind_head_pose` → `idle_flirt_stretch`, `bounce_giggle` → `idle_flirt_shy_giggle`.

  `make_manifest_v2.py` carries the v1 `allure` / `idle_variant` flags, which import-manifest turns into
  tags. The app's pose cycle picks these clips by name, so `idle_flirt_*` are not `variant_of:idle`.
  The 84 v1 `*_jiggle` clips are not rebuilt: each is a code-jiggled copy of a base clip, and v2 bakes
  secondary motion into every clip.
- The seven own-pose event bases (`pose_clutch_head`, `pose_hacking`, `pose_pout`, `pose_present`,
  `pose_shush`, `pose_vsign`, `pose_wait_palm`) are also built as standalone clips in `assets/generated/`
  (whole 49-frame take, f1-47 step 2, 24 frames @12 fps, scale 0.53, fixed centre, loop), adding 7 clips (136 in
  total after batch 13). Their entries were appended to the committed `manifest.json` rather than regenerating it, because
  the committed file carries walk root-motion / gait fields that `make_manifest_v2.py` does not emit.
- The runtime looks for `$LILLITH_MANIFEST`, then `./assets/generated/manifest.toml` (relative to the
  working directory), then `$XDG_DATA_HOME/lillith/generated/manifest.toml`.

## Variants (batch 13)

- `make_manifest_v2.py` has a `VARIANT_OF` map that tags alternative takes with `variant_of:<base>`:
  - `walk_{sassy,skip,saunter}_{right,left}` -> `walk_right` / `walk_left`;
  - `sit_{bored_chin,cross_legged_bob,heel_kick,stretch_yawn}` -> `sit_swing`;
  - `idle_{bored_sigh,nail_check,tail_fidget,yawn_stretch}` -> `idle` (these also carry `idle_variant`).
- Each take starts and ends on the base clip's source still. Placement matches the base:
  - walks: scale 0.54, body_trend centring, per-frame snap;
  - sits: scale 0.5, fixed, aligned to sit_swing, same seat_y / seat_x_range;
  - idles: the idle_a still.
- The walk variants carry `engine_springs` (gait disabled) like the base walks. They have no `root_motion` or `contact_frames` yet.
- Engine support: every `variant_of:<base>` tag is read (Animator `variant_map`; bases idle, walk_right, walk_left, sit_swing).
  With `IdleConfig::variant_play_once` (default true) the clean base loops for `variant_hold_s` (30 s), then one random variant plays once at a base loop end and hands back to the base. A mirrored walk (walk_right drawn flipped) draws walk_right's variants flipped.
  While a walk variant is drawn the window moves at the fixed walk speed (no `root_motion` on those clips yet).
- `variant_play_once = false` keeps the legacy behaviour: idle only, one ~4 s idle variant loops for the whole 30 s hold, and the clean idle is not drawn.
  A shorter per-variant hold, or playing them as one-shots, would suit them better.

## Known caveats

- `twirl` shows a brief back view mid-spin, which is natural for the move.
- `pose_clutch_head` trips the horn-hue QA check because of its baked dizzy swirl. This is expected.
- The crop corners of `event_screenshot` reach y=395 by design.

## MP4 pose source

Every clip in `assets/generated/` has a `<clip>/<clip>.mp4` next to its sheet. The manifest entry's
`video` field points at it, and the engine plays the MP4. The JSON keeps all timing: `fps`,
`loop`/`once` and `frame_durations_ms` holds. The MP4 holds exactly one frame per sheet cell, so the
PNG sheet stays the fallback and the bounds-check reference.

- **Format** (`alpha_side_by_side`): H.264 High, 4:2:0, full range BT.709, `2W x H` (800×400).
  - The left half is straight (un-premultiplied) colour, black where alpha = 0.
  - The right half is alpha as grey.
  - Encoding uses crf 16, `-preset veryslow -tune animation`, no B-frames and one GOP per clip, with
    `+faststart` and bitexact output.
  - The decoder takes alpha from the green channel, snaps ≤3 → 0 and ≥251 → 255, then premultiplies.
  - Measured over 141 clips, the average edge error is 2.35/255 (worst 3.57). Halo is
    +0.06 on a dark backdrop and −0.18 on a light one, i.e. no visible fringe. Alpha differs from
    the PNG by 0.08/255 on average.
- **Engine** (`lillith-anim::video`, used by `render::PageCache`):
  - ffmpeg runs as a local child process (`-protocol_whitelist file,pipe`; the MP4 bytes go in
    through stdin). A standby process is pre-spawned so a clip change doesn't wait for ffmpeg startup.
  - Frames stream in as they decode. The overlay's `Stage` never blocks for more than 30 ms: it keeps
    the previous frame until the next one is ready, and it prefetches a blend's target clip.
  - Decoded clips sit in an LRU cache with a RAM budget: `LILLITH_VIDEO_CACHE_MB`, default 192 MB.
    One 400×400 frame is 625 KiB, so a 36-frame pose is about 22 MB.
  - `LILLITH_VIDEO=0` (or `off`/`png`) forces the PNG sheets. `LILLITH_FFMPEG` sets the binary path.
  - If ffmpeg is missing, or a clip fails to decode, the engine logs it once and falls back to the
    sheet for that clip.
  - `log_stats` on exit reports decodes, resident clips/MB, evictions, failures and renders that
    waited on a frame.
- **Import check:** `import-manifest --check` probes every MP4 with ffprobe (h264, `yuv(j)420p`,
  `2W x H`, frame count = clip frames). It then decodes the MP4 the way the engine does and compares
  the alpha of every frame with the PNG cell (mean absolute difference ≤ 3/255), which catches stale,
  misordered or alpha-less files. Use `--no-video-check` on a box without ffmpeg.
- **Packing:**
  - `tools/video-pipeline/pack_mp4.py <clip_dir>... [--crf 16] [--manifest assets/generated/manifest.json]`
    re-encodes from the sheet and prints the quality metrics.
  - It also writes a `video` block (metrics, size) into `<clip>.json` and sets the entry's `video`.
  - Unchanged clips are skipped unless you pass `--force`.
- **Repo size:** the per-frame `frames/` PNGs were byte-for-byte copies of the sheet cells (verified
  on all 2690 frames) and nothing read them, so they are no longer committed. They stay in the build
  dir. `assets/generated/` went from 381 MiB (sheets 185 + frames 195) to about 246 MiB
  (sheets 205 incl. the 8 new poses + MP4s 33.5). The eight `_long` clips add about 111 MiB, for about
  348 MiB in total (sheets 302 + MP4s 46.3). Dropping the sheets as well would save another
  205 MiB, but it would lose the ffmpeg-free fallback and the `--check` reference, so they stay.
- **Measured** (8-core box, release build):
  - `cargo run --release -p lillith-anim --example video_bench` over 143 clip videos (3060 frames):
    - PNG sheet load+crop per clip: median 32 ms, p95 144 ms.
    - MP4 first frame on clip change: median 65 ms, p95 110 ms.
    - Whole clip: median 114 ms, p95 239 ms (5.9 ms/frame).
    - Playing every clip under the 192 MB budget leaves 18 clips / 191 MiB resident with 0 failures.
  - Overlay on Xvfb + picom at scale 1.0 (overlay + its ffmpeg child):
    - About 4% of one core idle and about 9% while a pose plays.
    - Idle RSS is 152 MiB with PNG and 154 MiB with MP4, plus about 46 MiB for the standby ffmpeg.
      After about 5 minutes of poses, RSS is about 260 MiB with a full 187 MiB video cache.
  - On-screen frame timing (x11grab at 30 fps, matched to decoded frames) follows the JSON durations
    within the 33 ms capture step, including holds, and is the same as the PNG path.
- **New pose in one command:** `tools/video-pipeline/new_pose.py`. See `tools/video-pipeline/README_MP4.md`.
- **Bold allure poses:** 8 poses come from `generated_v2` (approved), each 36 frames, 12 fps, loop, and
  are listed in `catalog.rs` *Flirty / Allure*: `body_roll_slow`, `arch_back_over_shoulder`,
  `kneel_hands_on_thighs`, `wall_lean_invisible`, `blow_kiss_hip_pop`, `hair_lift_stretch`,
  `slow_turn_spin_tease` and `bend_forward_wink`. They ship as sheet + JSON + MP4, without a preview GIF or a
  `frames/` dir.
- **`*_long` variants:**
  - Each `_long` variant is a 15 s take with a ~10 s living peak hold: 180 frames at 12 fps, loop,
    with chest, pigtail and hip jiggle baked into the pixels. The importer tags these
    `disable_springs`, so the engine springs stay off.
  - All eight are registered and appear in *Flirty / Allure*: `body_roll_slow_long`,
    `arch_back_over_shoulder_long`, `kneel_hands_on_thighs_long`, `wall_lean_invisible_long`,
    `blow_kiss_hip_pop_long`, `hair_lift_stretch_long`, `slow_turn_spin_tease_long` and
    `bend_forward_wink_long`. They are tagged `baked_motion` + `jiggle_exaggerated` (exaggerated cartoon
    chest bounce with overshoot, baked in post by `generated_v2/_tools/bake_jiggle_v2.py`). Each MP4 is
    about 1.3–1.9 MiB and each sheet 11–14 MiB.
  - Decoded, a 180-frame clip takes about 110 MB of the 192 MB video cache. Two long clips playing
    back to back evict each other. Each re-decode streams in the background, about 1 s for 180
    frames with the first frame after about 65 ms, so playback does not stall.
  - New `_long` takes from `generated_v2` are registered once their matte, finalize and jiggle steps
    are done: `new_pose.py <clip>_long --from copy --allure --tag baked_motion`.

## Dominant-persona set (batch 16)

Image-to-video from the v3 design still (`_stills/v3_design/idle_v3.png`, white BG, first frame =
last frame), then the standard `new_pose.py ... --allure --tag dom --to pack` flow. Every clip is
tagged `baked_motion`, `allure`, `dom` and uses the canonical outfit. The dominance is carried by
expression, gaze, gestures and body language toward the viewer. There is no added skin and no
chest-, crotch- or butt-focused framing. Takes that zoomed in, bent toward the camera or glitched
were retaken (see `generated_v2/STATUS.md`).

- Gestures: `dom_beckon_finger`, `dom_point_down`, `dom_arms_crossed_demand`, `smug_smirk`,
  `hungry_lip_lick`, `predator_lean_in`, `tail_wrap_pull`, `kiss_then_come_here`,
  `hug_screen_possessive`, `kneel_pout_beg` (6 s), `soul_contract_sign` (6 s, neon-red DemonOS
  contract), and the `seduce_*` set (`seduce_beckon_finger`, `seduce_lip_bite_glance`,
  `seduce_lean_to_screen_whisper`, `seduce_blow_kiss_screen`, `seduce_tap_glass`).
- Locomotion: `stalk_prowl_right` / `stalk_prowl_left` (mirror), tagged
  `variant_of:walk_right` / `variant_of:walk_left`. They are kept out of the pose menu.
- `knife_tap_neon` with `blend_idle_to_knife_tap_neon` / `blend_knife_tap_neon_to_idle`
  (neon blood splat FX, see `tools/video-pipeline/neon_blood_overlay.py`).

_Review note (batch 15):_ `arch_back_over_shoulder`, `bend_forward_wink` and `slow_turn_spin_tease` (+ `_long`) failed
the batch 15 content review (chest/butt-focused framing). They are left unchanged on this branch
at the lead's request, and a separate keep/remove decision is pending.
