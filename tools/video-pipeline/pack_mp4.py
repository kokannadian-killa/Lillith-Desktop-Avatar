#!/usr/bin/env python3
"""pack_mp4.py: matted clip PNGs -> alpha-packed H.264 MP4 (the engine's pose video source).

  pack_mp4.py <clip_dir> [<clip_dir> ...] [--crf 16] [--bleed 0] [--manifest assets/generated/manifest.json]
              [--metrics-only] [--force]

Input : <clip_dir>/<clip>_sheet.png (horizontal strip, W x H cells) and <clip_dir>/<clip>.json
        (frame_count, frame_width, frame_height, fps).
Output: <clip_dir>/<clip>.mp4 and a `video` block in <clip>.json; with --manifest, the clip's
        entry in manifest.json gets "video": "<clip>/<clip>.mp4" (then re-run import-manifest).

Packing ("alpha_side_by_side", see docs/VIDEO_PIPELINE.md):
  frame = 2W x H, yuv420p, full range, BT.709, one video frame per clip frame, constant rate
  = clip fps (the engine times frames from frame_durations_ms; the MP4 rate is informational).
  left  half: STRAIGHT (un-premultiplied) colour, black where alpha = 0. Because the colour is
              not premultiplied, the decoded alpha's small error only changes coverage, never the
              edge colour -> no dark/white halo (premultiplied-on-black measured edge_mae 3.1 vs
              2.6 on hair_flip). --bleed N copies the nearest body colour N px into the transparent
              area (no measurable gain at CRF 16, +35 % size, so off by default).
  no B-frames, one keyframe per clip: the engine streams frames as they decode (low delay).
  right half: alpha as grey (R=G=B=alpha), i.e. luma.
The engine (lillith_anim::video) decodes with a local `ffmpeg`, snaps alpha <= 3 to 0 and
>= 251 to 255, and premultiplies colour by the decoded alpha.

Quality metric (printed + stored in the JSON): the decoded frames composited over black and
over white vs the PNG reference, in the edge band (ref alpha in 1..254, dilated 2 px):
  edge_mae      mean |diff| (0-255, RGB) over both backgrounds
  edge_p99      99th percentile |diff|
  halo_dark     mean signed luma diff over BLACK in the band (+ = bright fringe)
  halo_light    mean signed luma diff over WHITE in the band (- = dark fringe)
  alpha_mae     mean |alpha diff| over the whole cell
Fully local: only runs ffmpeg on local files.
"""
import argparse, json, os, subprocess, sys
import numpy as np
from PIL import Image
from scipy import ndimage as nd

LAYOUT = 'alpha_side_by_side'
A_LO, A_HI = 3, 251  # decoder snap thresholds (keep in sync with crates/lillith-anim/src/video.rs)


def load_cells(d, clip, js):
    W, H, N = js.get('frame_width', 400), js.get('frame_height', 400), js['frame_count']
    sheet = np.asarray(Image.open(os.path.join(d, js.get('sheet', f'{clip}_sheet.png'))).convert('RGBA'))
    assert sheet.shape[0] >= H and sheet.shape[1] >= W * N, f'{clip}: sheet {sheet.shape} < {N}x{W}x{H}'
    return [sheet[:H, i * W:(i + 1) * W] for i in range(N)], W, H


def pack_frame(rgba, bleed):
    a = rgba[..., 3]
    rgb = rgba[..., :3].astype(np.uint8).copy()
    known = a > 0
    if known.any() and bleed > 0:
        dist, (iy, ix) = nd.distance_transform_edt(~known, return_indices=True)
        fill = rgb[iy, ix]
        near = (~known) & (dist <= bleed)
        rgb[near] = fill[near]
        rgb[(~known) & (dist > bleed)] = 0
    else:
        rgb[~known] = 0
    al = np.repeat(a[..., None], 3, axis=2)
    return np.concatenate([rgb, al], axis=1)


