#!/usr/bin/env bash
# Build the household app (../mastomini_ui) and bundle it into .embuild/web
# for `--features bundled-web` (spec/06). Firmware builds run this first.
set -euo pipefail
cd "$(dirname "$0")/../../mastomini_ui"
if [[ ! -d node_modules ]]; then
  npm ci
fi
npm run build
cd ../mastomini_rs
node scripts/bundle-web.mjs
