"""Lillith's voice engines (Pocket TTS clone of Ko's sample = default, Kokoro presets) + DSP.
Offline only (netguard is imported first)."""
from . import netguard  # noqa: F401  must stay first

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unicodedata
import wave
from dataclasses import dataclass, field, asdict

import numpy as np

from . import launchguard as G

SR = 24000  # Kokoro's native sample rate

# --------------------------------------------------------------------------- paths
DATA_HOME = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
CONFIG_HOME = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
VOICE_HOME = os.environ.get("LILLITH_VOICE_HOME") or os.path.join(DATA_HOME, "lillith-voice")
MODEL_DIR = os.environ.get("LILLITH_VOICE_MODELS") or os.path.join(VOICE_HOME, "models")
MODEL_FILE = os.environ.get("LILLITH_VOICE_MODEL_FILE", "kokoro-v1.0.onnx")  # or kokoro-v1.0.int8.onnx
VOICES_FILE = "voices-v1.0.bin"
POCKET_DIR = os.environ.get("LILLITH_POCKET_DIR") or os.path.join(MODEL_DIR, "pocket")
# which Pocket graphs run int8 (smaller/faster) - default chosen by similarity tests, see README
POCKET_INT8 = os.environ.get("LILLITH_POCKET_INT8", "")
PRESETS_OVERRIDE = os.path.join(CONFIG_HOME, "lillith-voice", "presets.json")


def runtime_dir() -> str:
    base = os.environ.get("XDG_RUNTIME_DIR") or tempfile.gettempdir()
    d = os.path.join(base, f"lillith-voice-{os.getuid()}" if base == tempfile.gettempdir() else "lillith-voice")
    os.makedirs(d, mode=0o700, exist_ok=True)
    return d


def socket_path() -> str:
    return os.environ.get("LILLITH_VOICE_SOCKET") or os.path.join(runtime_dir(), "voiced.sock")


# --------------------------------------------------------------------------- presets
@dataclass
class Preset:
    name: str
    mix: dict = field(default_factory=dict)  # Kokoro: {kokoro_voice: weight}
    speed: float = 1.0        # Kokoro speed (0.5..2.0); Pocket: tempo via rubberband (1.0 = untouched)
    pitch: float = 2.0        # semitones, formant-preserving (rubberband); P1 ignores it (F0 target instead)
    highpass: float = 90.0    # Hz
    eq: list = field(default_factory=lambda: [[300, 1.0, -2.0], [9000, 0.8, 4.0]])  # [Hz, width(q), dB]
    echo: str = "0.8:0.85:28|47:0.06|0.03"  # ffmpeg aecho in_gain:out_gain:delays:decays ("" = off)
    lufs: float = -16.0
    true_peak: float = -1.0
    note: str = ""
    engine: str = "kokoro"    # "pocket" (cloned voice, ONNX) | "kokoro"
    chain: str = "legacy"     # "polish" (numpy, cloned voice) | "p1" (ffmpeg, F0-targeted) | "legacy" | "none"
    voice: str = ""           # Pocket: voice embedding name in models/pocket/voices/<voice>.npy (or a path)
    temperature: float = 0.7  # Pocket sampling temperature (expressiveness: 0.5 calm .. 0.9 lively)
    seed: int = 1234          # Pocket: fixed seed -> the same line sounds the same (None = random)
    f0_target: float = 0.0    # P1: per-line formant-preserving shift to this median F0 (Hz)
    lift: float = 0.0         # P1: formant-SHIFTED lift (semitones) before the F0 shift
    treble: list = field(default_factory=list)  # P1: [gain_dB, Hz] treble shelf
    out_rate: int = 24000     # P1 renders at 44.1 kHz
    gap_scale: float = 1.0    # pause length between sentences (x natural default)
    int8: str = ""            # Pocket: graphs to run int8, e.g. "flow_lm_main" (faster, less like Ko).
    #   Do NOT combine flow_lm_main+flow_lm_flow int8: tested, the errors compound and she loops ("so, so, so...").
    #   flow_lm_main int8 alone (4x smaller transformer) is also unreliable: in 2 of 6 test renders she stopped early
    #   or said "Yeah." instead of the line, and once produced gibberish. Only flow_lm_flow int8 was 6/6 correct.


