"""Build generated_v2/idle from source/video/idle/idle_v2_a.mp4 frames.
Usage: build_idle_v2.py <framesdir> <outdir> <start> <stop_exclusive> <step> <fps>"""
import sys, json, os
import numpy as np
from PIL import Image
sys.path.insert(0, os.path.dirname(__file__))
from key_white import key_white, key_combined

src, out, a, b, step, fps = sys.argv[1], sys.argv[2], *map(int, sys.argv[3:7])
S = 0.54                      # same scale as generated/idle (source framing identical)
CX_SRC = 645                  # body centre in source px (as generated/idle)
os.makedirs(f'{out}/frames', exist_ok=True)
idx = list(range(a, b, step))
keyed = [key_combined(np.asarray(Image.open(f'{src}/f_{i:03d}.png').convert('RGB'))) for i in idx]
# global baseline from the planted boots (median lowest opaque row) -> no jitter
lows = [np.where(k[..., 3] > 16)[0].max() for k in keyed]
low = float(np.median(lows))
tx = 200 - CX_SRC * S
ty = 392 - (low + 1) * S + 1      # bottom edge of lowest row lands on y=392
frames = []
for n, k in enumerate(keyed):
    im = Image.fromarray(k, 'RGBA')
    im = im.resize((round(im.width * S), round(im.height * S)), Image.LANCZOS)
    cv = Image.new('RGBA', (400, 400), (0, 0, 0, 0))
    cv.paste(im, (int(round(tx)), int(np.floor(ty))), im)
    arr = np.asarray(cv).copy(); arr[392 + 1:, :, 3] = np.where(arr[393:, :, 3] > 16, arr[393:, :, 3], 0)
    cv = Image.fromarray(arr, 'RGBA')
    cv.save(f'{out}/frames/idle_{n:02d}.png'); frames.append(cv)
N = len(frames)
sheet = Image.new('RGBA', (400 * N, 400), (0, 0, 0, 0))
for n, f in enumerate(frames): sheet.paste(f, (400 * n, 0))
sheet.save(f'{out}/idle_sheet.png')
gray = Image.new('RGBA', sheet.size, (128, 128, 128, 255)); gray.alpha_composite(sheet); gray.convert('RGB').save(f'{out}/idle_sheet_on_gray.png')
# preview gif on gray
gif = []
for f in frames:
    g = Image.new('RGBA', (400, 400), (128, 128, 128, 255)); g.alpha_composite(f); gif.append(g.convert('RGB'))
gif[0].save(f'{out}/idle_preview.gif', save_all=True, append_images=gif[1:], duration=int(1000 / fps), loop=0)
# stats
stats = []
for f in frames:
    al = np.asarray(f)[..., 3]; ys, xs = np.where(al > 16)
    stats.append((int(xs.min()), int(xs.max()), int(ys.min()), int(ys.max())))
meta = {
  "name": "idle", "placeholder": False, "frame_count": N, "frame_width": 400, "frame_height": 400,
  "canvas_w": 400, "canvas_h": 400, "layout": "horizontal_strip", "sheet": "idle_sheet.png", "frames_dir": "frames/",
  "fps": fps, "loop": True, "baseline_y": 392, "anchor": [200, 392],
  "style": "canonical_ko (thigh-highs + shorts)", "source": "grok-imagine-video",
  "source_files": [os.environ.get("SRC_VIDEO", "../../source/video/idle/idle_v2_a.mp4")],
  "source_frame_range": {"first": a, "last": idx[-1], "step": step, "video_fps": 24, "note": "1-based ffmpeg frame numbers"},
  "scale": S, "body_anchor_x": 200, "facing": "front",
  "generator": "Grok Imagine image-to-video (GenerateVideo, 720p, first=last frame = _imagine_raw/idle_full_02.jpg)",
  "keying": "white flood-fill key + enclosed-hole removal (size/purity or isnet-anime matte) + matte trim + soft edge un-mix against white (_tools/key_white.py)",
  "baked_secondary_motion": ["pigtail_swing", "tail_swish", "hip_weight_shift", "head_tilt", "blink_wink"],
  "engine_note": "v2 bakes secondary motion into the pixels. Mute or strongly reduce the engine chest/pigtail springs and breathing for this clip, or the motion doubles.",
  "canvas_compliant": all(s[0] >= 3 and s[1] <= 396 and s[3] <= 393 for s in stats),
  "bbox_per_frame": stats,
}
json.dump(meta, open(f'{out}/idle.json', 'w'), indent=2)
print(N, 'frames; lows', sorted(set(lows)), 'tx,ty', tx, ty); print(stats)
