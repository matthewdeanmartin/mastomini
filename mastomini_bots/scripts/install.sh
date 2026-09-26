#!/usr/bin/env bash
# FIRST INSTALL ONLY (erases the board after a full backup). See DEPLOY.md.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -n "${1:-}" ]] || { echo 'Usage: bash scripts/install.sh COM12 --dry-run | --confirm-mac aa:bb:cc:dd:ee:ff' >&2; exit 2; }
port=$1; shift
source scripts/esp-python.sh
bash scripts/build-esp32.sh
"$esp_python" scripts/install.py --port "$port" --images "$images" \
  --backup-dir ../.local/board-backups "$@"
