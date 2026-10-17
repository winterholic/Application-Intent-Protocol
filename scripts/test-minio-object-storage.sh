#!/usr/bin/env bash
set -euo pipefail

if ! command -v minio >/dev/null 2>&1 || ! command -v mc >/dev/null 2>&1; then
  echo "the pinned MinIO server and client binaries are required; install them from the versions in .github/workflows/object-storage.yml" >&2
  exit 2
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
task_dir="$(mktemp -d)"
port="${AIP_MINIO_TEST_PORT:-9000}"
server_pid=""
cleanup() {
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" >/dev/null 2>&1 || true
    wait "$server_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$task_dir"
}
trap cleanup EXIT

mkdir -p "$task_dir/minio-certs" "$task_dir/mc-config/certs/CAs" "$task_dir/data"
cat >"$task_dir/ca.cnf" <<'EOF'
[req]
distinguished_name=dn
x509_extensions=v3_ca
prompt=no
[dn]
CN=AIP ephemeral MinIO integration CA
[v3_ca]
basicConstraints=critical,CA:TRUE
keyUsage=critical,keyCertSign,cRLSign
subjectKeyIdentifier=hash
EOF
cat >"$task_dir/server.cnf" <<'EOF'
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
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -keyout "$task_dir/ca.key" -out "$task_dir/ca.pem" -config "$task_dir/ca.cnf" >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -keyout "$task_dir/minio-certs/private.key" -out "$task_dir/server.csr" -config "$task_dir/server.cnf" >/dev/null 2>&1
openssl x509 -req -in "$task_dir/server.csr" -CA "$task_dir/ca.pem" -CAkey "$task_dir/ca.key" -CAcreateserial \
  -out "$task_dir/minio-certs/public.crt" -days 2 -sha256 -extensions v3_req -extfile "$task_dir/server.cnf" >/dev/null 2>&1
cp "$task_dir/ca.pem" "$task_dir/mc-config/certs/CAs/test-ca.pem"
chmod 644 "$task_dir/minio-certs/private.key" "$task_dir/minio-certs/public.crt" "$task_dir/ca.pem"

MINIO_ROOT_USER=aip-test-access \
MINIO_ROOT_PASSWORD=aip-test-secret-please-rotate \
  minio server "$task_dir/data" --address "127.0.0.1:${port}" --certs-dir "$task_dir/minio-certs" \
  >"$task_dir/minio.log" 2>&1 &
server_pid=$!

ready=false
for _ in $(seq 1 60); do
  if curl --silent --show-error --fail --cacert "$task_dir/ca.pem" "https://localhost:${port}/minio/health/ready" >/dev/null 2>&1; then
    ready=true
    break
  fi
  sleep 1
done
if [[ "$ready" != true ]]; then
  cat "$task_dir/minio.log" >&2
  echo "MinIO did not become ready" >&2
  exit 1
fi

cp "$task_dir/ca.pem" "$task_dir/mc-config/certs/CAs/test-ca.pem"
MC_CONFIG_DIR="$task_dir/mc-config" mc alias set aip-test "https://localhost:${port}" aip-test-access aip-test-secret-please-rotate >/dev/null
MC_CONFIG_DIR="$task_dir/mc-config" mc mb aip-test/object-test-bucket >/dev/null

cd "$repo_root"
AIP_MINIO_ENDPOINT="https://localhost:${port}" \
AIP_MINIO_BUCKET=object-test-bucket \
AIP_MINIO_ACCESS_KEY=aip-test-access \
AIP_MINIO_SECRET_KEY=aip-test-secret-please-rotate \
AIP_MINIO_CA_PEM="$task_dir/ca.pem" \
  cargo test -p aip-execution --test cloud_storage minio_https_actual_object_and_multipart_contract --locked -- --ignored --nocapture

# Per-owner quota and expired-upload cleanup through the real server (needs the dedicated test PostgreSQL, see VALIDATION.md).
cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test object_ledger_s3 --locked -- --ignored --nocapture
