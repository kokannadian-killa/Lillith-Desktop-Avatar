# vdt progress (executor) — update after each milestone
## Kit spoken-red fix (DONE, installed into vdt-tmp/home)
- backup of original kit code: scratch/vdt-tmp/kit-backup-2254.tgz (kit is not a git repo)
- changed: lillith_voice/stt.py (fuzzy lone-red safeword, OOC phonetic forms, red x1-4, short-clip 0.8+0.4 s pad decodes,
  retry on empty/1-2-word non-safe non-affirmation: normalised 0.3 pad + spectral-subtraction denoise (+ tempo 0.92 if empty),
  safeword candidate wins, VAD-miss fallback, speech_s passed to classify); consent_ok always False
- added tests/safeword_test.py (43 cases PASS), wired into tests/e2e.sh
- corpus scratch/vdt-tmp/redcorpus: red-only 126 trials: old 93 missed -> new 6 (2 runs), 0/144 false stops
- kit e2e run1: all PASS except PTT '' (load flake; direct PTT test OK); run2: stt suite 9/9 PASS (interrupted after)
## TODO
- [x] tarballs rebuilt 23:46: /workspace/lillith/lillith-voice-kit.tar.gz (766 MB), lillith-voice-kit-nomodels.tar.gz (85 KB)
- [ ] 3a4bd2e e2e prebuilt: NO_BUILD=1 BIN=vdt-tmp/bin/lillith-3a4bd2e SKIP_TESTS=1 SKIP_WINCHECK=1 SLOW=2 (runner now targets target-vdt)
- [ ] verdict msgs (none sent yet: no SendToAgent; return drafts to parent)
- 3a4bd2e prebuilt run started 23:46, runner pid 4119933, log vdt-tmp/run-3a4bd2e-b.out
- [x] 3a4bd2e prebuilt e2e 23:47: PASS 30/0 (results vdt-tmp/results/3a4bd2e-234637); red_kill_switch PASS (bubble log present), voice yes PASS
- note: cargo test + windows check not rerun on 3a4bd2e (cargo test killed when target-shared retired); last: 5136ca4 140/0 + wincheck PASS
- [x] verdict drafts returned to parent in final report (none sent by me)

## voice-kit-src (2026-10-23 23:52 MDT)
- worktree wt-voice-kit-src, branch feat/voice-kit-src off e7f685c; kit in tools/lillith-voice, install.sh downloads opt-in, README updated, INSTALL.md + .gitignore added
- tests from repo copy: safeword 43/43, transport 15/15 ALL PASSED. Next: commit + push.
- committed 1b1c4b4 (47 files, 249,186 B). push: no creds (could not read Username). bundle: /workspace/lillith/feat-voice-kit-src.bundle. DONE.
## Task B 01:10: wt-voice-kit-src recreated; cherry-picked 6d258d5 onto 1b1c4b4 -> b594483 (6d258d5 sits on main line incl. 14 non-kit commits, so cherry-pick keeps branch kit-only)
- Task B DONE: tests netguard/chat 19/19 (new: v4/v6 bind blocks, chat temp .95/max 160), transport 15/15, safeword 43/43 (logs vdt-tmp/results/taskB). pushed feat/voice-kit-src 1b1c4b4..b594483 OK.
## Task A (main 3156237) started
- Task A daemon e2e started 01:13 (vdt-tmp/e2e/run_e2e.sh, out results/main-3156237/daemon-e2e); build waiting for <2 cargo (voicee2e-build.sh)
- run1 had a test bug (stop_utts read before all stop events arrived; lillith_ooc false FAIL). fixed, rerun -> results/main-3156237/daemon-e2e
- runner: WT/RESDIR overridable; SW_LEGS=1 adds app.safeword_{red,ooc}_{streaming,speaking}; stub 'slowly' mode (backup lib/stub.py.bak); installed lib synced to main kit (backup lib-backup-*.tgz)
- daemon e2e rerun DONE: 18/20 PASS. red+OOC(spelled) cut speech & drop queued utts in stt/ptt/always/always-nobarge; negs never stop. FAIL only literal TTS "OOC, stop." (bella/p1 say "ook" -> whisper "You'll stop."/"You'll start."); "You'll stop." is an explicit NEGATIVE case in safeword_test -> NOT adding it (would be a false stop on a normal sentence). No matcher fix.
- build OK 10m57s; bin vdt-tmp/bin/lillith-3156237; app run (SW_LEGS) started 01:30 -> results/main-3156237/app
- app run DONE: PASS 34/0 (results/main-3156237/app/summary.txt); app.safeword_* 4/4 PASS. next: cargo test in target-voicee2e
- cargo test (target-voicee2e) 470/0/2 ignored. summary: results/main-3156237/summary.txt. target-voicee2e DELETED 01:41. No fix branch (fix/voice-safeword-e2e created locally then removed, nothing committed).
## Task A DONE (PASS). Task B DONE (feat/voice-kit-src b594483 pushed).
## reprobe 39a8c17 (2026-10-24 06:06-06:25 MDT) DONE
- bin vdt-tmp/bin/lillith-39a8c17; results vdt-tmp/results/reprobe-39a8c17/{e2e,e2e-de-rerun,test-voice.log}; harness vdt-tmp/reprobe/reprobe-run.sh (standalone; vdt-run.sh untouched)
- 16 PASS / 2 FAIL: no WARN for daemon-down fallback (voice_warning never logged/shown); no fallback voice after daemon dies post-switch (prober exits). wt-reprobe + target-reprobe deleted.

## 2026-10-24 06:59 MDT reprobe retest ec9e1c5 (fix/reprobe-fallback)
- cargo test -p lillith-voice --features voice: 66/0. e2e as-built: FAIL 19/5 (c2,c3,c4,d,7b) - app VoiceRecovery ladder counts each daemon failover as engine error (speech.rs:838-840 -> main.rs:916-959): Hold ~30s then TextOnly on 2nd outage.
- local-only verify w/o that increment (bin/lillith-ec9e1c5-noerrcount, patch in results/reprobe-ec9e1c5/patched-verify): all pass (d flagged by harness expectation only).
- wt-reprobe + target-reprobe deleted.

## 2026-10-24 07:38 MDT reprobe retest 61e88cf: cargo test lillith-voice --features voice 71/0; e2e 29 pass + 2 harness-artifact FAILs (d timer race, 4c log race) both verified (d via e2e-d2 PASS, 4c from proxy.log). Holdoff 60/120/reset-60 PASS. wt-reprobe + target-reprobe deleted.
