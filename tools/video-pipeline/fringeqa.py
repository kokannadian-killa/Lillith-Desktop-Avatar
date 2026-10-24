#!/usr/bin/env python3
"""Read-only white-fringe QA of re-matted clips, reproducing video.rs unpack_frame (snap <=3 ->0, >=251 ->255, premultiply).

  python3 tools/video-pipeline/fringeqa.py <owner> <clip> <clip_dir>     -> one JSON line (verdict PASS/WARN/FAIL)

Ported from the build box's scratch-fringeqa/fringeqa.py (the "fringeqa" gate of the seasonal recipe). Env:
  FQA_ALL=1        check every frame (default: 8 even + 4 highest-motion frames)
  FQA_OUT=DIR      where crops of non-PASS frames go (default ./fringeqa_out); nothing else is written
  LILLITH_FFMPEG   ffmpeg binary (default `ffmpeg` on PATH); only local file/pipe protocols are allowed
"""
import sys, os, json, hashlib, subprocess, heapq
import numpy as np
from scipy import ndimage as nd
from PIL import Image

SCALE_VF = "scale=in_color_matrix=bt709:in_range=pc:flags=accurate_rnd+full_chroma_int"
BGS = {"dark": (16, 16, 16), "green": (0, 255, 102), "magenta": (255, 0, 170)}
RING = 3          # px inside the alpha edge
BG_LIFT = 48      # composite whiteness (min channel) must exceed background by this
IN_LIFT = 32      # ...and exceed her nearest solid interior colour by this
NEUTRAL_MIN, NEUTRAL_SAT = 170, 40   # straight colour is light & near-neutral (backdrop white), not warm skin
RIM_FAIL, RIM_WARN = 60, 25  # light (pale, not-white) rim px in any sampled frame
FAIL_PX = 8       # worst sampled frame > 8 white edge px on her body -> FAIL (4..8 = WARN)
POCKET_FAIL, POCKET_WARN = 24, 8
N_EVEN, N_MOTION = 8, 4
EXCL = None  # optional fn(clip, frame) -> bool mask of intentional art (icons/panels/FX) to exclude
OUT = os.environ.get("FQA_OUT") or os.path.join(os.getcwd(), "fringeqa_out")
FFMPEG = os.environ.get("LILLITH_FFMPEG", "ffmpeg")

def unpack(packed, w):
    col = packed[:, :w].astype(np.int32); araw = packed[:, w:, 1].astype(np.int32)
    a = np.where(araw <= 3, 0, np.where(araw >= 251, 255, araw))
    P = (col * a[..., None] + 127) // 255
    return col, araw, a, P

def analyse(col, araw, a, P, ex=None):
    clear = a == 0
    if clear.all():
        z = np.zeros(a.shape, bool); return 0, 0, 0, z, z, {}
    d = nd.distance_transform_edt(~clear)
    ring = (~clear) & (d <= RING)
    amax = 255
    if (a > 16).any():   # uniformly faded (intentionally translucent) frames: "solid interior" = her top alpha band
        amax = max(int(np.percentile(a[a > 16], 90)), 32)
    ref = (a >= amax - 8) & (d >= RING + 1)
    if not ref.any():
        ref = a >= amax - 8
    if not ref.any():
        ref = ~clear
    _, (ry, rx) = nd.distance_transform_edt(~ref, return_indices=True)
    inner = col[ry, rx]
    Winner = inner.min(-1)
    mn = col.min(-1); sat = col.max(-1) - mn
    neutral = (mn >= NEUTRAL_MIN) & (sat <= NEUTRAL_SAT)
    strict = (mn >= 215) & (sat <= 25)
    halo = np.zeros(a.shape, bool); white = np.zeros(a.shape, bool); per = {}
    for k, bg in BGS.items():
        bgv = np.array(bg)
        comp = P + (bgv * (255 - a[..., None]) + 127) // 255
        W = comp.min(-1)
        h = ring & neutral & (W >= bgv.min() + BG_LIFT) & (W >= Winner + IN_LIFT)
        per[k] = int((h & strict).sum()); halo |= h; white |= h & strict
    snapw = ring & (araw >= 251) & (araw <= 254) & (mn >= 215) & (sat <= 25) & (col.mean(-1) > inner.mean(-1) + 24)
    white |= snapw
    lab, nl = nd.label(a > 16, np.ones((3, 3), bool))
    if nl:
        sz = nd.sum(np.ones(a.shape), lab, range(1, nl + 1))
        body = nd.binary_dilation(lab == int(np.argmax(sz)) + 1, iterations=3)
    else:
        body = np.zeros(a.shape, bool)
    per["white_all"] = int(white.sum()); per["rim_all"] = int(halo.sum())
    if ex is not None: body = body & ~ex
    white &= body; halo &= body
    # opaque backdrop pocket: pure neutral white (a>=128) flood-connected within 8 px to a soft/clear pixel
    nw = (mn >= 236) & (sat <= 12) & (a > 0)
    seed = nd.binary_dilation(a < 200, np.ones((3, 3), bool)) & nw
    pocket = nd.binary_dilation(seed, np.ones((3, 3), bool), iterations=8, mask=nw) & (a >= 128) if seed.any() else seed
    per["pocket_all"] = int(pocket.sum()); pocket &= body
    per["pocket"] = int(pocket.sum()); nrim = int(halo.sum())
    halo |= pocket
    return int(white.sum()), nrim, int(snapw.sum()), white, halo, per

