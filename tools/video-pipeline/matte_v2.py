#!/usr/bin/env python3
"""matte_v2: improved white-background matte for Lillith generated_v2 clips.

  matte_v2.py <clip> [--out DIR] [--no-temporal] [--threads 2] [--keep-cache]

Rebuilds <clip> from its raw video with a better matte and writes ONLY to
$LILLITH_V2/_matte_v2/<clip>/ (tools/video-pipeline/paths.py) (frames/, sheet, sheet_on_gray, preview gif, json
copied from the v2 clip + matte_version note, ab.png, metrics.json).
Frame selection, scale and per-frame placement are taken from the existing v2
clip (json source_frame_range/scale + integer registration against the v2
frame alpha), so output is a drop-in replacement for build_clip.py output.
Clips with `mirrored_from` are mirrored from that clip's _matte_v2 result.

Matte (per video frame, 1280x720):
 1. isnet-anime soft matte M (onnxruntime direct, low-memory session, cached in /tmp).
 2. background plate B: smooth estimate of the actual (near-)white backdrop.
 3. foreground colour estimate Fest: pull-push fill of solid interior colours into the edge band.
 4. edge band alpha: projection of (I-B) onto (Fest-B) where Fest is distinguishable
    from white (hair, fishnet, skin, outline), blended with M by that reliability;
    outside M (horn/orb glow) alpha = minimal-alpha unmix so coloured glow survives.
 5. temporal: flow-compensated (DIS) median of alpha with the adjacent video frames
    (t-2..t+2) in the edge band, only where photometrically consistent -> no edge flicker.
 6. colour decontamination: unmix against B, fall back to Fest at low alpha, clamp
    edge lightness to the interior (no white halo); alpha<1/255 -> rgba 0; specks dropped.
 7. premultiplied area downscale, placement registered to the v2 frame, horn_fix as build_clip.
"""
import os, sys, json, glob, argparse, subprocess, shutil, time
import numpy as np, cv2
from PIL import Image, ImageDraw
from scipy import ndimage as nd

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import paths
V2 = paths.V2
OUTROOT = os.environ.get('LILLITH_MATTE_OUT', os.path.join(V2, '_matte_v2'))
MODEL = os.path.join(paths.U2NET, 'isnet-anime.onnx')
CACHE = os.environ.get('LILLITH_MATTE_CACHE', '/tmp/matte_v2_cache')
VERSION = 'matte_v2.1'

# ---------------------------------------------------------------- isnet
_S = None
def isnet(rgb, threads=2):
    global _S
    import onnxruntime as ort
    if _S is None:
        import matte_model; matte_model.check_file(MODEL)   # clear error, never a raw onnxruntime failure
        so = ort.SessionOptions(); so.intra_op_num_threads = threads; so.inter_op_num_threads = 1
        so.enable_cpu_mem_arena = False; so.enable_mem_pattern = False
        _S = ort.InferenceSession(MODEL, so, providers=['CPUExecutionProvider'])
    h, w = rgb.shape[:2]
    x = cv2.resize(rgb, (1024, 1024), interpolation=cv2.INTER_LANCZOS4).astype(np.float32) / 255.0
    x = x / max(x.max(), 1e-6)
    x = (x - np.array([0.485, 0.456, 0.406], np.float32)) / 1.0
    x = x.transpose(2, 0, 1)[None]
    y = _S.run(None, {_S.get_inputs()[0].name: x})[0][0, 0]
    y = (y - y.min()) / max(y.max() - y.min(), 1e-6)
    return np.clip(cv2.resize(y, (w, h), interpolation=cv2.INTER_LINEAR), 0, 1)

# ---------------------------------------------------------------- helpers
def fill_colour(rgb, known, sigmas=(1.0, 2.0, 5.0, 12.0, 30.0)):
    """Fest: normalised-convolution fill of rgb from known pixels, finest level wins."""
    k = known.astype(np.float32)
    out = np.zeros(rgb.shape, np.float32); have = np.zeros(k.shape, bool)
    for s in sigmas:
        w = cv2.GaussianBlur(k, (0, 0), s)
        c = cv2.GaussianBlur(rgb * k[..., None], (0, 0), s)
        ok = (w > 0.03) & ~have
        out[ok] = c[ok] / w[ok, None]; have |= ok
    out[~have] = rgb[~have]
    return out

