"""numpy-only DSP for Lillith's cloned voice ("polish" chain) + helpers shared with the P1 chain.

Pure numpy on purpose: identical on Linux, Windows (no ffmpeg needed) and portable to Android (Kotlin/Rust).
  polish():  trim -> HPF 90 Hz -> gentle laptop EQ (-1.5 dB @250, +1.5 dB @3 kHz, -3 dB shelf > 9 kHz)
             -> STFT de-esser (5-10 kHz, max -6 dB) -> partial loudness match (-17 LUFS-ish) -> limiter
             -> 8 ms fades.  No pitch shifting, no reverb: the clone already sounds like Ko's sample.
"""
import numpy as np

# --------------------------------------------------------------------------- basic helpers


def fade(a, sr, ms_in=8.0, ms_out=12.0):
    a = np.asarray(a, np.float32).copy()
    ni, no = min(len(a), int(sr * ms_in / 1000)), min(len(a), int(sr * ms_out / 1000))
    if ni > 1:
        a[:ni] *= np.sin(np.linspace(0, np.pi / 2, ni)) ** 2
    if no > 1:
        a[-no:] *= np.cos(np.linspace(0, np.pi / 2, no)) ** 2
    return a


def trim(a, sr, thresh_db=-48.0, pre_ms=40.0, post_ms=90.0):
    """Drop leading/trailing near-silence but keep breath-room so starts/ends never sound clipped."""
    if len(a) == 0:
        return a
    hop = max(1, int(sr * 0.005))
    n = len(a) // hop
    if n < 2:
        return a
    e = np.sqrt(np.mean(a[: n * hop].reshape(n, hop) ** 2, axis=1) + 1e-12)
    ref = np.percentile(e, 95)
    on = np.nonzero(20 * np.log10(e / (ref + 1e-12) + 1e-12) > thresh_db)[0]
    if not len(on):
        return a
    s = max(0, on[0] * hop - int(sr * pre_ms / 1000))
    t = min(len(a), (on[-1] + 1) * hop + int(sr * post_ms / 1000))
    return a[s:t]


def crossfade_concat(blocks, sr, ms=6.0):
    """Join streamed decoder blocks with a tiny equal-power crossfade (no clicks at block seams)."""
    blocks = [np.asarray(b, np.float32) for b in blocks if len(b)]
    if not blocks:
        return np.zeros(0, np.float32)
    out = blocks[0]
    n = int(sr * ms / 1000)
    for b in blocks[1:]:
        k = min(n, len(out), len(b))
        if k < 2:
            out = np.concatenate([out, b])
            continue
        w = np.linspace(0, np.pi / 2, k)
        mid = out[-k:] * np.cos(w) + b[:k] * np.sin(w)
        out = np.concatenate([out[:-k], mid, b[k:]])
    return out


def gap(sr, seconds, room_db=None, rng=None):
    n = int(sr * max(0.0, seconds))
    if room_db is None:
        return np.zeros(n, np.float32)
    rng = rng or np.random.default_rng(0)
    w = rng.standard_normal(n)
    # cheap pink-ish noise (one-pole lowpass of white)
    y = np.empty(n)
    acc = 0.0
    for i, v in enumerate(w):
        acc = 0.98 * acc + 0.02 * v
        y[i] = acc
    y /= (np.sqrt(np.mean(y ** 2)) + 1e-9)
    return (y * 10 ** (room_db / 20)).astype(np.float32)


# --------------------------------------------------------------------------- frequency-domain EQ


def _bell_db(f, f0, q, g_db):
    # smooth log-frequency bell (good enough for broad tonal moves)
    bw = 1.0 / max(q, 1e-3)
    x = np.log2(np.maximum(f, 1.0) / f0) / bw
    return g_db * np.exp(-0.5 * (2.0 * x) ** 2)


def _shelf_db(f, f0, g_db, slope_oct=1.0):
    x = np.log2(np.maximum(f, 1.0) / f0) / slope_oct
    return g_db / (1.0 + np.exp(-4.0 * x))


