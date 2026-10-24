"""clipclean shared lib: metrics + cleaning for 400x400 straight-alpha RGBA cells."""
import os as _os
_ROOT = _os.environ.get('CC_REPO', _os.path.abspath(_os.path.join(_os.path.dirname(__file__), '..', '..', '..')))
_OUT = _os.environ.get('CC_OUT', '/workspace/verify/clipclean')
import numpy as np
from scipy import ndimage as nd

OVERLAY_PREFIX = ('event_',)
OVERLAY = {'beam_cast', 'blow_kiss', 'heart_hands', 'peace_sign_pose', 'hip_pop_wink', 'spell_orb_cast'}
def is_overlay(name):
    return name.startswith(OVERLAY_PREFIX) or name in OVERLAY

S8 = np.ones((3, 3), bool)
WHITE_MIN, WHITE_SAT = 226, 20

def components(a):
    """main body mask (largest alpha>16 component plus anything within 3 px of it), islands mask."""
    solid = a > 16
    lab, n = nd.label(solid, S8)
    if n == 0:
        z = np.zeros_like(solid); return z, z, lab, []
    sz = nd.sum(solid, lab, range(1, n + 1)); main = int(np.argmax(sz)) + 1
    near = nd.binary_dilation(lab == main, iterations=3)
    touch = np.unique(lab[near & solid])
    body = np.isin(lab, touch[touch > 0])
    body = nd.binary_dilation(body, iterations=2) & (a > 0) | body
    islands = (a > 0) & ~body
    return body, islands, lab, touch

def nearwhite(rgb):
    mn = rgb.min(-1); sat = rgb.max(-1) - mn
    return (mn >= WHITE_MIN) & (sat <= WHITE_SAT), mn, sat

def leak_mask(rgb, a, body, depth=8):
    """near-white, near-neutral pixels connected (geodesically, <= depth px) to keyed/semi pixels."""
    nw, mn, sat = nearwhite(rgb)
    nw &= (a > 0) & body
    seed = nd.binary_dilation(a < 200, S8) & nw
    if not seed.any():
        return seed
    R = nd.binary_dilation(seed, S8, iterations=depth, mask=nw)
    # keep only flood components that read as background (pure, neutral white)
    lab, n = nd.label(R, S8)
    if n:
        ids = np.arange(1, n + 1)
        mmn = nd.mean(mn, lab, ids); msat = nd.mean(sat, lab, ids)
        ok = ids[(mmn >= 236) & (msat <= 12)]
        R = np.isin(lab, ok)
    return R

def metrics(c, name=''):
    rgb = c[..., :3].astype(np.int32); a = c[..., 3].astype(np.int32)
    body, islands, lab, _ = components(a)
    clear = a < 8
    d = nd.distance_transform_edt(~clear)
    band = (a > 0) & (d <= 2.5)
    nw, mn, sat = nearwhite(rgb)
    white_edge = int((band & nw & body).sum())
    white_edge_opaque = int((band & nw & body & (a >= 200)).sum())
    leak = leak_mask(rgb, a, body)
    leak_px = int((leak & (a >= 128)).sum())
    isl = islands & (a > 16)
    stray = int(isl.sum())
    far = int((isl & (nd.distance_transform_edt(~body) > 8) & (a > 128)).sum())
    semi = (a > 16) & (a < 200)
    thick_semi = int(nd.binary_erosion(semi, iterations=2).sum())
    fill = nd.binary_fill_holes(body & (a >= 128))
    holes = fill & ~(a >= 128)
    hl, hn = nd.label(holes, S8)
    small_holes = 0
    if hn:
        col = holes & (a > 0) & (sat > 25)
        ids = np.arange(1, hn + 1)
        hs = nd.sum(holes, hl, ids); hc = nd.sum(col, hl, ids)
        small_holes = int(sum(s for s, k in zip(hs, hc) if s <= 12 and k > 0))
    interior = nd.binary_erosion(fill, iterations=3)
    see_through = int((body & (d >= 3) & (a >= 8) & (a < 250)).sum())
    floor = int(((a > 8) & (a < 200) & (sat <= 30) & (mn > 60))[372:].sum())
    return dict(white_edge=white_edge, white_edge_opaque=white_edge_opaque, leak=leak_px, stray=stray, far_opaque=far,
                thick_semi=thick_semi, small_holes=small_holes, see_through=see_through, floor_semi=floor,
                body_px=int((a >= 128).sum()), interior_px=int(interior.sum()))

def severity(m, overlay=False):
    s = 3.0 * m['leak'] + 1.0 * m['white_edge_opaque'] + 0.3 * m['white_edge'] + 2.0 * m['thick_semi'] + 0.3 * m['see_through'] \
        + 1.0 * m['small_holes'] + 0.5 * m['floor_semi']
    if not overlay:
        s += 1.0 * m['stray'] + 3.0 * m['far_opaque']
    return s

