#!/usr/bin/env python3
"""Generic video -> engine clip builder for Lillith generated_v2.

  build_clip.py --clip walk_right --video ../../source/video/walk_right/walk_right_v3_b.mp4 \
      --start 9 --stop 37 --step 2 --fps 12 --anchor-x 200 --scale 0.54 --mode loop \
      --center body_trend --compare-old walk_right [--mirror-to walk_left --mirror-anchor-x 200]

Frame numbers are 1-based video frames (ffmpeg f_%03d), stop is exclusive.
Steps: extract -> key (white flood + isnet-anime matte, key_white.key_combined)
 -> scale (fixed, same as old set: head size matched) -> place (baseline 392,
 horizontal centering mode) -> horn hue lock (horn_fix, ref = processed video
 frame 1 = the source still) -> sheet / frames / json / gif / on_gray / compare.

--center:
  fixed       : source x --cx-src goes to --anchor-x for every frame (idle)
  body        : per-frame body point (avg of hip centre and head centre) -> anchor-x
  body_trend  : linear trend of the body point removed (keeps natural sway, kills travel)
"""
import argparse, json, os, subprocess, sys, glob
import numpy as np
from PIL import Image, ImageDraw
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from key_white import key_combined
from horn_fix import fix_frames

import os as _o, sys as _s; _s.path.insert(0, _o.path.dirname(_o.path.abspath(__file__))); import paths
ROOT = paths.V2
OLD = paths.V1
os.environ.setdefault('U2NET_HOME', paths.U2NET)

def extract(video):
    d = f'/tmp/lillith_frames/{os.path.splitext(os.path.basename(video))[0]}'
    if not glob.glob(f'{d}/f_*.png'):
        os.makedirs(d, exist_ok=True)
        subprocess.run(['ffmpeg', '-v', 'error', '-y', '-i', video, f'{d}/f_%03d.png'], check=True)
    return d

def body_x(alpha):
    """Body point in source px: average of hip-band centre and head-band centre,
    using the longest opaque run per row (ignores the thin tail / stray limbs)."""
    a = alpha > 128
    ys = np.where(a.any(1))[0]; top, bot = ys.min(), ys.max(); H = bot - top
    def band(f0, f1):
        cs = []
        for y in range(int(top + f0 * H), int(top + f1 * H)):
            row = a[y]; xs = np.where(row)[0]
            if len(xs) == 0: continue
            # longest run
            br = np.split(xs, np.where(np.diff(xs) > 1)[0] + 1)
            r = max(br, key=len); cs.append((r[0] + r[-1]) / 2)
        return float(np.median(cs)) if cs else float(np.mean(np.where(a)[1]))
    return (band(0.45, 0.55) + band(0.06, 0.16)) / 2

def place(rgba, S, tx, ty):
    im = Image.fromarray(rgba, 'RGBA')
    im = im.resize((round(im.width * S), round(im.height * S)), Image.LANCZOS)
    cv = Image.new('RGBA', (400, 400), (0, 0, 0, 0))
    cv.paste(im, (int(round(tx)), int(np.floor(ty))), im)
    arr = np.asarray(cv).copy()
    arr[393:, :, 3] = 0; arr[393:, :, :3] = 0
    return Image.fromarray(arr, 'RGBA')

def bbox(f):
    al = np.asarray(f)[..., 3]; ys, xs = np.where(al > 16)
    return [int(xs.min()), int(xs.max()), int(ys.min()), int(ys.max())]

