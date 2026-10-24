# Windows voice packaging: progress (executor)
Updated 2026-10-24 04:10 MDT

- Worktree /workspace/lillith/wt-voice-win, branch feat/voice-windows off origin/main cf51783.
- DONE: build-voice-win.sh + voice-win.lock.json, core.py/stt.py Windows fixes, winlaunch.py (--ping/--stop/--start-app),
  tests/win_smoke.py, .cmd launchers, NSIS SecVoice (!ifdef VOICE) + uninstall, windows/build.sh VOICE_PAYLOAD, docs.
- DONE: wine smoke (netns off): smoke1.log ALL PASSED; smoke2.log transport 16/16 + Run-cmd ping/stop; --start-app OK.
- DONE: final payload c24106598226 -> /workspace/lillith/release/lillith-voice-win-c24106598226.zip
  779931989 bytes, sha256 b6d7662b2911b291353ab3a7b02934beed5285eb8f9eb5a7c4a0016a12f1120a
- RUNNING: makensis full installer (dummy exes, real assets+voice) -> scratch/voicewin/dist/ (log nsis-build.log), started 03:54.
- DONE: pushed feat/voice-windows a446db9 (a446db943cfd776b16a65c719901d34828303cdf).
- TODO: wine /S install test of setup.exe; cleanup (out/voice keep until install test, tgt, dist, wheels-3.13, /tmp/sigv, ucrt, vcrt); report.
- makensis OK 18.5 min: dist/Lillith-0.0.1-setup.exe 1140034693 B, portable zip 1248407315 B. Removed wineprefix voice symlink; next: wine /S install test.
- DONE 04:30: wine /S install test (install-test.log, install-test2.log): files, HKCU Run, VoiceDir, shortcuts, daemon autostarted by installer,
  reinstall over running daemon, --start-app Startup lnk, uninstall removes all + Run entry. Portable zip has Lillith/voice + Start cmd.
- Cleaned: dist, tgt, out, wheels-3.13, vcrt, sigstore venv/wheels. Kept: cache/ (254M, pinned downloads), wineprefix/ (test prefix w/ native ucrtbase), ucrt/.
- Remaining: final report only.
