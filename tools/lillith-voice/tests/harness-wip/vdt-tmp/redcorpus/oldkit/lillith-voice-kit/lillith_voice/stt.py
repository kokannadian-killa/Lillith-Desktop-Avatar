"""Local speech-to-text for Lillith: mic capture -> silero VAD -> whisper tiny.en (sherpa-onnx, ONNX).

Everything is on-device: models live in <models>/stt, sherpa-onnx is a bundled wheel, no network.
  * push-to-talk:      Listener.ptt_start() ... Listener.ptt_stop() -> transcript
  * one utterance:     Listener.listen_once()  (stops at end of speech, VAD)
  * always-listening:  Listener.set_always(True)  -> transcripts as events
  * barge-in:          user speech while Lillith talks stops her (strict VAD gate while she plays)
  * echo safety:       by default the mic is NOT transcribed while she is playing (echo="mute")
  * safety:            voice transcripts never count as consent ("consent_ok": false), and the
                       safeword ("red" / "OOC: stop") stops speech immediately.
Capture: LILLITH_MIC_CMD (any command writing s16le 16 kHz mono to stdout), else
pw-record (PipeWire) -> parec (Pulse) -> arecord (ALSA) on Linux; sounddevice (WASAPI) on Windows.
"""
from . import netguard  # noqa: F401

import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import threading
import time
import wave

import numpy as np

from .core import MODEL_DIR, CONFIG_HOME

STT_DIR = os.environ.get("LILLITH_STT_DIR") or os.path.join(MODEL_DIR, "stt")
STT_MODEL = os.environ.get("LILLITH_STT_MODEL", "tiny.en")
MIC_CONFIG = os.path.join(CONFIG_HOME, "lillith-voice", "mic.json")
RATE = 16000
DEBUG = bool(os.environ.get("LILLITH_DEBUG"))
WIN = 512  # silero window @16 kHz (32 ms)