# Local-only: every ffmpeg call is limited to file/pipe protocols.
FFMPEG = os.environ.get('LILLITH_FFMPEG', 'ffmpeg')


def encode(frames, W, H, fps, out, crf):
    tmp = out + '.tmp.mp4'
    cmd = [FFMPEG, '-v', 'error', '-nostdin', '-y', '-protocol_whitelist', 'file,pipe', '-f', 'rawvideo', '-pix_fmt', 'rgb24',
           '-s', f'{2 * W}x{H}', '-r', f'{fps:g}', '-i', 'pipe:0',
           '-vf', 'scale=out_color_matrix=bt709:out_range=pc:flags=accurate_rnd+full_chroma_int',
           '-c:v', 'libx264', '-preset', 'veryslow', '-tune', 'animation', '-crf', str(crf),
           '-profile:v', 'high', '-level', '4.0', '-pix_fmt', 'yuv420p',
           '-color_range', 'pc', '-colorspace', 'bt709', '-color_primaries', 'bt709', '-color_trc', 'bt709',
           '-g', str(max(len(frames), 1)), '-bf', '0', '-movflags', '+faststart',
           '-map_metadata', '-1', '-fflags', '+bitexact', '-flags:v', '+bitexact', tmp]
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE)
    for f in frames:
        p.stdin.write(np.ascontiguousarray(f).tobytes())
    p.stdin.close()
    if p.wait() != 0:
        raise SystemExit(f'ffmpeg encode failed for {out}')
    os.replace(tmp, out)


def decode(path, W, H):
    """Same decode as the engine: rgb24, full range BT.709."""
    cmd = [FFMPEG, '-v', 'error', '-nostdin', '-threads', '1', '-flags', 'low_delay',
           '-protocol_whitelist', 'file,pipe', '-i', os.path.abspath(path), '-vf',
           'scale=in_color_matrix=bt709:in_range=pc:flags=accurate_rnd+full_chroma_int',
           '-f', 'rawvideo', '-pix_fmt', 'rgb24', 'pipe:1']
    raw = subprocess.run(cmd, check=True, capture_output=True).stdout
    fr = np.frombuffer(raw, np.uint8).reshape(-1, H, 2 * W, 3)
    out = []
    for f in fr:
        a = f[:, W:, 1].astype(np.int32)  # green = best luma proxy
        a = np.where(a <= A_LO, 0, np.where(a >= A_HI, 255, a)).astype(np.uint8)
        out.append(np.dstack([f[:, :W], a]))
    return out


def comp(rgba, bg):
    a = rgba[..., 3:4].astype(np.float32) / 255.0
    return rgba[..., :3].astype(np.float32) * a + bg * (1 - a)


def metrics(ref, dec):
    maes, p99s, hd, hl, amae = [], [], [], [], []
    lum = np.array([0.2126, 0.7152, 0.0722], np.float32)
    for r, d in zip(ref, dec):
        ra = r[..., 3]
        band = nd.binary_dilation((ra > 0) & (ra < 255), iterations=2) | nd.binary_dilation(ra > 0, iterations=1) ^ nd.binary_erosion(ra > 0, iterations=1)
        amae.append(np.abs(r[..., 3].astype(np.int32) - d[..., 3].astype(np.int32)).mean())
        if not band.any():
            continue
        diffs = []
        for bg, lst in ((0.0, hd), (255.0, hl)):
            df = comp(d, bg) - comp(r, bg)
            diffs.append(np.abs(df[band]).ravel())
            lst.append(float((df[band] @ lum).mean()))
        allv = np.concatenate(diffs)
        maes.append(float(allv.mean())); p99s.append(float(np.percentile(allv, 99)))
    return dict(edge_mae=round(float(np.mean(maes)), 3), edge_p99=round(float(np.max(p99s)), 2),
                halo_dark=round(float(np.mean(hd)), 3), halo_light=round(float(np.mean(hl)), 3),
                alpha_mae=round(float(np.mean(amae)), 4))


