#!/bin/bash
set -euo pipefail
mode="${1:-enable}"
app="${2:-/Applications/LANLink.app}"
plist="$HOME/Library/LaunchAgents/app.lanlink.agent.plist"
if [[ "$mode" == disable ]]; then
  if launchctl print "gui/$(id -u)/app.lanlink.agent" > /dev/null 2>&1; then
    launchctl bootout "gui/$(id -u)/app.lanlink.agent"
  fi
  rm -f "$plist"
  exit 0
fi
[[ "$mode" == enable && -x "$app/Contents/MacOS/lanlink-agent" ]] || exit 2
if launchctl print "gui/$(id -u)/app.lanlink.agent" > /dev/null 2>&1; then
    launchctl bootout "gui/$(id -u)/app.lanlink.agent"
  fi
mkdir -p "$(dirname "$plist")"
/usr/bin/plutil -create xml1 "$plist"
/usr/bin/plutil -insert Label -string app.lanlink.agent "$plist"
/usr/bin/plutil -insert ProgramArguments -array "$plist"
/usr/bin/plutil -insert ProgramArguments.0 -string "$app/Contents/MacOS/lanlink-agent" "$plist"
/usr/bin/plutil -insert ProgramArguments.1 -string --web-dir "$plist"
/usr/bin/plutil -insert ProgramArguments.2 -string "$app/Contents/Resources/web" "$plist"
/usr/bin/plutil -insert RunAtLoad -bool true "$plist"
launchctl bootstrap "gui/$(id -u)" "$plist"
