# lillith-core: local attachments and bots for Lillith

Part of DemonOS - Phantomcore. Everything runs on your machine: Python 3 stdlib, SQLite, and
optional poppler, tesseract and Pillow. The only HTTP traffic goes to llama-server on
`127.0.0.1:8080` and to the optional bot API on `127.0.0.1:8765`. Non-loopback URLs are
refused, proxy env vars are ignored, and nothing is downloaded or sent anywhere at runtime.

## Install (Garuda / Arch, fish)

```fish
cd lillith-core
./install.sh                 # pacman: poppler tesseract tesseract-data-eng python-pillow libnotify python-pytest
fish_add_path ~/.local/bin   # once
systemctl --user enable --now lillith-scheduler.timer   # routines (every minute)
systemctl --user enable --now lillith-api.service       # optional HTTP API for the GUI
```
`./install.sh --no-deps` skips pacman. Uninstall:
`rm -r ~/.local/share/lillith/core ~/.local/bin/lillith-{attach,bot}`.
Your data stays in `~/.local/share/lillith`.

## Attachments: `lillith-attach`

```fish
lillith-attach add ~/Documents/manual.pdf ~/Pictures/shot.png -c main
lillith-attach list                    # or: list -c main / --json list
lillith-attach show att_1a2b3c4d5e6f7a8b
lillith-attach read manual.pdf --offset 200 --limit 100    # negative offset = from the end
lillith-attach search "llama" -C 1     # -e regex, -s case-sensitive, -c conversation
lillith-attach chat "summarize this" -a ~/notes.md -c main  # --dry-run shows the messages
lillith-attach chat "what is in the picture?" -a shot.png --vision   # needs llama-server --mmproj
lillith-attach export manual.pdf ~/Desktop/ --with-text
lillith-attach detach main manual.pdf ; lillith-attach rm manual.pdf
lillith-attach verify
```

Storage (`~/.local/share/lillith/attachments/`):
`blobs/<aa>/<sha256>.<ext>` (read-only, deduped), `meta/<sha256>.json` (id, name, mime,
size, created, sha256, text_path, thumbnail_path, dimensions, pages, notes), `text/`,
`thumbs/`, and `lillith.sqlite` (conversation attachment lists and history). Every record
has a `uri` and `thumbnail_uri` (`file://`) that a GUI can display.

Extraction: text, code, JSON and CSV files are read directly, capped by `max_text_bytes`.
PDFs go through `pdftotext -layout`. For images, the store records dimensions (Pillow or a
stdlib header parser), makes a 256px PNG thumbnail (Pillow or ImageMagick), and runs
optional OCR (pytesseract or the `tesseract` CLI).

Chat injection: attachments go in the user turn as
`<attachment id=".." name=".." mime=".." size="..">…</attachment>`, and the system prompt
tells the model to treat their contents as untrusted data. If a file contains an
attachment tag, the tag is escaped so it can't close or fake a block. The `token_budget`
is split fairly: small files go in whole, large ones are cut at a line boundary with a
`[TRUNCATED … offset=N]` note.

Config file: `~/.config/lillith/attachments.json`. Keys: `llama_url`, `model`,
`token_budget`, `max_file_bytes`, `max_text_bytes`, `read_default_limit`,
`max_line_chars`, `thumb_size`, `ocr`, `vision`, `vision_max_bytes`, `system_prompt`.
Env overrides: `LILLITH_ATTACH_HOME`, `LILLITH_ATTACH_CONFIG`, `LILLITH_LLAMA_URL`.

Python API for the right-click GUI:
```python
from lillith_core.attachments import AttachmentStore, chat
st = AttachmentStore()
atts = st.add_uri_list(drop_event_text, conversation="main")   # KDE/Qt text/uri-list
st.add_bytes(clipboard_png, "paste.png", conversation="main")
st.read(atts[0].id, offset=1, limit=100); st.search("foo")
print(chat(st, "summarize", conversation="main")["reply"])
```

## Bots: `lillith-bot`