def eq_curve(f, highpass=90.0, bells=((250, 1.0, -1.5), (3000, 0.9, 1.5)), hishelf=(9000, -3.0)):
    db = np.zeros_like(f, dtype=np.float64)
    for f0, q, g in bells:
        db += _bell_db(f, f0, q, g)
    if hishelf:
        db += _shelf_db(f, hishelf[0], hishelf[1])
    mag = 10 ** (db / 20)
    if highpass:
        r = (np.maximum(f, 1e-3) / highpass) ** 2   # 2nd-order Butterworth magnitude
        mag *= r / np.sqrt(1 + r ** 2)
    return mag


def stft(x, n=1024, hop=256):
    w = np.hanning(n).astype(np.float32)
    pad = np.concatenate([np.zeros(n, np.float32), x, np.zeros(n, np.float32)])
    frames = 1 + (len(pad) - n) // hop
    idx = np.arange(n)[None, :] + hop * np.arange(frames)[:, None]
    return np.fft.rfft(pad[idx] * w, axis=1), w, len(pad)


def istft(X, w, n_pad, x_len, n=1024, hop=256):
    fr = np.fft.irfft(X, n, axis=1) * w
    out = np.zeros(n_pad, np.float64)
    norm = np.zeros(n_pad, np.float64)
    for i in range(fr.shape[0]):
        out[i * hop:i * hop + n] += fr[i]
        norm[i * hop:i * hop + n] += w ** 2
    out /= np.maximum(norm, 1e-6)
    return out[n:n + x_len].astype(np.float32)


def eq_deess(x, sr, highpass=90.0, bells=((250, 1.0, -1.5), (3000, 0.9, 1.5)), hishelf=(9000, -3.0),
             deess_db=6.0, deess_band=(5000, 10000), deess_thresh=0.35):
    """Static EQ + dynamic de-esser in one STFT pass (zero-phase, no ringing on short chunks)."""
    if len(x) < 256:
        return x
    n, hop = 1024, 256
    X, w, n_pad = stft(x, n, hop)
    f = np.fft.rfftfreq(n, 1 / sr)
    X *= eq_curve(f, highpass, bells, hishelf)[None, :]
    if deess_db > 0:
        band = (f >= deess_band[0]) & (f <= deess_band[1])
        p = np.abs(X) ** 2
        ratio = p[:, band].sum(1) / (p.sum(1) + 1e-12)       # share of energy in the 's' band
        loud = p.sum(1) > 1e-4 * p.sum(1).max()
        over = np.clip((ratio - deess_thresh) / (1 - deess_thresh), 0, 1) * loud
        # smooth over ~20 ms so it never pumps
        k = np.ones(3) / 3
        over = np.convolve(over, k, mode="same")
        g = 10 ** (-deess_db * over / 20)
        X[:, band] *= g[:, None]
    return istft(X, w, n_pad, len(x), n, hop)


# --------------------------------------------------------------------------- loudness


def loudness(x, sr):
    """Approximate integrated loudness (gated RMS with a crude K-weighting tilt), in 'LUFS'."""
    if len(x) < sr // 10:
        return None
    X = np.fft.rfft(x)
    f = np.fft.rfftfreq(len(x), 1 / sr)
    X = X * (10 ** (_shelf_db(f, 1500, 4.0) / 20)) * eq_curve(f, 60, (), None)
    y = np.fft.irfft(X, len(x))
    b = int(0.4 * sr)
    hop = b // 4
    if len(y) < b:
        e = np.array([np.mean(y ** 2)])
    else:
        e = np.array([np.mean(y[i:i + b] ** 2) for i in range(0, len(y) - b + 1, hop)])
    e = e[e > 10 ** (-70 / 10)]
    if not len(e):
        return None
    rel = np.mean(e) * 10 ** (-10 / 10)
    e = e[e > rel]
    return float(-0.691 + 10 * np.log10(np.mean(e)))


