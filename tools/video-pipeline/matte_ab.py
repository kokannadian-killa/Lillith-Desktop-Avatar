"""A/B metrics + comparison image for matte_v2 (old v2 frames vs new)."""
import numpy as np, cv2
from PIL import Image, ImageDraw
from scipy import ndimage as nd

def _edge(al):
    solid = al > 0
    return solid & ~nd.binary_erosion(solid, iterations=2)

def _flicker_maps(frames):
    dis = cv2.DISOpticalFlow_create(cv2.DISOPTICAL_FLOW_PRESET_MEDIUM)
    maps = []
    for t in range(len(frames) - 1):
        f0, f1 = frames[t].astype(np.float32), frames[t + 1].astype(np.float32)
        def gray(f):
            a = f[..., 3:] / 255; c = f[..., :3] * a + 128 * (1 - a)
            return cv2.cvtColor(c.astype(np.uint8), cv2.COLOR_RGB2GRAY)
        g0, g1 = gray(frames[t]), gray(frames[t + 1])
        fl = dis.calc(g0, g1, None)
        xx, yy = np.meshgrid(np.arange(400, dtype=np.float32), np.arange(400, dtype=np.float32))
        mx, my = xx + fl[..., 0], yy + fl[..., 1]
        a1 = cv2.remap(f1[..., 3], mx, my, cv2.INTER_LINEAR, borderValue=0)
        gw = cv2.remap(g1, mx, my, cv2.INTER_LINEAR)
        band = nd.binary_dilation(_edge(frames[t][..., 3]), iterations=2)
        ok = band & (np.abs(gw.astype(np.float32) - g0) < 14)
        maps.append(np.where(ok, np.abs(a1 - f0[..., 3]), 0))
    return maps

def metrics(frames):
    edge_n = white_n = 0; halo = []; specks = 0; dirty = 0; haze = 0; semi = 0
    for f in frames:
        al = f[..., 3]; e = _edge(al)
        edge_n += e.sum(); white_n += (e & (f[..., :3].min(-1) > 225)).sum()
        inner = al >= 250
        if inner.any():
            idx = nd.distance_transform_edt(~inner, return_distances=False, return_indices=True)
            lin = f[idx[0], idx[1], :3].astype(np.float32).mean(-1)
            ex = np.maximum(0, f[..., :3].astype(np.float32).mean(-1) - lin) * al / 255
            halo.append(float(ex[e].mean()) if e.any() else 0)
        solid = al > 0; lab, n = nd.label(solid, structure=np.ones((3, 3)))
        if n > 1:
            sz = nd.sum(solid, lab, range(1, n + 1)); main = int(np.argmax(sz)) + 1
            near = nd.binary_dilation(lab == main, iterations=4)
            for i, s_ in enumerate(sz):
                if s_ < 30 and i + 1 != main and not (near & (lab == i + 1)).any(): specks += 1
        dirty += int(((al == 0) & (f[..., :3].max(-1) > 0)).sum())
        haze += int(((al > 0) & (al < 12) & ~nd.binary_dilation(al >= 128, iterations=3)).sum())
        semi += int(((al > 0) & (al < 250)).sum())
    fm = _flicker_maps(frames)
    # static-edge flicker: edge pixels whose look on white (~ the source video) did not change between
    # consecutive frames, so any alpha change there is pure matte noise
    sf = []
    for t in range(len(frames) - 1):
        f0, f1 = frames[t].astype(np.float32), frames[t + 1].astype(np.float32)
        w0 = (f0[..., :3] * f0[..., 3:] / 255 + 255 * (1 - f0[..., 3:] / 255)).mean(-1)
        w1 = (f1[..., :3] * f1[..., 3:] / 255 + 255 * (1 - f1[..., 3:] / 255)).mean(-1)
        band = nd.binary_dilation(_edge(frames[t][..., 3]) | _edge(frames[t + 1][..., 3]), iterations=1)
        st = band & (np.abs(cv2.GaussianBlur(w0, (5, 5), 1.2) - cv2.GaussianBlur(w1, (5, 5), 1.2)) < 3)
        if st.sum() > 50: sf.append(float(np.abs(f0[..., 3] - f1[..., 3])[st].mean()))
    fl = [float(m[m > 0].mean()) if (m > 0).any() else 0 for m in fm]
    N = len(frames)
    return {'white_edge_frac': round(white_n / max(edge_n, 1), 4), 'light_halo': round(float(np.mean(halo)), 3),
            'edge_flicker': round(float(np.mean(fl)), 3), 'edge_flicker_p95_frame': round(float(np.percentile(fl, 95)), 3), 'static_edge_flicker': round(float(np.mean(sf)) if sf else 0, 3),
            'specks': specks, 'dirty_bg_px': dirty, 'haze_px_per_frame': round(haze / N, 1), 'semi_px_per_frame': round(semi / N, 1)}

