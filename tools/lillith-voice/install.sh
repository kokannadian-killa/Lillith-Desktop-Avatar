#!/usr/bin/env bash
# Lillith offline voice kit installer (Garuda / Arch). Run from any shell:  bash install.sh [options]
#
#   --pacman           OPT-IN: sudo pacman -S the system deps (downloads from the Arch mirrors). Default: off,
#                      only checks that python3 / ffmpeg / a player are already installed. (--no-pacman: accepted, no-op)
#   --wheels DIR       install Python packages from DIR only (no PyPI). Default: ./wheels if present
#                      (the full kit tarball ships wheels/ and models/; the git repo copy ships neither).
#   --online-pip       OPT-IN: fetch the pinned packages from PyPI (install-time only). Without --wheels/./wheels
#                      and without this flag the installer stops instead of touching the network.
#   --systemd          install AND enable+start the systemd --user daemon (default: install unit only)
#   --piper-shim       also install ~/.local/bin/piper -> Lillith's voice (lets wt-tts lillith-voice use it as-is)
#   --python PATH      python interpreter for the venv (default: python3)
#   -y, --yes          pass --noconfirm to pacman
#
# Nothing here ever downloads a model. After install, nothing makes network calls.
# Models (all bundled in ./models, checked against models/SHA256SUMS):
#   pocket/  Ko's cloned voice (Pocket TTS ONNX + voices/lillith.npy)  -> default preset 'lillith'
#   *.onnx/bin at top level: Kokoro (presets p1 / bella*, and the fallback if pocket/ is missing)
#   stt/     whisper tiny.en (int8 ONNX) + silero VAD  -> mic / lillith-listen / lillith-chat --mic
set -euo pipefail

KIT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LV_HOME="${LILLITH_VOICE_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/lillith-voice}"
BIN_DIR="$HOME/.local/bin"
CFG="${XDG_CONFIG_HOME:-$HOME/.config}"
MODELS=(kokoro-v1.0.onnx voices-v1.0.bin)

DO_PACMAN=0; WHEELS=""; ONLINE_PIP=0; DO_SYSTEMD=0; PIPER_SHIM=0; PY="python3"; YES=""
while [ $# -gt 0 ]; do
  case "$1" in
    --pacman) DO_PACMAN=1 ;;
    --no-pacman) DO_PACMAN=0 ;;
    --wheels) WHEELS="$2"; shift ;;
    --online-pip) ONLINE_PIP=1 ;;
    --systemd) DO_SYSTEMD=1 ;;
    --piper-shim) PIPER_SHIM=1 ;;
    --python) PY="$2"; shift ;;
    -y|--yes) YES="--noconfirm" ;;
    -h|--help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
[ -z "$WHEELS" ] && [ "$ONLINE_PIP" = 0 ] && [ -d "$KIT/wheels" ] && WHEELS="$KIT/wheels"