def bg_plate(rgb, M):
    bgm = (M < 0.02) & (rgb.min(-1) > 200)
    bgm = nd.binary_erosion(bgm, iterations=3)
    if bgm.sum() < 1000:
        return np.full(rgb.shape, 255, np.float32)
    small = cv2.resize(rgb, None, fx=0.25, fy=0.25, interpolation=cv2.INTER_AREA)
    ms = cv2.resize(bgm.astype(np.float32), None, fx=0.25, fy=0.25, interpolation=cv2.INTER_AREA) > 0.99
    med = np.median(rgb[bgm], axis=0)
    w = cv2.GaussianBlur(ms.astype(np.float32), (0, 0), 12)
    c = cv2.GaussianBlur(small * ms[..., None], (0, 0), 12)
    p = np.where(w[..., None] > 0.05, c / np.maximum(w, 1e-6)[..., None], med)
    return cv2.resize(p, (rgb.shape[1], rgb.shape[0]), interpolation=cv2.INTER_LINEAR).astype(np.float32)

def raw_alpha(rgb, M):
    """Per-frame alpha (before temporal) + aux data for colour pass."""
    I = rgb.astype(np.float32)
    B = bg_plate(I, M)
    D = B - I                                            # >0 where darker than backdrop
    d = np.abs(D).max(-1)
    # backdrop regions independent of the matte: near-white connected to the border, or big
    # very pure enclosed blobs (gap between arm and head, tail loops) - same rule as key_white
    mn = (I.min(-1) >= 242)
    lab, n = nd.label(mn)
    holes = np.zeros(M.shape, bool)
    if n:
        ids = np.arange(1, n + 1)
        sz = nd.sum(mn, lab, ids); mean = nd.mean(I.min(-1), lab, ids)
        border = np.unique(np.concatenate([lab[0], lab[-1], lab[:, 0], lab[:, -1]]))
        sel = np.zeros(n + 1, bool); sel[border] = True
        sel[ids[(sz >= 150) & (mean >= 249)]] = True; sel[0] = False
        holes = sel[lab]
    if os.environ.get('LILLITH_WHITE_TRIM') == '1':
        # white outfit parts (Santa fur trim, white lace) on the white backdrop: keep near-white
        # pixels the isnet matte is confident about instead of keying them as backdrop
        holes &= ~(nd.binary_erosion(M > 0.85, iterations=1))
    M = np.where(holes, np.minimum(M, 0.0), M)
    core = nd.binary_erosion(M > 0.95, iterations=2) & ~nd.binary_dilation(holes, iterations=3)
    noise = 5.0
    a_min = np.clip((np.maximum(D, 0) - noise).max(-1) / np.maximum(B.max(-1) - noise, 1), 0, 1)
    # known colours: solid interior + pixels that are certainly opaque (dark outline / lineart)
    fgm = (M > 0.5) & ~holes
    inner = fgm & (nd.distance_transform_edt(fgm) > 2.5)          # >=3 px inside the matte: not an edge mix
    Fest = fill_colour(I, core | inner | ((a_min > 0.85) & (M > 0.3)))
    FB = Fest - B
    nFB = np.sqrt((FB ** 2).sum(-1))
    a_proj = np.clip(((I - B) * FB).sum(-1) / np.maximum(nFB ** 2, 1e-3), 0, 1)
    rel = np.clip((nFB - 25.0) / 60.0, 0, 1)             # Fest distinguishable from white?
    band = (nd.binary_dilation(M > 0.05, iterations=4) | nd.binary_dilation(holes, iterations=4)) & ~core & ~holes
    glow = nd.binary_dilation(M > 0.3, iterations=14) & ~core & (d > 9) & (M < 0.5)
    core = core | inner                                  # >=3 px inside the matte: opaque (textured interiors never see-through)
    band = band & ~core
    A = np.where(core, 1.0, 0.0).astype(np.float32)
    ab = np.maximum(rel * a_proj + (1 - rel) * M, a_min)  # a_min = physical lower bound (F >= 0)
    ab = np.where(d < noise, 0.0, ab)                    # backdrop-coloured pixels in the band are background
    A[band] = ab[band]
    A[glow] = np.maximum(A[glow], np.minimum(a_min[glow], 0.85))
    # enclosed backdrop holes (gaps between tail and leg etc.): pure backdrop & matte says bg
    hole = holes | ((d < noise) & (M < 0.5))
    A[hole] = 0
    return A, dict(I=I, B=B, Fest=Fest, rel=rel, core=core, band=band | glow)

