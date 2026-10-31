#!/usr/bin/env bash
# Steady traffic against every API flow. Usage: ./load.sh [api_url] [interval_seconds]
set -euo pipefail
API="${1:-http://localhost:8081}"
INTERVAL="${2:-1}"
STARS=(Vega Sirius Rigel Deneb Altair Antares)
SITES=(paranal mauna-kea la-palma)

i=0
while true; do
  star=${STARS[$((i % ${#STARS[@]}))]}
  site=${SITES[$((i % ${#SITES[@]}))]}
  curl -fsS -o /dev/null "$API/stars" || true
  curl -fsS -o /dev/null -H 'content-type: application/json' \
    -d "{\"star\":\"$star\",\"site\":\"$site\"}" "$API/observations" || true
  curl -fsS -o /dev/null "$API/catalog/summary" || true
  if (( i % 3 == 0 )); then
    curl -fsS -o /dev/null -X POST "$API/catalog/recompute" || true
  fi
  i=$((i + 1))
  sleep "$INTERVAL"
done
