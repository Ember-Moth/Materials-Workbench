#!/bin/sh
# Packages the macOS release (docs/design/release.md §2.1, §3):
#
#   Materials-Workbench-<version>-macos-universal.dmg   the application
#   mw-<version>-macos-universal.tar.gz                 the command line alone
#
# Both executables are universal (Apple silicon and Intel). Signed ad hoc:
# Developer ID signing and notarization wait for a decision (§3.4, §10).
#
#   packaging/macos/package.sh [OUT_DIR]      default: target/packages
#
# Needs Xcode's command line tools, the Rust targets aarch64-apple-darwin and
# x86_64-apple-darwin, and cargo-about (see packaging/common.sh).
set -eu
. "$(dirname "$0")/../common.sh"

OUT=${1:-target/packages}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
STAGE="$ROOT/target/packaging/macos"
rm -rf "$STAGE"
mkdir -p "$STAGE"
TARGETS="aarch64-apple-darwin x86_64-apple-darwin"
NAME="macos-universal"

echo "Materials Workbench $VERSION ($BUILD), $NAME"

# Rust's oldest target for Apple silicon is macOS 11; the application says
# the same in Info.plist.
export MACOSX_DEPLOYMENT_TARGET=11.0
targets=
for target in $TARGETS; do
    targets="$targets --target $target"
done
# shellcheck disable=SC2086
cargo build --release --locked -p workbench -p mw-cli --bins $targets

for bin in materials-workbench mw; do
    slices=
    for target in $TARGETS; do
        slices="$slices target/$target/release/$bin"
    done
    # shellcheck disable=SC2086
    lipo -create $slices -output "$STAGE/$bin"
done
# shellcheck disable=SC2086
third_party_licenses "$STAGE/THIRD-PARTY-LICENSES.html" $TARGETS

# The application bundle (§3.1).
APP="$STAGE/Materials Workbench.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$STAGE/materials-workbench" "$STAGE/mw" "$APP/Contents/MacOS/"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@BUILD@/$BUILD/g" \
    packaging/macos/Info.plist >"$APP/Contents/Info.plist"
plutil -lint -s "$APP/Contents/Info.plist"
cp packaging/icons/materials-workbench.icns "$APP/Contents/Resources/AppIcon.icns"
cp LICENSE-MIT LICENSE-APACHE "$STAGE/THIRD-PARTY-LICENSES.html" "$APP/Contents/Resources/"

# Signing, from the inside out. Ad hoc: without it Apple silicon calls the
# application damaged; with it, the first start still needs the user's
# permission in System Settings > Privacy & Security (§3.4).
#
# With a Developer ID (not set up, §10), sign here instead, each with
# `--options runtime --timestamp --sign "Developer ID Application: …"`,
# and notarize the disk image below.
codesign --force --sign - "$APP/Contents/MacOS/mw"
codesign --force --sign - "$APP"
codesign --verify --strict --deep "$APP"

# The disk image: the application and a link to /Applications (§3.5).
DMG_ROOT="$STAGE/dmg"
mkdir -p "$DMG_ROOT"
ditto "$APP" "$DMG_ROOT/Materials Workbench.app"
ln -s /Applications "$DMG_ROOT/Applications"
DMG="$OUT/Materials-Workbench-$VERSION-$NAME.dmg"
hdiutil create -quiet -ov -fs HFS+ -format UDZO \
    -volname "Materials Workbench $VERSION" -srcfolder "$DMG_ROOT" "$DMG"
# With a Developer ID: sign the image, then
#   xcrun notarytool submit "$DMG" --keychain-profile … --wait   (Accepted?)
#   xcrun stapler staple "$DMG" && xcrun stapler validate "$DMG"

# The command line alone (§2.1).
CLI="mw-$VERSION-$NAME"
mkdir -p "$STAGE/$CLI"
cp "$STAGE/mw" LICENSE-MIT LICENSE-APACHE "$STAGE/THIRD-PARTY-LICENSES.html" "$STAGE/$CLI/"
codesign --force --sign - "$STAGE/$CLI/mw"
tar -czf "$OUT/$CLI.tar.gz" -C "$STAGE" "$CLI"

echo "$DMG"
echo "$OUT/$CLI.tar.gz"
