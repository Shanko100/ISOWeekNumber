#!/bin/bash
# UI and integration tests of the macOS edition, on the runner's desktop:
#
#   tests/macos/ui-test.sh <WeekNumber.app> <screenshot folder>
#
# A fresh home folder each run. The app starts as a login session starts
# it, shows its menu bar item, opens the calendar on glass, the menu and
# every dialog in both themes, and hands over between launches; every step
# is checked and each window is saved as a screenshot. Writes a Markdown
# table of results to $GITHUB_STEP_SUMMARY (or stdout) and exits non-zero
# if a test failed.
set -uo pipefail

bundle=$(cd "$1" && pwd)
exe="$bundle/Contents/MacOS/WeekNumber"
mkdir -p "$2"
shots=$(cd "$2" && pwd)
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
swiftc -O "$here/windows.swift" -o "$work/windows"

export HOME="$work/home" TMPDIR="$work/tmp/"
mkdir -p "$HOME" "$TMPDIR"
settings="$HOME/Library/Application Support/WeekNumber/settings.json"
unset WEEKNUMBER_OPEN
rows=()
failed=0
pass() { rows+=("| $1 | passed |"); echo "PASS  $1"; }
fail() { rows+=("| $1 | **FAILED** |"); echo "FAIL  $1"; failed=1; }
check() {
  local name=$1
  shift
  if "$@"; then pass "$name"; else fail "$name"; fi
}
wait_for() {
  local cmd=$1 limit=${2:-10} i
  for ((i = 0; i < limit * 10; i++)); do
    eval "$cmd" >/dev/null 2>&1 && return 0
    sleep 0.1
  done
  return 1
}
alive() { kill -0 "$1" 2>/dev/null && [ "$(ps -o stat= -p "$1" | cut -c1)" != Z ]; }
# The app's windows above the menu bar layer 0: "x y w h layer".
windows() { "$work/windows" "$1"; }
# The calendar: pop-up menu level (101), the largest first.
popup() { windows "$1" | awk '$5 == 101 {print $3 * $4, $0}' | sort -rn | head -1 | cut -d' ' -f2-; }
# The menu bar item: a small window of the app inside the menu bar.
status_item() { windows "$1" | awk '$5 > 0 && $2 < 40 && $4 <= 40' | head -1; }
# The item's native menu: a tall window above the normal level, below the
# menu bar.
native_menu() { windows "$1" | awk '$5 > 0 && $4 > 60 && $2 >= 0 {print $3 * $4, $0}' | sort -rn | head -1 | cut -d' ' -f2-; }
# On a failed window check: what the app has on screen.
dump() { echo "  windows of $1 (x y w h level):"; windows "$1" | sed 's/^/    /'; }
dialog() { windows "$1" | awk '$5 == 0 {print $3 * $4, $0}' | sort -rn | head -1 | cut -d' ' -f2-; }
read -r sw sh < <(system_profiler SPDisplaysDataType 2>/dev/null |
  awk '/Resolution:/ {print $2, $4; exit}')
sw=${sw:-1024} sh=${sh:-768}
# Points of the main display (the screenshot rectangle unit).
read -r pw ph < <(osascript -l JavaScript -e 'ObjC.import("AppKit"); var f = $.NSScreen.mainScreen.frame; f.size.width + " " + f.size.height' 2>/dev/null || echo "$sw $sh")
shot() { screencapture -x -R "$2,$3,$4,$5" "$shots/$1.png"; }
shot_rect() {
  local x y w h m=${3:-32}
  read -r x y w h _ <<<"$2"
  shot "$1" $((x - m)) $((y - m)) $((w + 2 * m)) $((h + 2 * m))
}
# The calendar with the menu bar corner it opened from.
shot_flyout() {
  local x y w h m=40
  read -r x y w h _ <<<"$2"
  shot "$1" $((x - m)) 0 $((w + 2 * m)) $((y + h + m))
}
write_settings() { "$here/../settings.sh" "$settings" "$1"; }

# The same wallpaper as the other platforms' tests, where the runner lets
# us set one.
python3 "$here/../wallpaper.py" "$work/wallpaper.png" "${pw%.*}" "${ph%.*}"
osascript -e "tell application \"System Events\" to tell every desktop to set picture to POSIX file \"$work/wallpaper.png\"" \
  >/dev/null 2>&1 &
