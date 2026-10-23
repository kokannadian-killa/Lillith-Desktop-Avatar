#!/usr/bin/env bash
# One command, from the repo root (Linux / WSL):
#
#     windows/build.sh
#
# Cross-builds lillith.exe + lillith-settings.exe for x86_64-pc-windows-gnu,
# stages them with the atlas pages Lillith needs (no GGUF, no models), and
# writes to $DIST (default: <target dir>/dist/windows):
#   Lillith-<version>-setup.exe              NSIS installer (per-user)
#   Lillith-<version>-windows-x64.zip        portable folder
#   SHA256SUMS
#
# Needs: rustup target x86_64-pc-windows-gnu, mingw-w64 (x86_64-w64-mingw32-gcc),
# nsis (makensis), python3 + Pillow (icon only). Nothing is downloaded at
# install or run time; this script only fetches crates if Cargo has to.
#
# Env: CARGO (cargo binary, default `cargo`), CARGO_TARGET_DIR, DIST,
#      SKIP_BUILD=1 (reuse the last release build), NO_ZIP=1.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
TARGET=x86_64-pc-windows-gnu
CARGO="${CARGO:-cargo}"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
DIST="${DIST:-$TARGET_DIR/dist/windows}"
STAGE="$TARGET_DIR/windows-stage/Lillith"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER:-x86_64-w64-mingw32-gcc}"
export CC_x86_64_pc_windows_gnu="${CC_x86_64_pc_windows_gnu:-x86_64-w64-mingw32-gcc}"
export AR_x86_64_pc_windows_gnu="${AR_x86_64_pc_windows_gnu:-x86_64-w64-mingw32-ar}"

need() { command -v "$1" >/dev/null 2>&1 || { echo "error: '$1' not found. $2" >&2; exit 1; }; }
need "$CARGO" "Install Rust (rustup) or set CARGO=/path/to/cargo."
need x86_64-w64-mingw32-gcc "Install mingw-w64 (Debian/Ubuntu: apt install gcc-mingw-w64-x86-64)."
need makensis "Install NSIS (Debian/Ubuntu: apt install nsis)."
need python3 "Install Python 3.11+."

VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)"
echo "==> Lillith $VERSION for Windows ($TARGET)"

if [[ "${SKIP_BUILD:-}" != 1 ]]; then
  cd "$ROOT"
  # The Win32 overlay is cfg(windows); Wayland/X11 backends are off.
  "$CARGO" build --release --locked --target "$TARGET" \
    -p lillith --no-default-features --features windows
  "$CARGO" build --release --locked --target "$TARGET" -p lillith-settings-gui
fi
BIN="$TARGET_DIR/$TARGET/release"

echo "==> staging $STAGE"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp "$BIN/lillith.exe" "$BIN/lillith-settings.exe" "$STAGE/"
python3 "$HERE/stage-assets.py" "$STAGE"
[[ -f "$HERE/lillith.ico" ]] || python3 "$HERE/make-icon.py"
cp "$HERE/lillith.ico" "$STAGE/"
cp "$HERE/INSTALL-windows.md" "$STAGE/INSTALL-windows.txt"
cp "$ROOT/LICENSE" "$STAGE/LICENSE.txt"
cp "$ROOT/LICENSE-ASSETS.md" "$STAGE/LICENSE-ASSETS.txt"
cp "$ROOT/assets/config.example.toml" "$STAGE/config.example.toml"

mkdir -p "$DIST"
SETUP="$DIST/Lillith-$VERSION-setup.exe"
echo "==> makensis -> $SETUP"
makensis -V2 -INPUTCHARSET UTF8 \
  -DVERSION="$VERSION" -DSTAGE="$STAGE" -DOUTFILE="$SETUP" -DICON="$HERE/lillith.ico" \
  "$HERE/installer.nsi"

if [[ "${NO_ZIP:-}" != 1 ]]; then
  ZIP="$DIST/Lillith-$VERSION-windows-x64.zip"
  echo "==> zip -> $ZIP"
  rm -f "$ZIP"
  (cd "$(dirname "$STAGE")" && python3 -m zipfile -c "$ZIP" Lillith)
fi

(cd "$DIST" && sha256sum Lillith-"$VERSION"-* > SHA256SUMS)
ls -l "$DIST"
echo "==> done"
