#!/usr/bin/env python3
"""Re-draw the old code-drawn hearts/sparkles (generated/_tools/build_allure.py painters, same shapes/colours)
on the v2 video clips blow_kiss, heart_hands, peace_sign_pose, hip_pop_wink, tracking the body per frame.

Tracking: the effect anchor is defined on the old key still (_work/allure/keys/<key>.png, same anchors as the
old clip). Per v2 frame it is predicted from the head crown (old offset to the crown, scaled 0.54/old scale) and,
for hand-anchored effects, refined by template matching a patch of the old key around the hand
(cv2.TM_CCOEFF_NORMED on gray composites, +-40 px around the prediction; falls back to the crown prediction
when the match is weak), then 3-tap smoothed. Effects only appear in the frames where she holds the pose
(windows below, from the keyframe timing of the videos). Un-effected frames are kept in <clip>/_base_frames/
so the pass is repeatable. usage: allure_fx_v2.py [clips...]"""
import sys, os, glob, json, shutil
import numpy as np, cv2
from PIL import Image
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import build_clip as bc
import os as _o, sys as _s; _s.path.insert(0, _o.path.dirname(_o.path.abspath(__file__))); import paths
sys.path.insert(0, paths.V1_TOOLS)
from icons import Pen, ROSE, MAG, ICE, WHITE
V2 = bc.ROOT; K = f'{paths.V1}/_work/allure/keys'
INFO = json.load(open(f'{K}/keys.json'))
def heart(p, x, y, r, col=ROSE, a=1.0):  # verbatim shape from build_allure.py
    t = np.linspace(0, 2 * np.pi, 40); hx = 16 * np.sin(t) ** 3; hy = -(13 * np.cos(t) - 5 * np.cos(2 * t) - 2 * np.cos(3 * t) - np.cos(4 * t))
    p.poly([(x + u * r / 16, y + v * r / 16) for u, v in zip(hx, hy)], col, a, outline=(58, 18, 78), ow=1.2)
def crown(a):
    al = a[..., 3] > 128; ys = np.where(al.sum(1) >= 18)[0]; y0 = ys.min(); xs = np.where(al[y0:y0 + 14].any(0))[0]
    return np.array([xs.mean(), float(y0)])
def gray(a):
    al = a[..., 3:4].astype(np.float32) / 255
    return cv2.cvtColor((a[..., :3] * al + 128 * (1 - al)).astype(np.uint8), cv2.COLOR_RGB2GRAY)
# clip -> key still, anchor fn(anchors) -> (x,y) on the old key, hand template?, active window (frame idx), painter(p, x, y, k)
def A(n): return {k: list(map(float, v)) for k, v in INFO[n]['anchors'].items()}
def p_hip(p, x, y, k):
    p.sparkle(x, y, 4.5 if k % 2 == 0 else 3, ROSE); p.sparkle(x + 9, y + 9, 2.5, WHITE)
def p_peace(p, x, y, k):
    p.sparkle(x, y, 4 + (k % 2) * 1.5, ICE); p.sparkle(x - 8, y + 12, 2.5, WHITE); p.sparkle(x + 4, y - 14, 3, ROSE)
def p_kiss(p, lx, ly, k):  # x start +14 px vs old so the first heart starts just off her cheek, not on her face
    for j in range(3):
        t = k - 5 - j * 1.3
        if t < 0: continue
        x = lx + 26 + 16 * t + 5 * np.sin(t * 1.7 + j); y = ly - 6 - 9 * t - 4 * j; a = max(0, 1 - t / 5.5); r = 4 + 2.2 * t
        if a > 0 and x < 392 and y > 6: heart(p, x, y, r, [ROSE, MAG, ROSE][j], a)
def p_hh(p, hx, hy, k):
    for j in range(3):
        t = k - 4 - j * 1.5
        if t < 0: continue
        x = hx + 40 + 9 * t + 4 * j + 3 * np.sin(t + j); y = hy - 6 - 11 * t - 5 * j; a = max(0, 1 - t / 4.5)
        if a > 0 and y > 6: heart(p, x, y, 4 + 1.5 * t, [ROSE, MAG, ROSE][j], a)
