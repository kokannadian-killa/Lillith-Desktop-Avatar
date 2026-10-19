# assets/

| Path | Contents | In git? |
|---|---|---|
| `rules.toml` | Event → pose/gesture/line/mood/SFX rules and the key map (loaded by `lillith-core::rules`) | yes |
| `source/sheets/` | 6 alpha sprite sheets (1792×1008, 3×10), left-run 4×3 sheet, alt-style action sheet | **not yet** (Git LFS, see `.gitattributes`) |
| `source/poses/` | 52 system-event pose stills (`NN_name.png/jpg`, names from `lillith_pose_map.txt`) | **not yet** |
| `source/reference/` | labelled pose sheets 1–3, full-body ref, physics keyframe refs A–D | **not yet** |
| `source/frames_640x960/` | 80 black-background video frames | **not yet** |
| `atlas/` | generated atlases + `manifest.toml` written by `tools/slice-sheets` | no (generated, git-ignored) |
| `generated/` (box only for now) | canonical full-body set from the image pipeline: 400×400 horizontal-strip sheets + `manifest.json`; convert with `tools/import-manifest` | **not yet** |

The curated originals currently live outside the repo on the build box (`/workspace/lillith/assets`, ~43 MB). They will be committed through Git LFS once the canonical art style is chosen (PLAN.md §6 Q1). Run `git lfs install` before adding them.

## Importing a generated set

```sh
cargo run -p import-manifest -- --input /path/to/generated/manifest.json --tag canonical --check
```

Writes `manifest.toml` next to `manifest.json` (pages stay relative to it): frame `i` at `x = i*400`, feet anchor `[body_anchor_x or 200, baseline_y]`, `sheet` → `page`, `loop` → `mode`, and `frame_durations_ms` / `baseline_y` / `scale` carried through. A phase-dependent scale object becomes `scale:<phase>=<v>` tags. `--check` re-loads it through `Manifest::load` + `load_page` and bounds-checks every frame.
