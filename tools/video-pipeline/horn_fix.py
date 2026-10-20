"""Lock Lillith's horn colours to a reference frame's palette.

Horn mask = saturated blue/purple/magenta pixels (hue 185..335 deg, S>0.30,
V>0.20, alpha>0) inside the top `top_frac` of the figure's opaque bbox (the
horns are the topmost part of her; ribbons/tattoos sit lower). The hue
distribution inside the mask is histogram-matched to the reference frame's
horn hues (no wrap: the band is contiguous), saturation is matched by mean
ratio, and the change is feathered at the mask edge.

API:   fix_frames(list_of_RGBA_PIL, ref_RGBA_PIL) -> list_of_RGBA_PIL, stats
CLI:   horn_fix.py <ref.png> <in_dir> [<out_dir>]   (in place if no out_dir)
"""
import sys, os, glob
import numpy as np
from PIL import Image
from scipy import ndimage

H_LO, H_HI = 185.0, 335.0

def _hsv(rgb):
    r, g, b = [rgb[..., i] for i in range(3)]
    mx = rgb.max(-1); mn = rgb.min(-1); d = mx - mn
    h = np.zeros_like(mx)
    m = d > 1e-6
    rm = m & (mx == r); gm = m & (mx == g) & ~rm; bm = m & ~rm & ~gm
    h[rm] = ((g - b)[rm] / d[rm]) % 6
    h[gm] = (b - r)[gm] / d[gm] + 2
    h[bm] = (r - g)[bm] / d[bm] + 4
    h *= 60.0
    s = np.where(mx > 1e-6, d / np.maximum(mx, 1e-6), 0)
    return h, s, mx

def _rgb(h, s, v):
    h = (h % 360) / 60.0; i = np.floor(h).astype(int) % 6; f = h - np.floor(h)
    p = v * (1 - s); q = v * (1 - s * f); t = v * (1 - s * (1 - f))
    out = np.zeros(h.shape + (3,), np.float32)
    for k, (a, b, c) in enumerate([(v, t, p), (q, v, p), (p, v, t), (p, q, v), (t, p, v), (v, p, q)]):
        sel = i == k
        out[sel] = np.stack([a[sel], b[sel], c[sel]], -1)
    return out

def horn_mask(arr, top_frac=0.13):
    a = arr[..., 3]
    ys = np.where(a.max(1) > 16)[0]
    if len(ys) == 0:
        return np.zeros(a.shape, bool)
    top, bot = ys.min(), ys.max()
    lim = int(top + top_frac * (bot - top))
    h, s, v = _hsv(arr[..., :3].astype(np.float32) / 255)
    m = (h >= H_LO) & (h <= H_HI) & (s > 0.30) & (v > 0.20) & (a > 0)
    m[lim:] = False
    m = ndimage.binary_opening(m, iterations=1) | (m & ndimage.binary_dilation(ndimage.binary_opening(m, iterations=1), iterations=2))
    return m

def fix_frame(img, ref_h, ref_s_mean, top_frac=0.13):
    arr = np.asarray(img.convert('RGBA')).astype(np.float32)
    m = horn_mask(arr.astype(np.uint8), top_frac)
    if m.sum() < 30:
        return img, 0.0
    rgb = arr[..., :3] / 255
    h, s, v = _hsv(rgb)
    hv = h[m]
    # histogram (quantile) matching of hue
    q = np.linspace(0, 1, 101)
    src_q = np.quantile(hv, q); ref_q = np.quantile(ref_h, q)
    new_h = np.interp(hv, src_q, ref_q)
    shift = float(np.mean(new_h - hv))
    s_ratio = ref_s_mean / max(float(s[m].mean()), 1e-3)
    h2 = h.copy(); s2 = s.copy()
    h2[m] = new_h; s2[m] = np.clip(s[m] * s_ratio, 0, 1)
    new_rgb = _rgb(h2, s2, v)
    w = ndimage.gaussian_filter(m.astype(np.float32), 0.8)[..., None]
    w = np.where(m[..., None], 1.0, w)
    out = rgb * (1 - w) + new_rgb * w
    arr[..., :3] = np.clip(out * 255, 0, 255)
    return Image.fromarray(arr.round().astype(np.uint8), 'RGBA'), shift

def ref_stats(ref, top_frac=0.13):
    arr = np.asarray(ref.convert('RGBA'))
    m = horn_mask(arr, top_frac)
    h, s, v = _hsv(arr[..., :3].astype(np.float32) / 255)
    return h[m], float(s[m].mean()) if m.any() else 0.5

def fix_frames(frames, ref, top_frac=0.13):
    rh, rs = ref_stats(ref, top_frac)
    out, shifts = [], []
    for f in frames:
        g, sh = fix_frame(f, rh, rs, top_frac); out.append(g); shifts.append(round(sh, 1))
    return out, {'ref_horn_px': int(len(rh)), 'ref_hue_median': round(float(np.median(rh)), 1) if len(rh) else None,
                 'mean_hue_shift_deg_per_frame': shifts}

if __name__ == '__main__':
    ref = Image.open(sys.argv[1]); ind = sys.argv[2]; outd = sys.argv[3] if len(sys.argv) > 3 else ind
    os.makedirs(outd, exist_ok=True)
    ps = sorted(glob.glob(f'{ind}/*.png'))
    fixed, st = fix_frames([Image.open(p) for p in ps], ref)
    for p, f in zip(ps, fixed): f.save(os.path.join(outd, os.path.basename(p)))
    print(st)
