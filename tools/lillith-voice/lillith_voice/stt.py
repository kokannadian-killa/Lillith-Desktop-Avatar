"""Local speech-to-text for Lillith: mic capture -> silero VAD -> whisper tiny.en (sherpa-onnx, ONNX).

Everything is on-device: models live in <models>/stt, sherpa-onnx is a bundled wheel, no network.
  * push-to-talk:      Listener.ptt_start() ... Listener.ptt_stop() -> transcript
  * one utterance:     Listener.listen_once()  (stops at end of speech, VAD)
  * always-listening:  Listener.set_always(True)  -> transcripts as events
  * barge-in:          user speech while Lillith talks stops her (strict VAD gate while she plays)
  * echo safety:       by default the mic is NOT transcribed while she is playing (echo="mute")
  * safety:            voice transcripts never count as consent ("consent_ok": false), and the
                       safeword ("red" / "OOC: stop") stops speech immediately.
Capture: LILLITH_MIC_CMD (an allowlisted recorder, absolute path: arecord / parecord / parec / pw-record /
ffmpeg, writing s16le 16 kHz mono to stdout; see launchguard.check_mic_command), else
pw-record (PipeWire) -> parec (Pulse) -> arecord (ALSA) on Linux; sounddevice (WASAPI) on Windows.
Mic gate: capture starts only after Ko's mouse-click Yes on a Soul Contract card (micgate.py, PROTOCOL.md),
and every capture start writes one LAUNCH line to Lillith's audit log.
"""
from . import netguard  # noqa: F401

import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time
import wave

import numpy as np

from .core import MODEL_DIR, CONFIG_HOME
from . import launchguard as G
from .micgate import MicGate

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
# "red" (alone or repeated), "OOC: stop" incl. how whisper spells a spoken "O-O-C" ("oh, see, stop", "o o c stop")
_SAFE_RE = re.compile(r"^\W*(red\W*){1,4}$|\b(o\.?\s?o\.?\s?c\.?|ooc|(oh?\W*){1,2}(see|c|sea)|out of character)\W*\s*stop\b"
                      # whole short utterance "oh (I'll) see, stop/start", "O-C. Start." (whisper's takes on "O-O-C stop")
                      r"|^\W*(oh?\W+){1,2}(i'?ll\W+)?(see|c|sea)\W+(stop|start)\W*$|^\W*(o\W*){1,2}c\W+(stop|start)\W*$", re.I)
_AFFIRM_RE = re.compile(r"^\W*(yes|yeah|yep|yup|sure|ok(ay)?|i (do|agree|accept|consent)|deal|confirm(ed)?|"
                        r"sign (it|me up)|let'?s do it|absolutely|of course)\b", re.I)


# whisper tiny mis-hears a lone, clipped "red" ("Right.", "Read.", "Rhett.", "Rid", "", ...). A short utterance whose
# only word sounds like "red" is the safeword: a false stop is the fail-safe direction. Sentences never match.
_RED_LIKE = re.compile(r"^(w?r+h?(e|ea|a|i|ee|ei|ai|ae|y)(d|dd|t|tt|th|dh)|wred|redd?s?)$")
_RIGHT_LIKE = re.compile(r"^(right|rite|wright|write|rights)$")
RED_MAX_S = 1.5      # longest speech (s) that may count as a lone fuzzy "red"
RIGHT_MAX_S = 1.2    # "right" counts only alone and even shorter


def _words(t):
    return re.findall(r"[a-z]+", (t or "").lower().replace("'", ""))


def fuzzy_red(text: str, speech_s=None) -> bool:
    """1-2 word utterance where every word sounds like "red" (red, read, rad, rid, rhett, wred, "red red");
    "right" only when it is the single word and very short. Duration unknown -> words alone decide."""
    w = _words(text)
    if not w or len(w) > 2 or (speech_s is not None and speech_s > RED_MAX_S):
        return False
    if all(_RED_LIKE.match(x) for x in w):
        return True
    return len(w) == 1 and _RIGHT_LIKE.match(w[0]) is not None and (speech_s is None or speech_s <= RIGHT_MAX_S)


def classify(text: str, speech_s=None) -> dict:
    t = (text or "").strip()
    return {"safeword": bool(_SAFE_RE.search(t)) or fuzzy_red(t, speech_s), "affirmation": bool(_AFFIRM_RE.search(t)),
            "voice": True, "consent_ok": False}     # a voice transcript is NEVER consent


