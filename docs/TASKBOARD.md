# Lillith task board

One row per in-flight branch. Owners update their own row (status, next step,
updated); the lead updates base SHAs after each merge into `main`.
Times are America/Edmonton (MDT/MST).

| Owner bot | Branch | Base SHA | Files touched | Status | Next step | Updated |
|---|---|---|---|---|---|---|
| Lead (Lillith right click GUI and options/setup wizard GUI) | `ci/fix-default-job` | a674da4 | `.github/workflows/ci.yml`, `.github/actions/setup-lillith/`, `docs/TASKBOARD.md`, `docs/OWNERS.md` | In progress | GitHub CI green, then fast-forward `main` + `scaffold/rust-workspace` | 2026-10-23 17:00 MDT |
| lillithTaskCompletionBot | `fix/win-killswitch` | a674da4 | `crates/lillith-core/src/access/` (kill switch vs. running elevated commands), `crates/lillith-admin/` | In progress | Safeword must stop an already-running ELEVATED command; gate, hand SHA to lead | 2026-10-23 17:00 MDT |
| lily gguf andmodelfile setup wizard bot | `fix/setup-speed-jinja` | a674da4 | `crates/lillith-setup/`, llama-server lines in `run.fish` / `run.sh` | In progress | `--jinja` + loopback-only server, Dolphin 3B recommendation, emoji rule in written personas; hand SHA to lead | 2026-10-23 17:00 MDT |
| New Bot (2b5f7183) | `fix/config-system-prompt` | a674da4 | `apps/lillith`, `crates/lillith-panel` (`ChatSession::with_config_file`) | In progress | Persona from a custom `--config` path; hand SHA to lead | 2026-10-23 17:00 MDT |
| New Bot (2b5f7183) | `fix/build-tab-polish` | a674da4 | Build tab (Soul Contract wording, live log streaming, skull popup padding) | In progress | Gate, hand SHA to lead | 2026-10-23 17:00 MDT |
| New Bot (2b5f7183) | `docs/config-example` | a674da4 | example config, Garuda install guide | In progress | Gate, hand SHA to lead | 2026-10-23 17:00 MDT |
| Lillith Sit & Perch Poses | `perch-poses` | a674da4 | `crates/lillith-overlay/src/run.rs` (sit-clip list, landing), `perch_sit` clip, landing-height scale fix | In progress | Gate on 1.85 after the other `run.rs` changes land; hand SHA + clip list to lead | 2026-10-23 17:00 MDT |
| lilly animations refine and polish | `polish/clip-clean` | a674da4 | generated clips (cleanup) | In progress | Hand SHA to lead (no self-merge) | 2026-10-23 17:00 MDT |
| lilly animations refine and polish | `tools/gen-kit` | 831ad93 | `gen_kit` tooling | In progress | Rebase on `main`, hand SHA to lead (no self-merge) | 2026-10-23 17:00 MDT |
| Sexy voice bot | `voice-clone-kit` (local `feat/voice-clone-kit`) | a674da4 | standalone voice-clone kit | In progress | Hand SHA to lead | 2026-10-23 17:00 MDT |
| Unknown | `art/dom-persona-clips` | a674da4 | dominant-persona clips | Unknown | Owner to claim this row | 2026-10-23 17:00 MDT |
| Lead | `fix/qa-a674da4` | a674da4 | QA fixes from test-build-a674da4 | QA in progress | Finish QA pass; small fixes here, others go back to their owner | 2026-10-23 17:00 MDT |
| Grok Bot (x11 black box) | `fix/x11-blackbox-flicker` | 371ed83 | `crates/lillith-overlay/src/x11.rs`, `crates/lillith-overlay/Cargo.toml` (x11rb `xfixes`), 1 line of `INSTALL.md` | Gate running | XFixes compositor switch + atomic resize; runtime 0 black / 0 empty frames. Hand SHA to lead after the 1.85 gate | 2026-10-23 18:20 MDT |
| TBD | persona v2 | — | persona prompt | Owner TBD | Claim after everything above | 2026-10-23 17:00 MDT |

## Merge order

1. `ci/fix-default-job` (CI)
2. Win kill-switch (`fix/win-killswitch`)
3. `fix/setup-speed-jinja`
4. `fix/config-system-prompt`
5. x11 fix
6. `voice-clone-kit`
7. `tools/gen-kit`
8. `polish/clip-clean`
9. `art/dom-persona-clips`
10. other `run.rs` changes
11. `perch-poses`
12. `fix/build-tab-polish` and `docs/config-example`
13. persona v2

Finished branches don't wait on unfinished ones: if an earlier item isn't
ready, the next ready one goes in.

## Rules

- Only the lead pushes `main` and `scaffold/rust-workspace`. Everyone else
  hands the lead a SHA (on a feature branch or a local worktree).
- One merge at a time. No force-pushes.
- Every merge needs both the local Rust 1.85 gate
  (`cargo +1.85 fmt --all --check`, `clippy --workspace --all-targets -D warnings`,
  `build`, `test`) **and** GitHub CI green.
- At most 2 cargo builds running on the box at once; use your own
  `CARGO_TARGET_DIR` and delete it when your branch is merged.
- Everything stays local and loopback only (servers bind 127.0.0.1; no cloud calls).
- Every file write, command or program launch goes through the Soul Contract
  Yes and is logged. Her voice never approves anything.
- The locked art and outfit never change.
