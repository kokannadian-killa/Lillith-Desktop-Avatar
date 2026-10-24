"""Pocket TTS (Kyutai, CC-BY-4.0 weights) on onnxruntime: Lillith's cloned voice.

Adapted from the Apache-2.0 reference runtime in KevinAHM/pocket-tts-onnx (pocket_tts_onnx.py),
trimmed to what the kit needs and made fully offline:
  * no huggingface_hub, no downloads; every file comes from the local model dir
  * the voice is a precomputed embedding (lillith_voice.npy, from Ko's sample) and the
    conditioned KV-cache state is computed ONCE and cloned per sentence
  * streaming generator (decoded audio in small chunks) for low first-audio latency
  * seeded sampling so a given line sounds the same every time unless you ask otherwise

Runs the same on Linux, Windows and (as a port) Android: only onnxruntime + numpy + sentencepiece.
"""
from . import netguard  # noqa: F401  must stay first

import json
import os
import time
from pathlib import Path

import numpy as np

SR = 24000
FILES = ("bundle.json", "tokenizer.model", "bos_before_voice.npy", "text_conditioner.onnx",
         "flow_lm_main", "flow_lm_flow", "mimi_decoder")


def _dtype(s):
    return {"float32": np.float32, "float16": np.float16, "int64": np.int64, "bool": np.bool_}[s]


def _fill(shape, dt, fill):
    if fill == "nan":
        return np.full(shape, np.nan, dtype=dt)
    if fill == "ones":
        return np.ones(shape, dtype=dt)
    return np.zeros(shape, dtype=dt)


