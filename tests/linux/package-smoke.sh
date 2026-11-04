#!/bin/bash
# Smoke tests of the installed Linux packages, on a headless Xfce desktop
# whose tray hosts both StatusNotifierItem (D-Bus) and XEmbed icons
# (start-desktop.sh). Run by build-linux.yaml only.
#
#   tests/linux/package-smoke.sh <dist folder> <version> <x64|arm64>
#
# Each of the .deb, .rpm, Flatpak and AppImage is installed (the .rpm into
# a private RPM database), started as from the applications menu, and
# checked: no install prompt, its tray icon at every size panels pick from,
# a left click (Activate) opens the calendar, a right click (ContextMenu)
# the menu, Run at Startup (an autostart entry for the .deb and .rpm, the
# Background portal for the Flatpak), Exit, and idle memory. Then the GNOME
# Shell route: with XDG_CURRENT_DESKTOP=GNOME the icon docks in the XEmbed
# tray instead, where button 1 opens the calendar and button 3 the menu.
# Needs sudo (dpkg, rpm), flatpak with the Freedesktop runtime available
# from Flathub, xfce4-panel, stalonetray, xdotool, x11-utils and dbus-x11.
set -uo pipefail
sudo() { if [ "$(id -u)" = 0 ]; then "$@"; else command sudo "$@"; fi; }

dist=$(realpath "$1")
version=$2
arch=$3
here=$(cd "$(dirname "$0")" && pwd)
base="$dist/WeekNumber-$version-linux-$arch"
app_id=io.github.shanko100.weeknumber

# A fresh user (not under /tmp, from where Run at Startup is refused, and
# where a Flatpak sandbox could not reach the runtime folder).
root=$(realpath -m "${TEST_ROOT:-target/package-smoke}")
rm -rf "$root"
mkdir -p "$root"
export HOME="$root/home"
export XDG_CONFIG_HOME="$HOME/.config" XDG_DATA_HOME="$HOME/.local/share"
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
[ -d "$XDG_RUNTIME_DIR" ] || { sudo mkdir -p "$XDG_RUNTIME_DIR"; sudo chown "$(id -u)" "$XDG_RUNTIME_DIR"; }
chmod 700 "$XDG_RUNTIME_DIR"
mkdir -p "$HOME"
unset WAYLAND_DISPLAY WEEKNUMBER_OPEN WEEKNUMBER_NO_INSTALL_PROMPT
export SCALE=1 XDG_CURRENT_DESKTOP=XFCE
eval "$("$here/start-desktop.sh")"
autostart="$XDG_CONFIG_HOME/autostart/weeknumber.desktop"

rows=()
failed=0
pass() { rows+=("| $1 | passed |"); echo "PASS  $1"; }
fail() { rows+=("| $1 | **FAILED** |"); echo "FAIL  $1"; failed=1; }
check() { local name=$1; shift; if "$@"; then pass "$name"; else fail "$name"; fi; }
wait_for() {
  local cmd=$1 limit=${2:-10} i
  for ((i = 0; i < limit * 10; i++)); do
    eval "$cmd" >/dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}
alive() { [ -e "/proc/$1" ] && ! grep -q '^State:.*Z' "/proc/$1/status"; }

# Visible pop-ups titled WeekNumber: the calendar is the large one, the
# tray menu (drawn by the app) the tall narrow one.
popups() {
  local w
  for w in $(xdotool search --onlyvisible --name '^WeekNumber$' 2>/dev/null); do
    xprop -id "$w" _NET_WM_WINDOW_TYPE | grep -q 'POPUP_MENU\|DROPDOWN_MENU' || continue
    xwininfo -id "$w" | awk -v id="$w" '/Width:/ {w=$2} /Height:/ {h=$2} END {print id, w, h}'
  done
}
calendar() { popups | awk '$2 > 350 && $2 > $3 {print $1; exit}'; }
own_menu() { popups | awk '$3 > $2 {print $1; exit}'; }
message_box() {
  local w
  for w in $(xdotool search --onlyvisible --name '^WeekNumber$' 2>/dev/null); do
    xprop -id "$w" _NET_WM_WINDOW_TYPE | grep -q DIALOG && return 0
  done
  return 1
}
close_popups() { xdotool key Escape; sleep 0.5; xdotool key Escape; wait_for '[ -z "$(popups)" ]' 3; }

