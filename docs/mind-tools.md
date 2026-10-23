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

## Gating

| Action  | Allowlisted path | Popup? | Audit line |
|---------|------------------|--------|------------|
| list / read / search | required | no, runs now | `AUTO` |
| write   | required | **always Yes/No** | `CONFIRM` / `DENY` |
| shell   | `allow_shell = true` | **always Yes/No** | `CONFIRM` / `DENY` |

Anything outside the allowlist is refused before the contract sees it. The
refusal goes back to the chat as a note.

## Config (`[mind]`)

```toml
[mind]
allow_paths    = ["~/Projects"]   # extra roots on top of config dir, knowledge_dir, skills_dir
allow_shell    = false            # shell is off by default; even when on, every command needs Yes
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
