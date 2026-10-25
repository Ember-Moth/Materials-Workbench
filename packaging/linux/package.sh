#!/bin/sh
# Packages the Linux release (docs/design/release.md §2.1, §5):
#
#   materials-workbench_<version>_amd64.deb            Debian and Ubuntu
#   materials-workbench-<version>-linux-x86_64.tar.gz  portable, with install.sh
#   mw-<version>-linux-x86_64.tar.gz                   the command line alone
#
#   packaging/linux/package.sh [OUT_DIR]      default: target/packages
#
# Runs on Debian or Ubuntu x86_64 with the build dependencies of
# docs/development.md §1, dpkg-dev (dpkg-shlibdeps) and cargo-about (see
# packaging/common.sh). Releases are built on the oldest Ubuntu supported,
# so that the executables run on as many distributions as possible.
set -eu
. "$(dirname "$0")/../common.sh"

OUT=${1:-target/packages}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
STAGE="$ROOT/target/packaging/linux"
rm -rf "$STAGE"
mkdir -p "$STAGE"
TARGET=x86_64-unknown-linux-gnu
NAME=linux-x86_64
ID=io.github.ember_moth.MaterialsWorkbench
SIZES="16 24 32 48 64 128 256 512"
# The .deb's maintainer (Debian policy §5.6.2: name and email address).
MAINTAINER=${MW_DEB_MAINTAINER:-"Materials Workbench <https://github.com/Ember-Moth/Materials-Workbench/issues>"}

echo "Materials Workbench $VERSION, $NAME"

cargo build --release --locked -p workbench -p mw-cli --bins --target "$TARGET"
BIN="target/$TARGET/release"
third_party_licenses "$STAGE/THIRD-PARTY-LICENSES.html" "$TARGET"

# stage ROOT PREFIX: the files under ROOT/PREFIX (bin/, share/).
stage() {
    prefix="$1/$2"
    install -Dm755 "$BIN/materials-workbench" "$prefix/bin/materials-workbench"
    install -Dm755 "$BIN/mw" "$prefix/bin/mw"
    install -Dm644 "packaging/linux/$ID.desktop" "$prefix/share/applications/$ID.desktop"
    for size in $SIZES; do
        install -Dm644 "packaging/icons/linux/${size}x$size.png" \
            "$prefix/share/icons/hicolor/${size}x$size/apps/$ID.png"
    done
    install -Dm644 packaging/icons/materials-workbench.svg \
        "$prefix/share/icons/hicolor/scalable/apps/$ID.svg"
}

# The Debian package: the files under /usr, the copyright file and the
# licenses in its documentation folder.
DEB="$STAGE/deb"
stage "$DEB" usr
DOC="$DEB/usr/share/doc/materials-workbench"
install -Dm644 "$STAGE/THIRD-PARTY-LICENSES.html" "$DOC/THIRD-PARTY-LICENSES.html"
{
    cat <<EOF
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: Materials Workbench
Source: https://github.com/Ember-Moth/Materials-Workbench
Comment: The licenses of the Rust crates compiled in are in
 /usr/share/doc/materials-workbench/THIRD-PARTY-LICENSES.html.

Files: *
Copyright: Materials Workbench contributors
License: MIT or Apache-2.0

License: MIT
EOF
    sed -e 's/^$/./' -e 's/^/ /' LICENSE-MIT
    printf '\nLicense: Apache-2.0\n On Debian systems, the full text of the Apache License, Version 2.0 is in\n /usr/share/common-licenses/Apache-2.0.\n'
} >"$DOC/copyright"
chmod 644 "$DOC/copyright"

# The libraries the executables link (dpkg-shlibdeps reads them), and the
# Vulkan loader, which wgpu opens only when it runs.
mkdir -p "$STAGE/shlibdeps/debian"
printf 'Source: materials-workbench\n\nPackage: materials-workbench\nArchitecture: amd64\n' \
    >"$STAGE/shlibdeps/debian/control"
LIBS=$(cd "$STAGE/shlibdeps" &&
    dpkg-shlibdeps -O "$DEB/usr/bin/materials-workbench" "$DEB/usr/bin/mw" |
    sed -n 's/^shlibs:Depends=//p')
[ -n "$LIBS" ] || die "dpkg-shlibdeps found no libraries"
mkdir -p "$DEB/DEBIAN"
cat >"$DEB/DEBIAN/control" <<EOF
Package: materials-workbench
Version: $VERSION
Architecture: amd64
Maintainer: $MAINTAINER
Installed-Size: $(du -sk "$DEB/usr" | cut -f1)
Depends: $LIBS, libvulkan1
Recommends: mesa-vulkan-drivers
Section: science
Priority: optional
Homepage: https://github.com/Ember-Moth/Materials-Workbench
Description: Materials science workbench for crystal structures
 Opens CIF and POSCAR files, shows crystal structures in 3D with their
 symmetry, bonds, coordination polyhedra, lattice planes and displacement
 ellipsoids, edits them in symmetry or P1 mode with undo, and exports CIF,
 POSCAR and extended XYZ. Structures are kept in projects. Includes the
 command line tool mw and an MCP server for AI assistants.
EOF
DEB_FILE="$OUT/materials-workbench_${VERSION}_amd64.deb"
dpkg-deb --root-owner-group --build "$DEB" "$DEB_FILE"

# The portable archive: the same files under one folder, with the licenses
# and install.sh, which installs them for the current user.
PORTABLE="materials-workbench-$VERSION-$NAME"
stage "$STAGE" "$PORTABLE"
for file in LICENSE-MIT LICENSE-APACHE "$STAGE/THIRD-PARTY-LICENSES.html"; do
    install -Dm644 "$file" "$STAGE/$PORTABLE/share/doc/materials-workbench/$(basename "$file")"
done
install -Dm755 packaging/linux/install.sh "$STAGE/$PORTABLE/install.sh"
tar -czf "$OUT/$PORTABLE.tar.gz" --owner=0 --group=0 -C "$STAGE" "$PORTABLE"

# The command line alone.
CLI="mw-$VERSION-$NAME"
mkdir -p "$STAGE/$CLI"
install -m755 "$BIN/mw" "$STAGE/$CLI/mw"
install -m644 LICENSE-MIT LICENSE-APACHE "$STAGE/THIRD-PARTY-LICENSES.html" "$STAGE/$CLI/"
tar -czf "$OUT/$CLI.tar.gz" --owner=0 --group=0 -C "$STAGE" "$CLI"

echo "$DEB_FILE"
echo "$OUT/$PORTABLE.tar.gz"
echo "$OUT/$CLI.tar.gz"