registered_items() {
  dbus-send --session --print-reply --dest=org.kde.StatusNotifierWatcher /StatusNotifierWatcher \
    org.freedesktop.DBus.Properties.Get string:org.kde.StatusNotifierWatcher \
    string:RegisteredStatusNotifierItems 2>/dev/null
}
# The bus name the app's item is registered under (well-known or unique):
# the registered item whose Id is weeknumber.
item_name() {
  registered_items | grep -o '"[^"]*"' | tr -d '"' | sed 's|/StatusNotifierItem$||' | while read -r n; do
    dbus-send --session --print-reply --dest="$n" /StatusNotifierItem \
      org.freedesktop.DBus.Properties.Get string:org.kde.StatusNotifierItem string:Id 2>/dev/null |
      grep -q '"weeknumber"' && { echo "$n"; break; }
  done
}
sni() { dbus-send --session --print-reply --dest="$item" "$@"; }
icon_sizes() {
  sni /StatusNotifierItem org.freedesktop.DBus.Properties.Get \
    string:org.kde.StatusNotifierItem string:IconPixmap |
    awk '/struct \{/ {getline; w=$2; getline; print w "x" $2}' | sort -n | tr '\n' ' '
}
menu_click() {
  dbus-send --session --type=method_call --dest="$item" /MenuBar \
    com.canonical.dbusmenu.Event int32:"$1" string:clicked variant:int32:0 uint32:0
}
# The app's own process (the Flatpak's runs inside the sandbox).
app_pid() { pgrep -n -x weeknumber || pgrep -n -x WeekNumber || pgrep -n -f 'WeekNumber.*AppImage$'; }
xembed_icon() { xwininfo -root -tree | awk '/"WeekNumber": \("weeknumber" "WeekNumber"\)/ && $(NF-1) ~ /^[0-9]+x[0-9]+\+/ {split($(NF-1), s, "x"); if (s[1] <= 64) {print $1; exit}}'; }

# run_case <label> <expected login start: entry|portal> <command...>
run_case() {
  local label=$1 login=$2
  shift 2
  rm -rf "$XDG_CONFIG_HOME/weeknumber" "$autostart" "$HOME/.var"
  "$@" >"$root/$label.log" 2>&1 &
  local launcher=$!
  item=""
  if wait_for '[ -n "$(item_name)" ]' 20; then
    item=$(item_name)
    pass "$label: starts with no install prompt, icon in the tray ($item)"
  else
    fail "$label: starts with no install prompt, icon in the tray"
    kill "$launcher" 2>/dev/null
    return
  fi
  local pid
  pid=$(app_pid)
  sleep 2
  check "$label: icon sent at 16, 22, 24, 32, 48 and 64 px" \
    test "$(icon_sizes)" = "16x16 22x22 24x24 32x32 48x48 64x64 "
  check "$label: one process, no child processes" \
    bash -c '[ -n "$0" ] && [ -z "$(pgrep -P "$0")" ]' "$pid"
  local idle_kb
  idle_kb=$(awk '/VmRSS/ {print $2}' "/proc/$pid/status")
  sni /StatusNotifierItem org.kde.StatusNotifierItem.Activate int32:1500 int32:880 >/dev/null
  check "$label: left click (Activate) opens the calendar" wait_for '[ -n "$(calendar)" ]' 8
  close_popups
  sni /StatusNotifierItem org.kde.StatusNotifierItem.ContextMenu int32:1500 int32:880 >/dev/null
  check "$label: right click (ContextMenu) opens the menu" wait_for '[ -n "$(own_menu)" ]' 8
  close_popups
  menu_click 1
  if [ "$login" = entry ]; then
    check "$label: Run at Startup writes an entry for /usr/bin/weeknumber" \
      wait_for 'grep -qxF "Exec=\"/usr/bin/weeknumber\"" "$autostart"' 5
    menu_click 1
    check "$label: Run at Startup off removes it" wait_for '[ ! -e "$autostart" ]' 5
  else
    # No portal on this test desktop: the request is refused with a
    # message, nothing is written outside the sandbox, and the app lives on.
    check "$label: Run at Startup without a portal says so" wait_for message_box 10
    check "$label: Run at Startup never writes the autostart folder itself" \
      bash -c '[ ! -e "$0" ] && kill -0 "$1"' "$autostart" "$pid"
    xdotool key Return
    wait_for '! message_box' 5
  fi
  menu_click 8
  check "$label: Exit ends it" wait_for "! alive $pid" 10
  kill "$launcher" 2>/dev/null
  rows+=("| $label: idle memory | $idle_kb KB |")
}

# --- .deb ---------------------------------------------------------------------
sudo dpkg -i "$base.deb" >/dev/null
check ".deb: installs /usr/bin/weeknumber, the menu entry and icons" \
  test -x /usr/bin/weeknumber -a -f "/usr/share/applications/$app_id.desktop" -a -f "/usr/share/icons/hicolor/48x48/apps/$app_id.png"
