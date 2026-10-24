#!/usr/bin/env python3
"""QA pass over every generated_v2 clip (+ _poses bases).
usage: qa_all.py [--fix] [clips...]
Checks per clip: json validity/consistency (frame_count == frames == sheet width/400, durations length,
points per frame), canvas 400x400 RGBA, margins (x 3..396, nothing below y=392), ground contact,
keying (white fringe on edge pixels, isolated specks), horn hue vs frame 0 and vs canon (~278 deg),
loop seam (last->first diff vs typical consecutive diff).
--fix (not on event_*/beam_cast overlays, whose code-drawn effects are white/detached on purpose):
  * removes isolated specks (alpha components < 30 px not within 4 px of the body)
  * de-fringes semi-transparent near-white edge pixels (colour from the opaque neighbours)
  * ground snap: frames whose lowest pixel is 1..8 px above y=392 are moved down onto it
    (not for airborne clips: drag_held, run_*, sit_swing, sleep, wake, tiptoe_bounce, blends with seated ends)
  then rewrites sheet/on_gray/preview gif and bbox_per_frame. Writes _review/qa_report.json + .md."""
import sys, os, glob, json
import numpy as np
from PIL import Image
from scipy import ndimage
import os as _o, sys as _s; _s.path.insert(0, _o.path.dirname(_o.path.abspath(__file__))); import paths
V2 = paths.V2
NO_SNAP = {'drag_held', 'run_right', 'run_left', 'sit_swing', 'sleep', 'wake', 'tiptoe_bounce', 'blend_idle_to_sit_swing',
           'blend_sit_swing_to_idle', 'blend_sleep_to_wake', 'blend_wake_to_sleep', 'salute', 'tada_celebrate', 'dance', 'spell_orb_cast', 'pose_vsign', 'pose_wait_palm', 'twirl', 'victory_jump', 'cheeky_tongue_wink', 'sit_bored_chin', 'sit_cross_legged_bob', 'sit_heel_kick', 'sit_stretch_yawn', 'walk_skip_right', 'walk_skip_left', 'perch_sit', 'cleavage_beckon', 'chest_press_tease', 'arch_trace', 'perch_lounge', 'blend_drag_held_to_perch_sit', 'taskbar_lounge_lie'}
def is_overlay(n): return n.startswith('event_') or n in ('beam_cast', 'blow_kiss', 'heart_hands', 'peace_sign_pose', 'hip_pop_wink')
def horn_hue(a):
    al = a[..., 3] > 200; ys = np.where(al.any(1))[0]
    if not len(ys): return None
    top = ys.min(); H = ys.max() - top; band = slice(top, top + int(0.13 * H))
    rgb = a[band, :, :3].astype(np.float32) / 255; m = al[band]
    mx = rgb.max(-1); mn = rgb.min(-1); d = mx - mn + 1e-6
    r, g, b = rgb[..., 0], rgb[..., 1], rgb[..., 2]
    h = np.where(mx == r, (g - b) / d % 6, np.where(mx == g, (b - r) / d + 2, (r - g) / d + 4)) * 60
    s = d / (mx + 1e-6); sel = m & (s > 0.35) & (mx > 0.25) & (h > 200) & (h < 320)
    return float(np.median(h[sel])) if sel.sum() > 30 else None
def edge_stats(a):
    al = a[..., 3]; solid = al > 0
    edge = solid & ~ndimage.binary_erosion(solid, iterations=2)
    semi = edge & (al < 250)
    white = edge & (a[..., :3].min(-1) > 225)
    return int(edge.sum()), int(white.sum())
def specks(a):
    solid = a[..., 3] > 0; lab, n = ndimage.label(solid, structure=np.ones((3, 3)))
    if n < 2: return [], lab
    sz = ndimage.sum(solid, lab, range(1, n + 1)); main = int(np.argmax(sz)) + 1
    near = ndimage.binary_dilation(lab == main, iterations=4)
    small = [i + 1 for i, s in enumerate(sz) if s < 30 and i + 1 != main and not (near & (lab == i + 1)).any()]
    return small, lab
