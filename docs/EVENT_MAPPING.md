# Event mapping: rules.toml ↔ generated clips

Reconciles the 57 `[[rule]]` events in `assets/rules.toml` with the canonical
generated set (`assets/generated/manifest.json`, 64 clips: 38 `event_<name>`
reactions and 26 regular animations).

The mapping lives in `assets/rules.toml` under `[rule_clips]` (the older name
`[event_clips]` is still accepted). It is resolved by
`lillith_core::rules::RuleSet::clip_for(event)`; `event_clip()` is kept as an
alias. A rule's clip can be an event reaction or any regular animation. A rule
with no entry falls back to its `pose`. Tests in `lillith-core` (`rules::tests`)
pin every mapped pair, check that 57 mapped + 0 unmapped = 57, and check that every target is in `KNOWN_EVENT_CLIPS` or
`KNOWN_BASE_CLIPS`.

**Totals: 57 / 57 rules have a clip** (27 event-clip links + 14 dedicated
batch-3 event clips + 10 regular animations + 6 low-confidence stand-ins).
**0 true art gaps.** All 38 event clips are used.

## 1. Event reaction clips: 27 rules → 24 clips

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

- **Note A:** `window.opened.folder/browser/terminal/media` now use `event_window_open` as a
  low-confidence stand-in (section 3). Their own poses and lines (peeking_folder, peeking,
  hacking, sitting+dance) are still in the rule and are better targets once that art exists.
- **Note B:** the `window.closed` rule is pose `knife` + gesture `stab` + line "Gotcha! 🗡️",
  but `event_window_close` is a friendly wave goodbye. The event matches; the
  personality doesn't. **Unchanged, pending Ko's tone decision.** (`input.click.left`
  now uses `knife_tap`, so the knife still has a home either way.)

## 2. Regular animations reused for rules: 10 (safe)

| Rule event | Rule pose | Clip | Notes |
|---|---|---|---|
| `idle.asleep` | sleep | sleep | |
| `input.drag` | drag_drop | drag_held | position by the clip's `grab_point` [200,76], not the feet |
| `input.click.left` | knife | knife_tap | |
| `llm.thinking` | waiting | think | |
| `llm.done` | sitting | tada_celebrate | rule gesture is `tada` |
| `idle.long` | sitting | sit_swing | uses seat_y / seat_x_range |
| `input.hover` | hover_sit_on_window | sit_swing | uses seat_y / seat_x_range |
| `focus.long_work` | code_inspect | shoulder_roll | fits the "stretch" line |
| `user.returned` | peeking | wake | plays once; ends on idle_00 |
| `input.secret` | dual_tail | tail_wag | |

## 3. LOW-CONFIDENCE stand-ins: 6 (override when dedicated art exists)

| Rule event | Rule pose | Stand-in clip | Why it's only a stand-in |
|---|---|---|---|
| `update.available` | dismiss_notif | event_notification | generic bell; no "update" art |
| `timer.fired` | scheduler | event_calendar | calendar, not a timer/clock |
| `window.opened.folder` | peeking_folder | event_window_open | generic window; rule wants peeking into a folder |
| `window.opened.browser` | peeking | event_window_open | generic window; rule wants peeking/surfing |
| `window.opened.terminal` | hacking | event_window_open | generic window; rule wants hacking |
| `window.opened.media` | sitting | event_window_open | generic window; rule wants sitting + dance |

## 4. Dedicated event clips, batch 3: 14 (formerly the true art gaps)

Each clip's own `event` field is the dotted rule name; checked to match.
`*` = brand-new pose: the clip is its own `base_clip` (self-reference, see below).

| Rule event | Clip | base_clip | Attachment points carried |
|---|---|---|---|
| `ram.high` | event_ram_high * | itself | icon_origin |
| `net.lost` | event_net_lost * | itself | icon_origin |
| `vpn.up` | event_vpn_up * | itself | finger_point, icon_origin (fades to ~72% opacity) |
| `hotspot.on` | event_hotspot_on | spell_orb_cast | broadcast_origin, icon_origin |
| `usb.added` | event_usb_added | wave | icon_origin, port_point |
| `clipboard.cut` | event_clipboard_cut * | itself | finger_point, icon_origin |
| `start_menu.opened` | event_start_menu_opened * | itself | icon_origin, menu_origin [262,30] (menu top-left), palm_point |
| `search.opened` | event_search_opened | think | icon_origin |
| `browser.bookmark` | event_browser_bookmark | dance | icon_origin |
| `browser.history` | event_browser_history | hip_sway | icon_origin |
| `approval.needed` | event_approval_needed * | itself | icon_origin, palm_point |
| `input.click.double` | event_input_click_double | tada_celebrate | click_point, icon_origin |
| `input.click.right` | event_input_click_right * | itself | click_point, icon_origin, menu_origin, palm_point |
| `input.hover_long` | event_input_hover_long | lean_forward | cursor_point, icon_origin |

**Self-referencing base clips (7):** event_ram_high, event_net_lost,
event_vpn_up, event_clipboard_cut, event_start_menu_opened,
event_approval_needed, event_input_click_right. The image bot's note listed 6;
event_input_click_right is a 7th. The loader accepts self-reference: the clip
exists and its own frame count bounds its `base_frames` (pinned by the
`allows_self_referencing_base_clip` test in lillith-anim). The engine should
treat `base_clip == name` as "this is the base pose", not as a separate base to
compose.

## 5. TRUE art gaps: none

All 57 rules have a clip. `UNMAPPED` in `rules::tests` is empty; a new rule
without art fails the tests until it is either mapped or listed there.

## 6. Event clips with NO rule: 0

All 38 event clips are reachable. Clips serving several rules: event_window_open
(5), event_wifi (2), event_error (2), event_help (2), event_notification (2),
event_calendar (2), sit_swing (2).

## Naming note

The 24 older event clips carry a snake_case `event` field (e.g. `cpu_busy`); the
14 batch-3 clips carry the dotted rule name (e.g. `ram.high`). The engine binds
rules to clips through `[rule_clips]`, so this doesn't affect lookup; it would
only matter if something matched on the clip's `event` field directly.

## Render notes (not importer/mapping issues)

event_bluetooth, event_volume, event_firewall_block, event_recycle_bin,
event_clipboard and event_diagnostics point at an icon on screen-LEFT (base
clip `point`). When she stands near the left screen edge, the renderer may need
to mirror the sheet (and any points) so the icon stays on screen.

event_screenshot (white flash), event_incognito (fade to ~45%) and event_vpn_up
(fade to ~72%) contain intentional translucent full-frame effects.
