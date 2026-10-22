# Right-click menu, Settings and custom avatars

## Right-click menu

Right-click Lillith (or press the context-menu key while she has focus) to
open her pink/plum popup menu next to her. It is drawn by the overlay itself
(`lillith-overlay::menu_popup`; an `xdg_popup` on Wayland, an override-redirect
window on X11) from the model in `lillith-core::menu`. Esc, a click outside
it, or a left click on her closes it. Opening the menu never starts a drag.

| Item | What it does |
|---|---|
| **Poses ▸** | **Show off ✨** (4–6 random allure clips in a row) and one entry per category (Flirty / Allure, Idle habits, Gestures, Dance & fun, Events / reactions, Talk). A category opens a page of up to 14 clips with **‹ Back** and **More…** for the next page. Only shown when the set has pose clips. |
| **Chat** | Opens the chat bubble (see [CHAT.md](CHAT.md)). Greyed out when chat is not wired. |
| **Avatar ▸** | Lillith, up to 6 recent custom avatars, or **Custom image…** (opens the `lillith-settings` window to import one). |
| **Resize ▸** | Small 0.45 / Normal 0.55 / Large 0.75 / XL 0.95 / XXL 1.25, plus **Bigger +** / **Smaller −** (0.05 steps, 0.3–1.6). Applies live with her feet kept in place and is saved to `scale`. |
| **Minimize to peek** | Shrinks her to a small peek icon at the screen edge. Click the icon to bring her back at her old size and place. |
| Mode ▸, Mood ▸, Mute voice, Pause system monitor | As before. |
| **Settings…** | Opens the separate `lillith-settings <config.toml>` window (crate `lillith-settings-gui`); `xdg-open config.toml` if it is not installed. |
| Reset position, About Lillith | As before. |
| **Close** | Quits Lillith and stops a `llama-server` that she started. |

Each item is a `MenuAction` in `lillith-core::menu`; `on_menu_action` turns a
pick into brain actions plus `MenuEffect`s (`PlayPose`, `ShowOff`,
`OpenChat`, `SetAvatar`, `ImportAvatar`, `Minimize`, `SetScale`,
`OpenPage`, `OpenSettings`, …) that the overlay's `Controller::menu_pick`
applies. The old egui menu window (`lillith menu`, `menu.*` events) is
retired.

### Scale

