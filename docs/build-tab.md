# Build tab — local "vibe coding" (Phase 3)

The **Build** tab in `lillith-settings` is a small, Replit-style surface for
scaffolding, building and running local projects. It is deliberately modest and
**entirely on-box**: it only drives tools that are already installed
(`cargo`, `python3`, `fish`), never touches the network, and never runs
anything without an explicit **Soul Contract Yes**.

## Where it lives

- UI: `crates/lillith-settings-gui/src/tabs/build.rs` (tab id `build`, between
  Voice and About). Open it directly with `lillith-settings --tab build`.
- Logic: `crates/lillith-core/src/build.rs` — language/template model,
  project-path confinement, and the offline scaffold/build/run implementations.
- Gate: `crates/lillith-core/src/soul_contract.rs` — the new
  `PendingAction::{ScaffoldProject, BuildProject, RunProject}` variants.

## The gate (nothing runs on its own)

Every button *proposes* a `PendingAction`; it does not execute it. A proposed
action shows a **SOUL CONTRACT** panel with a one-line summary and a preview of
the exact command, plus two buttons:

- **✓ Yes, run it** — runs the action on a background thread and streams the
  captured output into the **LOG** pane. Audited as `CONFIRM`.
- **✕ No** — discards the action. Audited as `DENY`.

This is the same gate as `/contract yes|no` in chat; the Yes/No buttons are the
GUI front-end for it. Confirmations and denials are appended to
`<data>/lillith/audit.log` (`$XDG_DATA_HOME/lillith`, else
`~/.local/share/lillith`), e.g.:

```
2026-10-22T20:41:03Z	CONFIRM	scaffold_project	/home/ko/Projects/lillith-builds/demo
```

"Open folder" is the one non-gated button: it only opens the project directory
in the file manager (`xdg-open`) and changes nothing.

## Confinement

All project paths are confined to the **projects root** by
`build::resolve_project_dir`:

- A bare name (`my-app`) or nested name (`games/snake`) is taken relative to the
  root.
- `..` is resolved lexically first, so it can never climb out of the root.
- An absolute or `~` path outside the root is rejected, as is the root itself.

The default root is `~/Projects/lillith-builds`. Add that folder to
`[mind] allow_paths` if you also want Lillith to read the generated code in
chat (the Build tab's own confinement is independent of `[mind]`).

## Languages, templates and commands

Everything below is offline. Cargo runs pass `--offline` with
`CARGO_NET_OFFLINE=true`; Python and fish scaffolds just write plain files.

| Language | New project (template)             | Build                         | Run               |
|----------|------------------------------------|-------------------------------|-------------------|
| Rust     | `cargo new --offline --vcs none [--lib]` (`bin`/`lib`) | `cargo build --offline` | `cargo run --offline` |
| Python   | writes `main.py`, `README.md` (+ package + `pyproject.toml` for `package`) | `python3 -m py_compile main.py` | `python3 main.py` |
| fish     | writes executable `main.fish`, `README.md` (`script`) | `fish -n main.fish` | `fish main.fish` |

Scaffolding refuses to overwrite a directory that already exists and is not
empty.

## Config — `[build]`

```toml
[build]
enabled = true                 # master switch for the tab
projects_dir = ""              # empty = ~/Projects/lillith-builds; ~ is expanded
default_language = "rust"      # rust | python | fish, preselected in the tab
```

- `BuildConfig::resolved_projects_dir()` → the configured root, else
  `~/Projects/lillith-builds`.
- `BuildConfig::resolved_default_language()` → the parsed language, defaulting
  to Rust.

Editing **Projects root** or **Language** in the tab stages
`build.projects_dir` / `build.default_language` into `config.toml`; they are
written (non-destructively) by the window's **Save** button.

## What is intentionally *not* here

- No network, no package downloads, no cloud build.
- No autonomous execution: the model/overlay can propose, but a human Yes is
  always required.
- No shell escape: the Build tab files only the three project actions. Free-form
  shell stays behind `[mind] allow_shell` + the contract, unchanged.