PRESETS = {
    "lillith": Preset("lillith", engine="pocket", chain="polish", voice="lillith", speed=1.0, pitch=0.0,
                      highpass=90.0, eq=[[250, 1.0, -1.5], [3000, 0.9, 1.5]], echo="", lufs=-17.0,
                      true_peak=-1.5, temperature=0.7,
                      note="DEFAULT - Ko's sample, cloned (Pocket TTS ONNX); gentle laptop-speaker polish"),
    "lillith_fast": Preset("lillith_fast", engine="pocket", chain="polish", voice="lillith", speed=1.0, pitch=0.0,
                           highpass=90.0, eq=[[250, 1.0, -1.5], [3000, 0.9, 1.5]], echo="", lufs=-17.0,
                           true_peak=-1.5, temperature=0.7, int8="flow_lm_flow",
                           note="Ko's clone, int8 flow head (a little faster, a little less RAM, same words)"),
    "p1": Preset("p1", {"af_bella": 1.0}, 0.95, 0.0, 90.0,
                 [[300, 1.0, -2.0], [9000, 0.8, 5.0]], "0.8:0.85:28|47:0.12|0.07", -16.0, -1.0,
                 "P1_A1style_290: af_bella 0.95, +1 st formant-shifted, F0->290 Hz; Kokoro fallback",
                 chain="p1", f0_target=290.0, lift=1.0, treble=[2.0, 12000], out_rate=44100),
    "bella": Preset("bella", {"af_bella": 1.0}, 1.00, 2.0,
                    note="VB1: af_bella, +2 st (previous default)"),
    "bella85_heart15": Preset("bella85_heart15", {"af_bella": 0.85, "af_heart": 0.15}, 1.02, 2.0,
                              note="VB2: sweeter"),
    "bella80_nicole20": Preset("bella80_nicole20", {"af_bella": 0.80, "af_nicole": 0.20}, 0.96, 2.5,
                               note="VB3: whispery, a touch slower"),
    "bella75_sky25": Preset("bella75_sky25", {"af_bella": 0.75, "af_sky": 0.25}, 1.03, 1.5,
                            note="VB4: lower, lazier"),
}
ALIASES = {"default": "lillith", "fast": "lillith_fast", "ko": "lillith", "clone": "lillith", "p": "p1", "vb1": "bella",
           "heart": "bella85_heart15", "vb2": "bella85_heart15", "nicole": "bella80_nicole20",
           "vb3": "bella80_nicole20", "sky": "bella75_sky25", "vb4": "bella75_sky25"}
DEFAULT_PRESET = os.environ.get("LILLITH_VOICE_PRESET", "lillith")
FALLBACK_PRESET = os.environ.get("LILLITH_VOICE_FALLBACK", "p1")  # when the cloner can't run here


def _load_overrides():
    """~/.config/lillith-voice/presets.json: {"name": {"mix": {...}, "speed": .., "pitch": ..}, ...}"""
    try:
        with open(PRESETS_OVERRIDE, encoding="utf-8") as f:
            data = json.load(f)
    except FileNotFoundError:
        return
    except Exception as e:  # never die on a bad user file
        print(f"lillith-voice: ignoring {PRESETS_OVERRIDE}: {e}", file=sys.stderr)
        return
    for name, d in data.items():
        if name == "default" and isinstance(d, str):
            global DEFAULT_PRESET
            DEFAULT_PRESET = d
            continue
        if not isinstance(d, dict):
            print(f"lillith-voice: ignoring preset {name!r} in {PRESETS_OVERRIDE}: not an object", file=sys.stderr)
            continue
        d = G.sanitize_override(name, d)   # numbers clamped, echo numbers-only: nothing odd reaches ffmpeg
        dflt = "bella" if ("mix" in d and "engine" not in d) else "lillith"
        base = asdict(PRESETS.get(name, PRESETS[dflt]))
        base.update(d)
        base["name"] = name
        PRESETS[name] = Preset(**base)


_load_overrides()


def get_preset(name=None) -> Preset:
    name = (name or DEFAULT_PRESET).strip()
    name = ALIASES.get(name.lower(), name)
    if name not in PRESETS:
        raise SystemExit(f"unknown preset {name!r}; have: {', '.join(PRESETS)}")
    return PRESETS[name]


# --------------------------------------------------------------------------- text
_EMOJI_CATS = {"So", "Sk", "Cs", "Co", "Cn"}
_ACTION_RE = re.compile(r"\*[^*\n]{1,80}\*")
_URL_RE = re.compile(r"https?://\S+|www\.\S+")
_CODE_RE = re.compile(r"```.*?```", re.S)


