import os as _os
_ROOT = _os.environ.get('CC_REPO', _os.path.abspath(_os.path.join(_os.path.dirname(__file__), '..', '..', '..')))
_OUT = _os.environ.get('CC_OUT', '/workspace/verify/clipclean')
import json, os, sys, glob
R = _OUT
tag = sys.argv[1]; cmp = sys.argv[2] if len(sys.argv) > 2 else None
def load(t):
    return {os.path.basename(p)[:-5]: json.load(open(p)) for p in glob.glob(f'{R}/audit_{t}/*.json') if not os.path.basename(p).startswith('_')}
A = load(tag); B = load(cmp) if cmp else {}
keys = ['leak', 'white_edge_opaque', 'white_edge', 'thick_semi', 'see_through', 'small_holes', 'stray', 'far_opaque', 'floor_semi']
rows = sorted(A.values(), key=lambda d: -max(d['png']['severity_mean'], d['mp4']['severity_mean']))
L = [f'# Clip alpha/white audit — {tag}' + (f' (vs {cmp})' if cmp else ''), '',
     'Counts are pixel totals summed over all frames (PNG sheet | MP4 decode). leak = near-white background pixels connected to the key; '
     'white_edge(_opaque) = near-white pixels within 2.5 px of transparency; see_through = semi-transparent pixels >=3 px inside her; '
     'thick_semi = semi-transparent band >2 px thick; stray/far_opaque = detached islands (not counted on overlay FX clips).', '',
     '| # | clip | frames | sev PNG | sev MP4 | ' + ' | '.join(keys) + ' | frames w/ leak |' + (' before sev PNG/MP4 |' if cmp else ''),
     '|' + '---|' * (6 + len(keys) + (2 if cmp else 1))]
tot = {k: 0 for k in keys}; n_flag = 0
for i, d in enumerate(rows, 1):
    p, m = d['png'], d['mp4']
    for k in keys: tot[k] += p[k]
    n_flag += (p['leak'] + p['white_edge_opaque']) > 0
    s = f"| {i} | {d['clip']} | {d['frames']} | {p['severity_mean']:.1f} | {m['severity_mean']:.1f} | " + ' | '.join(f"{p[k]}/{m[k]}" for k in keys) + f" | {p['frames_with_leak']} |"
    if cmp and d['clip'] in B:
        b = B[d['clip']]; s += f" {b['png']['severity_mean']:.1f}/{b['mp4']['severity_mean']:.1f} |"
    L.append(s)
L += ['', f'Clips with any white leak or opaque white edge (PNG): {n_flag}/{len(rows)}', 'Totals (PNG): ' + ', '.join(f'{k} {v}' for k, v in tot.items())]
open(f'{R}/report_{tag}.md', 'w').write('\n'.join(L) + '\n')
json.dump({'tag': tag, 'n': len(rows), 'flagged': n_flag, 'totals_png': tot,
           'ranked': [[d['clip'], d['png']['severity_mean'], d['mp4']['severity_mean']] for d in rows]}, open(f'{R}/report_{tag}.json', 'w'), indent=1)
print(f'{len(rows)} clips, flagged {n_flag}, totals {tot}'); print([r['clip'] for r in rows[:12]])
