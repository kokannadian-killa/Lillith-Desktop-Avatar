#!/usr/bin/env python3
"""make_clip.py: Lillith seductive-clip generator kit, one command per step.

  make_clip.py list                         recipes in prompts/recipes/
  make_clip.py prompts <recipe>             filled still + video prompts (also written to out/<clip>/prompts.md)
  make_clip.py build <recipe> --video RAW.mp4 [--clip NAME] [--take a] [--draft]
                                            raw I2V take -> 400x400 clip + preview GIFs + automatic checks
  make_clip.py check <clipdir>              re-run the automatic checks on a built clip dir

build runs tools/video-pipeline/new_pose.py --to finalize (ingest -> build_clip.py -> matte_v2 -> finalize)
into a scratch tree (default /workspace/lillith/gen_work/out, override with GENKIT_OUT), so nothing in the
repo's assets/ or the shared generated_v2 tree is touched. To ship a clip that passed, rerun new_pose.py
with the printed command (adds copy/register/pack/import into assets/generated/).

Exit code: 0 = all checks pass, 2 = a check failed (see out/<clip>/check_report.md).
"""
import argparse, glob, json, os, subprocess, sys
import numpy as np
from PIL import Image
from scipy import ndimage

KIT = os.path.dirname(os.path.realpath(__file__))
REPO = os.path.abspath(os.path.join(KIT, '..', '..'))
PIPE = os.path.join(REPO, 'tools', 'video-pipeline')
OUT = os.environ.get('GENKIT_OUT', '/workspace/lillith/gen_work/out')
RECIPES = os.path.join(KIT, 'prompts', 'recipes')

# slow-pacing window (matches the shipped allure clips: 12 fps cells, real-time or slower playback)
FPS_RANGE = (8.0, 12.0)
MIN_LOOP_S = 3.0
MAX_STEP_PX = 6.0     # p95 per-frame body-centroid travel on the 400px canvas
BASELINE = 392


def recipe(name):
    p = os.path.join(RECIPES, f'{name}.json')
    if not os.path.exists(p):
        sys.exit(f'no recipe {name}; have: {", ".join(list_recipes())}')
    return json.load(open(p))


def list_recipes():
    return sorted(os.path.splitext(f)[0] for f in os.listdir(RECIPES) if f.endswith('.json'))


def read(p):
    return '\n'.join(l for l in open(os.path.join(KIT, 'prompts', p)).read().splitlines() if not l.startswith('#')).strip()


def prompts(r):
    lock = read('character_lock.txt')
    avoid = read('avoid.txt')
    avoid_short = 'Avoid: ' + avoid.split('\n', 1)[1].replace('\n', ' ')
    still = read('still_template.txt').format(CHARACTER_LOCK=lock, PEAK_POSE=r['peak_pose'],
                                              EXPRESSION=r['expression'], AVOID=avoid)
    video = read('video_template.txt').format(MOTION=r['motion'], AVOID_SHORT=avoid_short,
                                              PEAK_AT_S=r['peak_at_s'], TAKE_S=r['take_s'])
    call = (f"GenerateVideo(image_path=BASE, last_frame_path=BASE, keyframes=[{{image_path: PEAK, "
            f"timestamp_s: {r['peak_at_s']}}}], duration={r['take_s']}, resolution='720p', "
            f"filename='{r['name']}_v3_a.mp4')")
    return still, video, call


def cmd_prompts(a):
    r = recipe(a.recipe)
    still, video, call = prompts(r)
    d = os.path.join(OUT, a.clip or r['name']); os.makedirs(d, exist_ok=True)
    md = (f"# {r['name']}\n\n## 1. Peak still (GenerateImage, 16:9, reference = base still)\n\n{still}\n\n"
          f"## 2. Motion (image-to-video)\n\n{video}\n\nCall: {call}\n")
    open(os.path.join(d, 'prompts.md'), 'w').write(md)
    print(md)


def frames_of(d):
    js = json.load(open(glob.glob(os.path.join(d, '*.json'))[0]))
    fs = sorted(glob.glob(os.path.join(d, 'frames', '*.png')))
    return js, [np.asarray(Image.open(f).convert('RGBA')) for f in fs]