def frames_mp4(path, w, h):
    cmd = [FFMPEG, "-hide_banner", "-loglevel", "error", "-nostdin", "-threads", "1", "-protocol_whitelist", "file,pipe", "-f", "mov", "-i", path,
           "-an", "-vf", SCALE_VF, "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"]
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    fsz = 2 * w * h * 3; i = 0
    while True:
        b = p.stdout.read(fsz)
        if len(b) < fsz: break
        yield i, np.frombuffer(b, np.uint8).reshape(h, 2 * w, 3); i += 1
    p.wait()

def crop_png(col, a, P, mask, path):
    ys, xs = np.nonzero(mask)
    # densest 48x48 window
    H, W = a.shape; S = 48
    cnt = nd.uniform_filter(mask.astype(np.float32), S)
    cy, cx = np.unravel_index(np.argmax(cnt), cnt.shape)
    y0 = int(np.clip(cy - S // 2, 0, H - S)); x0 = int(np.clip(cx - S // 2, 0, W - S))
    tiles = []
    for k in ("dark", "magenta"):
        bgv = np.array(BGS[k]); comp = (P + (bgv * (255 - a[..., None]) + 127) // 255).astype(np.uint8)
        tiles.append(comp[y0:y0 + S, x0:x0 + S])
    mk = tiles[0].copy(); mk[mask[y0:y0 + S, x0:x0 + S]] = (255, 0, 0)
    tiles.append(mk)
    strip = np.concatenate([np.pad(t, ((2, 2), (2, 2), (0, 0)), constant_values=128) for t in tiles], 1)
    Image.fromarray(strip).resize((strip.shape[1] * 6, strip.shape[0] * 6), Image.NEAREST).save(path, optimize=True)
    # full-frame thumbnail with box, for the summary sheet
    full = (P + (np.array(BGS["dark"]) * (255 - a[..., None]) + 127) // 255).astype(np.uint8)
    full[mask] = (255, 0, 0)
    im = Image.fromarray(full)
    from PIL import ImageDraw
    ImageDraw.Draw(im).rectangle([x0, y0, x0 + S, y0 + S], outline=(255, 255, 0))
    return im, (x0, y0)

def qa(owner, clip, d):
    r = {"owner": owner, "clip": clip, "dir": d}
    jp = f"{d}/{clip}.json"
    try:
        js = json.load(open(jp))
    except Exception as e:
        r.update(status="NO_JSON", err=str(e)); return r
    sheet = f"{d}/{js.get('sheet', clip + '_sheet.png')}"
    mc = js.get("matte_clean") or {}
    try:
        sha = hashlib.sha256(open(sheet, "rb").read()).hexdigest()
    except Exception:
        sha = None
    r["cleaned"] = bool(mc) and mc.get("sheet_sha256") == sha
    r["matte_clean_present"] = bool(mc)
    v = js.get("video") or {}
    mp4 = f"{d}/{v.get('file', clip + '.mp4')}"
    w, h = js["frame_width"], js["frame_height"]; n = js.get("frame_count", 0)
    if os.path.exists(mp4) and os.path.exists(sheet):
        r["mp4_older_than_sheet"] = os.path.getmtime(mp4) + 1 < os.path.getmtime(sheet)
    m0 = {f: os.path.getmtime(f) for f in (jp, mp4, sheet) if os.path.exists(f)}
    even = set(np.linspace(0, max(n - 1, 0), min(N_EVEN, max(n, 1))).round().astype(int).tolist())
    if os.environ.get("FQA_ALL"): even = set(range(n))
    kept = {}; motion = []; prev = None; nf = 0
    src = "mp4" if os.path.exists(mp4) else "sheet"
    if src == "mp4":
        it = frames_mp4(mp4, w, h)
    else:
        sh = np.asarray(Image.open(sheet).convert("RGBA"))
        def it_sheet():
            for i in range(n):
                c = sh[:h, i * w:(i + 1) * w]
                pk = np.concatenate([c[..., :3], np.repeat(c[..., 3:], 3, -1)], 1)
                yield i, pk
        it = it_sheet()
    for i, pk in it:
        nf += 1
        al = pk[::4, w::4, 1].astype(np.int16)
        if i in even: kept[i] = pk.copy()
        if prev is not None:
            s = float(np.abs(al - prev).mean())
            heapq.heappush(motion, (s, i, pk.copy()))
            if len(motion) > N_MOTION + 8: heapq.heappop(motion)
        prev = al
    mot = [x for x in sorted(motion, reverse=True) if x[1] not in kept][:N_MOTION]
    for s, i, pk in mot: kept[i] = pk
    r["src"] = src; r["frames"] = nf; r["sampled"] = sorted(kept); r["motion_frames"] = [i for _, i, _ in mot]
    perframe = {}; best = None
    for i, pk in sorted(kept.items()):
        col, araw, a, P = unpack(pk, w)
        if src == "sheet":  # straight sheet: no codec snap in PNG path
            a = araw; P = (col * a[..., None] + 127) // 255
        ex = EXCL(clip, i) if EXCL else None
        wp, lp, sw, wmask, lmask, per = analyse(col, araw, a, P, ex)
        perframe[i] = [wp, lp, sw, per.get("pocket", 0), per.get("white_all", 0), per.get("pocket_all", 0)]
        key = (wp, lp)
        if best is None or key > best[0]:
            best = (key, i, (col, a, P, wmask, lmask), per)
    (wp, lp), wi, data, per = best
    r["worst_frame"] = wi; r["white_px"] = wp; r["light_rim_px"] = lp; r["white_per_bg"] = per
    r["snapwhite_px"] = perframe[wi][2]; r["max_light_rim_any_frame"] = max(v[1] for v in perframe.values())
    r["perframe"] = perframe
    r["max_pocket_any_frame"] = max(v[3] for v in perframe.values())
    r["max_white_any_frame"] = max(v[0] for v in perframe.values())
    r["props_white_max"] = max(v[4] - v[0] for v in perframe.values())
    r["props_pocket_max"] = max(v[5] - v[3] for v in perframe.values())
    fail = wp > FAIL_PX or r["max_light_rim_any_frame"] > RIM_FAIL or r["max_pocket_any_frame"] > POCKET_FAIL
    warn = wp >= 4 or r["max_light_rim_any_frame"] > RIM_WARN or r["max_pocket_any_frame"] > POCKET_WARN
    r["verdict"] = "FAIL" if fail else ("WARN" if warn else "PASS")
    r["changed_during_qa"] = any(os.path.exists(f) and os.path.getmtime(f) != t for f, t in m0.items())
    worst = (wp, wi, None)
    if r["verdict"] != "PASS":
        col, a, P, wmask, lmask = data
        mask = wmask if wp > FAIL_PX or (wp >= 4 and r["verdict"] == "WARN") else lmask
        fd = f"{OUT}/fails/{owner}"; os.makedirs(fd, exist_ok=True)
        cp = f"{fd}/{clip}_{r['verdict']}_f{wi:03d}_white{wp}_rim{lp}.png"
        im, xy = crop_png(col, a, P, mask, cp)
        th = f"{OUT}/thumbs"; os.makedirs(th, exist_ok=True)
        im.resize((200, 200)).save(f"{th}/{owner}__{clip}.png")
        r["crop"] = cp; r["crop_xy"] = xy
    return r
if __name__ == "__main__":
    owner, clip, d = sys.argv[1:4]
    r = qa(owner, clip, d)
    print(json.dumps(r))