def colour(A, aux):
    I, B, Fest, rel = aux['I'], aux['B'], aux['Fest'], aux['rel']
    a = np.clip(A, 0, 1)[..., None]
    F = np.clip((I - (1 - a) * B) / np.maximum(a, 1e-3), 0, 255)
    w = np.clip((a - 0.12) / 0.55, 0, 1)
    use_est = rel[..., None]                               # glow (rel~0) keeps its unmixed colour
    F = w * F + (1 - w) * (use_est * Fest + (1 - use_est) * F)
    # defringe: edge pixels not lighter than the interior they belong to
    band = aux['band'][..., None]
    lum = F.mean(-1, keepdims=True); lest = Fest.mean(-1, keepdims=True)
    cap = lest + 14.0
    scale = np.where((lum > cap) & band & (rel[..., None] > 0.3), cap / np.maximum(lum, 1e-3), 1.0)
    F = np.clip(F * scale, 0, 255)
    out = np.dstack([F, A[..., None] * 255.0])
    out = np.clip(np.round(out), 0, 255).astype(np.uint8)
    out[out[..., 3] == 0] = 0
    return out

def drop_specks(rgba, min_px=60, near=6):
    al = rgba[..., 3] > 4
    lab, n = nd.label(al, structure=np.ones((3, 3)))
    if n < 2: return rgba
    sz = nd.sum(al, lab, range(1, n + 1)); main = int(np.argmax(sz)) + 1
    nearm = nd.binary_dilation(lab == main, iterations=near)
    bad = np.zeros(n + 1, bool)
    for i, s in enumerate(sz):
        j = i + 1
        if j != main and s < min_px and not (nearm & (lab == j)).any(): bad[j] = True
    rgba = rgba.copy(); kill = bad[lab] | ((rgba[..., 3] <= 4) & ~nd.binary_dilation(al, iterations=2))
    rgba[kill] = 0
    return rgba

_DIS = None
def flow(g0, g1):
    """flow from g0 to g1 at half res (returns full-res map coords to sample g1/a1 into g0)."""
    global _DIS
    if _DIS is None:
        _DIS = cv2.DISOpticalFlow_create(cv2.DISOPTICAL_FLOW_PRESET_MEDIUM)
    s0 = cv2.resize(g0, None, fx=0.5, fy=0.5, interpolation=cv2.INTER_AREA)
    s1 = cv2.resize(g1, None, fx=0.5, fy=0.5, interpolation=cv2.INTER_AREA)
    f = _DIS.calc(s0, s1, None)
    f = cv2.resize(f, (g0.shape[1], g0.shape[0]), interpolation=cv2.INTER_LINEAR) * 2
    h, w = g0.shape
    xx, yy = np.meshgrid(np.arange(w, dtype=np.float32), np.arange(h, dtype=np.float32))
    return xx + f[..., 0], yy + f[..., 1]

def temporal(A, g, An, gn_list, band):
    """flow-compensated median of alpha with neighbours, edge band only."""
    vals = [A]
    for An_, gn in zip(An, gn_list):
        mx, my = flow(g, gn)
        aw = cv2.remap(An_, mx, my, cv2.INTER_LINEAR, borderMode=cv2.BORDER_CONSTANT, borderValue=0)
        gw = cv2.remap(gn, mx, my, cv2.INTER_LINEAR, borderMode=cv2.BORDER_REPLICATE)
        ok = np.abs(gw.astype(np.float32) - g.astype(np.float32)) < 10
        vals.append(np.where(ok, aw, A))
    if len(vals) == 1: return A
    S = np.median(np.stack(vals), 0) if len(vals) % 2 == 1 else np.mean(np.stack(vals), 0)
    return np.where(band, S, A).astype(np.float32)

# ---------------------------------------------------------------- canvas
def downscale(rgba, S):
    f = rgba.astype(np.float32) / 255.0
    pm = np.dstack([f[..., :3] * f[..., 3:], f[..., 3:]])
    w, h = round(rgba.shape[1] * S), round(rgba.shape[0] * S)
    r = cv2.resize(pm, (w, h), interpolation=cv2.INTER_AREA)
    a = r[..., 3:]
    col = np.where(a > 1e-4, r[..., :3] / np.maximum(a, 1e-4), 0)
    out = np.clip(np.round(np.dstack([col, a]) * 255), 0, 255).astype(np.uint8)
    out[out[..., 3] == 0] = 0
    return out

def put(small, tx, ty):
    cv = np.zeros((400, 400, 4), np.uint8)
    h, w = small.shape[:2]
    x0, y0 = max(0, tx), max(0, ty); x1, y1 = min(400, tx + w), min(400, ty + h)
    if x1 > x0 and y1 > y0:
        cv[y0:y1, x0:x1] = small[y0 - ty:y1 - ty, x0 - tx:x1 - tx]
    cv[393:] = 0
    return cv

