#!/usr/bin/env python3
"""user_import.py: turn a user's own video (MP4 / GIF / WebM / MOV, any size) into Lillith pose candidates.

  python3 tools/video-pipeline/user_import.py --input <file> --out <staging_dir> --json-progress [--max-ram-mb 1500]

Pipeline (docs/VIDEO_IMPORT.md has the full contract):
  probe   ffprobe the input (local ffmpeg/ffprobe only, file/pipe protocols only, no network)
  decode  stream 12 fps, 64 px-wide grey signatures from an ffmpeg pipe (never the whole video in memory)
  split   scene cuts + motion holds (pose changes) -> movements of --min-seconds..--max-seconds
  matte   second streaming pass at work resolution (720 px high, ~1.85x the canvas scale): per-frame matte
          (source alpha if the file has one, else the local isnet-anime model if present, else a backdrop key),
          each matted frame spilled to <out>/.user_import_work as a cropped PNG so RAM stays flat
  zoom    zoom_tease.py placement + slow cleavage zoom (same smoothstep ease-in/hold/ease-out, zmax 1.8) onto the
          400x400 canvas, anchor (200, 392), scale 0.54, premultiplied INTER_AREA downscale, nothing below row 392
  clean   matte_clean.clean_cell (tools/matte-clean, v6) per cell, then cellfix.post (holefill -> edgefix ->
          pocketfill against the pre-clean cell -> holefill: holefill is the last image step)
  encode  sheet + json + alpha-packed H.264 mp4 exactly like pack_mp4.py (crf 16, 800x400, no B-frames)
  qa      qa_all.py --white-edge --mp4 and fringeqa.py (FQA_ALL=1) on the candidate dir

stdout carries JSON lines only (with --json-progress); everything else goes to stderr. Writes only inside --out.
Exit 0 ok, 1 error, 130 cancelled (SIGTERM / SIGINT / CTRL_BREAK: stops within ~1 s, deletes this run's staging).
"""
import sys
sys.dont_write_bytecode = True  # never drop __pycache__ next to the tools: write only inside --out

import argparse, json, os, re, shutil, signal, subprocess, threading, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
VERSION = 'user_import 1'
EXTS = {'.mp4', '.m4v', '.gif', '.webm', '.mov'}
CANVAS, BASELINE, ANCHOR_X = 400, 392, 200
FPS = 12
WORK_H = 720           # matte resolution (the Grok Imagine takes are 720p; scale 0.54 is defined against it)
BASE_SCALE = 0.54
ZMAX = 1.8
TARGET = (200.0, 195.0)
FFMPEG_RESERVE_MB = 150  # budget kept free for the ffmpeg decoder child (-threads 1)
ISNET_MIN_MB = 1100      # below this cap the isnet model is not loaded (backdrop key instead)


class Cancelled(Exception):
    pass


class Fail(Exception):
    pass


# ------------------------------------------------------------------ memory
def _proc_status(key):
    try:
        with open('/proc/self/status') as f:
            for l in f:
                if l.startswith(key + ':'):
                    return int(l.split()[1]) / 1024.0
    except OSError:
        return None
    return None


def _win_mem():
    import ctypes
    from ctypes import wintypes

    class PMC(ctypes.Structure):
        _fields_ = [('cb', wintypes.DWORD), ('PageFaultCount', wintypes.DWORD),
                    ('PeakWorkingSetSize', ctypes.c_size_t), ('WorkingSetSize', ctypes.c_size_t),
                    ('QuotaPeakPagedPoolUsage', ctypes.c_size_t), ('QuotaPagedPoolUsage', ctypes.c_size_t),
                    ('QuotaPeakNonPagedPoolUsage', ctypes.c_size_t), ('QuotaNonPagedPoolUsage', ctypes.c_size_t),
                    ('PagefileUsage', ctypes.c_size_t), ('PeakPagefileUsage', ctypes.c_size_t)]
    c = PMC(); c.cb = ctypes.sizeof(PMC)
    k = ctypes.windll.kernel32; p = ctypes.windll.psapi
    p.GetProcessMemoryInfo(k.GetCurrentProcess(), ctypes.byref(c), c.cb)
    return c.WorkingSetSize / 2**20, c.PeakWorkingSetSize / 2**20


def rss_mb():
    if os.name == 'nt':
        try:
            return _win_mem()[0]
        except Exception:
            return 0.0
    v = _proc_status('VmRSS')
    if v is not None:
        return v
    import resource
    r = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return r / 2**20 if sys.platform == 'darwin' else r / 1024.0


def peak_mb():
    if os.name == 'nt':
        try:
            return _win_mem()[1]
        except Exception:
            return 0.0
    v = _proc_status('VmHWM')
    return v if v is not None else rss_mb()


def _linux_tree_rss(root):
    """RSS (MB) of root + every descendant, from /proc (children files when the kernel has them, else a ppid scan)."""
    def kids_fast(pid):
        out = []
        try:
            for t in os.listdir(f'/proc/{pid}/task'):
                with open(f'/proc/{pid}/task/{t}/children') as f:
                    out += [int(x) for x in f.read().split()]
        except OSError:
            return None
        return out
    pids = [root]; i = 0; scan = None
    while i < len(pids):
        k = kids_fast(pids[i])
        if k is None:
            if scan is None:
                scan = {}
                for d in os.listdir('/proc'):
                    if d.isdigit():
                        try:
                            with open(f'/proc/{d}/stat') as f:
                                pp = int(f.read().rsplit(')', 1)[1].split()[1])
                            scan.setdefault(pp, []).append(int(d))
                        except (OSError, ValueError, IndexError):
                            pass
            k = scan.get(pids[i], [])
        pids += [c for c in k if c not in pids]; i += 1
    tot = 0.0
    for pid in pids:
        try:
            with open(f'/proc/{pid}/status') as f:
                for l in f:
                    if l.startswith('VmRSS:'):
                        tot += int(l.split()[1]) / 1024.0; break
        except OSError:
            pass
    return tot


def _win_tree_rss(root):
    """RSS (working set, MB) of root + descendants via a Toolhelp32 snapshot (no psutil needed)."""
    import ctypes
    from ctypes import wintypes

    class PE(ctypes.Structure):
        _fields_ = [('dwSize', wintypes.DWORD), ('cntUsage', wintypes.DWORD), ('th32ProcessID', wintypes.DWORD),
                    ('th32DefaultHeapID', ctypes.c_void_p), ('th32ModuleID', wintypes.DWORD), ('cntThreads', wintypes.DWORD),
                    ('th32ParentProcessID', wintypes.DWORD), ('pcPriClassBase', ctypes.c_long), ('dwFlags', wintypes.DWORD),
                    ('szExeFile', ctypes.c_char * 260)]

    class PMC(ctypes.Structure):
        _fields_ = [('cb', wintypes.DWORD), ('PageFaultCount', wintypes.DWORD),
                    ('PeakWorkingSetSize', ctypes.c_size_t), ('WorkingSetSize', ctypes.c_size_t),
                    ('QuotaPeakPagedPoolUsage', ctypes.c_size_t), ('QuotaPagedPoolUsage', ctypes.c_size_t),
                    ('QuotaPeakNonPagedPoolUsage', ctypes.c_size_t), ('QuotaNonPagedPoolUsage', ctypes.c_size_t),
                    ('PagefileUsage', ctypes.c_size_t), ('PeakPagefileUsage', ctypes.c_size_t)]
    k = ctypes.windll.kernel32; ps = ctypes.windll.psapi
    k.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    snap = k.CreateToolhelp32Snapshot(0x2, 0)
    kids = {}
    try:
        e = PE(); e.dwSize = ctypes.sizeof(PE)
        ok = k.Process32First(snap, ctypes.byref(e))
        while ok:
            kids.setdefault(e.th32ParentProcessID, []).append(e.th32ProcessID)
            ok = k.Process32Next(snap, ctypes.byref(e))
    finally:
        k.CloseHandle(snap)
    pids = [root]; i = 0
    while i < len(pids):
        pids += [c for c in kids.get(pids[i], []) if c not in pids and c != 0]; i += 1
    tot = 0.0
    for pid in pids:
        h = k.OpenProcess(0x1000 | 0x0010, False, pid)  # QUERY_LIMITED_INFORMATION | VM_READ
        if not h:
            continue
        try:
            c = PMC(); c.cb = ctypes.sizeof(PMC)
            if ps.GetProcessMemoryInfo(h, ctypes.byref(c), c.cb):
                tot += c.WorkingSetSize / 2**20
        finally:
            k.CloseHandle(h)
    return tot


