#!/usr/bin/env bash
# Offline certificate gate, run by make certs and every firmware build: the
# files the firmware embeds must be ones every client accepts. Needs only the
# public CA and the server certificate + key (not the CA's private key).
set -euo pipefail
cd "$(dirname "$0")/.."
export MSYS2_ARG_CONV_EXCL='*'

out="${MASTOMINI_CERT_DIR:-certs}"
if command -v cygpath >/dev/null 2>&1; then out=$(cygpath -m "$out"); fi
leaf="$out/mastomini.crt"
key="$out/mastomini.key"
ca="$out/household-ca.crt"
for file in "$leaf" "$key" "$ca" "$out/household-ca.der" "$out/certificate.json"; do
  [[ -f $file ]] || { echo "Missing $file. Run: make certs" >&2; exit 1; }
done
if [[ -z "${MASTOMINI_CERT_HOST:-}" ]]; then
  # Same default and lookup order as scripts/certs.sh and build.rs.
  name=${MASTOMINI_HOSTNAME:-}
  for file in .env ../.env; do
    [[ -z $name && -f $file ]] || continue
    name=$(grep -E '^[[:space:]]*(export[[:space:]]+)?MASTOMINI_HOSTNAME[[:space:]]*=' "$file" | tail -n 1 \
      | sed -e 's/^[^=]*=[[:space:]]*//' -e 's/[[:space:]]*$//' -e 's/^"\(.*\)"$/\1/' -e "s/^'\(.*\)'$/\1/" || true)
  done
  MASTOMINI_CERT_HOST="${name:-mastomini}.local"
fi
host=$MASTOMINI_CERT_HOST

fail() { echo "$1" >&2; [[ -z "${2:-}" ]] || echo "$2" >&2; exit 1; }

openssl verify -CAfile "$ca" -verify_hostname "$host" "$leaf" >/dev/null \
  || fail "The server certificate is not valid for $host under the household CA." \
          'New name or CA: make reissue-cert (same CA) or make rotate-certs (new CA).'
openssl x509 -in "$leaf" -noout -checkend 0 >/dev/null || fail 'The server certificate has expired.' 'Run: make reissue-cert, then deploy.'
openssl x509 -in "$ca" -noout -checkend 0 >/dev/null || fail 'The household CA has expired.' 'Run: make rotate-certs'
openssl pkey -in "$key" -check -noout >/dev/null

leaf_public=$(openssl x509 -in "$leaf" -pubkey -noout | openssl pkey -pubin -outform DER | openssl dgst -sha256)
key_public=$(openssl pkey -in "$key" -pubout -outform DER | openssl dgst -sha256)
[[ "$leaf_public" == "$key_public" ]] || fail 'The server certificate and key do not match.' 'Run: make reissue-cert'

der_from_pem=$(openssl x509 -in "$ca" -outform DER | openssl dgst -sha256)
der_file=$(openssl dgst -sha256 <"$out/household-ca.der")
[[ "$der_from_pem" == "$der_file" ]] || fail "$out/household-ca.der is not the household CA. Run: make certs"

leaf_text=$(openssl x509 -in "$leaf" -noout -text)
ca_text=$(openssl x509 -in "$ca" -noout -text)
grep -q 'Public Key Algorithm: rsaEncryption' <<<"$leaf_text" \
  || fail 'The server certificate must use RSA (browser and board compatibility).' 'Run: make reissue-cert'
grep -q 'TLS Web Server Authentication' <<<"$leaf_text" || fail 'The server certificate lacks TLS server usage.' 'Run: make reissue-cert'
grep -q 'CA:TRUE' <<<"$ca_text" || fail 'The household certificate is not a CA.'

# Apple refuses server certificates valid for more than 825 days.
start=$(date -u -d "$(openssl x509 -in "$leaf" -noout -startdate | sed 's/^notBefore=//')" +%s)
end=$(date -u -d "$(openssl x509 -in "$leaf" -noout -enddate | sed 's/^notAfter=//')" +%s)
(( (end - start) <= 825 * 86400 )) \
  || fail 'The server certificate is valid for more than 825 days: iPhones and Macs will refuse it.' 'Run: make reissue-cert'
days_left=$(( (end - $(date -u +%s)) / 86400 ))
if (( days_left < 60 )); then
  echo "WARNING: the server certificate expires in $days_left days. Run make reissue-cert, then deploy." >&2
fi
echo "Certificate checks passed: $host, RSA, TLS server usage, key matches, $days_left days left."
