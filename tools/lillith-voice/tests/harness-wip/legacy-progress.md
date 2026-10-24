# legacy-launches verify progress (executor) started Sat Oct 24 05:09:49 MDT 2026
- 05:11:19 kit ae1fb83 + 9f6c899 tools/lillith-voice installed offline into /workspace/lillith/scratch-legacy-tmp/home (install.log OK, self-test STT round trip OK)
- 05:15:38 TASK 3 DONE: fix/voice-textnorm 648277e0deab1df0f0d2f9703043c37cd972f07e pushed (off origin/main 5ac283e). textnorm 20/20 (baseline 6 FAIL), safeword 43/43, transport 15/15 (scratch/legacy-results/textnorm). kit synced (backup scratch/textnorm.py.kit-pre-648277e.bak). worktree wt-textnorm (sparse).
- 05:17:18 TASK 4 DONE: feat/android-voice-assets 1651c6eaf88d37296f82725b1ab89c1bd3b77498 pushed (b03ca7d..). STAGE_ONLY default: voice 8 files 400,624,007 B, assets total 869,414,108 B; STT=1: 12 files 504,895,419 B / 973,685,520 B. logs scratch/legacy-results/android-stage
- 05:19:49 win payload c24106598226 unpacked to scratch-legacy-tmp/win, overlaid 9f6c899 lillith_voice/*.py + tests + PROTOCOL.md, SHA256SUMS regenerated (temp copy only)

## Update (end of run)
- T1 safety run2: ALL PASSED (safety/safety.log): hung card -> timeout 70.09 s, helper killed, no mic; PTT red / OOC corpus (lillith+bella) -> safeword, stop reason=safeword, queue dropped, consent_ok false; audit clean.
- T1 race: safety/race.log FAIL -> PTT released while card pending + late Yes => mic left running (stt.py:524-545). Real bug.
- T1 wine (win/): win_smoke ALL PASSED, transport ALL PASSED, safeword 43/43; micgate_test 76 pass / 8 POSIX-only portability fails (not product). resolve.log: bundled ffmpeg.exe + rubberband resolved, default player None->winsound, relative overrides refused.
- T2: voice-tests.sh 6003/0, --smoke PASS (corr 1.000000). Manifest perms: no INTERNET/RECORD_AUDIO/NETWORK_STATE; AAR manifest not passed to aapt2. With 648277e kit: 229 fails -> TextNorm.java needs port.
- Cleanup: wt-andrt-7ae9f7a removed; win/out removed. Kept kit/home/win/e2e for rerun on new SHA.

## fix/android-textnorm (05:34 task)
- 4941f77a38901acdb3f58e9fdf7a14a27e7df948 pushed (on 3ddd1d0 = cherry-pick of 648277e, off origin/main 4f5aa11).
- voice-tests.sh 6243/0, --smoke PASS; negative control (old TextNorm.java) 266 fails incl. 6 pinned. Logs: legacy-results/android-textnorm/.
- wt-andtn removed; scratch-legacy-tmp kept.

## fix/ptt-per-press @ bd0473e retest (05:42-06:03 MDT)
- kit overlay + reinstall OK. Unit: micgate 115/0, ptt_race 18/0, safeword 43/0, transport 15/0, textnorm 14/20 (fix 648277e not on this branch).
- e2e NETNS x4: 27/1 each, different flaky check each time (VAD listen-once; lillith_fast LUFS; lipsync stop event x2 -> test race lipsync_sub.py:81, proven by lipsync_dbg). All bd0473e checks pass every run.
- race_test (extended) 27/27 ALL PASSED; safety run 39/39 ALL PASSED; wine win_smoke 15/0, transport 16/0, safeword 43/0 (micgate 9 + ptt_race POSIX-only fails).
- BUG: token from mic_card Yes survives stop reason=safeword -> later press opens mic (tok.log). stt.py:587-593, daemon.py:75-80, micgate.py:139/217.
- scratch-legacy-tmp KEPT (not all passed).

## fix/ptt-per-press @ 20e7d56 final retest (2026-10-24 ~06:20 MDT) — PASS
- tok_test (revoke cases) 31 PASS / 0; race 27/27; safety 39/39; lipsync_sub.py 8/8 LIPSYNC OK; e2e NETNS=1 28/0
- unit: micgate 118/0, ptt_race 26/0, safeword 43/0, transport 15/0
- wine: win_smoke 16/0, transport 17/0, safeword 43/0; micgate 9 fail + ptt_race fail = same POSIX-only set as bd0473e
- logs: scratch/legacy-results/ptt-20e7d56/

## fix/mouse-only-followups @ 3cff41d retest (2026-10-24 09:05-09:40 MDT): HOLD. All requested checks pass; BUG: bubble exit while the card waits + later Yes = orphaned hot mic (voice_mic.rs:116-123, stt.py:530-566). See legacy-results/mouse-3cff41d/summary.txt

## 2026-10-24 11:11 MDT fix/mic-owner-gone @ 9185665: HOLD
Owner-gone fix works (0 mics on quit/drop/kill, hold, cap, Stuck, Drop bound pass). N1 fails: Mic::hush races its listen stop (voice_mic.rs:395-399/389; stt.py:648) -> pre-safeword speech broadcast to subscribers 5/10; D2 in-flight same root. Details: /workspace/lillith/scratch/legacy-results/mic-owner-9185665/summary.txt. wt+target deleted.

## mic-owner-gone @ cee4653 (2026-10-24) — PASS
hush_race 25/25, owner 38/0, X1 stress 16/0, close_test 100/0, GUI 29/2 (harness-only), all suites green; see legacy-results/mic-owner-cee4653/summary.txt. Worktree/target removed.