sleep 5

# The right half of the menu bar, where menu bar items appear.
menubar() { screencapture -x -R "$((${pw%.*} / 2)),0,$((${pw%.*} / 2)),24" "$1"; }

launch() {
  if [ -n "$1" ]; then WEEKNUMBER_OPEN=$1 "$exe" & else "$exe" & fi
  pid=$!
}

# --- First run ---------------------------------------------------------------
menubar "$work/bar-before.png"
launch ""
sleep 6
check "First run: stays running" alive "$pid"
check "Process: one process, no child processes" test -z "$(pgrep -P "$pid")"
check "Settings: written on first run" grep -q '"StartupInitialized": true' "$settings"
# The item's window (macOS 15), or failing that a menu bar that changed
# when the app started (macOS 26 lists no window for it).
menubar "$work/bar-after.png"
if wait_for '[ -n "$(status_item $pid)" ]' 5 || ! cmp -s "$work/bar-before.png" "$work/bar-after.png"; then
  pass "Menu bar: the week number item is shown"
else
  fail "Menu bar: the week number item is shown"
  dump "$pid"
fi
idle_kb=$(ps -o rss= -p "$pid" | tr -d ' ')
item=$(status_item "$pid")
[ -n "$item" ] && shot_rect macos-menubar-item "$item" 60

# --- The calendar ----------------------------------------------------------------
old=$pid
launch calendar
check "Single instance: a new launch takes over from the running copy" wait_for '! alive $old' 8
if wait_for '[ -n "$(popup $pid)" ]' 10; then
  sleep 1.5
  cal=$(popup "$pid")
  pass "Calendar: opens"
  read -r cx cy cw ch _ <<<"$cal"
  check "Calendar: opens under the menu bar" test "$cy" -ge 20 -a "$cy" -le 60
  shot_flyout macos-calendar-light "$cal"
  open_kb=$(ps -o rss= -p "$pid" | tr -d ' ')
else
  fail "Calendar: opens"
fi

# --- Every window, light and dark: each launch hands over ----------------------
for theme in dark light; do
  write_settings "$theme"
  for what in calendar color holidays timezone about legal; do
    [ "$theme" = light ] && [ "$what" = legal ] && continue
    old=$pid
    launch "$what"
    case $what in
      calendar) find=popup ;;
      *) find=dialog ;;
    esac
    if wait_for '[ -n "$($find $pid)" ]' 10; then
      sleep 1.5
      r=$($find "$pid")
      case $what in
        calendar) shot_flyout "macos-calendar-$theme-holidays" "$r" ;;
        *) shot_rect "macos-$what-$theme" "$r" 40 ;;
      esac
      pass "Window ($theme): $what opens"
    else
      fail "Window ($theme): $what opens"
    fi
    check "Single instance: the $what launch took over" wait_for '! alive $old' 8
  done
  # The menu bar item's menu: native, so it holds the app until it closes.
  kill "$pid" 2>/dev/null
  wait_for '! alive $pid' 5
  launch menu
  if wait_for '[ -n "$(native_menu $pid)" ]' 10; then
    sleep 1.5
    shot_flyout "macos-menu-$theme" "$(native_menu "$pid")"
    pass "Menu ($theme): the menu bar item's menu opens"
  else
    fail "Menu ($theme): the menu bar item's menu opens"
    dump "$pid"
  fi
  kill "$pid" 2>/dev/null
  wait_for '! alive $pid' 5
  launch ""
  sleep 3
done
kill "$pid" 2>/dev/null

limit_kb=$((100 * 1024))
check "Memory: idle below 100 MB (resident, AppKit included)" test "${idle_kb:-999999}" -lt "$limit_kb"

{
  echo "### UI tests (macOS $(sw_vers -productVersion), $(uname -m))"
  echo ""
  echo "| Test | Result |"
  echo "|---|---|"
  printf '%s\n' "${rows[@]}"
  echo ""
  echo "| Idle RSS (KB) | Calendar open (KB) | App size (bytes) |"
  echo "|---|---|---|"
  echo "| ${idle_kb:-?} | ${open_kb:-?} | $(stat -f %z "$exe") |"
} >>"${GITHUB_STEP_SUMMARY:-/dev/stdout}"
exit $failed
