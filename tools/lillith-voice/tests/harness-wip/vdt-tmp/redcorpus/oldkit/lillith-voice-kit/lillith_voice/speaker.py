"""Shared speak pipeline: render sentence N+1 while sentence N plays. Used by the daemon and one-shot mode.

Also the source of lip-sync events (see PROTOCOL.md):
  envelope (30 fps RMS, before the chunk plays), chunk_start (when audible, latency-compensated,
  CLOCK_MONOTONIC ms), chunk_end, utt_end, stop.
"""
from . import netguard  # noqa: F401

import itertools
import queue
import re
import threading
import time

import numpy as np

from .core import SR, Voice, Player, clean_text, split_sentences, get_preset, write_wav
from .dsp import envelope, gap, fade

FPS = 30


def pause_after(chunk: str, scale=1.0) -> float:
    """Natural pause after a sentence, by its final punctuation (seconds)."""
    c = chunk.rstrip(" \"')]")
    if c.endswith(("...", "…")):
        g = 0.42
    elif c.endswith("?"):
        g = 0.30
    elif c.endswith("!"):
        g = 0.26
    elif c.endswith("."):
        g = 0.32
    elif c.endswith((",", ";", ":", "-", "—")):
        g = 0.14
    else:
        g = 0.22
    return g * scale


class Job:
    def __init__(self, kind, text, preset=None, speed=None, pitch=None, fx=True, out=None, rate=None,
                 actions="drop", utt=None):
        self.kind, self.text, self.preset, self.speed, self.pitch = kind, text, preset, speed, pitch
        self.fx, self.out, self.rate, self.actions = fx, out, rate, actions
        self.done = threading.Event()
        self.stats = {}
        self.error = None
        self.gen = 0
        self.utt = utt


