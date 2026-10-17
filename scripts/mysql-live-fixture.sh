#!/usr/bin/env bash
# Disposable MySQL 8.4 / MariaDB 11.4 fixture for the live adapter tests (WBS 7.2.1).
#
#   scripts/mysql-live-fixture.sh up     start both containers (TLS on, tmpfs datadir, loopback only) and seed
#   scripts/mysql-live-fixture.sh env [mysql|mariadb]   print the environment the live tests read
#   scripts/mysql-live-fixture.sh down   remove containers and generated certificates
#
# Container names and ports are run-owned. Override AIP_FIXTURE_TAG/AIP_FIXTURE_MYSQL_PORT/
# AIP_FIXTURE_MARIADB_PORT when another run is using the same host. Nothing here touches the
# default 3306 endpoint, and the generated credentials only protect throwaway containers.
set -euo pipefail

tag="${AIP_FIXTURE_TAG:-$(id -un)}"
mysql_port="${AIP_FIXTURE_MYSQL_PORT:-34391}"
mariadb_port="${AIP_FIXTURE_MARIADB_PORT:-34392}"
state_dir="${AIP_FIXTURE_DIR:-/private/tmp/aip-mysql-fixture-$tag}"
mysql_name="aip-fixture-mysql84-$tag"
mariadb_name="aip-fixture-mariadb114-$tag"
password="aip_fixture_pw_$tag"
database="aip_test_admin"

gen_certs() {
  mkdir -p "$state_dir/certs"
  cd "$state_dir/certs"
  [ -f ca.pem ] && return 0
  openssl req -x509 -newkey rsa:2048 -nodes -keyout ca-key.pem -out ca.pem -days 2 -subj "/CN=aip-fixture-ca" 2>/dev/null
  openssl req -x509 -newkey rsa:2048 -nodes -keyout untrusted-key.pem -out untrusted-ca.pem -days 2 -subj "/CN=aip-fixture-untrusted-ca" 2>/dev/null
  openssl req -newkey rsa:2048 -nodes -keyout server-key.pem -out server.csr -subj "/CN=127.0.0.1" 2>/dev/null
  # SAN is the IP only on purpose: connecting through the name "localhost" is the hostname-mismatch case.
  printf 'subjectAltName=IP:127.0.0.1\n' > san.ext
  openssl x509 -req -in server.csr -CA ca.pem -CAkey ca-key.pem -CAcreateserial -out server-cert.pem -days 2 -extfile san.ext 2>/dev/null
  # The server runs as an unprivileged user inside the container; the key lives only in this disposable dir.
  chmod 644 server-key.pem server-cert.pem ca.pem
}

wait_ready() {
  local name="$1" ping="$2"
  for _ in $(seq 1 60); do
    if docker exec "$name" "$ping" ping -uroot -p"$password" 2>/dev/null | grep -q alive; then return 0; fi
    sleep 2
  done
  echo "ERROR: $name did not become ready" >&2
  return 1
}

seed() {
  local name="$1" client="$2"
  docker exec -i "$name" "$client" -uroot -p"$password" <<'SQL'
DROP DATABASE IF EXISTS aip_mysql_test;
CREATE DATABASE aip_mysql_test;
CREATE TABLE aip_mysql_test.users (
  user_id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
  display_name VARCHAR(80) NOT NULL,
  email VARCHAR(120) NOT NULL,
  UNIQUE KEY users_email_uq (email)
) ENGINE=InnoDB;
CREATE TABLE aip_mysql_test.posts (
  post_id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY,
  user_id BIGINT NOT NULL,
  title VARCHAR(200) NOT NULL,
  KEY posts_user_idx (user_id),
  CONSTRAINT posts_user_fk FOREIGN KEY (user_id) REFERENCES users (user_id) ON DELETE CASCADE ON UPDATE RESTRICT
) ENGINE=InnoDB;
SQL
}

case "${1:-}" in
  up)
    gen_certs
    docker rm -f "$mysql_name" "$mariadb_name" >/dev/null 2>&1 || true
    docker run -d --name "$mysql_name" -e MYSQL_ROOT_PASSWORD="$password" -e MYSQL_DATABASE="$database" \
      -p "127.0.0.1:$mysql_port:3306" --tmpfs /var/lib/mysql -v "$state_dir/certs:/certs:ro" mysql:8.4 \
      --ssl-ca=/certs/ca.pem --ssl-cert=/certs/server-cert.pem --ssl-key=/certs/server-key.pem >/dev/null
    docker run -d --name "$mariadb_name" -e MARIADB_ROOT_PASSWORD="$password" -e MARIADB_DATABASE="$database" \
      -p "127.0.0.1:$mariadb_port:3306" --tmpfs /var/lib/mysql -v "$state_dir/certs:/certs:ro" mariadb:11.4 \
      --ssl-ca=/certs/ca.pem --ssl-cert=/certs/server-cert.pem --ssl-key=/certs/server-key.pem >/dev/null
    wait_ready "$mysql_name" mysqladmin
    wait_ready "$mariadb_name" mariadb-admin
    seed "$mysql_name" mysql
    seed "$mariadb_name" mariadb
    echo "fixture ready: $mysql_name :$mysql_port, $mariadb_name :$mariadb_port" >&2
    ;;
  env)
    # The driver's live unit tests read the AIP_TEST_MYSQL_* names and pick the dialect from the URL scheme, so the
    # engine argument decides which server those names point at. mysql_fence_verification also reads MARIADB_URL.
    certs="$state_dir/certs"
    engine="${2:-mysql}"
    case "$engine" in
      mysql) scheme=mysql; port="$mysql_port" ;;
      mariadb) scheme=mariadb; port="$mariadb_port" ;;
      *) echo "usage: $0 env [mysql|mariadb]" >&2; exit 2 ;;
    esac
    cat <<EOF
export AIP_TEST_MYSQL_URL='$scheme://root:$password@127.0.0.1:$port/$database'
export AIP_TEST_MARIADB_URL='mariadb://root:$password@127.0.0.1:$mariadb_port/$database'
export AIP_TEST_MYSQL_TLS_URL='$scheme://root:$password@127.0.0.1:$port/$database?sslmode=verify-full'
export AIP_TEST_MYSQL_TLS_MISMATCH_URL='$scheme://root:$password@localhost:$port/$database?sslmode=verify-full'
export AIP_TEST_MYSQL_TLS_CA='$certs/ca.pem'
export AIP_TEST_MYSQL_TLS_UNTRUSTED_CA='$certs/untrusted-ca.pem'
EOF
    ;;
  down)
    docker rm -f "$mysql_name" "$mariadb_name" >/dev/null 2>&1 || true
    rm -rf "$state_dir"
    ;;
  *)
    echo "usage: $0 up|env|down" >&2
    exit 2
    ;;
esac