def write_clip(name, frames, fps, mode, anchor_x, scale, meta_extra, outdir, compare_old=None):
    os.makedirs(f'{outdir}/frames', exist_ok=True)
    for p in glob.glob(f'{outdir}/frames/*.png'): os.remove(p)
    for n, f in enumerate(frames): f.save(f'{outdir}/frames/{name}_{n:02d}.png')
    N = len(frames)
    sheet = Image.new('RGBA', (400 * N, 400), (0, 0, 0, 0))
    for n, f in enumerate(frames): sheet.paste(f, (400 * n, 0))
    sheet.save(f'{outdir}/{name}_sheet.png')
    g = Image.new('RGBA', sheet.size, (128, 128, 128, 255)); g.alpha_composite(sheet)
    g.convert('RGB').save(f'{outdir}/{name}_sheet_on_gray.png')
    gif = []
    for f in frames:
        b = Image.new('RGBA', (400, 400), (128, 128, 128, 255)); b.alpha_composite(f); gif.append(b.convert('RGB'))
    gif[0].save(f'{outdir}/{name}_preview.gif', save_all=True, append_images=gif[1:],
                duration=int(round(1000 / fps)), loop=0 if mode == 'loop' else 1)
    bbs = [bbox(f) for f in frames]
    meta = {"name": name, "placeholder": False, "frame_count": N, "frame_width": 400, "frame_height": 400,
            "canvas_w": 400, "canvas_h": 400, "layout": "horizontal_strip", "sheet": f"{name}_sheet.png",
            "frames_dir": "frames/", "fps": fps, "loop": mode == 'loop', "baseline_y": 392,
            "anchor": [anchor_x, 392], "style": "canonical_ko (thigh-highs + shorts)",
            "source": "grok-imagine-video", "scale": scale, "body_anchor_x": anchor_x}
    meta.update(meta_extra)
    meta["canvas_compliant"] = all(b[0] >= 3 and b[1] <= 396 and b[3] <= 392 for b in bbs)
    meta["bbox_per_frame"] = bbs
    json.dump(meta, open(f'{outdir}/{name}.json', 'w'), indent=2)
    if compare_old:
        make_compare(compare_old, name, frames, fps, f'{outdir}/compare.png')
    return meta