class PocketVoice:
    """One loaded Pocket TTS bundle + one cloned voice."""

    def __init__(self, bundle_dir, voice_npy=None, threads=4, precision=None, temperature=0.7,
                 lsd_steps=1, encoder_ok=True):
        import onnxruntime as ort
        import sentencepiece as spm

        self.dir = Path(bundle_dir)
        meta = json.loads((self.dir / "bundle.json").read_text())
        self.meta = meta
        self.sr = int(meta["sample_rate"])
        self.frame_rate = float(meta["frame_rate"])
        self.latent_dim = int(meta["latent_dim"])
        self.cond_dim = int(meta["conditioning_dim"])
        self.max_tok = int(meta.get("max_token_per_chunk", 50))
        self.flow_manifest = meta["flow_lm_state_manifest"]
        self.mimi_manifest = meta["mimi_state_manifest"]
        self.temperature = temperature
        self.lsd_steps = lsd_steps
        self.tok = spm.SentencePieceProcessor()
        self.tok.Load(str(self.dir / meta["tokenizer_file"]))
        bos = meta.get("bos_before_voice_file")
        self.bos = np.load(self.dir / bos).astype(np.float32) if bos and meta.get("insert_bos_before_voice") else None

        # precision per component: default fp32 (best similarity to Ko's sample);
        # int8 for flow_lm_main / flow_lm_flow is the "fast" option (decoder int8 hurts similarity most)
        self.default_int8 = frozenset(k for k, v in (precision or {}).items() if v == "int8")
        so = ort.SessionOptions()
        so.intra_op_num_threads = threads
        so.inter_op_num_threads = 1
        so.log_severity_level = 3
        so.enable_cpu_mem_arena = os.environ.get("LILLITH_ORT_ARENA", "0") == "1"  # ~1 GB less RSS
        so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
        self._so, self._prov, self._ort = so, ["CPUExecutionProvider"], ort
        self._sessions = {}
        t0 = time.time()
        self.text_cond = self._sess("text_conditioner", False)
        self.main, self.flow, self.dec = self._graphs(self.default_int8)
        self.encoder = None
        self.load_s = time.time() - t0
        self.threads = threads
        self._st = [(np.array([[j / lsd_steps]], np.float32), np.array([[(j + 1) / lsd_steps]], np.float32))
                    for j in range(lsd_steps)]
        self._voice_states = {}
        self.voice_npy = voice_npy

    def _sess(self, stem, int8):
        f = self.dir / (f"{stem}_int8.onnx" if int8 else f"{stem}.onnx")
        if not f.exists():  # bundle may ship only one precision
            f = self.dir / (f"{stem}.onnx" if int8 else f"{stem}_int8.onnx")
        if f.name not in self._sessions:
            self._sessions[f.name] = self._ort.InferenceSession(str(f), sess_options=self._so, providers=self._prov)
        return self._sessions[f.name]

    def _graphs(self, int8):
        return tuple(self._sess(n, n in int8) for n in ("flow_lm_main", "flow_lm_flow", "mimi_decoder"))

    @property
    def files(self):
        return sorted(self._sessions)

    # ------------------------------------------------------------------ voice
    def encode_reference(self, wav_path) -> np.ndarray:
        """Reference WAV (24 kHz mono) -> voice embedding. Needs mimi_encoder.onnx (shipped)."""
        import onnxruntime as ort
        import wave
        if self.encoder is None:
            self.encoder = self._sess("mimi_encoder", False)
        with wave.open(str(wav_path), "rb") as w:
            if w.getframerate() != self.sr or w.getnchannels() != 1 or w.getsampwidth() != 2:
                raise ValueError("reference must be 24 kHz mono 16-bit WAV "
                                 "(ffmpeg -i in.mp3 -ac 1 -ar 24000 -c:a pcm_s16le ref.wav)")
            a = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(np.float32) / 32768.0
        e = self.encoder.run(None, {"audio": a.reshape(1, 1, -1)})[0]
        while e.ndim > 3:
            e = e.squeeze(0)
        return e.astype(np.float32)

    def _init(self, manifest):
        return {e["input_name"]: _fill(e["shape"], _dtype(e["dtype"]), e["fill"]) for e in manifest}

    @staticmethod
    def _upd(state, res, manifest, off):
        for e in manifest:
            state[e["input_name"]] = res[off + e["index"]]

    def voice_state(self, key=None, main=None):
        key = key or self.voice_npy
        main = main or self.main
        key = str(key) if isinstance(key, (str, Path)) else key
        if (key, id(main)) not in self._voice_states:
            emb = np.load(key).astype(np.float32) if isinstance(key, (str, Path)) else key
            while emb.ndim > 3:
                emb = emb.squeeze(0)
            if emb.ndim < 3:
                emb = emb.reshape(1, -1, emb.shape[-1])
            if self.bos is not None:
                emb = np.concatenate([self.bos, emb], axis=1)
            st = self._init(self.flow_manifest)
            res = main.run(None, {"sequence": np.zeros((1, 0, self.latent_dim), np.float32),
                                  "text_embeddings": emb, **st})
            self._upd(st, res, self.flow_manifest, 2)
            self._voice_states[(key, id(main))] = st
        return {k: v.copy() for k, v in self._voice_states[(key, id(main))].items()}

    # ------------------------------------------------------------------ text
    def prepare(self, text):
        t = " ".join(text.replace("\r", " ").split())
        if not t:
            return "", 1
        if self.meta.get("remove_semicolons"):
            t = t.replace(";", ",")
        after = 3 if len(t.split()) <= 4 else 1
        t = t[0].upper() + t[1:]
        if t[-1].isalnum():
            t += "."
        return t, after

    def split(self, text):
        """Split to chunks of <= max_token_per_chunk tokens at sentence (then comma) boundaries."""
        t, _ = self.prepare(text)
        if not t:
            return []
        import re
        parts = [p for p in re.split(r"(?<=[.!?…])\s+", t) if p]
        out = []
        for p in parts:
            if len(self.tok.Encode(p)) <= self.max_tok:
                out.append(p)
                continue
            cur = ""
            for q in re.split(r"(?<=[,;:])\s+", p):
                cand = (cur + " " + q).strip()
                if cur and len(self.tok.Encode(cand)) > self.max_tok:
                    out.append(cur)
                    cur = q
                else:
                    cur = cand
            if cur:
                out.append(cur)
        # merge tiny neighbours (fewer restarts = smoother prosody)
        merged = []
        for p in out:
            if merged and len(self.tok.Encode(merged[-1] + " " + p)) <= self.max_tok and len(merged[-1]) < 40:
                merged[-1] += " " + p
            else:
                merged.append(p)
        return merged

    # ------------------------------------------------------------------ synthesis
    def stream(self, text, temperature=None, seed=None, first_frames=3, chunk_frames=12, voice=None, int8=None):
        """Yield float32 audio blocks for ONE chunk of text (<= max tokens). Low latency."""
        t, after = self.prepare(text)
        if not t:
            return
        temp = self.temperature if temperature is None else temperature
        rng = np.random.default_rng(seed)
        ids = np.array(self.tok.Encode(t), np.int64).reshape(1, -1)
        main, flow, dec = self._graphs(self.default_int8 if int8 is None else frozenset(int8))
        st = self.voice_state(voice, main)
        te = self.text_cond.run(None, {"token_ids": ids})[0]
        if te.ndim == 2:
            te = te[None]
        res = main.run(None, {"sequence": np.zeros((1, 0, self.latent_dim), np.float32), "text_embeddings": te, **st})
        self._upd(st, res, self.flow_manifest, 2)
        empty = np.zeros((1, 0, self.cond_dim), np.float32)
        curr = np.full((1, 1, self.latent_dim), np.nan, np.float32)
        limit = int(np.ceil((ids.shape[1] / 3.0 + 2.0) * self.frame_rate))
        ms = self._init(self.mimi_manifest)
        # guard against premature EOS (int8 graphs occasionally fire early and clip the line):
        # speech runs ~3 frames per token, so ignore an EOS before ~1.5 frames/token.
        min_eos = max(4, int(1.5 * ids.shape[1]))
        pend, eos, emitted, dt = [], None, 0, 1.0 / self.lsd_steps
        for step in range(limit):
            res = main.run(None, {"sequence": curr, "text_embeddings": empty, **st})
            cond, eos_logit = res[0], res[1]
            self._upd(st, res, self.flow_manifest, 2)
            if eos is None and step >= min_eos and eos_logit[0][0] > -4.0:
                eos = step
            if eos is not None and step >= eos + after:
                break
            x = (rng.normal(0.0, np.sqrt(temp), (1, self.latent_dim)).astype(np.float32)
                 if temp > 0 else np.zeros((1, self.latent_dim), np.float32))
            for s, tt in self._st:
                x = x + flow.run(None, {"c": cond, "s": s, "t": tt, "x": x})[0] * dt
            lat = x.reshape(1, 1, self.latent_dim)
            pend.append(lat)
            curr = lat
            need = first_frames if emitted == 0 else chunk_frames
            if len(pend) >= need:
                r = dec.run(None, {"latent": np.concatenate(pend, axis=1), **ms})
                self._upd(ms, r, self.mimi_manifest, 1)
                emitted += len(pend)
                pend = []
                yield r[0].reshape(-1).astype(np.float32)
        if pend:
            r = dec.run(None, {"latent": np.concatenate(pend, axis=1), **ms})
            yield r[0].reshape(-1).astype(np.float32)

    def synth(self, text, temperature=None, seed=None, voice=None, int8=None):
        blocks = []
        for i, c in enumerate(self.split(text)):
            blocks.extend(self.stream(c, temperature, None if seed is None else seed + i, voice=voice, int8=int8))
        return np.concatenate(blocks) if blocks else np.zeros(0, np.float32)
