#!/bin/sh
# A headless Xfce desktop for the Linux tests and screenshots: Xvfb, the
# xfwm4 compositor (transparency and shadows, as on a real desktop), a
# wallpaper, and a dark bottom panel, as on Linux Mint, whose system tray
# hosts StatusNotifierItem icons over D-Bus. SCALE=2 makes it a HiDPI
# desktop (Xft.dpi 192), as on a laptop at 200 %.
#
#   eval "$(tests/linux/start-desktop.sh)"   (sets DISPLAY and the D-Bus address)
#
# Needs XDG_CONFIG_HOME and XDG_RUNTIME_DIR set to the test's own folders.
set -eu
: "${DISPLAY:=:99}"
: "${SCALE:=1}"
export DISPLAY
here=$(cd "$(dirname "$0")" && pwd)
conf="$XDG_CONFIG_HOME/xfce4/xfconf/xfce-perchannel-xml"
mkdir -p "$conf"
cp "$here/xfce4-panel.xml" "$conf/xfce4-panel.xml"
w=$((1600 * SCALE)) h=$((900 * SCALE))
Xvfb "$DISPLAY" -screen 0 "${w}x${h}x24" -nolisten tcp >/dev/null 2>&1 &
for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
bus=$(dbus-launch --sh-syntax)
eval "$bus"
export DBUS_SESSION_BUS_ADDRESS
# A soft gradient wallpaper, like the default of most desktops.
python3 "$here/../wallpaper.py" "$XDG_RUNTIME_DIR/wallpaper.png" "$w" "$h"
export GDK_SCALE="$SCALE" GTK_THEME=Adwaita:dark
xfwm4 --compositor=on --replace >/dev/null 2>&1 &
sleep 1
xfce4-panel >/dev/null 2>&1 &
# The panel reserves its strut and takes the tray selection.
for _ in $(seq 100); do
  xprop -root _NET_WORKAREA 2>/dev/null | grep -qv "0, 0, $w, $h" &&
    xprop -root 2>/dev/null | grep -q "_NET_SYSTEM_TRAY_S0\|_NET_WORKAREA" && break
  sleep 0.1
done
feh --no-fehbg --bg-fill "$XDG_RUNTIME_DIR/wallpaper.png"
# After the desktop's own start-up, which resets the resource database.
echo "Xft.dpi: $((96 * SCALE))" | xrdb -nocpp -merge
sleep 2
echo "export DISPLAY=$DISPLAY"
echo "$bus"
