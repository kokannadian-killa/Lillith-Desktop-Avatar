# Mind tools (Phase 2): scoped file/shell API behind the Soul Contract

The local GGUF mind can ask for file and shell actions by writing a fenced
`tool` block in its reply. `lillith-mind::fstools` parses it, checks it against
the allowlist, and sends it through `lillith-core::soul_contract`.

## Fenced format (what the model writes)

The first line of the body is the command. For `write` and `shell`, everything
after that line is the payload.

    ```tool
    list ~/Projects
    ```

    ```tool
    read ~/Projects/notes.md
    ```

    ```tool
    search ~/Projects TODO
    ```

    ```tool
    write ~/Projects/hello.txt
    file contents go here
    ```

    ```tool
    shell
    ls -la ~/Projects
    ```

You can also put the command on the fence line (```` ```tool read ~/x ````).
A JSON body also works, e.g. `{"tool":"write","path":"~/x","body":"..."}`
(with `"query"` for search and `"command"` for shell). Plain fences, and
fences such as ```` ```tooling ````, are ignored.

## Gating: one Soul Contract plan

Looking is free. Doing is a plan.

| Action  | Allowlisted path | Review? | Audit line |
|---------|------------------|---------|------------|
| list / read / search / locate | required (home + mounts by default) | no, runs now | `AUTO` |
| programs / installed / list_skills / read_skill | — | no, runs now | — |
| write / edit (incl. a ```` ```skill ```` draft) | required | **plan item with a diff** | `PROPOSED` / `APPROVED` / `DECLINED` / `RESULT-*` |
| move / copy / rename / mkdir / sort | required | **plan items** (exact destination, exact `name (2)` rename) | same |
| undo    | — | **plan item** (lists what goes back) | same |
| delete  | required, never system paths | **plan item + signature** (full recursive list + typed phrase) | same + `deletions.log` |
| import_skills (and the Desktop import at start) | `skills = true` | **plan items** (each copy, exact `(from desktop)` name) | same + `skills-import.log` |
| open (app or program, with files) | `programs = true` | **plan item** (program, full path, args, files, cwd, risky) | same + `commands.log` |
| run / skill scripts | `commands = true` | **plan item** (exact command, cwd, risky) | same + `commands.log` |
| shell   | `allow_shell = true` | **risky plan item** | same + `commands.log` |
| start / restart the local model server (`[chat.server]`) | `[chat.server]` set | **card, click on Yes only** (never typed/spoken yes); `lillith chat --spawn-server` without a desktop: refused (`NO_UI`) and prints the command to start it yourself | `PROPOSED` / `CONFIRM` / `DENY` / `TIMEOUT` / `NO_UI` / `CANCELLED`, then `USED` / `SPAWN_REFUSED` / `SPAWN_FAILED` from the starter, all with `card=<id>` + `commands.log` |
| run / shell that removes files | — | **refused** (use delete) | — |

When one reply holds several ```` ```tool ```` blocks, the free ones run and
everything else becomes **one combined plan in her order**. The review (bubble
popup or `lillith chat` prompt) shows every numbered item: operation, full
source and destination, size, exact auto-rename, a diff for edits, every file
a delete would remove with sizes, and for programs the program, its full path,
arguments, files touched, working folder and risky yes/no. The plan ends with
totals. You choose:

- **APPROVE ALL**,
- **APPROVE SELECTED** — tick items (folders group their children; select
  all / none), or at the console type `1,3-5`,
- **DECLINE** (or `none`, Esc, empty input): nothing happens.

Only approved items run, in order. The approval is single-use and bound to the
exact set: if anything on disk changed since the plan was made (a file
changed, a destination appeared, a program moved), nothing runs and you get a
fresh plan. Selected deletes still need the typed phrase + SIGN; Trash is the
default and system paths are always refused. Every proposal, decision and
result is logged in `~/.local/share/lillith/audit.log` and `plans.log`; file
changes are journaled for `undo`.

Anything outside the allowlist is refused before the contract sees it. The
refusal goes back to the chat as a note.

## Config (`[mind]`)

```toml
[mind]
allow_paths    = ["~/Projects"]   # extra roots on top of config dir, knowledge_dir, skills_dir
allow_shell    = false            # shell is off by default; even when on, every command is a reviewed plan item
max_read_bytes = 65536            # cap for read/search output
unlock_paths   = []               # opt-in roots inside the default deny list (e.g. "/etc/nginx")
```

These paths are denied by default even when an allow root covers them, unless
you list them in `unlock_paths`: `/etc`, `~/.ssh`, `~/.gnupg`, `~/.aws`,
`~/.config/gcloud`, `~/.password-store`, `~/.docker`, `~/.kube`. No network
tool is offered.

## Answering

In the chat bubble, type `/contract yes` or `/contract no`. `/skill yes|no`
still works as an alias. In the console (`lillith chat`) you answer at the
`[y/N]` prompt.

## Default-on access (`[mind.access]`)

Everything below is **on by default** and can be switched off in
`config.toml` or in Settings ▸ **Access**. It is all local: no network tool
exists, nothing is uploaded.

```toml
[mind.access]
files    = true    # browse/read/search/locate under roots
organize = true    # move/copy/rename/mkdir/sort (never overwrites, undoable)
programs = true    # list apps, check installed, open apps (with files)
commands = true    # run commands for a task (each one is a plan item)
skills   = true    # list_skills / read_skill / skill scripts via run
import_desktop_skills = true   # at start, propose copying ~/Desktop/skills into her skills folder (reviewed plan)
roots    = ["~", "/mnt", "/media", "/run/media"]
command_timeout_s = 30
```

`/proc`, `/sys`, `/dev` are never walked, and the secrets deny list above
still applies (`unlock_paths` re-opens a folder on purpose).

### The tools she knows about

Her system prompt gets a separate **"My hands on Ko's machine (tools)"**
section (`lillith-mind::capabilities`), built from these switches: a switched-
off group is not mentioned. The persona text itself is untouched. One tool per
reply; the tool's result is sent back to her as a `[tool result] … [/tool
result]` message so she can tell you what she found (up to 3 chained steps).

    ```tool
    locate ~ *.iso
    ```
    ```tool
    move ~/Downloads/report.pdf -> ~/Documents
    ```
    ```tool
    rename ~/Documents/scan1.pdf -> taxes-2026.pdf
    ```
    ```tool
    sort ~/Downloads by type      (or: by date)
    ```
    ```tool
    undo
    ```
    ```tool
    delete ~/Downloads/old.iso
    ~/Downloads/old-folder
    ```
    ```tool
    open kate -> ~/notes.md
    ```
    ```tool
    installed gimp
    ```
    ```tool
    run du -sh ~/Downloads
    ```
    ```tool
    read_skill tidy downloads
    ```

`run`/`exec`/`command` now mean the classified command runner; raw `shell`
is unchanged (still needs `allow_shell` and always asks).

### Organizing

- Never overwrites: a taken name becomes `name (2).ext`.
- Every batch is journaled in `~/.local/share/lillith/organize-journal/`.
  `lillith undo-organize` (or asking her to `undo`) reverses the newest batch:
  moves go back, copies go to the Trash, folders she made are removed only if
  empty. `lillith undo-organize --list` shows the journal.
- Every batch is shown as a plan first; only the items you approve run.

### Deleting (Soul Contract)

She can only *ask*. The contract lists **every** file and folder (full paths,
sizes, folder contents recursively) and the totals. You sign by typing the
phrase it shows (e.g. `DELETE 12 ITEMS`) and pressing **SIGN** in the bubble
(or typing it at the `lillith chat` prompt). Trash is the default;
**Permanent** must be picked explicitly. `/contract yes` is *not* a signature.

- The signature is for that exact list only. If any file changed, appeared or
  vanished before you signed, nothing is deleted and you get the new list.
- System paths (`/usr`, `/etc`, `/boot`, …), secrets folders, Lillith's own
  data, and the folders `/`, your home, `~/.config`, `~/Desktop`, … and mount
  points *themselves* are refused even when signed (files inside your own
  folders are fine).
- Every removed path is logged in `~/.local/share/lillith/deletions.log`;
  the Trash follows the freedesktop spec (restore from Dolphin).

### Commands and programs

Every command and launch is a plan item, even read-only ones; after you
approve, the output goes back to her. Writes, redirections, `sudo`/`pkexec`,
`kill`, `systemctl`, `chmod`, `mkfs` and unknown programs are marked
**risky** in the plan. Commands that remove files (`rm`, `find -delete`,
`git clean`, `rsync --delete`, …) are refused: deleting goes through the
contract. Everything is logged in `~/.local/share/lillith/commands.log`.

### Skills

Her skills folder (`[mind] skills_dir`, default `~/.local/share/lillith/skills`)
is listed with `list_skills` and read with `read_skill NAME`. Scripts in it
run through `run` (a plan item). Drop a folder named `skills`, `Skills` or
`lillith-skills` on your desktop (`xdg-user-dir DESKTOP`): at every start, via
`import_skills`, or `lillith import-skills [--from DIR]`, she proposes a plan
and only the copies you approve happen.
Same files are skipped, a differing file is kept next to hers as
`name (from desktop).md`, the desktop original is never touched, and each
import is logged in `skills-import.log`. The installer does the same.