def clean(c, name=''):
    """returns cleaned straight RGBA uint8 and stats. Never lowers alpha deeper than 2.5 px inside the
    silhouette except on near-white background pockets (leak) that touch keyed pixels."""
    ov = is_overlay(name)
    rgb = c[..., :3].astype(np.float32); a = c[..., 3].astype(np.float32)
    ai = a.astype(np.int32)
    body, islands, lab, _ = components(ai)
    st = {}
    # 1) islands (not on overlay clips: their FX are detached on purpose)
    if not ov and islands.any():
        il, n = nd.label(islands & (ai > 0), S8)
        rm = np.zeros_like(islands)
        nw, mn, sat = nearwhite(rgb.astype(np.int32))
        for i in range(1, n + 1):
            m = il == i; area = m.sum()
            if area < 400 or sat[m].mean() < 30 or a[m].mean() < 120:
                rm |= m
        a[rm] = 0; st['island_px'] = int(rm.sum())
    a_in = a.copy()
    # 2) white leak pockets (iterate: removing a rim exposes the next rim)
    tot = 0
    for _ in range(3):
        L = leak_mask(rgb.astype(np.int32), a.astype(np.int32), body)
        if not L.any(): break
        mn = rgb.min(-1)
        unmix = np.clip((255.0 - mn) / 55.0, 0, 1)
        newa = np.minimum(a, a * unmix)
        tot += int((L & (a >= 128) & (newa < 128)).sum())
        a = np.where(L, newa, a)
    st['leak_px_cleared'] = tot
    # 2b) neutral grey ghosts (keyed-background residue): inside enclosed gaps, and on the outer edge where the
    #     interior colour is clearly not neutral (skin, hair, pink) so a grey semi pixel cannot be her edge
    ri = rgb.astype(np.int32); mn = ri.min(-1); sat = ri.max(-1) - mn; luma = ri.mean(-1)
    G = (a > 0) & (a < 170) & (sat <= 18) & (luma >= 70) & body
    ghost = np.zeros_like(G)
    if G.any():
        solid = (a >= 170) & body
        pocket = nd.binary_fill_holes(solid) & ~solid
        pl, pn = nd.label(pocket, S8)
        if pn:
            ids = np.arange(1, pn + 1)
            psat = nd.mean(sat, pl, ids); pmin_a = nd.minimum(a, pl, ids)
            bgp = np.isin(pl, ids[(psat <= 22) | (pmin_a < 8)])
            ghost |= G & bgp
        clear0 = a < 8
        d0 = nd.distance_transform_edt(~clear0)
        ref0 = (a >= 250) & (d0 >= 3) & body
        if ref0.any():
            _, (ry, rx) = nd.distance_transform_edt(~ref0, return_indices=True)
            F0 = rgb[ry, rx].astype(np.int32)
            fsat = F0.max(-1) - F0.min(-1)
            ghost |= G & (d0 <= 2.5) & (fsat > 30) & (np.abs(ri - F0).mean(-1) > 50)
        a[ghost] = 0
    st['ghost_px_cleared'] = int(ghost.sum())
    # 2c) enclosed white slivers between hair strands (background seen through the hair, alpha left at 255)
    ri = rgb.astype(np.int32); mn = ri.min(-1); sat = ri.max(-1) - mn
    W = (mn >= 236) & (sat <= 12) & (a > 0) & body
    wl, wn = nd.label(W, S8); sl = np.zeros_like(W)
    if wn:
        R_, G_, B_ = ri[..., 0], ri[..., 1], ri[..., 2]
        blonde = (R_ > 120) & (R_ >= G_) & (G_ >= B_) & (R_ - B_ > 20) & (a >= 128)
        transp = a < 128
        dclear = nd.distance_transform_edt(a >= 8)
        for i, sls in enumerate(nd.find_objects(wl), 1):
            ys, xs = sls
            ys = slice(max(ys.start - 2, 0), ys.stop + 2); xs = slice(max(xs.start - 2, 0), xs.stop + 2)
            m = wl[ys, xs] == i
            if m.sum() > 300 or dclear[ys, xs][m].min() > 6: continue
            ring = nd.binary_dilation(m, S8, iterations=2) & ~m
            hr = (blonde[ys, xs][ring] | transp[ys, xs][ring]).mean()
            if hr >= 0.6:
                sl[ys, xs] |= m
        mnf = rgb.min(-1)
        a = np.where(sl, a * np.clip((255.0 - mnf) / 55.0, 0, 1), a)
    st['sliver_px_cleared'] = int(sl.sum())
    # 2d) pure-white opaque specks hugging the silhouette edge (backdrop seen through a gap whose transparent
    #     core was already keyed): pure neutral white (min>=244, sat<=6), <=300 px, within 4 px of clear.
    ri = rgb.astype(np.int32); mn = ri.min(-1); sat = ri.max(-1) - mn
    P = (mn >= 244) & (sat <= 6) & (a > 0) & body
    pl_, pn_ = nd.label(P, S8); pw = np.zeros_like(P)
    if pn_:
        dclear = nd.distance_transform_edt(a >= 8)
        ids = np.arange(1, pn_ + 1)
        ar = nd.sum(P, pl_, ids); dm = nd.minimum(dclear, pl_, ids)
        pw = np.isin(pl_, ids[(ar <= 300) & (dm <= 4)])
        # grow 1 px into the near-white rim so no light fringe is left around the removed speck
        rim = nd.binary_dilation(pw, S8) & (mn >= 226) & (sat <= 14) & body
        pw |= rim
        mnf = rgb.min(-1)
        a = np.where(pw, a * np.clip((255.0 - mnf) / 55.0, 0, 1), a)
    st['white_speck_px_cleared'] = int(pw.sum())
    # 2e) debris: once a white pocket is opened, tiny specks and 1-px dangling ink inside it read as noise.
    #     Only within 3 px of pixels just cleared above.
    cleared = (a_in - a) > 100
    deb = np.zeros_like(cleared)
    if cleared.any():
        zone = nd.binary_dilation(cleared, S8, iterations=3)
        for _ in range(2):
            vis = a > 16
            nb = nd.convolve(vis.astype(np.int32), np.ones((3, 3), np.int32), mode='constant') - vis
            d1 = zone & vis & (nb <= 2)
            dl, dn = nd.label(vis, S8)
            if dn:
                ids = np.arange(1, dn + 1); sz = nd.sum(vis, dl, ids)
                small = np.isin(dl, ids[sz < 16]) & zone
            else:
                small = np.zeros_like(vis)
            k = d1 | small
            if not k.any(): break
            a[k] = 0; deb |= k
    st['debris_px_cleared'] = int(deb.sum())
    # 3) edge decontamination against the white backdrop
    clear = a < 8
    d, (iy, ix) = nd.distance_transform_edt(~clear, return_indices=True)
    ref = (a >= 250) & (d >= 3) & body
    if ref.any():
        _, (ry, rx) = nd.distance_transform_edt(~ref, return_indices=True)
        F = rgb[ry, rx]
        band = (a > 0) & (d <= 2.5) & body
        u = 255.0 - F; v = rgb - F
        uu = (u * u).sum(-1) + 1e-3
        t = np.clip((v * u).sum(-1) / uu, 0, 1)
        lighter = rgb.mean(-1) > F.mean(-1) + 6
        fix = band & lighter & (t > 0.04) & (uu > 900)
        a2 = np.where(fix, a * (1 - t), a)
        k = np.clip(t * 2.5, 0, 1)[..., None]
        rgb = np.where(fix[..., None], rgb * (1 - k) + F * k, rgb)
        st['edge_px_decontaminated'] = int(fix.sum())
        a = a2
    # 4) interior translucency (key ate her): coloured / dark pixels >= 3 px inside the silhouette go opaque
    ri = rgb.astype(np.int32); sat = ri.max(-1) - ri.min(-1); luma = ri.mean(-1)
    dcl = nd.distance_transform_edt(a >= 8)
    inner = body & (dcl >= 3) & (a >= 8) & (a < 250) & ((sat > 25) | (luma < 90))
    a[inner] = 255
    st['interior_px_opaqued'] = int(inner.sum())
    out = np.dstack([np.clip(rgb, 0, 255), np.clip(a, 0, 255)]).round().astype(np.uint8)
    out[out[..., 3] <= 3] = 0
    if not ov:
        out, st['specks_dropped'] = drop_specks(out)
    return out, st

def drop_specks(out):
    """5) detached specks: alpha components < 30 px not within 4 px of the main body (same rule as qa_all)."""
    solid = out[..., 3] > 0; lab, n = nd.label(solid, S8)
    if n < 2: return out, 0
    sz = nd.sum(solid, lab, range(1, n + 1)); main = int(np.argmax(sz)) + 1
    near = nd.binary_dilation(lab == main, iterations=4)
    hit = np.unique(lab[near])
    drop = [i + 1 for i, s_ in enumerate(sz) if s_ < 30 and i + 1 != main and (i + 1) not in hit]
    if not drop: return out, 0
    m = np.isin(lab, drop); out = out.copy(); out[m] = 0
    return out, int(m.sum())
