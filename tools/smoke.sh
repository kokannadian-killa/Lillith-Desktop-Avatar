#!/usr/bin/env bash
# tools/smoke.sh: headless end-to-end smoke test of the Lillith overlay (Linux, X11).
#
# What it checks (exit 0 = pass, 1 = fail; the last line says why):
#   1. the overlay starts under a private Xvfb, against a stub llama-server
#      and a throw-away HOME / XDG_CONFIG_HOME (your real config is never read
#      or written), with voice, tray and system monitor off;
#   2. her window ("Lillith", class "lillith") is mapped within SMOKE_WAIT s;
#   3. the first drawn frame is not black: more than SMOKE_MIN_LIT (default
#      1%) of the pixels inside her window rect are non-black;
#   4. a right click on an opaque pixel of her opens the right-click menu:
#      on X11 that is a second override-redirect window titled "Lillith menu"
#      (crates/lillith-overlay/src/x11.rs, sync_menu); it must become viewable
#      and draw non-black pixels;
#   5. `lillith chat` gets the stub's reply from /v1/chat/completions
#      (SMOKE_CHAT=0 skips this).
# No compositor is needed: without one the X11 backend falls back to the
# shaped (bounding-shape) mode, which Xvfb supports. SMOKE_COMPOSITOR=1 also
# starts picom (xrender) to cover the 32-bit ARGB composited path.
#
# Dependencies (Debian/Ubuntu):
#   sudo apt-get install xvfb xdotool x11-utils ffmpeg python3 python3-pil
#   (x11-utils = xwininfo; ffmpeg does the x11grab screenshots; PIL counts pixels;
#    picom only for SMOKE_COMPOSITOR=1)
#
# Usage:  tools/smoke.sh            # builds `lillith` (cargo build -p lillith --locked)
#         SMOKE_BIN=/path/to/lillith tools/smoke.sh   # use a prebuilt binary, no cargo
#
# Env knobs:
#   SMOKE_BIN         binary to test (default: cargo build, then
#                     ${CARGO_TARGET_DIR:-target}/debug/lillith)
#   SMOKE_NO_BUILD=1  don't run cargo; use the default binary path as is
#   SMOKE_JOBS        cargo -j (default 2)
#   SMOKE_OUT         artifacts dir (default target/smoke): first-frame.png,
#                     menu.png, app.log, stub.log, xvfb.log, chat.log, result.txt
#   SMOKE_DISPLAY     use this existing X display instead of starting Xvfb
#                     (a plain $DISPLAY is ignored on purpose so the test never
#                     lands on your real desktop)
#   SMOKE_WAIT        seconds to wait for the window / first frame (default 20)
#   SMOKE_MIN_LIT     min fraction of non-black pixels in her rect (default 0.01)
#   SMOKE_MANIFEST    manifest.toml to draw (default: assets/generated/manifest.toml
#                     when its sheets are real PNGs, i.e. Git LFS is pulled; else a
#                     generated 1-clip test sheet)
#   SMOKE_SYNTHETIC=1 always use the generated test sheet
#   SMOKE_COMPOSITOR=1  also run picom (composited ARGB path)
#   SMOKE_CHAT=0      skip the `lillith chat` round trip
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
OUT="${SMOKE_OUT:-$ROOT/target/smoke}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
rm -f "$OUT"/*.png "$OUT"/*.log "$OUT"/result.txt
WAIT="${SMOKE_WAIT:-20}"
MIN_LIT="${SMOKE_MIN_LIT:-0.01}"
TMP="$(mktemp -d -t lillith-smoke.XXXXXX)"
PIDS=()

log() { printf '[smoke] %s\n' "$*"; }
finish() { # $1 = exit code, $2 = one-line reason
  printf '%s\n' "$2" >"$OUT/result.txt"
  if [ "$1" -ne 0 ]; then
    for f in app.log stub.log xvfb.log; do
      [ -s "$OUT/$f" ] && { printf -- '----- %s (last 20 lines) -----\n' "$f" >&2; tail -n 20 "$OUT/$f" >&2; }
    done
  fi
  printf '%s\n' "$2"
  exit "$1"
}
fail() { finish 1 "SMOKE FAIL: $*"; }
cleanup() {
  local p
  for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done
  sleep 0.3
  for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill -9 "$p" 2>/dev/null; done
  wait 2>/dev/null
  rm -rf "$TMP"
}
trap cleanup EXIT
trap 'fail "interrupted"' INT TERM

need() { command -v "$1" >/dev/null 2>&1 || fail "missing tool '$1' ($2)"; }
need xdotool "apt-get install xdotool"
need xwininfo "apt-get install x11-utils"
need ffmpeg "apt-get install ffmpeg"
need python3 "apt-get install python3"
python3 -c 'import PIL' 2>/dev/null || fail "missing python3 PIL (apt-get install python3-pil)"
[ -n "${SMOKE_DISPLAY:-}" ] || need Xvfb "apt-get install xvfb"

# ---------- binary ----------
BIN="${SMOKE_BIN:-}"
if [ -z "$BIN" ]; then
  BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/lillith"
  if [ "${SMOKE_NO_BUILD:-0}" != 1 ]; then
    log "cargo build -p lillith --locked -j ${SMOKE_JOBS:-2}"
    cargo build -p lillith --locked -j "${SMOKE_JOBS:-2}" >"$OUT/build.log" 2>&1 ||
      fail "cargo build failed (see $OUT/build.log)"
  fi
fi
[ -x "$BIN" ] || fail "no lillith binary at $BIN (set SMOKE_BIN or allow the build)"
log "binary: $BIN"

# ---------- manifest ----------
is_png() { [ -f "$1" ] && [ "$(head -c 4 "$1" | od -An -tx1 | tr -d ' \n')" = "89504e47" ]; }
MANIFEST="${SMOKE_MANIFEST:-}"
if [ -z "$MANIFEST" ]; then
  first_page="$(awk -F'"' '/^page *= *"/ {print $2; exit}' assets/generated/manifest.toml 2>/dev/null)"
  if [ "${SMOKE_SYNTHETIC:-0}" != 1 ] && [ -n "$first_page" ] && is_png "assets/generated/$first_page"; then
    MANIFEST="$ROOT/assets/generated/manifest.toml"
  else
    log "repo sheets are not real PNGs (Git LFS not pulled?): using a generated test sheet"
    MANIFEST="$TMP/art/manifest.toml"
    mkdir -p "$TMP/art"
    python3 - "$TMP/art" <<'PY' || fail "could not write the generated test sheet"
import sys
from PIL import Image, ImageDraw
out, W, H = sys.argv[1], 400, 400
im = Image.new("RGBA", (W * 2, H), (0, 0, 0, 0))
d = ImageDraw.Draw(im)
for i in range(2):  # an opaque pink body in each cell, transparent around it
    d.rectangle([i * W + 120, 60 + i * 4, i * W + 279, 391], fill=(230, 60, 140, 255))
im.save(f"{out}/idle_sheet.png")
with open(f"{out}/manifest.toml", "w") as f:
    f.write('[clips.idle]\npage = "idle_sheet.png"\nfps = 6.0\nmode = "loop"\nbaseline_y = 392\n')
    for i in range(2):
        f.write(f"\n[[clips.idle.frames]]\nx = {i * W}\ny = 0\nw = {W}\nh = {H}\nanchor = [200, 392]\n")
PY
  fi
fi
log "manifest: $MANIFEST"

# ---------- stub llama-server (127.0.0.1, free port, no model) ----------
REPLY="smoke-ok-$$"
python3 -u - "$TMP/port" "$REPLY" >"$OUT/stub.log" 2>&1 <<'PY' &
import json, os, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
port_file, reply = sys.argv[1], sys.argv[2]

class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def _send(self, code, ctype, body):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
    def do_GET(self):
        p = self.path.split("?")[0].rstrip("/")
        if p == "/health":
            self._send(200, "application/json", b'{"status":"ok"}')
        elif p == "/v1/models":
            self._send(200, "application/json", b'{"object":"list","data":[{"id":"local","object":"model"}]}')
        else:
            self._send(404, "application/json", b'{"error":"not found"}')
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        print("POST", self.path, body[:300].decode("utf-8", "replace"), flush=True)
        if self.path.split("?")[0].rstrip("/") != "/v1/chat/completions":
            return self._send(404, "application/json", b'{"error":"not found"}')
        try:
            req = json.loads(body or b"{}")
        except ValueError:
            return self._send(400, "application/json", b'{"error":"bad json"}')
        base = {"id": "chatcmpl-smoke", "created": int(time.time()), "model": req.get("model", "local")}
        if req.get("stream"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.send_header("Connection", "close")
            self.end_headers()
            half = len(reply) // 2
            chunks = [{"delta": {"role": "assistant", "content": ""}}, {"delta": {"content": reply[:half]}},
                      {"delta": {"content": reply[half:]}}, {"delta": {}, "finish_reason": "stop"}]
            for c in chunks:
                c.setdefault("finish_reason", None)
                c["index"] = 0
                ev = dict(base, object="chat.completion.chunk", choices=[c])
                self.wfile.write(("data: %s\n\n" % json.dumps(ev)).encode())
                self.wfile.flush()
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
            self.close_connection = True
        else:
            out = dict(base, object="chat.completion",
                       choices=[{"index": 0, "message": {"role": "assistant", "content": reply}, "finish_reason": "stop"}],
                       usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2})
            self._send(200, "application/json", json.dumps(out).encode())
    def log_message(self, fmt, *a):
        print("stub:", fmt % a, flush=True)

srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
open(port_file + ".tmp", "w").write(str(srv.server_address[1]))
os.rename(port_file + ".tmp", port_file)
srv.serve_forever()
PY
PIDS+=($!)
for _ in $(seq 50); do [ -s "$TMP/port" ] && break; sleep 0.1; done
[ -s "$TMP/port" ] || fail "stub LLM server did not start (see $OUT/stub.log)"
ENDPOINT="http://127.0.0.1:$(cat "$TMP/port")"
log "stub llama-server: $ENDPOINT"

# ---------- isolated HOME / XDG dirs + config ----------
# The app reads `[chat] endpoint` (base URL; /v1/chat/completions and /health
# are appended, see crates/lillith-mind/src/llm.rs) and the legacy `llm_url`.
# `[chat.server]` stays empty so it never spawns a real llama-server.
export HOME="$TMP/home" XDG_CONFIG_HOME="$TMP/home/.config" XDG_DATA_HOME="$TMP/home/.local/share" \
  XDG_CACHE_HOME="$TMP/home/.cache" XDG_STATE_HOME="$TMP/home/.local/state" XDG_RUNTIME_DIR="$TMP/run"
mkdir -p "$XDG_CONFIG_HOME/lillith" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
CFG="$XDG_CONFIG_HOME/lillith/config.toml"
cat >"$CFG" <<TOML
mode = "stay"
muted = true
voice_enabled = false
llm_backend = "llama-server"
llm_url = "$ENDPOINT"
autostart = false

[voice]
enabled = false
mute = true

[overlay]
auto_fix_kwin = false

[monitor]
enabled = false

[chat]
enabled = true
endpoint = "$ENDPOINT"
save_history = false
overlay_speech = true
timeout_s = 10.0
TOML
unset WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS PULSE_SERVER
export RUST_LOG="${RUST_LOG:-info}" RUST_BACKTRACE=1 NO_AT_BRIDGE=1

# ---------- X server ----------
if [ -n "${SMOKE_DISPLAY:-}" ]; then
  DISP="$SMOKE_DISPLAY"
  log "using existing display $DISP"
else
  DISP=""
  for n in $(seq 99 199); do
    [ -e "/tmp/.X11-unix/X$n" ] || [ -e "/tmp/.X$n-lock" ] || { DISP=":$n"; break; }
  done
  [ -n "$DISP" ] || fail "no free X display number in :99..:199"
  Xvfb "$DISP" -screen 0 1280x800x24 -br -nolisten tcp -noreset +extension Composite >"$OUT/xvfb.log" 2>&1 &
  PIDS+=($!)
  for _ in $(seq 100); do [ -e "/tmp/.X11-unix/X${DISP#:}" ] && break; sleep 0.1; done
  [ -e "/tmp/.X11-unix/X${DISP#:}" ] || fail "Xvfb did not start on $DISP (see $OUT/xvfb.log)"
  log "Xvfb on $DISP (1280x800x24)"
fi
export DISPLAY="$DISP"
read -r SW SH < <(xwininfo -root | awk '/Width:/{w=$2} /Height:/{h=$2} END{print w, h}')
[ -n "${SW:-}" ] || fail "cannot query the root window of $DISP"

if [ "${SMOKE_COMPOSITOR:-0}" = 1 ]; then
  need picom "apt-get install picom"
  picom --backend xrender --no-fading-openclose >"$OUT/compositor.log" 2>&1 &
  PIDS+=($!)
  sleep 1
  log "picom (xrender) started"
fi

grab() { # $1 = output png: full-screen capture
  ffmpeg -loglevel error -y -f x11grab -draw_mouse 0 -video_size "${SW}x${SH}" -i "$DISP" -frames:v 1 "$1"
}
# count.py PNG X Y W H -> "lit total" (lit = max(r,g,b) > 16) inside the rect
lit_in() {
  python3 - "$@" <<'PY'
import sys
from PIL import Image
path, x, y, w, h = sys.argv[1], *map(int, sys.argv[2:6])
im = Image.open(path).convert("RGB")
x0, y0 = max(x, 0), max(y, 0)
x1, y1 = min(x + w, im.width), min(y + h, im.height)
if x1 <= x0 or y1 <= y0:
    print(0, 0); sys.exit()
b = im.crop((x0, y0, x1, y1)).tobytes()
print(sum(1 for i in range(0, len(b), 3) if max(b[i], b[i + 1], b[i + 2]) > 16), (x1 - x0) * (y1 - y0))
PY
}

if [ -z "${SMOKE_DISPLAY:-}" ]; then
  grab "$TMP/empty.png" || fail "ffmpeg x11grab cannot capture $DISP"
  read -r l _ < <(lit_in "$TMP/empty.png" 0 0 "$SW" "$SH")
  [ "$l" -eq 0 ] || fail "the empty Xvfb screen is not black ($l lit pixels), the not-black check would prove nothing"
fi

# ---------- launch ----------
"$BIN" --config "$CFG" --manifest "$MANIFEST" --backend x11 --no-tray --no-voice --no-monitor \
  --no-save --reset-position --mode stay --exit-after $((WAIT + 40)) >"$OUT/app.log" 2>&1 &
APP=$!
PIDS+=($APP)
log "lillith started (pid $APP), log: $OUT/app.log"

alive() { kill -0 "$APP" 2>/dev/null; }
viewable() { xwininfo -id "$1" 2>/dev/null | grep -q 'Map State: IsViewable'; }
# window id(s) with exact title $1, viewable ones only
find_win() {
  local w
  for w in $(xdotool search --name "^$1\$" 2>/dev/null); do viewable "$w" && { echo "$w"; return 0; }; done
  return 1
}
geom() { # id -> "x y w h" (absolute)
  xwininfo -id "$1" | awk '/Absolute upper-left X:/{x=$4} /Absolute upper-left Y:/{y=$4} /Width:/{w=$2} /Height:/{h=$2} END{print x, y, w, h}'
}

WIN=""
deadline=$((SECONDS + WAIT))
while [ $SECONDS -lt $deadline ]; do
  alive || fail "lillith exited before mapping her window (see $OUT/app.log)"
  WIN="$(find_win Lillith)" && break
  sleep 0.25
done
[ -n "$WIN" ] || fail "no viewable 'Lillith' window within ${WAIT}s"
read -r WX WY WW WH < <(geom "$WIN")
log "window $WIN at ${WX},${WY} ${WW}x${WH}"

# ---------- first frame not black ----------
lit=0 tot=1
while :; do
  alive || fail "lillith exited before drawing (see $OUT/app.log)"
  grab "$OUT/first-frame.png" || fail "screenshot failed"
  read -r lit tot < <(lit_in "$OUT/first-frame.png" "$WX" "$WY" "$WW" "$WH")
  python3 -c "import sys; sys.exit(0 if $lit > $MIN_LIT * $tot else 1)" && break
  [ $SECONDS -lt $deadline ] || fail "first frame is black: $lit/$tot lit pixels in her window rect after ${WAIT}s (need > $MIN_LIT; $OUT/first-frame.png)"
  sleep 0.25
done
pct="$(python3 -c "print(f'{100 * $lit / $tot:.1f}')")"
log "PASS first frame: $lit/$tot pixels lit (${pct}%) -> $OUT/first-frame.png"

# ---------- right click -> menu ----------
# Click an opaque pixel: the overlay only takes clicks on her silhouette
# (Stage::hit). Pick the lit pixel nearest to the centroid of her lit pixels.
read -r CX CY < <(python3 - "$OUT/first-frame.png" "$WX" "$WY" "$WW" "$WH" <<'PY'
import sys
from PIL import Image
path, x, y, w, h = sys.argv[1], *map(int, sys.argv[2:6])
im = Image.open(path).convert("RGB").crop((x, y, x + w, y + h))
b = im.tobytes()
pts = [((i // 3) % w, (i // 3) // w) for i in range(0, len(b), 3) if max(b[i], b[i + 1], b[i + 2]) > 40]
if not pts:
    sys.exit(1)
mx = sum(p[0] for p in pts) / len(pts); my = sum(p[1] for p in pts) / len(pts)
bx, by = min(pts, key=lambda p: (p[0] - mx) ** 2 + (p[1] - my) ** 2)
print(x + bx, y + by)
PY
)
[ -n "${CX:-}" ] || fail "could not pick a pixel on her to right-click"
xdotool mousemove "$CX" "$CY" && sleep 0.4
xdotool click 3 || fail "xdotool click failed"
log "right-clicked at $CX,$CY"

MENU=""
mdead=$((SECONDS + 8))
while [ $SECONDS -lt $mdead ]; do
  alive || fail "lillith exited after the right click (see $OUT/app.log)"
  MENU="$(find_win 'Lillith menu')" && break
  sleep 0.2
done
[ -n "$MENU" ] || fail "right-click menu did not open: no viewable 'Lillith menu' window within 8s (app.log: $(grep -c 'right-click menu opened' "$OUT/app.log") 'menu opened' lines)"
read -r MX MY MW MH < <(geom "$MENU")
mlit=0 mtot=1
while [ $SECONDS -lt $mdead ]; do
  sleep 0.3
  grab "$OUT/menu.png" || fail "menu screenshot failed"
  read -r mlit mtot < <(lit_in "$OUT/menu.png" "$MX" "$MY" "$MW" "$MH")
  python3 -c "import sys; sys.exit(0 if $mlit > 0.05 * $mtot else 1)" && break
done
python3 -c "import sys; sys.exit(0 if $mlit > 0.05 * $mtot else 1)" ||
  fail "menu window $MENU is mapped but draws nothing: $mlit/$mtot lit pixels ($OUT/menu.png)"
log "PASS menu: override-redirect window $MENU at ${MX},${MY} ${MW}x${MH}, $mlit/$mtot pixels lit -> $OUT/menu.png"

# Escape closes it again (informational only).
xdotool key Escape 2>/dev/null
for _ in $(seq 20); do find_win 'Lillith menu' >/dev/null || break; sleep 0.1; done
find_win 'Lillith menu' >/dev/null && log "note: menu still open after Escape" || log "menu closed on Escape"
alive || fail "lillith exited after closing the menu (see $OUT/app.log)"

kill "$APP" 2>/dev/null
wait "$APP" 2>/dev/null

# ---------- chat round trip against the stub ----------
if [ "${SMOKE_CHAT:-1}" != 0 ]; then
  out="$(timeout 30 "$BIN" --config "$CFG" chat --no-history "smoke test: say hi" 2>"$OUT/chat.log")" ||
    fail "lillith chat exited non-zero against the stub (see $OUT/chat.log)"
  printf '%s\n' "$out" >>"$OUT/chat.log"
  grep -q "POST /v1/chat/completions" "$OUT/stub.log" || fail "lillith chat never reached $ENDPOINT/v1/chat/completions"
  printf '%s' "$out" | grep -q "$REPLY" || fail "lillith chat did not print the stub reply '$REPLY'"
  log "PASS chat: stub reply came back via POST /v1/chat/completions"
fi

finish 0 "SMOKE PASS: window mapped, first frame ${pct}% lit, right-click menu opened (artifacts: $OUT)"
