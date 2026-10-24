"""Tiny synthetic test video for user_import tests (generated on the fly, never committed)."""
import subprocess
import numpy as np
import cv2


def figure(img, cx, base, arm, bob=0, skin=(240, 200, 180), shorts=(220, 30, 150), top=(20, 20, 20)):
    """A crude front-facing figure: head, torso, top, shorts, legs, one waving arm. Dark outline, plain colours."""
    H = img.shape[0]
    y = lambda f: int(base - f * (H * 0.85)) + bob
    out = (40, 20, 40)
    cv2.ellipse(img, (cx, y(0.88)), (int(H * .07), int(H * .085)), 0, 0, 360, out, -1)
    cv2.ellipse(img, (cx, y(0.88)), (int(H * .062), int(H * .077)), 0, 0, 360, skin, -1)
    cv2.rectangle(img, (cx - int(H * .09), y(0.78)), (cx + int(H * .09), y(0.45)), out, -1)
    cv2.rectangle(img, (cx - int(H * .082), y(0.775)), (cx + int(H * .082), y(0.455)), skin, -1)
    cv2.rectangle(img, (cx - int(H * .082), y(0.72)), (cx + int(H * .082), y(0.62)), top, -1)
    cv2.rectangle(img, (cx - int(H * .085), y(0.47)), (cx + int(H * .085), y(0.38)), shorts, -1)
    for dx in (-1, 1):
        cv2.line(img, (cx + dx * int(H * .045), y(0.38)), (cx + dx * int(H * .05), y(0.0)), out, int(H * .05))
        cv2.line(img, (cx + dx * int(H * .045), y(0.38)), (cx + dx * int(H * .05), y(0.01)), skin, int(H * .036))
    sx, sy = cx + int(H * .085), y(0.74)
    ex = int(sx + np.cos(arm) * H * .22); ey = int(sy - np.sin(arm) * H * .22)
    cv2.line(img, (sx, sy), (ex, ey), out, int(H * .04)); cv2.line(img, (sx, sy), (ex, ey), skin, int(H * .028))


def make(path, ffmpeg='ffmpeg', w=320, h=240, fps=24, wave=1.6, hold=0.8, bounce=1.4, scene2=1.8):
    """wave (motion) -> hold (still) -> bounce (motion) -> hard scene cut to a blue backdrop -> sway."""
    frames = []
    base = int(h * 0.95)
    for i in range(int(wave * fps)):
        img = np.full((h, w, 3), 255, np.uint8); figure(img, w // 2, base, 0.6 + 0.9 * np.sin(i / fps * 2 * np.pi * 1.5)); frames.append(img)
    last_arm = 0.6 + 0.9 * np.sin((int(wave * fps) - 1) / fps * 2 * np.pi * 1.5)
    for i in range(int(hold * fps)):
        img = np.full((h, w, 3), 255, np.uint8); figure(img, w // 2, base, last_arm); frames.append(img)
    for i in range(int(bounce * fps)):
        img = np.full((h, w, 3), 255, np.uint8)
        figure(img, w // 2, base, last_arm, bob=-int(abs(np.sin(i / fps * 2 * np.pi * 1.2)) * h * 0.06)); frames.append(img)
    for i in range(int(scene2 * fps)):
        img = np.full((h, w, 3), (255, 215, 190), np.uint8)  # BGR light blue backdrop
        figure(img, w // 2 + int(np.sin(i / fps * 2 * np.pi) * w * 0.12), base, 2.2, shorts=(60, 200, 40), top=(120, 0, 120))
        frames.append(img)
    cmd = [ffmpeg, '-v', 'error', '-nostdin', '-y', '-f', 'rawvideo', '-pix_fmt', 'bgr24', '-s', f'{w}x{h}', '-r', str(fps),
           '-i', 'pipe:0', '-c:v', 'libx264', '-crf', '18', '-pix_fmt', 'yuv420p', str(path)]
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE)
    for f in frames:
        p.stdin.write(f.tobytes())
    p.stdin.close()
    assert p.wait() == 0
    return len(frames) / fps
