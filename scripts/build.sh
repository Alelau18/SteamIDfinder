#!/bin/sh
# Local Linux build: runs the tests, builds the release binary and, optionally, the release
# tarball and AppImage into dist/. Windows builds come from CI.
set -eu

usage() { echo "Usage: scripts/build.sh [--dist] [--appimage] [--all]"; }

dist=0
appimage=0
for arg in "$@"; do
    case $arg in
        --dist) dist=1 ;;
        --appimage) appimage=1 ;;
        --all) dist=1; appimage=1 ;;
        -h | --help) usage; exit 0 ;;
        *) echo "build.sh: unknown option '$arg'" >&2; usage >&2; exit 2 ;;
    esac
done

cd "$(dirname "$0")/.."
cargo test --locked
cargo build --release --locked
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
echo "Built target/release/steamidfinder ($version)"

if [ "$dist" = 1 ]; then
    scripts/package-linux.sh "$version"
fi
if [ "$appimage" = 1 ]; then
    scripts/build-appimage.sh "$version"
fi