def limit(y, sr, ceiling, look_ms=4.0, release_ms=60.0):
    from numpy.lib.stride_tricks import sliding_window_view as swv
    w = max(3, int(look_ms * sr / 1000) | 1)
    env = np.abs(y)
    if not len(y) or env.max() <= ceiling:
        return y
    pad = np.pad(env, (w // 2, w // 2), mode="edge")
    peak = swv(pad, w).max(axis=1)
    g = np.minimum(1.0, ceiling / np.maximum(peak, 1e-9))
    r = max(3, int(release_ms * sr / 1000) | 1)
    g = swv(np.pad(g, (r - 1, 0), mode="edge"), r).min(axis=1)
    g = np.convolve(np.pad(g, (w // 2, w // 2), mode="edge"), np.ones(w) / w, mode="valid")
    return np.clip(y * g, -ceiling, ceiling)


def polish(a, sr, lufs=-17.0, true_peak=-1.5, highpass=90.0, bells=((250, 1.0, -1.5), (3000, 0.9, 1.5)),
           hishelf=(9000, -3.0), deess_db=6.0, ref_gain_db=None):
    """Full 'lillith' chain for one sentence. ref_gain_db: reuse an utterance-level gain so all
    sentences of a reply sit at the same level (natural dynamics instead of per-sentence AGC)."""
    a = trim(np.asarray(a, np.float32), sr)
    if not len(a):
        return a, ref_gain_db
    y = eq_deess(a, sr, highpass, bells, hishelf, deess_db)
    if ref_gain_db is None:
        L = loudness(y, sr)
        ref_gain_db = 0.0 if L is None else float(np.clip(lufs - L, -18, 18))
    y = y * 10 ** (ref_gain_db / 20)
    y = limit(y, sr, 10 ** (true_peak / 20))
    return fade(y, sr), ref_gain_db


# --------------------------------------------------------------------------- pitch (YIN, from voice_samples/f0.py)


def median_f0(a, sr, fmin=120, fmax=600):
    """Median F0 in Hz - same YIN-style logic as voice_samples/f0.py (40 ms frames, 10 ms hop,
    threshold 0.15, frames under 3% of peak skipped), vectorised per frame."""
    from numpy.lib.stride_tricks import sliding_window_view as swv
    a = np.asarray(a, np.float64)
    fl, hop = int(0.04 * sr), int(0.01 * sr)
    lo, hi = int(sr / fmax), int(sr / fmin)
    thr = 0.03 * (np.max(np.abs(a)) if len(a) else 0)
    f0 = []
    for i in range(0, len(a) - fl - hi, hop):
        x = a[i:i + fl + hi]
        if np.sqrt(np.mean(x[:fl] ** 2)) < thr:
            continue
        lag = swv(x[1:], fl)[:hi]
        d = np.sum((x[:fl][None, :] - lag) ** 2, axis=1)
        c = d / (np.cumsum(d) / np.arange(1, hi + 1) + 1e-12)
        cand = np.where(c[lo - 1:] < 0.15)[0]
        if len(cand):
            f0.append(sr / (cand[0] + lo))
    return (float(np.median(f0)) if f0 else 0.0), len(f0)


# --------------------------------------------------------------------------- lipsync


def envelope(a, sr, fps=30, ref=None):
    """RMS per 1/fps frame -> [[t_ms, rms0..1], ...]; ref = utterance normaliser (95th pct RMS)."""
    hop = max(1, int(round(sr / fps)))
    n = int(np.ceil(len(a) / hop)) if len(a) else 0
    if n == 0:
        return [], ref
    pad = np.zeros(n * hop, np.float32)
    pad[:len(a)] = a
    r = np.sqrt(np.mean(pad.reshape(n, hop) ** 2, axis=1))
    if ref is None:
        voiced = r[r > 1e-4]
        ref = float(np.percentile(voiced, 95)) if len(voiced) else 1.0
        ref = max(ref, 1e-4)
    v = np.clip(r / ref, 0, 1)
    return [[int(round(i * 1000 / fps)), round(float(x), 3)] for i, x in enumerate(v)], ref
