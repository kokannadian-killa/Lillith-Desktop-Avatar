"""Slow rushed clips: per-clip fps (and frame_durations_ms) in manifest.json and <clip>.json.
f = clamp(motion_per_s / CAP, 1, fps / FLOOR). Locomotion with root motion / fixed glide is left alone (foot slide).
Writes /workspace/verify/clipclean/retime_table.{json,md}."""
import os as _os
_ROOT = _os.environ.get('CC_REPO', _os.path.abspath(_os.path.join(_os.path.dirname(__file__), '..', '..', '..')))
_OUT = _os.environ.get('CC_OUT', '/workspace/verify/clipclean')
import json, os, math
G = _ROOT + '/assets/generated'; R = _OUT
CAP = 45.0; FLOOR = 8.0
SKIP_PREFIX = ('walk_', 'run_', 'strut_')
def skip(n): return n.startswith(SKIP_PREFIX)
m = json.load(open(f'{G}/manifest.json'))
rows = []; plan = {}
for e in m['animations']:
    if e.get('alias_of'): continue
    d = e['sheet'].split('/')[0]
    t = json.load(open(f'{R}/audit_before/{d}.json'))['timing']
    fps = float(e['fps']); n = e['frame_count']; fdm = e.get('frame_durations_ms')
    dur = sum(fdm) / 1000 if fdm else n / fps
    eff = n / dur
    dps = t['diff_per_s']
    if skip(e['name']):
        f = 1.0; why = 'locomotion: root_motion / fixed glide speed (slowing = foot slide)'
    else:
        f = min(max(dps / CAP, 1.0), max(eff / FLOOR, 1.0))
        why = '' if f > 1.001 else 'already unhurried'
    new_fps = fps; new_fdm = fdm
    if f > 1.001:
        new_eff = round(eff / f * 2) / 2   # 0.5 fps steps
        new_eff = max(new_eff, min(FLOOR, eff))
        f = eff / new_eff
        if fdm:
            new_fdm = [int(round(x * f)) for x in fdm]
            new_fps = round(fps / f, 2)
        else:
            new_fps = new_eff
        why = why or f'motion {dps:.0f}/s > {CAP:.0f}/s'
    new_dur = sum(new_fdm) / 1000 if new_fdm else n / new_fps
    plan[d] = (new_fps, new_fdm)
    rows.append(dict(clip=e['name'], frames=n, loop=e.get('loop'), fps_before=fps, fps_after=new_fps,
                     eff_fps_before=round(eff, 2), eff_fps_after=round(n / new_dur, 2), dur_before=round(dur, 3),
                     dur_after=round(new_dur, 3), slow_factor=round(new_dur / dur, 3), motion_per_s_before=round(dps, 1),
                     motion_per_s_after=round(dps * dur / new_dur, 1), note=why))
# apply: manifest entries (incl. aliases follow their sheet) and per-clip json
for e in m['animations']:
    d = e['sheet'].split('/')[0]
    if d in plan:
        nf, nd = plan[d]; e['fps'] = nf
        if nd is not None: e['frame_durations_ms'] = nd
open(f'{G}/manifest.json', 'w').write(json.dumps(m, indent=1) + '\n')
for d, (nf, nd) in plan.items():
    jp = f'{G}/{d}/{d}.json'; j = json.load(open(jp))
    j['fps'] = nf
    if nd is not None: j['frame_durations_ms'] = nd
    open(jp, 'w').write(json.dumps(j, indent=2))
json.dump(rows, open(f'{R}/retime_table.json', 'w'), indent=1)
L = ['# Clip timing before/after (authored; the engine additionally applies playback_speed, default 0.7)', '',
     '| clip | frames | loop | fps before | fps after | duration before (s) | duration after (s) | slow x | motion/s before | after | note |', '|' + '---|' * 11]
for r in sorted(rows, key=lambda r: -r['slow_factor']):
    L.append(f"| {r['clip']} | {r['frames']} | {r['loop']} | {r['eff_fps_before']} | {r['eff_fps_after']} | {r['dur_before']} | {r['dur_after']} | {r['slow_factor']} | {r['motion_per_s_before']} | {r['motion_per_s_after']} | {r['note']} |")
ch = [r for r in rows if r['slow_factor'] > 1.001]
L += ['', f'{len(ch)} of {len(rows)} unique clips slowed.']
open(f'{R}/retime_table.md', 'w').write('\n'.join(L) + '\n')
print(len(ch), 'slowed of', len(rows))