def defringe(a):
    a = a.copy(); al = a[..., 3]; solid = al > 0
    edge = solid & ~ndimage.binary_erosion(solid, iterations=2)
    bad = edge & (a[..., :3].min(-1) > 215) & (al < 250)
    if not bad.any(): return a, 0
    inner = (al >= 250) & ~bad
    # nearest inner pixel colour
    idx = ndimage.distance_transform_edt(~inner, return_distances=False, return_indices=True)
    near = a[idx[0], idx[1], :3]
    a[bad, :3] = near[bad]
    return a, int(bad.sum())
def lowest(a):
    ys = np.where((a[..., 3] > 128).any(1))[0]; return int(ys.max()) if len(ys) else None
def rewrite(d, name, frames, js):
    import build_clip as bc  # noqa
    N = len(frames); sheet = Image.new('RGBA', (400 * N, 400), (0, 0, 0, 0))
    for i, f in enumerate(frames):
        Image.fromarray(f).save(f'{d}/frames/{name}_{i:02d}.png'); sheet.paste(Image.fromarray(f), (400 * i, 0))
    sheet.save(f'{d}/{name}_sheet.png')
    g = Image.new('RGBA', sheet.size, (128, 128, 128, 255)); g.alpha_composite(sheet); g.convert('RGB').save(f'{d}/{name}_sheet_on_gray.png')
    gif = []
    for f in frames:
        b = Image.new('RGBA', (400, 400), (128, 128, 128, 255)); b.alpha_composite(Image.fromarray(f)); gif.append(b.convert('RGB'))
    durs = js.get('frame_durations_ms') or [int(1000 / js['fps'])] * N
    gif[0].save(f'{d}/{name}_preview.gif', save_all=True, append_images=gif[1:], duration=durs, loop=0)
    bbs = []
    for f in frames:
        ys, xs = np.where(f[..., 3] > 0); bbs.append([int(xs.min()), int(xs.max()), int(ys.min()), int(ys.max())])
    js['bbox_per_frame'] = bbs
    js['canvas_compliant'] = all(b[0] >= 3 and b[1] <= 396 and b[3] <= 392 for b in bbs)
    json.dump(js, open(f'{d}/{name}.json', 'w'), indent=2)
