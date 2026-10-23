#!/usr/bin/env bash
# Headless smoke test for the Lillith overlay (Linux / X11).
#
#   1. starts a stub llama-server on 127.0.0.1 (python3, loopback only,
#      no model) that answers GET /health and POST /v1/chat/completions;
#   2. starts the overlay under Xvfb (--backend x11, no tray/voice/monitor),
#      grabs the first frame with ffmpeg x11grab and fails if it is all black;
#   3. runs `lillith chat` against the stub and fails unless the stub's
#      reply comes back.
#
# Needs: Xvfb, ffmpeg (x11grab), python3, a built `lillith` binary.
#
# Env knobs:
#   LILLITH_BIN    binary to test (default: build with cargo, then
#                  $CARGO_TARGET_DIR or ./target /debug/lillith)
#   SMOKE_NO_BUILD=1  never run cargo (fail if LILLITH_BIN is missing)
#   SMOKE_MANIFEST manifest.toml to draw (default: assets/generated/manifest.toml
#                  when its sheets are real PNGs, else a generated 1-clip test sheet)
#   SMOKE_SYNTHETIC=1  always use the generated 1-clip test sheet
#   SMOKE_WAIT     seconds to wait for the first frame (default 30)
#   SMOKE_KEEP=1   keep the temp dir (frame.png, logs) for inspection
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
WAIT="${SMOKE_WAIT:-30}"
TMP="$(mktemp -d -t lillith-smoke.XXXXXX)"
PIDS=()

log()  { printf '[smoke] %s\n' "$*"; }
fail() { printf '[smoke] FAIL: %s\n' "$*" >&2; dump_logs; exit 1; }
dump_logs() {
  for f in "$TMP"/app.log "$TMP"/stub.log "$TMP"/xvfb.log; do
    [ -s "$f" ] && { printf '\n----- %s (last 30 lines) -----\n' "${f##*/}" >&2; tail -n 30 "$f" >&2; }
  done
  return 0
}
cleanup() {
  for p in "${PIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null || true; done
  wait 2>/dev/null || true
  if [ "${SMOKE_KEEP:-0}" = 1 ]; then log "kept $TMP"; else rm -rf "$TMP"; fi
}
trap cleanup EXIT

for t in Xvfb ffmpeg python3; do
  command -v "$t" >/dev/null || fail "missing tool: $t (apt-get install xvfb ffmpeg python3)"
done

# ---------- binary ----------
BIN="${LILLITH_BIN:-}"
if [ -z "$BIN" ]; then
  BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/lillith"
  if [ "${SMOKE_NO_BUILD:-0}" != 1 ]; then
    log "cargo build -p lillith --locked"
    cargo build -p lillith --locked || fail "cargo build failed"
  fi
fi
[ -x "$BIN" ] || fail "no lillith binary at $BIN (set LILLITH_BIN or allow the build)"

# ---------- manifest ----------
is_png() { [ -f "$1" ] && [ "$(head -c 4 "$1" | od -An -c | tr -d ' ')" = "211PNG" ]; }
MANIFEST="${SMOKE_MANIFEST:-}"
if [ -z "$MANIFEST" ]; then
  first_page="$(awk -F'"' '/^page = "/ {print $2; exit}' assets/generated/manifest.toml 2>/dev/null || true)"
  if [ "${SMOKE_SYNTHETIC:-0}" != 1 ] && [ -n "$first_page" ] && is_png "assets/generated/$first_page"; then
    MANIFEST="$ROOT/assets/generated/manifest.toml"
  else
    log "using a generated 1-clip test sheet (repo sheets not checked out, or SMOKE_SYNTHETIC=1)"
    MANIFEST="$TMP/art/manifest.toml"
    mkdir -p "$TMP/art"
    python3 - "$TMP/art" <<'PY' || fail "could not write the test sheet"
import struct, sys, zlib
out = sys.argv[1]
W = H = 400
def png(path, frames):
    w = W * frames
    rows = bytearray()
    for y in range(H):
        rows.append(0)
        for x in range(w):
            fx = x % W
            # an opaque pink "body" in the middle of each cell, transparent around it
            inside = 120 <= fx < 280 and 60 <= y < 392
            rows += bytes((230, 60, 140, 255)) if inside else bytes(4)
    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    data = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, H, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes(rows), 6)) + chunk(b"IEND", b""))
    open(path, "wb").write(data)
png(f"{out}/idle_sheet.png", 2)
with open(f"{out}/manifest.toml", "w") as f:
    f.write('[clips.idle]\npage = "idle_sheet.png"\nfps = 6.0\nmode = "loop"\nbaseline_y = 392\n')
    for i in range(2):
        f.write(f'\n[[clips.idle.frames]]\nx = {i*W}\ny = 0\nw = {W}\nh = {H}\nanchor = [200, 392]\n')
PY
  fi
fi
log "manifest: $MANIFEST"

