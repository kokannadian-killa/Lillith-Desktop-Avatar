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
* **Stop all**, Ctrl-C / SIGTERM on `lillith-settings`, Ctrl-C in `lillith chat` / `lillith dream`,
  and the overlay's Ctrl-C / SIGTERM handler (installed only when `[chat.server]` spawning is
  configured; otherwise a signal just ends the overlay) are the **cross-process kill switch**: the stopping process
  appends to the per-user stop barrier (`$XDG_RUNTIME_DIR/lillith/stop/barrier.log`, dir 0700,
  files 0600; local files only, no socket or network) and fsyncs it *before* reporting anything.
  Every Lillith process that joined the stop session at start-up (the overlay / `lillith` CLI and
  `lillith-settings` do) sees it within ~20–50 ms: its command runners kill their process
  groups, its bots drop their plans and queued steps (nothing approved earlier is replayed; new
  work needs a new plan and a new mouse Yes), its stop hooks run, and it acknowledges.
  The Settings window says **Stopped** only when every live Lillith process confirmed its task
  trees are gone; otherwise it says the stop was saved but **not confirmed** (a process that did
  not answer, a tree still alive, or an OS where trees can't be checked). "Sent" / "accepted" is
  never shown as stopped. A restarted process ignores old barrier entries for new work and has
  nothing to resurrect (bot plans are not persisted).
* Limits: a command's descendant that leaves its process group (`setsid`, double fork) is not
  tracked or killed by a stop. If a Lillith process is killed with SIGKILL while a command runs,
  that command's tree is not torn down. Tree verification needs `/proc` (Linux); on Windows the
  Job-object kill still runs but the stop is reported as not confirmed.
* The safeword ("red" / "OOC: stop") only hushes her speech and queued lines (and cancels cards
  waiting for Ko) in the process where it was said. It never writes the barrier and never stops
  a bot, a command or any task.
