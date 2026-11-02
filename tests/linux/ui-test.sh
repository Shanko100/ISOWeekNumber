#!/bin/bash
# UI and integration tests of the Linux edition, on a headless Xfce desktop
# with a compositor and a StatusNotifierItem tray (start-desktop.sh):
#
#   tests/linux/ui-test.sh <WeekNumber.AppImage> <screenshot folder>
#
# It goes the way a user does: the AppImage is double-clicked in Downloads
# and installs itself, the tray icon is clicked, the calendar is browsed
# and closed, Run at Startup is switched on from the panel's menu, the
# dialogs open, and the app is uninstalled again. Every step is checked;
# screenshots of each window are saved. Writes a Markdown table of results
# to $GITHUB_STEP_SUMMARY (or stdout) and exits non-zero if a test failed.
set -uo pipefail

app=$(realpath "$1")
shots=$(realpath -m "$2")
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$shots"

# A fresh user: home, config, data and runtime folders of its own (not
# under /tmp, from where Run at Startup is refused on purpose).
root=$(realpath -m "${TEST_ROOT:-target/ui-test}")
rm -rf "$root"
mkdir -p "$root"
export HOME="$root/home"
export XDG_CONFIG_HOME="$HOME/.config" XDG_DATA_HOME="$HOME/.local/share"
export XDG_RUNTIME_DIR="$root/run"
mkdir -p "$HOME/Downloads" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
unset WAYLAND_DISPLAY WEEKNUMBER_OPEN WEEKNUMBER_NO_INSTALL_PROMPT
export SCALE="${SCALE:-2}"
eval "$("$here/start-desktop.sh")"

installed="$HOME/.local/bin/weeknumber"
desktop="$XDG_DATA_HOME/applications/io.github.shanko100.weeknumber.desktop"
autostart="$XDG_CONFIG_HOME/autostart/weeknumber.desktop"
rows=()
failed=0

pass() { rows+=("| $1 | passed |"); echo "PASS  $1"; }
fail() { rows+=("| $1 | **FAILED** |"); echo "FAIL  $1"; failed=1; }
# check "<name>" <command...>: one test, passed when the command succeeds.
check() {
  local name=$1
  shift
  if "$@"; then pass "$name"; else fail "$name"; fi
}

# Wait up to $2 seconds for a command to succeed.
wait_for() {
  local cmd=$1 limit=${2:-10} i
  for ((i = 0; i < limit * 10; i++)); do
    eval "$cmd" >/dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}

# The visible window whose title is exactly $1 (first match).
window() { xdotool search --onlyvisible --name "^$1\$" 2>/dev/null | head -1; }

# The calendar: the pop-up window titled WeekNumber (a message box has the
# same title but is a dialog).
calendar() {
  local w
  for w in $(xdotool search --onlyvisible --name '^WeekNumber$' 2>/dev/null); do
    xprop -id "$w" _NET_WM_WINDOW_TYPE | grep -q POPUP_MENU && { echo "$w"; return; }
  done
}

# Frame geometry "x y w h" of a window, decorations included.
geometry() {
  xwininfo -frame -id "$1" | awk '
    /Absolute upper-left X/ {x=$4} /Absolute upper-left Y/ {y=$4}
    /Width:/ {w=$2} /Height:/ {h=$2} END {print x, y, w, h}'
}

# shot <name> <x> <y> <w> <h>: save that part of the screen.
shot() {
  local sw sh
  read -r sw sh < <(xdpyinfo | awk '/dimensions:/ {split($2, d, "x"); print d[1], d[2]}')
  local x=$(($2 < 0 ? 0 : $2)) y=$(($3 < 0 ? 0 : $3))
  local w=$(($2 + $4 > sw ? sw - x : $4 - (x - $2))) h=$(($3 + $5 > sh ? sh - y : $5 - (y - $3)))
  import -window root -crop "${w}x${h}+${x}+${y}" +repage "$shots/$1.png"
}

# shot_window <name> <window id>: the window with room for its shadow.
shot_window() {
  local x y w h m=$((40 * SCALE))
  read -r x y w h < <(geometry "$2")
  shot "$1" $((x - m)) $((y - m)) $((w + 2 * m)) $((h + 2 * m))
}

# The calendar with the panel corner it opened from.
shot_calendar() {
  local x y w h sw sh m=$((48 * SCALE))
  read -r x y w h < <(geometry "$2")
  read -r sw sh < <(xdpyinfo | awk '/dimensions:/ {split($2, d, "x"); print d[1], d[2]}')
  shot "$1" $((x - m)) $((y - m)) $((sw - x + m)) $((sh - y + m))
}

# Whether a process still runs (an exited child stays a zombie until reaped).
alive() { [ -e "/proc/$1" ] && ! grep -q '^State:.*Z' "/proc/$1/status"; }