def clean_text(text: str, actions: str = "drop") -> str:
    """Make LLM output speakable: drop code blocks, URLs, emoji, markdown; *actions* dropped or read."""
    t = _CODE_RE.sub(" ", text)
    t = _URL_RE.sub(" link ", t)
    if actions == "drop":
        t = _ACTION_RE.sub(" ", t)
    t = re.sub(r"<think>.*?</think>", " ", t, flags=re.S)  # reasoning models
    t = "".join(" " if unicodedata.category(c) in _EMOJI_CATS or c in "\u200d\ufe0f" else c for c in t)
    t = re.sub(r"[*_#`~>|\[\]{}]", " ", t)
    t = re.sub(r"\s+", " ", t).strip()
    return t


_SENT_RE = re.compile(r"(?<=[.!?…])[\"')\]]*\s+|\n+")


def split_sentences(text: str, first_max: int = 120, max_len: int = 260):
    """Sentence chunks; the first chunk is kept short so audio starts sooner."""
    parts = [p.strip() for p in _SENT_RE.split(text) if p and p.strip()]
    chunks, cur = [], ""
    for p in parts:
        limit = first_max if not chunks else max_len
        if not cur:
            cur = p
        elif len(cur) + 1 + len(p) <= limit:
            cur += " " + p
        else:
            chunks.append(cur)
            cur = p
        if not chunks and len(cur) >= 25:  # flush first sentence early
            chunks.append(cur)
            cur = ""
    if cur:
        chunks.append(cur)
    # hard-split pathological run-ons at commas / spaces
    out = []
    for c in chunks:
        while len(c) > max_len * 2:
            cut = max(c.rfind(", ", 0, max_len), c.rfind(" ", 0, max_len))
            cut = cut if cut > 0 else max_len
            out.append(c[:cut + 1].strip())
            c = c[cut + 1:].strip()
        if c:
            out.append(c)
    return out


# --------------------------------------------------------------------------- ffmpeg
def _default_ffmpeg():
    w = shutil.which("ffmpeg")
    return G.checked_exe(w)[0] if w and os.path.isabs(w) else None


# $LILLITH_FFMPEG only as an absolute path to an existing executable file (else logged + the default)
FFMPEG = G.env_exe("LILLITH_FFMPEG") or _default_ffmpeg()
_HAS_RB = None


def has_rubberband() -> bool:
    global _HAS_RB
    if _HAS_RB is None:
        _HAS_RB = False
        if FFMPEG and os.environ.get("LILLITH_NO_RUBBERBAND") != "1":
            try:
                G.audit_once("voice_ffmpeg_probe", [FFMPEG, "-hide_banner", "-filters"])
                out = subprocess.run([FFMPEG, "-hide_banner", "-filters"], capture_output=True, text=True,
                                     timeout=20).stdout
                _HAS_RB = bool(re.search(r"^\s*\S+\s+rubberband\s", out, re.M))
            except Exception:
                pass
    return _HAS_RB


def _fft_resample(x: np.ndarray, n_out: int) -> np.ndarray:
    """Band-limited resample (pure numpy)."""
    n = len(x)
    if n == 0 or n_out == n:
        return x.astype(np.float32)
    X = np.fft.rfft(x)
    m = n_out // 2 + 1
    Y = np.zeros(m, dtype=complex)
    k = min(m, len(X))
    Y[:k] = X[:k]
    return (np.fft.irfft(Y, n_out) * (n_out / n)).astype(np.float32)


def _echo_np(x, spec: str):
    spec = G.safe_echo(spec)
    if not spec:
        return x
    ig, og, delays, decays = spec.split(":")
    ig, og = float(ig), float(og)
    y = x * ig
    out = y.copy()
    for d, g in zip(delays.split("|"), decays.split("|")):
        s = int(float(d) * SR / 1000)
        out[s:] += y[:-s] * float(g)
    return out * og


def _onepole_hp(x, fc):
    a = np.exp(-2 * np.pi * fc / SR)
    y = np.empty_like(x)
    prev_x = prev_y = 0.0
    # vectorised enough for short chunks; only used without ffmpeg
    for i, v in enumerate(x):
        prev_y = a * (prev_y + v - prev_x)
        prev_x = v
        y[i] = prev_y
    return y


