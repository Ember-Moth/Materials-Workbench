#!/bin/sh
# Installs Materials Workbench for the current user from the folder this
# script is in, the unpacked materials-workbench-<version>-linux-x86_64.tar.gz
# (docs/design/release.md §5):
#
#   ~/.local/bin/                     materials-workbench, mw
#   $XDG_DATA_HOME/applications/      the launcher (~/.local/share/…)
#   $XDG_DATA_HOME/icons/hicolor/     its icon
#   $XDG_DATA_HOME/doc/materials-workbench/   the licenses
#
#   sh install.sh               installs, or upgrades an earlier installation
#   sh install.sh --uninstall   removes these files; settings and projects stay
#
# Check the archive against the release's SHA256SUMS before unpacking it:
#   sha256sum --check --ignore-missing SHA256SUMS
# Needs no root and nothing but a POSIX shell.
set -eu

ID=io.github.ember_moth.MaterialsWorkbench
HERE=$(cd "$(dirname "$0")" && pwd)
BIN_DIR="$HOME/.local/bin"
DATA_DIR=${XDG_DATA_HOME:-$HOME/.local/share}
case $DATA_DIR in
/*) ;;
*)
    echo "install.sh: XDG_DATA_HOME must be an absolute path" >&2
    exit 1
    ;;
esac
APPS_DIR="$DATA_DIR/applications"
ICONS_DIR="$DATA_DIR/icons/hicolor"
DOC_DIR="$DATA_DIR/doc/materials-workbench"
SIZES="16 24 32 48 64 128 256 512"

# Tells desktops that launchers and icons changed; not every desktop has
# these tools, nor needs them.
refresh() {
    update-desktop-database -q "$APPS_DIR" 2>/dev/null || true
    gtk-update-icon-cache -q -t "$ICONS_DIR" 2>/dev/null || true
}

if [ "${1:-}" = --uninstall ]; then
    rm -f "$BIN_DIR/materials-workbench" "$BIN_DIR/mw" "$APPS_DIR/$ID.desktop" \
        "$ICONS_DIR/scalable/apps/$ID.svg"
    for size in $SIZES; do
        rm -f "$ICONS_DIR/${size}x$size/apps/$ID.png"
    done
    rm -rf "$DOC_DIR"
    refresh
    echo "Materials Workbench removed; settings and projects are where they were."
    exit 0
elif [ $# -gt 0 ]; then
    sed -n '2,/^set -eu/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//' >&2
    exit 2
fi

for file in bin/materials-workbench bin/mw "share/applications/$ID.desktop"; do
    [ -f "$HERE/$file" ] || {
        echo "install.sh: $HERE/$file is missing; is this the unpacked archive?" >&2
        exit 1
    }
done

# put SOURCE DESTINATION MODE: copies under a temporary name first, so that
# a running copy or an interrupted installation never sees half a file.
put() {
    mkdir -p "$(dirname "$2")"
    cp "$1" "$2.installing"
    chmod "$3" "$2.installing"
    mv -f "$2.installing" "$2"
}

put "$HERE/bin/materials-workbench" "$BIN_DIR/materials-workbench" 755
put "$HERE/bin/mw" "$BIN_DIR/mw" 755
for size in $SIZES; do
    put "$HERE/share/icons/hicolor/${size}x$size/apps/$ID.png" \
        "$ICONS_DIR/${size}x$size/apps/$ID.png" 644
done
put "$HERE/share/icons/hicolor/scalable/apps/$ID.svg" "$ICONS_DIR/scalable/apps/$ID.svg" 644
for file in LICENSE-MIT LICENSE-APACHE THIRD-PARTY-LICENSES.html; do
    put "$HERE/share/doc/materials-workbench/$file" "$DOC_DIR/$file" 644
done

# The launcher names the executable by its absolute path: the desktop's
# PATH need not contain ~/.local/bin. Quoted as the Desktop Entry
# specification asks, twice: for the command line, then for the file.
exec_path=$(printf '%s' "$BIN_DIR/materials-workbench" |
    sed -e 's/\\/\\\\\\\\/g' -e 's/"/\\\\"/g' -e 's/`/\\\\`/g' -e 's/\$/\\\\$/g' -e 's/%/%%/g')
replacement=$(printf '%s' "$exec_path" | sed -e 's/[|&\\]/\\&/g')
mkdir -p "$APPS_DIR"
sed -e "s|^Exec=materials-workbench |Exec=\"$replacement\" |" \
    "$HERE/share/applications/$ID.desktop" >"$APPS_DIR/$ID.desktop.installing"
chmod 644 "$APPS_DIR/$ID.desktop.installing"
mv -f "$APPS_DIR/$ID.desktop.installing" "$APPS_DIR/$ID.desktop"
refresh

echo "Materials Workbench installed: $BIN_DIR/materials-workbench"
case :$PATH: in
*":$BIN_DIR:"*) ;;
*) echo "$BIN_DIR is not on PATH; the launcher works without it, mw on the command line does not." ;;
esac
