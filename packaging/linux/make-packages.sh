#!/bin/sh
# Make the .deb, .rpm and Flatpak bundle from WeekNumber's static Linux
# executable.
#
#   packaging/linux/make-packages.sh <executable> <legal.txt> <version> <x64|arm64> <out dir>
#
# <legal.txt> is the Legal notices text the build composed for this
# executable (build.rs writes it next to legal.bin): the packages ship
# exactly what the app shows under About -> Legal notices.
#
# Every package holds the same program, the menu entry and icons the
# program writes itself (--appimage-extract), the AppStream description
# and the notices:
#   .deb      /usr/bin/weeknumber, /usr/share/doc/weeknumber/{copyright,LEGAL_NOTICES.txt}
#   .rpm      /usr/bin/weeknumber, /usr/share/licenses/weeknumber/{LICENSE,LEGAL_NOTICES.txt}
#   .flatpak  /app/bin/weeknumber, /app/share/licenses/<app id>/{LICENSE,LEGAL_NOTICES.txt}
# No package has dependencies or install scripts. The Flatpak runs on the
# Freedesktop runtime, which Flatpak fetches from Flathub when the bundle is
# installed; its sandbox gets the X11 display and the tray watcher on the
# session bus only -- no network, no files, no devices.
#
# Needs nfpm (deb, rpm) and flatpak (build-finish, build-export,
# build-bundle). Nothing is downloaded here.
set -eu

exe=$1
legal=$2
version=$3
arch=$4
out=$5

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
app_id=io.github.shanko100.weeknumber
runtime_branch=25.08
case "$arch" in
  x64) deb_arch=amd64; fp_arch=x86_64 ;;
  arm64) deb_arch=arm64; fp_arch=aarch64 ;;
  *) echo "unknown architecture $arch" >&2; exit 1 ;;
esac
base="WeekNumber-$version-linux-$arch"

mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The menu entry and icons, from the program itself.
cp "$exe" "$work/weeknumber"
chmod 755 "$work/weeknumber"
(cd "$work" && ./weeknumber --appimage-extract >/dev/null)
appdir="$work/squashfs-root"
grep -v '^X-AppImage-' "$appdir/$app_id.desktop" > "$work/$app_id.desktop"
grep -q '^Exec=weeknumber$' "$work/$app_id.desktop"
icons=$(cd "$appdir/usr/share/icons" && find hicolor -name "$app_id.png" | sort)
test -n "$icons"
cp "$legal" "$work/LEGAL_NOTICES.txt"
grep -q 'Legal notices' "$work/LEGAL_NOTICES.txt"

# --- .deb and .rpm (nfpm) -------------------------------------------------
license=$(sed -n 's|.*<project_license>\(.*\)</project_license>.*|\1|p' "$here/$app_id.metainfo.xml")
test -n "$license"
{
  cat <<EOF
name: weeknumber
arch: $deb_arch
platform: linux
version: "$version"
version_schema: none
release: "1"
section: utils
priority: optional
maintainer: "Shanko100 <https://github.com/Shanko100/ISOWeekNumber>"
vendor: Shanko100
homepage: https://github.com/Shanko100/ISOWeekNumber
license: "$license"
description: |-
  The ISO week number in the system tray
  WeekNumber shows the current ISO week number in the panel. A click opens
  a two-month calendar with week numbers, public holidays and a time-zone
  clock. Offline: no network, no accounts, no tracking. A non-profit project.
deb:
  compression: xz
rpm:
  compression: xz
  summary: The ISO week number in the system tray
  group: Applications/Productivity
  packager: "Shanko100 <https://github.com/Shanko100/ISOWeekNumber>"
contents:
  - src: $work/weeknumber
    dst: /usr/bin/weeknumber
    file_info: {mode: 0755}
  - src: $work/$app_id.desktop
    dst: /usr/share/applications/$app_id.desktop
    file_info: {mode: 0644}
  - src: $here/$app_id.metainfo.xml
    dst: /usr/share/metainfo/$app_id.metainfo.xml
    file_info: {mode: 0644}
  - src: $here/copyright
    dst: /usr/share/doc/weeknumber/copyright
    packager: deb
    file_info: {mode: 0644}
  - src: $work/LEGAL_NOTICES.txt
    dst: /usr/share/doc/weeknumber/LEGAL_NOTICES.txt
    packager: deb
    file_info: {mode: 0644}
  - src: $repo/LICENSE
    dst: /usr/share/licenses/weeknumber/LICENSE
    packager: rpm
    type: license
    file_info: {mode: 0644}
  - src: $work/LEGAL_NOTICES.txt
    dst: /usr/share/licenses/weeknumber/LEGAL_NOTICES.txt
    packager: rpm
    type: license
    file_info: {mode: 0644}
EOF
  for i in $icons; do
    printf '  - src: %s\n    dst: %s\n    file_info: {mode: 0644}\n' \
      "$appdir/usr/share/icons/$i" "/usr/share/icons/$i"
  done
} > "$work/nfpm.yaml"
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git -C "$repo" log -1 --format=%ct 2>/dev/null || echo 0)}
export SOURCE_DATE_EPOCH
nfpm package --config "$work/nfpm.yaml" --packager deb --target "$out/$base.deb"
nfpm package --config "$work/nfpm.yaml" --packager rpm --target "$out/$base.rpm"

# --- Flatpak bundle ---------------------------------------------------------
build="$work/flatpak"
files="$build/files"
mkdir -p "$files/bin" "$files/share/applications" "$files/share/metainfo" \
  "$files/share/licenses/$app_id"
install -m 755 "$work/weeknumber" "$files/bin/weeknumber"
install -m 644 "$work/$app_id.desktop" "$files/share/applications/$app_id.desktop"
install -m 644 "$here/$app_id.metainfo.xml" "$files/share/metainfo/$app_id.metainfo.xml"
install -m 644 "$repo/LICENSE" "$files/share/licenses/$app_id/LICENSE"
install -m 644 "$work/LEGAL_NOTICES.txt" "$files/share/licenses/$app_id/LEGAL_NOTICES.txt"
for i in $icons; do
  install -D -m 644 "$appdir/usr/share/icons/$i" "$files/share/icons/$i"
done
# The program needs nothing from the runtime but the sandbox it runs in.
cat > "$build/metadata" <<EOF
[Application]
name=$app_id
runtime=org.freedesktop.Platform/$fp_arch/$runtime_branch
sdk=org.freedesktop.Sdk/$fp_arch/$runtime_branch
EOF
# Least privilege: the X11 display (also XWayland) for the windows and the
# XEmbed tray, and the StatusNotifierItem watcher for the D-Bus tray. The
# item registers under its own unique bus name, so it owns no names; Run at
# Startup goes through the Background portal, open to every Flatpak.
flatpak build-finish "$build" \
  --command=weeknumber \
  --socket=x11 \
  --talk-name=org.kde.StatusNotifierWatcher
# The icon check runs unsandboxed: the icons are the program's own, and
# build machines need not allow the user namespaces bubblewrap uses.
flatpak build-export --disable-sandbox --arch="$fp_arch" "$work/repo" "$build" stable
flatpak build-bundle --arch="$fp_arch" \
  --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
  "$work/repo" "$out/$base.flatpak" "$app_id" stable

ls -l "$out/$base.deb" "$out/$base.rpm" "$out/$base.flatpak"
