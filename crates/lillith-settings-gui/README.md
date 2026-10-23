# lillith-settings-gui

Lillith's settings / options pop-up (egui/eframe, glow; Wayland + X11).
Ancient-demon-relic × DemonOS chrome from `lillith-theme`.

```sh
lillith-settings [CONFIG_PATH] [--tab ID] [--step NAME] [--size WxH] [--decorated] [--quiet] [--screenshot PNG]
cargo run -p lillith-settings-gui -- --step pick-model
```

Tabs (registry in `src/tabs/mod.rs`, trait `SettingsTab`): Companion (generated
from `lillith_core::menu::build_menu`, 1:1 with the right-click menu), Motion,
Chat / Brain (first-run wizard: Avatar ✔ › Runtime › Pick model › Persona ›
Test & save, on `lillith-setup`), Avatar, Icon (pick the app icon from
`branding/icons.toml`; saves `icon = "<id>"` and updates the menu entry /
shortcuts), Voice, About, Sealed I–II (empty
add-in slots; `TabRegistry::unseal` fills one).

Persistence: everything goes to `config.toml`. Non-`[llm]` keys are staged in
`store::ConfigStore` and applied onto a fresh read of the file (toml_edit,
comments kept, floats rounded); `[llm]` goes through
`lillith_setup::config::save(path, cfg, sync_chat = true)` (temperature rounded
to 2 places); the Modelfile is written to `[llm] modelfile_path` (default
`~/.config/lillith/Modelfile.lily`). Companion picks are also printed on
stdout as JSON lines (`{"menu_action":"set_mode","value":"wander"}`), and every
successful save prints `{"menu_action":"reload_config"}` so the running overlay
re-reads config.toml live (`--quiet` turns all stdout lines off).

Wire: every stdout line comes from `lillith_core::menu::wire` (`action_line`,
`action_line_in`, `pick_line`), the same table the overlay parses with
`parse_pick`. Companion is generated from `build_menu` with real menu extras:
Poses (from the animation set's manifest via `lillith_anim::catalog`, pages
shown inline, picks sent by clip name as `pose`), Show off, Chat, Minimize,
Avatar (recents from the avatar store, sent by id as `avatar`; "Custom image"
jumps to the Avatar tab), Resize (`set_size` small…xxl, `scale_step` ±1; the
overlay owns size, nothing is written).

Avatar tab: the gallery (Lillith + stored custom avatars), the local importer
(background removal, fit to the 400×400 stage) and the anchor editor with a
live animated preview, all on `lillith_anim::avatar` + `lillith_anim::avatar_edit`
(the same non-UI code `lillith-panel::settings` uses). "Save & use" (or the
footer Save while an import is open) stores it under `<config dir>/avatars`,
selects it in `avatar.json` and prints `{"menu_action":"avatar","value":"<id>"}`
then `{"menu_action":"reload_config"}`.

Local-only: only loopback is contacted (detect / ping / test chat via
`lillith-setup`); llama-server and `ollama create` run only when clicked.
The file dialog (feature `file-dialog`, default) uses rfd's XDG portal
backend; without a portal, type the path.

Screenshots for docs: `--screenshot out.png` renders under any X display
(e.g. Xvfb + Mesa llvmpipe) and exits after the first settled frame.
