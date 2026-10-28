#!/bin/bash
# relay_check.sh - a quiet inbox check, good for a timer or a watcher loop.
#
# It reads Claude's relay inbox. Plain "status" messages are logged to a file and acknowledged
# automatically. Anything else (question, alert, done, instruction) is printed so a person or
# an AI can act on it.
#
# Exit code: 0 = something needs attention (it was printed), 1 = nothing to do, 2 = error (message on stderr).
#
# Requirements: grokbot-claude-relay installed and initialized, plus jq (sudo apt install jq).
#
# Settings (environment variables, all optional):
#   RELAY_BIN   Path to the program. Default: grokbot-claude-relay (found on your PATH)
#   RELAY_AS    Which participant's inbox to read. Default: claude
#   STATUS_LOG  File that collects status messages. Default: $HOME/.local/share/grokbot-claude-relay/status.log
R="${RELAY_BIN:-grokbot-claude-relay}"
WHO="${RELAY_AS:-claude}"
LOG="${STATUS_LOG:-$HOME/.local/share/grokbot-claude-relay/status.log}"

command -v jq >/dev/null || { echo "jq is not installed" >&2; exit 2; }
mkdir -p "$(dirname "$LOG")"

j=$($R --as "$WHO" inbox --json --limit 50) || exit 2
{ [ -z "$j" ] || [ "$j" = "[]" ]; } && exit 1

need=$(printf '%s' "$j" | jq -r '.[] | select(.kind != "status") | "#\(.id) \(.ts) \(.sender) [\(.kind)] ref=\(.ref // "") :: \(.body)"')
maxs=$(printf '%s' "$j" | jq -r '[.[] | select(.kind == "status") | .id] | max // empty')

if [ -z "$need" ]; then
  printf '%s' "$j" | jq -r '.[] | select(.kind == "status") | "#\(.id) \(.ts) \(.sender) ref=\(.ref // "") :: \(.body)"' >> "$LOG"
  [ -n "$maxs" ] && $R --as "$WHO" ack "$maxs" >/dev/null 2>&1
  exit 1
fi
echo "$need"
exit 0
