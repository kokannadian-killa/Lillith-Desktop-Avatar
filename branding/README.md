# Branding: Lillith's icons

Fifteen app icons; the user picks one (`icon = "<id>"` in config.toml,
Settings ▸ Icon, right-click ▸ Icon…, `lillith icon <id>`, the Windows
installer's "App icon" page, `linux/install.sh --icon <id>`, the Android
setup screen). The default is **Phantomcore Red**.

| id | name |
|---|---|
| `neon-portrait` | Neon Portrait |
| `wink-chibi` | Wink Chibi |
| `phantomcore-red` | Phantomcore Red (default) |
| `demonos-green-chibi` | DemonOS Green Chibi |
| `neon-sign` | Neon Sign |
| `demonos-emblem` | DemonOS Emblem |
| `hooded-phantom` | Hooded Phantom |
| `green-hologram` | Green Hologram |
| `chibi-perched` | Chibi Perched |
| `kiss-skullframe` | Kiss Skullframe |
| `winged-sigil` | Winged Sigil |
| `pixel-chibi` | Pixel Chibi |
| `glitch` | Glitch |
| `chibi-terminal` | Chibi Terminal |
| `classic` | Classic (frame 0 of the idle clip, the original icon) |

## Outputs

| Output | Used by |
|---|---|
| `icons.toml` | the registry (ids, names, paths); compiled into `lillith_core::icons` |
| `icons/<id>/icon-1024.png` | master: rounded square (~18–27 % radius, soft edge), transparent outside |
| `icons/<id>/lillith-{16,24,32,48,64,128,256}.png` | Linux hicolor (`Icon=lillith`), Settings ▸ Icon thumbnails, window icon |
| `icons/<id>/<id>.ico` (16–256, 32-bit PNG entries) | Windows shortcuts (`icons\<id>.ico` when installed) |
| `../windows/lillith.ico` | copy of the default's `.ico`, embedded in the `.exe` files (`build.rs`) and the installer |
| `../android/app/res/mipmap-anydpi-v26/ic_icon_<id>.xml` + `mipmap-xxxhdpi/ic_icon_<id>_fg.png` | Android adaptive icon per alias; `ic_launcher*` = the default |
| `../android/app/AndroidManifest.xml` (`ICON-ALIASES` block), `../android/app/src/…/IconSet.java` | one `activity-alias` per icon (only the default enabled) |

The 14 art icons come from 1024² JPGs (rounded square on black); the script
finds the rounded square (corner flood fill), crops it, and cuts it out with
a supersampled rounded mask. PNGs are squeezed with `pngquant` (sizes below
32 px stay lossless). Small sizes (≤ 48 px) get a little sharpening; Classic
also keeps its tighter head crop at ≤ 32 px.

```sh
python3 branding/make-icons.py --from-sources DIR  # rebuild the set from DIR/NN-<id>.jpg (sources not in the repo)
python3 branding/make-icons.py                    # regenerate outputs from the masters
python3 branding/make-icons.py --from-video       # re-cut the Classic master from the idle video
python3 branding/make-icons.py --sheet out.png    # contact sheet (256 + 32 px)
python3 branding/make-icons.py --check            # verify every output exists
```

Adding an icon: add `(id, name, source stem)` to `SET` in `make-icons.py`,
re-run it (registry, Android aliases and `IconSet.java` follow), and the
Rust test `icons::tests::every_id_resolves_to_real_files` checks the files.

Needs Python 3 with Pillow and numpy/scipy; `pngquant` optional (bigger
files without it); `--from-video` also needs ffmpeg.
