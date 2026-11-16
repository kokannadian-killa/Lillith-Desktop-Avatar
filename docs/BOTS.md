# Bots tab: Lillith's local helper bots

Open it with **Settings ▸ Bots** (`lillith-settings --tab bots`, `--step <bot id|name|new|settings>`).
The left column lists the bots (avatar, name, role, status: idle / working / needs approval /
done / failed / stopped / off), **+ New bot** (from a template), **Stop all** (the kill switch),
and the dream-learning status. Pick a bot to give it a job and watch its task log and the
files it made, or open **Settings** to set its name, description, on/off, model and allowed folders.

| Bot (rename freely) | Role | What it may propose |
|---|---|---|
| Mammon | File Organizer | an organize plan (mkdir / move / copy, journaled, `lillith undo-organize`) |
| Belphegor | Program Builder | scaffold a project from a template, build it, write files, run a command |
| Paimon | Web Page Builder | write `index.html` / `style.css` (and other files) |
| Baal | OS Builder (DemonOS / Phantomcore respin) | write an archiso profile (package list, configs, `build.sh`); running it is a separate gated step |
| Andras | Red Team: **your own machine and projects only** | a read-only local audit (permissions, setuid, secret-looking file names, listening ports) written as a report; refuses URLs, public IPs and domains |
| Naamah | Lillith Pose Builder | run the video pose import pipeline on your videos; candidates land in Pose Builder for approval |

The roster lives in `bots.toml` next to `config.toml` (Save in the bottom bar writes it).

## Safety rules

* Every write, command or launch is one step on the **Soul Contract card**: it only runs after a
  **mouse click on Yes**. Enter, Space, typed or spoken "yes" never approve; Esc / No declines.
  The audit log gets `READ` (job planned), `BOT_STEP` + `CONFIRM` (approved) or `DENY` / `CANCELLED`.
* Bots only touch paths inside their allowed folders (`..` and symlinks out are refused), and only
  the operations their kind allows; anything else is dropped before the card and shown as refused.
* Planning uses Lillith's local model on loopback (127.0.0.1) or a built-in recipe. No network.
  Online research only exists in Memory Engine's Ko-gated Tor dream sessions; the Bots tab
  only shows that session's status (`DreamStatusSource` hook) and never starts one.
* Bots are supervised worker threads inside the `lillith-settings` process (a separate,
  detached process from the overlay). A bot's **Stop** ends that bot; if it was running a
  command, every command of this Settings window is killed (process-local, the other Lillith
  processes keep going).
* **Stop all**, Ctrl-C / SIGTERM on `lillith-settings`, Ctrl-C / SIGTERM on the overlay (always,
  with or without `[chat.server]`), Ctrl-C in `lillith chat` / `lillith dream` are one route: the
  **cross-process kill switch** (`kill_switch::stop_all`). The stopping process writes a
  `disarmed` marker and appends to the per-user stop barrier
  (`$XDG_RUNTIME_DIR/lillith/stop/barrier.log`; dir 0700, files 0600; local files only, no
  socket or network), fsyncing both *before* reporting anything. Every Lillith process joins
  this stop session on first use and sees the barrier within ~20–50 ms: its command runners kill
  their process group **and** every descendant they have seen (also ones that left the group
  with `setsid`), its bots drop their plans and queued steps, its stop hooks run, and it
  acknowledges whether its trees are really gone.
* **Crashed Lillith processes.** Each joined process keeps its entry in the stop directory
  (`proc-<pid>`, 0600, rewritten atomically and fsynced) up to date with the command trees it
  runs: each process group with its leader's start time and every descendant (pid + start time)
  its ~200 ms `/proc` snapshots found. If that process is SIGKILLed or crashes, the next stop
  (from any Lillith process) SIGKILLs those groups, the recorded descendants, their own
  descendants and the groups they moved to, then checks they are gone. Only then is the dead
  process's entry removed. Pid reuse: a pid is only signalled while its start time
  (`/proc/<pid>/stat` field 22) still matches the recorded one, and a group only while its
  leader pid is gone or still has the recorded start time.
* **Reporting.** `stop_all` waits (at most 5 s) for those acknowledgements. Settings and chat say
  **Stopped** only when every live Lillith process confirmed the command trees it *tracked* are
  gone and every tree a crashed Lillith process left behind was killed and verified gone
  (Quiescent; the line says "tracked", see the gaps below). Otherwise the line says the stop was
  saved but **not confirmed** (a process that did not answer, a tree still alive, a crashed
  process's tree that survived the kill or whose entry / `/proc` can't be read, or an OS where
  trees can't be checked), or that it only applied in this process (barrier not saved).
  "Sent" / "accepted" is never shown as stopped.
* **Disarmed until Re-arm.** After a global stop, every Lillith process refuses to start any
  command, including processes started later (restarts, a new overlay after Ctrl-C), until Ko
  clicks **Re-arm** in Settings ▸ Bots (a mouse click only; Tab + Enter never re-arms). Re-arm only removes the marker: it never re-runs,
  resumes or re-approves anything (stopped plans and queued steps are gone; each new step still
  needs its own mouse Yes). `$XDG_RUNTIME_DIR` is cleared at logout / reboot, so a reboot also
  clears the barrier and the disarm.
* **Who joins.** Every release-built Lillith process joins. `LILLITH_STOP_SESSION=off` keeps a
  process out of the session (stops stay process-local) **in debug builds only**; a release
  build ignores it and logs a warning. Under `cargo test` / `cargo run` a process only joins in
  a fully scratch XDG environment (all `XDG_*` base dirs set, runtime dir in the same private
  tree as the state dir), so test runs never disarm a live Lillith. That also means a **dev
  `cargo run` with your normal XDG dirs does not join the stop session**: Stop all / Ctrl-C in
  Settings does not reach it, and its own Ctrl-C stays process-local. Use the installed
  (release) build, or give the dev run scratch XDG dirs, when you need the cross-process stop.
* **Known gaps (honest):**
  - *Double fork between snapshots:* runners record descendants by a `/proc` parent walk about
    every 200 ms and again at kill time. A process that double-forks and loses its parent link
    between two snapshots (and also left the process group) is not found and survives the stop.
    The acknowledgement only covers what was tracked, so the stop can still read as
    **Stopped** ("the command trees they tracked are gone"); that wording is deliberate.
  - *SIGKILLed / crashed Lillith:* the command trees it recorded are no longer torn down at the
    moment it dies, only at the **next** stop (Stop all / Ctrl-C / kill switch from any Lillith
    process), which kills and checks them as described above. Until then they keep running.
    What the dead process never recorded is not covered: a descendant that appeared after its
    last ~200 ms snapshot (and left the group), or a command it spawned in the instant before
    the entry was rewritten. If a recorded tree survives the kill, or the entry can't be read
    (corrupt, written by an older Lillith that did not record trees) or `/proc` is unavailable,
    every later stop says **not confirmed** and the entry stays until it can be checked (a
    reboot / logout clears `$XDG_RUNTIME_DIR`).
  - *Background pipe holder on a normal exit:* when a command exits but a background child keeps
    its output pipe open, the runner keeps waiting for the pipe (as before). A stop now ends
    that wait and kills the holder, but without a stop the wait has no time limit.
  - *Windows:* the Job-object kill still runs, but trees can't be checked without `/proc`, so a
    stop is always reported as **not confirmed** there.
* The safeword ("red" / "OOC: stop") only hushes her speech and queued lines (and cancels cards
  waiting for Ko) in the process where it was said. It never writes the barrier and never stops
  a bot, a command or any task.
