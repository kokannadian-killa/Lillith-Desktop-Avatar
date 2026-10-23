# Chest bounce (Settings ▸ Motion)

Stylized cartoon jiggle for Lillith's chest. She stays fully clothed; this
only tunes secondary-motion springs. Code: `lillith_core::bounce`, the
physics driver (`physics_driver.rs`), and the Settings section
`lillith-settings-gui/src/tabs/bounce.rs`.

## The section

At the top of **Settings ▸ Motion**, in the panel titled `CHEST · BOUNCE [springs.bounce]`:

| Control | Range | Does |
|---|---|---|
| **Preset** | Subtle · Natural · Bouncy · **Extra (sexy)** (default) · *Custom* | Sets both sliders. Shows *Custom* when the sliders don't match a preset. |
| **Intensity** | 0 – 300 % (steps of 5) | Multiplies the chest kick gain. 0 % = no engine chest bounce at all. Above 100 % it also layers a light bounce over baked-jiggle poses (see below). |
| **Softness** | 0 – 100 % (firm ↔ soft) | The spring's character: frequency, damping, how far it lags her body. The readout shows Hz and the number of wobbles. |
| **Reset** | – | Back to Extra (sexy), 100 %. |

**Live apply.** These two keys, and nothing else, are written to
`config.toml` while you drag: about 8 writes a second, plus one when you
release. Each write sends `{"menu_action":"reload_config"}`. The overlay
re-reads the file (`reload_config` → `apply_config` → `Animator::set_springs` →
`PhysicsDriver::set_user_springs`), so she changes as you move the slider.
Nothing restarts. Other staged edits still wait for **Save**.

```toml
[springs.bounce]
intensity = 1.0   # 0.0 … 3.0  (100 % = 1.0)
softness = 0.85   # 0.0 firm … 1.0 softest
```

Configs without the table load with the defaults (Extra, 100 %).

## Presets

| Preset | Intensity | Softness | Spring (k / c) | Freq | ζ | Wobbles after the first swing |
|---|---|---|---|---|---|---|
| Subtle | 60 % | 0.00 | 127.9 / 20.36 | 1.80 Hz | 0.90 | 0 |
| Natural | 100 % | 0.30 | **90 / 14.5** (physics.toml) | 1.51 Hz | 0.76 | 0 |
| Bouncy | 100 % | 0.60 | ≈ 70.3 / 8.47 | 1.33 Hz | 0.51 | 1 |
| **Extra (sexy)** | 100 % | 0.85 | **≈ 55.7 / 4.32** | **1.19 Hz** | **0.29** | **2** (visible ≥ 0.2 px: 3–4) |

Softness interpolates (frequency, ζ, kick boost, body coupling) between
these anchors:

- 0 → (1.8 Hz, 0.90, ×0.85, 0)
- 0.3 → (physics.toml 90 / 14.5, ×1.0, 0)
- 1 → (1.1 Hz, 0.16, ×1.6, 1)

Then `stiffness = ω²` and `damping = 2ζω`. ζ never drops below 0.15, so
there is no endless buzz. The frequency stays at or below 1.8 Hz, so ω·h
stays under 0.4 even at 30 Hz physics and the semi-implicit Euler sub-step
is stable at every slider value.

Measured peaks after a `knife_tap.strong`, in px with breathing removed
(see `chat_shots/bounce_spring_plot.png`):

| Setting | Peaks |
|---|---|
| Old default | 3.6, 1.9, −0.1 |
| Extra 100 % | 10.3, 1.8, −0.8, 0.35, −0.16 |
| Extra 300 % | 17.6 (soft-limited), 5.3, −2.3, 1.0, −0.5 |
| 0 % | nothing |

## What makes it "sexier" (all softness > Natural only)

- **Bigger, slower swing with overshoot.** Softer settings raise the kick
  boost to as much as ×1.6. Extra is about 2.8× the old amplitude.
- **Lag behind her body.** The chest reacts to the body's vertical
  velocity change (`BODY_COUPLING`). It drops behind a hop, catches up on
  the landing and overshoots.
- **More triggers.** Walk starts and turn-arounds give a small chest
  kick. Footfalls, stops, landings, hip pops and reaction recipes already
  kick it (physics.toml).
- **No steps.** Chest kicks are delivered over a 30 ms time constant
  instead of in one sub-step. Pose cuts no longer snap the chest offset.
  A C1 soft knee (linear up to 8 px, rational ease towards 24 px) rounds
  big swings instead of flat-topping on a clamp. The hard clamps open up
  to 400 px/s and 60 px and only guard against pile-ups.
- **Squash/stretch.** The renderer already squashes the sprite against
  the chest offset (`render.rs`: 0.004 per px, ±8 %). A bigger bounce
  gives more visible squash with no renderer change.
- Mood multipliers still apply on top (Playful softer, Sleepy calmer).

At Natural, the result is exactly the physics-lab tuning: 90 / 14.5 and
gain 1.3, with no coupling or extra kicks.

## Baked jiggle clips (`baked_motion`, `jiggle_exaggerated`)

These clips draw their own chest motion, so the engine chest spring is
switched off while they play: the `baked_motion` tag mask, or the
manifest's `disable_springs`.

| Intensity | Engine bounce on these clips |
|---|---|
| ≤ 100 % | None. The art alone, so there is no double jiggle. |
| > 100 % | A light layer: `baked_layer(i)` eases from 0 at 100 % to 45 % of the soft-limited engine offset at 300 %. |

The chest keeps integrating while it is layered. `Animator::transform_for`
draws `Physics::layered_chest_offset()` for masked chest channels, so
there is no pop when the layer turns on or off. On the shipping v2 art,
which is all `baked_motion`, the slider therefore matters mostly above
100 %. Other clips and custom avatars use the full engine bounce.

## Compatibility

- `[springs] chest_bounce_gain` (the old panel knob) still multiplies the
  base gain. The softness slider owns the chest stiffness and damping on
  the physics-driver path. A legacy `[springs.chest]` is superseded there,
  but still used by the plain-springs fallback.
- `jiggle_mode = "off"` still zeroes everything.
- `PhysicsDriver::set_user_springs` now re-derives from the loaded
  physics.toml every time. This fixes two bugs: a reload used to reset
  physics.toml's gains (e.g. chest 1.3 → 1.0), and a knob moved back to its
  default didn't actually go back.