def check(d, seated=None):
    js, F = frames_of(d)
    name = js['name']
    res = {}
    def put(k, ok, val): res[k] = {'pass': bool(ok), 'value': val}
    # canvas
    put('canvas_400x400_rgba', all(f.shape == (400, 400, 4) for f in F) and len(F) == js['frame_count'],
        f"{len(F)} frames, json frame_count {js['frame_count']}")
    # alpha cleanliness
    edge_px = below = white_edge = edges = specks = opaque_white_border = 0
    for f in F:
        al = f[..., 3]; solid = al > 0
        edge_px += int(solid[:3].sum() + solid[:, :3].sum() + solid[:, -3:].sum())
        below += int(solid[BASELINE + 1:].sum())
        e = solid & ~ndimage.binary_erosion(solid, iterations=2)
        edges += int(e.sum()); white_edge += int((e & (f[..., :3].min(-1) > 225)).sum())
        lab, n = ndimage.label(solid, structure=np.ones((3, 3)))
        if n > 1:
            sz = ndimage.sum(solid, lab, range(1, n + 1)); main = int(np.argmax(sz)) + 1
            near = ndimage.binary_dilation(lab == main, iterations=4)
            specks += sum(1 for i, s in enumerate(sz) if s < 30 and i + 1 != main and not (near & (lab == i + 1)).any())
        # leftover backdrop: semi/opaque near-white pixels in the outer 2px of the silhouette that form a band
        opaque_white_border += int((e & (al < 250) & (f[..., :3].min(-1) > 215)).sum())
    wr = white_edge / max(edges, 1)
    put('no_edge_pixels', edge_px == 0, f'{edge_px} alpha px within 3px of top/left/right edge')
    put('nothing_below_baseline', below == 0, f'{below} alpha px below y={BASELINE}')
    put('no_white_halo', wr <= 0.003 and opaque_white_border <= 20 * len(F),
        f'white edge ratio {wr:.4%}, semi-transparent white fringe px {opaque_white_border}')
    put('no_specks', specks == 0, f'{specks} isolated specks')
    # pacing
    fps = float(js['fps']); n = len(F); dur = n / fps
    sfr = js.get('source_frame_range', {})
    rate = sfr.get('step', 2) * fps / sfr.get('video_fps', 24) if sfr else None
    cents = []
    for f in F:
        ys, xs = np.nonzero(f[..., 3] > 128); cents.append((xs.mean(), ys.mean()))
    cents = np.array(cents); steps = np.hypot(*np.diff(cents, axis=0).T)
    p95 = float(np.percentile(steps, 95)) if len(steps) else 0.0
    put('fps_in_slow_range', FPS_RANGE[0] <= fps <= FPS_RANGE[1], f'{fps} fps')
    put('not_sped_up', rate is None or rate <= 1.0001, f'playback rate x{rate:.2f} of the source' if rate else 'n/a')
    put('long_enough', (not js.get('loop', True)) or dur >= MIN_LOOP_S, f'{n} frames = {dur:.2f} s')
    put('smooth_motion', p95 <= MAX_STEP_PX, f'p95 body travel {p95:.2f} px/frame (max {MAX_STEP_PX})')
    # baseline
    low = [int(np.nonzero((f[..., 3] > 128).any(1))[0].max()) for f in F]
    seated = js.get('genkit_seated', False) if seated is None else seated
    if seated:
        put('baseline', max(low) <= BASELINE, f'lowest px {min(low)}..{max(low)} (seated: only must stay <= {BASELINE})')
    else:
        off = [abs(l - BASELINE) for l in low]
        put('baseline', max(off) <= 1, f'lowest px per frame {min(low)}..{max(low)} (target {BASELINE} +-1)')
    # loop seam
    if js.get('loop', True) and n > 2:
        d_ = [np.abs(F[i + 1][..., 3].astype(int) - F[i][..., 3]).mean() for i in range(n - 1)]
        seam = np.abs(F[0][..., 3].astype(int) - F[-1][..., 3]).mean()
        put('loop_seam', seam <= 2.5 * np.median(d_) + 0.5, f'seam {seam:.2f} vs median step {np.median(d_):.2f}')
    ok = all(v['pass'] for v in res.values())
    rep = {'clip': name, 'dir': d, 'pass': ok, 'checks': res}
    json.dump(rep, open(os.path.join(d, 'check_report.json'), 'w'), indent=1)
    with open(os.path.join(d, 'check_report.md'), 'w') as fh:
        fh.write(f'# {name}: {"PASS" if ok else "FAIL"}\n\n')
        for k, v in res.items():
            fh.write(f"- {'PASS' if v['pass'] else 'FAIL'} {k}: {v['value']}\n")
    review_gif(d, name, F, fps)
    print(open(os.path.join(d, 'check_report.md')).read())
    return ok