def tree_rss_mb(root=None):
    """RSS of this process (or root) plus all of its children (ffmpeg, qa_all, fringeqa, ...), in MB."""
    root = root or os.getpid()
    try:
        import psutil  # optional
        p = psutil.Process(root); tot = 0.0
        for q in [p] + p.children(recursive=True):
            try:
                tot += q.memory_info().rss / 2**20
            except Exception:
                pass
        return tot
    except Exception:
        pass
    try:
        if os.name == 'nt':
            return _win_tree_rss(root)
        if os.path.isdir('/proc'):
            return _linux_tree_rss(root)
    except Exception:
        pass
    return rss_mb() if root == os.getpid() else 0.0


class TreeSampler:
    """Background thread: samples tree_rss_mb() every `period` s and keeps the peak (importer + every child)."""

    def __init__(self, period=0.25, root=None):
        self.period = period; self.root = root or os.getpid()
        self.peak = 0.0; self.last = 0.0
        self._stop = threading.Event()
        self._t = threading.Thread(target=self._loop, name='rss_sampler', daemon=True)

    def _loop(self):
        while not self._stop.is_set():
            self.sample()
            self._stop.wait(self.period)

    def sample(self):
        v = tree_rss_mb(self.root)
        self.last = v; self.peak = max(self.peak, v)
        return v

    def start(self):
        self._t.start(); return self

    def stop(self):
        self._stop.set(); self._t.join(2)
        self.sample()
        return self.peak


# ------------------------------------------------------------------ job state, output
class Job:
    def __init__(self, a, out_stream):
        self.a = a
        self.out = out_stream
        self.cancel = threading.Event()
        self.lock = threading.Lock()
        self.procs = set()
        self.pct = 0
        self.last_emit = 0.0
        self.created = []        # top-level paths inside --out made by this run (deleted on cancel)
        self.out_dir = Path(a.out).resolve()
        self.out_existed = self.out_dir.exists()
        self.work = self.out_dir / '.user_import_work'
        self.ff = self.fp = None
        self.peak_seen = 0.0
        self.t0 = time.monotonic()
        self.sampler = None   # TreeSampler, started by main()
        self.current = None   # candidate dir being built (removed on error)

    # every write goes through here: refuse anything outside --out
    def inside(self, p):
        p = Path(p).resolve()
        if p != self.out_dir and self.out_dir not in p.parents:
            raise Fail(f'refusing to write outside --out: {p}')
        return p

    def mkdir(self, p):
        p = self.inside(p)
        if p == self.out_dir:
            p.mkdir(parents=True, exist_ok=True)
            return p
        top = self.out_dir / p.relative_to(self.out_dir).parts[0]
        if not top.exists():
            with self.lock:
                self.created.append(top)
        p.mkdir(parents=True, exist_ok=True)
        return p

    def check(self):
        if self.cancel.is_set():
            raise Cancelled()

    def emit(self, ev):
        with self.lock:
            if self.cancel.is_set() and ev.get('event') != 'error':
                return
            if self.a.json_progress:
                self.out.write(json.dumps(ev, separators=(',', ':')) + '\n')
            else:
                e = ev.get('event')
                if e == 'progress':
                    self.out.write(f"[{ev['pct']:3d}%] {ev['stage']:<6} {ev['msg']}\n")
                elif e == 'candidate':
                    self.out.write(f"candidate {ev['id']}: {ev['frames']} frames, qa {'PASS' if ev['qa']['pass'] else 'FAIL'} -> {ev['mp4']}\n")
                elif e == 'done':
                    self.out.write(f"done: {ev['candidates']} candidates\n")
                else:
                    self.out.write(f"error: {ev.get('msg')}\n")
            self.out.flush()

    def progress(self, stage, pct, msg, force=False):
        pct = max(self.pct, min(100, int(pct)))
        now = time.monotonic()
        if not force and pct == self.pct and now - self.last_emit < 0.5:
            return
        self.pct = pct; self.last_emit = now
        self.emit({'event': 'progress', 'stage': stage, 'pct': pct, 'msg': msg,
                   'ts': round(time.time(), 3), 'elapsed_s': round(now - self.t0, 3)})

    def mem_guard(self, where):
        r = rss_mb()
        self.peak_seen = max(self.peak_seen, r)
        budget = self.a.max_ram_mb - FFMPEG_RESERVE_MB
        tree = self.sampler.last if self.sampler else 0.0
        return r, r > budget or tree > self.a.max_ram_mb, budget

    # subprocesses: tracked so a cancel can kill them
    def popen(self, cmd, **kw):
        self.check()
        p = subprocess.Popen([str(c) for c in cmd], **kw)
        with self.lock:
            self.procs.add(p)
        return p

    def done_proc(self, p):
        with self.lock:
            self.procs.discard(p)

    def run(self, cmd, env=None, cwd=None, timeout=3600):
        p = self.popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, stdin=subprocess.DEVNULL, env=env, cwd=cwd)
        try:
            out, err = p.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            p.kill(); p.communicate()
            raise Fail(f'{Path(cmd[1]).name if len(cmd) > 1 else cmd[0]} timed out')
        finally:
            self.done_proc(p)
        self.check()
        return p.returncode, out.decode('utf-8', 'replace'), err.decode('utf-8', 'replace')

    def kill_all(self):
        with self.lock:
            ps = list(self.procs)
        for p in ps:
            try:
                p.kill()
            except Exception:
                pass

    def cleanup(self, everything):
        """everything=True (cancel): remove all this run made inside --out; else only the work dir."""
        targets = [self.work] + ([self.current] if self.current else [])
        if everything:
            targets = [self.out_dir] if not self.out_existed else list(self.created) + [self.work]
        for t in targets:
            for _ in range(5):
                try:
                    if t.exists():
                        shutil.rmtree(t)
                    break
                except OSError:
                    time.sleep(0.05)


# ------------------------------------------------------------------ ffmpeg
def find_tools(a):
    def pick(flag, env, name, sibling=None):
        for c in (flag, os.environ.get(env)):
            if c:
                pc = Path(c)
                if pc.is_dir():  # a folder such as the Windows zip's bin\
                    for s in (name + '.exe', name):
                        if (pc / s).is_file():
                            return str((pc / s).resolve())
                w = str(pc.resolve()) if pc.is_file() else shutil.which(c)
                if w:
                    return w
                raise Fail(f'{name} not found at {c} ({"--" + name if c == flag else env})')
        if sibling:
            for s in (name, name + '.exe'):
                cand = Path(sibling).with_name(s)
                if cand.is_file():
                    return str(cand)
        return shutil.which(name)
    ff = pick(a.ffmpeg, 'LILLITH_FFMPEG', 'ffmpeg')
    fp = pick(a.ffprobe, 'LILLITH_FFPROBE', 'ffprobe', sibling=ff)
    missing = [n for n, v in (('ffmpeg', ff), ('ffprobe', fp)) if not v]
    if missing:
        raise Fail(f"{' and '.join(missing)} not found. Install FFmpeg (https://ffmpeg.org/download.html; on Windows add its "
                   f"bin folder to PATH) or pass --ffmpeg/--ffprobe or set LILLITH_FFMPEG/LILLITH_FFPROBE.")
    for b in (ff, fp):
        try:
            r = subprocess.run([b, '-version'], capture_output=True, timeout=20)
            if r.returncode != 0:
                raise OSError(r.returncode)
        except Exception as e:
            raise Fail(f'{b} does not run ({e})')
    return ff, fp


