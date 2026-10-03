#!/bin/sh
# Builds dist/SteamIDfinder-<version>-<arch>.AppImage from target/release/steamidfinder.
# Downloads appimagetool into .cache/ on first use.
set -eu
cd "$(dirname "$0")/.."
version=${1:?usage: scripts/build-appimage.sh VERSION}
arch=$(uname -m)
tool=.cache/appimagetool-$arch.AppImage
appdir=dist/SteamIDfinder.AppDir

if [ ! -x "$tool" ]; then
    mkdir -p .cache
    curl -fsSL -o "$tool" \
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$arch.AppImage"
    chmod +x "$tool"
fi

rm -rf "$appdir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/applications" \
    "$appdir/usr/share/icons/hicolor/scalable/apps" "$appdir/usr/share/icons/hicolor/256x256/apps"
install -m 755 target/release/steamidfinder "$appdir/usr/bin/steamidfinder"
install -m 644 assets/steamidfinder.desktop "$appdir/usr/share/applications/"
install -m 644 assets/steamidfinder.desktop "$appdir/"
install -m 644 assets/steamidfinder.svg "$appdir/usr/share/icons/hicolor/scalable/apps/"
install -m 644 assets/icon-256.png "$appdir/usr/share/icons/hicolor/256x256/apps/steamidfinder.png"
install -m 644 assets/steamidfinder.svg "$appdir/steamidfinder.svg"
install -m 644 assets/icon-256.png "$appdir/.DirIcon"
ln -s usr/bin/steamidfinder "$appdir/AppRun"

out=dist/SteamIDfinder-$version-$arch.AppImage
# Extract-and-run avoids needing FUSE (CI runners don't have it).
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=$arch "$tool" --no-appstream "$appdir" "$out"
rm -rf "$appdir"
echo "Wrote $out"
