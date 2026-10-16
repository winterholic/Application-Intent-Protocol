#!/usr/bin/env bash
set -euo pipefail

CERTS_DIR="${MINIO_CERTS_DIR:-/certs}"
DATA_DIR="${MINIO_DATA_DIR:-/data}"

mkdir -p "$CERTS_DIR" "$DATA_DIR"

if [[ ! -f "$CERTS_DIR/public.crt" || ! -f "$CERTS_DIR/private.key" || ! -f "$CERTS_DIR/ca.pem" ]]; then
  TMP_CONF="$(mktemp -d)"
  cat >"$TMP_CONF/ca.cnf" <<'EOF'
[req]
distinguished_name=dn
x509_extensions=v3_ca
prompt=no
[dn]
CN=AIP MinIO Integration CA
[v3_ca]
basicConstraints=critical,CA:TRUE
keyUsage=critical,keyCertSign,cRLSign
subjectKeyIdentifier=hash
EOF

  cat >"$TMP_CONF/server.cnf" <<'EOF'
[req]
distinguished_name=dn
req_extensions=v3_req
prompt=no
[dn]
CN=minio
[v3_req]
subjectAltName=@alt_names
keyUsage=digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
[alt_names]
DNS.1=minio
DNS.2=localhost
IP.1=127.0.0.1
EOF

  openssl req -x509 -newkey rsa:2048 -nodes -days 2 -keyout "$TMP_CONF/ca.key" -out "$CERTS_DIR/ca.pem" -config "$TMP_CONF/ca.cnf" >/dev/null 2>&1
  openssl req -newkey rsa:2048 -nodes -keyout "$CERTS_DIR/private.key" -out "$TMP_CONF/server.csr" -config "$TMP_CONF/server.cnf" >/dev/null 2>&1
  openssl x509 -req -in "$TMP_CONF/server.csr" -CA "$CERTS_DIR/ca.pem" -CAkey "$TMP_CONF/ca.key" -CAcreateserial \
    -out "$CERTS_DIR/public.crt" -days 2 -sha256 -extensions v3_req -extfile "$TMP_CONF/server.cnf" >/dev/null 2>&1
  chmod 644 "$CERTS_DIR/private.key" "$CERTS_DIR/public.crt" "$CERTS_DIR/ca.pem"
  rm -rf "$TMP_CONF"
fi

exec minio server "$DATA_DIR" --address "0.0.0.0:9000" --certs-dir "$CERTS_DIR"
