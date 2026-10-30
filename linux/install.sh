#!/usr/bin/env bash
# Lillith for Arch / Garuda / Manjaro (pacman), per-user install.
#
#     bash install.sh                 the top-level wrapper (any folder, any shell)
#     ./linux/install.sh              from the repo root (works from fish too)
#     ./linux/install.sh --uninstall  remove it again (asks before deleting settings)
#
# Builds Lillith from this checkout and installs, for the current user only:
#   ~/.local/bin/lillith, ~/.local/bin/lillith-settings
#   ~/.local/share/lillith/generated/          her animation set (found automatically)
#   ~/.local/share/applications/lillith.desktop + icons (hicolor, Icon=lillith)
#   ~/.local/share/lillith/icons/<id>/         the whole icon set (Settings > Icon)
#   a desktop shortcut, optional autostart, the KWin bridge (Plasma 6)
#   excludes her from KDE force blur (Better Blur DX), see kwin-blur-exclude.sh
#   ~/.config/lillith/config.toml              via `lillith setup` (local model)
#   ~/.local/share/lillith/llama/linux-x64/    the bundled llama-server, if this
#                                              is the all-in-one download
#                                              (bundle/llama/, see INSTALL.md)
#
# LOCAL ONLY: the only network use is pacman installing distro packages,
# Cargo fetching crates for the build and, for a GitHub "Download ZIP" whose
# art is still LFS placeholders, fetching that art from this repo (asked first). It never downloads a model, never
# installs from the AUR, and Lillith only talks to llama-server on 127.0.0.1.
#
# Run with --help for all options. Safe to re-run (upgrades in place).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"

DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
BIN_DIR="${LILLITH_BIN_DIR:-$HOME/.local/bin}"
DATA_DIR="$DATA_HOME/lillith"
ASSET_DIR="$DATA_DIR/generated"
CONFIG_DIR="$CONFIG_HOME/lillith"
CONFIG_FILE="$CONFIG_DIR/config.toml"
APP_FILE="$DATA_HOME/applications/lillith.desktop"
AUTOSTART_FILE="$CONFIG_HOME/autostart/lillith.desktop"
ICON_BASE="$DATA_HOME/icons/hicolor"
ICON_SIZES=(16 24 32 48 64 128 256)
ICONS_DIR="$DATA_DIR/icons"          # the icon set (lillith finds it here)
ICONS_TOML="$ROOT/branding/icons.toml"
KWIN_ID="lillith-bridge"
MARK="X-Lillith-Installer=linux/install.sh"
LLAMA_DIR="$DATA_DIR/llama"           # installed copy of bundle/llama/<platform>/
# The all-in-one download's prebuilt llama-server (tools/bundle/make-bundle.sh):
# bundle/llama/ in this checkout, or next to source/ in the release zip.
BUNDLE_LLAMA=""
for d in "$ROOT/bundle/llama" "$ROOT/../bundle/llama"; do
  if [[ -x $d/linux-x64/llama-server ]]; then BUNDLE_LLAMA="$(cd "$d" && pwd)"; break; fi
done

# ── options ─────────────────────────────────────────────────────────
YES=0 UNINSTALL=0 SKIP_DEPS=0 SKIP_BUILD=0 PURGE=0 LIST_ICONS=0
ASSETS="" GGUF="" LLAMA="" CTX="" PREBUILT="" VOICE=""
AUTOSTART="" DESKTOP_SHORTCUT="" KWIN="" START="" ICON=""

usage() {
  cat <<'USAGE'
Usage: ./linux/install.sh [options]

  -y, --yes              no questions: take every default (autostart off,
                         desktop shortcut on, model auto-picked if found)
      --uninstall        remove Lillith (same as ./linux/uninstall.sh)
      --purge            with --uninstall: also delete settings + chat history
                         (otherwise asked; GGUF model files are never touched)

  Install steps:
      --skip-deps        don't run pacman (packages already installed, or not Arch)
      --skip-build       reuse the last release build in the Cargo target dir
      --prebuilt DIR     install the ready-made programs in DIR (lillith,
                         lillith-settings) instead of building; no Rust needed.
                         The all-in-one Lillith-main-linux.zip passes its bin/.
      --no-voice         don't set up her voice from the bundled offline kit
      --assets PATH      animation set to install: a folder with manifest.toml,
                         or a .zip containing assets/generated (e.g. the release's
                         Lillith-<ver>-windows-x64.zip). Default: ./assets/generated
      --gguf FILE        the .gguf model already on disk (skips the search)
      --llama-server BIN the llama-server program already on disk
      --ctx-size N       llama-server context size (default: lillith's)
      --autostart / --no-autostart          start with the desktop session (default: no)
      --desktop-shortcut / --no-desktop-shortcut   icon on the desktop (default: yes)
      --no-kwin          don't install/enable the KWin bridge script
      --icon ID          app icon (menu entry, desktop shortcut); saved as
                         icon = "ID" in config.toml. Default: the configured
                         one, else neon-sign. --list-icons shows the ids.
                         Change it later in Settings > Icon or: lillith icon ID
      --list-icons       print the icon ids and exit
      --start / --no-start                  start Lillith at the end (asked; --yes: no)

  Environment: CARGO_TARGET_DIR (build dir, default ./target),
               LILLITH_BIN_DIR (default ~/.local/bin), XDG_DATA_HOME, XDG_CONFIG_HOME.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -y|--yes) YES=1 ;;
    --uninstall) UNINSTALL=1 ;;
    --purge) PURGE=1 ;;
    --skip-deps) SKIP_DEPS=1 ;;
    --skip-build) SKIP_BUILD=1 ;;
    --prebuilt) PREBUILT="${2:?--prebuilt needs a folder}"; shift ;;
    --prebuilt=*) PREBUILT="${1#*=}" ;;
    --no-voice) VOICE=0 ;;
    --assets) ASSETS="${2:?--assets needs a path}"; shift ;;
    --assets=*) ASSETS="${1#*=}" ;;
    --gguf) GGUF="${2:?--gguf needs a file}"; shift ;;
    --gguf=*) GGUF="${1#*=}" ;;
    --llama-server) LLAMA="${2:?--llama-server needs a path}"; shift ;;
    --llama-server=*) LLAMA="${1#*=}" ;;
    --ctx-size) CTX="${2:?--ctx-size needs a number}"; shift ;;
    --ctx-size=*) CTX="${1#*=}" ;;
    --autostart) AUTOSTART=1 ;;
    --no-autostart) AUTOSTART=0 ;;
    --desktop-shortcut) DESKTOP_SHORTCUT=1 ;;
    --no-desktop-shortcut) DESKTOP_SHORTCUT=0 ;;
    --no-kwin) KWIN=0 ;;
    --icon) ICON="${2:?--icon needs an id (see --list-icons)}"; shift ;;
    --icon=*) ICON="${1#*=}" ;;
    --list-icons) LIST_ICONS=1 ;;
    --start) START=1 ;;
    --no-start) START=0 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
  shift
