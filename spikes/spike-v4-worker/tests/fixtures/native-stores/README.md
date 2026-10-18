# Slice 13-A native driver fixture

This fixture binds only loopback high ports: MySQL 8.4 at `15440`, MongoDB 8.0 at `15441`, Redis 7.4 at `15442`. It does not use the shared service ports. Credentials are local fixture values and must not be reused.

```sh
docker compose -f spikes/spike-v4-worker/tests/fixtures/native-stores/compose.yaml up -d --wait
AIP_TEST_DATABASE_URL='postgres://fixture:fixture@127.0.0.1:15432/aip_test_slice13a?sslmode=disable' \
AIP_TEST_ADMIN_URL='postgres://fixture:fixture@127.0.0.1:15432/postgres?sslmode=disable' \
AIP_TEST_MONGO_URL='mongodb://aip_test_slice13a:local_fixture_only_13a@127.0.0.1:15441/aip_test_slice13a?authSource=admin' \
AIP_TEST_REDIS_URL='redis://:local_fixture_only_13a@127.0.0.1:15442/' \
cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --test native_capabilities
docker compose -f spikes/spike-v4-worker/tests/fixtures/native-stores/compose.yaml down -v
```

The PostgreSQL values satisfy the repository's test build guard only; this suite does not connect to PostgreSQL. The tests write and delete only their dedicated Mongo database and Redis keys. The MySQL container is available for separate MySQL adapter tests but is not exercised by `native_capabilities`.