def _on(f, c):
    b = Image.new('RGBA', (400, 400), c + (255,)); b.alpha_composite(Image.fromarray(f, 'RGBA')); return b.convert('RGB')

def ab_image(clip, old, new, mo, mn, path):
    m = len(old) // 2
    BGS = [(18, 18, 22), (128, 128, 128), (236, 236, 236)]
    # zoom: 80x80 window with the most semi-transparent pixels in the old frame (hair/fishnet edges)
    semi = ((old[m][..., 3] > 0) & (old[m][..., 3] < 250)).astype(np.float32)
    s = cv2.boxFilter(semi, -1, (80, 80), normalize=False)
    cy, cx = np.unravel_index(np.argmax(s), s.shape); x0 = int(np.clip(cx - 40, 0, 320)); y0 = int(np.clip(cy - 40, 0, 320))
    hm_o = np.sum(_flicker_maps(old), 0); hm_n = np.sum(_flicker_maps(new), 0); hmax = max(hm_o.max(), hm_n.max(), 1)
    W = 3 * 300 + 2 * 320 + 300 + 6 * 10; H = 40 + 2 * 330
    out = Image.new('RGB', (W, H), (60, 60, 64)); d = ImageDraw.Draw(out)
    d.text((8, 6), f'{clip}  frame {m}  |  top: OLD generated_v2 (key_white)  bottom: NEW _matte_v2  |  bgs dark/gray/light, 4x edge zoom dark/gray, edge-flicker heatmap (sum over clip)', fill=(255, 255, 255))
    for r, (fr, mt, hm, lab) in enumerate([(old, mo, hm_o, 'OLD'), (new, mn, hm_n, 'NEW')]):
        y = 40 + r * 330; x = 0
        for c in BGS:
            out.paste(_on(fr[m], c).resize((300, 300), Image.LANCZOS), (x, y)); x += 310
        for c in BGS[:2]:
            z = _on(fr[m], c).crop((x0, y0, x0 + 80, y0 + 80)).resize((320, 320), Image.NEAREST); out.paste(z, (x, y)); x += 330
        h = (np.clip(hm / hmax, 0, 1) ** 0.5 * 255).astype(np.uint8)
        hmimg = cv2.applyColorMap(h, cv2.COLORMAP_INFERNO)[..., ::-1]
        out.paste(Image.fromarray(hmimg).resize((300, 300)), (x, y))
        d.text((4, y + 302), f"{lab}: white_edge {mt['white_edge_frac']:.3f}  light_halo {mt['light_halo']:.2f}  edge_flicker {mt['edge_flicker']:.2f} (p95 {mt['edge_flicker_p95_frame']:.2f})  static_edge_flicker {mt['static_edge_flicker']:.2f}  specks {mt['specks']}  dirty_bg {mt['dirty_bg_px']}  haze/f {mt['haze_px_per_frame']}  semi/f {mt['semi_px_per_frame']}", fill=(255, 255, 0))
    d.rectangle([3 * 310 - 2, 38, 3 * 310 + 2 * 330, 40], fill=(90, 90, 90))
    out.save(path)
