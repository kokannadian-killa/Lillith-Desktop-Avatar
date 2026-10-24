#!/usr/bin/env fish
# Launch Lillith from this portable package.
#   ./run.fish                    -> Wayland (KDE Plasma layer-shell overlay)
#   ./run.fish --backend x11      -> force the X11/XWayland fallback instead
#   ./run.fish --fix-kwin         -> first exclude her from KDE force blur
#                                    (Better Blur DX: the dark box), then launch
#   ./run.fish status | rules | preview --list | panel   (subcommands work too)
# Any extra arguments are passed straight to the binary.
# LILLITH_FIX_KWIN=1 does the same as --fix-kwin.
set -l pkg (dirname (realpath (status --current-filename)))
cd $pkg
set -gx LILLITH_MANIFEST $pkg/assets/generated/manifest.toml

# --fix-kwin is ours, not the binary's: take it out of the arguments.
set -l fix_kwin 0
set -q LILLITH_FIX_KWIN; and test "$LILLITH_FIX_KWIN" = 1; and set fix_kwin 1
if contains -- --fix-kwin $argv
    set fix_kwin 1
    set -l keep
    for a in $argv
        test "$a" = --fix-kwin; or set keep $keep $a
    end
    set argv $keep
end

# Launching her = no arguments, or options only (`--backend x11`);
# `status`, `rules`, `preview --list`, `setup` … are subcommands.
set -l launch 0
if test (count $argv) -eq 0; or string match -q -- '-*' $argv[1]
    set launch 1
end

# KDE force blur (Better Blur DX / Better Blur) paints a dark rounded panel
# behind her. With consent, exclude only her window class; otherwise check
# once (only when launching her) and print the fix.
if test $fix_kwin = 1
    $pkg/bin/lillith fix-kwin
else if test $launch = 1; and command -q kreadconfig6
    set -q XDG_STATE_HOME; and set -l statedir $XDG_STATE_HOME; or set -l statedir $HOME/.local/state
    set -l marker $statedir/lillith/kwin-blur-checked
    if not test -f $marker
        set -l report ($pkg/bin/lillith fix-kwin --check 2>/dev/null)
        if string match -q -r -- 'would exclude|regex' $report
            printf '%s\n' $report \
                "  -> This draws a dark box around Lillith. Fix (only her window is excluded):" \
                "       ./run.fish --fix-kwin    (or: ./kwin-blur-exclude.sh apply; undo: ./kwin-blur-exclude.sh revert)" \
                "  (shown once)" >&2
        end
        mkdir -p (dirname $marker); and touch $marker
    end
end

set -l extra
set -q XDG_CONFIG_HOME; and set -l cfgdir $XDG_CONFIG_HOME; or set -l cfgdir $HOME/.config
# Prefer a personal config if one exists, else the bundled one. `setup`
# always writes the personal config (seeded from the bundled one).
if not test -f $cfgdir/lillith/config.toml; and not contains -- --config $argv; and not contains -- setup $argv
    set extra $extra --config $pkg/config.toml
end
# Default to Wayland unless a backend was given or a subcommand is run.
if test $launch = 1
    if not contains -- --backend $argv; and set -q WAYLAND_DISPLAY
        set extra $extra --backend wayland   # use: ./run.fish --backend x11  for the X11 fallback
    end
end
exec $pkg/bin/lillith $extra $argv
