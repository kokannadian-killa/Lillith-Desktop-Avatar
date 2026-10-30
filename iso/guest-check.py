#!/usr/bin/env python3
"""In-guest verification for iso/test-boot.sh, over the QEMU serial console.

  guest-check.py SERIAL_SOCK SERIAL_LOG CHECK_AFTER_SECS

Waits for the ttyS0 getty ("login:"), lets the desktop come up for
CHECK_AFTER_SECS, logs in as root (the live image's root has no password, as
in releng), and runs read-only checks. The only thing it starts is a
llama-server bound to 127.0.0.1 inside the guest (Lillith itself never starts
one without a clicked consent card), to prove a local model answers.
Output: a plain-text report on stdout, one "== name" section per check, and a
final "RESULT name PASS|FAIL" table.
"""
import re, socket, sys, time

sock_path, log_path, after = sys.argv[1], sys.argv[2], int(sys.argv[3])
MODEL = "/usr/share/lillith/models/qwen2.5-0.5b-instruct-q4_k_m.gguf"
U = ("runuser -u demon -- env HOME=/home/demon XDG_RUNTIME_DIR=/run/user/1000"
     " DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus")

def log(*a):
    print(*a, flush=True)


for _ in range(60):
    try:
        s = socket.socket(socket.AF_UNIX); s.connect(sock_path); break
    except OSError:
        time.sleep(2)
else:
    log("FATAL no serial socket"); sys.exit(1)
s.settimeout(1)
buf = b""

def pump(t=1.0):
    global buf
    end = time.time() + t
    while time.time() < end:
        try:
            d = s.recv(65536)
            if not d:
                raise SystemExit("serial closed")
            buf += d
        except socket.timeout:
            pass

def expect(pat, timeout):
    global buf
    end = time.time() + timeout
    rx = re.compile(pat)
    while time.time() < end:
        m = rx.search(buf)
        if m:
            out = buf[:m.end()]; buf = buf[m.end():]
            return out
        pump(1)
    return None

def drain(seconds, pat=None):
    """Keep reading the serial socket (an unread socket back-pressures the
    guest UART and stalls GRUB/the kernel), optionally until pat shows up."""
    global buf
    end = time.time() + seconds
    while time.time() < end:
        pump(2)
        if pat is not None and re.search(pat, buf):
            return True
        buf = buf[-8192:]
    return pat is None

def send(line):
    s.sendall(line.encode() + b"\r")

t0 = time.time()
if not drain(3600, rb"login:"):
    log("FATAL no login prompt within 3600s"); sys.exit(1)
log(f"# getty login prompt seen after ~{int(time.time()-t0)}s of checker time; waiting {after}s for the desktop")
drain(after); buf = b""
for attempt in range(10):
    send("")
    if expect(rb"login: ?$", 60):
        break
send("root")
pump(45)  # empty root password (releng); zsh + grml prompt is slow under TCG
send("exec env -i TERM=dumb PATH=/usr/local/sbin:/usr/local/bin:/usr/bin HOME=/root bash --norc --noprofile")
pump(15); send("PS1='GC> '; stty -echo; dmesg -n 1")
if expect(rb"GC> ", 180) is None:
    log("FATAL no root shell on ttyS0; last output:\n" + buf[-2000:].decode(errors="replace")); sys.exit(1)
pump(3); buf = b""

n = 0
def run(name, cmd, timeout=300):
    global n, buf
    n += 1
    send(f"{{ {cmd} ; }} 2>&1; echo __GC_{n}_RC_$?__")
    out = expect(rf"__GC_{n}_RC_(\d+)__".encode(), timeout)
    if out is None:
        text, rc = buf.decode(errors="replace"), "TIMEOUT"; buf = b""
    else:
        rc = re.search(rb"__GC_\d+_RC_(\d+)__", out).group(1).decode()
        text = re.sub(rb"__GC_\d+_RC_\d+__", b"", out).decode(errors="replace")
    text = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]|\r|GC> ", "", text).strip()
    log(f"== {name}  (rc={rc})\n$ {cmd}\n{text}\n")
    return rc, text

