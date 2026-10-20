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
6. **Import:** `cargo run -p import-manifest -- --input assets/generated/manifest.json --tag canonical --check`
   writes `assets/generated/manifest.toml`, then loads it and bounds-checks every page and frame.
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
- Engine support: only `variant_of:<idle base>` clips are cycled (Animator, `IdleConfig::variant_hold_s`, 30 s by default).
  There is no walk or sit variant picker yet, so those tags are inert until one is added.
- The idle gesture variants are ~4 s loops. With the 30 s hold, one gesture repeats several times before the next pick.
  A shorter per-variant hold, or playing them as one-shots, would suit them better.

## Known caveats

- `twirl` shows a brief back view mid-spin, which is natural for the move.
- `pose_clutch_head` trips the horn-hue QA check because of its baked dizzy swirl. This is expected.
- The crop corners of `event_screenshot` reach y=395 by design.
