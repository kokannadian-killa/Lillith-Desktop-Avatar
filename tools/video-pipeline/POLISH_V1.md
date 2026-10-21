# polish_v1: clip-definition fixes for the generated_v2 set

`polish_v1.json` lists the fixes. `polish_v1.py` rebuilds only those clips from the shipped generated_v2 definition
(same source take, step, fps, anchor, centering, snap and keying) plus the overrides. Output goes to
`$LILLITH_V2/_polish_v1/<clip>/`. It never writes `generated_v2/<clip>/`, the manifest or `_matte_v2/`.
Each rebuilt json gets a `polish_v1` block with the reason and the old and new values.

    python3 polish_v1.py                 # everything in polish_v1.json
    python3 polish_v1.py idle_flirt_stretch blend_sleep_to_wake [--dry]

Nothing is resampled and fps is unchanged. A loop that gets longer gains one real source frame at its end. A start
trim drops frozen duplicate frames and keeps one, so the eased hold stays.

## What changed
- **Loop ends** (16 clips): the shipped cut stopped 2 source frames (one output frame) before the frame that wraps
  back onto frame 1. The new last frame is chosen so that the next source frame is about equal to the first.
  Clips: idle_flirt_{look_away, stretch, pigtail_twirl, foot_tap, glove_adjust, hands_behind_sway, hip_lean,
  lip_bite, shy_giggle}, idle_bored_sigh, sway_dance, lean_forward, tail_tease, hip_sway, shoulder_roll,
  shoulder_shimmy.
- **Static start trims** (loops): idle_flirt_lip_bite (from source frame 5), idle_flirt_shy_giggle (9),
  idle_bored_sigh (3).
- **Scale**: pose_hacking and pose_present 0.53 -> 0.526. At 0.53 their frame-0 standing height was 373 px against
  the 370 px house height. Every other non-walk clip is within 2 px, or doesn't start in the standing pose.
- **blend_sleep_to_wake**: the transition video has settled on wake[0] by about frame 45, so the old picks f49 and
  f61 were near-duplicate dead frames. It now uses 5 interior frames over f1-f45 (f8, f16, f23, f30, f38), at the
  same count and fps. `build_transition.py` gained `--end=F` and `--out-dir=DIR` for this. Both default to the old
  behaviour.
- `build_clip.py` gained `--ground-from START:STOP`. `polish_v1.py` also registers each re-cut to the shipped clip
  at the integer-pixel level, which corrects a 1 px vertical shift on the 45->47 clips caused by the ground
  percentile. Every frame shared with the shipped clip is now pixel-identical to it.

## Not changed (needs other work)
- Clips whose events or overlays are built on top: beam_cast and spell_orb_cast (3 frozen tail frames;
  overlay/frame_points). If pose_hacking or pose_present are adopted, the events built on them need
  `build_events_v3.py` re-runs.
- Short eased once-clip tails, kept as settle holds: cheeky_tongue_wink, victory_jump.
- idle_flirt_tail_curl: its tail looks static at the threshold, but motion is tail-only and tiny, so it was left
  alone.