def probe(job, path):
    rc, out, err = job.run([job.fp, '-v', 'error', '-protocol_whitelist', 'file,pipe', '-select_streams', 'v:0',
                            '-show_entries', 'stream=codec_name,width,height,pix_fmt,r_frame_rate,avg_frame_rate,nb_frames,duration'
                            ':stream_tags=alpha_mode,rotate:stream_side_data=rotation:format=duration,format_name',
                            '-of', 'json', str(path)], timeout=120)
    if rc != 0:
        raise Fail(f'ffprobe could not read the input: {err.strip()[-300:] or "unknown error"}')
    js = json.loads(out or '{}')
    st = (js.get('streams') or [None])[0]
    if not st or not st.get('width'):
        raise Fail('the input has no video stream')
    w, h = int(st['width']), int(st['height'])
    rot = 0
    for sd in st.get('side_data_list') or []:
        if 'rotation' in sd:
            rot = int(float(sd['rotation']))
    rot = rot or int(float((st.get('tags') or {}).get('rotate', 0) or 0))
    if abs(rot) % 180 == 90:
        w, h = h, w
    dur = st.get('duration') or (js.get('format') or {}).get('duration')
    try:
        dur = float(dur)
    except (TypeError, ValueError):
        dur = None
    codec = st.get('codec_name', '')
    pix = st.get('pix_fmt', '') or ''
    tags = st.get('tags') or {}
    alpha = pix.startswith(('rgba', 'bgra', 'argb', 'abgr', 'yuva', 'gbrap', 'ya', 'pal8')) or str(tags.get('alpha_mode', '')) == '1'
    dec = None
    if str(tags.get('alpha_mode', '')) == '1' and codec in ('vp8', 'vp9'):
        dec = 'libvpx-vp9' if codec == 'vp9' else 'libvpx'
    return dict(w=w, h=h, dur=dur, codec=codec, pix=pix, alpha=bool(alpha), decoder=dec,
                format=(js.get('format') or {}).get('format_name', ''))


def stream(job, info, path, w, h, pix):
    """Yield frames (numpy HxWxC uint8) at FPS from an ffmpeg pipe, one at a time."""
    import numpy as np
    ch = {'gray': 1, 'rgb24': 3, 'rgba': 4}[pix]
    cmd = [job.ff, '-v', 'error', '-nostdin', '-hide_banner', '-threads', '1', '-filter_threads', '1',
           '-protocol_whitelist', 'file,pipe']
    if info.get('decoder'):
        cmd += ['-c:v', info['decoder']]
    flt = 'area' if w * h <= info['w'] * info['h'] else 'lanczos'
    cmd += ['-i', str(path), '-an', '-sn', '-dn', '-vf', f'fps={FPS},scale={w}:{h}:flags={flt}',
            '-f', 'rawvideo', '-pix_fmt', pix, 'pipe:1']
    p = job.popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, stdin=subprocess.DEVNULL, bufsize=0)
    errbuf = []
    t = threading.Thread(target=lambda: errbuf.append(p.stderr.read()), daemon=True); t.start()
    fsz = w * h * ch
    stopped = False
    try:
        while True:
            job.check()
            buf = bytearray(fsz); mv = memoryview(buf); got = 0
            while got < fsz:
                n = p.stdout.readinto(mv[got:])
                if not n:
                    break
                got += n
            if got < fsz:
                break
            yield np.frombuffer(buf, np.uint8).reshape(h, w, ch) if ch > 1 else np.frombuffer(buf, np.uint8).reshape(h, w)
    finally:
        try:
            p.stdout.close()
        except Exception:
            pass
        if p.poll() is None:
            stopped = True  # we stopped reading early (or are cancelling): not a decode error
            p.kill()
        p.wait(); t.join(2)
        job.done_proc(p)
    job.check()
    if not stopped and p.returncode != 0:
        msg = (errbuf[0] if errbuf else b'').decode('utf-8', 'replace').strip()
        raise Fail(f'ffmpeg could not decode the input (exit {p.returncode}): {msg[-300:] or "no message"}')


# ------------------------------------------------------------------ split
def split(job, info, path, total_est):
    import numpy as np
    sw = 64; sh = max(2, int(round(sw * info['h'] / info['w'] / 2)) * 2)
    diffs, cuts = [], set()
    prev = None
    n = 0
    for i, g in enumerate(stream(job, info, path, sw, sh, 'gray')):
        f = g.astype(np.int16)
        if prev is not None:
            d = float(np.abs(f - prev).mean())
            recent = diffs[-12:]
            med = float(np.median(recent)) if recent else 0.0
            h0 = np.histogram(prev, 32, (0, 256))[0]; h1 = np.histogram(f, 32, (0, 256))[0]
            hd = float(np.abs(h0 - h1).sum()) / (2 * f.size)
            if (d > 28 and d > 3 * (med + 1.0)) or (hd > 0.45 and d > 12):
                cuts.add(i)
            diffs.append(d)
        else:
            diffs.append(0.0)
        prev = f
        n += 1
        if total_est:
            job.progress('decode', 2 + 8 * min(1.0, (i + 1) / total_est), f'scanning frame {i + 1}/{total_est}')
    if n == 0:
        raise Fail('the input decoded to 0 frames')
    job.progress('split', 10, f'{n} frames at {FPS} fps, {len(cuts)} scene cut(s)', force=True)
    mn = max(1, int(round(job.a.min_seconds * FPS))); mx = max(mn + 1, int(round(job.a.max_seconds * FPS)))
    keep_min = max(1, int(round(0.5 * FPS)))
    m = np.array(diffs, float)
    for c in cuts:
        m[c] = 0.0  # a cut is not motion
    ms = np.convolve(m, np.ones(3) / 3, mode='same')
    bounds = [0] + sorted(cuts) + [n]
    segs = []
    for s0, s1 in zip(bounds[:-1], bounds[1:]):
        if s1 - s0 <= 0:
            continue
        sc = ms[s0 + 1:s1] if s1 - s0 > 1 else ms[s0:s1]
        med = float(np.median(sc)) if len(sc) else 0.0
        still_thr = max(0.6, 0.3 * med)
        still = ms[s0:s1] < still_thr
        pieces, start = [], s0
        # split in the middle of every motion hold (>= 0.4 s still) that separates two movements
        hold_min = max(2, int(round(0.4 * FPS)))
        k = s0
        while k < s1:
            if still[k - s0]:
                j = k
                while j < s1 and still[j - s0]:
                    j += 1
                if j - k >= hold_min and k > s0 and j < s1:
                    mid = (k + j) // 2
                    if mid - start >= mn and s1 - mid >= mn:
                        pieces.append((start, mid)); start = mid
                k = j
            else:
                k += 1
        pieces.append((start, s1))
        # enforce the max length: cut at the calmest frame of the allowed window
        out = []
        for a0, a1 in pieces:
            while a1 - a0 > mx:
                lo, hi = a0 + mn, min(a0 + mx, a1 - mn)
                if hi <= lo:
                    cut = a0 + mx
                else:
                    cut = lo + int(np.argmin(ms[lo:hi + 1]))
                out.append((a0, cut)); a0 = cut
            out.append((a0, a1))
        # pieces shorter than the minimum: merge with a neighbour of the same scene
        merged = []
        for p in out:
            if merged and (p[1] - p[0] < mn or merged[-1][1] - merged[-1][0] < mn) and p[1] - merged[-1][0] <= mx:
                merged[-1] = (merged[-1][0], p[1])
            else:
                merged.append(p)
        segs += [p for p in merged if p[1] - p[0] >= keep_min or n < keep_min]
    if not segs:
        segs = [(0, n)]
    return segs, n


