#!/bin/bash
# grokbot_send.sh "<one-line message>"
#
# Types one message into the chat box of the Grok Bot desktop app and presses Enter,
# then gives keyboard focus back to the window you were using.
# Use it as a nudge: "check your relay inbox".
#
# Requirements: Linux with X11 (not Wayland), plus xdotool and wmctrl.
#   sudo apt install xdotool wmctrl
#
# Settings (environment variables, all optional):
#   GROKBOT_TITLE   Window title to look for. Default: "Grok Bot". The title must END with this text.
#   CLICK_X_PCT     Where to click, as a percent of window width, to focus the chat box. Default: 48
#   CLICK_Y_PCT     Where to click, as a percent of window height. Default: 97
#                   The defaults aim at a chat box near the bottom middle. If your app looks
#                   different, change these two numbers until the click lands in the box.
set -u

TITLE="${GROKBOT_TITLE:-Grok Bot}"
XPCT="${CLICK_X_PCT:-48}"
YPCT="${CLICK_Y_PCT:-97}"

if [ $# -lt 1 ] || [ -z "$1" ]; then
  echo "usage: $0 \"message\"" >&2
  exit 1
fi
for tool in wmctrl xdotool; do
  command -v "$tool" >/dev/null || { echo "$tool is not installed" >&2; exit 3; }
done

# Find the window id. awk matches the title at the end of the line.
W=$(wmctrl -l | awk -v t="$TITLE" 'index($0, t) && substr($0, length($0) - length(t) + 1) == t {print $1; exit}')
[ -n "$W" ] || { echo "window \"$TITLE\" not found" >&2; exit 2; }

PREV=$(xdotool getactivewindow 2>/dev/null)
eval "$(xdotool getwindowgeometry --shell "$W")"   # sets X Y WIDTH HEIGHT

wmctrl -i -a "$W"; sleep 0.6
xdotool mousemove $((X + WIDTH * XPCT / 100)) $((Y + HEIGHT * YPCT / 100)) click 1; sleep 0.3
xdotool type --delay 4 --clearmodifiers -- "$1"; sleep 0.3
xdotool key Return; sleep 0.5
[ -n "${PREV:-}" ] && xdotool windowactivate "$PREV" 2>/dev/null
echo sent
