# UUID·외부 PostgreSQL 구현 검증 (2026-10-14)

UUID와 기존 DB 채택을 구현했다. 권장 숫자 Id/owned schema와 기존 prototype marker 채택은 유지했다. 구현은 `/tmp/aip-wt-uuid` detached worktree에서 작은 커밋으로 나누고, 다른 작업자의 일반 수정 입력·Python 생성 기능과 rebase로 통합했다. 운영 DB·원본 Memoir·.env·저장된 비밀은 읽거나 변경하지 않았다.

## 실제 PostgreSQL·SDK

`node --test product/tests/uuid-external.test.mjs`: **owned/external × safe/decimal 4구성**을 새 DB에서 실행한다. 설치 SDK와 생성 binding으로 UUID 생성·read·Ref·정렬·after 커서·전이·제목/본문 patch·nullable expect/CAS 충돌·멱등 replay/mismatch·캐시 무효화·다른 사용자 거부를 확인한다. Python SDK와 생성 UUID NewType/Row도 확인한다.

외부 schema는 AIP와 무관하게 UUID users/notes 테이블을 만든다. introspect의 deny-all 초안을 검토해 정의로 사용하고, 원본 SELECT만 가진 채택 계정으로 view/metadata를 만든다. DML만 가진 runtime 계정으로 serve한다. 원본 default·FK·UNIQUE·CHECK와 UUID notify/audit를 사용하며 업무 DDL은 없다. UUID actor의 자기 참조는 actor 행 읽기 정책이 닫혀 있어도 기존 숫자 경로와 동일하게 허용하고, 다른 숨은 대상은 거부한다.

원본 default drift 뒤 실행 중 요청·ready·재기동을 거부한다. 채택 plan 재시도는 alreadyApplied이고, 같은 매핑의 정책 재배포는 metadata만 갱신해 principal/idem을 보존한다. 새 DB와 시험 역할은 finally에서 삭제한다.

```text
node --test product/tests/uuid-external.test.mjs
owned/external × safe/decimal: passed
pass 1, fail 0
```

`cargo test -p aip-migrate --test uuid_migration --locked`: 숫자 앱에 UUID resource 추가→outbox text 업그레이드→UUID Ref backfill→catalog preflight, 1 passed. `spike-v3-write --test uuid_ids`: native uuid default·명시적 클라이언트 UUID·대문자 거부·전이·기존 numeric 감사 이력의 text 업그레이드 보존을 실제 PG로 확인한다.

SDK 음성 대조는 숫자 resource의 성공응답에 UUID를 주면 pending을 해제하지 않음을 변경 전 실패→변경 후 통과로 확인한다. UUID가 선언된 resource는 같은 값을 정상 왕복한다.

## 회귀

로컬에서 실행한 결과:

| 검증 | 결과 |
|---|---|
| workspace fmt/clippy, auth/migrate/service all-targets clippy `-D warnings` | 통과 |
| `cargo test --workspace --locked -- --skip operation_only_service_preserves_principal_contract_revocation_and_deployment_fences` | 통과. worker를 시작하는 1개 경로 제외 |
| `cargo test -p aip-cli --features poc-runtime --locked` + 같은 worker 제외 | 통과 |
| 독립 V1/V2/V3/V5 `cargo test --manifest-path ... --locked --target-dir target` | 통과 |
| V6 library | 59 passed |
| SDK·패키지·일반 update·UUID integration·UUID 음성 대조 | 127 passed, 0 failed |
| Python SDK·Google provider unittest | 24 tests, OK |
| V4 library 중 환경에서 실행 가능한 경로 | 34 passed, 1 ignored, 격리/프로세스 관리 8개 제외 |
| V6 integration 62개 target 전수 시도 | 35 target 통과. 24 target은 sandbox 오류 직접 확인. 3 target은 격리 worker 시작/종료 실패로 실동작 미확인 |

`bash tools/verify.sh` 전체는 로컬 worker 시작 테스트에서 중단됐다. 미실행: 이 Ttakkari 실행 환경에서 `sandbox-exec -p '(version 1)(allow default)' /usr/bin/true`조차 `sandbox_apply: Operation not permitted`(exit 71)로 거부된다. 프로세스 조회/종료 권한을 요구하는 정리 경로도 로컬에서 보증하지 않는다. V6의 `extension_unix`, `read_extensions`, `write_isolation`은 로그가 일반 worker 실패로 축약되므로 환경과의 연관은 source/probe에 따른 추론이며 해당 테스트를 통과로 합산하지 않았다.

SDK 패키징의 npm cache 쓰기는 환경 제한에 걸렸고, 프로젝트 코드를 바꾸지 않고 `/tmp` cache를 사용하는 npm wrapper로 재검증했다. HOME이나 비밀을 바꾸지 않았다. 기존 SDK 회귀 126개와 신규 실 DB 통합 1개가 포함된 최종 127개는 통과했다.

신규 실 DB 통합 테스트는 `tools/verify.sh`에 추가했다. 첫 CI 실행에서 UUID 테스트 fixture만 기본 Unix socket을 선택해 접속에 실패했다. `product/tests/uuid-external.test.mjs`의 SQL helper를 `host=localhost` 접속으로 맞춘 뒤 macOS와 Linux CI에서 재검증했다.

## UUID fixture 접속 수정 및 최종 CI

수정 커밋 `7724ef785194f13ed56017faf5e1aaafaccbb58e`는 Verify run [38521559207](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/38521559207)에서 **success**했다. `core` 전체 job이 12분 41초 만에 통과했다. fixture의 생성·초기화·정리는 모두 기존 테스트 전용 DB 연결을 사용하며 테스트를 생략하거나 약화하지 않았다.

macOS 실 PostgreSQL 17에서 기본 Unix socket 환경 변수를 의도적으로 무효화해 실행했다. helper가 명시적 localhost 접속을 사용함을 검증했고, 테스트는 owned/external × safe/decimal 네 경로를 실행했다.

```text
✔ UUID owned and external PostgreSQL: introspect, adopt, restricted role, installed TS/Python SDK, drift refusal (4049.235875ms)
ℹ pass 1
ℹ fail 0
```

## 산출물과 제한

- [설계·명령·권한·제약](DESIGN.md)
- [외부 인프라 연결 목록·우선순위](INFRASTRUCTURE-CONNECTIONS.md)
- 결정: docs/DECISIONS D-A31·D-A32.

PostgreSQL15+, 단일 숫자/UUID PK가 현재 범위다. 자동 introspect의 JSONB/enum/numeric/domain·복합키·cross-schema FK는 명시적으로 거부한다. 매핑/필드/operation 구조 변경과 PK 종류 변환의 자동 이관은 제공하지 않는다. 외부 DBA의 독립 DDL·다른 schema 함수/GRANT 변경·외부 부작용까지 자동 관리하지 않는다. DB 밖 공급자는 이번에 새로 구현하지 않았고 연결 표에 제안으로 남겼다.
