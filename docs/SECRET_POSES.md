# Secret poses (Easter eggs)

Secret poses are ordinary manifest clips that the GUI should keep out of the
normal pose menu, the random "Show off" pool and idle rotation until the owner
unlocks them.

## How a clip is marked secret

In `assets/generated/manifest.json` (and the generated `manifest.toml`), a
secret clip carries these free-form tags:

| tag | meaning |
| --- | --- |
| `secret` | hide from menus and random/idle selection until unlocked |
| `easter_egg` | show in the "Secrets" list once unlocked |
| `unlock:<method>` | suggested unlock trigger (see below) |
| `season:<name>` | outfit season, for the seasonal outfit picker |

Secret clips are **not** marked `allure`, so they never enter the Flirty /
Allure category or `catalog::show_off`.

> Until the GUI filters the `secret` tag, `catalog::grouped` lists every pose
> clip, so the clip shows up under "Gestures". The one-line fix is to add
> `&& !c.has_tag("secret")` (unless unlocked) to `catalog::is_pose`, or to
> filter in the menu builder.

## Clips

| clip | season | length | unlock tag |
| --- | --- | --- | --- |
| `secret_oops_retie` | Halloween (micro witch bikini, cape, hat) | 20 s, 12 fps, plays once | `unlock:konami_or_triple_click_hat` |

`secret_oops_retie`: her halter string comes loose, the camera zooms fast to
her chest (baked zoom, up to 2.3x). She catches the string in her teeth with a
wide-eyed gasp, fumbles to retie it with a cartoon bounce, finishes the bow,
winks, and the camera eases back to normal framing (frame 0 and the last frame
are at normal framing: 400x400, anchor (200,392)). Her top stays on and covers
her on every frame; every frame was inspected before commit.

## Suggested unlock flow (for the GUI lead)

1. Pick a trigger. Any one is enough:
   - triple-click on her hat while the Halloween outfit is active (or between
     Oct 24 and Nov 1 with outfit = auto);
   - the Konami code (Up Up Down Down Left Right Left Right B A) while the
     overlay has focus;
   - a typed chat phrase such as `trick or treat`.
2. On the first trigger, persist `unlocked_secrets = ["secret_oops_retie"]`
   in the settings file (`lillith-core` config) so it stays unlocked.
3. Play the clip once (`LoopMode::Once`), then return to idle.
4. After unlocking, list it in a "Secrets" submenu of the right-click menu
   (only clips tagged `secret` whose name is in `unlocked_secrets`).
5. Respect the existing allure/content settings: if allure poses are
   disabled, secret clips stay locked too.
