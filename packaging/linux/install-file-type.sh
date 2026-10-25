#!/bin/sh
# Registers .cosmic job files for the current user: the file type, its icon in the file
# manager, and CosmicCAM as the program that opens them.
#
#   packaging/linux/install-file-type.sh [path/to/CosmicCAM]   install (default: CosmicCAM on PATH)
#   packaging/linux/install-file-type.sh --uninstall           remove it again
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
icon=application-x-cosmiccam-job
icons="$data/icons/hicolor"

refresh() {
    command -v update-mime-database >/dev/null && update-mime-database "$data/mime" || true
    command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -f -t "$icons" >/dev/null 2>&1 || true
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$data/mime/packages/cosmiccam-job.xml" "$icons/scalable/mimetypes/$icon.svg" \
        "$data/applications/cosmiccam.desktop"
    refresh
    echo "Removed the .cosmic file type."
    exit 0
fi

exe=${1:-CosmicCAM}
case "$exe" in
    /*) ;;
    */*) exe="$(pwd)/$exe" ;;
esac

mkdir -p "$data/mime/packages" "$icons/scalable/mimetypes" "$data/applications"
cp "$here/cosmiccam-job.xml" "$data/mime/packages/cosmiccam-job.xml"
cp "$root/assets/icons/file/cosmic-job.svg" "$icons/scalable/mimetypes/$icon.svg"
cat > "$data/applications/cosmiccam.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=CosmicCAM
Comment=CAM: toolpaths and G-code from CAD models
Exec="$exe" %f
Icon=$icon
Terminal=false
Categories=Graphics;Engineering;
MimeType=application/x-cosmiccam-job;
DESKTOP
refresh
command -v xdg-mime >/dev/null && xdg-mime default cosmiccam.desktop application/x-cosmiccam-job || true
echo "Registered .cosmic files (opened with $exe)."
