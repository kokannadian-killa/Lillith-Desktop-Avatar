#!/usr/bin/env python3
"""Versioned polish rebuild (see polish_v1.json / POLISH_V1.md).
Rebuilds only the listed clips from their shipped generated_v2 definition plus the overrides, into
generated_v2/_polish_v1/<clip>/. Never writes generated_v2/<clip>/, the manifest or _matte_v2.
usage: polish_v1.py [clip ...] [--dry]"""
import sys, os, json, subprocess, shutil, glob
import numpy as np
from PIL import Image
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE)
import paths
V2 = paths.V2; DEFS = json.load(open(os.path.join(HERE, 'polish_v1.json')))
OUT = os.path.join(V2, DEFS['out_root'])
want = [a for a in sys.argv[1:] if not a.startswith('--')]; dry = '--dry' in sys.argv
def src_json(c):
    n = os.path.basename(c); d = os.path.join(V2, c); return d, n, json.load(open(os.path.join(d, f'{n}.json')))
def tmp_frames(video): return f'/tmp/lillith_frames/{os.path.splitext(os.path.basename(video))[0]}'
def finish(n, orig, entry, changes):
    od = os.path.join(OUT, n); p = os.path.join(od, f'{n}.json'); js = json.load(open(p))
    for k, v in orig.items():
        if k not in js: js[k] = v
    js['notes'] = orig.get('notes', '')
    js['polish_v1'] = {'why': entry['why'], 'changes': changes, 'replaces': f"generated_v2/{entry['clip']}"}
    json.dump(js, open(p, 'w'), indent=2)
def shift(a, dy, dx):
    a = np.roll(np.roll(a, dy, 0), dx, 1)
    if dy > 0: a[:dy] = 0
    if dy < 0: a[dy:] = 0
    if dx > 0: a[:, :dx] = 0
    if dx < 0: a[:, dx:] = 0
    a[393:] = 0; return a
def register(d, n, r, first, step):
    """Integer-px shift that puts a shared source frame exactly where the shipped clip has it (the global ground
    line is a percentile over the sampled frames, so a re-cut can land 1 px off). Returns [dy, dx]."""
    import qa_all
    s0 = max(first, r['first']); io, inew = (s0 - r['first']) // step, (s0 - first) // step
    od = os.path.join(OUT, n); nf = sorted(glob.glob(f'{od}/frames/*.png'))
    A = np.asarray(Image.open(sorted(glob.glob(f'{d}/frames/*.png'))[io]).convert('RGBA'), np.int16)
    B = np.asarray(Image.open(nf[inew]).convert('RGBA'), np.int16)
    best = min((np.abs(shift(B, dy, dx) - A).mean(), dy, dx) for dy in range(-3, 4) for dx in range(-3, 4))
    if (best[1], best[2]) == (0, 0): return [0, 0]
    fr = [shift(np.asarray(Image.open(f).convert('RGBA')).copy(), best[1], best[2]) for f in nf]
    qa_all.rewrite(od, n, fr, json.load(open(f'{od}/{n}.json'))); return [best[1], best[2]]
def run(cmd):
    print('+', ' '.join(cmd), flush=True)
    if not dry: subprocess.run(cmd, check=True)
for e in DEFS['clips']:
    d, n, j = src_json(e['clip'])
    if want and n not in want: continue
    assert not j.get('frame_durations_ms'), f'{n}: has frame_durations_ms; frame count change needs a timing decision'
    r = j['source_frame_range']; first = e.get('first', r['first']); last = e.get('last', r['last']); step = r['step']
    scale = e.get('scale', j['scale']); video = os.path.normpath(os.path.join(d, j['source_files'][0]))
    snap = j.get('baseline_snap', '')
    cmd = [sys.executable, os.path.join(HERE, 'build_clip.py'), '--clip', n, '--video', video, '--start', str(first),
           '--stop', str(last + 1), '--step', str(step), '--fps', str(j['fps']), '--anchor-x', str(j.get('body_anchor_x', 200)),
           '--scale', str(scale), '--mode', 'loop' if j['loop'] else 'once', '--center', j.get('centering', 'fixed'),
           '--facing', j.get('facing', 'front'), '--out-dir', os.path.join(OUT, n)]
    if (first, last) != (r['first'], r['last']): cmd += ['--ground-from', f"{r['first']}:{r['last'] + 1}"]
    if snap == 'per_frame': cmd.append('--snap')
    elif snap.startswith('frames within'): cmd += ['--snap-thresh', snap.split()[2].rstrip('px')]
    if not j.get('horn_fix'): cmd.append('--no-horn-fix')
    had = os.path.isdir(tmp_frames(video))
    run(cmd); sh = [0, 0] if dry or scale != j['scale'] else register(d, n, r, first, step)
    run([sys.executable, os.path.join(HERE, 'qa_all.py'), '--fix', f"{DEFS['out_root']}/{n}"])
    if not had and not dry: shutil.rmtree(tmp_frames(video), ignore_errors=True)
    ch = {k: [old, new] for k, old, new in [('first', r['first'], first), ('last', r['last'], last), ('scale', j['scale'], scale)] if old != new}
    if sh != [0, 0]: ch['placement_shift_px_dy_dx'] = sh
    if not dry: finish(n, j, e, ch)
for e in DEFS['transitions']:
    d, n, j = src_json(e['clip'])
    if want and n not in want: continue
    video = os.path.normpath(os.path.join(d, j['source_files'][0])); a, b = j['from_clip'], j['to_clip']
    had = os.path.isdir(tmp_frames(video))
    run([sys.executable, os.path.join(HERE, 'build_transition.py'), n, video, a, str(j['source_frames'][a]), b, str(j['source_frames'][b]),
         str(len(j['source_video_frames'])), str(j['fps']), f"--end={e['end']}", f"--out-dir={os.path.join(OUT, n)}"])
    run([sys.executable, os.path.join(HERE, 'qa_all.py'), '--fix', f"{DEFS['out_root']}/{n}"])
    if not had and not dry: shutil.rmtree(tmp_frames(video), ignore_errors=True)
    if not dry: finish(n, j, e, {'source_video_frames': [j['source_video_frames'], json.load(open(os.path.join(OUT, n, f'{n}.json')))['source_video_frames']]})