rss_kb() { awk '/VmRSS/ {print $2}' "/proc/$1/status"; }

# A menu item of the tray icon, clicked over D-Bus as the panel does.
menu_click() {
  dbus-send --session --type=method_call --dest="org.kde.StatusNotifierItem-$1-1" \
    /MenuBar com.canonical.dbusmenu.Event int32:"$2" string:clicked variant:int32:0 uint32:0
}

registered_items() {
  dbus-send --session --print-reply --dest=org.kde.StatusNotifierWatcher /StatusNotifierWatcher \
    org.freedesktop.DBus.Properties.Get string:org.kde.StatusNotifierWatcher \
    string:RegisteredStatusNotifierItems 2>/dev/null
}

# Centre of the panel's system tray (the plugin hosting the icon).
tray_point() {
  xwininfo -root -tree | awk '/"wrapper-2.0": \("wrapper-2.0"/ && $NF ~ /^\+[0-9]+\+[0-9]+$/ {
      split($(NF-1), s, "x"); split($NF, p, "+");
      if (s[1] > 10 && p[3] > 0) print p[2] + int(s[1] / 2), p[3] + int(s[2] / 2)
    }' | tail -1
}

# --- The AppImage ---------------------------------------------------------
offset=$("$app" --appimage-offset)
listing=$(unsquashfs -o "$offset" -l "$app" 2>/dev/null)
check "AppImage: payload holds the menu entry and the icons" \
  grep -q "io.github.shanko100.weeknumber.desktop" <<<"$listing"
(cd "$root" && "$app" --appimage-extract '*.desktop' >/dev/null)
check "AppImage: menu entry passes desktop-file-validate" \
  desktop-file-validate "$root/squashfs-root/io.github.shanko100.weeknumber.desktop"

# --- Install by double-click ------------------------------------------------
download="$HOME/Downloads/$(basename "$app")"
cp "$app" "$download"
chmod +x "$download"
"$download" &
pid=$!
if wait_for '[ -n "$(window WeekNumber)" ]' 15; then
  sleep 1
  prompt=$(window WeekNumber)
  shot_window linux-install-prompt "$prompt"
  pass "Install: started from Downloads, the app offers to install itself"
  xdotool windowactivate --sync "$prompt" key Return 2>/dev/null || xdotool key Return
else
  fail "Install: started from Downloads, the app offers to install itself"
fi
check "Install: the installed program replaces the download (same process)" \
  wait_for '[ "$(readlink /proc/$pid/exe)" = "$installed" ]' 10
check "Install: program, menu entry and icons are in place" \
  test -x "$installed" -a -f "$desktop" -a \
  -f "$XDG_DATA_HOME/icons/hicolor/256x256/apps/io.github.shanko100.weeknumber.png"
check "Install: the menu entry passes desktop-file-validate" desktop-file-validate "$desktop"
check "Install: every icon size is exact (sharp)" bash -c '
  for s in 16 24 32 48 64 128 256; do
    f="$0/icons/hicolor/${s}x${s}/apps/io.github.shanko100.weeknumber.png"
    [ "$(identify -format "%wx%h" "$f")" = "${s}x${s}" ] || exit 1
  done' "$XDG_DATA_HOME"
check "Install: no copy of the AppImage payload in the installed program" \
  bash -c '! "$0" --appimage-offset >/dev/null 2>&1' "$installed"

# --- Running ----------------------------------------------------------------
check "Tray: the icon is registered with the panel (StatusNotifierItem)" \
  wait_for 'registered_items | grep -q "StatusNotifierItem-$pid-1"' 15
check "Process: one process, no child processes" \
  bash -c '[ "$(pgrep -c -f "^$0\$")" = 1 ] && [ -z "$(pgrep -P "$1")" ]' "$installed" "$pid"
sleep 2
idle_kb=$(rss_kb "$pid")

read -r tx ty < <(tray_point)
xdotool mousemove "$tx" "$ty" click 1
if wait_for '[ -n "$(calendar)" ]' 10; then
  sleep 1
  cal=$(calendar)
  pass "Calendar: a click on the tray icon opens it"
  read -r cx cy cw ch < <(geometry "$cal")
  read -r sw sh < <(xdpyinfo | awk '/dimensions:/ {split($2, d, "x"); print d[1], d[2]}')
  # (The window includes its shadow margin.)
  check "Calendar: opens above the panel, at the tray" \
    test $((cy + ch)) -le $((sh - 30 * SCALE)) -a $((cx + cw)) -gt $((sw * 3 / 4))
  open_kb=$(rss_kb "$pid")
  shot_calendar linux-calendar-light "$cal"
  before=$(import -window "$cal" -crop 100%x70%+0+0 - | md5sum)
  xdotool key Right
  sleep 1
  after=$(import -window "$cal" -crop 100%x70%+0+0 - | md5sum)
  check "Calendar: the arrow keys browse the months" test "$before" != "$after"
  xdotool key Left
  xdotool key Escape
  check "Calendar: Escape closes it" wait_for '[ -z "$(calendar)" ]' 5