# ------------------------------------------------------------------ matte
sys.path.insert(0, str(HERE))
import matte_model  # noqa: E402  (the one place that knows where isnet-anime.onnx lives)
MODEL_NAME = matte_model.MODEL_NAME


def model_candidates(flag=None, env=None, home=None, **kw):
    """isnet-anime lookup order (never downloaded; see matte_model.py):
      1. --model (a file, or a folder holding isnet-anime.onnx)
      2. $U2NET_HOME/isnet-anime.onnx
      3. <install>/models/rembg/isnet-anime.onnx (Arch /opt/lillith/models/rembg, Windows <install>\\models\\rembg)
      4. <home>/.u2net/isnet-anime.onnx          (rembg's own folder)
      5. <home>/.rembg/models/isnet-anime/isnet-anime.onnx   (home = pathlib home: ~ on Linux/macOS,
         %USERPROFILE% on Windows)"""
    return matte_model.model_candidates(flag, env, home, **kw)


def find_model(flag=None, env=None, home=None, **kw):
    """(path or None, [every path tried]); strict=True skips files that are not the whole model"""
    return matte_model.find_model(flag, env, home, **kw)


class Matter:
    def __init__(self, job, info):
        self.job = job; self.mode = job.a.matte; self.threads = job.a.threads
        import matte_v2 as mv
        self.mv = mv
        model, tried = find_model(getattr(job.a, 'model', None), strict=True)   # a half-finished download is not a model
        self.tried = tried
        self.found = model is not None
        if model:
            mv.MODEL = str(model); mv._S = None
        model_ok = model is not None and job.a.max_ram_mb >= ISNET_MIN_MB and _has_ort()
        self.fallback = 'isnet' if model_ok and job.a.matte in ('auto', 'isnet') else 'key'
        if self.mode == 'auto':
            self.mode = 'alpha' if info['alpha'] else self.fallback
        if self.mode == 'isnet':
            if model is None:
                raise Fail('--matte isnet: the isnet-anime model was not found (nothing is downloaded). Tried: '
                           + '; '.join(str(t) for t in tried)
                           + '. ' + matte_model.fix_hint() + ' Or pass --model <file>.')
            if not _has_ort():
                raise Fail("--matte isnet needs the onnxruntime Python package. "
                           + ('Fix: sudo pacman -S --needed python-onnxruntime-cpu (or run the Lillith installer again).' if os.name != 'nt'
                              else 'Fix: run Lillith-Setup.exe again.'))

    def describe(self):
        return {'alpha': "the file's own alpha channel", 'isnet': f'local isnet-anime model ({Path(self.mv.MODEL).name}) + matte_v2 edge alpha',
                'key': 'backdrop key (no model' + (
                    '' if self.found else '; isnet-anime not found, tried ' + '; '.join(str(t) for t in self.tried)) + ')'}[self.mode]

    def downgrade(self):
        if self.mode in ('isnet', 'alpha'):
            if self.mode == 'alpha':
                return False
            self.mv._S = None
            import gc; gc.collect()
            self.mode = 'key'
            return True
        return False

    def __call__(self, frame):
        import numpy as np
        mv = self.mv
        if self.mode == 'alpha' and frame.shape[-1] == 4 and frame[..., 3].min() == 255:
            # alpha-capable format but this frame is fully opaque (most GIFs, opaque ProRes): matte it normally
            if self.job.a.matte == 'alpha':
                raise Fail('--matte alpha: the input has no transparency')
            self.mode = self.fallback
        if self.mode == 'alpha':
            rgba = np.ascontiguousarray(frame).copy()
            rgba[rgba[..., 3] == 0] = 0
            return mv.drop_specks(rgba)
        rgb = frame[..., :3]
        if self.mode == 'isnet':
            M = mv.isnet(rgb, self.threads)
            A, aux = mv.raw_alpha(rgb, M)
            return mv.drop_specks(mv.colour(A, aux))
        return backdrop_key(rgb, mv)


def _has_ort():
    try:
        import onnxruntime  # noqa: F401
        return True
    except Exception:
        return False


def backdrop_key(rgb, mv):
    """No-model matte. White/near-white backdrop (the Grok Imagine take template): key_white flood key as the soft
    matte, refined by matte_v2.raw_alpha/colour exactly like the isnet path. Any other plain backdrop: flood the
    border colour and un-mix against it."""
    import numpy as np
    from scipy import ndimage as nd
    border = np.concatenate([rgb[0], rgb[-1], rgb[:, 0], rgb[:, -1]]).astype(np.float32)
    bg = np.median(border, 0)
    if bg.min() >= 242:
        from key_white import key_white
        M = key_white(rgb)[..., 3].astype(np.float32) / 255.0
        A, aux = mv.raw_alpha(rgb, M)
        return mv.drop_specks(mv.colour(A, aux))
    I = rgb.astype(np.float32)
    d = np.abs(I - bg).max(-1)
    tol = max(18.0, float(np.percentile(np.abs(border - bg).max(-1), 95)) + 8.0)
    cand = d < tol
    lab, n = nd.label(cand)
    edge = np.unique(np.concatenate([lab[0], lab[-1], lab[:, 0], lab[:, -1]]))
    bgm = np.isin(lab, edge[edge > 0])
    alpha = np.where(bgm, 0.0, 1.0).astype(np.float32)
    near = nd.binary_dilation(bgm, iterations=2) & ~bgm
    soft = np.clip((d - tol) / (2 * tol), 0, 1)
    alpha[near] = np.minimum(alpha[near], np.maximum(soft[near], 0.25))
    a = np.clip(alpha, 1e-3, 1)[..., None]
    col = np.clip((I - (1 - a) * bg) / a, 0, 255)
    out = np.dstack([col, alpha[..., None] * 255]).round().astype(np.uint8)
    out[alpha <= 0] = 0
    return mv.drop_specks(out)


# ------------------------------------------------------------------ placement
FIT_MARGIN = 4      # keep the normal-framing bbox inside x FIT_MARGIN..CANVAS-1-FIT_MARGIN (canvas_compliant wants 3..396)


def fit_placement(S0, top, low, xl, xr, cx):
    """One scale + horizontal body point for a whole movement (no per-frame pumping).

    S0 is the shipped scale (0.54 at 720 p) and is the maximum, so standing poses match the shipped clips.
    It shrinks only when the movement's union bbox (rows top..low, columns xl..xr, work px) would not fit the
    400x400 canvas at normal framing: height up to the baseline, width inside the side margins. cx (the body
    point, normally build_clip.body_x) is kept when the figure fits around it, else moved the least needed
    so the bbox fits (a lying pose is wider than it is tall and is not centred on its body point).
    Returns (S, cx, how) with how in {'fixed', 'fit_height', 'fit_width', 'shifted'}."""
    S = S0; how = 'fixed'
    fig_h = low - top + 1; fig_w = xr - xl + 1
    if fig_h * S > BASELINE - 6:
        S = (BASELINE - 6) / fig_h; how = 'fit_height'
    span = CANVAS - 1 - 2 * FIT_MARGIN - 1     # -1: one px of slack for the INTER_AREA footprint
    if fig_w * S > span:
        S = span / fig_w; how = 'fit_width'
    lo_cx = xr - (CANVAS - 1 - FIT_MARGIN - ANCHOR_X) / S   # right edge inside
    hi_cx = xl + (ANCHOR_X - FIT_MARGIN) / S                # left edge inside
    if lo_cx > hi_cx:
        lo_cx = hi_cx = (xl + xr) / 2.0
    ncx = min(max(cx, lo_cx), hi_cx)
    if abs(ncx - cx) > 0.5 and how == 'fixed':
        how = 'shifted'
    return S, float(ncx), how


