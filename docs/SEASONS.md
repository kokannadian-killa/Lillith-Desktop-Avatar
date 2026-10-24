# Seasonal outfits

Lillith gets a themed pin-up outfit per season: Halloween, Christmas/Winter, New Year,
Valentine's, Spring, Summer. Each outfit is a set of 4 baked allure clips (about 16 s each,
two slow cleavage zoom cycles, normal framing at the start and end), plus a menu thumbnail. Everything is
data: [`assets/seasons.toml`](../assets/seasons.toml). **No Rust reads it yet.** This page lists what the GUI
and runtime need to wire up.

## Data

| Where | What |
|---|---|
| `assets/seasons.toml` `[[season]]` | `id`, `name`, inclusive local-date window `start`/`end` (`"MM-DD"`, end < start wraps the year), `priority`, `outfit` |
| `assets/seasons.toml` `[[outfit]]` | `id`, `name` (menu label), `season`, `thumbnail`, `clips` (manifest names), `hidden`, optional `unlock` |
| `assets/seasons.toml` `[setting]` | user setting key `seasonal_outfit`: `"auto"` (default) / `"off"` / an outfit id |
| `assets/generated/outfits/<id>.png` | 128x128 RGBA thumbnail with a transparent background |
| manifest | each seasonal clip is a normal manifest clip named `<season>_<pose>`, tagged `seasonal`, `season:<id>`, `outfit:<id>` (plus `allure`, `baked_motion`, `cleavage_zoom`). The per-clip json also carries `"season"` and `"outfit"` fields. |

The default windows are: halloween 10-01..11-02, xmas 12-01..12-31, newyear 12-31..01-02 (priority 20, so it wins on
Dec 31), valentines 02-07..02-15, spring 03-20..05-31, summer 06-01..08-31.

Reserved for secret or unlockable content: `hidden = true` keeps an outfit (or one clip, via
`[[outfit.secret]] clip/hidden/unlock`) out of the menu and out of auto mode until `unlock` (a free-form condition id,
e.g. `"secret:oops_retie"`) is satisfied. Nothing uses it yet.

## Selection rule

```
active_outfit =
  setting == "off"                -> none
  setting == <outfit id>          -> that outfit (manual always beats the date)
  setting == "auto"               -> outfit of the highest-priority [[season]] whose window contains today (local date), else none
```

An outfit is *available* when at least one of its `clips` exists in the loaded manifest. Clips missing from the manifest
are skipped silently. Branches land at different times, so some outfits have `clips = []` for now.

## What the GUI lead needs to wire

1. **Config key.** Add `seasonal_outfit: String` (default `"auto"`) to the companion config in `lillith-core::config`, and add
   a commented example to `assets/config.example.toml`. Save it with `update_config` like `scale`.
2. **Loader.** Add a small `Seasons` struct (serde, toml) for `assets/seasons.toml` with `active_outfit(today, setting)`
   implementing the rule above. The date math is month-day only, with wrap when `end < start`. Unit-test Dec 31 (newyear
   wins), Jan 2 (newyear), Oct 31 (halloween), Nov 3 (none), and a manual id in July.
3. **Right-click menu.** Under **Avatar ▸** (`lillith-core::menu`, next to `MenuAction::Avatar`), add an
   **Outfit ▸** page: **Auto (by date)**, **Off**, then one entry per non-hidden *available* `[[outfit]]` (label = `name`).
   The current choice gets a check mark. A pick emits a new `MenuEffect::SetSeasonalOutfit(String)` that saves the key and
   re-evaluates.
4. **Settings window.** In the companion tab (`lillith-settings-gui/src/companion.rs`, near `avatars_of`), add the same choice
   as a row of thumbnails (`thumbnail`, 128x128, drawn on the plum background), plus Auto / Off.
5. **Playback.**
   - Seasonal clips (tag `seasonal`) are allure clips. Today `catalog::category_of` only puts names in Flirty / Allure
     via the `ALLURE` list, so the new names would land in *Gestures*. Treat tag `seasonal` as category 0, or add the names.
   - Only the **active** outfit's clips should go into **Show off ✨**, the Flirty page and random allure picks.
     Clips of inactive outfits stay hidden from pools and pages. Simplest: filter `seasonal` clips whose `season:<id>` tag
     isn't the active outfit's season.
   - With `off`, no seasonal clip ever plays.
6. **Canvas.** Same as the other baked clips: 400x400, anchor (200,392), standing clips planted on the baseline. The broom
   clip is seated/floating, with its lowest opaque pixel on the baseline. The zoom is baked in, so play them like other
   `baked_motion` allure clips (no physics bounce on top).

## Producing clips

See `/workspace/lillith/seasonal_RECIPE.md` (box-local) and [VIDEO_PIPELINE.md](VIDEO_PIPELINE.md): `zoom_tease.py`,
`new_pose.py --allure`, matte-clean, `qa_all.py --white-edge --mp4`, fringeqa, and `import-manifest --check`. A new season's
branch adds its clips plus `assets/generated/outfits/<id>.png` and fills in its outfit's `clips` list.