check ".deb: menu entry passes desktop-file-validate" desktop-file-validate "/usr/share/applications/$app_id.desktop"
run_case ".deb" entry /usr/bin/weeknumber

sudo dpkg -r weeknumber >/dev/null
check ".deb: removes cleanly" test ! -e /usr/bin/weeknumber

# --- .rpm (a private database: this machine's packages are dpkg's) ----------
db="$root/rpmdb"
mkdir -p "$db"
sudo rpm --dbpath "$db" -i "$base.rpm"
check ".rpm: installs /usr/bin/weeknumber and the license files" \
  test -x /usr/bin/weeknumber -a -f /usr/share/licenses/weeknumber/LEGAL_NOTICES.txt
run_case ".rpm" entry /usr/bin/weeknumber
sudo rpm --dbpath "$db" -e weeknumber
check ".rpm: removes cleanly" test ! -e /usr/bin/weeknumber

# --- Flatpak ----------------------------------------------------------------
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
if flatpak install --user -y --noninteractive --bundle "$base.flatpak" >/dev/null; then
  pass "Flatpak: installs, with the Freedesktop runtime from Flathub"
  run_case "Flatpak" portal flatpak run --user "$app_id"
  flatpak uninstall --user -y --noninteractive "$app_id" >/dev/null
else
  fail "Flatpak: installs, with the Freedesktop runtime from Flathub"
fi

# --- AppImage (installed for the user as a double-click would) -------------
appimage="$base.AppImage"
"$appimage" --install >/dev/null
WEEKNUMBER_NO_INSTALL_PROMPT=1 "$HOME/.local/bin/weeknumber" >"$root/appimage.log" 2>&1 &
check "AppImage: the installed copy registers its icon" wait_for '[ -n "$(item_name)" ]' 15
item=$(item_name)
check "AppImage: icon sent at every size" test "$(icon_sizes)" = "16x16 22x22 24x24 32x32 48x48 64x64 "
[ -n "$item" ] && menu_click 8
sleep 1

# --- GNOME Shell: the XEmbed tray first ----------------------------------------
# With both a D-Bus watcher and an XEmbed tray on the desktop (the Xfce
# panel hosts both, as GNOME's AppIndicator extension does), the icon goes
# to the XEmbed tray.
installed="$HOME/.local/bin/weeknumber"
XDG_CURRENT_DESKTOP=ubuntu:GNOME WEEKNUMBER_NO_INSTALL_PROMPT=1 "$installed" >"$root/gnome.log" 2>&1 &
check "GNOME: the icon docks in the XEmbed tray" wait_for '[ -n "$(xembed_icon)" ]' 15
check "GNOME: and offers no D-Bus item as well" test -z "$(item_name)"
pkill -x weeknumber
# Clicks as the extension's tray hands them on, on an on-screen XEmbed
# tray (the Xfce panel draws its XEmbed icons off-screen).
pkill xfce4-panel
sleep 1
stalonetray --geometry 4x1-0-0 --icon-size 24 --kludges force_icons_size -bg '#202020' >/dev/null 2>&1 &
sleep 2
XDG_CURRENT_DESKTOP=ubuntu:GNOME WEEKNUMBER_NO_INSTALL_PROMPT=1 "$installed" >>"$root/gnome.log" 2>&1 &
if wait_for '[ -n "$(xembed_icon)" ]' 15; then
  sleep 1
  read -r ix iy < <(xwininfo -id "$(xembed_icon)" | awk '/Absolute upper-left X/ {x=$4} /Absolute upper-left Y/ {y=$4} /Width:/ {w=$2} END {print x + int(w / 2), y + int(w / 2)}')
  xdotool mousemove "$ix" "$iy" click 1
  check "GNOME: left click opens the calendar" wait_for '[ -n "$(calendar)" ]' 8
  close_popups
  xdotool mousemove "$ix" "$iy" click 3
  check "GNOME: right click opens the menu" wait_for '[ -n "$(own_menu)" ]' 8
  close_popups
else
  fail "GNOME: left and right clicks (the icon did not dock)"
fi
pkill -x weeknumber
pkill stalonetray
"$installed" --uninstall >/dev/null

{
  echo "### Package smoke tests ($arch)"
  echo ""
  echo "| Test | Result |"
  echo "|---|---|"
  printf '%s\n' "${rows[@]}"
} >> "${GITHUB_STEP_SUMMARY:-/dev/stdout}"
pkill xfce4-panel; pkill xfwm4; pkill stalonetray; pkill Xvfb
exit $failed
