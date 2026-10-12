#!/usr/bin/env bash
set -euo pipefail

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required for the MinIO object-storage integration test" >&2
  exit 2
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
task_dir="$(mktemp -d)"
network="aip-minio-objects-$$"
container="aip-minio-objects-$$"
port="${AIP_MINIO_TEST_PORT:-9000}"
cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  docker network rm "$network" >/dev/null 2>&1 || true
  rm -rf "$task_dir"
}
trap cleanup EXIT

mkdir -p "$task_dir/minio-certs" "$task_dir/mc-config/certs/CAs"
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

docker network create "$network" >/dev/null
docker run --detach --name "$container" --network "$network" --network-alias minio \
  --publish "127.0.0.1:${port}:9000" \
  --env MINIO_ROOT_USER=aip-test-access \
  --env MINIO_ROOT_PASSWORD=aip-test-secret-please-rotate \
  --volume "$task_dir/minio-certs:/root/.minio/certs:ro" \
  quay.io/minio/minio:RELEASE.2025-09-06T17-38-46Z server /data --console-address :9001 >/dev/null

ready=false
for _ in $(seq 1 60); do
  if curl --silent --show-error --fail --cacert "$task_dir/ca.pem" "https://localhost:${port}/minio/health/ready" >/dev/null 2>&1; then
    ready=true
    break
  fi
  sleep 1
done
if [[ "$ready" != true ]]; then
  docker logs "$container" >&2
  echo "MinIO did not become ready" >&2
  exit 1
fi

docker run --rm --network "$network" \
  --env MC_CONFIG_DIR=/root/.mc \
  --volume "$task_dir/mc-config:/root/.mc" \
  quay.io/minio/mc:RELEASE.2025-08-13T08-35-41Z \
  alias set aip-test https://minio:9000 aip-test-access aip-test-secret-please-rotate >/dev/null
docker run --rm --network "$network" \
  --env MC_CONFIG_DIR=/root/.mc \
  --volume "$task_dir/mc-config:/root/.mc" \
  quay.io/minio/mc:RELEASE.2025-08-13T08-35-41Z \
  mb aip-test/object-test-bucket >/dev/null

cd "$repo_root"
AIP_MINIO_ENDPOINT="https://localhost:${port}" \
AIP_MINIO_BUCKET=object-test-bucket \
AIP_MINIO_ACCESS_KEY=aip-test-access \
AIP_MINIO_SECRET_KEY=aip-test-secret-please-rotate \
AIP_MINIO_CA_PEM="$task_dir/ca.pem" \
  cargo test -p aip-execution --test cloud_storage minio_https_actual_object_and_multipart_contract --locked -- --ignored --nocapture