class Speaker:
    def __init__(self, voice: Voice | None = None, threads=None, emit=None, preload=None):
        self.voice = voice or Voice(threads=threads, preload=preload)
        self.player = Player()
        self.jobs = queue.Queue()
        self.gen = 0
        self.emit = emit or (lambda ev: None)
        self._utt = itertools.count(1)
        self._active = {}          # utt -> Job (rendering or playing)
        self._lock = threading.Lock()
        threading.Thread(target=self._worker, daemon=True, name="lillith-render").start()

    # ------------------------------------------------------------------ API
    def submit(self, job: Job) -> Job:
        get_preset(job.preset)  # validate early
        job.gen = self.gen
        if job.utt is None:
            job.utt = next(self._utt)
        if job.kind == "say":
            with self._lock:
                self._active[job.utt] = job
        self.jobs.put(job)
        return job

    def busy(self) -> bool:
        return bool(self._active) or self.player.playing

    def stop(self, reason="stop"):
        self.gen += 1
        self.player.stop()
        with self._lock:
            active = list(self._active.items())
            self._active.clear()
        for utt, job in active:
            self.emit({"ev": "stop", "utt": utt, "reason": reason})
            job.stats.setdefault("stopped", True)
            job.done.set()

    def warmup(self):
        self.voice.render("Hi.", get_preset(None))

    # ------------------------------------------------------------------ worker
    def _worker(self):
        while True:
            job = self.jobs.get()
            try:
                if job.gen != self.gen:
                    job.stats = {"skipped": True, "utt": job.utt}
                    job.done.set()
                    continue
                if job.kind == "file":
                    self._file(job)
                else:
                    self._play(job)
            except BaseException as e:  # report, keep serving
                job.error = f"{type(e).__name__}: {e}"
                with self._lock:
                    self._active.pop(job.utt, None)
                job.done.set()

    def _render_all(self, job):
        """All sentences of a job -> one waveform with natural pauses (for files / piper shim)."""
        chunks = split_sentences(clean_text(job.text, job.actions))
        parts, st_all, rate, gref = [], [], SR, None
        for i, c in enumerate(chunks):
            a, st = self.voice.render(c, job.preset, job.speed, job.pitch, job.fx, gain_ref=gref)
            gref = st.get("gain_db", gref)
            rate = st["sr"]
            parts.append(a)
            if i < len(chunks) - 1:
                parts.append(gap(rate, pause_after(c, get_preset(job.preset).gap_scale)))
            st_all.append(st)
        audio = np.concatenate(parts) if parts else np.zeros(0, np.float32)
        return audio, rate, st_all

    def _file(self, job):
        t0 = time.time()
        if not clean_text(job.text, job.actions):
            job.stats = {"audio_s": 0, "empty": True}
            job.done.set()
            return
        audio, rate, sts = self._render_all(job)
        out_rate = job.rate or rate
        write_wav(job.out, audio, out_rate, src_rate=rate)
        job.stats = {"audio_s": round(len(audio) / rate, 3),
                     "compute_s": round(sum(s["synth_s"] + s["dsp_s"] for s in sts), 3),
                     "rtf": round(sum(s["synth_s"] + s["dsp_s"] for s in sts) / max(1e-6, len(audio) / rate), 3),
                     "wall_s": round(time.time() - t0, 3), "out": job.out, "rate": out_rate,
                     "preset": sts[0]["preset"] if sts else None, "engine": sts[0]["engine"] if sts else None,
                     "chunks": len(sts)}
        job.done.set()

    def _play(self, job):
        t0 = time.time()
        chunks = split_sentences(clean_text(job.text, job.actions))
        utt = job.utt
        if not chunks:  # empty say = barrier: done once everything queued before it has played
            job.stats = {"audio_s": 0, "empty": True, "utt": utt}
            with self._lock:
                self._active.pop(utt, None)
            self.player.enqueue(None, self.player.gen, job.done.set)
            return
        audio_s = compute_s = 0.0
        first = None
        n = len(chunks)
        env_ref = gref = None
        scale = get_preset(job.preset).gap_scale
        job.stats["utt"] = utt
        for i, c in enumerate(chunks):
            if job.gen != self.gen:
                job.stats.update({"stopped": True})
                job.done.set()
                return
            a, st = self.voice.render(c, job.preset, job.speed, job.pitch, job.fx, gain_ref=gref)
            gref = st.get("gain_db", gref)
            rate = st["sr"]
            compute_s += st["synth_s"] + st["dsp_s"]
            audio_s += st["audio_s"]
            if first is None:
                first = time.time() - t0
            last = i == n - 1
            pts, env_ref = envelope(a, rate, FPS, env_ref)
            if not last:  # natural pause: part of this chunk, so the next chunk starts on time
                g = gap(rate, pause_after(c, scale))
                a = np.concatenate([fade(a, rate, 0, 6), g])
            chunk_ms = int(round(len(a) * 1000 / rate))
            gen = self.player.gen
            if job.gen != self.gen:
                continue
            self.emit({"ev": "envelope", "utt": utt, "chunk": i, "fps": FPS, "dur_ms": chunk_ms, "pts": pts,
                       "text": c})
            if last:
                job.stats.update({"audio_s": round(audio_s, 3), "compute_s": round(compute_s, 3),
                                  "rtf": round(compute_s / audio_s, 3) if audio_s else 0,
                                  "first_audio_s": round(first, 3), "chunks": n,
                                  "preset": st["preset"], "engine": st["engine"]})

            def on_start(mono_ms, i=i, gen=gen, chunk_ms=chunk_ms):
                if gen == self.player.gen:
                    self.emit({"ev": "chunk_start", "utt": utt, "chunk": i, "mono_ms": int(mono_ms),
                               "dur_ms": chunk_ms})

            def cb(job=job, last=last, i=i, gen=gen):
                if gen != self.player.gen:
                    return
                self.emit({"ev": "chunk_end", "utt": utt, "chunk": i})
                if last:
                    job.stats["played_s"] = round(time.time() - t0, 3)
                    with self._lock:
                        self._active.pop(utt, None)
                    self.emit({"ev": "utt_end", "utt": utt})
                    job.done.set()
            self.player.enqueue(a, gen, cb, rate, on_start)