def _runs(row):
    import numpy as np
    xs = np.flatnonzero(row)
    if not len(xs):
        return []
    br = np.split(xs, np.flatnonzero(np.diff(xs) > 1) + 1)
    return [(int(r[0]), int(r[-1])) for r in br]


def chest_point(alpha, body_x=None):
    """Estimated chest (x, y) in the alpha's px, and which rule found it.

    The mask (alpha > 128) is opened by ~3 % of the figure height and reduced to its largest blob, which drops
    the thin tail, loose hair strands and fingers. Then:
      upright (bbox w <= 1.25 h: standing, sitting, kneeling, crawling): the widest band in 18-45 % of the
          height, measured on the opaque run at (or nearest) the body point, smoothed over ~4 % of the height;
      lying (w > 1.25 h): the head end is the end whose outer quarter reaches highest; the chest is 30 % of
          the width in from it, vertically centred on the body there.
    None when the alpha is empty."""
    import numpy as np, cv2
    from scipy import ndimage as nd
    m = alpha > 128
    ys, xs = np.nonzero(m)
    if not len(xs):
        return None, 'empty'
    h = ys.max() - ys.min() + 1
    k = max(3, int(round(0.03 * h))) | 1
    o = cv2.morphologyEx(m.astype(np.uint8), cv2.MORPH_OPEN, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (k, k))) > 0
    lab, n = nd.label(o)
    if n:
        o = lab == (int(np.argmax(nd.sum(o, lab, range(1, n + 1)))) + 1)
    else:
        o = m
    ys, xs = np.nonzero(o)
    y0, y1, x0, x1 = ys.min(), ys.max(), xs.min(), xs.max()
    H = y1 - y0 + 1; W = x1 - x0 + 1
    if W <= 1.25 * H:
        cx = body_x if body_x is not None else (x0 + x1) / 2.0
        r0, r1 = int(y0 + 0.18 * H), max(int(y0 + 0.45 * H), int(y0 + 0.18 * H) + 1)
        wid = np.zeros(r1 - r0); cen = np.full(r1 - r0, float(cx))
        for i, y in enumerate(range(r0, r1)):
            rs = _runs(o[y])
            if not rs:
                continue
            r = min(rs, key=lambda r: 0 if r[0] <= cx <= r[1] else min(abs(r[0] - cx), abs(r[1] - cx)))
            wid[i] = r[1] - r[0] + 1; cen[i] = (r[0] + r[1]) / 2.0
        if not wid.any():
            return (float(cx), float(y0 + 0.30 * H)), 'upright_fallback'
        kk = max(1, int(0.04 * H))
        ws = np.convolve(wid, np.ones(kk) / kk, 'same')
        i = int(np.argmax(ws))
        return (float(cen[i]), float(r0 + i)), 'upright_widest_band'
    q = max(1, W // 4)
    tl = int(np.flatnonzero(o[:, x0:x0 + q].any(1)).min()); tr = int(np.flatnonzero(o[:, x1 - q + 1:x1 + 1].any(1)).min())
    head_left = tl <= tr
    x = int(x0 + 0.30 * W) if head_left else int(x1 - 0.30 * W)
    col = np.flatnonzero(o[:, x])
    if not len(col):
        return (float(x), float((y0 + y1) / 2)), 'lying_fallback'
    rs = np.split(col, np.flatnonzero(np.diff(col) > 1) + 1); r = max(rs, key=len)
    return (float(x), float((r[0] + r[-1]) / 2.0)), 'lying_head_' + ('left' if head_left else 'right')


# ------------------------------------------------------------------ segment render
def zoom_curve(N, zmax):
    """zoom_tease.zoom_of with automatic slow keyframes (15/40/60/85 % of the movement)."""
    if zmax <= 1.0 or N < 12:
        return [1.0] * N, [0.0] * N, None
    ks = [int(round(f * (N - 1))) for f in (0.15, 0.40, 0.60, 0.85)]
    for j in range(1, 4):
        ks[j] = max(ks[j], ks[j - 1] + 1)
    K0, K1, K2, K3 = ks

    def ss(u):
        u = min(max(u, 0.0), 1.0); return u * u * (3 - 2 * u)
    zs, us = [], []
    for k in range(N):
        if k <= K0 or k >= K3:
            u = 0.0
        elif k < K1:
            u = ss((k - K0) / (K1 - K0))
        elif k <= K2:
            u = 1.0
        else:
            u = 1 - ss((k - K2) / (K3 - K2))
        zs.append(1 + (zmax - 1) * u); us.append(u)
    return zs, us, ks


def render_segment(job, seg, name, cdir, meta, work_wh, pct0, pct1):
    """meta: per frame (path or None, x0, y0, top, low) in work-res px. Returns cells + json fields."""
    import numpy as np, cv2
    sys.path.insert(0, str(HERE.parent / 'matte-clean'))
    from build_clip import body_x
    W, H = work_wh
    live = [m for m in meta if m[0]]
    if not live:
        return None, 'no subject found in this movement'
    tops = [m[3] for m in live]; lows = [m[4] for m in live]
    S0 = BASE_SCALE * WORK_H / H
    zmax = job.a.zmax
    # union rect of every crop: one sampling grid for the whole movement (no sub-pixel shimmer)
    ux0 = min(m[1] for m in live); uy0 = min(m[2] for m in live)
    ux1 = max(m[1] + m[5] for m in live); uy1 = max(m[2] + m[6] for m in live)
    def full_alpha(m):
        f = cv2.imread(m[0], cv2.IMREAD_UNCHANGED)
        a_ = np.zeros((H, W), np.uint8); a_[m[2]:m[2] + f.shape[0], m[1]:m[1] + f.shape[1]] = f[..., 3]
        return a_
    first = live[0]
    a0 = full_alpha(first)
    body_cx = body_x(a0)
    xl = min(m[7] for m in live); xr = max(m[8] for m in live)
    S, cx_src, fit = fit_placement(S0, min(tops), max(lows), xl, xr, body_cx)
    tx = int(round(ANCHOR_X - cx_src * S))
    ty = int(np.floor(BASELINE - (max(lows) + 1) * S + 1))
    # zoom focus: median chest over up to 5 frames spread through the movement (one focus: no jitter)
    picks = sorted({live[int(round(j * (len(live) - 1) / 4))][0] for j in range(5)}) if len(live) > 1 else [first[0]]
    byp = {m[0]: m for m in live}
    pts, rules = [], []
    for pth in picks:
        m_ = byp[pth]
        a_ = a0 if m_ is first else full_alpha(m_)
        pt, rule = chest_point(a_, body_x(a_))
        if pt is not None:
            pts.append(pt); rules.append(rule)
    if pts:
        chest_src = np.median(np.array(pts), 0)
        chest_rule = max(set(rules), key=rules.count)
    else:
        chest_src = np.array([body_cx, first[3] + 0.30 * (first[4] - first[3])]); chest_rule = 'fixed_30pct'
    C = S * chest_src + np.array([tx, ty], float)
    T = np.array(TARGET)
    N = len(meta)
    zs, us, ks = zoom_curve(N, zmax)
    uw, uh = ux1 - ux0, uy1 - uy0
    cells = []
    import matte_clean as mc
    cstats = {'matte_clean_px': 0, 'holefill_px': 0, 'edgefix_px': 0, 'pocketfill_px': 0, 'pocket_skipped_px': 0}
    import cellfix
    for k, m in enumerate(meta):
        job.check()
        z, u = zs[k], us[k]
        if not m[0]:
            cells.append(np.zeros((CANVAS, CANVAS, 4), np.uint8)); continue
        im = cv2.imread(m[0], cv2.IMREAD_UNCHANGED)
        im = cv2.cvtColor(im, cv2.COLOR_BGRA2RGBA)
        U = np.zeros((uh, uw, 4), np.float32)
        pm = im.astype(np.float32) / 255.0; pm[..., :3] *= pm[..., 3:]
        U[m[2] - uy0:m[2] - uy0 + im.shape[0], m[1] - ux0:m[1] - ux0 + im.shape[1]] = pm
        s = S * z
        nw, nh = max(1, int(round(uw * s))), max(1, int(round(uh * s)))
        r = cv2.resize(U, (nw, nh), interpolation=cv2.INTER_AREA)
        t = np.array([tx, ty], float)
        o = z * t + C * (1 - z) + u * (T - C) + s * np.array([ux0, uy0], float)
        if z == 1.0:
            o = np.round(o)
        M = np.float32([[1, 0, o[0]], [0, 1, o[1]]])
        q = cv2.warpAffine(r, M, (CANVAS, CANVAS), flags=cv2.INTER_NEAREST if z == 1.0 else cv2.INTER_LINEAR,
                           borderMode=cv2.BORDER_CONSTANT, borderValue=0)
        al = q[..., 3:]
        col = np.where(al > 1e-4, q[..., :3] / np.maximum(al, 1e-4), 0)
        u8 = np.clip(np.round(np.dstack([col, al]) * 255), 0, 255).astype(np.uint8)
        u8[u8[..., 3] == 0] = 0; u8[BASELINE + 1:] = 0
        job.progress('zoom', pct0 + (pct1 - pct0) * 0.35 * (k + 1) / N, f'{name}: zoom frame {k + 1}/{N}')
        c2, st = mc.clean_cell(u8, name)
        cstats['matte_clean_px'] += int(sum(v for v in st.values() if isinstance(v, (int, np.integer))))
        c3, st2 = cellfix.post(c2, raw=u8)
        for kk in ('holefill_px', 'edgefix_px', 'pocketfill_px', 'pocket_skipped_px'):
            cstats[kk] += st2[kk]
        c3[BASELINE + 1:] = 0
        cells.append(c3)
        job.progress('clean', pct0 + (pct1 - pct0) * (0.35 + 0.4 * (k + 1) / N), f'{name}: matte-clean + holefill {k + 1}/{N}')
    fields = dict(scale=round(S, 5), tx=tx, ty=ty, zmax=zmax, keyframes=ks, zs=[round(v, 4) for v in zs],
                  focus=C.tolist(), stats=cstats, fit=fit, base_scale=round(S0, 5), chest_rule=chest_rule,
                  chest_src=[round(float(v), 1) for v in chest_src])
    return cells, fields


FF_ENCODE = ['-vf', 'scale=out_color_matrix=bt709:out_range=pc:flags=accurate_rnd+full_chroma_int',
             '-c:v', 'libx264', '-preset', 'veryslow', '-tune', 'animation', '-crf', '16',
             '-profile:v', 'high', '-level', '4.0', '-pix_fmt', 'yuv420p',
             '-color_range', 'pc', '-colorspace', 'bt709', '-color_primaries', 'bt709', '-color_trc', 'bt709']


def encode(job, cells, out):
    """pack_mp4.encode, with a tracked (cancellable) ffmpeg."""
    import pack_mp4
    tmp = out.with_name(out.name + '.tmp.mp4')
    cmd = [job.ff, '-v', 'error', '-nostdin', '-y', '-protocol_whitelist', 'file,pipe', '-f', 'rawvideo', '-pix_fmt', 'rgb24',
           '-s', f'{2 * CANVAS}x{CANVAS}', '-r', f'{FPS:g}', '-i', 'pipe:0'] + FF_ENCODE + \
          ['-g', str(max(len(cells), 1)), '-bf', '0', '-movflags', '+faststart',
           '-map_metadata', '-1', '-fflags', '+bitexact', '-flags:v', '+bitexact', str(tmp)]
    p = job.popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        for c in cells:
            job.check()
            p.stdin.write(pack_mp4.pack_frame(c, 0).tobytes())
        p.stdin.close()
        err = p.stderr.read()
        rc = p.wait()
    except (BrokenPipeError, OSError):
        job.check()
        rc = p.wait(); err = p.stderr.read() if p.stderr else b''
    finally:
        job.done_proc(p)
    job.check()
    if rc != 0:
        raise Fail(f'ffmpeg (libx264) encode failed: {err.decode("utf-8", "replace").strip()[-300:]}')
    os.replace(tmp, out)
    dec = pack_mp4.decode(str(out), CANVAS, CANVAS)
    if len(dec) != len(cells):
        raise Fail(f'{out.name}: decoded {len(dec)} frames, expected {len(cells)}')
    return pack_mp4.metrics(cells, dec)


def run_qa(job, cdir, name):
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE='1', LILLITH_FFMPEG=job.ff, FQA_ALL='1',
               FQA_OUT=str(job.inside(cdir / 'qa')), CC_REPO=str(REPO))
    rc, out, err = job.run([sys.executable, HERE / 'qa_all.py', '--white-edge', '--mp4', cdir], env=env, cwd=str(cdir))
    line = next((l for l in out.splitlines() if l.startswith(name)), out.strip().splitlines()[-1] if out.strip() else err.strip()[-200:])
    we_ok = rc == 0
    white = ' '.join(line.split()[1:]) if line.startswith(name) else (line or f'qa_all exit {rc}')
    rc2, out2, err2 = job.run([sys.executable, HERE / 'fringeqa.py', 'user', name, cdir], env=env, cwd=str(cdir))
    fr = None
    for l in out2.splitlines():
        if l.startswith('{'):
            try:
                fr = json.loads(l)
            except ValueError:
                pass
    if fr is None:
        fringe, f_ok = f'fringeqa error: {(err2 or out2).strip()[-200:]}', False
    else:
        fringe = (f"{fr.get('verdict')} max_white {fr.get('max_white_any_frame')} rim {fr.get('max_light_rim_any_frame')} "
                  f"pocket {fr.get('max_pocket_any_frame')} worst_frame {fr.get('worst_frame')} frames {fr.get('frames')}")
        f_ok = fr.get('verdict') in ('PASS', 'WARN')
    qd = cdir / 'qa'
    if qd.exists() and not any(qd.rglob('*.png')):
        shutil.rmtree(qd, ignore_errors=True)
    return {'pass': bool(we_ok and f_ok), 'white_edge': white, 'fringe': fringe}


