#!/usr/bin/env bash
# A ~5-minute performance check of a board, for comparing boards or builds.
# The full runs (make bench-transport, sweep.py defaults) take ~25 minutes;
# this keeps their shape with fewer samples, levels and seconds.
#
#   bash loadtests/quick_perf.sh <address> [tls-name] [ca] [label]
#   make perf-quick ADDRESS=192.168.1.164 HOST=mastomini-p4.local CA=certs-p4/household-ca.crt
#
# Read-only and anonymous (the household CA verifies HTTPS). Reports go to
# loadtests/reports/ and ../.local/quick-<label>-*; compare transport reports
# with loadtests/compare_reports.py.
set -euo pipefail
cd "$(dirname "$0")/.."
address=${1:?usage: quick_perf.sh <address> [tls-name] [ca] [label]}
host=${2:-mastomini.local}
ca=${3:-certs/household-ca.crt}
label=${4:-quick-$(date +%Y%m%d-%H%M%S)}
out=../.local
mkdir -p "$out"
run() { uv run --project loadtests python "$@"; }

echo "== handshakes (~20 s)"
run loadtests/idle_close.py --address "$address" --host "$host" --ca "$ca" --handshakes 8
echo "== HTTP/2 burst (~5 s)"
run loadtests/h2_burst.py --address "$address" --host "$host" --ca "$ca" --repeat 1
echo "== transport, short (~90 s)"
run loadtests/transport.py --address "$address" --host "$host" --ca "$ca" --label "$label" \
  --samples 5 --repeats 3 --levels 1 6 --duration 10 | grep -E "cold|warm|report:"
echo "== capacity sweep, short (~2 min)"
MASTOMINI_TLS_NAME="$host" MASTOMINI_CA="$ca" run loadtests/sweep.py --host "https://$address" \
  --levels 1,4,8,12 --seconds 15 --warmup 5 --output "$out/$label-sweep.json" > "$out/$label-sweep.log" 2>&1 || true
python - "$out/$label-sweep.json" <<'PY'
import json, sys
for s in json.load(open(sys.argv[1]))["stages"]:
    print(f"  {s['users']:2} users: {s['requests_per_second']:6.1f} req/s, p50 {s['p50_ms']} ms, "
          f"p95 {s['p95_ms']} ms, failed {s['failures']}/{s['requests']}")
PY
echo "== large transfers (~20 s)"
run loadtests/transfer_stalls.py --address "$address" --host "$host" --ca "$ca" --runs 6 --scheme both
echo "== board after"
curl -s -m 10 "http://$address/api/v1/sys" | python -c "import json,sys; d=json.load(sys.stdin); h=d['heap']; \
print(f\"internal free {h['internal_free']} min {h['internal_min']} | psram free {h['psram_free']}\")"
curl -s -m 10 "http://$address/.well-known/incidents" | python -c "import json,sys; d=json.load(sys.stdin); \
print({c['kind']: c['count'] for c in d['counters'] if c['count']})"
