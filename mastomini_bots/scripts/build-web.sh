#!/usr/bin/env bash
# Build the admin app (../mastomini_bots_ui) and bundle it into .embuild/web
# for `--features bundled-web`. Firmware builds run this first.
set -euo pipefail
cd "$(dirname "$0")/../../mastomini_bots_ui"
if [[ ! -d node_modules ]]; then
  npm ci --no-audit --no-fund
fi
npm run build
cd ../mastomini_bots
node scripts/bundle-web.mjs
