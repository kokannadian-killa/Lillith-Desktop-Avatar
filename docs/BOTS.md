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
* Bots are supervised worker threads: **Stop**, **Stop all**, the kill switch and Ctrl-C /
  SIGTERM on `lillith-settings` stop them and any command they are running.