def register(small, old_a, guess=None):
    """integer offset of `small` on the canvas that best matches old alpha."""
    oa = old_a.astype(np.float32) / 255
    sa = small[..., 3]
    ys, xs = np.where(oa > 0.5); ys2, xs2 = np.where(sa > 128)
    gx = int(xs.min() - xs2.min()); gy = int(ys.min() - ys2.min())
    best = None
    for dy in range(-6, 7):
        for dx in range(-6, 7):
            p = put(small, gx + dx, gy + dy)[..., 3].astype(np.float32) / 255
            e = np.abs(p - oa).sum()
            if best is None or e < best[0]: best = (e, gx + dx, gy + dy)
    return best[1], best[2], best[0] / max(1, (oa > 0.5).sum())

# ---------------------------------------------------------------- outputs
def write_outputs(name, frames, js, outdir, durs):
    os.makedirs(f'{outdir}/frames', exist_ok=True)
    for p in glob.glob(f'{outdir}/frames/*.png'): os.remove(p)
    ims = [Image.fromarray(f, 'RGBA') for f in frames]
    for n, im in enumerate(ims): im.save(f'{outdir}/frames/{name}_{n:02d}.png', optimize=True)
    N = len(ims)
    sheet = Image.new('RGBA', (400 * N, 400), (0, 0, 0, 0))
    for n, im in enumerate(ims): sheet.paste(im, (400 * n, 0))
    sheet.save(f'{outdir}/{name}_sheet.png', optimize=True)
    g = Image.new('RGBA', sheet.size, (128, 128, 128, 255)); g.alpha_composite(sheet)
    g.convert('RGB').save(f'{outdir}/{name}_sheet_on_gray.png')
    gif = []
    for im in ims:
        b = Image.new('RGBA', (400, 400), (128, 128, 128, 255)); b.alpha_composite(im); gif.append(b.convert('RGB'))
    gif[0].save(f'{outdir}/{name}_preview.gif', save_all=True, append_images=gif[1:], duration=durs,
                loop=0 if js.get('loop', True) else 1)
    bbs = []
    for f in frames:
        ys, xs = np.where(f[..., 3] > 16); bbs.append([int(xs.min()), int(xs.max()), int(ys.min()), int(ys.max())])
    js = dict(js)
    js['bbox_per_frame'] = bbs
    js['canvas_compliant'] = all(b[0] >= 3 and b[1] <= 396 and b[3] <= 392 for b in bbs)
    json.dump(js, open(f'{outdir}/{name}.json', 'w'), indent=2)
    return js

def frames_of(clip):
    return sorted(glob.glob(f'{V2}/{clip}/frames/{clip}_*.png'))

def durations(js):
    d = js.get('frame_durations_ms')
    return d if d and len(d) == js['frame_count'] else int(round(1000 / js.get('fps', 12)))

