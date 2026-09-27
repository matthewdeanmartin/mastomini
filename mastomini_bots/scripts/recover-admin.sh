#!/usr/bin/env bash
# Explicit physical-USB recovery: app-only upgrade, then reset only the
# admin password once. Bot settings, API keys and memory are preserved.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -n "${1:-}" ]] || { echo 'Usage: bash scripts/recover-admin.sh COM15 [--dry-run]' >&2; exit 2; }
recovery_id="$(python -c 'import uuid; print(uuid.uuid4().hex)')"
echo 'One-time admin recovery: the next boot will reopen password setup; bot settings and keys are preserved.'
MASTOMINI_BOTS_RESET_ADMIN_ONCE="$recovery_id" bash scripts/deploy.sh "$@"
