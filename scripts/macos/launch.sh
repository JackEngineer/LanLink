#!/bin/bash
set -euo pipefail
contents="$(cd "$(dirname "$0")/.." && pwd)"
logs="$HOME/Library/Logs/LANLink"
mkdir -p "$logs"
nohup "$contents/MacOS/lanlink-agent" --web-dir "$contents/Resources/web" >> "$logs/launcher.log" 2>&1 < /dev/null &
for attempt in {1..40}; do
  health=$(/usr/bin/curl -fsS --max-time 1 http://127.0.0.1:38761/api/v1/health 2>/dev/null || true)
  if [[ "$health" == *'"service":"lanlink"'* && "$health" == *'"status":"ok"'* ]]; then
    open http://127.0.0.1:38761
    exit 0
  fi
  sleep 0.25
done
osascript -e 'display alert "LANLink 未能启动" message "请检查 ~/Library/Logs/LANLink/launcher.log。"'
exit 1