```fish
lillith-bot create Lillith --persona-file ~/lillith/persona.txt --avatar ~/lillith/avatar.png \
    --description "desktop companion" --temperature 0.7 --max-tokens 400
lillith-bot list ; lillith-bot show lillith
lillith-bot edit lillith --memory-budget 600 --tools search_memory,write_memory,read_attachment
lillith-bot chat lillith "hey"         # without a message: interactive chat (/attach PATH, /quit)
lillith-bot chat lillith "read this" -a ~/todo.md
lillith-bot history lillith -n 20      # --clear to wipe
lillith-bot memory add lillith "Ko uses fish shell"
lillith-bot memory journal lillith "Set up Phantomcore today"   # --day 2026-10-23
lillith-bot memory list lillith ; lillith-bot memory search lillith fish ; lillith-bot memory rm lillith fact 1
lillith-bot attach add lillith ~/notes.pdf ; lillith-bot attach list lillith ; lillith-bot attach rm lillith notes.pdf
lillith-bot routine add lillith morning "0 9 * * *" "Give Ko a short good-morning note"
lillith-bot routine list ; lillith-bot routine run 1 ; lillith-bot routine disable 1 ; lillith-bot routine rm 1
lillith-bot export lillith -o lillith.template.json   # add --with-facts to include facts, --no-avatar to leave out the avatar
lillith-bot import lillith.template.json --name lillith-2
lillith-bot scheduler --once           # what the systemd timer runs
lillith-bot serve                      # API on 127.0.0.1:8765
lillith-bot rm lillith-2               # --keep-files keeps its attachments
```

- **Profile:** the bot's settings are stored in SQLite and mirrored to
  `~/.local/share/lillith/bots/profiles/<name>.json`. They cover name, title, description,
  persona, avatar, endpoint (loopback only), model, sampling (temperature, top_p, top_k,
  max_tokens, repeat_penalty, stop), tools, and the token budgets for context, memory,
  attachments and history.
- **Prompt:** each request is built from the persona, the date, a `<memory>` block (facts
  first, then the newest journal entries, within `memory_budget`), tool instructions, the
  bot's attachments (within `attachment_budget`), and the newest history that fits
  `history_budget`.
- **Tools:** bots can only use these: `list_attachments`, `read_attachment`,
  `search_attachments`, `search_memory`, `write_memory`. A tool must be in both the built-in
  registry and the bot's `tools` list. Tools can only reach that bot's own attachments and
  memory. They have no file paths, shell or network. The model calls a tool by writing
  `<tool_call>{"name":…,"arguments":{…}}</tool_call>`; OpenAI-style `tool_calls` also work.
  A turn allows at most 3 rounds, and results come back fenced as `<tool_result>`.
- **Routines:** schedules use 5-field cron plus `@hourly`, `@daily`, `@weekly`, `@monthly`
  and `@yearly`, with names like `mon-fri` and `jan`. Each result is saved to the bot's chat
  history with source `routine:<name>` and shown with `notify-send`, using the bot's avatar
  as the icon.
- **Templates:** an export includes the profile, routines and an embedded avatar, but no
  history, timestamps, or facts unless you add `--with-facts`.

### HTTP API (loopback only)
The API only binds to 127.0.0.1 or ::1 and rejects requests with a non-loopback `Host` or
`Origin` header, which blocks DNS rebinding and browser CSRF. Set `LILLITH_API_TOKEN` to
also require a bearer token. The routes are listed in `lillith_core/bots/api.py`. Example:
```fish
curl -s localhost:8765/bots
curl -s -X POST localhost:8765/bots/lillith/chat -H 'Content-Type: application/json' -d '{"message":"hi"}'
curl -s -X POST localhost:8765/bots/lillith/attachments -H 'Content-Type: application/json' -d '{"path":"/home/ko/a.pdf"}'
```

### systemd --user units (`systemd/`)
- `lillith-scheduler.timer` + `lillith-scheduler.service`: one pass every minute
  (recommended).
- `lillith-scheduler-daemon.service`: a Python loop instead. Enable this one or the timer,
  not both.
- `lillith-api.service`: the HTTP API.

All units set `IPAddressDeny=any` / `IPAddressAllow=localhost`.

## Tests
```fish
python -m pytest -q tests       # uses tests/mock_llama.py, the model is not needed
python tests/mock_llama.py --port 8080   # a fake llama-server for manual GUI testing
```

## Limitations
- The 3B Dolphin model is text-only. Image understanding needs a multimodal GGUF plus
  `--mmproj` on llama-server and `"vision": true`. Without that, the model only gets image
  metadata and any OCR text.
- Token counts are estimated at about 4 characters per token, not measured with the
  model's tokenizer. Leave some headroom in the context size.
- Scanned PDFs have no text layer. OCR runs on images only, not on PDF pages.
- Small models follow the tool-call format inconsistently. Bad calls are ignored safely.
- Search is a linear substring or regex scan, which is fine for thousands of files. There
  is no semantic or vector search.
- Deleting an attachment removes it from every conversation, because identical files are
  stored once. Use `detach` to remove it from one conversation only.
