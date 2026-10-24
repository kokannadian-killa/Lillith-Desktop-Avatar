# Owners

Who to hand changes to for each crate or area. "Lead" is the
**Lillith right click GUI and options/setup wizard GUI** bot, which also owns
integration, merges into `main` / `scaffold/rust-workspace`, and releases.
Anything not listed here belongs to the lead. See `docs/TASKBOARD.md` for
what's in flight.

| Area | Paths | Owner |
|---|---|---|
| Integration, merges, releases, test-build zips | `main`, `scaffold/rust-workspace`, release tags | Lead |
| CI | `.github/` | Lead |
| Right-click menu, settings, theme | `crates/lillith-panel/`, `crates/lillith-overlay/src/settings_window.rs`, `crates/lillith-core/src/menu.rs` | Lead |
| App entry points / chat wiring | `apps/lillith/` | Lead (New Bot 2b5f7183 on `fix/config-system-prompt`) |
| Build tab | Build tab UI, `docs/build-tab.md` | Lead (New Bot 2b5f7183 on `fix/build-tab-polish`) |
| Example config, install docs | example config, Garuda/Arch install guide | Lead (New Bot 2b5f7183 on `docs/config-example`) |
| Overlay, KWin blur pre-start | `crates/lillith-overlay/` (except `run.rs` sit/landing), `kwin/`, `linux/` | Lead |
| Sit / perch / landing | `crates/lillith-overlay/src/run.rs` sit + landing code, `perch_*` clips | Lillith Sit & Perch Poses |
| File / program / terminal access, Soul Contract, Windows admin helper | `crates/lillith-core/src/access/`, `crates/lillith-core/src/soul_contract.rs`, `crates/lillith-admin/` | lillithTaskCompletionBot |
| Speak-out-loud toggle | `crates/lillith-voice/src/config.rs` toggle wiring | lillithTaskCompletionBot |
| Voice, TTS, emoji stripping | `crates/lillith-voice/` | Sexy voice bot |
| Voice-clone kit | `lillith-voice-kit`, `voice-clone-kit` branch | Sexy voice bot |
| Setup wizard, runtime detect, GGUF/Modelfile, llama-server launch | `crates/lillith-setup/`, llama-server lines in `run.fish` / `run.sh` | lily gguf andmodelfile setup wizard bot |
| Persona prompt | `crates/lillith-mind/src/persona.rs` | lily gguf andmodelfile setup wizard bot (persona v2: TBD) |
| Mind / LLM / memory / tools | `crates/lillith-mind/` (rest) | Lead |
| Animation engine, lip sync | `crates/lillith-anim/` | LILLITH Image Genorater |
| Generated art, poses, MP4 pose source | `assets/generated/` (locked art and outfit never change) | LILLITH Image Genorater |
| Clip cleanup, generation kit | `polish/clip-clean`, `tools/gen-kit`, `gen_kit` | lilly animations refine and polish |
| Dominant-persona clips | `art/dom-persona-clips` | Lead (owner unknown) |
| Core engine (physics, rules, events, springs) | `crates/lillith-core/` (rest) | Lead |
| Sensing | `crates/lillith-sense/` | Lead |
| Tray | `crates/lillith-tray/` | Lead |
| X11 fix | `fix/x11-blackbox-flicker` | Lead (owner TBD) |
