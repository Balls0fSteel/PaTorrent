#!/bin/bash
# run.sh SLICE [SECONDS] [SCRIPT.applescript] [-- APP ARGUMENTS] (on the Mac VM): under the VM
# lock, starts a portable copy of ~/rp/SLICE/out/Rookparcel.app in the logged-in GUI session
# (~/rp/SLICE/run, with an empty rookparcel.ini beside it, so nothing goes to ~/Library), with
# ROOKPARCEL_SNAPSHOTS=~/rp/SLICE/shots; waits SECONDS (default 20) in all, runs the AppleScript
# file half way (System Events clicks menus by name; it can read RP_SHOTS through
# `system attribute "RP_SHOTS"` and call `screencapture -x` itself), takes `screencapture -x`
# pictures before and after it, prints the process's CPU and memory, and quits the app (its
# Quit menu item, clicked through System Events; then kill).
#
#   RP_APPEARANCE=dark|light  switches the system appearance for the run and back afterwards
#   RP_INI='key=value'        lines for the portable rookparcel.ini (default: empty)
#
# The app's stdout and stderr (its log) go to ~/rp/SLICE/shots/app.log.
set -euo pipefail

SLICE=${1:?usage: run.sh SLICE [SECONDS] [SCRIPT] [-- ARGS]}
shift
SECONDS_TOTAL=20
SCRIPT=""
if [ $# -gt 0 ] && [ "$1" != "--" ]; then SECONDS_TOTAL=$1; shift; fi
if [ $# -gt 0 ] && [ "$1" != "--" ]; then SCRIPT=$1; shift; fi
[ "${1:-}" = "--" ] && shift
. "$(dirname "$0")/lib.sh"

APP_SRC="$RP/$SLICE/out/Rookparcel.app"
RUN="$RP/$SLICE/run"
SHOTS="$RP/$SLICE/shots"
[ -d "$APP_SRC" ] || { echo "run.sh: build first ($APP_SRC is missing)" >&2; exit 1; }
[ -z "$SCRIPT" ] || [ -f "$SCRIPT" ] || { echo "run.sh: $SCRIPT is missing" >&2; exit 1; }

lock "run $SLICE"
rm -rf "$RUN" "$SHOTS"
mkdir -p "$RUN" "$SHOTS"
cp -R "$APP_SRC" "$RUN/"
# RP_INI: lines for the portable rookparcel.ini (for example list.sort=size:desc)
printf '%s\n' "${RP_INI:-}" > "$RUN/rookparcel.ini"
EXE="$RUN/Rookparcel.app/Contents/MacOS/Rookparcel"

appearance_was=""
set_dark() { osascript -e "tell application \"System Events\" to tell appearance preferences to set dark mode to $1" >/dev/null; }
if [ -n "${RP_APPEARANCE:-}" ]; then
    appearance_was=$(osascript -e 'tell application "System Events" to tell appearance preferences to get dark mode')
    [ "$RP_APPEARANCE" = dark ] && set_dark true || set_dark false
fi
restore() {
    [ -n "$appearance_was" ] && set_dark "$appearance_was"
    unlock
}
trap restore EXIT INT TERM

# `open` starts it in the GUI session (as Finder would), with the environment variable
open -n --env "ROOKPARCEL_SNAPSHOTS=$SHOTS" --stdout "$SHOTS/app.log" --stderr "$SHOTS/app.log" \
    "$RUN/Rookparcel.app" --args "$@"
pid=""
for _ in $(seq 1 50); do
    pid=$(pgrep -f "$EXE" | head -1 || true)
    [ -n "$pid" ] && break
    sleep 0.2
done
[ -n "$pid" ] || { echo "run.sh: the app did not start" >&2; cat "$SHOTS/app.log" >&2 || true; exit 1; }
echo "run: pid $pid"

half=$((SECONDS_TOTAL / 2))
sleep "$half"
screencapture -x "$SHOTS/screen-1.png"
if [ -n "$SCRIPT" ]; then
    RP_SHOTS="$SHOTS" osascript "$SCRIPT" > "$SHOTS/script.log" 2>&1 || echo "run: the script failed (script.log)"
fi
sleep $((SECONDS_TOTAL - half))
screencapture -x "$SHOTS/screen-2.png"
echo "run: cpu and memory (ps: %cpu rss KB)"
ps -o %cpu=,rss= -p "$pid" || true
top -l 3 -s 2 -pid "$pid" -stats pid,cpu,mem,threads | tail -2 || true

# Quit is the application menu's last item (System Events has Accessibility; telling the app
# itself to quit would ask for an Automation permission)
osascript -e 'tell application "System Events" to tell process "Rookparcel" to click last menu item of menu 1 of menu bar item 2 of menu bar 1' >/dev/null 2>&1 || true
for _ in $(seq 1 50); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.2
done
if kill -0 "$pid" 2>/dev/null; then
    echo "run: still running after quit; killing it"
    kill "$pid" || true
fi
echo "run: done; $(ls "$SHOTS" | wc -l | tr -d ' ') files in $SHOTS"
