#!/usr/bin/env bash
# Live model tests (e2e/test_llm.py): a real mastomini, mastomini-bots and
# OpenRouter. The key comes from OPENROUTER_API_KEY, or from the .env file
# named by OPENROUTER_ENV_FILE; it is never printed. A few small calls to a
# cheap model (MASTOBOTS_TEST_MODEL, default google/gemma-4-31b-it).
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ -z "${OPENROUTER_API_KEY:-}" && -n "${OPENROUTER_ENV_FILE:-}" ]]; then
  line=$(grep -E '^[[:space:]]*(export[[:space:]]+)?OPENROUTER_API_KEY[[:space:]]*=' "$OPENROUTER_ENV_FILE" | tail -n 1 || true)
  value=${line#*=}
  value=$(sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' -e 's/^"\(.*\)"$/\1/' -e "s/^'\(.*\)'$/\1/" <<<"$value")
  export OPENROUTER_API_KEY="$value"
fi
if [[ -z "${OPENROUTER_API_KEY:-}" ]]; then
  echo 'Set OPENROUTER_API_KEY, or OPENROUTER_ENV_FILE=path/to/.env holding it.' >&2
  exit 2
fi
uv run --project "${CLIENTTESTS:-../mastomini_rs/clienttests}" pytest e2e/test_llm.py -q -s -p no:cacheprovider