def speech_seconds(a) -> float:
    """voiced length of a clip: silero VAD segments, else frames above -40 dBFS (32 ms windows)"""
    a = np.asarray(a, np.float32)
    try:
        v = Vad()
        segs = v.feed(a) + v.flush()
        if segs:
            return sum(len(x) for x in segs) / RATE
    except BaseException:
        pass
    n = len(a) // WIN
    if not n:
        return 0.0
    rms = np.sqrt((a[:n * WIN].reshape(n, WIN) ** 2).mean(1) + 1e-12)
    return float((20 * np.log10(rms) > -40).sum() * WIN / RATE)


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

    def transcribe(self, audio16k: np.ndarray, speech_s=None) -> str:
        a = np.asarray(audio16k, np.float32)
        if len(a) < RATE // 5:
            return ""
        short = len(a) <= RATE * 3 if speech_s is None else speech_s <= RED_MAX_S + 0.5
        # whisper tiny mis-hears tight VAD cuts ("Red." -> "Right", "Hey Lillith..." -> "Sh? ( ( ("):
        # give it ~0.8 s of air on both sides, and retry with a different pad if the output looks degenerate.
        # sherpa-onnx whisper has no initial_prompt / beam search (greedy only), so the safeword bias is done here:
        # short clips are also decoded with a tight 0.4 s pad, and a candidate that is the safeword wins.
        cands = [self._decode(a, 0.8)]
        if short or _degenerate(cands[0]) or _cut_loop(cands[0]) != cands[0].strip():
            cands.append(self._decode(a, 0.4))
        t = self._pick_best(cands, speech_s)
        c0 = classify(t, speech_s)
        if short and _voiced(a) and (not t or (len(_words(t)) <= 2 and not c0["safeword"] and not c0["affirmation"])):
            # retry once with different decoding. Empty: level-normalised with a 0.3 s pad, slowed ~8%, denoised
            # (whisper's greedy path often drops a lone word at a quiet/loud level or under noise).
            # A 1-2 word non-safeword reading of a short clip ("Mind.", "The rain."): the same normalised + denoised
            # decodes, so a noisy "red" still gets its chance (whisper int8 greedy output also varies run to run);
            # a safeword reading wins, otherwise the first answer stands.
            na = _normalise(a)
            cands = [self._decode(na, 0.3), self._decode(_normalise(_denoise(a)), 0.4)]
            if not t:
                cands.append(self._decode(_tempo(na, 0.92), 0.5))
            r = self._pick_best(cands, speech_s)
            if not t or (r and classify(r, speech_s)["safeword"]):
                t = r
        return t

    def _pick_best(self, cands, speech_s=None) -> str:
        good = [c for c in (_cut_loop(x) for x in cands) if c and not _degenerate(c)]
        good = [_dedupe(c) for c in good]
        # whisper hallucinations on near-silence
        good = [c for c in good if not re.fullmatch(r"\W*(\[.*?\]|\(.*?\)|thank you\.?|you)\W*", c, re.I)]
        if not good:
            return ""
        safe = [c for c in good if classify(c, speech_s)["safeword"]]
        if safe:
            return safe[0]                       # fail-safe: any reading that says "red" stops her
        return max(good, key=lambda c: sum(ch.isalnum() for ch in c))

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


def _voiced(a) -> bool:
    a = np.asarray(a, np.float32)
    return len(a) > 0 and 20 * np.log10(np.sqrt(np.mean(a ** 2)) + 1e-9) > -50 and np.abs(a).max() > 0.01


def _normalise(a, peak=0.7):
    m = float(np.abs(a).max()) if len(a) else 0.0
    return a if m < 1e-4 else (a * (peak / m)).astype(np.float32)


def _denoise(a, n=512, hop=128, over=1.5):
    """spectral subtraction: noise = mean spectrum of the quietest 20% frames (the air around a short word)"""
    a = np.asarray(a, np.float32)
    if len(a) < n * 4:
        return a
    w = np.hanning(n).astype(np.float32)
    idx = np.arange(0, len(a) - n, hop)
    F = np.fft.rfft(np.stack([a[i:i + n] * w for i in idx]), axis=1)
    mag, ph = np.abs(F), np.angle(F)
    e = mag.sum(1)
    noise = mag[e <= np.percentile(e, 20)].mean(0)
    mag = np.maximum(mag - over * noise, 0.05 * mag)
    y = np.fft.irfft(mag * np.exp(1j * ph), n=n, axis=1) * w
    out = np.zeros(len(a), np.float32)
    norm = np.zeros(len(a), np.float32)
    for k, i in enumerate(idx):
        out[i:i + n] += y[k]
        norm[i:i + n] += w * w
    return (out / np.maximum(norm, 1e-3)).astype(np.float32)


def _tempo(a, f):
    """slow down/speed up by resampling (pitch moves too; fine for a retry)"""
    n = int(round(len(a) / f))
    return np.interp(np.linspace(0, len(a) - 1, n), np.arange(len(a)), a).astype(np.float32)


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
def _which_abs(name):
    """`name` on PATH, only as an absolute path to an executable file (relative PATH entries are ignored)."""
    w = shutil.which(name)
    return G.checked_exe(w)[0] if w and os.path.isabs(w) else None


def mic_command(device=None):
    env = G.mic_env_command()   # None when unset or refused (logged): fall back to the default recorder
    if env:
        return env
    device = device or mic_config().get("device") or os.environ.get("LILLITH_MIC_DEVICE")
    if device is not None and not (isinstance(device, str) and G.mic_arg_ok(device)):
        print(f"lillith-voice: mic device {device!r} refused (letters, digits, _.,:=+@%-); using the default device",
              file=sys.stderr, flush=True)
        device = None
    pw = _which_abs("pw-record")
    if pw:
        c = [pw, "--rate", str(RATE), "--channels", "1", "--format", "s16"]
        return c + (["--target", device] if device else []) + ["-"]
    pa = _which_abs("parec")
    if pa:
        c = [pa, "--raw", f"--rate={RATE}", "--channels=1", "--format=s16le", "--latency-msec=30"]
        return c + ([f"--device={device}"] if device else [])
    ar = _which_abs("arecord")
    if ar:
        return [ar, "-q", "-t", "raw", "-f", "S16_LE", "-r", str(RATE), "-c", "1"] + (["-D", device] if device else [])
    return None