done

# ── helpers ─────────────────────────────────────────────────────────
if [[ -t 1 ]]; then B=$'\e[1m' G=$'\e[32m' Y=$'\e[33m' R=$'\e[31m' N=$'\e[0m'; else B="" G="" Y="" R="" N=""; fi
step() { printf '\n%s==> %s%s\n' "$B" "$*" "$N"; }
ok()   { printf '  %s✓%s %s\n' "$G" "$N" "$*"; }
warn() { printf '  %s!%s %s\n' "$Y" "$N" "$*" >&2; }
die()  { printf '%serror:%s %s\n' "$R" "$N" "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# ask "Question?" y|n  → 0 for yes. --yes (or no terminal) takes the default.
ask() {
  local q="$1" def="$2" hint ans
  if [[ $def == y ]]; then hint="[Y/n]"; else hint="[y/N]"; fi
  if [[ $YES == 1 || ! -t 0 ]]; then
    printf '  %s %s %s\n' "$q" "$hint" "$([[ $def == y ]] && echo yes || echo no)"
    [[ $def == y ]]
    return
  fi
  while true; do
    read -r -p "  $q $hint " ans || ans=""
    case "${ans,,}" in
      "") [[ $def == y ]]; return ;;
      y|yes) return 0 ;;
      n|no) return 1 ;;
    esac
  done
}