SPEC = {
 'hip_pop_wink': dict(key='hip_pop_wink_01', anchor=lambda a: (a['pigtail_right_point'][0] + 4, a['pigtail_right_point'][1] - 12),
                      hand=False, frames=range(9, 18), k=lambda i: i, paint=p_hip, point='sparkle_point'),
 'peace_sign_pose': dict(key='vsign_01', anchor=lambda a: (a['pigtail_left_point'][0] - 20, a['pigtail_left_point'][1] - 6),
                         hand=True, frames=range(9, 18), k=lambda i: i, paint=p_peace, point='vsign_point'),
 'blow_kiss': dict(key='blow_kiss_01', anchor=lambda a: ((a['pigtail_left_point'][0] + a['pigtail_right_point'][0]) / 2 - 6, a['pigtail_left_point'][1] + 14),
                   hand=True, frames=range(12, 24), k=lambda i: 5 + (i - 12) * 0.6, paint=p_kiss, point='kiss_origin'),
 'heart_hands': dict(key='heart_hands_01', anchor=lambda a: (a['chest_point'][0], a['chest_point'][1] - 14),
                     hand=True, frames=range(11, 24), k=lambda i: 4 + (i - 11) * 0.5, paint=p_hh, point='heart_origin'),
}
def run(name):
    S = SPEC[name]; d = f'{V2}/{name}'; bd = f'{d}/_base_frames'
    if not os.path.isdir(bd):
        os.makedirs(bd); [shutil.copy(f, bd) for f in glob.glob(f'{d}/frames/*.png')]
    fr = [np.asarray(Image.open(f).convert('RGBA')) for f in sorted(glob.glob(f'{bd}/*.png'))]
    js = json.load(open(f'{d}/{name}.json'))
    key = np.asarray(Image.open(f'{K}/{S["key"]}.png').convert('RGBA')); ks = INFO[S['key']].get('scale', 0.53)
    s = js['scale'] / ks; ax, ay = S['anchor'](A(S['key'])); kc = crown(key); off = (np.array([ax, ay]) - kc) * s
    tmpl = None
    if S['hand']:
        r = 26; x0, y0 = int(ax - r), int(ay - r); tmpl = gray(key)[max(0, y0):y0 + 2 * r, max(0, x0):x0 + 2 * r]
        if s != 1: tmpl = cv2.resize(tmpl, None, fx=s, fy=s)
    pts, how = [], []
    for a in fr:
        pr = crown(a) + off
        if tmpl is not None:
            g = gray(a); th, tw = tmpl.shape; R = 40
            X0, Y0 = int(max(0, pr[0] - tw / 2 - R)), int(max(0, pr[1] - th / 2 - R))
            win = g[Y0:int(pr[1] + th / 2 + R), X0:int(pr[0] + tw / 2 + R)]
            if win.shape[0] > th and win.shape[1] > tw:
                res = cv2.matchTemplate(win, tmpl, cv2.TM_CCOEFF_NORMED); _, mv, _, ml = cv2.minMaxLoc(res)
                if mv > 0.55: pr = np.array([X0 + ml[0] + tw / 2, Y0 + ml[1] + th / 2]); how.append(round(float(mv), 2))
                else: how.append('crown')
        pts.append(pr)
    P = np.array(pts); P = np.vstack([P[:1], P, P[-1:]]); P = (P[:-2] + 2 * P[1:-1] + P[2:]) / 4
    out = []
    for i, a in enumerate(fr):
        im = Image.fromarray(a.copy())
        if i in S['frames']:
            p = Pen(); S['paint'](p, P[i][0], P[i][1], S['k'](i)); im.alpha_composite(p.render())
        out.append(im)
    js.setdefault('frame_points', {})[S['point']] = [[int(round(x)), int(round(y))] for x, y in P]
    js[S['point']] = js['frame_points'][S['point']][S['frames'][0]]
    js['effects_overlay'] = {'painter': 'generated/_tools/build_allure.py shapes (heart / Pen.sparkle), redrawn by _tools/allure_fx_v2.py',
                             'frames': list(S['frames']), 'tracking': 'head crown + hand template match' if S['hand'] else 'head crown',
                             'match_scores': how}
    for k in ('frame_count', 'canvas_compliant', 'bbox_per_frame'): js.pop(k, None)
    m = bc.write_clip(name, out, js['fps'], 'loop' if js.get('loop') else 'once', js.get('body_anchor_x', 200), js['scale'], js, d, name)
    print(name, 'fx frames', list(S['frames'])[0], '-', list(S['frames'])[-1], 'matches', how[list(S['frames'])[0]:list(S['frames'])[-1] + 1] if how else 'crown', 'compliant', m['canvas_compliant'])
if __name__ == '__main__':
    for n in sys.argv[1:] or list(SPEC): run(n)