# ---------- stub llama-server (loopback only) ----------
REPLY="smoke-ok-$$"
python3 - "$TMP/port" "$REPLY" >"$TMP/stub.log" 2>&1 <<'PY' &
import json, sys
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
        if self.path.rstrip("/") == "/health":
            self._send(200, "application/json", b'{"status":"ok"}')
        else:
            self._send(404, "text/plain", b"not found")
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        print("POST", self.path, body[:200].decode("utf-8", "replace"), flush=True)
        if self.path != "/v1/chat/completions":
            return self._send(404, "text/plain", b"not found")
        try:
            req = json.loads(body or b"{}")
        except ValueError:
            return self._send(400, "text/plain", b"bad json")
        if req.get("stream"):
            parts = [reply[: len(reply) // 2], reply[len(reply) // 2 :]]
            sse = "".join("data: %s\n\n" % json.dumps({"choices": [{"delta": {"content": p}, "finish_reason": None}]}) for p in parts)
            sse += "data: %s\n\ndata: [DONE]\n\n" % json.dumps({"choices": [{"delta": {}, "finish_reason": "stop"}]})
            self._send(200, "text/event-stream", sse.encode())
        else:
            out = {"choices": [{"index": 0, "message": {"role": "assistant", "content": reply}, "finish_reason": "stop"}]}
            self._send(200, "application/json", json.dumps(out).encode())
    def log_message(self, fmt, *a):
        print("stub:", fmt % a, flush=True)
srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
open(port_file + ".tmp", "w").write(str(srv.server_address[1]))
import os; os.rename(port_file + ".tmp", port_file)
srv.serve_forever()
PY
PIDS+=($!)
for _ in $(seq 50); do [ -s "$TMP/port" ] && break; sleep 0.1; done
[ -s "$TMP/port" ] || fail "stub LLM server did not start"
PORT="$(cat "$TMP/port")"
ENDPOINT="http://127.0.0.1:$PORT"
log "stub llama-server on $ENDPOINT"

# ---------- isolated config / data dirs ----------
export HOME="$TMP/home" XDG_CONFIG_HOME="$TMP/home/.config" XDG_DATA_HOME="$TMP/home/.local/share" \
       XDG_CACHE_HOME="$TMP/home/.cache" XDG_RUNTIME_DIR="$TMP/run"
mkdir -p "$XDG_CONFIG_HOME/lillith" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
CFG="$XDG_CONFIG_HOME/lillith/config.toml"
cat >"$CFG" <<TOML
[chat]
enabled = true
endpoint = "$ENDPOINT"
save_history = false
timeout_s = 10.0
TOML
unset WAYLAND_DISPLAY
export RUST_LOG="${RUST_LOG:-info}" LILLITH_MANIFEST="$MANIFEST"

# ---------- 1) first frame under Xvfb ----------
DISP=""
for n in $(seq 90 140); do
  [ -e "/tmp/.X11-unix/X$n" ] || [ -e "/tmp/.X$n-lock" ] || { DISP=":$n"; break; }
done
[ -n "$DISP" ] || fail "no free X display number"
Xvfb "$DISP" -screen 0 1280x720x24 -br -nolisten tcp +extension Composite >"$TMP/xvfb.log" 2>&1 &
PIDS+=($!)
for _ in $(seq 50); do [ -e "/tmp/.X11-unix/X${DISP#:}" ] && break; sleep 0.1; done
[ -e "/tmp/.X11-unix/X${DISP#:}" ] || fail "Xvfb did not start on $DISP"
export DISPLAY="$DISP"
log "Xvfb on $DISP"

# The empty screen must be black, otherwise the check below proves nothing.
lit_pixels() {  # count of non-black pixels on the screen; writes $TMP/frame.png
  ffmpeg -loglevel error -y -f x11grab -draw_mouse 0 -video_size 1280x720 -i "$DISP" -frames:v 1 "$TMP/frame.png" &&
  ffmpeg -loglevel error -i "$TMP/frame.png" -f rawvideo -pix_fmt gray - |
    python3 -c 'import sys; d=sys.stdin.buffer.read(); print(sum(1 for b in d if b > 8))'
}
base="$(lit_pixels)" || fail "ffmpeg x11grab could not capture $DISP"
[ "$base" -eq 0 ] || fail "empty Xvfb screen is not black ($base lit pixels)"

APP_SECS=$((WAIT + 5))
"$BIN" --config "$CFG" --manifest "$MANIFEST" --backend x11 --no-tray --no-voice --no-monitor \
  --no-save --reset-position --exit-after "$APP_SECS" >"$TMP/app.log" 2>&1 &
APP=$!
PIDS+=($APP)
lit=0
for _ in $(seq $((WAIT * 2))); do
  kill -0 "$APP" 2>/dev/null || { wait "$APP" || true; fail "overlay exited before drawing (see app.log)"; }
  lit="$(lit_pixels)" || lit=0
  [ "$lit" -gt 0 ] && break
  sleep 0.5
done
[ "$lit" -gt 0 ] || fail "first frame is all black after ${WAIT}s (overlay drew nothing)"
log "PASS frame: $lit non-black pixels on the first captured frame"
[ "${SMOKE_KEEP:-0}" = 1 ] && cp "$TMP/frame.png" "$TMP/first-frame.png"
kill "$APP" 2>/dev/null || true
wait "$APP" 2>/dev/null || true

# ---------- 2) a reply from the stub LLM ----------
out="$(timeout 60 "$BIN" --config "$CFG" chat --no-history "smoke test: say hi" 2>"$TMP/chat.err")" ||
  { cat "$TMP/chat.err" >>"$TMP/app.log"; fail "lillith chat exited non-zero (output: ${out:-<none>})"; }
grep -q "POST /v1/chat/completions" "$TMP/stub.log" || fail "lillith chat never reached the stub at $ENDPOINT/v1/chat/completions"
printf '%s' "$out" | grep -q "$REPLY" || fail "chat output did not contain the stub reply '$REPLY': ${out:-<none>}"
log "PASS chat: got the stub reply via POST /v1/chat/completions"
log "all smoke checks passed"
