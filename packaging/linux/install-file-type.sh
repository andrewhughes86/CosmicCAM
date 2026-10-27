#!/bin/sh
# Registers CosmicDMS for the current user: the application (its launcher entry and icon, which
# Wayland taskbars use for the running window), the .cosmic job file type, its icon in the file
# manager, and CosmicDMS as the program that opens them.
#
#   packaging/linux/install-file-type.sh [path/to/CosmicDMS]   install (default: CosmicDMS on PATH)
#   packaging/linux/install-file-type.sh --uninstall           remove it again
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
icon=application-x-cosmicdms-job
app_icon=cosmicdms
icons="$data/icons/hicolor"
# Named after the window's app id, so taskbars match the running window to it.
desktop="$data/applications/CosmicDMS.desktop"
# What an install under the application's former name, CosmicCAM, left behind.
legacy() {
    rm -f "$data/mime/packages/cosmiccam-job.xml" "$icons/scalable/mimetypes/application-x-cosmiccam-job.svg" \
        "$icons/scalable/apps/cosmiccam.svg" "$data/applications/CosmicCAM.desktop" "$data/applications/cosmiccam.desktop"
}

refresh() {
    command -v update-mime-database >/dev/null && update-mime-database "$data/mime" || true
    command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -f -t "$icons" >/dev/null 2>&1 || true
}

if [ "${1:-}" = "--uninstall" ]; then
    legacy
    rm -f "$data/mime/packages/cosmicdms-job.xml" "$icons/scalable/mimetypes/$icon.svg" \
        "$icons/scalable/apps/$app_icon.svg" "$desktop" "$data/applications/cosmicdms.desktop"
    refresh
    echo "Removed CosmicDMS's launcher and the .cosmic file type."
    exit 0
fi

exe=${1:-CosmicDMS}
case "$exe" in
    /*) ;;
    */*) exe="$(pwd)/$exe" ;;
esac

mkdir -p "$data/mime/packages" "$icons/scalable/mimetypes" "$icons/scalable/apps" "$data/applications"
cp "$here/cosmicdms-job.xml" "$data/mime/packages/cosmicdms-job.xml"
cp "$root/assets/icons/file/cosmic-job.svg" "$icons/scalable/mimetypes/$icon.svg"
cp "$root/assets/icons/app/cosmicdms.svg" "$icons/scalable/apps/$app_icon.svg"
# An entry from an older install under another name would show twice.
legacy
cat > "$desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=CosmicDMS
Comment=CAM: toolpaths and G-code from CAD models
Exec="$exe" %f
Icon=$app_icon
StartupWMClass=CosmicDMS
Terminal=false
Categories=Graphics;Engineering;
MimeType=application/x-cosmicdms-job;
DESKTOP
refresh
command -v xdg-mime >/dev/null && xdg-mime default CosmicDMS.desktop application/x-cosmicdms-job || true
echo "Installed the CosmicDMS launcher and registered .cosmic files (opened with $exe)."
