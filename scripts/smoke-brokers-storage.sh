#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
compose_file="$repo_root/conformance/brokers-storage/docker-compose.yml"
certs_dir="$repo_root/conformance/brokers-storage/certs"

cleanup() {
  echo "==> Cleaning up broker & storage containers..."
  docker compose -f "$compose_file" down -v >/dev/null 2>&1 || true
  rm -rf "$certs_dir"
}
trap cleanup EXIT

mkdir -p "$certs_dir"

echo "==> Starting Kafka, RabbitMQ, and TLS MinIO containers..."
docker compose -f "$compose_file" up -d

echo "==> Waiting for services to become healthy..."
# 1. Wait for MinIO certs & readiness
minio_ready=false
for _ in $(seq 1 60); do
  if [[ -f "$certs_dir/ca.pem" ]] && curl --silent --show-error --fail --cacert "$certs_dir/ca.pem" "https://localhost:9000/minio/health/ready" >/dev/null 2>&1; then
    minio_ready=true
    break
  fi
  sleep 1
done
if [[ "$minio_ready" != true ]]; then
  echo "MinIO did not become ready" >&2
  docker compose -f "$compose_file" logs minio >&2
  exit 1
fi
echo "  [OK] MinIO TLS is ready"

# 2. Wait for RabbitMQ
rabbit_ready=false
for _ in $(seq 1 60); do
  if docker compose -f "$compose_file" exec -T rabbitmq rabbitmq-diagnostics -q ping >/dev/null 2>&1; then
    rabbit_ready=true
    break
  fi
  sleep 1
done
if [[ "$rabbit_ready" != true ]]; then
  echo "RabbitMQ did not become ready" >&2
  docker compose -f "$compose_file" logs rabbitmq >&2
  exit 1
fi
echo "  [OK] RabbitMQ is ready"

# 3. Wait for Kafka
kafka_ready=false
for _ in $(seq 1 60); do
  if docker compose -f "$compose_file" exec -T kafka /opt/kafka/bin/kafka-topics.sh --bootstrap-server localhost:9092 --list >/dev/null 2>&1; then
    kafka_ready=true
    break
  fi
  sleep 1
done
if [[ "$kafka_ready" != true ]]; then
  echo "Kafka did not become ready" >&2
  docker compose -f "$compose_file" logs kafka >&2
  exit 1
fi
echo "  [OK] Kafka is ready"

echo "==> Provisioning Kafka topics and MinIO buckets..."
docker compose -f "$compose_file" exec -T kafka /opt/kafka/bin/kafka-topics.sh --bootstrap-server localhost:9092 --create --if-not-exists --topic aip-ci-outbound --partitions 1 --replication-factor 1 >/dev/null
docker compose -f "$compose_file" exec -T kafka /opt/kafka/bin/kafka-topics.sh --bootstrap-server localhost:9092 --create --if-not-exists --topic aip-ci-inbound --partitions 1 --replication-factor 1 >/dev/null
docker compose -f "$compose_file" exec -T kafka /opt/kafka/bin/kafka-topics.sh --bootstrap-server localhost:9092 --create --if-not-exists --topic aip-ci-inbound-dead --partitions 1 --replication-factor 1 >/dev/null

docker compose -f "$compose_file" exec -T minio mc alias set local https://127.0.0.1:9000 aip-test-access aip-test-secret-please-rotate --insecure >/dev/null
docker compose -f "$compose_file" exec -T minio mc mb local/object-test-bucket --insecure >/dev/null 2>&1 || true

echo "==> Running live Kafka and RabbitMQ integration tests..."
cd "$repo_root"

# Validate the dedicated worker DSNs and explicit fallback port before starting
# any live DB test. The Rust tests cannot silently use libpq's port 5432 default.
cargo test -p aip-test-support --features strict-db --locked

export AIP_TEST_KAFKA_BOOTSTRAP="127.0.0.1:9092"
export AIP_TEST_KAFKA_OUT_TOPIC="aip-ci-outbound"
export AIP_TEST_KAFKA_IN_TOPIC="aip-ci-inbound"
export AIP_TEST_RABBITMQ_URI="amqp://aiptest:aip-fixture-password@127.0.0.1:5672/%2f"
export AIP_TEST_RABBITMQ_OUT_QUEUE="aip-ci-outbound"
export AIP_TEST_RABBITMQ_IN_QUEUE="aip-ci-inbound"
export AIP_TEST_DATABASE="${AIP_TEST_DATABASE_URL:?AIP_TEST_DATABASE_URL is required}"

if [[ -d "/opt/homebrew/opt/openssl@3" ]]; then
  export OPENSSL_DIR="/opt/homebrew/opt/openssl@3"
fi

cargo test --locked -p aip-execution --features external-brokers --test brokers_live -- --ignored --nocapture
cargo test --locked -p aip-service --features external-brokers broker_worker_recovers -- --ignored --nocapture

echo "==> Running live MinIO object storage integration tests..."
export AIP_MINIO_ENDPOINT="https://localhost:9000"
export AIP_MINIO_BUCKET="object-test-bucket"
export AIP_MINIO_ACCESS_KEY="aip-test-access"
export AIP_MINIO_SECRET_KEY="aip-test-secret-please-rotate"
export AIP_MINIO_CA_PEM="$certs_dir/ca.pem"

cargo test -p aip-execution --test cloud_storage minio_https_actual_object_and_multipart_contract --locked -- --ignored --nocapture

echo "==> [SUCCESS] All live broker and object storage smoke tests passed!"