def slug(s):
    s = re.sub(r'[^a-z0-9]+', '_', s.lower()).strip('_')
    return (s or 'clip')[:24].strip('_') or 'clip'


def finish_segment(job, k, seg, meta, work_wh, base, info, matter, pct0, pct1):
    import numpy as np
    from PIL import Image
    j = k
    while True:
        name = f'user_{base}_{j:02d}'
        cdir = job.out_dir / name
        if not cdir.exists():
            break
        j += 1
    job.mkdir(cdir)
    job.current = cdir
    cells, fields = render_segment(job, seg, name, cdir, meta, work_wh, pct0, pct1)
    if cells is None:
        shutil.rmtree(cdir, ignore_errors=True)
        job.progress('clean', pct1, f'movement {k}: {fields}, skipped', force=True)
        return None
    N = len(cells)
    job.progress('encode', pct0 + (pct1 - pct0) * 0.78, f'{name}: writing sheet + mp4', force=True)
    sheet = np.zeros((CANVAS, CANVAS * N, 4), np.uint8)
    for i, c in enumerate(cells):
        sheet[:, i * CANVAS:(i + 1) * CANVAS] = c
    Image.fromarray(sheet, 'RGBA').save(job.inside(cdir / f'{name}_sheet.png'), optimize=False, compress_level=6)
    del sheet
    bbs = []
    for c in cells:
        ys, xs = np.nonzero(c[..., 3] > 16)
        bbs.append([int(xs.min()), int(xs.max()), int(ys.min()), int(ys.max())] if len(xs) else [0, 0, 0, 0])
    js = {'name': name, 'placeholder': False, 'frame_count': N, 'frame_width': CANVAS, 'frame_height': CANVAS,
          'canvas_w': CANVAS, 'canvas_h': CANVAS, 'layout': 'horizontal_strip', 'sheet': f'{name}_sheet.png',
          'frames_dir': 'frames/', 'fps': FPS, 'loop': False, 'baseline_y': BASELINE, 'anchor': [ANCHOR_X, BASELINE],
          'source': 'user-video', 'scale': fields['scale'], 'body_anchor_x': ANCHOR_X, 'facing': 'front',
          'source_files': [Path(job.a.input).name],
          'source_frame_range': {'first': seg[0], 'last': seg[1] - 1, 'step': 1, 'video_fps': FPS,
                                 'note': f'0-based frame numbers of the input resampled to {FPS} fps'},
          'generator': f'tools/video-pipeline/user_import.py ({VERSION})',
          'matte_version': f'{matter.mode}: {matter.describe()}', 'matte_tool': 'tools/video-pipeline/user_import.py (matte_v2 functions)',
          'centering': fields['fit'], 'scale_max': fields['base_scale'],
          'baseline_snap': 'fixed (lowest frame of the movement on the baseline)',
          'cleavage_zoom': {'zmax': fields['zmax'], 'focus_canvas_xy': fields['focus'], 'target_canvas_xy': list(TARGET),
                            'keyframes_ease_in_hold_out': [fields['keyframes']] if fields['keyframes'] else [],
                            'cycles': 1 if fields['keyframes'] else 0, 'easing': 'smoothstep', 'zoom_per_frame': fields['zs'],
                            'focus_rule': fields['chest_rule'], 'focus_src_xy': fields['chest_src'],
                            'note': 'zoom_tease.py math with automatic keyframes; focus = detected chest (user_import.chest_point)'},
          'baked_secondary_motion': ['breathing', 'sway', 'pigtails', 'tail', 'chest_follow'],
          'engine_note': 'v2 bakes secondary motion into the pixels. Mute or strongly reduce engine chest/pigtail springs and breathing for this clip, or the motion doubles.',
          'tags': ['baked_motion', 'cleavage_zoom', 'user_import'] if fields['keyframes'] else ['baked_motion', 'user_import'],
          'bbox_per_frame': bbs,
          'canvas_compliant': all(b[0] >= 3 and b[1] <= 396 and b[3] <= 392 for b in bbs),
          'user_import': {'version': VERSION, 'input': str(Path(job.a.input).resolve()), 'movement': k,
                          'post': ['matte_clean.clean_cell v6', 'cellfix.post (holefill, edgefix, pocketfill vs the raw matte, holefill)'], **fields['stats']}}
    mp4 = cdir / f'{name}.mp4'
    q = encode(job, cells, mp4)
    js['video'] = {'file': mp4.name, 'layout': 'alpha_side_by_side', 'codec': 'h264 high yuv420p full-range bt709',
                   'crf': 16, 'edge_bleed_px': 0, 'width': 2 * CANVAS, 'height': CANVAS, 'frame_count': N, 'fps': float(FPS),
                   'bytes': mp4.stat().st_size, 'quality': q, 'tool': 'tools/video-pipeline/user_import.py (pack_mp4 encode)'}
    with open(job.inside(cdir / f'{name}.json'), 'w') as f:
        json.dump(js, f, indent=2)
    pk = fields['keyframes']
    pi = (pk[1] + pk[2]) // 2 if pk else N // 2
    g = Image.new('RGBA', (CANVAS, CANVAS), (128, 128, 128, 255)); g.alpha_composite(Image.fromarray(cells[pi], 'RGBA'))
    prev = cdir / f'{name}_preview.png'
    g.convert('RGB').save(job.inside(prev))
    del cells
    job.progress('qa', pct0 + (pct1 - pct0) * 0.85, f'{name}: qa_all --white-edge --mp4 + fringeqa', force=True)
    qa = run_qa(job, cdir, name)
    job.emit({'event': 'candidate', 'id': name, 'mp4': str(mp4.resolve()), 'preview_png': str(prev.resolve()),
              'frames': N, 'fps': FPS, 'qa': qa, 'suggested_name': name})
    job.current = None
    job.progress('qa', pct1, f'{name}: qa {"PASS" if qa["pass"] else "FAIL"}', force=True)
    return name