def pack_one(d, a):
    d = os.path.abspath(d.rstrip('/'))
    clip = os.path.basename(d)
    jp = os.path.join(d, f'{clip}.json')
    js = json.load(open(jp))
    cells, W, H = load_cells(d, clip, js)
    out = os.path.join(d, f'{clip}.mp4')
    fps = float(js.get('fps', 12))
    if not a.metrics_only and (a.force or not os.path.exists(out)
                               or os.path.getmtime(out) < os.path.getmtime(os.path.join(d, js.get('sheet', f'{clip}_sheet.png')))):
        encode([pack_frame(c, a.bleed) for c in cells], W, H, fps, out, a.crf)
    dec = decode(out, W, H)
    assert len(dec) == len(cells), f'{clip}: decoded {len(dec)} frames, expected {len(cells)}'
    m = metrics(cells, dec)
    size = os.path.getsize(out)
    js['video'] = {'file': f'{clip}.mp4', 'layout': LAYOUT, 'codec': 'h264 high yuv420p full-range bt709',
                   'crf': a.crf, 'edge_bleed_px': a.bleed, 'width': 2 * W, 'height': H,
                   'frame_count': len(cells), 'fps': fps, 'bytes': size, 'quality': m,
                   'tool': 'tools/video-pipeline/pack_mp4.py'}
    indent = json_indent(jp, 2)
    with open(jp, 'w') as f:
        json.dump(js, f, indent=indent)
    print(f'{clip:<28} {len(cells):>3}f {size/1024:7.1f} KiB  edge_mae {m["edge_mae"]:.2f} p99 {m["edge_p99"]:.1f} '
          f'halo dark {m["halo_dark"]:+.2f} light {m["halo_light"]:+.2f}  alpha_mae {m["alpha_mae"]:.3f}', flush=True)
    return clip, m, size


def json_indent(path, default=1):
    """Indent width the file already uses, so a rewrite keeps the diff small."""
    try:
        with open(path) as f:
            f.readline()
            line = f.readline()
        n = len(line) - len(line.lstrip(' '))
        return n or default
    except OSError:
        return default


def register(manifest, clip):
    indent = json_indent(manifest)
    mj = json.load(open(manifest))
    hit = False
    for e in mj['animations']:
        sheet_dir = e.get('sheet', '').split('/')[0]
        if e['name'] == clip or sheet_dir == clip:
            e['video'] = f'{sheet_dir}/{sheet_dir}.mp4'
            hit = True
    if not hit:
        print(f'warning: {clip} has no entry in {manifest}', file=sys.stderr)
    with open(manifest, 'w') as f:
        json.dump(mj, f, indent=indent)
        f.write('\n')


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('dirs', nargs='+')
    ap.add_argument('--crf', type=int, default=16)
    ap.add_argument('--bleed', type=int, default=0)
    ap.add_argument('--manifest')
    ap.add_argument('--metrics-only', action='store_true')
    ap.add_argument('--force', action='store_true')
    ap.add_argument('--summary', help='write per-clip metrics JSON here')
    a = ap.parse_args()
    rows = {}
    for d in a.dirs:
        clip, m, size = pack_one(d, a)
        rows[clip] = dict(m, bytes=size)
        if a.manifest:
            register(a.manifest, clip)
    if a.summary:
        json.dump(rows, open(a.summary, 'w'), indent=1)
    if len(rows) > 1:
        k = lambda key: [r[key] for r in rows.values()]
        print(f'TOTAL {len(rows)} clips {sum(k("bytes"))/2**20:.1f} MiB; edge_mae mean {np.mean(k("edge_mae")):.2f} '
              f'max {max(k("edge_mae")):.2f}; halo_dark mean {np.mean(k("halo_dark")):+.2f}; '
              f'halo_light mean {np.mean(k("halo_light")):+.2f}')


if __name__ == '__main__':
    main()
