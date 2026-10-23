#!/usr/bin/env python3
"""new_pose.py: one command from a raw Grok Imagine take to a registered,
MP4-sourced pose in the repo.

  new_pose.py <clip> --video RAW.mp4 --start N --stop M [build options]
              [--finalize keyframed|matte|build] [--allure] [--tag T ...]
              [--from STEP] [--dry-run]

Steps (each one is a separate tool you can also run by hand):
  ingest    copy RAW.mp4 to $LILLITH_SRC_VIDEO/<clip>/<clip>_v3_<take>.mp4 (never overwrites)
  build     build_clip.py: frames start..stop step N -> 400x400 cells, white key,
            placement, horn fix -> $LILLITH_V2/<clip>/
  matte     matte_v2.py: isnet-anime + unmix + temporal matte -> $LILLITH_V2/_matte_v2/<clip>/
  finalize  keyframed: finalize_keyframed_loop.py (zoom-creep fix + baseline snap)
            matte:     take the matte_v2 output as is
            build:     keep the build_clip output (skip matte; quick drafts)
  copy      <clip>.json, sheet, preview -> <repo>/assets/generated/<clip>/
  register  add / update the assets/generated/manifest.json entry
  pack      pack_mp4.py: sheet -> <clip>.mp4 (colour | alpha, crf 16) + halo metrics,
            sets entry.video
  import    cargo run -p import-manifest -- --tag canonical --check (probes + decodes
            every MP4, compares alpha with the PNG)

Timing stays in JSON (fps, loop/once, frame_durations_ms); the MP4 holds one
frame per cell. Content rules: the locked canonical style; clothed, non-explicit.
Paths: tools/video-pipeline/paths.py (LILLITH_ASSETS, LILLITH_V2, LILLITH_SRC_VIDEO).
"""
import argparse, json, os, shutil, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import paths  # noqa: E402

REPO = os.path.abspath(os.path.join(HERE, '..', '..'))
STEPS = ['ingest', 'build', 'matte', 'finalize', 'copy', 'register', 'pack', 'import']
PY = sys.executable


def run(cmd, dry, cwd=None):
    print('+', ' '.join(map(str, cmd)), flush=True)
    if not dry:
        subprocess.run(list(map(str, cmd)), check=True, cwd=cwd)