def pipeline(job):
    import numpy as np
    a = job.a
    src = Path(a.input)
    if not src.is_file():
        raise Fail(f'input not found: {src}')
    if src.suffix.lower() not in EXTS:
        raise Fail(f'unsupported file type {src.suffix or "(none)"}: use MP4, GIF, WebM or MOV')
    r, over, budget = job.mem_guard('start')
    if over:
        raise Fail(f'--max-ram-mb {a.max_ram_mb} is too low: the importer alone needs ~{int(r) + FFMPEG_RESERVE_MB} MB')
    job.progress('probe', 0, f'probing {src.name}', force=True)
    job.ff, job.fp = find_tools(a)
    os.environ['LILLITH_FFMPEG'] = job.ff
    import pack_mp4
    pack_mp4.FFMPEG = job.ff
    info = probe(job, src)
    total = int(info['dur'] * FPS) + 1 if info['dur'] else 0
    job.progress('probe', 2, f"{info['w']}x{info['h']} {info['codec']} {info['pix']}" +
                 (f", {info['dur']:.1f} s" if info['dur'] else '') + (', has alpha' if info['alpha'] else ''), force=True)
    job.mkdir(job.out_dir)
    segs, n = split(job, info, src, total)
    if a.max_candidates and len(segs) > a.max_candidates:
        segs = segs[:a.max_candidates]
    job.progress('split', 10, f'{len(segs)} movement(s): ' + ', '.join(f'{s0}-{s1 - 1}' for s0, s1 in segs), force=True)
    matter = Matter(job, info)
    job.progress('matte', 10, f'matte: {matter.describe()}', force=True)
    H = a.work_height or (WORK_H if a.max_ram_mb >= 700 else 540)
    W = max(2, int(round(H * info['w'] / info['h'] / 2)) * 2)
    pix = 'rgba' if matter.mode == 'alpha' else 'rgb24'
    job.mkdir(job.work)
    base = slug(src.stem)
    frames_total = sum(s1 - s0 for s0, s1 in segs)
    which = {}
    for si, (s0, s1) in enumerate(segs):
        for i in range(s0, s1):
            which[i] = si
    metas = [[] for _ in segs]
    names = []; downgraded = False; finished = set()
    span = 88.0 / max(1, frames_total)
    lo = []; acc = 10.0
    for s0, s1 in segs:
        lo.append(acc); acc += span * (s1 - s0)
    lo.append(acc)

    def finish(si):
        mid = (lo[si] + lo[si + 1]) / 2
        nm = finish_segment(job, si + 1, segs[si], metas[si], (W, H), base, info, matter, mid, lo[si + 1])
        if nm:
            names.append(nm)
        shutil.rmtree(job.work / f'm{si:02d}', ignore_errors=True)
        metas[si] = []; finished.add(si)
    for i, fr in enumerate(stream(job, info, src, W, H, pix)):
        if i not in which:
            if i > segs[-1][1]:
                break
            continue
        si = which[i]
        rgba = matter(fr)
        r, over, budget = job.mem_guard('matte')
        if over:
            if not downgraded and matter.downgrade():
                downgraded = True
                job.progress('matte', job.pct, f'RAM {int(r)} MB over the {budget} MB budget: switching to the backdrop key', force=True)
            else:
                import gc; gc.collect()
                r = rss_mb()
                if r > budget:
                    raise Fail(f'memory cap exceeded: {int(r)} MB used, --max-ram-mb {a.max_ram_mb} '
                               f'(leaves {budget} MB after the ffmpeg reserve)')
        ys, xs = np.nonzero(rgba[..., 3])
        sd = job.work / f'm{si:02d}'
        if len(xs):
            x0, x1, y0, y1 = int(xs.min()), int(xs.max()) + 1, int(ys.min()), int(ys.max()) + 1
            solid = np.nonzero((rgba[..., 3] > 128).any(1))[0]
            top, low = (int(solid.min()), int(solid.max())) if len(solid) else (y0, y1 - 1)
            cols = np.nonzero((rgba[..., 3] > 16).any(0))[0]
            xl_, xr_ = (int(cols.min()), int(cols.max())) if len(cols) else (x0, x1 - 1)
            import cv2
            if not sd.exists():
                job.mkdir(sd)
            p = job.inside(sd / f'f_{i:06d}.png')
            cv2.imwrite(str(p), cv2.cvtColor(np.ascontiguousarray(rgba[y0:y1, x0:x1]), cv2.COLOR_RGBA2BGRA),
                        [cv2.IMWRITE_PNG_COMPRESSION, 1])
            metas[si].append((str(p), x0, y0, top, low, x1 - x0, y1 - y0, xl_, xr_))
        else:
            metas[si].append((None, 0, 0, 0, 0, 0, 0, 0, 0))
        del rgba, fr
        segn = segs[si][1] - segs[si][0]
        job.progress('matte', lo[si] + (lo[si + 1] - lo[si]) / 2 * len(metas[si]) / segn,
                     f'movement {si + 1}/{len(segs)}: matte frame {len(metas[si])}/{segn}')
        if i == segs[si][1] - 1:
            finish(si)
    job.check()
    for si in range(len(segs)):   # the matte pass decoded fewer frames than the scan: finish what we have
        if si not in finished and metas[si]:
            finish(si)
    job.cleanup(False)
    return names