R = {}
run("uptime+system state", "uptime; systemctl is-system-running; systemctl is-active sddm display-manager")
rc, t = run("desktop processes", "pgrep -a kwin_wayland; pgrep -au demon plasmashell; loginctl list-sessions --no-legend")
R["desktop session (kwin_wayland + plasmashell as demon)"] = "kwin_wayland" in t and "plasmashell" in t
rc, t = run("lillith process", "pgrep -au demon -x lillith || pgrep -au demon lillith")
R["lillith running as demon"] = rc == "0" and "lillith" in t
rc, t = run("~/.biotek", "test -d /home/demon/.biotek && echo BIOTEK_DIR_OK; ls -la /home/demon/.biotek /home/demon/.biotek/config 2>&1; find /home/demon/.biotek -maxdepth 3 | head -40")
R["~/.biotek exists"] = "BIOTEK_DIR_OK" in t
rc, t = run("awakening state", "f=/home/demon/.biotek/config/awakening.json; if [ -e $f ]; then echo DONE; cat $f; else echo ARMED-no-flag; fi; pgrep -au demon -f 'lillith-settings' || echo no-settings-window; pgrep -af demonos-awakening || true")
R["awakening dialog up or armed"] = ("lillith-settings" in t and "--step awakening" in t) or "ARMED-no-flag" in t
rc, t = run("lillith quantumsec --once", f"cd /home/demon && timeout 120 {U} BIOTEK_HOME=/home/demon/.biotek lillith quantumsec --once; echo exit=$?; ls -la /home/demon/.biotek/quantumsec 2>&1; cat /home/demon/.biotek/quantumsec/status.json 2>/dev/null | head -c 1500", 200)
R["`lillith quantumsec --once` exits 0"] = "exit=0" in t
rc, t = run("systemd user units", "ls -l /usr/lib/systemd/user/lillith*.service; systemctl --global is-enabled lillith.service lillith-quantumsec.service; ls -la /home/demon/.config/systemd/user 2>&1; " + U + " systemctl --user is-enabled lillith.service lillith-quantumsec.service; " + U + " systemctl --user is-active lillith.service lillith-quantumsec.service")
R["user units present"] = "/usr/lib/systemd/user/lillith.service" in t and "/usr/lib/systemd/user/lillith-quantumsec.service" in t
R["user units not enabled"] = "enabled" not in re.sub(r"disabled|not-enabled", "", t)
# DoD "systemd service works": enable + start the shield unit as demon (the same
# command the blood-seal / unit docs show), then check it runs /usr/bin/lillith
# and keeps rewriting status.json.
SJ = "/home/demon/.biotek/quantumsec/status.json"
rc, t = run("enable --now lillith-quantumsec.service", f"{U} systemctl --user enable --now lillith-quantumsec.service; sleep 30; "
            f"{U} systemctl --user is-active lillith-quantumsec.service; "
            f"{U} systemctl --user show -p ExecStart -p MainPID -p NRestarts lillith-quantumsec.service; "
            f"{U} systemctl --user cat lillith-quantumsec.service | grep -E '^(# /|ExecStart)'; "
            f"a=$(stat -c %Y {SJ}); sleep 45; b=$(stat -c %Y {SJ}); echo status_json_mtime $a '->' $b; "
            f"[ \"$b\" -gt \"$a\" ] && echo STATUS_JSON_UPDATING; head -c 300 {SJ}; echo; "
            f"{U} journalctl --user -u lillith-quantumsec.service --no-pager -n 5", 400)
R["lillith-quantumsec.service active (enabled for the test)"] = re.search(r"^active\s*$", t, re.M) is not None and "/usr/bin/lillith" in t
R["shield unit keeps writing status.json"] = "STATUS_JSON_UPDATING" in t
rc, t = run("live session: no blanking / no lock", f"{U} kreadconfig6 --file powerdevilrc --group AC --group Display --key TurnOffDisplayWhenIdle; "
            f"{U} kreadconfig6 --file powerdevilrc --group AC --group Display --key DimDisplayWhenIdle; "
            f"{U} kreadconfig6 --file kscreenlockerrc --group Daemon --key Autolock")
