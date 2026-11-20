# Layered art (pilot)

Add-only sprite layers so the engine's chest and pigtail springs can move
those parts independently instead of squashing the whole frame (PLAN.md
§3.1 design note, line 241; roadmap P2 “Per-layer chest/pigtail
deformation still deferred”).

This first pilot covers the main idle clip `idle_a` (the `idle` alias
points at the same sheet). It is a technical split of the already-shipped
frames: no new poses, her locked base look is unchanged, and her top stays
on in every chest-layer pixel, exactly as in the source.

`feat/layered-jiggle-physics` was fetched read-only at `origin/main`
`1184b90d` and had no unique commits and no layer-input schema yet. This
file is the proposed input for that work (and for the existing
`lillith-core` springs). This branch does not commit to that feature
branch and does not change engine code or `manifest.toml`.

## Layout

```
assets/generated/idle_a/
  idle_a_sheet.png          # unchanged shipped atlas (horizontal 72×400)
  idle_a.json               # clip json; optional "layers" pointer (ignored by today's importer)
  idle_a.layers.json        # this format
  layers/
    base_sheet.png
    chest_sheet.png
    pigtail_l_sheet.png
    pigtail_r_sheet.png
```

Sheets use the same `horizontal_strip` layout, 400×400 cells, 72 frames,
11 fps as `idle_a`. Colour is straight (non-premultiplied) RGBA.

## `idle_a.layers.json`

| Field | Role |
|---|---|
| `schema` | `lillith.layered_art.v1` |
| `z_order` | `[base, chest, pigtail_left, pigtail_right]` — composite with `over()` (top last) |
| `sheets` | paths to the four layer atlases, relative to the clip directory |
| `layers[]` | `id`, `sheet`, `z`, `movable`, `spring`, `spring_sign`, `pivot`, `max_offset_px`, `motion` |
| `frames[]` | per-frame `pivots`, body-motion hints, and the chest-bounce-3 bake residual |

### Spring bindings

These names match `lillith_core::spring::Channel` and
`assets/physics.toml` `[attachments]`:

| Layer | `spring` | `spring_sign` | Pivot | Suggested motion |
|---|---|---|---|---|
| `base` | none | — | — | Not moved |
| `chest` | `chest` | `+1` | `pivots.chest` (bust centre) | Translation; optional squash about the pivot |
| `pigtail_left` | `pigtails` | `−1` | `pivots.pigtail_left` (root at the head / tie) | Translation + small rotation about the root |
| `pigtail_right` | `pigtails` | `+1` | `pivots.pigtail_right` | Same |

A later engine may grow separate `pigtail_left` / `pigtail_right`
channels; the file already has one spring name per layer so that is a
compatible change.

`max_offset_px` is the slack that the base fill was authored for: **10 px
for the chest, 16 px for each pigtail**, plus a ~2 px feather. Soft masks
and the fill behind each layer are there so small spring offsets do not
open a hole onto the backdrop.

### Body-velocity / acceleration hints

Each frame records:

- `torso_anchor` — a midriff point just below the bust (the body, not the
  bouncing chest)
- `torso_delta_px` — wrap-around delta from the previous frame (loop)
- `torso_vel_px_s`, `torso_acc_px_s2` — central differences, **+y down**,
  using the clip `fps` (11)
- `chest_pivot_delta_px` / `chest_pivot_vel_px_s` / `chest_pivot_acc_px_s2`
  — the tracked bust centre, which already includes baked bounce
- `baked_chest_offset_px` — the chest-bounce-3 bake residual from
  `idle_a.json` `chest_bounce_bake`

Drive the chest and pigtail springs from `torso_acc` (and/or
`torso_vel`) so extra bounce ramps up with her body motion.

### Composite rule and exactness

`over(pigtail_right, over(pigtail_left, over(chest, base)))`.

At **zero offsets** this must match the shipped `idle_a` frame within 1
level (it does, on visible pixels; a PNG round-trip may show 2). The base
stores the exact un-`over` remainder in the soft fringe, and an inpainted
fill in the hidden interiors and in the root/torso overlap, chosen only
where that fill still composites within 1 level.

## Baked chest bounce on `idle_a`

`idle_a` already has chest-bounce-3 baked into the pixels
(`chest_bounce_bake`, max ~5.93 px, pigtails/hip/tail were not re-warped
in that bake). Layer pivots move with that bake. Treat engine springs as
*extra* bounce (see `docs/CHEST_BOUNCE.md` `baked_layer` above 100 %
intensity), or mute them at 100 % so the motion is not doubled. A later
rebake could drop the pixel bake and let these layers carry all of it.

`baked_secondary_motion` on the clip is `full_body_bounce`, `hip_sway`,
`pigtails`, `tail`, `chest_follow`.

## What the engine still needs

This branch does **not** change Rust, `manifest.toml`, or
`manifest.json`. A `layers` object on `idle_a.json` is an unused extra
field: `tools/import-manifest` already flattens unknown keys into
`extra`, and `layers` is not a typed field or a `*_point` name, so it is
harmless.

To consume the pilot, the animator / renderer should:

1. If `idle_a.layers.json` (or the clip-json pointer) exists, load the
   four sheets alongside the existing atlas cell.
2. Composite in `z_order` with `over()`.
3. Apply `Physics` / `PhysicsDriver` offsets for `chest` and `pigtails`
   as translations (and a small pigtail rotation) about that frame's
   pivot, scaled by `spring_sign`.
4. Feed `torso_acc` (or `torso_vel`) into those springs so bounce grows
   with body motion.
5. Keep the current whole-sprite squash path as a fallback when a clip
   has no layers.
6. Honour `CHEST_BOUNCE.md` so baked `idle_a` does not double-jiggle at
   100 % intensity.
7. Optionally import `impulse_frames` (still deferred; PLAN.md / importer
   note) as kicks on the named spring.

A preview tool lives at `tools/layered-art/preview_layers.py`. It
recomposites with test offsets, reports zero-offset error and seam-crack
counts, and can write a contact sheet plus an MP4/GIF.

```
python3 tools/layered-art/preview_layers.py \
  --manifest assets/generated/idle_a/idle_a.layers.json \
  --source-sheet assets/generated/idle_a/idle_a_sheet.png \
  --outdir /tmp/layers-preview
```

The split itself was built with `tools/layered-art/build_idle_a_layers.py`
(from the official `idle_a` sheet only; no new Imagine generations).