def preflight(argv):
    """`user_import.py --preflight [--matte M] [--model F]`: one JSON line on stdout, exit 0. The Pose Builder runs
    this before it starts a job and shows the problems (what is missing and how to fix it) instead of crashing."""
    ap = argparse.ArgumentParser()
    ap.add_argument('--preflight', action='store_true')
    ap.add_argument('--matte', choices=('auto', 'isnet', 'key', 'alpha'), default='auto')
    ap.add_argument('--model')
    a = ap.parse_args(argv)
    uses_model = a.matte in ('auto', 'isnet')
    mods = matte_model.REQUIRED_MODULES if uses_model else tuple(m for m in matte_model.REQUIRED_MODULES if m != 'onnxruntime')
    r = matte_model.check(need_model=uses_model, flag=a.model, modules=mods)
    r['matte'] = a.matte
    r['fatal'] = a.matte == 'isnet'      # auto degrades to the backdrop key; isnet cannot
    print(json.dumps(r), flush=True)
    return 0


def main(argv=None):
    if '--preflight' in (sys.argv[1:] if argv is None else argv):
        return preflight(sys.argv[1:] if argv is None else argv)
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--input', required=True, help='MP4, GIF, WebM or MOV')
    ap.add_argument('--out', required=True, help='staging dir; created if missing; nothing is written elsewhere')
    ap.add_argument('--json-progress', action='store_true', help='JSON lines on stdout (the GUI contract)')
    ap.add_argument('--max-ram-mb', type=int, default=1500, help='RAM cap for the importer + its ffmpeg (default 1500)')
    ap.add_argument('--matte', choices=('auto', 'isnet', 'key', 'alpha'), default='auto')
    ap.add_argument('--model', help='isnet-anime.onnx file or folder. Lookup: --model, $U2NET_HOME, the installer copy '
                         '(<install>/models/rembg), ~/.u2net, ~/.rembg/models/isnet-anime (%%USERPROFILE%% on Windows); '
                         'never downloaded (see matte_model.py)')
    ap.add_argument('--zmax', type=float, default=ZMAX, help='peak cleavage zoom (1 = no zoom; default 1.8)')
    ap.add_argument('--min-seconds', type=float, default=1.5)
    ap.add_argument('--max-seconds', type=float, default=8.0)
    ap.add_argument('--max-candidates', type=int, default=0, help='0 = no limit')
    ap.add_argument('--threads', type=int, default=2, help='onnxruntime threads (default 2)')
    ap.add_argument('--ffmpeg', help='ffmpeg binary or its folder (default: LILLITH_FFMPEG, then PATH), e.g. C:\\Lillith\\bin\\ffmpeg.exe')
    ap.add_argument('--ffprobe', help='ffprobe binary or its folder (default: LILLITH_FFPROBE, next to --ffmpeg, then PATH)')
    ap.add_argument('--work-height', type=int, default=0, help=argparse.SUPPRESS)  # tests: tiny matte resolution
    a = ap.parse_args(argv)

    # stdout is the JSON channel: keep a private handle and point fd 1 / sys.stdout at stderr so no library
    # print or inherited child output can corrupt it
    out_stream = os.fdopen(os.dup(1), 'w', encoding='utf-8', buffering=1)
    try:
        os.dup2(2, 1)
    except OSError:
        pass
    sys.stdout = sys.stderr
    sys.path.insert(0, str(HERE)); sys.path.insert(0, str(HERE / 'clipclean')); sys.path.insert(0, str(HERE.parent / 'matte-clean'))
    os.environ.setdefault('CC_REPO', str(REPO))
    os.environ['PYTHONDONTWRITEBYTECODE'] = '1'
    job = Job(a, out_stream)
    job.sampler = TreeSampler().start()
    result = {}

    def work():
        try:
            names = pipeline(job)
            result['names'] = names
        except Cancelled:
            result['cancelled'] = True
        except Fail as e:
            result['error'] = str(e)
        except Exception as e:  # noqa: BLE001
            import traceback; traceback.print_exc()
            result['error'] = f'{type(e).__name__}: {e}'

    def on_signal(signum, frame):
        job.cancel.set()
        job.kill_all()

    for s in ('SIGTERM', 'SIGINT', 'SIGBREAK'):
        if hasattr(signal, s):
            try:
                signal.signal(getattr(signal, s), on_signal)
            except (ValueError, OSError):
                pass
    t = threading.Thread(target=work, name='user_import', daemon=True)
    t.start()
    while t.is_alive():
        t.join(0.1)
        if job.cancel.is_set():
            job.kill_all()
            t.join(0.6)
            job.cleanup(True)
            try:
                out_stream.flush()
            except Exception:
                pass
            os._exit(130)
    if job.cancel.is_set() or result.get('cancelled'):
        job.cleanup(True)
        os._exit(130)
    if 'error' in result:
        job.cleanup(False)
        job.emit({'event': 'error', 'msg': result['error']})
        out_stream.flush()
        return 1
    tree_peak = job.sampler.stop()
    job.emit({'event': 'done', 'candidates': len(result.get('names', [])),
              'peak_rss_mb': int(round(max(tree_peak, peak_mb(), job.peak_seen)))})
    out_stream.flush()
    return 0


if __name__ == '__main__':
    sys.exit(main())
