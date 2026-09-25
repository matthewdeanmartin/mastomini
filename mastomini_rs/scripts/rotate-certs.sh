#!/usr/bin/env bash
# Replace the household CA and the server certificate. Every device must then
# trust the new CA from /trust, so only do this when the CA is lost, leaked or
# unusable. For renewal or a new name, make reissue-cert keeps the CA.
#
# Nothing is deleted: the old files move under .local/cert-backups/<UTC time>.
# A CA shared through MASTOMINI_CA_DIR (another project's, or mkcert's) is not
# moved: only this project's server certificate is replaced.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ ${1:-} == --yes ]] || { echo 'Usage: bash scripts/rotate-certs.sh --yes' >&2; exit 2; }
command -v openssl >/dev/null || { echo 'Install OpenSSL first.' >&2; exit 1; }

out="${MASTOMINI_CERT_DIR:-certs}"
backup=".local/cert-backups/$(date -u +%Y%m%dT%H%M%SZ)"
[[ ! -e $backup ]] || { echo "Backup already exists: $backup" >&2; exit 1; }
mkdir -p "$backup/certs"

for file in mastomini.crt mastomini.key household-ca.crt household-ca.der certificate.json; do
  [[ ! -e "$out/$file" ]] || mv "$out/$file" "$backup/certs/$file"
done
if [[ -z "${MASTOMINI_CA_DIR:-}" && -d .local/ca ]]; then
  mv .local/ca "$backup/ca"
elif [[ -n "${MASTOMINI_CA_DIR:-}" ]]; then
  echo "Keeping the shared CA in $MASTOMINI_CA_DIR; only the server certificate is new."
fi

if ! bash scripts/certs.sh; then
  echo "Generating new certificates failed. The previous ones are in $backup" >&2
  exit 1
fi
echo "Previous certificates archived at $backup"
echo 'Deploy, then every device must trust the new CA from http://<board>/trust.'
