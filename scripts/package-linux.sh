#!/bin/sh
# Packs target/release/steamidfinder with its desktop entry, icon and installer into
# dist/steamidfinder-<version>-linux-x86_64.tar.gz.
set -eu
cd "$(dirname "$0")/.."
version=${1:?usage: scripts/package-linux.sh VERSION}
arch=$(uname -m)
name=steamidfinder-$version-linux-$arch
stage=dist/$name

rm -rf "$stage"
mkdir -p "$stage"
install -m 755 target/release/steamidfinder "$stage/steamidfinder"
install -m 755 install.sh "$stage/install.sh"
install -m 644 assets/steamidfinder.desktop assets/steamidfinder.svg assets/icon-256.png \
    README.md LICENSE "$stage/"
tar -C dist -czf "dist/$name.tar.gz" "$name"
rm -rf "$stage"
echo "Wrote dist/$name.tar.gz"
