#!/usr/bin/env bash
# Regenerates the throwaway CAs and server certificates used by the TLS probe.
#
# Two sets on purpose:
#   certs/      signed by the CA embedded in the probe APK  -> must be trusted
#   certs/ca2/  signed by a different CA                     -> must be rejected
#
# Nothing here is secret: these keys exist only to make a local 127.0.0.1 test
# server. They are gitignored anyway so nobody mistakes them for credentials.
set -euo pipefail

cd "$(dirname "$0")"

issue() {
  local dir="$1" cn="$2"
  mkdir -p "$dir"
  cd "$dir"
  openssl req -x509 -newkey rsa:2048 -sha256 -days 3650 -nodes \
    -keyout ca.key -out ca.pem -subj "/CN=$cn/O=AICC" \
    -addext "basicConstraints=critical,CA:TRUE" >/dev/null 2>&1
  openssl req -newkey rsa:2048 -sha256 -nodes \
    -keyout server.key -out server.csr -subj "/CN=127.0.0.1/O=AICC" >/dev/null 2>&1
  printf 'subjectAltName=IP:127.0.0.1,DNS:localhost\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n' > san.ext
  openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial \
    -out server.pem -days 3650 -sha256 -extfile san.ext >/dev/null 2>&1
  cd - >/dev/null
}

issue certs "AICC Test CA"
issue certs/ca2 "AICC Other CA"

# The probe trusts only the first CA; it ships as a raw resource.
cp certs/ca.pem ../app/src/main/res/raw/aicc_test_ca.pem

echo "SPKI pin of certs/server.pem (put it in MainActivity.EXPECTED_PIN):"
openssl x509 -in certs/server.pem -pubkey -noout \
  | openssl pkey -pubin -outform der \
  | openssl dgst -sha256 -binary | openssl base64