# ---------------------------------------------------------------- main
def build(clip, out=None, use_temporal=True, threads=2, keep_cache=False, tspan=2):
    t0 = time.time()
    js = json.load(open(f'{V2}/{clip}/{clip}.json'))
    outdir = out or os.path.join(OUTROOT, clip)
    note = {"matte_version": VERSION, "matte_tool": "generated_v2/_matte_v2/matte_v2.py",
            "matte_note": "isnet-anime soft matte + backdrop-plate projection alpha in the edge band (hair/fishnet), minimal-alpha unmix for glow, "
                          "flow-compensated temporal median of edge alpha (t-2..t+2 video frames), colour decontamination against the "
                          "backdrop + interior colour fallback, edge lightness clamp (no white halo), premultiplied area downscale, "
                          "placement registered to the v2 frame, horn_fix as build_clip. Replaces key_white.key_combined.",
            "matte_ab_of": f"generated_v2/{clip}/"}
    if js.get('mirrored_from'):
        src = js['mirrored_from']
        sd = os.path.join(OUTROOT, src)
        if not os.path.exists(f'{sd}/{src}.json'): raise SystemExit(f'build {src} first (mirror source)')
        frames = [np.asarray(Image.open(p).convert('RGBA'))[:, ::-1].copy() for p in sorted(glob.glob(f'{sd}/frames/{src}_*.png'))]
        old = [np.asarray(Image.open(p).convert('RGBA')) for p in frames_of(clip)]
        js2 = dict(js); js2.update(note); js2['matte_note'] = f'Horizontal mirror of _matte_v2/{src}. ' + note['matte_note']
        write_outputs(clip, frames, js2, outdir, durations(js))
        return finish(clip, old, frames, outdir, t0)
    rng = js['source_frame_range']; S = float(js['scale'])
    video = os.path.normpath(os.path.join(V2, clip, js['source_files'][0]))
    idx = list(range(rng['first'], rng['last'] + 1, rng['step']))
    old = [np.asarray(Image.open(p).convert('RGBA')) for p in frames_of(clip)]
    assert len(old) == len(idx), (len(old), len(idx))
    fd = os.path.join(CACHE, os.path.splitext(os.path.basename(video))[0])
    if not glob.glob(f'{fd}/f_*.png'):
        os.makedirs(fd, exist_ok=True)
        subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-y', '-protocol_whitelist', 'file,pipe', '-i', os.path.abspath(video), f'{fd}/f_%03d.png'], check=True)
    nvid = len(glob.glob(f'{fd}/f_*.png'))
    load = lambda i: np.asarray(Image.open(f'{fd}/f_{i:03d}.png').convert('RGB'))
    def mask(i):
        p = f'{fd}/m_{i:03d}.png'
        if os.path.exists(p): return np.asarray(Image.open(p)).astype(np.float32) / 255
        m = isnet(load(i), threads); Image.fromarray((m * 255).round().astype(np.uint8)).save(p); return m
    cacheA = {}
    def alpha_of(i):
        if i not in cacheA:
            rgb = load(i); M = mask(i); A, aux = raw_alpha(rgb, M)
            cacheA[i] = (A, aux, cv2.cvtColor(rgb, cv2.COLOR_RGB2GRAY))
        return cacheA[i]
    frames = []; offs = []
    for k, i in enumerate(idx):
        A, aux, g = alpha_of(i)
        if use_temporal:
            nb = [j for j in range(i - tspan, i + tspan + 1) if j != i and 1 <= j <= nvid]
            nbs = [alpha_of(j) for j in nb]
            A = temporal(A, g, [n[0] for n in nbs], [n[2] for n in nbs], aux['band'])
        rgba = drop_specks(colour(A, aux))
        small = downscale(rgba, S)
        tx, ty, err = register(small, old[k][..., 3])
        frames.append(put(small, tx, ty)); offs.append([tx, ty, round(float(err), 4)])
        for j in list(cacheA):
            if j < i - tspan: del cacheA[j]
        print(f'  {clip} {k + 1}/{len(idx)} vid {i} off {tx},{ty} reg_err {err:.3f}', flush=True)
    horn = None
    if True:
        try:
            from horn_fix import fix_frames
            A1, aux1, _ = alpha_of(1) if 1 <= nvid else alpha_of(idx[0])
            ref = put(downscale(drop_specks(colour(A1, aux1)), S), offs[0][0], offs[0][1])
            fixed, horn = fix_frames([Image.fromarray(f, 'RGBA') for f in frames], Image.fromarray(ref, 'RGBA'))
            frames = [np.asarray(f.convert('RGBA')).copy() for f in fixed]
        except Exception as e:
            print('horn_fix skipped:', e)
    for f in frames: f[f[..., 3] == 0] = 0; f[393:] = 0
    js2 = dict(js); js2.update(note)
    js2['matte_placement'] = {'per_frame_offset_xy_regerr': offs, 'note': 'integer canvas offset registered to the v2 frame alpha'}
    if horn is not None: js2['horn_fix'] = horn
    write_outputs(clip, frames, js2, outdir, durations(js))
    if not keep_cache: shutil.rmtree(fd, ignore_errors=True)
    return finish(clip, old, frames, outdir, t0)

def finish(clip, old, new, outdir, t0):
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    from matte_ab import metrics, ab_image
    mo, mn = metrics(old), metrics(new)
    json.dump({'clip': clip, 'old_v2': mo, 'matte_v2': mn, 'seconds': round(time.time() - t0)}, open(f'{outdir}/metrics.json', 'w'), indent=2)
    ab_image(clip, old, new, mo, mn, f'{outdir}/ab.png')
    print(clip, 'old', mo, '\n', ' ' * len(clip), 'new', mn, f'({time.time() - t0:.0f}s)')
    return mo, mn

if __name__ == '__main__':
    ap = argparse.ArgumentParser()
    ap.add_argument('clips', nargs='+'); ap.add_argument('--out')
    ap.add_argument('--no-temporal', action='store_true'); ap.add_argument('--threads', type=int, default=2)
    ap.add_argument('--keep-cache', action='store_true'); ap.add_argument('--tspan', type=int, default=2, help='temporal radius in video frames (2 -> median of 5)')
    a = ap.parse_args()
    import matte_model; matte_model.preflight_or_exit()
    for c in a.clips:
        build(c, a.out if len(a.clips) == 1 else None, not a.no_temporal, a.threads, a.keep_cache, a.tspan)
