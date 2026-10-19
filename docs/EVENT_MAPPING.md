# Event mapping: rules.toml ↔ event reaction clips

Reconciles the 57 `[[rule]]` events in `assets/rules.toml` with the 24
`event_<name>` reaction clips in the canonical generated set
(`assets/generated/manifest.json`, final batch of 50 clips).

The mapping lives in `assets/rules.toml` under `[event_clips]` and is resolved by
`lillith_core::rules::RuleSet::event_clip(event)`. A rule with no entry has no
reaction clip yet and falls back to its `pose`. Tests in `lillith-core`
(`rules::tests`) check every mapped pair and every unmapped rule below, and that
every clip in the table exists in `KNOWN_EVENT_CLIPS`.

I only mapped a rule when the clip shows the same thing. "pose match" means the
rule's own `pose` is the pose the clip depicts.

## 1. Mapped: 27 rules → 24 clips

| Event clip | Rule event(s) | Basis | Confidence |
|---|---|---|---|
| event_cpu_busy | `cpu.high` | pose `busy`; CPU chip heating, sweat | exact |
| event_battery_low | `battery.low` | pose `battery_low` | exact |
| event_wifi | `wifi.changed` | pose `wifi` | exact |
| event_wifi | `net.restored` | rule also uses pose `wifi` (arcs appear, pulse, sparkle) | pose match |
| event_bluetooth | `bt.paired` | pose `bluetooth` | exact |
| event_volume | `volume.changed` | pose `volume` | exact |
| event_brightness | `brightness.changed` | pose `brightness` | exact |
| event_firewall_block | `firewall.blocked` | pose `firewall`; shield + no-entry mark | exact |
| event_usb_eject | `usb.removed` | USB stick rises and fades under the eject symbol, wave bye | exact (semantics) |
| event_printing | `print.job` | pose `printing` | exact |
| event_archiving | `archive.created` | pose `compress`; files into a box, zip line | exact (semantics) |
| event_cloud_sync | `cloud.sync` | pose `cloud_sync` | exact |
| event_recycle_bin | `trash.added` | pose `recycle`; paper ball into bin | exact (semantics) |
| event_incognito | `window.private` | pose `private`; dark glasses + fade to ~45% | exact (semantics) |
| event_notification | `notify.received` | pose `dismiss_notif`; bell with a "1" badge | exact (semantics) |
| event_calendar | `calendar.reminder` | pose `calendar`; page flips, day circled | exact |
| event_screenshot | `screenshot.taken` | pose `screenshot`; crop corners, flash, thumbnail | exact |
| event_clipboard | `clipboard.copied` | pose `clipboard`; check mark | exact |
| event_window_open | `window.opened.app` | generic window grows open; `.app` is the generic case | medium (see note A) |
| event_window_close | `window.closed` | same event; **tone differs** (note B) | event match |
| event_window_minimize | `window.minimized` | pose `minimize` | exact |
| event_loading | `app.loading` | pose `loading`; spinner | exact |
| event_error | `journal.error` | pose `error`; warning triangle | pose match |
| event_error | `app.crash` | pose `error` | pose match |
| event_help | `help.requested` | pose `help`; "?" bubble | exact |
| event_help | `input.click.middle` | pose `help` (middle click = help per `help.requested` source) | pose match |
| event_diagnostics | `system.status` | pose `diagnostics`; monitor ECG | exact |

- **Note A:** `window.opened.folder/browser/terminal/media` each have their own pose and line
  (peeking_folder, peeking, hacking, sitting+dance), so they stay unmapped. `event_window_open`
  could be their fallback if you want.
- **Note B:** the `window.closed` rule is pose `knife` + gesture `stab` + line "Gotcha! 🗡️",
  but `event_window_close` is a friendly wave goodbye. The event matches; the
  personality doesn't. Decide which one wins, or keep the clip for the event and
  the knife for `input.click.left`.

## 2. Rules with NO event clip: 30

| Rule event | Rule pose | Closest existing clip (not mapped) | Notes |
|---|---|---|---|
| `ram.high` | htop | event_cpu_busy? | clip draws a CPU chip, so it's specifically CPU |
| `net.lost` | connection_lost | — | needs a "disconnected" reaction (event_wifi is the positive one) |
| `vpn.up` | vpn | — | |
| `hotspot.on` | hotspot | — | |
| `usb.added` | drive_eject | event_usb_eject? | rule reuses `drive_eject`, but the clip is a goodbye/eject, the opposite meaning |
| `clipboard.cut` | crop | event_clipboard? | clip shows a check mark, not a cut |
| `update.available` | dismiss_notif | event_notification? | rule reuses the notification pose; plausible fallback |
| `timer.fired` | scheduler | event_calendar? | different object (timer vs calendar) |
| `window.opened.folder` | peeking_folder | event_window_open? | see note A |
| `window.opened.browser` | peeking | event_window_open? | see note A |
| `window.opened.terminal` | hacking | event_window_open? | see note A |
| `window.opened.media` | sitting | event_window_open? / sit_swing, dance | see note A |
| `start_menu.opened` | start_menu | — | |
| `search.opened` | magnify | — | |
| `browser.bookmark` | bookmark | — | |
| `browser.history` | browser_history | — | |
| `focus.long_work` | code_inspect | shoulder_roll / lean_forward (base clips) | "stretch" line fits shoulder_roll |
| `idle.long` | sitting | sit_swing (base clip) | |
| `idle.asleep` | sleep | sleep (base clip) | covered by the base `sleep` clip |
| `user.returned` | peeking | wake / wave (base clips) | |
| `llm.thinking` | waiting | think (base clip) | |
| `llm.done` | sitting | tada_celebrate (base clip) | gesture is `tada` |
| `approval.needed` | waiting | — | |
| `input.click.left` | knife | knife_tap (base clip) | |
| `input.click.double` | hacking | — | |
| `input.click.right` | cleaning | — | |
| `input.hover` | hover_sit_on_window | sit_swing (base clip) | |
| `input.hover_long` | peeking | — | |
| `input.drag` | drag_drop | drag_held (base clip) | covered by base `drag_held` (pin `grab_point`) |
| `input.secret` | dual_tail | dance / tail_wag (base clips) | gesture is `dance` |

Many of these are already covered by a non-event base clip from the 50 (sleep,
drag_held, knife_tap, think, sit_swing, …). That is a separate rule→base-clip
binding and is not part of `[event_clips]`. The real art gaps (no event clip and
no obvious base clip) are: **ram.high, net.lost, vpn.up, hotspot.on, usb.added,
clipboard.cut, start_menu.opened, search.opened, browser.bookmark,
browser.history, approval.needed, input.click.double, input.click.right,
input.hover_long**, plus the four specific `window.opened.*` variants if
they should differ from `event_window_open`.

## 3. Event clips with NO rule: 0

All 24 event clips are reachable from at least one rule. Clips that serve more
than one rule: event_wifi (2), event_error (2), event_help (2).

## Render note (not an importer/mapping issue)

event_bluetooth, event_volume, event_firewall_block, event_recycle_bin,
event_clipboard and event_diagnostics point at an icon on screen-LEFT (base
clip `point`). When she stands near the left screen edge, the renderer may need
to mirror the sheet (and any points) so the icon stays on screen.
