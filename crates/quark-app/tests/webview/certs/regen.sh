#!/usr/bin/env bash
# Regenerates the webview fixture's synthetic TLS material. Test use only.
#
# ca.pem signs leaf.pem (127.0.0.1 and localhost); rogue-leaf.pem is signed
# by a throwaway CA that is never written out, so it must stay untrusted.
# Leaves last 825 days, the longest validity Apple's TLS policy accepts for
# any server certificate; the CA lasts a century. With --ca the CA is
# replaced too.
set -euo pipefail
cd "$(dirname "$0")"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

leaf_ext="$tmp/leaf.ext"
printf '%s\n' \
  'basicConstraints=critical,CA:FALSE' \
  'keyUsage=critical,digitalSignature,keyEncipherment' \
  'extendedKeyUsage=serverAuth' \
  'subjectAltName=IP:127.0.0.1,DNS:localhost' > "$leaf_ext"

new_ca() { # key out, cert out, common name
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
    -keyout "$1" -out "$2" -days 36500 -subj "/CN=$3" \
    -addext 'basicConstraints=critical,CA:TRUE,pathlen:0' \
    -addext 'keyUsage=critical,keyCertSign,cRLSign' 2>/dev/null
}

new_leaf() { # ca cert, ca key, leaf cert out, leaf pkcs8 key out
  openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes \
    -keyout "$tmp/leaf.key" -out "$tmp/leaf.csr" -subj '/CN=127.0.0.1' 2>/dev/null
  openssl x509 -req -in "$tmp/leaf.csr" -CA "$1" -CAkey "$2" \
    -CAserial "$tmp/serial" -CAcreateserial -out "$3" -days 825 \
    -extfile "$leaf_ext" 2>/dev/null
  openssl pkcs8 -topk8 -nocrypt -in "$tmp/leaf.key" -out "$4"
}

if [[ "${1:-}" == --ca || ! -f ca.key ]]; then
  new_ca ca.key ca.pem 'quark webview fixture test CA'
fi
new_leaf ca.pem ca.key leaf.pem leaf.key
new_ca "$tmp/rogue-ca.key" "$tmp/rogue-ca.pem" 'quark webview fixture rogue CA'
new_leaf "$tmp/rogue-ca.pem" "$tmp/rogue-ca.key" rogue-leaf.pem rogue-leaf.key
chmod 644 ./*.key
openssl verify -CAfile ca.pem leaf.pem
