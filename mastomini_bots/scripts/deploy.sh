#!/usr/bin/env bash
# Routine upgrade: rebuild, verify the mastomini-bots layout (and that the
# board is not mastomini), write the app only. See DEPLOY.md.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -n "${1:-}" ]] || { echo 'Usage: bash scripts/deploy.sh COM12 [--dry-run]' >&2; exit 2; }
port=$1; shift
[[ $# == 0 || ( $# == 1 && $1 == --dry-run ) ]] || { echo 'Only --dry-run is accepted.' >&2; exit 2; }
source scripts/esp-python.sh
bash scripts/build-esp32.sh
"$esp_python" scripts/deploy.py --port "$port" --image "$images/mastomini-bots-esp32.bin" "$@"
