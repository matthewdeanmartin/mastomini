#!/usr/bin/env bash
# Household certificates for HTTPS (docs/security/https.md, spec/05 Model A).
#
#   bash scripts/certs.sh            create what is missing, then check it
#   bash scripts/certs.sh --reissue  new server certificate, same CA (devices
#                                    keep trusting it: renewal, new name or IP)
#
# Two parts:
#   - the household CA, in $MASTOMINI_CA_DIR (default .local/ca): rootCA.pem
#     and rootCA-key.pem, the same names nanacoin and mkcert use. If that
#     directory already holds a CA, it is reused; otherwise a new one is made.
#     Its private key never leaves this computer: it is not copied, bundled,
#     flashed or served.
#   - the server certificate, in $MASTOMINI_CERT_DIR (default certs):
#     mastomini.crt + mastomini.key (embedded in the firmware), plus the CA's
#     public certificate (household-ca.crt / .der, served at /ca) and
#     certificate.json (names and expiry, shown in the household app).
#
# Settings (environment, else the first .env / ../.env that sets them):
#   MASTOMINI_HOSTNAME   mDNS name, default mastomini (-> mastomini.local)
#   MASTOMINI_CERT_NAMES extra DNS names, comma separated (mastomini.lan,...)
#   MASTOMINI_CERT_IPS   extra IP addresses, comma separated (192.168.1.161)
#
# Nothing is installed in this computer's trust store, and existing files are
# never replaced silently: see scripts/rotate-certs.sh for a new CA.
set -euo pipefail
cd "$(dirname "$0")/.."
umask 077

reissue=
case "${1:-}" in
  '') ;;
  --reissue) reissue=1 ;;
  *) echo 'Usage: bash scripts/certs.sh [--reissue]' >&2; exit 2 ;;
esac
command -v openssl >/dev/null || { echo 'Install OpenSSL first (Git for Windows includes it).' >&2; exit 1; }
# Git Bash otherwise rewrites a leading /O= X.509 subject into a Windows path.
export MSYS2_ARG_CONV_EXCL='*'

