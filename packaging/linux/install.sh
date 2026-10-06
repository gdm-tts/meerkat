#!/bin/sh
# Install meerkat for the current user: binary, menu entry and icon.
#
#   ./install.sh               install into ~/.local (or $PREFIX)
#   ./install.sh --uninstall   remove what was installed
#
# The menu entry starts meerkat in your home directory, so it uses
# ~/meerkat.txt as repository list; edit the Exec= line of
# ~/.local/share/applications/meerkat.desktop to pass another list file.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-$HOME/.local}
bin="$prefix/bin/meerkat"
apps="$prefix/share/applications"
icons="$prefix/share/icons/hicolor/scalable/apps"

refresh() {
    update-desktop-database "$apps" >/dev/null 2>&1 || true
    gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" >/dev/null 2>&1 || true
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$bin" "$apps/meerkat.desktop" "$icons/meerkat.svg"
    refresh
    echo "meerkat removed from $prefix"
    exit 0
fi

mkdir -p "$(dirname "$bin")" "$apps" "$icons"
cp "$here/meerkat" "$bin"
chmod 755 "$bin"
cp "$here/meerkat.svg" "$icons/meerkat.svg"
chmod 644 "$icons/meerkat.svg"

# Absolute Exec path: ~/.local/bin is not always on the PATH of the session.
sed -e "s|^Exec=.*|Exec=\"$bin\"|" \
    -e "/^Exec=/a Path=$HOME" \
    "$here/meerkat.desktop" > "$apps/meerkat.desktop"
chmod 644 "$apps/meerkat.desktop"
refresh

echo "meerkat installed:"
echo "  binary     $bin"
echo "  menu entry $apps/meerkat.desktop"
echo "  icon       $icons/meerkat.svg"
echo "Repository list used from the menu: $HOME/meerkat.txt"