# choose "prompt" item... → prints the chosen item (or nothing for "skip").
# --yes picks the first item.
choose() {
  local prompt="$1"; shift
  local items=("$@") i n
  if [[ $YES == 1 || ! -t 0 ]]; then printf '%s' "${items[0]}"; return; fi
  for i in "${!items[@]}"; do printf '    %d) %s\n' "$((i + 1))" "${items[$i]}" >&2; done
  printf '    0) skip\n' >&2
  while true; do
    read -r -p "  $prompt [1] " n || n=0
    n="${n:-1}"
    if [[ $n == 0 ]]; then return; fi
    if [[ $n =~ ^[0-9]+$ ]] && ((n >= 1 && n <= ${#items[@]})); then
      printf '%s' "${items[$((n - 1))]}"; return
    fi
  done
}

running() { pgrep -x lillith >/dev/null 2>&1; }

# Plasma / KDE helpers (each optional).
kwin_reconfigure() {
  if have qdbus6; then qdbus6 org.kde.KWin /KWin reconfigure >/dev/null 2>&1 || true
  elif have dbus-send; then
    dbus-send --session --type=method_call --dest=org.kde.KWin /KWin org.kde.KWin.reconfigure >/dev/null 2>&1 || true
  fi
}
refresh_menus() {
  if have update-desktop-database; then update-desktop-database "$DATA_HOME/applications" >/dev/null 2>&1 || true; fi
  if have gtk-update-icon-cache; then gtk-update-icon-cache -q -t "$ICON_BASE" >/dev/null 2>&1 || true; fi
  if have kbuildsycoca6; then kbuildsycoca6 --noincremental >/dev/null 2>&1 || true; fi
}
# icon_ids: the ids in branding/icons.toml, in order.
icon_ids() { sed -n 's/^id = "\(.*\)"$/\1/p' "$ICONS_TOML"; }
icon_default() { sed -n 's/^default = "\(.*\)"$/\1/p' "$ICONS_TOML" | head -n 1; }
is_icon() { icon_ids | grep -qxF -- "$1"; }
# configured_icon: top-level `icon = "..."` of config.toml (before any [table]).
configured_icon() {
  [[ -f $CONFIG_FILE ]] || return 0
  awk '/^[[:space:]]*\[/ { exit }
       /^[[:space:]]*icon[[:space:]]*=/ { sub(/^[^=]*=[[:space:]]*"/, ""); sub(/".*$/, ""); print; exit }' "$CONFIG_FILE"
}
desktop_dir() {
  local d=""
  have xdg-user-dir && d="$(xdg-user-dir DESKTOP 2>/dev/null || true)"
  # xdg-user-dir falls back to $HOME itself when no desktop folder is set.
  if [[ -z $d || $d == "$HOME" || $d == "$HOME/" ]]; then d="$HOME/Desktop"; fi
  printf '%s' "$d"
}

# ── icon choice ─────────────────────────────────────────────────────
[[ -f $ICONS_TOML ]] || { echo "missing $ICONS_TOML (run from a full checkout)" >&2; exit 1; }
if [[ $LIST_ICONS == 1 ]]; then
  def="$(icon_default)"
  while read -r id; do
    if [[ $id == "$def" ]]; then echo "$id (default)"; else echo "$id"; fi
  done < <(icon_ids)
  exit 0
fi
ICON_SAVE=0
if [[ -n $ICON ]]; then
  is_icon "$ICON" || { echo "unknown icon '$ICON'; ids: $(icon_ids | tr '\n' ' ')" >&2; exit 2; }
  ICON_SAVE=1
else
  ICON="$(configured_icon)"
  is_icon "${ICON:-}" || ICON="$(icon_default)"
fi

# ════════════════════════════════════════════════════════════════════
# Uninstall
# ════════════════════════════════════════════════════════════════════
if [[ $UNINSTALL == 1 ]]; then
  step "Uninstalling Lillith (your GGUF models and own llama.cpp are never touched)"
  if running; then
    if ask "Lillith is running. Stop her now?" y; then pkill -x lillith || true; sleep 1; fi
  fi
  for f in "$BIN_DIR/lillith" "$BIN_DIR/lillith-settings" "$APP_FILE"; do
    [[ -e $f ]] && rm -f -- "$f" && ok "removed $f"
  done
  for unit in lillith.service lillith-quantumsec.service; do
    u="$CONFIG_HOME/systemd/user/$unit"
    if [[ -f $u ]]; then
      if have systemctl; then systemctl --user disable --now "$unit" >/dev/null 2>&1 || true; fi
      rm -f -- "$u" && ok "removed $u"
    fi
  done
  if have systemctl; then systemctl --user daemon-reload >/dev/null 2>&1 || true; fi
  # Autostart: same file Lillith's own settings toggle writes.
  if [[ -f $AUTOSTART_FILE ]] && grep -q 'lillith' "$AUTOSTART_FILE"; then
    rm -f -- "$AUTOSTART_FILE" && ok "removed $AUTOSTART_FILE"
  fi
  DESK="$(desktop_dir)/lillith.desktop"
  if [[ -f $DESK ]] && grep -q "^$MARK" "$DESK"; then rm -f -- "$DESK" && ok "removed $DESK"; fi
  for s in "${ICON_SIZES[@]}" 512; do rm -f -- "$ICON_BASE/${s}x${s}/apps/lillith.png"; done
  if [[ -d $ICONS_DIR ]]; then rm -rf -- "$ICONS_DIR"; fi
  ok "removed icons"
  if [[ -d $ASSET_DIR ]]; then rm -rf -- "$ASSET_DIR" && ok "removed $ASSET_DIR"; fi
  # Only the bundled copy this installer made (marked by its BUNDLE.txt).
  if compgen -G "$LLAMA_DIR/*/BUNDLE.txt" >/dev/null; then
    rm -rf -- "$LLAMA_DIR" && ok "removed the bundled llama-server ($LLAMA_DIR)"
  fi
  if have kpackagetool6 && kpackagetool6 --type=KWin/Script --show "$KWIN_ID" >/dev/null 2>&1; then
    if have kwriteconfig6; then kwriteconfig6 --file kwinrc --group Plugins --key "${KWIN_ID}Enabled" --delete || true; fi
    if kpackagetool6 --type=KWin/Script --remove "$KWIN_ID" >/dev/null 2>&1; then
      ok "removed the KWin bridge script"
    else
      warn "couldn't remove the KWin script"
    fi
    kwin_reconfigure
  fi
  # Undo the force-blur exclusion the installer added (only what it changed).
  if [[ -f "$DATA_DIR/kwin-blur-exclusion.state" ]]; then
    "$HERE/kwin-blur-exclude.sh" revert && ok "restored the blur effect's window classes"
  fi
  refresh_menus

  # Her voice (installed by step 7b from the bundled kit): the daemon, the
  # lillith-say/-listen/... commands and ~1 GB of models + venv.
  LV_HOME="${LILLITH_VOICE_HOME:-$DATA_HOME/lillith-voice}"
  VOICE_UNIT="$CONFIG_HOME/systemd/user/lillith-voice.service"
  if [[ -d $LV_HOME || -f $VOICE_UNIT || -e $BIN_DIR/lillith-say ]]; then
    if ask "Remove her voice too ($LV_HOME, about 1 GB, and the lillith-voice service)?" y; then
      if have systemctl; then
        systemctl --user disable --now lillith-voice.service >/dev/null 2>&1 || true
      fi
      rm -f -- "$VOICE_UNIT" "$CONFIG_HOME/fish/conf.d/lillith-voice.fish"
      for b in lillith-say lillith-voiced lillith-chat lillith-piper lillith-listen; do
        rm -f -- "$BIN_DIR/$b"
      done
      if [[ -f $BIN_DIR/piper ]] && grep -qs lillith "$BIN_DIR/piper"; then rm -f -- "$BIN_DIR/piper"; fi
      # Only her own voice folder (its name ends in lillith-voice), never a
      # path someone pointed LILLITH_VOICE_HOME at by mistake.
      if [[ -d $LV_HOME && ${LV_HOME%/} == */lillith-voice ]]; then
        rm -rf -- "$LV_HOME"
      elif [[ -d $LV_HOME ]]; then
        warn "not removing $LV_HOME (not a lillith-voice folder); delete it yourself"
      fi
      if have systemctl; then systemctl --user daemon-reload >/dev/null 2>&1 || true; fi
      ok "removed her voice"
    else
      ok "kept her voice (remove later: bash tools/lillith-voice/uninstall.sh)"
    fi
  fi

  # Settings + personal data: named files only (never a blind rm -rf, so a
  # model someone parked in these folders survives). Her knowledge/ and
  # skills/ folders are hers (she makes them) and go with the same answer.
  USER_FILES=("$CONFIG_FILE" "$CONFIG_DIR/Modelfile.lily" "$DATA_DIR/chat_history.json"
              "$DATA_DIR/memory.db" "$DATA_DIR/memory.db-wal" "$DATA_DIR/memory.db-shm"
              "$DATA_DIR/lillith_memory.db" "$DATA_DIR/lillith_memory.db-wal" "$DATA_DIR/lillith_memory.db-shm")
  USER_DIRS=("$DATA_DIR/knowledge" "$DATA_DIR/skills")
  present=() present_dirs=()
  for f in "${USER_FILES[@]}" "$CONFIG_DIR"/config.toml.*; do [[ -e $f ]] && present+=("$f"); done
  for d in "${USER_DIRS[@]}"; do [[ -d $d ]] && present_dirs+=("$d"); done
  if ((${#present[@]} + ${#present_dirs[@]})); then
    printf '  Your settings, chat history and her memory:\n'
    printf '    %s\n' "${present[@]}" "${present_dirs[@]/%//}"
    if [[ $PURGE == 1 ]] || ask "Delete these too?" n; then
      ((${#present[@]})) && rm -f -- "${present[@]}"
      ((${#present_dirs[@]})) && rm -rf -- "${present_dirs[@]}"
      rmdir -- "$CONFIG_DIR" "$DATA_DIR" 2>/dev/null || true
      ok "deleted settings and chat history"
    else
      ok "kept your settings (reinstalling picks them up again)"
    fi
  fi
  rmdir -- "$DATA_DIR" 2>/dev/null || true
  step "Lillith is uninstalled."
  exit 0
fi

# ════════════════════════════════════════════════════════════════════
# Install
# ════════════════════════════════════════════════════════════════════
printf '%sLillith installer%s (Arch / Garuda, per-user, local-only)\n' "$B" "$N"
printf '  repo: %s\n' "$ROOT"
[[ -f $ROOT/Cargo.toml && -d $ROOT/apps/lillith ]] || die "run this from a Lillith checkout (linux/ next to apps/ and crates/)"
[[ $EUID -ne 0 ]] || die "run as your normal user, not root (it asks for sudo only for pacman)"

# ── 1. animation set first, so a main.zip without the art fails fast ─
step "1/8  Checking her animation set"
have python3 || [[ $SKIP_DEPS == 0 ]] || die "python3 is needed (sudo pacman -S python)"
ASSET_SRC="${ASSETS:-$ROOT/assets/generated}"
ASSET_STATE=ok
if have python3; then
  set +e; python3 "$HERE/stage-assets.py" --check "$ASSET_SRC"; rc=$?; set -e
  if [[ $rc == 3 ]]; then
    ASSET_STATE=lfs
    if [[ -z $ASSETS ]] && git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1 && have git-lfs; then
      if ask "The art is still Git LFS pointers. Fetch it now with 'git lfs pull' (from your GitHub repo)?" y; then
        git -C "$ROOT" lfs pull --include "assets/generated/**" && ASSET_STATE=ok
        python3 "$HERE/stage-assets.py" --check "$ASSET_SRC" >/dev/null || ASSET_STATE=lfs
      fi
    fi
    # A GitHub "Download ZIP" (main.zip): no git, so fetch the real files
    # straight from the repo's Git LFS storage (needs your GitHub login).
    if [[ $ASSET_STATE == lfs && -z $ASSETS ]]; then
      if ask "Her art isn't in this zip yet (only placeholders). Download it from your GitHub repo now (about 530 MB)?" y; then
        set +e; python3 "$HERE/fetch-art.py"; frc=$?; set -e
        if [[ $frc == 4 && -t 0 ]]; then
          if ! have gh && have pacman && [[ $SKIP_DEPS == 0 ]]; then
            ask "Install the GitHub login tool (github-cli) with pacman?" y && sudo pacman -S --needed github-cli
          fi
          if have gh && ask "Log in to GitHub now (opens your browser)?" y; then
            gh auth login --hostname github.com --git-protocol https --web || true
            set +e; python3 "$HERE/fetch-art.py"; frc=$?; set -e
          fi
        fi
        if [[ $frc == 0 ]] && python3 "$HERE/stage-assets.py" --check "$ASSET_SRC" >/dev/null 2>&1; then
          ASSET_STATE=ok; ok "her art is downloaded and checked"
        fi
      fi
    fi
    if [[ $ASSET_STATE == lfs ]] && python3 "$HERE/stage-assets.py" --check "$ASSET_DIR" >/dev/null 2>&1; then
      warn "keeping the animation set that's already installed in $ASSET_DIR"
      ASSET_STATE=keep
    fi
    if [[ $ASSET_STATE == lfs ]]; then
      cat >&2 <<MSG

  ${R}Her animation frames aren't in this download.${N} GitHub's "Download ZIP"
  (main.zip) has only small Git LFS placeholders instead of the PNG art.
  Any one of these fixes it, then run the installer again (bash install.sh):

    0) Log in to GitHub so the installer can download the art itself:
         sudo pacman -S --needed github-cli
         gh auth login --web

    a) Point the installer at a copy that has the art, e.g. the portable zip
       from the GitHub release (Lillith-0.0.1-windows-x64.zip, same art):
         ./linux/install.sh --assets ~/Downloads/Lillith-0.0.1-windows-x64.zip
    b) Use git instead of the zip:
         sudo pacman -S --needed git git-lfs
         git clone https://github.com/kokannadian-killa/Lillith-Desktop-Avatar.git
         cd Lillith-Desktop-Avatar; git lfs install; git lfs pull
    c) In the GitHub repo: Settings > General > Archives >
       "Include Git LFS objects in archives", then download main.zip again.
MSG
      exit 3
    fi
  elif [[ $rc != 0 ]]; then
    die "no usable animation set at $ASSET_SRC (see above)"
  fi
fi

# ── 2. packages ─────────────────────────────────────────────────────
step "2/8  System packages"
# ffmpeg plays her MP4 poses; the X11/GL libraries are for the Settings
# window and the X11 fallback (a normal Plasma desktop already has them).
PKGS=(libxkbcommon wayland dbus libpulse systemd-libs alsa-lib python ffmpeg pipewire-audio
      libx11 libxcb libxcursor libxi libxrender libglvnd mesa)
# Building needs the toolchain too; the prebuilt programs don't.
[[ -n $PREBUILT ]] || PKGS=(base-devel git pkgconf "${PKGS[@]}")
if [[ $SKIP_DEPS == 1 ]]; then
  warn "skipped (--skip-deps); needed on Arch: ${PKGS[*]} + rust"
else
  have pacman || die "pacman not found: this installer is for Arch-based systems (use --skip-deps if the libraries are already there; see INSTALL.md for apt/dnf)"
  [[ -n $PREBUILT ]] || have cargo || PKGS+=(rust)
  missing="$(pacman -T "${PKGS[@]}" 2>/dev/null || true)"
  if [[ -z $missing ]]; then
    ok "all present: ${PKGS[*]}"
  else
    printf '  missing: %s\n' "$(echo "$missing" | tr '\n' ' ')"
    if ask "Install them with 'sudo pacman -S --needed'?" y; then
      PACMAN_ARGS=(-S --needed)
      [[ $YES == 1 ]] && PACMAN_ARGS+=(--noconfirm)
      mapfile -t missing_list <<<"$missing"
      sudo pacman "${PACMAN_ARGS[@]}" "${missing_list[@]}"
    else
      die "can't build without them"
    fi
  fi
fi
if [[ -n $PREBUILT ]]; then
  ok "ready-made programs in $PREBUILT: no Rust or build tools needed"
else
# rustup present but no default toolchain → cargo exists but fails.
if have rustup && ! cargo --version >/dev/null 2>&1; then
  if ask "rustup has no default toolchain. Run 'rustup default stable'?" y; then rustup default stable; fi
fi
have cargo || die "cargo not found (sudo pacman -S rust)"
RUSTV="$(rustc --version | awk '{print $2}')"
if [[ "$(printf '%s\n1.85.0\n' "$RUSTV" | sort -V | head -n1)" != 1.85.0 ]]; then
  die "Rust $RUSTV is too old, need 1.85+ (sudo pacman -Syu rust, or rustup update stable)"
fi
ok "rust $RUSTV"
fi

# ── 3. build ────────────────────────────────────────────────────────
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
REL="$TARGET_DIR/release"
step "3/8  Building (release; first build takes a few minutes)"
BUILD_PKGS=(-p lillith)
HAS_SETTINGS=0
if [[ -f $ROOT/crates/lillith-settings-gui/Cargo.toml ]]; then BUILD_PKGS+=(-p lillith-settings-gui); HAS_SETTINGS=1; fi
if [[ -n $PREBUILT ]]; then
  REL="$(cd "$PREBUILT" && pwd)"
  ok "skipped: using the ready-made programs in $REL"
elif [[ $SKIP_BUILD == 1 ]]; then
  warn "skipped (--skip-build), using $REL"
else
  (cd "$ROOT" && cargo build --release --locked "${BUILD_PKGS[@]}")
fi
[[ -x $REL/lillith ]] || die "$REL/lillith wasn't built"
ok "built $REL/lillith"

# ── 4. files ────────────────────────────────────────────────────────
step "4/8  Installing files"
if running; then warn "Lillith is running; she keeps the old version until restarted"; fi
install -Dm755 "$REL/lillith" "$BIN_DIR/lillith"
ok "$BIN_DIR/lillith"
if [[ $HAS_SETTINGS == 1 && -x $REL/lillith-settings ]]; then
  install -Dm755 "$REL/lillith-settings" "$BIN_DIR/lillith-settings"
  ok "$BIN_DIR/lillith-settings"
fi
mkdir -p "$DATA_DIR"
chmod 700 "$DATA_DIR"   # her memory.db and knowledge live here: yours only
if [[ $ASSET_STATE == ok ]]; then
  python3 "$HERE/stage-assets.py" "$ASSET_SRC" "$ASSET_DIR" | sed 's/^/  /'
fi
# Seasonal outfits: Lillith reads seasons.toml next to generated/ (else her
# built-in copy). It lives in assets/, not assets/generated/.
if [[ -f $ROOT/assets/seasons.toml ]]; then
  install -Dm644 "$ROOT/assets/seasons.toml" "$DATA_DIR/seasons.toml"
fi
# No env var or config key needed: Lillith looks in
# $XDG_DATA_HOME/lillith/generated/manifest.toml by itself.

# All-in-one download: copy the bundled llama-server (CPU build, plus the
# optional Vulkan build) so the install keeps working without the zip.
if [[ -n $BUNDLE_LLAMA ]]; then
  for plat in linux-x64 linux-x64-vulkan; do
    [[ -x $BUNDLE_LLAMA/$plat/llama-server ]] || continue
    rm -rf -- "${LLAMA_DIR:?}/$plat"
    mkdir -p "$LLAMA_DIR"
    cp -a "$BUNDLE_LLAMA/$plat" "$LLAMA_DIR/$plat"
    [[ -f $BUNDLE_LLAMA/llama.lock ]] && cp "$BUNDLE_LLAMA/llama.lock" "$LLAMA_DIR/"
    ok "bundled llama-server -> $LLAMA_DIR/$plat"
  done
fi

# The whole icon set (PNGs only; the .ico files are for Windows), so
# Settings > Icon / `lillith icon` can switch without the checkout.
if [[ -d $ICONS_DIR ]]; then rm -rf -- "$ICONS_DIR"; fi
n_icons=0
while read -r id; do
  for s in "${ICON_SIZES[@]}"; do
    src="$ROOT/branding/icons/$id/lillith-$s.png"
    [[ -f $src ]] && install -Dm644 "$src" "$ICONS_DIR/$id/lillith-$s.png"
  done
  n_icons=$((n_icons + 1))
done < <(icon_ids)
ok "$n_icons icons -> $ICONS_DIR"
# The menu / desktop icon: hicolor `lillith` = the chosen icon.
for s in "${ICON_SIZES[@]}"; do
  src="$ICONS_DIR/$ICON/lillith-$s.png"
  [[ -f $src ]] && install -Dm644 "$src" "$ICON_BASE/${s}x${s}/apps/lillith.png"
done
rm -f -- "$ICON_BASE/512x512/apps/lillith.png"   # older installs shipped 512 px
ok "icon '$ICON' -> $ICON_BASE/<size>/apps/lillith.png"

# desktop_entry [extra Desktop Entry lines...]
desktop_entry() {
  local extra=""
  if (($#)); then extra="$(printf '%s\n' "$@")"$'\n'; fi
  cat <<DESK
[Desktop Entry]
Type=Application
Name=Lillith
GenericName=Desktop companion
Comment=Your desktop demon: a local-only animated companion
Exec="$BIN_DIR/lillith"
Path=$DATA_DIR
Icon=lillith
Terminal=false
Categories=Utility;
Keywords=avatar;companion;pet;mascot;
StartupNotify=false
Actions=settings;
${extra}$MARK

[Desktop Action settings]
Name=Lillith Settings
Exec="$BIN_DIR/lillith-settings"
DESK
}
mkdir -p "$(dirname "$APP_FILE")"
desktop_entry > "$APP_FILE"
ok "app menu entry $APP_FILE"

# QuantumSec / Lillith user units (install only — never enable here).
SYSTEMD_USER_DIR="$CONFIG_HOME/systemd/user"
mkdir -p "$SYSTEMD_USER_DIR"
for unit in lillith.service lillith-quantumsec.service; do
  if [[ -f $HERE/systemd/$unit ]]; then
    install -Dm644 "$HERE/systemd/$unit" "$SYSTEMD_USER_DIR/$unit"
    ok "user unit $SYSTEMD_USER_DIR/$unit (not enabled; enable with: systemctl --user enable --now $unit)"
  fi
done
# Ensure Biotek data home skeleton exists (additive).
mkdir -p "$HOME/.biotek"/{config,logs,memory,reports,bots/registry,bots/swarm,consent,quantumsec,teams/red,teams/blue,teams/purple}
ok "Biotek data home ~/.biotek (layout)"

# ── 5. desktop shortcut + autostart ─────────────────────────────────
step "5/8  Shortcuts"
if [[ -z $DESKTOP_SHORTCUT ]]; then
  if ask "Put a Lillith icon on the desktop?" y; then DESKTOP_SHORTCUT=1; else DESKTOP_SHORTCUT=0; fi
fi
DESK="$(desktop_dir)/lillith.desktop"
if [[ $DESKTOP_SHORTCUT == 1 ]]; then
  mkdir -p "$(dirname "$DESK")"
  desktop_entry > "$DESK"
  chmod +x "$DESK"   # Plasma: an executable .desktop on the desktop is trusted
  if have gio; then gio set "$DESK" metadata::trusted true >/dev/null 2>&1 || true; fi
  ok "desktop shortcut $DESK"
elif [[ -f $DESK ]] && grep -q "^$MARK" "$DESK"; then
  rm -f -- "$DESK"; ok "removed the desktop shortcut"
fi

if [[ -z $AUTOSTART ]]; then
  if ask "Start Lillith automatically when you log in?" n; then AUTOSTART=1; else AUTOSTART=0; fi
fi
if [[ $AUTOSTART == 1 ]]; then
  mkdir -p "$(dirname "$AUTOSTART_FILE")"
  desktop_entry "X-GNOME-Autostart-enabled=true" "X-KDE-autostart-after=panel" > "$AUTOSTART_FILE"
  ok "autostart $AUTOSTART_FILE"
elif [[ -f $AUTOSTART_FILE ]] && grep -q 'lillith' "$AUTOSTART_FILE"; then
  rm -f -- "$AUTOSTART_FILE"; ok "autostart off (removed $AUTOSTART_FILE)"
else
  ok "autostart off"
fi
refresh_menus

# ── 6. KWin bridge ──────────────────────────────────────────────────
step "6/8  KWin bridge (window perching on Plasma 6)"
if [[ $KWIN == 0 ]]; then
  ok "skipped (--no-kwin)"
elif ! have kpackagetool6 || ! have kwriteconfig6; then
  warn "not Plasma 6 (no kpackagetool6/kwriteconfig6): skipped; she lands on the screen bottom instead of windows"
else
  if kpackagetool6 --type=KWin/Script --show "$KWIN_ID" >/dev/null 2>&1; then
    kpackagetool6 --type=KWin/Script --upgrade "$ROOT/kwin/lillith-bridge" >/dev/null
    # KWin keeps the old copy loaded until it's toggled.
    kwriteconfig6 --file kwinrc --group Plugins --key "${KWIN_ID}Enabled" false
    kwin_reconfigure
    ok "upgraded the KWin script"
  else
    kpackagetool6 --type=KWin/Script --install "$ROOT/kwin/lillith-bridge" >/dev/null
    ok "installed the KWin script"
  fi
  kwriteconfig6 --file kwinrc --group Plugins --key "${KWIN_ID}Enabled" true
  kwin_reconfigure
  if have qdbus6 && [[ "$(qdbus6 org.kde.KWin /Scripting org.kde.kwin.Scripting.isScriptLoaded "$KWIN_ID" 2>/dev/null)" == true ]]; then
    ok "enabled and loaded"
  else
    ok "enabled (loads with KWin; check: System Settings > Window Management > KWin Scripts)"
  fi
fi

# KDE force blur (Better Blur DX on Garuda, Better Blur): it draws a dark,
# blurred, rounded panel behind every window it targets, including her
# transparent one (the "black box"). Exclude only her window class; the
# effect stays on for everything else. Undone by --uninstall.
if have kreadconfig6 && have kwriteconfig6; then
  if ! "$HERE/kwin-blur-exclude.sh" check >/dev/null 2>&1; then
    "$HERE/kwin-blur-exclude.sh" check || true
    if ask "Exclude Lillith from the blur effect (fixes the dark box around her; other windows keep their blur)?" y; then
      "$HERE/kwin-blur-exclude.sh" apply
      if "$HERE/kwin-blur-exclude.sh" check >/dev/null 2>&1; then
        ok "Lillith excluded from force blur (undo: ./linux/kwin-blur-exclude.sh revert)"
      elif ask "Her class can't be excluded automatically (a /regex/ matches it). Turn the blur effect off instead (uninstall turns it back on)?" y; then
        "$HERE/kwin-blur-exclude.sh" disable
        ok "blur effect turned off (undo: ./linux/kwin-blur-exclude.sh revert)"
      else
        warn "edit the window classes in System Settings → Desktop Effects → Better Blur DX"
      fi
    else
      warn "left as is; fix later with: ./linux/kwin-blur-exclude.sh apply   (or: lillith fix-kwin)"
    fi
  else
    ok "no KDE force-blur effect targets her"
  fi
fi

# ── 7. local brain ──────────────────────────────────────────────────
step "7/8  Lily's brain (a local GGUF model; nothing is downloaded)"
LILLITH="$BIN_DIR/lillith"
HAS_SETUP=0
"$LILLITH" setup --help >/dev/null 2>&1 && HAS_SETUP=1

find_gguf() {
  # Ranked by gguf_rank (below).
  { for d in "$ROOT/bundle/models" "$ROOT/../bundle/models"; do
      [[ -d $d ]] && find "$d" -maxdepth 4 -type f -iname '*.gguf' ! -iname '*mmproj*' -print 2>/dev/null
    done
    find "$HOME" -xdev -maxdepth 6 \
      \( -path "$HOME/.local/share/Trash" -o -path "$HOME/.cargo" -o -path "$HOME/.rustup" \
         -o -name node_modules -o -name .git \) -prune -o \
      -type f -iname '*.gguf' ! -iname '*mmproj*' -size +50M -print 2>/dev/null
    find /usr/share /opt -maxdepth 4 -type f -iname '*.gguf' -size +50M -print 2>/dev/null
  } | awk '!seen[$0]++' | gguf_rank | head -n 12
}
# Same order as `lillith setup --auto-gguf` (crates/lillith-setup/src/pick.rs):
# ~/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf, then Dolphin models that fit
# in RAM, then the smallest that fits (size + 3 GiB <= RAM), then the rest,
# smallest first; models too big for RAM are marked so they're never a silent pick.
ram_bytes() { [[ -n ${LILLITH_RAM_BYTES:-} ]] && { echo "$LILLITH_RAM_BYTES"; return; }
  awk '/^MemTotal:/ { printf "%.0f", $2 * 1024 }' /proc/meminfo 2>/dev/null || echo 0; }
gguf_rank() {
  local ram pref f
  ram="$(ram_bytes)"; [[ $ram =~ ^[0-9]+$ && $ram -gt 0 ]] || ram=$((8 * 1024 ** 3))
  pref="$HOME/AI/models/dolphin3.0-llama3.2-3b-q4_k_m.gguf"
  while IFS= read -r f; do
    printf '%s\t%s\n' "$(stat -Lc %s "$f" 2>/dev/null || echo 0)" "$f"
  done | awk -F'\t' -v ram="$ram" -v pref="$pref" -v head=$((3 * 1024 ** 3)) '{
      n = tolower($2); sub(/.*\//, "", n); fit = ($1 + head <= ram)
      r = ($2 == pref) ? 0 : (fit && n ~ /dolphin/) ? 1 : fit ? 2 : 3
      printf "%d\t%020.0f\t%s\n", r, $1, $2 }' |
    sort -t$'\t' -k1,1n -k2,2 -k3 | cut -f3-
}
gguf_too_big() {
  local ram size
  ram="$(ram_bytes)"; [[ $ram =~ ^[0-9]+$ && $ram -gt 0 ]] || return 1
  size="$(stat -Lc %s "$1" 2>/dev/null)" || return 1
  ((size + 3 * 1024 ** 3 > ram))
}
find_llama() {
  {
    # The bundled one first (all-in-one download), then whatever is installed.
    if [[ -x $LLAMA_DIR/linux-x64/llama-server ]]; then
      echo "$LLAMA_DIR/linux-x64/llama-server"
    elif [[ -n $BUNDLE_LLAMA ]]; then
      echo "$BUNDLE_LLAMA/linux-x64/llama-server"
    fi
    command -v llama-server 2>/dev/null || true
    for p in /usr/bin/llama-server /usr/local/bin/llama-server "$HOME/.local/bin/llama-server"; do
      [[ -x $p ]] && echo "$p"
    done
    find "$HOME" -xdev -maxdepth 5 \( -path "$HOME/.local/share/Trash" -o -name .git \) -prune -o \
      -type f -name llama-server -perm -u+x -print 2>/dev/null
  } | awk '!seen[$0]++' | head -n 8
}

if [[ $HAS_SETUP == 0 ]]; then
  warn "this build has no 'lillith setup'; edit [llm] in $CONFIG_FILE by hand"
else
  if [[ -z $GGUF ]]; then
    printf '  looking for .gguf files under %s and /usr/share, /opt ...\n' "$HOME"
    mapfile -t ggufs < <(find_gguf)
    if ((${#ggufs[@]})); then
      GGUF="$(choose "Which model should Lily use?" "${ggufs[@]}")"
      if [[ -n $GGUF ]] && gguf_too_big "$GGUF"; then
        warn "$GGUF is too big for this computer's RAM ($(($(ram_bytes) / 1024 ** 2)) MiB; it needs the model + ~3 GB free)."
        warn "Lily may be very slow or fail to start; a ~3B Q4_K_M model such as dolphin3.0-llama3.2-3b-q4_k_m.gguf fits better."
      fi
    else
      warn "no .gguf model found. Copy one onto this computer (e.g. Dolphin 3.0 Llama 3.2 3B Q4_K_M)"
      warn "and run: lillith setup --gguf /path/to/model.gguf --llama-server /usr/bin/llama-server"
    fi
  fi
  # The all-in-one zip carries its own llama-server (bundle/llama/, copied to
  # $LLAMA_DIR above): use that copy; otherwise pick the system one.
  if [[ -z $LLAMA && -x $LLAMA_DIR/linux-x64/llama-server ]]; then
    LLAMA="$LLAMA_DIR/linux-x64/llama-server"
    ok "using the bundled llama-server ($LLAMA)"
  fi
  if [[ -z $LLAMA ]]; then
    mapfile -t servers < <(find_llama)
    if ((${#servers[@]})); then
      LLAMA="$(choose "Which llama-server?" "${servers[@]}")"
    else
      warn "llama-server isn't installed. It comes from llama.cpp:"
      warn "  sudo pacman -S llama-cpp    (Arch 'extra' repo; Garuda has it too)"
      warn "  or the AUR: paru -S llama.cpp   (this installer never installs from the AUR)"
      LLAMA_PKG=""
      if have pacman && [[ $SKIP_DEPS == 0 ]]; then
        for p in llama-cpp llama.cpp; do
          if pacman -Si "$p" >/dev/null 2>&1; then LLAMA_PKG="$p"; break; fi
        done
      fi
      if [[ -n $LLAMA_PKG ]]; then
        if ask "$LLAMA_PKG is in your pacman repos. Install it now?" n; then
          sudo pacman -S --needed "$LLAMA_PKG"
          mapfile -t servers < <(find_llama)
          ((${#servers[@]})) && LLAMA="${servers[0]}"
        fi
      fi
    fi
  fi
  SETUP_ARGS=(setup)
  [[ -n $GGUF ]] && SETUP_ARGS+=(--gguf "$GGUF")
  [[ -n $LLAMA ]] && SETUP_ARGS+=(--llama-server "$LLAMA")
  [[ -n $CTX ]] && SETUP_ARGS+=(--ctx-size "$CTX")
  # Also creates config.toml (commented defaults) when it doesn't exist yet.
  if RUST_LOG=warn "$LILLITH" "${SETUP_ARGS[@]}" | sed 's/^/  /'; then
    ok "config $CONFIG_FILE (endpoint stays http://127.0.0.1:8080, loopback only)"
  else
    warn "lillith setup failed; Lillith still runs, just without chat. Retry: lillith ${SETUP_ARGS[*]}"
  fi
fi
if [[ $ICON_SAVE == 1 ]]; then
  if "$LILLITH" --config "$CONFIG_FILE" icon "$ICON" --no-apply >/dev/null 2>&1; then
    ok "icon = \"$ICON\" saved to $CONFIG_FILE"
  else
    warn "couldn't save the icon choice; run: lillith icon $ICON"
  fi
fi
if [[ -f $CONFIG_FILE ]]; then
  # Keep the Settings checkbox in step with the autostart entry.
  want=$([[ $AUTOSTART == 1 ]] && echo true || echo false)
  sed -i -E "s/^autostart = (true|false)\$/autostart = $want/" "$CONFIG_FILE"
fi

# Her skills: a `skills` / `lillith-skills` folder on your desktop can be
# COPIED into her skills folder. You review the exact plan first (every copy,
# its destination, any "(from desktop)" rename) and approve all, some, or
# none. Hers are never overwritten and the original is never touched. She
# also proposes this at start ([mind.access] import_desktop_skills).
desk="$(desktop_dir)"
skills_src=""
for n in skills Skills lillith-skills Lillith-skills lillith_skills "Lillith Skills" lily-skills; do
  if [[ -d $desk/$n ]]; then skills_src="$desk/$n"; break; fi
done
if [[ -n $skills_src ]]; then
  if [[ -t 0 && -t 1 ]]; then
    if RUST_LOG=warn "$LILLITH" --config "$CONFIG_FILE" import-skills --from "$skills_src"; then
      ok "skills import reviewed ($skills_src)"
    else
      warn "couldn't review $skills_src; she'll propose it when she starts"
    fi
  else
    ok "found $skills_src; she'll show you the import plan when you chat (or run: lillith import-skills)"
  fi
fi

# ── 7b. voice (offline, from the bundled kit) ───────────────────────
# The all-in-one zip ships the voice kit's models/ and wheels/ inside
# tools/lillith-voice/. Its own installer makes a venv and pip-installs
# from those wheels only (--no-index); nothing is downloaded.
VKIT=""
for k in "$ROOT/tools/lillith-voice" "$ROOT/../tools/lillith-voice"; do
  if [[ -f $k/install.sh && -f $k/models/kokoro-v1.0.onnx && -d $k/wheels ]]; then VKIT="$(cd "$k" && pwd)"; break; fi
done
if [[ $VOICE != 0 && -n $VKIT ]]; then
  step "7b/8 Her voice (offline kit bundled in this zip; nothing is downloaded)"
  VPY=""
  # The bundled wheels are for CPython 3.13 and 3.14.
  for c in python3 python3.14 python3.13; do
    have "$c" || continue
    v="$("$c" -c 'import sys;print(f"{sys.version_info[0]}{sys.version_info[1]}")' 2>/dev/null || true)"
    if [[ $v == 313 || $v == 314 ]] && compgen -G "$VKIT/wheels/*-cp$v-*.whl" >/dev/null; then
      VPY="$(command -v "$c")"; break
    fi
  done
  if [[ -z $VPY ]]; then
    warn "her voice needs Python 3.13 or 3.14 (the bundled wheels are cp313/cp314);"
    warn "found: $(python3 --version 2>&1 || echo none). Continuing without voice."
    warn "Later: sudo pacman -S python, then: bash $VKIT/install.sh --python python3"
  elif ask "Set up her voice now (offline; about 1 GB in ~/.local/share/lillith-voice)?" y; then
    if bash "$VKIT/install.sh" --wheels "$VKIT/wheels" --python "$VPY" --systemd 2>&1 | sed 's/^/  /'; [[ ${PIPESTATUS[0]} == 0 ]]; then
      ok "voice installed (models: ${XDG_DATA_HOME:-$HOME/.local/share}/lillith-voice/models)"
      if [[ -f $CONFIG_FILE ]] && ! grep -q '^\[voice\]' "$CONFIG_FILE"; then
        printf '\n[voice]\nengine = "daemon"   # the offline voice kit (lillith-voice.service)\npreset = "lillith"\n' >> "$CONFIG_FILE"
        ok "[voice] engine = \"daemon\" saved to $CONFIG_FILE"
      fi
    else
      warn "voice setup failed (see above); Lillith still runs without voice. Retry: bash $VKIT/install.sh"
    fi
  fi
fi

# ── 8. check + start ────────────────────────────────────────────────
step "8/8  Checking the install"
"$LILLITH" --help >/dev/null || die "$LILLITH --help failed"
ok "lillith --help runs"
if [[ $ASSET_STATE != lfs ]]; then
  # The list ends with an "N clips" line (after a header row): use that number.
  n="$(cd "$DATA_DIR" && "$LILLITH" preview --list 2>/dev/null | awk '/^[0-9]+ clips$/ { n = $1 } END { print n + 0 }')" || n=0
  if [[ $n -gt 0 ]]; then ok "her animation set loads ($n clips)"; else warn "lillith preview --list found no clips"; fi
fi
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) warn "$BIN_DIR isn't on your PATH. fish: fish_add_path $BIN_DIR   bash: export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac

if [[ -z $START ]]; then
  if [[ $YES == 1 ]]; then START=0
  elif ask "Start Lillith now?" y; then START=1; else START=0; fi
fi
if [[ $START == 1 ]]; then
  if [[ -z ${WAYLAND_DISPLAY:-} && -z ${DISPLAY:-} ]]; then
    warn "no graphical session here; start her from the app menu instead"
  else
    if running; then pkill -x lillith || true; sleep 1; fi
    (cd "$DATA_DIR" && nohup "$LILLITH" >/dev/null 2>&1 &)
    ok "started. Right-click her for the menu; drag to move her."
  fi
fi

step "Done."
cat <<DONE
  Start:      Lillith in the app menu, the desktop icon, or 'lillith'
  Settings:   right-click her > Settings, or 'lillith-settings'
  Icon:       right-click her > Icon…, or 'lillith icon --list' / 'lillith icon ID'
  Model:      lillith setup --gguf FILE.gguf --llama-server /usr/bin/llama-server
  Skills:     drop a 'skills' folder on your desktop; she copies it in at start
              (or: lillith import-skills). Undo her tidying: lillith undo-organize
  Uninstall:  bash install.sh --uninstall   (keeps GGUF models; asks about settings)
DONE
