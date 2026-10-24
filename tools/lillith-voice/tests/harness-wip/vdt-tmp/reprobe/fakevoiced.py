"""Fake lillith-voiced for timeout tests. mode=hang: accepts, reads, never replies.
mode=hangsay: answers ping/subscribe/stop, never answers say. Logs every request with epoch time."""
import json, os, socket, sys, threading, time
SOCK, MODE, LOG = sys.argv[1], sys.argv[2], sys.argv[3]
log = open(LOG, "a", buffering=1)
def w(*a): log.write(f"{time.time():.3f} " + " ".join(str(x) for x in a) + "\n")
PING = {"ok": True, "pid": os.getpid(), "protocol": 2, "presets": ["lillith", "p1"], "default": "lillith", "cloned_voice": True}
def handle(c, cid):
    f = c.makefile("rb")
    try:
        for raw in f:
            try: o = json.loads(raw)
            except Exception: w(cid, "BAD", raw[:80]); continue
            cmd = o.get("cmd") or o.get("op")
            w(cid, ">", json.dumps(o)[:4000])
            if MODE == "hang": continue
            if cmd == "ping": c.sendall((json.dumps(PING) + "\n").encode())
            elif cmd == "subscribe": c.sendall(b'{"ok": true, "subscribed": ["lipsync"]}\n')
            elif cmd == "stop": c.sendall(b'{"ok": true, "stopped": true}\n'); w(cid, "< stop ok")
            elif cmd == "say": w(cid, "HOLDING say (no reply)")
            else: c.sendall(b'{"ok": true}\n')
        w(cid, "EOF")
    except Exception as e: w(cid, "ERR", e)
    finally: c.close()
if os.path.exists(SOCK): os.unlink(SOCK)
s = socket.socket(socket.AF_UNIX); old = os.umask(0o177); s.bind(SOCK); os.umask(old); s.listen(64); w("ready", MODE)
n = 0
while True:
    c, _ = s.accept(); n += 1; w(f"c{n}", "OPEN"); threading.Thread(target=handle, args=(c, f"c{n}"), daemon=True).start()