# Same lookup as build.rs, so the certificate names match the firmware.
setting() {
  local key=$1 file line value
  if [[ -n "${!key:-}" ]]; then printf '%s' "${!key}"; return; fi
  for file in .env ../.env; do
    [[ -f $file ]] || continue
    line=$(grep -E "^[[:space:]]*(export[[:space:]]+)?$key[[:space:]]*=" "$file" | tail -n 1 || true)
    [[ -n $line ]] || continue
    value=${line#*=}
    value=$(sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' -e "s/^\"\(.*\)\"$/\1/" -e "s/^'\(.*\)'$/\1/" <<<"$value")
    printf '%s' "$value"
    return
  done
}

# Paths as C:/... on Windows: native openssl must see them unconverted.
native() { if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s' "$1"; fi; }

host="$(setting MASTOMINI_HOSTNAME)"
host="${host:-mastomini}.local"
ca_dir="$(native "${MASTOMINI_CA_DIR:-.local/ca}")"
out="$(native "${MASTOMINI_CERT_DIR:-certs}")"
root_cert="$ca_dir/rootCA.pem"
root_key="$ca_dir/rootCA-key.pem"
leaf="$out/mastomini.crt"
key="$out/mastomini.key"

# Subject alternative names: the board's name first, then localhost for the
# desktop server, then anything the household asked for.
names=("$host" localhost)
ips=(127.0.0.1)
IFS=',' read -r -a extra <<<"$(setting MASTOMINI_CERT_NAMES)"
for n in "${extra[@]}"; do n=${n// /}; [[ -z $n ]] || names+=("$n"); done
IFS=',' read -r -a extra <<<"$(setting MASTOMINI_CERT_IPS)"
for n in "${extra[@]}"; do n=${n// /}; [[ -z $n ]] || ips+=("$n"); done

# --- signing ------------------------------------------------------------
# Certificates start a day in the past. Made in the evening in the Americas,
# a start of "now" is already tomorrow's date in UTC, which is what devices
# display, and a device whose clock runs behind would reject the certificate
# as not valid yet. `openssl ca` signs rather than `req -x509` / `x509 -req`
# because OpenSSL 3.2 (Git for Windows) can set a start date only there.
work=$(native "$(mktemp -d)")
trap 'rm -rf "$work"' EXIT
: >"$work/index.txt"
cat >"$work/ca.cnf" <<EOF
[ca]
default_ca = household
[household]
database = $work/index.txt
new_certs_dir = $work
rand_serial = yes
default_md = sha256
policy = anything
unique_subject = no
email_in_dn = no
[anything]
organizationName = optional
organizationalUnitName = optional
commonName = supplied
EOF
start=$(( $(date -u +%s) - 86400 ))
stamp() { date -u -d "@$1" +%Y%m%d%H%M%SZ; }

# sign <csr> <certificate out> <days> <extensions file> <signer arguments...>
sign() {
  local csr=$1 cert=$2 days=$3 ext=$4
  shift 4
  openssl ca -batch -notext -preserveDN -config "$work/ca.cnf" -in "$csr" -out "$cert" \
    -startdate "$(stamp "$start")" -enddate "$(stamp $(( start + days * 86400 )))" \
    -extfile "$ext" "$@" 2>"$work/ca.log" || { cat "$work/ca.log" >&2; return 1; }
}

# --- the household CA ---------------------------------------------------
if [[ -e $root_cert || -e $root_key ]]; then
  [[ -f $root_cert && -f $root_key ]] || { echo "Incomplete CA in $ca_dir (need rootCA.pem and rootCA-key.pem); refusing to replace it." >&2; exit 1; }
  echo "Household CA: $ca_dir"
else
  mkdir -p "$ca_dir"
  # A century: devices install it once. RSA for the widest client support.
  # Name constraints limit what the CA can vouch for to household names and
  # private addresses, so a device that trusts it cannot be fooled about
  # public sites even if this key leaked. MASTOMINI_CA_NAME_CONSTRAINTS=0
  # leaves them out (a CA that must also sign public-looking names).
  cat >"$work/ca.ext" <<'EOF'
basicConstraints=critical,CA:TRUE,pathlen:0
keyUsage=critical,keyCertSign,cRLSign
subjectKeyIdentifier=hash
EOF
  if [[ "${MASTOMINI_CA_NAME_CONSTRAINTS:-1}" != 0 ]]; then
    echo 'nameConstraints=critical,permitted;DNS:local,permitted;DNS:localhost,permitted;DNS:lan,permitted;DNS:home.arpa,permitted;DNS:internal,permitted;IP:10.0.0.0/255.0.0.0,permitted;IP:172.16.0.0/255.240.0.0,permitted;IP:192.168.0.0/255.255.0.0,permitted;IP:127.0.0.0/255.0.0.0' >>"$work/ca.ext"
  fi
  # Named with this computer's date, the day the household made it.
  openssl req -quiet -new -newkey rsa:3072 -sha256 -nodes -keyout "$root_key" -out "$work/ca.csr" \
    -subj "/O=mastomini household/OU=Private home CA/CN=mastomini Household CA $(date +%Y-%m-%d)"
  sign "$work/ca.csr" "$root_cert" 36525 "$work/ca.ext" -selfsign -keyfile "$root_key"
  echo "Created a new household CA in $ca_dir. Every device must trust it once (/trust)."
fi

# --- the server certificate ---------------------------------------------
mkdir -p "$out"
if [[ -n $reissue && ( -e $leaf || -e $key ) ]]; then
  backup=".local/cert-backups/$(date -u +%Y%m%dT%H%M%SZ)-reissue"
  mkdir -p "$backup"
  for f in "$leaf" "$key"; do [[ ! -e $f ]] || mv "$f" "$backup/"; done
  echo "Previous server certificate archived at $backup"
fi
if [[ -e $leaf || -e $key ]]; then
  [[ -f $leaf && -f $key ]] || { echo "Incomplete server certificate in $out; run: make reissue-cert" >&2; exit 1; }
else
  san=""
  for n in "${names[@]}"; do san+="${san:+,}DNS:$n"; done
  for n in "${ips[@]}"; do san+=",IP:$n"; done
  cat >"$work/leaf.ext" <<EOF
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=$san
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid,issuer
EOF
  # RSA-2048: what the board's mbedTLS and every client handle well (nanacoin
  # saw Firefox reject an ECDSA appliance certificate). 820 days in all,
  # counting the day of backdating: Apple rejects server certificates valid
  # for more than 825 days, even from a CA the user installed. Renew with
  # make reissue-cert; devices keep trusting the same CA.
  openssl req -quiet -new -newkey rsa:2048 -sha256 -nodes -keyout "$key" -out "$work/leaf.csr" \
    -subj "/O=mastomini household/CN=$host"
  sign "$work/leaf.csr" "$leaf" 820 "$work/leaf.ext" -cert "$root_cert" -keyfile "$root_key"
  echo "Issued a server certificate for: ${names[*]} ${ips[*]}"
fi
local_date() { date -d "$(openssl x509 -in "$1" -noout "-$2" | sed 's/^[^=]*=//')" '+%Y-%m-%d %H:%M'; }
echo "Server certificate valid from $(local_date "$leaf" startdate) to $(local_date "$leaf" enddate), this computer's time."

# --- derived files: always rebuilt from the two above -------------------
cp "$root_cert" "$out/household-ca.crt"
openssl x509 -in "$root_cert" -outform DER -out "$out/household-ca.der"
not_after=$(openssl x509 -in "$leaf" -noout -enddate | sed 's/^notAfter=//')
not_after_iso=$(date -u -d "$not_after" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || printf '%s' "$not_after")
leaf_names=$(openssl x509 -in "$leaf" -noout -ext subjectAltName | tail -n +2 | tr ',' '\n' \
  | sed -e 's/^[[:space:]]*//' -e 's/^DNS://' -e 's/^IP Address://' | grep -v '^$')
ca_name=$(openssl x509 -in "$root_cert" -noout -subject -nameopt multiline | sed -n 's/^ *commonName *= *//p')
json_names=$(printf '%s\n' "$leaf_names" | sed 's/.*/"&"/' | paste -sd, -)
cat >"$out/certificate.json" <<EOF
{"names":[$json_names],"not_after":"$not_after_iso","ca_name":"$ca_name"}
EOF
chmod 644 "$out/household-ca.crt" "$out/household-ca.der" "$out/certificate.json" "$leaf"

MASTOMINI_CERT_HOST="$host" bash scripts/certs-check.sh
echo 'Household CA SHA-256 fingerprint (compare it on each device, or on /trust over a trusted channel):'
openssl x509 -in "$root_cert" -noout -fingerprint -sha256 | sed 's/^.*=//'