def make_compare(old_clip, name, frames, fps, path, c=200, per_row=18):
    old = [Image.open(p).convert('RGBA') for p in sorted(glob.glob(f'{OLD}/{old_clip}/frames/{old_clip}_*.png'))]
    oj = json.load(open(f'{OLD}/{old_clip}/{old_clip}.json')) if os.path.exists(f'{OLD}/{old_clip}/{old_clip}.json') else {}
    rows = [(old, f"OLD {old_clip}\n{len(old)}f @{oj.get('fps','?')}fps", 0)]
    for k in range(0, len(frames), per_row):
        rows.append((frames[k:k + per_row], f"NEW {name}\n{k:02d}-{min(k+per_row,len(frames))-1:02d}\n{len(frames)}f @{fps}fps", k))
    cols = max(len(r[0]) for r in rows)
    W = 100 + cols * c; H = 24 + len(rows) * (c + 24)
    out = Image.new('RGBA', (W, H), (128, 128, 128, 255)); d = ImageDraw.Draw(out)
    d.text((4, 4), f'{old_clip} old vs {name} new. red = baseline y=392, blue = anchor x', fill=(255, 255, 255, 255))
    for r, (lst, lab, off) in enumerate(rows):
        y = 24 + r * (c + 24); d.multiline_text((4, y + 60), lab, fill=(255, 255, 255, 255))
        ax = (oj.get('body_anchor_x', 200) if r == 0 else None)
        for i, f in enumerate(lst):
            x = 100 + i * c; out.alpha_composite(f.resize((c, c), Image.LANCZOS), (x, y))
            d.line([(x, y + c * 392 // 400), (x + c, y + c * 392 // 400)], fill=(255, 80, 80, 255))
            d.text((x + 3, y + 2), f'{i + off:02d}', fill=(255, 255, 0, 255))
    out.convert('RGB').save(path)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--clip', required=True); ap.add_argument('--out-name'); ap.add_argument('--out-dir', help='default generated_v2/<out-name>')
    ap.add_argument('--video', required=True)
    ap.add_argument('--start', type=int, required=True); ap.add_argument('--stop', type=int, required=True)
    ap.add_argument('--step', type=int, default=2); ap.add_argument('--fps', type=float, default=12)
    ap.add_argument('--anchor-x', type=int, default=200); ap.add_argument('--scale', type=float, default=0.54)
    ap.add_argument('--mode', choices=['loop', 'once'], default='loop')
    ap.add_argument('--center', choices=['fixed', 'body', 'body_trend'], default='fixed')
    ap.add_argument('--cx-src', type=float, default=None, help='fixed mode: source x of body centre (default: body point of video frame 1)')
    ap.add_argument('--facing', default='front'); ap.add_argument('--no-horn-fix', action='store_true')
    ap.add_argument('--compare-old'); ap.add_argument('--mirror-to'); ap.add_argument('--mirror-anchor-x', type=int)
    ap.add_argument('--note', default='')
    ap.add_argument('--align-old', help='fixed placement: match video frame 1 to old clip frame 0 (top y + body x)')
    ap.add_argument('--keep-old-fields', default='', help='comma list of fields copied from the old clip json (e.g. seat_y,seat_x_range)')
    ap.add_argument('--durations-from-old', action='store_true', help='derive frame_durations_ms from the old clip: resample its profile to N frames, scale non-final frames to the real video time, keep the final hold')
    ap.add_argument('--snap-thresh', type=float, default=0, help='snap only frames whose lowest pixel is within this many canvas px of the ground (keeps real flight frames)')
    ap.add_argument('--ground-from', help='START:STOP (stop exclusive): take the global ground line only from these video frames (keeps a re-cut clip at the exact height of the shipped one)')
    ap.add_argument('--snap', action='store_true', help='per-frame: lowest pixel -> y=392 (clips with constant ground contact, e.g. walk)')
    a = ap.parse_args()
    name = a.out_name or a.clip
    video = os.path.abspath(a.video); fd = extract(video)
    idx = list(range(a.start, a.stop, a.step))
    load = lambda i: np.asarray(Image.open(f'{fd}/f_{i:03d}.png').convert('RGB'))
    keyed = [key_combined(load(i)) for i in idx]
    ref_keyed = key_combined(load(1))
    lows = [np.where(k[..., 3] > 16)[0].max() for k in keyed]
    if a.ground_from:
        g0, g1 = map(int, a.ground_from.split(':')); gl = [lo for i, lo in zip(idx, lows) if g0 <= i < g1 and (i - g0) % a.step == 0]
        ground = float(np.percentile(gl, 90))
    else:
        ground = float(np.percentile(lows, 90))
    ty = 392 - (ground + 1) * a.scale + 1
    if a.center == 'fixed' and a.cx_src is None:
        a.cx_src = body_x(ref_keyed[..., 3])
    if a.center == 'fixed':
        txs = [a.anchor_x - a.cx_src * a.scale] * len(keyed); bx = [a.cx_src] * len(keyed)
    else:
        bx = np.array([body_x(k[..., 3]) for k in keyed])
        if a.center == 'body_trend':
            t = np.arange(len(bx)); p = np.polyfit(t, bx, 1); use = np.polyval(p, t)
        else:
            use = bx
        txs = [a.anchor_x - u * a.scale for u in use]
    if a.snap:
        tys = [392 - (lo + 1) * a.scale + 1 for lo in lows]
    elif a.snap_thresh > 0:
        tys = [392 - (lo + 1) * a.scale + 1 if (ground - lo) * a.scale <= a.snap_thresh else ty for lo in lows]
    else:
        tys = [ty] * len(keyed)
    if a.align_old:
        of = np.asarray(Image.open(sorted(glob.glob(f'{OLD}/{a.align_old}/frames/*.png'))[0]).convert('RGBA'))
        otop = np.where(of[..., 3].max(1) > 16)[0].min(); obx = body_x(of[..., 3])
        rtop = np.where(ref_keyed[..., 3].max(1) > 16)[0].min(); rbx = body_x(ref_keyed[..., 3])
        txs = [obx - rbx * a.scale] * len(keyed); tys = [otop - rtop * a.scale] * len(keyed)
    frames = [place(k, a.scale, tx, t) for k, tx, t in zip(keyed, txs, tys)]
    horn = None
    if not a.no_horn_fix:
        ref = place(ref_keyed, a.scale, txs[0], tys[0])
        frames, horn = fix_frames(frames, ref)
    outdir = os.path.abspath(a.out_dir) if a.out_dir else os.path.join(ROOT, name)
    rel = os.path.relpath(video, outdir)
    meta_extra = {"facing": a.facing, "source_files": [rel],
        "source_frame_range": {"first": idx[0], "last": idx[-1], "step": a.step, "video_fps": 24, "note": "1-based ffmpeg frame numbers"},
        "generator": "Grok Imagine image-to-video (GenerateVideo, 720p, first=last frame = source still) + generated_v2/_tools/build_clip.py",
        "keying": "white flood-fill key + enclosed-hole removal (size/purity or isnet-anime matte) + matte trim (_tools/key_white.py)",
        "centering": a.center, "baseline_snap": "per_frame" if a.snap else (f"frames within {a.snap_thresh}px of ground snapped" if a.snap_thresh else "global (video ground line)"), "body_point_src_x": [round(float(x), 1) for x in bx],
        "horn_fix": horn, "baked_secondary_motion": ["full_body_bounce", "hip_sway", "pigtails", "tail", "chest_follow"],
        "engine_note": "v2 bakes secondary motion into the pixels. Mute or strongly reduce engine chest/pigtail springs and breathing for this clip, or the motion doubles.",
        "notes": a.note}
    if a.durations_from_old:
        oj = json.load(open(f'{OLD}/{a.compare_old or a.clip}/{a.compare_old or a.clip}.json'))
        od = oj.get('frame_durations_ms')
        if od:
            N = len(idx); on = len(od)
            prof = [od[min(on - 2, round(i * (on - 2) / max(1, N - 2)))] for i in range(N - 1)]
            real = (N - 1) * a.step / 24 * 1000
            sc = real / sum(prof)
            meta_extra['frame_durations_ms'] = [max(40, int(round(p * sc))) for p in prof] + [od[-1]]
            meta_extra['durations_note'] = f'Derived from old {a.compare_old or a.clip} frame_durations_ms: profile resampled to {N} frames and scaled to the real video time ({real:.0f} ms), final hold {od[-1]} ms kept.'
    if a.keep_old_fields:
        oj = json.load(open(f'{OLD}/{a.compare_old or a.clip}/{a.compare_old or a.clip}.json'))
        for k in a.keep_old_fields.split(','):
            if k in oj: meta_extra[k] = oj[k]
    m = write_clip(name, frames, a.fps, a.mode, a.anchor_x, a.scale, meta_extra, outdir, a.compare_old)
    print(name, m['frame_count'], 'frames', 'compliant', m['canvas_compliant'],
          'x', min(b[0] for b in m['bbox_per_frame']), max(b[1] for b in m['bbox_per_frame']),
          'low', sorted(set(b[3] for b in m['bbox_per_frame'])), 'ground src', ground)
    if a.mirror_to:
        mx = a.mirror_anchor_x if a.mirror_anchor_x is not None else 400 - a.anchor_x
        mf = [f.transpose(Image.FLIP_LEFT_RIGHT) for f in frames]
        me = dict(meta_extra); me["facing"] = "left" if a.facing.startswith('right') else "right"
        me["notes"] = f"Horizontal mirror of {name} (x' = 399 - x). Tattoo/ribbon asymmetry is mirrored. " + a.note
        me["mirrored_from"] = name
        m2 = write_clip(a.mirror_to, mf, a.fps, a.mode, mx, a.scale, me, os.path.join(os.path.dirname(outdir), a.mirror_to), a.mirror_to)
        print(a.mirror_to, m2['frame_count'], 'frames', 'compliant', m2['canvas_compliant'])

if __name__ == '__main__':
    main()
