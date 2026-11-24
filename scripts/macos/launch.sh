#!/bin/bash
set -euo pipefail
contents="$(cd "$(dirname "$0")/.." && pwd)"
logs="$HOME/Library/Logs/LANLink"
mkdir -p "$logs"
version=$(/usr/libexec/PlistBuddy -c 'Print :LANLinkAgentVersion' "$contents/Info.plist")
mode="${1:-}"
case "$mode" in ''|--browser) ;; *) exit 64;; esac
shell="$contents/Helpers/LANLink Workbench.app"
read_health() {
  /usr/bin/curl --noproxy '*' -sS --max-time 1 --max-filesize 8192 \
    http://127.0.0.1:38761/api/v1/health 2>/dev/null || true
}
matches() {
  [[ "$1" == *'"service":"lanlink"'* && "$1" == *'"status":"ok"'* && "$1" == *"\"version\":\"$version\""* ]]
}
health=$(read_health)
if [[ -n "$health" ]] && ! matches "$health"; then
  printf '%s\n' 'LANLink: current localhost service is another version; not stopping it.' >>"$logs/launcher.log"
  exit 69
fi
if [[ -z "$health" ]]; then
  nohup "$contents/MacOS/lanlink-agent" --web-dir "$contents/Resources/web" >> "$logs/launcher.log" 2>&1 < /dev/null &
fi
for attempt in {1..40}; do
  if matches "$health"; then
    if [[ "$mode" == --browser ]]; then
      /usr/bin/open http://127.0.0.1:38761/
    else
      /usr/bin/open "$shell" || {
        printf '%s\n' 'LANLink: Workbench could not be opened; browser recovery is explicit.' >>"$logs/launcher.log"
        exit 72
      }
    fi
    exit 0
  fi
  if [[ -n "$health" ]]; then break; fi
  sleep 0.25
  health=$(read_health)
done
printf '%s\n' 'LANLink: the expected Agent did not become ready.' >>"$logs/launcher.log"
exit 71
