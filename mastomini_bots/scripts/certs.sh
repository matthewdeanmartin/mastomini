#!/usr/bin/env bash
# The admin site's HTTPS certificate, from the SAME household CA mastomini
# uses (../mastomini_rs/.local/ca): every device that already trusts
# mastomini trusts mastomini-bots.local without doing anything.
#
#   bash scripts/certs.sh            create what is missing, then check it
#   bash scripts/certs.sh --reissue  new certificate, same CA
#
# Runs mastomini's scripts/certs.sh with this crate's name and output
# directory. It never makes a second household CA: without one it stops.
# MASTOMINI_BOTS_CERT_IPS adds IP addresses (the board's, to browse by IP).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
mastomini="$here/../mastomini_rs"
ca_dir="${MASTOMINI_CA_DIR:-$mastomini/.local/ca}"
if [[ ! -f "$ca_dir/rootCA.pem" || ! -f "$ca_dir/rootCA-key.pem" ]]; then
  echo "No household CA in $ca_dir. Make mastomini's first (cd ../mastomini_rs && make certs)," >&2
  echo 'so both boards share it; this script will not create a second one.' >&2
  exit 1
fi
native() { if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s' "$1"; fi; }
out="$here/certs"
mkdir -p "$out"
# certs.sh names its files after mastomini; keep ours under neutral names.
[[ ! -f "$out/server.crt" ]] || cp "$out/server.crt" "$out/mastomini.crt"
[[ ! -f "$out/server.key" ]] || cp "$out/server.key" "$out/mastomini.key"
MASTOMINI_HOSTNAME="${MASTOMINI_BOTS_HOSTNAME:-mastomini-bots}" \
MASTOMINI_CERT_DIR="$(native "$out")" \
MASTOMINI_CA_DIR="$(native "$ca_dir")" \
MASTOMINI_CERT_NAMES="${MASTOMINI_BOTS_CERT_NAMES:- }" \
MASTOMINI_CERT_IPS="${MASTOMINI_BOTS_CERT_IPS:- }" \
  bash "$mastomini/scripts/certs.sh" "$@"
mv "$out/mastomini.crt" "$out/server.crt"
mv "$out/mastomini.key" "$out/server.key"
