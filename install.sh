#!/bin/sh
# SteamIDfinder installer: puts the binary, desktop entry and icon where app launchers
# (Noctalia, rofi, fuzzel, wofi, GNOME, KDE, …) look for them.
#
# Works from an unpacked release tarball (binary next to this script) or from a source
# checkout (builds with cargo if there's no release binary yet).
set -eu

usage() {
    cat <<'USAGE'
Usage: ./install.sh [--system | --prefix DIR] [--uninstall]

  (no options)   install for the current user under ~/.local
  --system       install for all users under /usr/local (uses sudo if needed)
  --prefix DIR   install under DIR instead
  --uninstall    remove an install (pass --system or --prefix again to match it)
USAGE
}

mode=install
system=0
prefix=
while [ $# -gt 0 ]; do
    case $1 in
        --system) system=1 ;;
        --prefix)
            [ $# -ge 2 ] || { usage >&2; exit 2; }
            prefix=$2
            shift
            ;;
        --prefix=*) prefix=${1#--prefix=} ;;
        --uninstall) mode=uninstall ;;
        -h | --help) usage; exit 0 ;;
        *) echo "install.sh: unknown option '$1'" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

if [ -n "$prefix" ]; then
    datadir=$prefix/share
elif [ "$system" = 1 ]; then
    prefix=/usr/local
    datadir=$prefix/share
else
    prefix=$HOME/.local
    datadir=${XDG_DATA_HOME:-$HOME/.local/share}
fi
bindir=$prefix/bin
appdir=$datadir/applications
icondir=$datadir/icons/hicolor

# The desktop entry quotes the binary path; these characters would need escaping there.
case $bindir in
    *[\"\`\$\\]*)
        echo "install.sh: the install path can't contain \", \`, \$ or \\: $bindir" >&2
        exit 1
        ;;
esac

# Use sudo only when the target isn't writable.
SUDO=
probe=$prefix
while [ ! -d "$probe" ]; do probe=$(dirname "$probe"); done
if [ ! -w "$probe" ] && [ "$(id -u)" != 0 ]; then
    command -v sudo >/dev/null 2>&1 || { echo "install.sh: $prefix isn't writable and sudo isn't available" >&2; exit 1; }
    SUDO=sudo
fi

here=$(cd "$(dirname "$0")" && pwd)

# Tarball layout keeps files next to this script; the source tree keeps them in assets/.
asset() {
    for candidate in "$here/$1" "$here/assets/$1"; do
        if [ -f "$candidate" ]; then
            echo "$candidate"
            return 0
        fi
    done
    echo "install.sh: can't find $1" >&2
    exit 1
}

refresh_caches() {
    if command -v update-desktop-database >/dev/null 2>&1; then
        $SUDO update-desktop-database -q "$appdir" 2>/dev/null || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -f "$icondir/index.theme" ]; then
        $SUDO gtk-update-icon-cache -q -t "$icondir" 2>/dev/null || true
    fi
}

if [ "$mode" = uninstall ]; then
    $SUDO rm -f "$bindir/steamidfinder" \
        "$appdir/steamidfinder.desktop" \
        "$icondir/scalable/apps/steamidfinder.svg" \
        "$icondir/256x256/apps/steamidfinder.png"
    refresh_caches
    echo "Removed SteamIDfinder from $prefix."
    echo "Lookup history is kept in ${XDG_DATA_HOME:-$HOME/.local/share}/steamidfinder; delete it by hand if you want it gone."
    exit 0
fi

binary=
for candidate in "$here/steamidfinder" "$here/target/release/steamidfinder"; do
    if [ -f "$candidate" ] && [ -x "$candidate" ]; then
        binary=$candidate
        break
    fi
done
if [ -z "$binary" ]; then
    if [ -f "$here/Cargo.toml" ] && command -v cargo >/dev/null 2>&1; then
        echo "No release binary yet; building one with cargo..."
        (cd "$here" && cargo build --release --locked)
        binary=$here/target/release/steamidfinder
    else
        echo "install.sh: no steamidfinder binary next to this script and no cargo to build one" >&2
        exit 1
    fi
fi

# Find every file before touching the system, so a broken tarball installs nothing.
desktop_file=$(asset steamidfinder.desktop) || exit 1
svg_icon=$(asset steamidfinder.svg) || exit 1
png_icon=$(asset icon-256.png) || exit 1

$SUDO mkdir -p "$bindir" "$appdir" "$icondir/scalable/apps" "$icondir/256x256/apps"
$SUDO install -m 755 "$binary" "$bindir/steamidfinder"
$SUDO install -m 644 "$svg_icon" "$icondir/scalable/apps/steamidfinder.svg"
$SUDO install -m 644 "$png_icon" "$icondir/256x256/apps/steamidfinder.png"
# Launchers don't always inherit a PATH containing ~/.local/bin, so point Exec at the binary.
tmp_desktop=$(mktemp)
STEAMIDFINDER_BIN="$bindir/steamidfinder" awk '
    $0 == "Exec=steamidfinder" { print "Exec=\"" ENVIRON["STEAMIDFINDER_BIN"] "\""; next }
    { print }
' "$desktop_file" >"$tmp_desktop"
$SUDO install -m 644 "$tmp_desktop" "$appdir/steamidfinder.desktop"
rm -f "$tmp_desktop"
refresh_caches

echo "Installed SteamIDfinder: $bindir/steamidfinder"
echo "It should now show up in your app launcher as \"SteamIDfinder\"."
case ":$PATH:" in
    *":$bindir:"*) ;;
    *) echo "Note: $bindir isn't on your PATH. The launcher entry works anyway; add it to PATH to run steamidfinder from a terminal." ;;
esac