def review_gif(d, name, F, fps):
    """Gray | black | checker side by side: white halos show on black, dark fringes on gray."""
    tiles = []
    chk = np.indices((400, 400)).sum(0) // 16 % 2
    chk = np.where(chk[..., None] == 1, 200, 255).astype(np.uint8).repeat(3, -1)
    for f in F:
        im = Image.fromarray(f, 'RGBA'); row = Image.new('RGB', (1200, 400))
        for i, bg in enumerate([Image.new('RGBA', (400, 400), (128, 128, 128, 255)),
                                Image.new('RGBA', (400, 400), (0, 0, 0, 255)),
                                Image.fromarray(chk).convert('RGBA')]):
            bg.alpha_composite(im); row.paste(bg.convert('RGB'), (400 * i, 0))
        tiles.append(row.resize((900, 300), Image.LANCZOS))
    tiles[0].save(os.path.join(d, f'{name}_check.gif'), save_all=True, append_images=tiles[1:],
                  duration=int(round(1000 / fps)), loop=0, optimize=True)


def cmd_build(a):
    r = recipe(a.recipe); clip = a.clip or r['name']
    env = dict(os.environ, LILLITH_V2=os.path.join(OUT, '_v2'), LILLITH_SRC_VIDEO=os.path.join(OUT, '_src'))
    nf = int(subprocess.run(['ffprobe', '-v', 'error', '-count_frames', '-select_streams', 'v:0', '-show_entries',
                             'stream=nb_read_frames', '-of', 'csv=p=0', a.video], capture_output=True, text=True).stdout.strip())
    stop = min(a.stop or r['stop'], nf - 1)  # last frame == first frame for loops -> dropped
    fin = 'build' if a.draft else r['finalize']
    cmd = [sys.executable, os.path.join(PIPE, 'new_pose.py'), clip, '--video', a.video, '--take', a.take,
           '--start', str(a.start or r['start']), '--stop', str(stop), '--step', str(r['step']), '--fps', str(r['fps']),
           '--mode', r['mode'], '--scale', str(r['scale']), '--anchor-x', str(r['anchor_x']), '--center', r['center'],
           '--finalize', fin, '--to', 'finalize', '--allure'] + [f'--build-arg={x}' for x in r.get('build_args', [])]
    print('+', ' '.join(cmd), flush=True)
    subprocess.run(cmd, check=True, env=env)
    d = os.path.join(env['LILLITH_V2'], clip)
    if fin == 'matte':  # matte output keeps the build json; make sure the dir is complete
        pass
    js_p = os.path.join(d, f'{clip}.json'); js = json.load(open(js_p))
    js['genkit_recipe'] = r['name']; js['genkit_seated'] = bool(r.get('seated'))
    json.dump(js, open(js_p, 'w'), indent=1)
    ok = check(d, r.get('seated'))
    print(f"preview: {d}/{clip}_preview.gif\nhalo check: {d}/{clip}_check.gif")
    ship = ' '.join(cmd[:2] + [clip, '--video', a.video, '--take', a.take] + cmd[7:cmd.index('--to')] + ['--allure'])
    print(f"\nship it (run from the repo, writes assets/generated/{clip}/):\n  {ship.replace('--allure --allure', '--allure')}")
    sys.exit(0 if ok else 2)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sp = ap.add_subparsers(dest='cmd', required=True)
    sp.add_parser('list')
    p = sp.add_parser('prompts'); p.add_argument('recipe'); p.add_argument('--clip')
    b = sp.add_parser('build'); b.add_argument('recipe'); b.add_argument('--video', required=True)
    b.add_argument('--clip'); b.add_argument('--take', default='a'); b.add_argument('--draft', action='store_true',
                   help='skip matte_v2 (fast white-key draft)')
    b.add_argument('--start', type=int); b.add_argument('--stop', type=int)
    c = sp.add_parser('check'); c.add_argument('dir'); c.add_argument('--seated', action='store_true', default=None)
    a = ap.parse_args()
    if a.cmd == 'list':
        for n in list_recipes():
            print(f"{n:22s} {recipe(n)['peak_pose'][:80]}")
    elif a.cmd == 'prompts':
        cmd_prompts(a)
    elif a.cmd == 'build':
        cmd_build(a)
    else:
        sys.exit(0 if check(a.dir, a.seated) else 2)


if __name__ == '__main__':
    main()