R["powerdevil DPMS/dim off, autolock off"] = t.split().count("false") >= 3
rc, t = run("LLMNR + mDNS off", "resolvectl status 2>/dev/null | grep -iE 'LLMNR|mDNS' | head -4; ss -Hlntup | grep -E ':5355' || echo NO_LLMNR_LISTENER; "
            "ss -Hlntup | grep -E ':5353' || echo NO_MDNS_LISTENER")
R["LLMNR off (no :5355 listener)"] = "NO_LLMNR_LISTENER" in t
R["mDNS off (no :5353 listener)"] = "NO_MDNS_LISTENER" in t

# main's newer features (Dream learning, Bots tab) are in the installed build:
# strings in the binaries + `--tab bots` opens the Bots tab (screendump below).
rc, t = run("Bots tab + Dream learning present",
            "grep -a -c 'Dream learning' /usr/bin/lillith; grep -a -c 'End dream early' /usr/bin/lillith; "
            "lillith-settings --help 2>&1 | grep -E -- '--tab|bots' | head -3; "
            f"{U} WAYLAND_DISPLAY=wayland-0 QT_QPA_PLATFORM=wayland setsid -f lillith-settings --tab bots >/tmp/bots-tab.log 2>&1; "
            "sleep 40; pgrep -au demon -f 'lillith-settings --tab bots' | head -2; tail -3 /tmp/bots-tab.log", 200)
lines = t.splitlines()
nums = [int(x) for x in lines if x.strip().isdigit()]
R["Dream learning strings in /usr/bin/lillith"] = len(nums) >= 2 and all(n > 0 for n in nums[:2])
R["lillith-settings --tab bots running"] = "lillith-settings --tab bots" in t and "bots" in t
def screendump(name):
    import os
    mon = os.path.join(os.path.dirname(sock_path), "mon.sock")
    out = os.path.join(os.path.dirname(sock_path), name)
    for _ in range(5):
        try:
            m = socket.socket(socket.AF_UNIX); m.settimeout(20); m.connect(mon); time.sleep(0.3)
            try: m.recv(65536)
            except Exception: pass
            m.sendall(f"screendump {out} -f png\n".encode()); time.sleep(2); m.close()
            if os.path.exists(out): return out
        except Exception as e:
            time.sleep(3)
    return None
shot = screendump("shot-bots-tab.png")
print("BOTS-TAB SCREENSHOT:", shot, flush=True)
R["Bots tab screenshot taken"] = shot is not None
run("listening sockets before model", "ss -Hltnp")
run("start llama-server on 127.0.0.1", f"{U} sh -c 'nohup /opt/llama/llama-server -m {MODEL} --host 127.0.0.1 --port 8080 -c 512 -t 2 --jinja >/tmp/llama.log 2>&1 &'; sleep 2; pgrep -a llama-server")
rc, t = run("wait for /health", "for i in $(seq 1 120); do curl -sf http://127.0.0.1:8080/health && break; sleep 10; done; echo; tail -5 /tmp/llama.log", 1500)
rc, t = run("chat completion", """curl -s -m 900 http://127.0.0.1:8080/v1/chat/completions -H 'Content-Type: application/json' -d '{"messages":[{"role":"user","content":"Reply with exactly: DemonOS awake"}],"max_tokens":16,"temperature":0}'""", 1000)
R["local model answers on 127.0.0.1:8080"] = '"content"' in t and "choices" in t
rc, t = run("model bind address", "ss -Hltnp | grep -E ':8080|:11434' || true")
R["llama-server bound to 127.0.0.1 only"] = "127.0.0.1:8080" in t and "0.0.0.0:8080" not in t and "*:8080" not in t
run("memory", "free -m")
log("== RESULTS")
for k, v in R.items():
    log(f"RESULT {'PASS' if v else 'FAIL'}  {k}")
log("GUEST-CHECK-DONE")