c_ok(){ printf '\033[32m✓\033[0m %s\n' "$*"; }
c_warn(){ printf '\033[33m!\033[0m %s\n' "$*"; }
die(){ printf '\033[31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

echo "== Lillith offline voice kit"
echo "   kit:     $KIT"
echo "   install: $LV_HOME"

# ---------------------------------------------------------------- 1. model files (checked first, never downloaded)
MODEL_SRC=""
for d in "$KIT/models" "$LV_HOME/models"; do
  if [ -f "$d/${MODELS[0]}" ] && [ -f "$d/${MODELS[1]}" ]; then MODEL_SRC="$d"; break; fi
done
if [ -z "$MODEL_SRC" ]; then
  cat >&2 <<MSG

✗ Kokoro model files not found. This installer never downloads them.
  Put these two files here:
      $KIT/models/kokoro-v1.0.onnx     (≈311 MiB, sha256 7d5df8ec…a6c5)
      $KIT/models/voices-v1.0.bin      (≈27 MiB,  sha256 bca610b8…bf7d)
  (or directly in $LV_HOME/models/), then re-run:  bash install.sh
  They are the same files as /workspace/lillith/voice_samples/ on the build box,
  i.e. the kokoro-onnx "model-files-v1.0" release assets.
MSG
  exit 1
fi
c_ok "model files found in $MODEL_SRC"
if [ -f "$MODEL_SRC/pocket/flow_lm_main.onnx" ] && [ -f "$MODEL_SRC/pocket/voices/lillith.npy" ]; then
  c_ok "cloned voice (Pocket TTS) found: default preset 'lillith'"
else
  c_warn "models/pocket/ missing: the default 'lillith' voice will fall back to preset p1 (Kokoro)"
fi
[ -f "$MODEL_SRC/stt/silero_vad.onnx" ] && c_ok "speech-to-text models found (whisper tiny.en + silero VAD)" \
  || c_warn "models/stt/ missing: microphone / lillith-listen disabled"

# ---------------------------------------------------------------- 2. system packages
if [ "$DO_PACMAN" = 1 ]; then
  if command -v pacman >/dev/null; then
    # ffmpeg (extra) is built with librubberband on Arch; pipewire-audio ships pw-play.
    PKGS=(python ffmpeg rubberband espeak-ng pipewire pipewire-audio pipewire-pulse)
    echo "== pacman: ${PKGS[*]}"
    sudo pacman -S --needed $YES "${PKGS[@]}"
  else
    c_warn "pacman not found; install python3, ffmpeg (with librubberband), espeak-ng, pipewire yourself"
  fi
fi

command -v "$PY" >/dev/null || die "$PY not found"
command -v ffmpeg >/dev/null || die "ffmpeg not found (sudo pacman -S ffmpeg, or re-run with --pacman)"
FILTERS="$(ffmpeg -hide_banner -filters 2>/dev/null || true)"
if grep -qE '^\s*\S+\s+rubberband\s' <<<"$FILTERS"; then
  c_ok "ffmpeg has the rubberband filter (formant-preserving pitch)"
else
  c_warn "ffmpeg lacks rubberband: lillith-say will use the pure-numpy pitch fallback (slightly less natural)"
fi
for b in pw-play paplay aplay; do command -v $b >/dev/null && { c_ok "audio player: $b"; break; }; done
command -v pw-play >/dev/null || c_warn "pw-play not found (pacman -S pipewire-audio); will fall back to paplay/aplay"

# ---------------------------------------------------------------- 3. venv + pinned packages
PYV=$("$PY" -c 'import sys;print(f"{sys.version_info[0]}.{sys.version_info[1]}")')
echo "== python $PYV -> venv $LV_HOME/venv"
mkdir -p "$LV_HOME"
[ -x "$LV_HOME/venv/bin/python" ] || "$PY" -m venv "$LV_HOME/venv"
VPY="$LV_HOME/venv/bin/python"
export PIP_DISABLE_PIP_VERSION_CHECK=1 PIP_NO_INPUT=1
PIP=("$VPY" -m pip install --quiet --only-binary=:all:)
if [ -n "$WHEELS" ]; then
  PIP+=(--no-index --find-links "$WHEELS"); c_ok "offline pip from $WHEELS"
elif [ "$ONLINE_PIP" = 1 ]; then
  c_warn "--online-pip: pip will fetch the pinned wheels from PyPI (install-time only, you asked for it)"
else
  die "no wheels: pass --wheels DIR (the full kit tarball has ./wheels), or --online-pip to fetch the pins from PyPI"
fi
REQ="$KIT/requirements.lock"
# kokoro-onnx 0.6.1 declares Requires-Python <3.14 but is pure Python and works on 3.14
# (tested on CPython 3.14.9 with these exact pins). Arch ships 3.14, so allow it explicitly.
if [ "$PYV" = "3.14" ] || [ "${PYV#3.}" -ge 14 ] 2>/dev/null; then
  grep -v '^kokoro-onnx' "$REQ" | grep -v '^#' > "$LV_HOME/.req-deps.txt"
  "${PIP[@]}" -r "$LV_HOME/.req-deps.txt"
  "${PIP[@]}" --no-deps --ignore-requires-python "$(grep '^kokoro-onnx' "$REQ")"
  rm -f "$LV_HOME/.req-deps.txt"
else
  "${PIP[@]}" -r "$REQ"
fi
c_ok "python packages installed (pinned)"

# ---------------------------------------------------------------- 4. code, models, launchers
mkdir -p "$LV_HOME/lib" "$LV_HOME/models" "$BIN_DIR"
rm -rf "$LV_HOME/lib/lillith_voice"
cp -r "$KIT/lillith_voice" "$LV_HOME/lib/"
find "$LV_HOME/lib" -name __pycache__ -prune -exec rm -rf {} +
if [ "$MODEL_SRC" != "$LV_HOME/models" ]; then
  (cd "$MODEL_SRC" && find . -type f ! -name SHA256SUMS) | while read -r m; do
    m="${m#./}"; mkdir -p "$LV_HOME/models/$(dirname "$m")"
    if ! cmp -s "$MODEL_SRC/$m" "$LV_HOME/models/$m" 2>/dev/null; then cp "$MODEL_SRC/$m" "$LV_HOME/models/$m"; fi
  done
fi
if [ -f "$KIT/models/SHA256SUMS" ]; then
  (cd "$LV_HOME/models" && sha256sum --quiet --ignore-missing -c "$KIT/models/SHA256SUMS") || die "model checksum mismatch in $LV_HOME/models"
  c_ok "model checksums OK"
fi
for b in lillith-say lillith-voiced lillith-chat lillith-piper lillith-listen; do install -m 755 "$KIT/bin/$b" "$BIN_DIR/$b"; done
c_ok "commands in $BIN_DIR: lillith-say lillith-voiced lillith-chat lillith-piper lillith-listen"

if [ "$PIPER_SHIM" = 1 ]; then
  if [ -e "$BIN_DIR/piper" ] && ! grep -q lillith "$BIN_DIR/piper" 2>/dev/null; then
    c_warn "$BIN_DIR/piper exists and is not ours; not overwriting"
  else
    install -m 755 "$KIT/bin/lillith-piper" "$BIN_DIR/piper"
    # marker "model" for the wt-tts crate: it only checks the file exists and reads sample_rate from .json
    # (file name kept for existing configs; it speaks the DEFAULT preset = Ko's cloned voice)
    echo "lillith-voice marker (real models: $LV_HOME/models)" > "$LV_HOME/lillith-kokoro.onnx"
    echo '{"audio": {"sample_rate": 24000}, "lillith_voice": "piper shim -> default preset (lillith)"}' > "$LV_HOME/lillith-kokoro.onnx.json"
    cp "$LV_HOME/lillith-kokoro.onnx" "$LV_HOME/lillith.onnx"; cp "$LV_HOME/lillith-kokoro.onnx.json" "$LV_HOME/lillith.onnx.json"
    c_ok "piper shim installed; set [voice] engine=\"piper\" model=\"$LV_HOME/lillith.onnx\""
  fi
fi

# ---------------------------------------------------------------- 5. fish + systemd
if [ -d "$CFG/fish" ] || command -v fish >/dev/null; then
  mkdir -p "$CFG/fish/conf.d"
  install -m 644 "$KIT/fish/lillith-voice.fish" "$CFG/fish/conf.d/lillith-voice.fish"
  c_ok "fish functions: say, say-p1, say-bella, say-fast, hush, lil, lil-talk, hear (open a new fish shell)"
fi
mkdir -p "$CFG/systemd/user"
install -m 644 "$KIT/systemd/lillith-voice.service" "$CFG/systemd/user/lillith-voice.service"
if command -v systemctl >/dev/null && systemctl --user daemon-reload 2>/dev/null; then
  if [ "$DO_SYSTEMD" = 1 ]; then
    systemctl --user enable --now lillith-voice.service && c_ok "daemon enabled + started (systemctl --user status lillith-voice)"
  else
    c_ok "systemd unit installed (enable: systemctl --user enable --now lillith-voice)"
  fi
fi

# ---------------------------------------------------------------- 6. self-test (offline, no playback)
echo "== self-test"
OUT="$(mktemp -t lillith-test-XXXX.wav)"
if LILLITH_PLAYER=none "$BIN_DIR/lillith-say" --no-daemon -v -o "$OUT" "Hiii, it's me. Like, I totally work offline now." 2>"$OUT.log"; then
  c_ok "rendered $(du -h "$OUT" | cut -f1) test WAV: $(tail -1 "$OUT.log" | "$VPY" -c 'import json,sys;d=json.loads(sys.stdin.read());print("%.1fs audio, RTF %.2f, model load %.1fs" % (d["audio_s"], d["rtf"], d["load_s"]))')"
  if [ -f "$LV_HOME/models/stt/silero_vad.onnx" ]; then
    HEARD="$("$BIN_DIR/lillith-listen" --no-daemon --wav "$OUT" 2>/dev/null || true)"
    [ -n "$HEARD" ] && c_ok "speech-to-text round trip heard: \"$HEARD\"" || c_warn "STT self-test produced no text"
  fi
  rm -f "$OUT" "$OUT.log"
else
  cat "$OUT.log" >&2; die "self-test failed"
fi
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) c_warn "$BIN_DIR is not on PATH (fish: fish_add_path ~/.local/bin)";; esac
echo
echo "Done. Try:   lillith-say \"Hiii babe, I missed you\""
echo "Presets:     lillith-say --list      (default: lillith = Ko's cloned voice)"
echo "Speed check: lillith-say --bench      (if RTF > 0.9: LILLITH_VOICE_PRESET=lillith_fast)"
echo "Microphone:  lillith-listen --setup   then  lillith-chat --ptt   or   lillith-chat --mic"
