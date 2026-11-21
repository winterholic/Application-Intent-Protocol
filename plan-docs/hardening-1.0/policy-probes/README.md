# 정책 경화 프로브

`policy-probes.mjs`는 [policy.md](../policy.md)의 반례를 실제 `aip service init/serve`와 PostgreSQL 17로 재현한다. `h.mjs`는 임시 스키마·RSA 키·JWKS·서비스 설정을 만들고 서버를 띄우는 도우미이며 시나리오가 끝나면 스키마를 지운다.

- `results-before.txt`: 수정 전(`origin/main` 0c5d108d의 `sema.rs`) 바이너리의 출력. `D*`가 `RECOVERED`로 가려진 값을 복원한다.
- `results-after.txt`: 수정 뒤 바이너리의 출력. 같은 정의가 `aip service init`에서 `REJECTED`로 막힌다. `C*`·`K*` 줄은 수정 전과 같다.

환경: `AIP_BIN`(aip 바이너리), `AIP_TEST_DATABASE_URL`(libpq key=value 형식), `AIP_PROBE_PG_CONTAINER`(선택. 주면 그 PG17 컨테이너 안에서 `psql`을 실행하고, 안 주면 호스트의 `psql`이 `AIP_TEST_DATABASE_URL`로 붙는다. 컨테이너는 사용자 `runner`, 데이터베이스 `aip_test_ci`), 내장 `fetch`가 있는 Node(이 작업은 v26.9.0으로 실행). 기본값은 이 문서를 쓴 작업의 임시 컨테이너 설정이라 다른 곳에서는 바꿔야 한다.

`C7`·`C8`(읽기 확장의 `ctx.data`)은 두 번째 작업에서 더했고 `results-after.txt`에만 있다. 수정 전 바이너리로는 다시 돌리지 않았다. 읽기 확장의 ctx 경로(`spike-v4-worker`의 `serve_call`)는 앞의 12건 수정이 건드리지 않은 코드다.