def qa(d, fix):
    name = os.path.basename(d); P = {'clip': name, 'issues': [], 'fixed': {}}
    try: js = json.load(open(f'{d}/{name}.json'))
    except Exception as e: P['issues'].append(f'json invalid: {e}'); return P
    fs = sorted(glob.glob(f'{d}/frames/*.png')); N = len(fs)
    sh = Image.open(f'{d}/{name}_sheet.png')
    if js.get('frame_count') != N: P['issues'].append(f"frame_count {js.get('frame_count')} != {N} frame files")
    if sh.size != (400 * N, 400): P['issues'].append(f'sheet size {sh.size} != {(400 * N, 400)}')
    fd = js.get('frame_durations_ms')
    if fd is not None and len(fd) != N: P['issues'].append(f'frame_durations_ms len {len(fd)} != {N}')
    for k, v in (js.get('frame_points') or {}).items():
        if len(v) != N: P['issues'].append(f'frame_points.{k} len {len(v)} != {N}')
    for k in ('fps', 'loop', 'anchor', 'baseline_y'):
        if k not in js: P['issues'].append(f'missing {k}')
    A = [np.asarray(Image.open(f)) for f in fs]
    if any(a.shape != (400, 400, 4) for a in A): P['issues'].append('frame not 400x400 RGBA'); return P
    changed = False
    if fix and not is_overlay(name):
        nsp = nfr = nsn = 0
        for i, a in enumerate(A):
            sm, lab = specks(a)
            if sm:
                a = a.copy(); a[np.isin(lab, sm)] = 0; nsp += len(sm)
            a, nb = defringe(a); nfr += nb
            ys = np.where((a[..., 3] > 0).any(1))[0]; lo = int(ys.max()) if len(ys) else None  # any-alpha lowest row
            snapped = name not in NO_SNAP and lo is not None and 384 <= lo < 392
            if snapped:
                s = 392 - lo; b = np.zeros_like(a); b[s:] = a[:-s]; a = b; nsn += 1
            below = a[393:, :, 3].any()
            if below: a = a.copy(); a[393:] = 0  # nothing below the baseline row
            if sm or nb or snapped or below: A[i] = a; changed = True
        P['fixed'] = {'specks_removed': nsp, 'fringe_px_recoloured': nfr, 'frames_ground_snapped': nsn}
    if changed: rewrite(d, name, A, js)
    # measurements (after fix)
    lows = [lowest(a) for a in A]; bb = []
    for a in A:
        ys, xs = np.where(a[..., 3] > 0); bb.append((xs.min(), xs.max(), ys.min(), ys.max()))
    if min(b[0] for b in bb) < 3 or max(b[1] for b in bb) > 396: P['issues'].append(f'x margin {min(b[0] for b in bb)}..{max(b[1] for b in bb)}')
    if max(b[3] for b in bb) > 392 and name != 'event_screenshot': P['issues'].append(f'pixels below baseline (y {max(b[3] for b in bb)})')
    P['lowest_y_range'] = [min(lows), max(lows)]
    if name not in NO_SNAP and not name.startswith(('blend_', 'event_')) and min(lows) < 384: P['issues'].append(f'feet lift {392 - min(lows)} px on some frame')
    es = [edge_stats(a) for a in A]; wf = sum(w for _, w in es) / max(1, sum(e for e, _ in es))
    P['white_edge_frac'] = round(wf, 4)
    if wf > 0.02 and not is_overlay(name): P['issues'].append(f'white edge fringe {wf:.1%}')
    nsp = sum(len(specks(a)[0]) for a in A); P['specks'] = nsp
    if nsp and not is_overlay(name) and name != 'spell_orb_cast': P['issues'].append(f'{nsp} isolated specks')
    hh = [horn_hue(a) for a in A]; hv = [h for h in hh if h is not None]
    if hv and not is_overlay(name):  # (event icons near the head pollute the horn band; their base clip is checked)
        P['horn_hue_median'] = round(float(np.median(hv)), 1); P['horn_hue_spread'] = round(float(np.ptp(hv)), 1)
        if np.ptp(hv) > 25: P['issues'].append(f'horn hue spread {np.ptp(hv):.0f} deg')
        if abs(np.median(hv) - 278) > 25: P['issues'].append(f'horn hue {np.median(hv):.0f} vs canon ~278')
    if js.get('loop') and N > 2:
        g = [np.abs(A[i + 1].astype(np.int16) - A[i].astype(np.int16)).mean() for i in range(N - 1)]
        seam = np.abs(A[0].astype(np.int16) - A[-1].astype(np.int16)).mean()
        P['loop_seam_ratio'] = round(float(seam / (np.median(g) + 1e-6)), 2)
        if P['loop_seam_ratio'] > 2.5: P['issues'].append(f"loop seam {P['loop_seam_ratio']}x typical step")
    return P
if __name__ == '__main__':
    sys.path.insert(0, f'{V2}/_tools')
    fix = '--fix' in sys.argv; names = [a for a in sys.argv[1:] if not a.startswith('--')]
    ds = [f'{V2}/{n}' for n in names] if names else sorted(d for d in glob.glob(f'{V2}/*') + glob.glob(f'{V2}/_poses/*')
                                                         if os.path.isdir(d) and not os.path.basename(d).startswith('_'))
    rep = []
    for d in ds:
        P = qa(d, fix); rep.append(P)
        print(P['clip'], 'OK' if not P['issues'] else '; '.join(P['issues']), P.get('fixed') or '', flush=True)
    if not names:
        json.dump(rep, open(f'{V2}/_review/qa_report.json', 'w'), indent=1)