else
  fail "Calendar: a click on the tray icon opens it"
fi
sleep 1
closed_kb=$(rss_kb "$pid")

# Dark Mode from the panel menu, then the calendar again.
menu_click "$pid" 4
sleep 1
xdotool mousemove "$tx" "$ty" click 1
if wait_for '[ -n "$(calendar)" ]' 10; then
  sleep 1
  shot_calendar linux-calendar-dark "$(calendar)"
  pass "Menu: Dark Mode switches the calendar's theme"
  xdotool key Escape
else
  fail "Menu: Dark Mode switches the calendar's theme"
fi
check "Settings: Dark Mode is saved" \
  grep -q '"DarkMode": true' "$XDG_CONFIG_HOME/weeknumber/settings.json"

# Run at Startup from the panel menu.
menu_click "$pid" 1
check "Run at Startup: the autostart entry starts the installed program" \
  wait_for 'grep -qF "Exec=\"$installed\"" "$autostart"' 5
menu_click "$pid" 1
check "Run at Startup: switched off, the entry is removed" wait_for '[ ! -e "$autostart" ]' 5

# --- Dialogs: each launch also hands over from the running copy -------------
"$here/../settings.sh" "$XDG_CONFIG_HOME/weeknumber/settings.json" light
# what:window title (regular expression; "." stands for the en dash)
for spec in "calendar:WeekNumber" "color:WeekNumber . Icon Color" \
  "holidays:WeekNumber . Public Holidays" "timezone:WeekNumber . Time Zone" \
  "about:About WeekNumber" "legal:WeekNumber . Legal notices"; do
  what=${spec%%:*}
  name=${spec#*:}
  title=${name#WeekNumber . }
  [ "$what" = calendar ] && title="Calendar with holidays and a second clock"
  old=$pid
  WEEKNUMBER_OPEN=$what "$installed" &
  pid=$!
  find="window \"$name\""
  [ "$what" = calendar ] && find=calendar
  if wait_for "[ -n \"\$($find)\" ]" 10; then
    sleep 1.5
    w=$(eval "$find")
    if [ "$what" = calendar ]; then
      # Off the tray (its tooltip) onto the calendar's empty top corner.
      read -r gx gy _ _ < <(geometry "$w")
      xdotool mousemove $((gx + 22 * SCALE)) $((gy + 20 * SCALE))
      sleep 1
      shot_calendar linux-calendar-holidays "$w"
    else
      shot_window "linux-$what" "$w"
    fi
    pass "Window: $title opens"
  else
    fail "Window: $title opens"
  fi
  check "Single instance: the new launch took over from the running copy" \
    wait_for '! alive $old' 5
done

# --- Safety -----------------------------------------------------------------
kill "$pid" 2>/dev/null
sleep 1
WEEKNUMBER_NO_INSTALL_PROMPT=1 "$download" &
pid=$!
wait_for 'registered_items | grep -q "StatusNotifierItem-$pid-1"' 15
menu_click "$pid" 1
if wait_for '[ -n "$(window WeekNumber)" ]' 5; then
  sleep 1
  shot_window linux-startup-refused "$(window WeekNumber)"
  xdotool key Return
fi
check "Run at Startup: refused for a copy in Downloads" test ! -e "$autostart"
kill "$pid" 2>/dev/null
sleep 1

check "Uninstall: removes the program, menu entry and icons" bash -c '
  "$0" --uninstall >/dev/null && [ ! -e "$0" ] && [ ! -e "$1" ] &&
  [ -z "$(find "$2/icons" -name "io.github.shanko100.weeknumber.png")" ]' \
  "$installed" "$desktop" "$XDG_DATA_HOME"

limit_kb=$((12 * 1024))
check "Memory: idle below 12 MB (resident)" test "${idle_kb:-99999}" -lt "$limit_kb"

{
  echo "### UI tests (Linux $(uname -m), Xfce at ${SCALE}x)"
  echo ""
  echo "| Test | Result |"
  echo "|---|---|"
  printf '%s\n' "${rows[@]}"
  echo ""
  echo "| Idle RSS (KB) | Calendar open (KB) | After closing (KB) | AppImage (bytes) |"
  echo "|---|---|---|---|"
  echo "| ${idle_kb:-?} | ${open_kb:-?} | ${closed_kb:-?} | $(stat -c %s "$app") |"
} >>"${GITHUB_STEP_SUMMARY:-/dev/stdout}"

pkill -P $$ 2>/dev/null
exit $failed
