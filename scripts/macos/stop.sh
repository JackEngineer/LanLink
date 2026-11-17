#!/bin/bash
set -euo pipefail
app="${1:-/Applications/LANLink.app}"
binary="$app/Contents/MacOS/lanlink-agent"
pids=$(ps -axww -o pid=,comm= | awk -v executable="$binary" '{ pid=$1; sub(/^[[:space:]]*[0-9]+[[:space:]]+/, ""); if ($0 == executable) print pid }')
while IFS= read -r pid; do
  [[ -n "$pid" ]] || continue
  kill -INT "$pid"
  for attempt in {1..40}; do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.25
  done
  if kill -0 "$pid" 2>/dev/null; then
    printf 'LANLink process %s did not stop.\n' "$pid" >&2
    exit 1
  fi
done <<< "$pids"