def mic_argv():
    """What a capture start would run (shown on the mic card and in the audit log)."""
    return mic_command() or ["(sounddevice)"]


def list_devices():
    out = []
    pactl, arecord = _which_abs("pactl"), _which_abs("arecord")
    if pactl:
        argv = [pactl, "list", "short", "sources"]
        G.audit_once("mic_devices", argv)
        r = subprocess.run(argv, capture_output=True, text=True)
        for ln in r.stdout.splitlines():
            p = ln.split("\t")
            if len(p) > 1 and ".monitor" not in p[1]:
                out.append({"id": p[1], "desc": " ".join(p[2:])})
    elif arecord:
        argv = [arecord, "-L"]
        G.audit_once("mic_devices", argv)
        r = subprocess.run(argv, capture_output=True, text=True)
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

    def start(self, card=None):
        """Starts capture. Callers go through the mic gate first (Listener); `card` is the approving card id."""
        if self.running:
            return
        self.running = True
        cmd = mic_command(self.device)
        G.audit(G.LAUNCH, "mic_capture", cmd or ["(sounddevice)"], card)
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

    def __init__(self, speaker=None, emit=None, echo=None, barge_in=None, threads=2, gate=None):
        cfg = mic_config()
        self.gate = gate or MicGate(mic_argv=lambda: mic_command(self.device) or ["(sounddevice)"])
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
        sp = len(seg) / RATE
        text = self.rec.transcribe(seg, sp)
        if DEBUG:
            print(f"lillith-voice: segment {len(seg) / RATE:.2f}s mode={self._mode} t={time.monotonic():.1f} -> {text!r}",
                  file=sys.stderr, flush=True)
        if not text:
            return
        info = {"ev": "transcript", "text": text, "audio_s": round(len(seg) / RATE, 2),
                "stt_s": round(time.time() - t0, 2), **classify(text, sp)}
        if info["safeword"] and self.speaker:
            self.speaker.stop(reason="safeword")
            self.emit({"ev": "safeword", "text": text})
        if self._mode == "once" and self._once is not None:
            self._once["result"] = info
            self._once["done"].set()
        self.emit(info)

    # ---------------------------------------------------------------- API
    def ptt_start(self, token=None):
        if not self.mic.running:
            refused = self.gate.allow("listen", token)
            if refused is not None:
                return refused
        if self._playing() or (self.speaker and self.speaker.busy()):
            self._barge("ptt")          # pressing talk always interrupts her
        with self._lock:
            self._ptt = []
            self._mode = "ptt"
        self.mic.start(card=self.gate.last_card)
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
        voiced_s = len(speech) / RATE
        if len(speech) and len(audio) <= RATE * 28:
            speech = np.asarray(audio, np.float32)   # short clip with speech: keep the natural edges
        elif not len(speech) and _voiced(audio) and len(audio) <= RATE * 3:
            speech = np.asarray(audio, np.float32)   # VAD missed a lone short word (quiet/clipped): still try
            voiced_s = speech_seconds(speech)
        text = self.rec.transcribe(speech, voiced_s) if len(speech) else ""
        info = {"ok": True, "text": text, "audio_s": round(len(audio) / RATE, 2), "speech_s": round(len(speech) / RATE, 2),
                "stt_s": round(time.time() - t0, 2), **classify(text, voiced_s)}
        if info["safeword"] and self.speaker:
            self.speaker.stop(reason="safeword")
            self.emit({"ev": "safeword", "text": text})
        self.emit({"ev": "transcript", **{k: v for k, v in info.items() if k != "ok"}})
        return info

    def listen_once(self, timeout=15.0, token=None):
        if not self.mic.running:
            refused = self.gate.allow("listen", token)
            if refused is not None:
                return refused
        if self._playing() or (self.speaker and self.speaker.busy()):
            self._barge("listen")
        done = threading.Event()
        self._once = {"done": done, "result": None}
        with self._lock:
            self.vad.reset()
            self._mode = "once"
        self.mic.start(card=self.gate.last_card)
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

    def set_always(self, on: bool, token=None):
        if on and not self.always:
            refused = self.gate.allow("always", token)   # every switch-on needs its own card / token
            if refused is not None:
                return refused
        self.always = bool(on)
        with self._lock:
            if on:
                self.vad.reset()
                self._mode = "always"
            elif self._mode == "always":
                self._mode = None
        if on:
            self.mic.start(card=self.gate.last_card)
        else:
            self.mic.stop()
        self.emit({"ev": "listen_mode", "on": self.always})
        return {"ok": True, "always": self.always}
