# Infisical 포팅 실DB 런북 (WBS 7.3.3.9용, 검증 세션 실행)

아래는 격리 PostgreSQL(루프백 전용, fixture DB)이 준비된 검증 세션이
순서대로 실행할 명령이다. 본 세션에서는 실행하지 않았다(환경 제약).

## 공통 준비

```sh
cd ~/development/projects/aip/aip
export AIP_TEST_DATABASE_URL='postgresql://fixture:fixture_pw@127.0.0.1:55439/aip_infisical'
cargo build -p aip-cli
```

## 7.3.3.3/7.3.3.4 선언 검증 (I03·I04)

```sh
./target/debug/aip service check porting/infisical/app.aip
./target/debug/aip service gen porting/infisical/app.aip --out /tmp/infisical-binding.ts
```

기대: `ok:true`. `unique project, folder, env, key`의 NULL folder 동등 처리와
`Secret.rotate`의 `version = version + 1`·`delete`의 값 nulling을 SDK apply로 확인할 것.

## 7.3.3.5 job 선언 검증 (I06)

```sh
./target/debug/aip service check porting/infisical/app.aip
```

기대: 16개 operation job 포함 `ok:true`. 다음은 serve 기동 후:

```sh
cat > /tmp/infisical-service.json <<'EOF'
{"schema": "aip_infisical_probe", "database_url_env": "AIP_TEST_DATABASE_URL",
 "migrate_database_url_env": "AIP_TEST_DATABASE_URL", "listen": "127.0.0.1:0",
 "wire": "decimal", "auth": {"issuer": "https://idp.example.test",
 "audience": "infisical-spike", "jwks": {"kind": "file", "path": "/tmp/probe-jwks.json"}}}
EOF
./target/debug/aip service init --config /tmp/infisical-service.json porting/infisical/app.aip
./target/debug/aip service serve --config /tmp/infisical-service.json porting/infisical/app.aip
```

job 확인: `./target/debug/aip service jobs --config /tmp/infisical-service.json list`

## 7.3.3.7 확장 spike 재실행 (I02)

```sh
/private/tmp/flaskbb-aip-host-venv/bin/python -m unittest discover -s porting/infisical/tests -v
```

기대: 7 passed. 소켓·외부 호출 없음.

## 7.3.3.9 clean 재현 게이트

`init → migrate → seed(Org/Project/Folder/Secret 최소 1식) → serve → SDK read 1건 →
job enqueue 1건(secretReminder)` 순서로 수행하고 각 단계의 원시 출력을
`porting/infisical/evidence/`에 보관한다. 원본 Infisical 백엔드는 기동하지 않는다.
