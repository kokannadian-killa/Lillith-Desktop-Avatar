# Branding: the Lillith icon

One icon for every platform, cut from frame 0 of the idle clip
(`assets/source/video/idle/idle_v2_a.mp4`).

| Output | Used by |
|---|---|
| `icon-1024.png` | master |
| `png/lillith-{16,24,32,48,64,128,256,512}.png` | Linux hicolor theme (`Icon=lillith`), PKGBUILD |
| `../windows/lillith.ico` (16–256) | embedded in `lillith.exe` / `lillith-settings.exe` (`build.rs`), NSIS installer + shortcuts |
| `../android/app/res/mipmap-*/ic_launcher*.png`, `mipmap-anydpi-v26/*.xml` | Android launcher icon, round icon, adaptive icon |

Small sizes (≤32 px) use a tighter head crop and a little sharpening.

```sh
python3 branding/make-icons.py              # regenerate everything from icon-1024.png
python3 branding/make-icons.py --from-video # re-cut the master from the idle video
python3 branding/make-icons.py --check      # verify every output exists
```

Needs Python 3 with Pillow (`--from-video` also needs numpy, scipy and ffmpeg).