def _rms_lufs(x):
    # crude fallback when ffmpeg is missing: RMS of non-silent blocks, ~LUFS for speech
    b = 4800
    if len(x) < b:
        r = np.sqrt(np.mean(x ** 2) + 1e-12)
        return 20 * np.log10(r) - 0.7
    blocks = x[: len(x) // b * b].reshape(-1, b)
    e = np.mean(blocks ** 2, axis=1)
    e = e[e > 10 ** (-60 / 10)]
    if not len(e):
        return -70.0
    return 10 * np.log10(np.mean(e)) - 0.7


def _limit(y, ceiling, look_ms=4.0, release_ms=60.0):
    """Look-ahead peak limiter (numpy). Keeps |y| <= ceiling with smooth gain, like loudnorm's
    dynamic fallback does when the -16 LUFS target would otherwise clip."""
    from numpy.lib.stride_tricks import sliding_window_view as swv
    w = max(3, int(look_ms * SR / 1000) | 1)
    env = np.abs(y)
    if env.max() <= ceiling:
        return y
    pad = np.pad(env, (w // 2, w // 2), mode="edge")
    peak = swv(pad, w).max(axis=1)                      # look-ahead window max
    g = np.minimum(1.0, ceiling / np.maximum(peak, 1e-9))
    # release: let gain recover slowly (running min over the release window, then smooth)
    r = max(3, int(release_ms * SR / 1000) | 1)
    gp = np.pad(g, (r - 1, 0), mode="edge")
    g = swv(gp, r).min(axis=1)
    k = np.ones(w) / w
    g = np.convolve(np.pad(g, (w // 2, w // 2), mode="edge"), k, mode="valid")
    out = y * g
    return np.clip(out, -ceiling, ceiling)


def _apply_loudness(y, lufs_measured, peak_db, p: Preset):
    if lufs_measured is None or lufs_measured < -69:
        return y
    gain_db = p.lufs - lufs_measured
    if peak_db is not None:  # allow at most 6 dB of limiting, like a gentle mastering limiter
        gain_db = min(gain_db, p.true_peak - 0.5 - peak_db + 6.0)
    gain_db = max(-24.0, min(24.0, gain_db))
    y = y * (10 ** (gain_db / 20))
    return _limit(y, 10 ** ((p.true_peak - 0.5) / 20)).astype(np.float32)


def process(audio: np.ndarray, p: Preset, pitch: float, fx: bool = True, prepitched: bool = False):
    """Lillith DSP chain. pitch is semitones. Returns float32 @ SR."""
    audio = np.asarray(audio, dtype=np.float32)
    if not fx:
        return audio
    pad = np.zeros(int(0.12 * SR), np.float32)  # room for the echo tail
    x = np.concatenate([audio, pad])
    chain = []
    if pitch and not prepitched:
        chain.append(f"rubberband=pitch={2 ** (pitch / 12):.6f}:formant=preserved:pitchq=quality")
    if p.highpass:
        chain.append(f"highpass=f={p.highpass:g}")
    for f, w, g in p.eq:
        chain.append(f"equalizer=f={f:g}:t=q:w={w:g}:g={g:g}")
    echo = G.safe_echo(p.echo, f"preset {p.name}")   # numbers only, clamped (presets.json is user-editable)
    if echo:
        chain.append(f"aecho={echo}")
    if FFMPEG:
        chain.append("ebur128=peak=sample:framelog=quiet")
        cmd = [FFMPEG, "-hide_banner", "-nostdin", "-nostats", "-loglevel", "info",
               "-protocol_whitelist", "pipe,file",
               "-f", "f32le", "-ar", str(SR), "-ac", "1", "-i", "pipe:0",
               "-af", ",".join(chain), "-f", "f32le", "-ar", str(SR), "-ac", "1", "pipe:1"]
        G.audit_once("voice_ffmpeg_dsp", cmd)
        r = subprocess.run(cmd, input=x.tobytes(), capture_output=True, timeout=120)
        if r.returncode != 0:
            raise RuntimeError("ffmpeg DSP failed: " + r.stderr.decode(errors="replace")[-600:])
        y = np.frombuffer(r.stdout, dtype=np.float32).copy()
        err = r.stderr.decode(errors="replace")
        mi = re.findall(r"^\s*I:\s*(-?[\d.]+|-inf) LUFS", err, re.M)
        mp = re.findall(r"^\s*Peak:\s*(-?[\d.]+|-inf) dBFS", err, re.M)
        lufs = float(mi[-1]) if mi and mi[-1] != "-inf" else None
        peak = float(mp[-1]) if mp and mp[-1] != "-inf" else None
        y = _apply_loudness(y, lufs, peak, p)
    else:  # numpy-only fallback: highpass + echo + loudness (no EQ)
        y = _onepole_hp(x, p.highpass) if p.highpass else x
        y = _echo_np(y, p.echo)
        pk = float(np.max(np.abs(y)) + 1e-9)
        y = _apply_loudness(y, _rms_lufs(y), 20 * np.log10(pk), p)
    # trim trailing near-silence of the pad
    nz = np.nonzero(np.abs(y) > 1e-4)[0]
    if len(nz):
        y = y[: min(len(y), nz[-1] + int(0.03 * SR))]
    return y.astype(np.float32)


# --------------------------------------------------------------------------- engines
def pocket_available(model_dir=None) -> bool:
    d = model_dir or POCKET_DIR
    need = ["bundle.json", "tokenizer.model", "text_conditioner.onnx", "flow_lm_main", "flow_lm_flow", "mimi_decoder"]
    files = set(os.listdir(d)) if os.path.isdir(d) else set()
    return all(any(f == n or f.startswith(n + ".") or f.startswith(n + "_") for f in files) for n in need)


def kokoro_phonemes(kokoro, text):
    """espeak spells 'Mmm' as letters (ɛm ɛm ɛm); force a real hum. Returns phonemes."""
    ph = kokoro.tokenizer.phonemize(text, "en-us")
    return ph.replace("ˌɛmˌɛmˈɛm", "ˈmmmm").replace("ˌɛmˈɛm", "ˈmmm")


def _ffmpeg(chain, x, sr_in, sr_out=None, measure=None):
    cmd = [FFMPEG, "-hide_banner", "-nostdin", "-nostats", "-loglevel", "info", "-protocol_whitelist", "pipe,file",
           "-f", "f32le", "-ar", str(sr_in), "-ac", "1", "-i", "pipe:0", "-af", ",".join(chain),
           "-f", "f32le", "-ar", str(sr_out or sr_in), "-ac", "1", "pipe:1"]
    G.audit_once("voice_ffmpeg_dsp", cmd)
    r = subprocess.run(cmd, input=np.asarray(x, np.float32).tobytes(), capture_output=True, timeout=180)
    if r.returncode != 0:
        raise RuntimeError("ffmpeg DSP failed: " + r.stderr.decode(errors="replace")[-600:])
    return np.frombuffer(r.stdout, dtype=np.float32).copy(), r.stderr.decode(errors="replace")


def process_p1(raw, p: Preset, fx=True):
    """P1 = voice_samples/sexy_proper/post_p.sh, but with a PER-LINE F0 target:
    aresample 44.1k -> rubberband +lift st formant=shifted -> rubberband (F0->target) formant=preserved
    -> highpass -> EQ -> treble shelf -> aecho -> two-pass loudnorm (I, TP, LRA=11, linear)."""
    from .dsp import median_f0, fade
    if not fx:
        return raw, SR, {}
    f0, _ = median_f0(raw, SR)
    lifted = f0 * 2 ** (p.lift / 12) if f0 else 0
    st2 = 12 * np.log2(p.f0_target / lifted) if lifted and p.f0_target else 3.32
    st2 = float(np.clip(st2, -2.0, 8.0))
    rate = p.out_rate or 44100
    if not FFMPEG or not has_rubberband():
        # no ffmpeg/rubberband (e.g. Windows without ffmpeg): resample pitch, then numpy polish
        from .dsp import polish
        r = 2 ** ((p.lift + st2) / 12)
        y = _fft_resample(raw, int(round(len(raw) / r)))
        y, _ = polish(y, SR, p.lufs, p.true_peak, p.highpass, [tuple(e) for e in p.eq], None, 4.0)
        return y, SR, {"f0_in": round(f0, 1), "shift_st": round(st2, 2), "p1": "numpy-fallback"}
    chain = [f"aresample={rate}"]
    if p.lift:
        chain.append(f"rubberband=pitch={2 ** (p.lift / 12):.6f}:formant=shifted:pitchq=quality")
    chain.append(f"rubberband=pitch={2 ** (st2 / 12):.6f}:formant=preserved:pitchq=quality")
    if p.highpass:
        chain.append(f"highpass=f={p.highpass:g}")
    for f, w, g in p.eq:
        chain.append(f"equalizer=f={f:g}:t=q:w={w:g}:g={g:g}")
    if p.treble:
        chain.append(f"treble=g={p.treble[0]:g}:f={p.treble[1]:g}")
    echo = G.safe_echo(p.echo, f"preset {p.name}")   # numbers only, clamped (presets.json is user-editable)
    if echo:
        chain.append(f"aecho={echo}")
    pad = np.zeros(int(0.15 * SR), np.float32)
    x = np.concatenate([raw, pad])
    y, _ = _ffmpeg(chain, x, SR, rate)
    ln = f"loudnorm=I={p.lufs:g}:TP={p.true_peak:g}:LRA=11"
    _, err = _ffmpeg([ln + ":print_format=json"], y, rate)
    m = re.search(r"\{[^{}]*\"input_i\"[^{}]*\}", err, re.S)
    if m:
        j = json.loads(m.group(0))
        if j["input_i"] not in ("-inf", "inf"):
            ln += (f":measured_I={j['input_i']}:measured_TP={j['input_tp']}:measured_LRA={j['input_lra']}"
                   f":measured_thresh={j['input_thresh']}:offset={j['target_offset']}:linear=true")
    y, _ = _ffmpeg([ln, f"aresample={rate}"], y, rate)
    nz = np.nonzero(np.abs(y) > 1e-4)[0]
    if len(nz):
        y = y[: min(len(y), nz[-1] + int(0.03 * rate))]
    return fade(y, rate), rate, {"f0_in": round(f0, 1), "shift_st": round(st2, 2)}


class Voice:
    """All engines behind one render(). Engines load lazily; render() is serialised (thread-safe)."""

    def __init__(self, model_dir: str = MODEL_DIR, threads: int | None = None, preload=None):
        self.model_dir = model_dir
        self.threads = threads or int(os.environ.get("LILLITH_THREADS", "0")) or min(4, os.cpu_count() or 4)
        self._lock = threading.Lock()
        self._kokoro = None
        self._pocket = None
        self._styles = {}
        self.load_s = 0.0
        self._warned = False
        if preload is not False:
            self.engine_for(get_preset(preload if isinstance(preload, str) else None))

    # ---------------------------------------------------------------- loading
    @property
    def kokoro(self):
        if self._kokoro is None:
            import onnxruntime as ort
            from kokoro_onnx import Kokoro
            model = os.path.join(self.model_dir, MODEL_FILE)
            voices = os.path.join(self.model_dir, VOICES_FILE)
            missing = [f for f in (model, voices) if not os.path.isfile(f)]
            if missing:
                raise SystemExit("lillith-voice: Kokoro model files missing:\n  " + "\n  ".join(missing) +
                                 f"\nPut {MODEL_FILE} and {VOICES_FILE} in {self.model_dir} (no downloads are ever made).")
            so = ort.SessionOptions()
            so.intra_op_num_threads = self.threads
            so.inter_op_num_threads = 1
            so.log_severity_level = 3
            so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
            t0 = time.time()
            sess = ort.InferenceSession(model, sess_options=so, providers=["CPUExecutionProvider"])
            self._kokoro = Kokoro.from_session(sess, voices)
            self.load_s += time.time() - t0
        return self._kokoro

    @property
    def pocket(self):
        if self._pocket is None:
            from .pocket import PocketVoice
            t0 = time.time()
            q = set(filter(None, POCKET_INT8.split(",")))
            self._pocket = PocketVoice(POCKET_DIR, threads=self.threads,
                                       precision={k: "int8" for k in q})
            self.load_s += time.time() - t0
        return self._pocket

    def voice_path(self, name):
        if os.path.isfile(name):
            return name
        for d in (os.path.join(VOICE_HOME, "voices"), os.path.join(POCKET_DIR, "voices")):
            f = os.path.join(d, name + ".npy")
            if os.path.isfile(f):
                return f
        raise SystemExit(f"lillith-voice: voice embedding {name!r} not found in {POCKET_DIR}/voices "
                         "(clone one with: lillith-say --clone ref.wav NAME)")

    def engine_for(self, p: Preset) -> Preset:
        """Returns the preset that will really be used (falls back to P1 when the cloner can't run)."""
        if p.engine == "pocket":
            if pocket_available():
                _ = self.pocket
                return p
            if not self._warned:
                print(f"lillith-voice: cloned-voice models not found in {POCKET_DIR}; "
                      f"falling back to preset {FALLBACK_PRESET!r}", file=sys.stderr)
                self._warned = True
            p = get_preset(FALLBACK_PRESET)
        _ = self.kokoro
        return p

    # ---------------------------------------------------------------- synthesis
    def style(self, p: Preset):
        key = tuple(sorted(p.mix.items()))
        if key not in self._styles:
            s = None
            for v, w in p.mix.items():
                s = w * self.kokoro.get_voice_style(v) if s is None else s + w * self.kokoro.get_voice_style(v)
            self._styles[key] = s
        return self._styles[key]

    def synth(self, text: str, p: Preset, speed: float) -> np.ndarray:
        speed = max(0.5, min(2.0, speed))
        with self._lock:
            ph = kokoro_phonemes(self.kokoro, text)
            audio, sr = self.kokoro.create(ph, voice=self.style(p), speed=speed, lang="en-us", is_phonemes=True)
        assert sr == SR
        return np.asarray(audio, dtype=np.float32)

    def synth_pocket(self, text, p: Preset, seed=None, temperature=None):
        from .dsp import crossfade_concat
        pv = self.pocket
        vp = self.voice_path(p.voice or "lillith")
        blocks = []
        with self._lock:
            for i, c in enumerate(pv.split(text)):
                s = None if seed is None else seed + 7919 * i
                it = pv.stream(c, temperature, s, voice=vp, int8=[x for x in p.int8.split(",") if x] or None)
                blocks.append(crossfade_concat(list(it), SR, 3.0))
        return crossfade_concat(blocks, SR, 8.0)

    def render(self, text: str, preset=None, speed=None, pitch=None, fx=True, seed=None, temperature=None,
               gain_ref=None):
        """Text chunk -> processed float32 audio. Returns (audio, stats); stats["sr"] is the audio rate."""
        from .textnorm import normalize
        p = get_preset(preset) if not isinstance(preset, Preset) else preset
        p = self.engine_for(p)
        text = normalize(text)
        sp = p.speed if speed is None else speed
        st = p.pitch if pitch is None else pitch
        t0 = time.time()
        extra = {}
        rate = SR
        if p.engine == "pocket":
            sd = p.seed if seed is None else seed
            if sd is not None:  # stable per text: same line -> same take
                sd = (int(sd) + sum(map(ord, text)) * 31) % (2 ** 31)
            raw = self.synth_pocket(text, p, sd, p.temperature if temperature is None else temperature)
            t1 = time.time()
            if fx and (abs(sp - 1.0) > 0.01 or st) and FFMPEG and has_rubberband():
                ch = []
                if abs(sp - 1.0) > 0.01:
                    ch.append(f"rubberband=tempo={sp:.4f}:pitchq=quality")
                if st:
                    ch.append(f"rubberband=pitch={2 ** (st / 12):.6f}:formant=preserved:pitchq=quality")
                raw, _ = _ffmpeg(ch, raw, SR)
            if fx and p.chain == "polish":
                from .dsp import polish
                out, g = polish(raw, SR, p.lufs, p.true_peak, p.highpass, [tuple(e) for e in p.eq],
                                (9000, -3.0), 6.0, gain_ref)
                extra["gain_db"] = round(g, 2) if g is not None else None
            elif fx and p.chain == "legacy":
                out = process(raw, p, 0, fx=True)
            else:
                out = raw
        else:
            prepitched = False
            if p.chain == "p1":
                raw = self.synth(text, p, sp)
                t1 = time.time()
                out, rate, extra = process_p1(raw, p, fx)
            else:
                if fx and st and not has_rubberband():
                    r = 2 ** (st / 12)
                    raw = self.synth(text, p, sp / r)
                    raw = _fft_resample(raw, int(round(len(raw) / r)))
                    prepitched = True
                else:
                    raw = self.synth(text, p, sp)
                t1 = time.time()
                out = process(raw, p, st, fx=fx, prepitched=prepitched) if p.chain != "none" else raw
        t2 = time.time()
        dur = len(out) / rate
        return out, {"audio_s": dur, "synth_s": t1 - t0, "dsp_s": t2 - t1, "sr": rate,
                     "rtf": (t2 - t0) / dur if dur else 0.0, "preset": p.name, "engine": p.engine, **extra}

    def clone(self, wav, name):
        """Make a new voice embedding from a 24 kHz mono WAV (consent of the speaker required)."""
        import numpy as _np
        e = self.pocket.encode_reference(wav)
        d = os.path.join(VOICE_HOME, "voices")
        os.makedirs(d, exist_ok=True)
        out = os.path.join(d, name + ".npy")
        _np.save(out, e)
        return out


# --------------------------------------------------------------------------- I/O
def to_pcm16(a: np.ndarray) -> bytes:
    return (np.clip(a, -1, 1) * 32767).astype("<i2").tobytes()


def write_wav(path: str, a: np.ndarray, rate: int = SR, src_rate: int = SR):
    if rate != src_rate:
        a = _fft_resample(a, int(round(len(a) * rate / src_rate)))
    tmp = path + ".part"
    with wave.open(tmp, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(to_pcm16(a))
    os.replace(tmp, path)


def player_cmd():
    """Command (list, with '{}' for the wav path) used to play a file."""
    env = os.environ.get("LILLITH_PLAYER")
    if env == "none":
        return None
    if env:
        # shlex (never a shell); the program must be an absolute path to an executable file, else the default
        argv = G.env_command("LILLITH_PLAYER")
        if argv:
            return argv
    for c in (["pw-play", "{}"], ["paplay", "{}"], ["aplay", "-q", "{}"],
              ["ffplay", "-nodisp", "-autoexit", "-loglevel", "quiet", "{}"]):
        w = shutil.which(c[0])
        if w and os.path.isabs(w) and G.checked_exe(w)[0]:
            return [w] + c[1:]
    return None


class Player:
    """Plays chunks back-to-back in a background thread; stop() kills the current one.
    Each item: (audio, gen, done_cb, rate, on_start). on_start(mono_ms) fires when the chunk is expected to be
    AUDIBLE: player spawn time + LILLITH_OUTPUT_LATENCY_MS (device/server latency compensation)."""

    def __init__(self):
        import queue
        self.q = queue.Queue()
        self.cmd = player_cmd()
        self.proc = None
        self.gen = 0
        self.latency_ms = float(os.environ.get("LILLITH_OUTPUT_LATENCY_MS", "60"))
        self._playing_until = 0.0
        self._active = False
        self._lock = threading.Lock()
        threading.Thread(target=self._run, daemon=True, name="lillith-player").start()

    @property
    def playing(self) -> bool:
        """True while audio is (probably) coming out of the speakers, plus a 250 ms echo hangover."""
        return self._active or time.monotonic() < self._playing_until

    def enqueue(self, audio, gen, done_cb=None, rate=SR, on_start=None):
        self.q.put((audio, gen, done_cb, rate, on_start))

    def stop(self):
        with self._lock:
            self.gen += 1
            if self.proc and self.proc.poll() is None:
                self.proc.kill()
            if os.name == "nt" and self.cmd is None:
                try:
                    import winsound
                    winsound.PlaySound(None, 0)
                except Exception:
                    pass
        while not self.q.empty():
            try:
                item = self.q.get_nowait()
                if item[2]:
                    item[2]()
            except Exception:
                break

    def _run(self):
        rd = runtime_dir()
        while True:
            audio, gen, cb, rate, on_start = self.q.get()
            try:
                if audio is None or gen != self.gen:
                    continue
                dur = len(audio) / rate
                if self.cmd is None and os.name == "nt":  # Windows: built-in winsound (WASAPI/MME), no extras
                    import winsound
                    fd, path = tempfile.mkstemp(suffix=".wav", dir=rd)
                    os.close(fd)
                    write_wav(path, audio, rate, src_rate=rate)
                    self._active = True
                    if on_start:
                        threading.Timer(self.latency_ms / 1000, on_start,
                                        (time.monotonic() * 1000 + self.latency_ms,)).start()
                    winsound.PlaySound(path, winsound.SND_FILENAME)  # blocking; stop() -> PlaySound(None)
                    os.unlink(path)
                    continue
                if self.cmd is None:  # no player: keep real-time pacing so callers behave the same
                    self._active = True
                    if on_start:
                        on_start(time.monotonic() * 1000)
                    time.sleep(dur)
                    continue
                fd, path = tempfile.mkstemp(suffix=".wav", dir=rd)
                os.close(fd)
                write_wav(path, audio, rate, src_rate=rate)
                with self._lock:
                    if gen != self.gen:
                        os.unlink(path)
                        continue
                    G.audit_once("voice_player", self.cmd)
                    self.proc = subprocess.Popen([path if a == "{}" else a for a in self.cmd],
                                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                                 stderr=subprocess.DEVNULL)
                    self._active = True
                t_aud = time.monotonic() * 1000 + self.latency_ms
                if on_start:
                    threading.Timer(max(0.0, self.latency_ms / 1000), on_start, (t_aud,)).start()
                self.proc.wait()
                try:
                    os.unlink(path)
                except OSError:
                    pass
            finally:
                self._active = False
                self._playing_until = time.monotonic() + 0.25
                if cb:
                    cb()