def mic_config():
    try:
        with open(MIC_CONFIG, encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return {}


# --------------------------------------------------------------------------- safety
_SAFE_RE = re.compile(r"^\W*(red)\W*$|\b(o\.?\s?o\.?\s?c\.?|out of character)\W*\s*stop\b", re.I)
_AFFIRM_RE = re.compile(r"^\W*(yes|yeah|yep|yup|sure|ok(ay)?|i (do|agree|accept|consent)|deal|confirm(ed)?|"
                        r"sign (it|me up)|let'?s do it|absolutely|of course)\b", re.I)


def classify(text: str) -> dict:
    t = (text or "").strip()
    return {"safeword": bool(_SAFE_RE.search(t)), "affirmation": bool(_AFFIRM_RE.search(t)),
            "voice": True, "consent_ok": False}


# --------------------------------------------------------------------------- models
class Recognizer:
    def __init__(self, stt_dir=STT_DIR, model=STT_MODEL, threads=2):
        import sherpa_onnx
        d = stt_dir
        enc = _pick(d, f"{model}-encoder")
        dec = _pick(d, f"{model}-decoder")
        tok = os.path.join(d, f"{model}-tokens.txt")
        for f in (enc, dec, tok):
            if not os.path.isfile(f):
                raise SystemExit(f"lillith-voice: STT model file missing: {f} (no downloads are ever made)")
        t0 = time.time()
        self.r = sherpa_onnx.OfflineRecognizer.from_whisper(encoder=enc, decoder=dec, tokens=tok, language="en",
                                                            task="transcribe", num_threads=threads)
        self.load_s = time.time() - t0
        self._lock = threading.Lock()

    def transcribe(self, audio16k: np.ndarray) -> str:
        a = np.asarray(audio16k, np.float32)
        if len(a) < RATE // 5:
            return ""
        # whisper tiny mis-hears tight VAD cuts ("Red." -> "Right", "Hey Lillith..." -> "Sh? ( ( ("):
        # give it ~0.8 s of air on both sides, and retry with a different pad if the output looks degenerate
        raw = self._decode(a, 0.8)
        cands = [raw]
        if _degenerate(raw) or _cut_loop(raw) != raw.strip():
            cands.append(self._decode(a, 0.5))       # a different pad usually un-sticks whisper
        good = [c for c in (_cut_loop(x) for x in cands) if c and not _degenerate(c)]
        if not good:
            return ""
        t = max(good, key=lambda c: sum(ch.isalnum() for ch in c))
        t = _dedupe(t)
        # whisper hallucinations on near-silence
        if re.fullmatch(r"\W*(\[.*?\]|\(.*?\)|thank you\.?|you)\W*", t, re.I):
            return ""
        return t

    def _decode(self, a, pad_s):
        pad = np.zeros(int(RATE * pad_s), np.float32)
        a = np.concatenate([pad, a, pad])
        out = []
        with self._lock:
            for i in range(0, len(a), RATE * 28):  # whisper window is 30 s
                s = self.r.create_stream()
                s.accept_waveform(RATE, a[i:i + RATE * 28])
                self.r.decode_stream(s)
                out.append(s.result.text.strip())
        return " ".join(x for x in out if x)


def _cut_loop(t: str) -> str:
    """cut a whisper loop tail ("...tonight? [ * * * * *"): drop from the first token repeated 4+ times in a row,
    then any dangling bracket / symbol junk at the end"""
    toks = t.split()
    for i in range(len(toks) - 3):
        if toks[i] == toks[i + 1] == toks[i + 2] == toks[i + 3]:
            toks = toks[:i]
            break
    t = " ".join(toks)
    t = re.sub(r"\s*[\[(][^\])]*$", "", t)          # unclosed [ or ( at the end
    t = re.sub(r"[\s*#~_\-\[\]()]+$", "", t)          # trailing symbol junk
    return t.strip()


def _dedupe(t: str) -> str:
    """whisper tiny sometimes says a short clip twice ("Yes, I agree. Yes, I agree."): drop repeated sentences"""
    parts = re.findall(r"[^.!?]+[.!?]*", t)
    out = []
    for p in (x.strip() for x in parts):
        if p and not (out and re.sub(r"\W", "", p.lower()) == re.sub(r"\W", "", out[-1].lower())):
            out.append(p)
    return " ".join(out) if out else t


def _degenerate(t: str) -> bool:
    """whisper loop / garbage: mostly punctuation, or one token repeated 4+ times in a row"""
    if not t:
        return False
    if sum(c.isalnum() for c in t) < 0.5 * len(t.replace(" ", "")):
        return True
    w = re.findall(r"[\w']+", t.lower())
    run = 1
    for i in range(1, len(w)):
        run = run + 1 if w[i] == w[i - 1] else 1
        if run >= 4:
            return True
    return False


def _pick(d, stem):
    for suf in (".int8.onnx", ".onnx"):
        f = os.path.join(d, stem + suf)
        if os.path.isfile(f):
            return f
    return os.path.join(d, stem + ".int8.onnx")


class Vad:
    """silero VAD (sherpa-onnx). feed() 16 kHz float32; returns finished speech segments."""

    def __init__(self, threshold=0.5, min_silence=0.6, min_speech=0.25, max_speech=20.0, stt_dir=STT_DIR):
        import sherpa_onnx
        cfg = sherpa_onnx.VadModelConfig()
        cfg.silero_vad.model = os.path.join(stt_dir, "silero_vad.onnx")
        cfg.silero_vad.threshold = threshold
        cfg.silero_vad.min_silence_duration = min_silence
        cfg.silero_vad.min_speech_duration = min_speech
        cfg.silero_vad.max_speech_duration = max_speech
        cfg.silero_vad.window_size = WIN
        cfg.sample_rate = RATE
        cfg.num_threads = 1
        if not os.path.isfile(cfg.silero_vad.model):
            raise SystemExit(f"lillith-voice: VAD model missing: {cfg.silero_vad.model}")
        self.v = sherpa_onnx.VoiceActivityDetector(cfg, buffer_size_in_seconds=60)
        self._buf = np.zeros(0, np.float32)

    def feed(self, a):
        self._buf = np.concatenate([self._buf, np.asarray(a, np.float32)])
        segs = []
        while len(self._buf) >= WIN:
            self.v.accept_waveform(self._buf[:WIN])
            self._buf = self._buf[WIN:]
            while not self.v.empty():
                segs.append(np.array(self.v.front.samples, np.float32))
                self.v.pop()
        return segs

    @property
    def speaking(self) -> bool:
        return bool(self.v.is_speech_detected())

    def flush(self):
        self.v.flush()
        segs = []
        while not self.v.empty():
            segs.append(np.array(self.v.front.samples, np.float32))
            self.v.pop()
        return segs

    def reset(self):
        self.v.reset()
        self._buf = np.zeros(0, np.float32)


# --------------------------------------------------------------------------- capture
def mic_command(device=None):
    env = os.environ.get("LILLITH_MIC_CMD")
    if env:
        return shlex.split(env)
    device = device or mic_config().get("device") or os.environ.get("LILLITH_MIC_DEVICE")
    if shutil.which("pw-record"):
        c = ["pw-record", "--rate", str(RATE), "--channels", "1", "--format", "s16"]
        return c + (["--target", device] if device else []) + ["-"]
    if shutil.which("parec"):
        c = ["parec", "--raw", f"--rate={RATE}", "--channels=1", "--format=s16le", "--latency-msec=30"]
        return c + ([f"--device={device}"] if device else [])
    if shutil.which("arecord"):
        return ["arecord", "-q", "-t", "raw", "-f", "S16_LE", "-r", str(RATE), "-c", "1"] + (["-D", device] if device else [])
    return None


def list_devices():
    out = []
    if shutil.which("pactl"):
        r = subprocess.run(["pactl", "list", "short", "sources"], capture_output=True, text=True)
        for ln in r.stdout.splitlines():
            p = ln.split("\t")
            if len(p) > 1 and ".monitor" not in p[1]:
                out.append({"id": p[1], "desc": " ".join(p[2:])})
    elif shutil.which("arecord"):
        r = subprocess.run(["arecord", "-L"], capture_output=True, text=True)
        out = [{"id": ln.strip(), "desc": ""} for ln in r.stdout.splitlines() if ln and not ln.startswith(" ")]
    try:
        import sounddevice as sd  # Windows (WASAPI) / anywhere it is installed
        for i, d in enumerate(sd.query_devices()):
            if d["max_input_channels"] > 0:
                out.append({"id": str(i), "desc": d["name"]})
    except Exception:
        pass
    return out


class Mic:
    """Streams 16 kHz mono float32 frames to on_audio(frames) from a capture subprocess (or sounddevice)."""

    def __init__(self, on_audio, device=None):
        self.on_audio = on_audio
        self.device = device
        self.proc = None
        self.stream = None
        self.running = False

    def start(self):
        if self.running:
            return
        self.running = True
        cmd = mic_command(self.device)
        if cmd is None:
            try:
                import sounddevice as sd
            except ImportError:
                self.running = False
                raise RuntimeError("no microphone capture tool (pw-record / parec / arecord / sounddevice)")
            dev = self.device if self.device is None else int(self.device)
            self.stream = sd.InputStream(samplerate=RATE, channels=1, dtype="float32", blocksize=WIN, device=dev,
                                         callback=lambda d, *_: self.on_audio(d[:, 0].copy()))
            self.stream.start()
            return
        self.proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, stdin=subprocess.DEVNULL)
        threading.Thread(target=self._read, daemon=True, name="lillith-mic").start()

    def _read(self):
        f = self.proc.stdout
        while self.running:
            b = f.read(WIN * 2)
            if not b:
                break
            self.on_audio(np.frombuffer(b[: len(b) // 2 * 2], "<i2").astype(np.float32) / 32768.0)
        self.running = False

    def stop(self):
        self.running = False
        if self.stream is not None:
            self.stream.stop()
            self.stream.close()
            self.stream = None
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(2)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        self.proc = None


def read_wav16k(path):
    with wave.open(path, "rb") as w:
        sr, ch, sw = w.getframerate(), w.getnchannels(), w.getsampwidth()
        a = np.frombuffer(w.readframes(w.getnframes()), {2: "<i2", 4: "<i4"}[sw]).astype(np.float32)
    a /= 32768.0 if sw == 2 else 2147483648.0
    if ch > 1:
        a = a.reshape(-1, ch).mean(1)
    if sr != RATE:
        from .core import _fft_resample
        a = _fft_resample(a, int(round(len(a) * RATE / sr)))
    return a


# --------------------------------------------------------------------------- listener
class Listener:
    """Mic + VAD + STT glued to a Speaker (for barge-in / echo safety) and an event sink."""

    def __init__(self, speaker=None, emit=None, echo=None, barge_in=None, threads=2):
        cfg = mic_config()
        self.speaker = speaker
        self.emit = emit or (lambda ev: None)
        self.echo = echo or cfg.get("echo") or os.environ.get("LILLITH_ECHO", "mute")   # mute | open
        self.barge_in = cfg.get("barge_in", True) if barge_in is None else barge_in
        self.barge_db = float(cfg.get("barge_in_dbfs", os.environ.get("LILLITH_BARGE_DBFS", -32)))
        self.barge_ms = float(cfg.get("barge_in_ms", 300))
        self.device = cfg.get("device")
        self.rec = Recognizer(threads=threads)
        self.vad = Vad(threshold=float(cfg.get("vad_threshold", 0.5)))
        self.mic = Mic(self._on_audio, self.device)
        self._lock = threading.Lock()
        self._mode = None          # None | "ptt" | "once" | "always"
        self._ptt = []
        self._once = None
        self._loud_ms = 0.0
        self._barged = False
        self._pre = []             # ~0.5 s pre-roll kept while muted (so a barge-in's first word survives)
        self.always = False

    # ---------------------------------------------------------------- audio path
    def _playing(self):
        return bool(self.speaker and self.speaker.player.playing)

    def _on_audio(self, a):
        with self._lock:
            mode = self._mode
            if mode == "ptt":
                self._ptt.append(a)
                return
            if mode is None:
                return
            if self._playing() and self.echo == "mute" and not self._barged:
                # echo safety: don't transcribe her own voice; only a strict barge-in check
                self._pre = (self._pre + [a])[-16:]
                if self.barge_in:
                    rms = 20 * np.log10(np.sqrt(np.mean(a ** 2)) + 1e-9)
                    self._loud_ms = self._loud_ms + len(a) / RATE * 1000 if rms > self.barge_db else 0.0
                    if self._loud_ms >= self.barge_ms:
                        self._loud_ms = 0.0
                        self._barge("energy")
                        self._barged = True       # he is talking now: stream straight into the VAD
                        self.vad.reset()
                        for p in self._pre:
                            self.vad.feed(p)
                        self._pre = []
                return
            segs = self.vad.feed(a)
            if self.vad.speaking and self._playing() and self.barge_in and not self._barged:
                self._barged = True
                self._barge("vad")
        for s in segs:
            self._segment(s)

    def _barge(self, why):
        if self.speaker:
            self.speaker.stop(reason="barge_in")
        self.emit({"ev": "barge_in", "by": why})

    def _segment(self, seg):
        self._barged = False
        t0 = time.time()
        text = self.rec.transcribe(seg)
        if DEBUG:
            print(f"lillith-voice: segment {len(seg) / RATE:.2f}s mode={self._mode} t={time.monotonic():.1f} -> {text!r}",
                  file=sys.stderr, flush=True)
        if not text:
            return
        info = {"ev": "transcript", "text": text, "audio_s": round(len(seg) / RATE, 2),
                "stt_s": round(time.time() - t0, 2), **classify(text)}
        if info["safeword"] and self.speaker:
            self.speaker.stop(reason="safeword")
            self.emit({"ev": "safeword", "text": text})
        if self._mode == "once" and self._once is not None:
            self._once["result"] = info
            self._once["done"].set()
        self.emit(info)

    # ---------------------------------------------------------------- API
    def ptt_start(self):
        if self._playing() or (self.speaker and self.speaker.busy()):
            self._barge("ptt")          # pressing talk always interrupts her
        with self._lock:
            self._ptt = []
            self._mode = "ptt"
        self.mic.start()
        self.emit({"ev": "listening", "mode": "ptt"})
        return {"ok": True, "listening": True}

    def ptt_stop(self):
        with self._lock:
            audio = np.concatenate(self._ptt) if self._ptt else np.zeros(0, np.float32)
            self._ptt = []
            self._mode = "always" if self.always else None
        if not self.always:
            self.mic.stop()
        return self.transcribe(audio)

    def transcribe(self, audio):
        """VAD-trim then transcribe (used for PTT buffers and WAV files)."""
        t0 = time.time()
        v = Vad()
        segs = v.feed(audio) + v.flush()
        speech = np.concatenate(segs) if segs else np.zeros(0, np.float32)
        if len(speech) and len(audio) <= RATE * 28:
            speech = np.asarray(audio, np.float32)   # short clip with speech: keep the natural edges
        text = self.rec.transcribe(speech) if len(speech) else ""
        info = {"ok": True, "text": text, "audio_s": round(len(audio) / RATE, 2), "speech_s": round(len(speech) / RATE, 2),
                "stt_s": round(time.time() - t0, 2), **classify(text)}
        if info["safeword"] and self.speaker:
            self.speaker.stop(reason="safeword")
            self.emit({"ev": "safeword", "text": text})
        self.emit({"ev": "transcript", **{k: v for k, v in info.items() if k != "ok"}})
        return info

    def listen_once(self, timeout=15.0):
        if self._playing() or (self.speaker and self.speaker.busy()):
            self._barge("listen")
        done = threading.Event()
        self._once = {"done": done, "result": None}
        with self._lock:
            self.vad.reset()
            self._mode = "once"
        self.mic.start()
        self.emit({"ev": "listening", "mode": "once"})
        t_end = time.time() + timeout
        while not done.wait(0.05):
            if time.time() > t_end or not self.mic.running:
                break
        with self._lock:
            self._mode = "always" if self.always else None
            tail = [] if done.is_set() else self.vad.flush()
        if not self.always:
            self.mic.stop()
        if not done.is_set() and tail:
            self._segment(np.concatenate(tail))
        r = self._once["result"] if self._once else None
        self._once = None
        if r is None:
            return {"ok": True, "text": "", "timeout": True, **classify("")}
        return {"ok": True, **{k: v for k, v in r.items() if k != "ev"}}

    def set_always(self, on: bool):
        self.always = bool(on)
        with self._lock:
            if on:
                self.vad.reset()
                self._mode = "always"
            elif self._mode == "always":
                self._mode = None
        if on:
            self.mic.start()
        else:
            self.mic.stop()
        self.emit({"ev": "listen_mode", "on": self.always})
        return {"ok": True, "always": self.always}