def register(manifest, clip, js, allure, tags):
    m = json.load(open(manifest))
    e = next((a for a in m['animations'] if a['name'] == clip), None)
    new = e is None
    if new:
        e = {'name': clip}
        m['animations'].append(e)
    for k in ('frame_count', 'frame_width', 'frame_height', 'fps', 'loop', 'baseline_y', 'anchor',
              'scale', 'body_anchor_x', 'facing', 'frame_durations_ms'):
        if k in js:
            e[k] = js[k]
    e.update({'canvas_compliant': True, 'placeholder': False, 'source': 'grok-imagine-video',
              'dir': f'{clip}/', 'sheet': f'{clip}/{clip}_sheet.png', 'manifest': f'{clip}/{clip}.json',
              'preview': f'{clip}/{clip}_preview.gif'})
    e.pop('frames', None)  # frames/ dirs are not copied: the sheet + MP4 are the sources
    e['tags'] = sorted(set(e.get('tags', []) + ['baked_motion'] + list(tags)))
    if allure:
        e['allure'] = True
    with open(manifest, 'w') as f:
        f.write(json.dumps(m, indent=1) + '\n')
    print(('added' if new else 'updated'), clip, 'in', manifest)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('clip')
    ap.add_argument('--video', help='raw take (required for ingest/build)')
    ap.add_argument('--take', default='a', help='take letter for the source copy (default a)')
    ap.add_argument('--start', type=int); ap.add_argument('--stop', type=int)
    ap.add_argument('--step', type=int, default=2); ap.add_argument('--fps', type=float, default=12)
    ap.add_argument('--mode', choices=['loop', 'once'], default='loop')
    ap.add_argument('--anchor-x', type=int, default=200); ap.add_argument('--scale', type=float, default=0.54)
    ap.add_argument('--center', choices=['fixed', 'body', 'body_trend'], default='fixed')
    ap.add_argument('--build-arg', action='append', default=[], help='extra build_clip.py arg (repeatable)')
    ap.add_argument('--finalize', choices=['keyframed', 'matte', 'build'], default='keyframed')
    ap.add_argument('--allure', action='store_true', help='mark as an allure pose (Flirty / Allure menu)')
    ap.add_argument('--tag', action='append', default=[])
    ap.add_argument('--crf', type=int, default=16)
    ap.add_argument('--repo', default=REPO)
    ap.add_argument('--from', dest='start_step', choices=STEPS, default='ingest')
    ap.add_argument('--to', dest='stop_step', choices=STEPS, default='import')
    ap.add_argument('--dry-run', action='store_true')
    a = ap.parse_args()
    c, dry = a.clip, a.dry_run
    steps = STEPS[STEPS.index(a.start_step):STEPS.index(a.stop_step) + 1]
    src = os.path.join(paths.SRC_VIDEO, c, f'{c}_v3_{a.take}.mp4')
    v2 = os.path.join(paths.V2, c)
    gen = os.path.join(a.repo, 'assets', 'generated')
    dst = os.path.join(gen, c)
    manifest = os.path.join(gen, 'manifest.json')

    if 'ingest' in steps:
        if not a.video:
            ap.error('--video is required for ingest')
        print(f'+ copy {a.video} -> {src}')
        if not dry:
            os.makedirs(os.path.dirname(src), exist_ok=True)
            if not os.path.exists(src):
                shutil.copy2(a.video, src)
    if 'build' in steps:
        if a.start is None or a.stop is None:
            ap.error('--start/--stop are required for build (find_loops.py <framesdir> helps)')
        run([PY, f'{HERE}/build_clip.py', '--clip', c, '--video', src, '--start', a.start, '--stop', a.stop,
             '--step', a.step, '--fps', a.fps, '--mode', a.mode, '--anchor-x', a.anchor_x, '--scale', a.scale,
             '--center', a.center, *a.build_arg], dry)
    if 'matte' in steps and a.finalize != 'build':
        run([PY, f'{HERE}/matte_v2.py', c], dry)
    if 'finalize' in steps:
        if a.finalize == 'keyframed':
            run([PY, f'{HERE}/finalize_keyframed_loop.py', c], dry)
        elif a.finalize == 'matte':
            m = os.path.join(paths.V2, '_matte_v2', c)
            print(f'+ copy matte output {m} -> {v2}')
            if not dry:
                shutil.copytree(m, v2, dirs_exist_ok=True)
    if 'copy' in steps:
        print(f'+ copy {v2}/{{json,sheet,preview}} -> {dst}')
        if not dry:
            os.makedirs(dst, exist_ok=True)
            for f in (f'{c}.json', f'{c}_sheet.png', f'{c}_preview.gif'):
                if os.path.exists(os.path.join(v2, f)):
                    shutil.copy2(os.path.join(v2, f), os.path.join(dst, f))
    if 'register' in steps:
        print(f'+ register {c} in {manifest}')
        if not dry:
            register(manifest, c, json.load(open(os.path.join(dst, f'{c}.json'))), a.allure, a.tag)
    if 'pack' in steps:
        run([PY, f'{HERE}/pack_mp4.py', dst, '--crf', a.crf, '--manifest', manifest, '--force'], dry)
    if 'import' in steps:
        run(['cargo', 'run', '-q', '-p', 'import-manifest', '--', '--input', 'assets/generated/manifest.json',
             '--tag', 'canonical', '--check'], dry, cwd=a.repo)
    if not dry:
        print(f'\ndone: {c}. Review {dst}/{c}_preview.gif, then commit assets/generated/{c}/ + manifest.*'
              + ('' if a.allure else '\n(not an allure pose: add it to a catalog.rs list if the menu category matters)'))


if __name__ == '__main__':
    main()
