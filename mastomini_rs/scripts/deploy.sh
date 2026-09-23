#!/usr/bin/env bash
# Routine upgrade: rebuild, verify the mastomini layout, write the app only.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -n "${1:-}" ]] || { echo 'Usage: bash scripts/deploy.sh COM11 [--dry-run]' >&2; exit 2; }
port=$1; shift
[[ $# == 0 || ( $# == 1 && $1 == --dry-run ) ]] || { echo 'Only --dry-run is accepted.' >&2; exit 2; }
source scripts/esp-python.sh
bash scripts/build-esp32.sh
"$esp_python" scripts/deploy.py --port "$port" --image "$images/mastomini-esp32.bin" "$@"
