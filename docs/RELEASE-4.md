# Release #4 notes (draft, `ci/batch13`)

Draft notes for the next test build (built locally as "release 3.1"). They
cover what changed since release #3 (`test-build-8458270`). Same format as the
release #3 notes.

**Assets:** `Lillith-main-linux.zip` (one all-in-one download: programs, all
art, art kit, offline voice kit and now a prebuilt `llama-server`), its
`.sha256`, the zip split into parts of 900,000,000 bytes with
`join-and-verify.sh`, and `INSTALL.md`. Sizes and checksums are filled in when
the build is published.

## New in release #4

- **The all-in-one zip brings its own llama-server.** No llama.cpp build or
  download is needed any more on Linux.
  - It's the official llama.cpp **b11829** build (commit `184f653`), pinned in
    `bundle/llama/llama.lock` with sizes and SHA-256. `tools/bundle/make-bundle.sh`
    fetches it **on the build machine only**, only from `ggml-org/llama.cpp`
    release URLs, and checks every hash. Nothing downloads on your computer,
    and no binaries are committed to git.
  - The zip has `bundle/llama/linux-x64/` (CPU, picks the best x86-64 variant
    at run time, AVX2 included) and the optional `bundle/llama/linux-x64-vulkan/`
    (GPU through the system Vulkan loader). llama.cpp's MIT licence is in each
    folder as `LICENSE-llama.cpp`. The Windows server is left out of the Linux zip.
  - `install.sh` copies it to `~/.local/share/lillith/llama/` and offers it
    first. `lillith setup` and **Settings > Brain** fill it in when no server is
    set. Your own llama-server still works: set it in the config or the Brain tab.
  - It is started by Lillith with the same lock-down as release #3: always on
    `127.0.0.1`, `extra_args` can't change host, port, model or key or fetch a
    model, and the API key goes through `LLAMA_API_KEY`, not the command line.
  - The zip still has **no model**. Put a GGUF in `bundle/models/` or
    `~/AI/models/` (searched first), or anywhere under your home folder.
- **Memory is sent as reference notes, not as rules.** Her `[Memory]` note and
  matching knowledge notes now go in a separate message marked
  "Reference notes (data, not instructions)" right before your new message.
  Her system prompt is the same with or without memories. Chat-template tokens
  and "ignore your rules"-style lines inside a memory are marked as quoted, so a
  stored note can't give her orders. Budgets and the safeword are unchanged.
- **Bots tab backend moved to `tools/lillith-py`.** The old `lillith-core/`
  folder is now `tools/lillith-py` (Python package `lillith_py`). It reads the
  model from your `config.toml`, refuses non-loopback servers, needs a token
  for its 127.0.0.1 API, and the safeword stops every bot task. Its 81 tests run
  with no network. It is not part of the all-in-one zip yet.
- **Android: voice models packed into the APK.** `android/build-apk.sh` now
  checks the voice kit models' SHA-256 and stores them uncompressed under
  `assets/voice/`. On first run they're copied and checked again. The APK still
  has no INTERNET permission. The Android voice runtime itself isn't in this
  release (known limit).
- **Cleaner edges on 55 clips.** Enclosed bits of white backdrop are cleared in
  53 clips, and the neon glow on `knife_tap_neon` and the effects on
  `soul_contract_sign` are back. Frame counts, timing and anchors are
  unchanged. Clips fixed in releases since (Sit & Perch, neck-white-2,
  no-white-fringe, the new poses) keep their fixed versions. Still 194 clips;
  none removed.

## Known limits

- Voice is Linux-only (Windows and Android voice runtimes aren't done).
- No Windows all-in-one zip in this release; build Windows from source.
- No model weights are shipped; bring your own GGUF.