`scale = 1.0` in config.toml (the default) means **the 0.55 default**
(~220 px, Ko's choice). Any other positive value is used as is, so the
presets above avoid exactly 1.0 and Bigger/Smaller skip over it.
`--scale` on the command line is always literal.

### Wire format (`lillith-settings` → overlay)

The settings window prints picks on stdout, one JSON object per line; the
overlay's reader (`lillith-overlay::settings_window`) parses them with
`lillith_core::menu::parse_pick` and applies them on its own thread. Names
are the constants in `lillith_core::menu::wire`; `action_line` /
`action_line_in` / `pick_line` print them.

| `menu_action` | `value` | Item |
|---|---|---|
| `set_mode` | `stay` / `wander` / `follow` | Mode ▸ |
| `set_mood` | `playful` / `happy` / `mischievous` / `alert` / `sleepy` | Mood ▸ |
| `toggle_mute`, `toggle_monitor` | – | Mute voice, Pause system monitor |
| `set_size` | `small` / `normal` / `large` / `xl` / `xxl` | Resize ▸ presets |
| `scale_step` | `+1` / `-1` (× 0.05) | Resize ▸ Bigger / Smaller |
| `pose` | clip name, e.g. `blow_kiss` | Poses ▸ <clip> |
| `pose_index` | `n` (index in the overlay's own list; prefer `pose`) | Poses ▸ <clip> |
| `pose_page` | `group/page` | Poses ▸ <category> page |
| `menu_back` | – | ‹ Back |
| `show_off` | – | Poses ▸ Show off |
| `chat` | – | Chat |
| `minimize` | – | Minimize to peek |
| `avatar` | avatar id, or `lillith` | Avatar ▸ switch |
| `avatar_index` | `n` ≥ 1 (recent list; prefer `avatar`) | Avatar ▸ recent |
| `avatar_import` | – | Avatar ▸ Custom image… |
| `open_settings` | – | Settings… (ignored from the window itself) |
| `reset_position`, `about` | – | Reset position, About Lillith |
| `quit` | – | Close |
| `reload_config` | – | config.toml changed: re-read and apply live |

## Interim settings window (`lillith settings`)

Her menu now opens `lillith-settings`. Until `lillith-settings-gui` adopts
them, the avatar importer, the anchor editor and the animation / jiggle /
chat knobs below stay in `lillith-panel::settings`, reachable as the hidden
`lillith settings [--section avatar] [--import IMAGE]` command (a separate
process; the overlay no longer opens it). Every change is applied live and saved to `config.toml` 0.3 s after
your last edit; the status line shows `saved ✓`.

- **Animation:** playback speed, talk (lip-flap) speed, smooth frames, and
  the cross-fade fraction.
- **Jiggle:** mode (baked clip variants / engine springs / off), spring
  gains (chest, pigtails, hips, damping), and a *Flirty preset*.
- **Chat:** endpoint, model, temperature, system prompt, the speech bubble
  over her head, and whether a click on her opens the chat.
- **Window:** always on top; click-through (only her head stays clickable,
  so you can still drag her and open the menu); start with the desktop
  session (writes `~/.config/autostart/lillith.desktop`, or removes it).
- **Avatar:** picker, import, and the anchor editor (see below).

## Custom avatars (single image, animated in 2.5D)

Any PNG, JPG or WebP can become your companion. Everything runs locally.

1. **Import.** Use Settings ▸ Avatar ▸ path or **Browse…** (zenity or
   kdialog), then **Import**.
   - An image that already has transparency is kept as is. You can also
     force this with **Already transparent**.
   - Otherwise a flat background is removed. The background colour is the
     median of the border pixels. The import flood-fills from the edges,
     clears enclosed background holes (inside a tail loop, between an arm
     and the body), and softens and de-spills the edges.
   - **background tolerance** sets how far a colour can be from the
     background and still be removed.
2. **Fit.** The subject is scaled into her 400×400 stage with its feet on
   the baseline (y = 392), centred on her anchor (200, 392).
3. **Anchors.** Head, mouth, chest, hip and the two hair tips are guessed
   automatically. Drag the dots in the anchor editor to correct them, while
   a live preview animates next to it.
   - **Character** turns on the jiggle and talk bob. It is unchecked
     automatically for wide, object-like images.
   - **jiggle** sets the warp strength.
   - **Re-guess anchors** resets the dots.
4. **Save & use.** Saved files:
   - `<config dir>/avatars/<id>/avatar.png`
   - `<config dir>/avatars/<id>/avatar.json` (the metadata)
   - `<config dir>/avatar.json`, which records the selection and the
     recent avatars
   
   Switching avatars applies live and is remembered across restarts. Pick
   **Lillith** to go back to her sprite clips.

### Motion

A custom avatar is animated continuously on every render frame, not in
baked frame steps:

- **Rendering:** frame-rate-independent time, sub-pixel placement,
  bilinear sampling, and a 24×24 warp mesh.
- **Idle:** a slow, eased breath (4.6 s) and sway (7 s).
- **Hover and interaction:** a hover bob and a parallax tilt toward the
  pointer. Drag and click reactions use critically or under-damped springs
  that settle softly.
- **Walking:** hop/glide steps with a lean.
- **Talking and jiggle:** a talk bob while she speaks. The jiggle is a
  mesh warp around the chest and hair anchors, driven by the same springs
  as Lillith.
- **Shadow:** a soft drop shadow under her.
- **Blending:** idle, walk, talk and react are blended with smoothed
  weights instead of hard switches. Inputs are low-pass filtered and
  velocities are clamped, so there is no jitter.

Lillith's own sprite clips have an optional **smooth mode** (on by
default). Over the last `crossfade` fraction of each frame, it cross-fades
into the next frame. The mouth patch position is interpolated as well.

Walking: walk clips with `root_motion` (the v2 set) are feet-locked: the
window moves by exactly what the drawn frames step, so the planted boot
stays put. Only for walk clips *without* root motion, a `move_speed_px_s`
(importer tag `move_speed_px_s:<n>`) sets the glide speed to *speed × scale
× playback speed*; with neither, the configured `wander_speed_px_s` is used.

### v2 clips (`baked_motion`)

The canonical v2 set (`assets/generated`) tags every clip `baked_motion`:
the video frames already contain the chest, pigtail and hip jiggle. While
such a clip is drawn the engine's chest, pigtail and hip (`root_x`) springs
are switched off through the same `SpringMask` path as `disable_springs`
(`AnimatorConfig::baked_motion_springs_off`, default on), so nothing jiggles
twice; breathing scale is damped by `baked_motion_gain`. The set has no
`talk_loop` or mouth patches: when she talks the base clip is left alone and
the speech bubble (and voice) carry the line.

### Config keys

```toml
always_on_top = true
click_through = false
autostart = false
jiggle_mode = "springs"   # baked | springs | off (unset: from baked_jiggle)

[anim]
playback_speed = 0.7
talk_speed = 0.62
smooth_frames = true      # cross-fade consecutive sprite frames
crossfade = 0.5           # fraction of each frame spent blending

[avatar]
pace = 0.8                # custom-avatar motion pace (lower = slower)
render_hz = 60.0          # redraw rate while a custom avatar is shown
```
