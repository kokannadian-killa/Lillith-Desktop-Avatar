"""Protocol-level checks against the real daemon. Prints 'RESULT <name> PASS|FAIL <detail>' lines."""
import json, os, sys, time, threading, shutil, collections
sys.path.insert(0, os.path.dirname(__file__)); from vc import conn, req
A, MIC = sys.argv[1], sys.argv[2]   # assets dir, fake-mic dir
def res(n, ok, d=""): print(f"RESULT {n} {'PASS' if ok else 'FAIL'} {d}", flush=True)
p1, p2 = req({"cmd": "ping"}), req({"op": "ping"})
res("proto.ping_cmd_op", p1.get("ok") and p2.get("ok") and p1.get("protocol") == 2 == p2.get("protocol"), f"cmd={p1.get('protocol')} op={p2.get('protocol')} cloned={p1.get('cloned_voice')}")
r = req({"text": "Hi.", "wait": True}); res("proto.say_no_key_defaults_to_say", r.get("ok") and "utt" in r, json.dumps(r)[:120])
for w, exp in (("red", "safeword"), ("yes", "affirmation")):
    t = req({"cmd": "stt", "wav": f"{A}/{w}.wav"})
    ok = t.get(exp) is True and (w != "yes" or (t.get("consent_ok") is False and not t.get("safeword")))
    res(f"proto.stt_{w}", ok, json.dumps(t)[:160])
    req({"cmd": "listen", "action": "start"}); time.sleep(0.4); shutil.copy(f"{A}/{w}.wav", f"{MIC}/mic_play.wav"); time.sleep(2.5)
    t = req({"op": "listen", "action": "stop"})
    ok = t.get(exp) is True and (w != "yes" or (t.get("consent_ok") is False and not t.get("safeword")))
    res(f"proto.listen_{w}", ok, json.dumps(t)[:160])
s, f = conn(); s.sendall(b'{"op":"subscribe","topics":["lipsync"]}\n'); sub = json.loads(f.readline())
evs = []
threading.Thread(target=lambda: [evs.append(json.loads(l)) for l in f], daemon=True).start()
out = {}
th = threading.Thread(target=lambda: out.update(r=req({"cmd": "say", "text": "This is a long sentence that should be interrupted. It keeps going, and going, with a second chunk after that one.", "wait": True})))
th.start(); time.sleep(3.0); st = req({"op": "stop", "reason": "barge_in"}); th.join(30)
res("proto.say_then_stop_2nd_conn", st.get("ok") and out.get("r", {}).get("stopped") is True, f"stop={st} say={out.get('r')}")
r = req({"cmd": "say", "text": "Hello Ko.", "wait": True}); time.sleep(0.6)
c = collections.Counter(e.get("ev") for e in evs)
res("proto.lipsync_events", sub.get("ok") and all(c[k] for k in ("envelope", "chunk_start", "chunk_end", "utt_end", "stop")) and any(e.get("ev") == "stop" and e.get("reason") == "barge_in" for e in evs), f"{dict(c)} (visemes not emitted by pocket/kokoro per PROTOCOL.md)")
utts = {type(e.get('utt')).__name__ for e in evs}; res("proto.utt_type", utts == {"int"}, f"utt types in events: {utts}; say reply utt={r.get('utt')!r}")
s.close()
