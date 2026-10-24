"""Daemon-level safeword e2e: Lillith speaks several queued lines; a spoken safeword (WAV fed through the
mic/STT path) must cut speech at once and drop every queued line.
usage: safeword_e2e.py SOCK MICDIR PLAYEDDIR SCENARIO CLIP [CLIP...]   (prints one JSON per clip)
SCENARIO: stt (cmd stt on the wav) | ptt (listen start/stop, app path) | always (listen_mode, mic)
          | neg (stt on a non-safeword clip: speech must continue)"""
import json, os, socket, sys, threading, time, wave, shutil

SOCK, MIC, PLAYED, SCEN = sys.argv[1:5]; CLIPS = sys.argv[5:]
LINES = ["Mmm, there you are, darling. I have been waiting all evening for you to come back to me.",
         "Tell me everything about your day, every little detail. I want to hear it all, slowly.",
         "And then, when you are done, I have a little surprise for you. You are going to love it.",
         "Just one more thing before that. Stay right here with me, okay? Don't you dare go anywhere."]

def req(o, timeout=180):
    s = socket.socket(socket.AF_UNIX); s.settimeout(timeout); s.connect(SOCK)
    s.sendall((json.dumps(o) + "\n").encode()); r = s.makefile("rb").readline(); s.close(); return json.loads(r)

EV = []; LOCK = threading.Lock()
def sub():
    s = socket.socket(socket.AF_UNIX); s.connect(SOCK)
    s.sendall(b'{"op":"subscribe","topics":["lipsync","stt"]}\n'); f = s.makefile("rb")
    for line in f:
        t = time.monotonic()
        try: o = json.loads(line)
        except Exception: continue
        if "ev" in o:
            with LOCK: EV.append((t, o))
threading.Thread(target=sub, daemon=True).start(); time.sleep(0.5)

def evs(pred, since=0.0):
    with LOCK: return [(t, o) for t, o in EV if t >= since and pred(o)]
def wait_ev(pred, since, timeout):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        e = evs(pred, since)
        if e: return e[0]
        time.sleep(0.01)
    return None
def dur(p):
    with wave.open(p) as w: return w.getnframes() / w.getframerate()
def drop(p):
    tmp = os.path.join(MIC, ".mp.tmp"); shutil.copy(p, tmp); os.replace(tmp, os.path.join(MIC, "mic_play.wav"))
    return time.monotonic()

for clip in CLIPS:
    res = {"scenario": SCEN, "clip": os.path.basename(os.path.dirname(clip)) + "/" + os.path.basename(clip),
           "clip_s": round(dur(clip), 2)}
    req({"cmd": "stop", "reason": "reset"}); time.sleep(0.5)
    t0 = time.monotonic()
    utts = [req({"cmd": "say", "text": l, "wait": False})["utt"] for l in LINES]
    res["queued_utts"] = utts
    first = wait_ev(lambda o: o.get("ev") == "chunk_start" and o.get("utt") == utts[0], t0, 240)
    if not first:
        res["verdict"] = "FAIL"; res["why"] = "she never started speaking"; print(json.dumps(res), flush=True); continue
    res["first_audio_s"] = round(first[0] - t0, 2)
    time.sleep(1.0)  # mid-sentence of line 1; lines 2-4 still queued
    tq = time.monotonic()
    if SCEN in ("stt", "neg"):
        r = req({"cmd": "stt", "wav": os.path.abspath(clip)}); t_trig = tq; t_reply = time.monotonic()
        res.update(text=r.get("text"), safeword=r.get("safeword"), consent_ok=r.get("consent_ok"), stt_s=r.get("stt_s"))
    elif SCEN == "ptt":
        req({"cmd": "listen", "action": "start"}); t_trig = tq
        barge = wait_ev(lambda o: o.get("ev") == "stop", tq, 5)
        res["ptt_press_to_stop_ms"] = round((barge[0] - tq) * 1000) if barge else None
        res["ptt_stop_reason"] = barge[1].get("reason") if barge else None
        time.sleep(0.3); td = drop(clip); time.sleep(dur(clip) + 0.8)
        r = req({"cmd": "listen", "action": "stop"}); t_reply = time.monotonic()
        res.update(text=r.get("text"), safeword=r.get("safeword"), consent_ok=r.get("consent_ok"), stt_s=r.get("stt_s"),
                   release_to_transcript_ms=round((t_reply - (td + dur(clip) + 0.8)) * 1000))
    elif SCEN == "always":
        req({"cmd": "listen_mode", "on": True}); time.sleep(0.3)
        t_trig = drop(clip); tq = t_trig
        tr = wait_ev(lambda o: o.get("ev") == "transcript" and o.get("text"), tq, 30)
        t_reply = tr[0] if tr else None
        res.update(text=tr[1].get("text") if tr else None, safeword=tr[1].get("safeword") if tr else None,
                   consent_ok=tr[1].get("consent_ok") if tr else None, stt_s=tr[1].get("stt_s") if tr else None)
    stop = wait_ev(lambda o: o.get("ev") == "stop", tq, 30 if SCEN != "neg" else 3)
    if stop:
        res["stop_reason"] = stop[1].get("reason")
        res["trigger_to_stop_ms"] = round((stop[0] - tq) * 1000)
        if SCEN == "always": res["clip_end_to_stop_ms"] = round((stop[0] - (tq + dur(clip))) * 1000)
        if SCEN in ("stt",): res["stt_reply_vs_stop_ms"] = round((stop[0] - t_reply) * 1000)
    n_played0 = len(os.listdir(PLAYED))
    time.sleep(6.0)  # long enough for queued lines 2-4 to render + start if they had survived
    tend = stop[0] if stop else tq
    late = evs(lambda o: o.get("ev") in ("chunk_start", "envelope", "utt_end") and o.get("utt") in utts, tend + 0.05)
    res["events_after_stop"] = [f"{o['ev']}:u{o['utt']}" for _, o in late][:8]
    res["played_files_after_stop"] = len(os.listdir(PLAYED)) - n_played0
    p = req({"cmd": "ping"}); res["speaking_after"] = p.get("speaking")
    if stop: res["stop_utts"] = sorted({o["utt"] for _, o in evs(lambda o: o.get("ev") == "stop", tq)})
    if SCEN == "always": req({"cmd": "listen_mode", "on": False})
    if SCEN == "neg":
        ok = (not res.get("safeword")) and not (stop and stop[1].get("reason") == "safeword") and res.get("consent_ok") is False
        res["verdict"] = "PASS" if ok else "FAIL"
    else:
        sw_ev = evs(lambda o: o.get("ev") == "safeword", tq)
        res["safeword_event"] = bool(sw_ev)
        ok = bool(res.get("safeword")) and stop is not None and not late and not res["speaking_after"] \
             and set(utts[1:]) <= set(res.get("stop_utts", [])) and res.get("consent_ok") is False
        res["verdict"] = "PASS" if ok else "FAIL"
    print(json.dumps(res), flush=True)
    if res["verdict"] == "FAIL" and os.environ.get("EVDUMP"):
        with open(os.environ["EVDUMP"], "a") as f:
            f.write(json.dumps({"clip": res["clip"], "t0": t0, "tq": tq, "events": [(round(t - t0, 3), {k: v for k, v in o.items() if k != "pts"}) for t, o in evs(lambda o: True, t0)]}) + "\n")
    req({"cmd": "stop", "reason": "reset"}); time.sleep(0.5)
