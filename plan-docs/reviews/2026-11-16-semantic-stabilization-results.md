# AIP 의미론 안정화 결과 — 수정·회귀 검증 및 잔여 위험

- 작성 기준일: 2026-11-16 (KST)
- 상태: main bd54d919dad3a1a83b79220b207d9e37cc0d5510의 Verify43820299403와 release43820299455는 completed/success다. release 원문은 full-regression·release-build·package·checksums·binary·artifact-smoke·baseline-build·upgrade-drill을 포함한 13단계 PASS와 ALL STEPS PASSED를 기록한다. 교정된 upgrade-drill은 기존 job 저장 보존·다른 배포 결과 비노출·새 job 결과 조회·binary-only rollback의 안전 거부·백업 복원 뒤 이전 job 결과 조회를 실제 PG17에서 검증했다. 아래 미검증 범위는 그대로 남고, 공식 1.0 출시나 모든 포팅 앱의 동등성을 뜻하지 않는다.
- 근거: A 데이터 감사(audit/semantic-a-data-20261116의 commit c1059eaa8d25e4ac6fc05e5880c7f06db7be536a에서 plan-docs/reviews/2026-11-16-semantic-audit-a-data.md를 git show로 읽음), C 정책 감사, B/D 트랜잭션·확장 감사, 원본 .aip-harness/state/aggregate-empty-nullability-20261116.json 및 task handoff.

## 결론

제품 aip service의 서비스 HTTP+PostgreSQL 회귀 3건(identity·데이터·정책)이 CI43810795517에서 통과했다. 정책 엔진 S-01/S-17 3건과 소유 SQL transaction 재시도·인접 회귀 18건은 별도의 유효 실행이다. Core aggregate-nullability 수정은 별도 compiler/IR/PG 경로로 main에 들어갔다. 서비스 데이터 회귀는 null/미선택·partial patch·Decimal·관계 선택·동률 정렬·빈 count와 삽입 순서 불변식을, 정책 회귀는 기본 거부·조직 조건·필드 은닉·hidden/missing 및 visible/forbidden·거부 후 DB 불변·다음 요청의 권한 변경을 다룬다. 이 결과는 해당 assertion의 증거이며 서비스 전체, generated SDK 전체, 앱 parity 또는 무교착의 보증이 아니다.

이번 범위의 구현 수정·회귀 보강·main 통합은 실행 근거를 확보했다. main bd54d91의 full-regression 말미는 `AIP core verification passed.`이며, package smoke는 설치된 Node SDK와 Python SDK를 decimal/safe 두 wire에서 사용해 read·write·replay·revoke·operation·job 결과를 확인했다. 이 smoke는 이번 서비스 정책 matrix의 generated SDK 직접 검증이나 세 원본 앱의 차등 시험을 대신하지 않는다.

가장 위험한 미검증 경계는 서비스 /read 집계가 row visibility를 자동 상속하지 않는 점과 hidden row 변경이 aggregate·to-many·filter/sort/page 결과를 바꾸지 않는지에 대한 실제 서비스 비간섭 검증이다. tenant 불변식과 운영 FaceReview 정의/인가/facts 일치도 확인하지 않았다.

## 기준 revision과 실행 경로

| 구분 | 기준 및 상태 | 해석 |
|---|---|---|
| 검증한 제품 revision | bd54d919dad3a1a83b79220b207d9e37cc0d5510; 부모가 일반 push 후 원격 SHA 일치 확인 | Core 6917e476 → 서비스 데이터 0518f31 → retry 341208c → 정책 회귀 68df3be → import 포맷 교정 8337e38 → upgrade job 결과 검증기 교정 bd54d91. 마지막 변경은 검증 도구·단위 테스트·기존 동작 문서이며 runtime/인가 구현은 바꾸지 않았다. 341208c 제품 patch-id와 검증 C6 소스 일치, 68df3be 테스트 blob과 원본 2ecb937 일치를 부모가 확인했다. CI 성공과 소스 통합을 구별한다. |
| release checkout | c4a0f469c6921b645d6f2ac17cec5d5cfec0fa57, dirty | 원래 /Users/winterholic/development/projects/aip/aip checkout이다. 기존 dirty/untracked를 보존했다. 초안에서 편집·reset·stash·빌드·테스트하지 않았다. |
| 현재 감사 브랜치 | audit/semantic-bd-tx-ext-20261116, 시작 HEAD e9011f3 | 문서 전용 커밋 d9096b9, e9011f3를 기존 이력으로 유지한다. 한 커밋으로 합치지 않는다. |
| PR / release | 2026-11-16 복원 시 GitHub API 재조회: open PR [], GitHub releases [] | release branch는 여전히 c4a0f469다. GitHub releases가 비어 있다는 사실은 git tag/설치본 부재를 뜻하지 않는다. |
| 제품 서비스 | aip service serve → aip-service → authored .aip facts → V6 HTTP/transport → V3 write 또는 V2 read → DB/SDK contract | service가 spike-* 엔진 crate를 런타임 의존성으로 직접 쓴다. 이름만으로 PoC라고 하지 않는다. 실행 증거는 해당 HTTP/DB harness 범위다. |
| Core PoC | aip-cli poc-runtime / syntax → sema → IR → plan → PG | aggregate-nullability 수정은 이 경로 전용이다. service facts/엔진은 Core compiler를 호출하지 않으므로 Core green을 service 결함 수정 증거로 세지 않는다. |
| 문서 전용 | docs/design/02-language.md, 04-safety-matrix.md, 05-execution-model.md, 13-field-encryption.md의 Proposed 내용 | 현행 제품 지원 계약으로 올리지 않는다. DSL 기능 확장이나 설계 문서만으로 실행 지원을 주장하지 않는다. |

## 판정 원칙

실행 검증은 해당 assertion의 실행 결과만 뜻한다. 코드 대조는 source/계약 비교이지 실행을 뜻하지 않는다. 미검증은 실패가 아니라 실행 증거가 없는 상태다. 수량을 신뢰도 백분율로 환산하지 않는다.

## 확인한 코드 경로와 감사 원문

다음 링크는 이동하는 main 대신 검토한 commit을 고정한다. Core 작성 언어→Semantic IR 경로와 서비스 .aip→facts 경로는 별개다. 두 경로가 같은 컴파일러를 통과한다고 가정하지 않았다.

| 대상 | 고정 소스·테스트 | 이번 판단에 사용한 내용 |
|---|---|---|
| 제품 정의 로딩 | [aip-service load](https://github.com/winterholic/Application-Intent-Protocol/blob/8337e384edbb56c08b409ca3c4acbb4690c2ad76/crates/aip-service/src/lib.rs#L566) | load_checked로 서비스 정의를 검사한다. Core PoC compiler 검증으로 대체하지 않는다. |
| 선택적 PoC 명령 | [CLI poc_runtime_disabled](https://github.com/winterholic/Application-Intent-Protocol/blob/8337e384edbb56c08b409ca3c4acbb4690c2ad76/crates/aip-cli/src/main.rs#L431) | feature로 분리된 PoC와 서비스 명령 경계를 확인한다. |
| Core NULL 검사 | [Checker expr_may_be_null](https://github.com/winterholic/Application-Intent-Protocol/blob/8337e384edbb56c08b409ca3c4acbb4690c2ad76/crates/aip-sema/src/check.rs#L254) · [IR shape](https://github.com/winterholic/Application-Intent-Protocol/blob/8337e384edbb56c08b409ca3c4acbb4690c2ad76/crates/aip-sema/src/to_core.rs#L2237) | 연산별 NULL 의미와 binding/required 검사, 출력 shape에 같은 nullable 정보를 전달한다. |
| SQL 소유 transaction | [bounded retry](https://github.com/winterholic/Application-Intent-Protocol/blob/8337e384edbb56c08b409ca3c4acbb4690c2ad76/spikes/spike-v3-write/src/lib.rs#L98) · [bundle](https://github.com/winterholic/Application-Intent-Protocol/blob/8337e384edbb56c08b409ca3c4acbb4690c2ad76/spikes/spike-v3-write/src/lib.rs#L1542) | apply/bundle/compose 소유 경로의 안전한 재시도와 caller-owned bundle_in 경계를 구분한다. |
| HTTP apply | [V6 handle_apply](https://github.com/winterholic/Application-Intent-Protocol/blob/8337e384edbb56c08b409ca3c4acbb4690c2ad76/spikes/spike-v6-transport/src/lib.rs#L246) | SQL-only 소유 transaction의 retry를 확인한다. extension callback 전체 재실행은 추가하지 않았다. |
| 서비스 데이터·정책 회귀 | [service_flow 데이터](https://github.com/winterholic/Application-Intent-Protocol/blob/8337e384edbb56c08b409ca3c4acbb4690c2ad76/crates/aip-cli/tests/service_flow.rs#L299) · [정책](https://github.com/winterholic/Application-Intent-Protocol/blob/8337e384edbb56c08b409ca3c4acbb4690c2ad76/crates/aip-cli/tests/service_flow.rs#L387) | 실제 JWT/HTTP/PG 상태 단언이다. generated SDK 직접 실행은 아니다. |
| 집계 정책의 남은 경계 | [aggregate advisory](https://github.com/winterholic/Application-Intent-Protocol/blob/8337e384edbb56c08b409ca3c4acbb4690c2ad76/spikes/spike-v1-fixture/src/codes.rs#L638) | source rows read를 자동 상속하지 않는다는 현 계약을 확인했다. 공개 집계를 의도할 수도 있으므로 무조건 정보 유출 결함으로 단정하지 않는다. |
| 상세 감사 A | [데이터 감사 c1059eaa](https://github.com/winterholic/Application-Intent-Protocol/blob/c1059eaa8d25e4ac6fc05e5880c7f06db7be536a/plan-docs/reviews/2026-11-16-semantic-audit-a-data.md) | 당시 재현·환경·정적 분석 기록이다. 이후 수정·CI 결과는 본 보고서를 우선한다. |
| 상세 감사 C | [정책 감사 f2cad274](https://github.com/winterholic/Application-Intent-Protocol/blob/f2cad274a5871781ff66b0bc23db4b35636736a1/plan-docs/reviews/2026-11-16-semantic-audit-c-policy.md) | 정책 SQL/서비스/PoC 구분 및 당시 검증 공백을 기록한다. 후속 service_flow 성공은 아래 표에 있다. |
| 상세 감사 B/D | [트랜잭션·확장 감사 e9011f3](https://github.com/winterholic/Application-Intent-Protocol/blob/e9011f31199a552260baefb1c7cd4e768668831b/plan-docs/reviews/2026-11-16-semantic-audit-bd-tx-ext.md) | §8의 세 앱 경계와 호환 분석을 포함한다. 초기 baseline 수치의 revision 공백과 후속 결과는 본 보고서에 구분한다. |

## A — 데이터 의미 9개 항목

| # | 항목 | 판정 | 근거·경계 |
|---:|---|---|---|
| A1 | Missing / Null / 값 존재 | 실행 검증, 범위 한정 | service_data_semantics_hold_across_http_and_database_state가 응답 필드 부재, JSON null, patch 생략값 보존, 명시적 null 저장을 확인했다. |
| A2 | 조건부 효과와 평가 시점 | 코드 대조; 제품 실DB 미검증 | 서비스 선언·전이 경로를 대조했다. 새 service matrix는 조건 평가/경합 시점을 검사하지 않는다. conditional_update_effect는 spike 단독 test다. |
| A3 | 정렬 기준·동률 | 실행 검증, 범위 한정 | 동률 score의 id 순서와 반대 삽입 순서의 결과 불변을 service matrix에서 확인했다. 전체 cursor/page 경계는 아니다. |
| A4 | latest / first / last | 과거 실패와 후속 engine 성공 보존; service 의미 미검증 | baseline 실패는 보존하되 CI43797649277/head b91f80d와 CI43806410294/head 22683321의 latest_effect_values 및 latest_nullable_projection 원문은 각각 1 passed/0 failed다. 후자는 nullable 후보와 absence fallback을 함께 검사한다. CI43809336539에서도 latest_effect_values 1건 통과. baseline의 macOS 차이는 미확정이고 service matrix는 latest를 호출하지 않았다. Core AST에는 Latest/First만 있고 Last는 없다. |
| A5 | 관계 참조·대상 선택 | 실행 검증, 범위 한정 | service matrix에서 선택된 member relation id/name을 확인했다. S-01 origin field to-one 숨김은 별도 PG spike test 1건이다. to-many 및 SDK 통합은 미검증이다. |
| A6 | 집계·빈 결과 | service count 실행 검증; Core 결함 수정 green | service matrix는 빈 count=0 및 존재 시 1을 확인했다. Core empty min/max/avg checker/shape 결함은 수정 후 suite green이다. service min/max/avg 및 SDK nullable shape는 미검증. C의 aggregate source rowRead 비상속 경계도 남는다. |
| A7 | 부분 수정·필드 삭제 | 부분 실행 검증 | title-only patch 뒤 나머지 값 보존 및 explicit null 저장을 확인했다. 삭제 전용 표기와 모든 null 조합은 아니다. |
| A8 | Decimal / Money 정밀도 | Decimal 실행; Money 미검증 | Decimal(10,2) 최대값 99999999.99를 HTTP 문자열/DB text로 대조했다. Money와 전 SDK 실DB 왕복은 없다. |
| A9 | 입력·출력 타입 일치 | 부분 실행 및 코드 대조 | HTTP 응답/DB 상태와 기존 SDK 생성 검사 일부를 대조했다. DSL부터 generated TS/Python SDK까지 end-to-end 적합성은 없다. |

### Core aggregate NULL 결함 및 수정

- 재현 조건: Core DSL에서 비어 있을 수 있는 후보의 min을 required 필드에 대입해도 진단되지 않았고 grouped query output shape가 빈 집합 NULL을 non-null로 표시했다. Boolean SQL 의미에서는 nullable range/set 및 null-propagating builtin과 IS NULL, IS DISTINCT, Boolean predicate 처리를 구분해야 하는 반례도 있었다.
- 5분류: Implementation 결함(Core checker/shape가 SQL nullable 의미를 누락). Product service 결함은 아니다.
- 근본 원인: aip-sema가 aggregate/SQL expression NULL 전파를 required assignment 검사와 grouped shape에 연결하지 못했다. 처음 넓게 적용한 Boolean null 전파는 기존 정상식을 과잉 거부해 RED로 다시 좁혔다.
- 영향: Core compiler/IR/PG PoC의 필드 호환성·출력 shape. aip service는 별도 facts와 엔진 경로이므로 동일 서비스 결함으로 계산하지 않는다.
- 수정 SHA: 6917e476e2037f649a6620ce422cbf0a9ebfb5cb, fix(sema): align nullability with SQL predicates. 이전 7cfc9c2, 15810ad는 불충분 교정 이력이다.
- 명령: CARGO_TARGET_DIR=/tmp/aip-audit-a/target cargo test -q -p aip-sema --test aggregate_boolean_nullability_review --test aggregate_nullability_review --test aggregate_empty_nullability --test conformance --test core_ir --locked --no-fail-fast — exit 0, aggregate 각 5/5 세 suite, conformance 3/3, core_ir 29/29.
- 이어서 CARGO_TARGET_DIR=/tmp/aip-audit-a/target cargo test -q -p aip-syntax -p aip-sema -p aip-ir -p aip-plan -p aip-pg -p aip-contract --locked --no-fail-fast — exit 0, six-crate suite 통과. Core 검사이며 service DB 실행이 아니다. 원문 .aip-harness/state/aggregate-empty-nullability-20261116.json 및 [수정 commit](https://github.com/winterholic/Application-Intent-Protocol/commit/6917e476e2037f649a6620ce422cbf0a9ebfb5cb).

## B — transaction·동시성 7개 항목

| # | 항목 | 판정 | 근거·경계 |
|---:|---|---|---|
| B1 | 복합 변경 원자성 | 실행 검증, 범위 한정 | CI43809336539 bundle_http 1건 통과. 모든 service wrapper 변형을 포괄하지 않는다. |
| B2 | 동시 수정 충돌 | 실행 검증, 경로 구분 | 같은 CI에서 absence_serializable_http 1건 및 lock-order retry 1건 통과. 명시 row lock, serializable absence retry, business refusal은 다른 계약이다. |
| B3 | lock order/deadlock | 재현 실패 후 bounded retry green; 무교착 아님 | CI43806410294/head 22683321에서 lock test 0 passed/1 failed와 retryable WIRE.CONFLICT가 남았다. PostgreSQL server는 aip_bd_lock_order_13748.task에서 transaction 상호 대기 및 40P01을 기록했다. retry 뒤 CI43809336539에서 test green. schema aip_bd_lock_order_5394에서 40P01 뒤 actor1 INVALID_STATE/retryable=false, actor2 OK, owners [2,2]를 대조했다. 한 실행 회복이며 무교착 보장은 아니다. |
| B4 | optimistic concurrency (expect/version) | 코드·기존 회귀 대조; 신규 DB test 없음 | type validation, locked-row NULL-safe compare, mismatch rollback/WIRE.CONFLICT 및 stale-expect 회귀를 대조했다. retry CI가 전체를 시험한 게 아니다. |
| B5 | 실패 rollback 범위 | 실행 검증, DB 한정 | write_extension_atomic 1건과 bundle_http, commit_unknown_wire 통과. 외부 provider 효과는 rollback되지 않는다. |
| B6 | commit 뒤 응답 유실 | 실행 검증 | commit_unknown_wire 13건 통과. COMMIT_UNKNOWN은 SQL retry와 분리되고 자동 재전송되지 않는다. |
| B7 | idem key 재사용/결과 replay | 동시 race 실행, 범위 한정 | CI43797649277과 CI43806410294의 audit-bd-idem-race.log 모두 twelve_concurrent_requests_with_one_key_execute_once_and_replay_eleven_times 1 passed/0 failed. 서로 다른 key로 counter/booking 시나리오를 분리한 corrected fixture다. 이 12연결 HTTP/DB 실행과 commit_unknown 13건을 구별하며 모든 멱등·만료 조합을 포괄하지 않는다. |

### 제품 owned transaction retry 결함

- 재현 조건: READ COMMITTED atomic bundle 둘이 같은 행을 [30,31] / [31,30] 순으로 잠가 역순 deadlock을 만든다. CI43806410294/head 22683321에서 actor 하나가 retryable conflict였고 server log의 40P01이 확인했다.
- 5분류: Implementation 결함. 초기 failing run은 Verification 증거이고 원인은 제품 owned transaction 경로의 retry 누락이다. 원본 앱 parity 후보와 다르다.
- 근본 원인: V3 apply/bundle/compose 및 V6 owned SQL-only HTTP /apply 재시도는 SERIALIZABLE에만 제한돼 READ COMMITTED의 40P01 rollback 뒤 자동 재실행이 없었다.
- 수정: retry_owned_transaction을 owned SQL transaction에 적용해 rollback 후 정책과 현재 상태를 다시 평가한다. caller-owned bundle_in/compose_in, 외부 callback/provider, durable callback은 무조건 재실행하지 않는다. 상한·business refusal·COMMIT_UNKNOWN 경계는 유지된다.
- 수정 SHA: 검증 소스 85371aa, CI head 6a4019e51443c73f74916380c418e07e5652eb4b. main 통합은 341208c848c4344f72d2cd2e3c6b3dbd7416efbd다. 제품 두 파일의 stable patch-id와 최종 lock-order 테스트 blob 일치를 부모가 확인했다. 이후 8337e38은 테스트 import 순서 3줄만 바꿨다.
- 검증: CI43809336539 PostgreSQL 17 — lock 1 + latest 1 + bundle HTTP 1 + serializable absence 1 + extension atomic 1 + commit-unknown 13 = 18 passed, 0 failed/ignored/filtered. 이는 한 충돌에서의 회복이며 무교착 보장은 아니다.

## C — policy 9개 항목

S-01/S-17 성공은 spike-v2-read 엔진과 PostgreSQL fixture 범위다.

| # | 항목 | 판정 | 근거·미검증 경계 |
|---:|---|---|---|
| C1 | 정책 선언 누락 기본 거부 | service HTTP/PG 실행 | Locked의 rows read 누락 → read 빈 배열 및 apply MISSING_TARGET, DB 값 불변. CI43810795517의 정책 테스트 1건에 포함된다. |
| C2 | read/write 평가 차이 | service HTTP/PG 실행, 범위 한정 | 같은 JWT로 Record read의 조직 행 필터와 visible peer apply의 FORBIDDEN, hidden apply의 MISSING_TARGET 및 거부 후 DB 불변을 검사했다. 모든 transition/from 조합은 아니다. |
| C3 | 직접 fieldRead redaction | service 실행; generated SDK 미검증 | Record.secret은 owner에게 원문, 같은 조직 peer에게 JSON null로 은닉됨을 검사했다. field filter/sort/aggregate 모든 비간섭 조합은 미검증이다. |
| C4 | relation origin field policy | engine 실행, service 미검증 | CI43796184726/head 57655d1fdf63ce830ad8b356a37aef345c7a10f0, security_authorization 2 passed. PG spike to-one test이며 service HTTP/SDK는 아니다. |
| C5 | to-many/aggregate/filter/sort/page 비간섭 | 미검증; 위험 경계 | traverseMany 생성은 코드 대조. hidden-only row 불변 시험 없음. aggregate source rowRead 자동 상속 없음, advisory 있음. |
| C6 | tenant 격리 | 명시 조직 조건 실행; 자동 tenant 보증 없음 | .aip에 tenant 선언이 없다. 사용자 정의 org 조건의 read 필터/hidden apply 거부를 검사했다. 자동 tenant invariant나 관계/집계 전체 격리를 주장하지 않는다. |
| C7 | 실행 중 권한/소유권 변경 | 다음 요청 실행; 요청 중간 경합 미검증 | 같은 JWT를 유지한 채 DB actor.org를 바꾸면 다음 read의 보이는 행이 바뀜을 검사했다. 실행 중간 revoke/ownership race는 미검증이다. |
| C8 | service/worker와 end-user actor | 코드 대조; 등가성 미검증 | actor mode·service principal/resource actor 경로 존재. user/anonymous/managed/worker 권한·audit actor matrix는 없다. |
| C9 | hidden/missing vs forbidden 및 nullable 부정식 | engine·service 실행; generated SDK 미검증 | S-17 active/suspended/anonymous [1,0,0] 1 passed. service에서는 hidden=없는 target의 MISSING_TARGET과 visible+denied의 FORBIDDEN 및 DB 불변을 확인했다. |

정책 이력: 초기 local command는 strict DSN guard에서 중단됐고 그 Codex nested sandbox의 initdb는 shmget ... Operation not permitted. CI43796184726에서 CLI baseline 51, S-01 2, S-17 1 passed. 후속 CI43810795517/head f2cad274a5871781ff66b0bc23db4b35636736a1은 completed/success이며 service-flow.log는 identity/data/policy 각 1건, 합계 3 passed/0 failed/0 ignored/0 filtered out다. 정책의 11개 단언을 11개 테스트로 세지 않는다. 테스트 파일은 main 68df3be와 원본 2ecb937의 blob 88f58c9906eecb5fb803ae8acee654510b76ace4가 같다.

## D — extension/외부 효과 7개 항목

| # | 항목 | 판정 | 근거·경계 |
|---:|---|---|---|
| D1 | extension input/output contract | revision 고정 CI 일부 + baseline 기록 | CI43809336539에서 write_extension_atomic 1 passed. write_extensions_http 2와 write_extension_commit_classification 12는 과거 baseline에 통과 기록이 있으나 원 revision/전체 invocation 미상이다. |
| D2 | extension data access 범위 | 제한된 CI 및 코드 대조 | 선언 access 경로와 atomic test를 대조했다. 전체 driver capability 및 production definition parity는 아니다. HTTP baseline 기록의 revision 공백은 아래 표에 남긴다. |
| D3 | DB transaction/callback 경계 | atomic CI 실행; 외부 효과 분리 | CI43809336539의 write_extension_atomic과 commit_unknown_wire를 근거로 DB 경계를 확인했다. rollback은 SMTP/Mongo/Redis/provider 효과를 되돌리지 않는다. owned SQL retry도 callback 전체를 재실행하지 않는다. |
| D4 | durable job retry/중복 실행 | main8337e38의 제한된 CI 실행 | Linux worker CI43815434485의 명시 job_retry 1건·job_breaker 2건이 모두 통과했다. 과거 revision 미상 baseline보다 강한 최신 근거지만 다중 worker·lease expiry·provider unknown의 모든 조합은 아니다. |
| D5 | 외부 API unknown | main8337e38 breaker 실행; 실 provider unknown 미검증 | 같은 worker CI의 job_breaker 2건이 통과했다. External brokers CI43815434511에서는 Kafka/RabbitMQ live 2건과 broker worker recovery 1건을 실행했다. 실제 비멱등 provider unknown의 모든 경우나 revision 미상 outbound_webhook baseline까지 같은 검증으로 올리지 않는다. |
| D6 | outbox/webhook 전달 | main8337e38 outbox 실행, 중복 허용 | worker CI43815434485의 outbox 2건이 통과했다. local mock webhook 1건은 여전히 revision 미상 baseline 기록이다. exactly-once consumer delivery나 현 main 전체 검증을 뜻하지 않는다. |
| D7 | 보상·복구 | 계약별 범위, 조합 미검증 | Mongo/Redis write extension saga log, WIRE.COMPENSATION_FAILED 및 수동 복구 경로. 일반 durable job saga compensation과 모든 provider lookup은 보장으로 세지 않는다. |

### 업그레이드 job 결과 검증기 결함

- 재현: PG17 release gate43815434522는 baseline4c27040에서 main8337e38로 migration/provision한 뒤 이전 SUCCEEDED job의 `/jobs/status`를 조회했지만 `/jobs/result`는 WIRE.DEPLOYMENT_CHANGED였다. `product/tools/upgrade-drill.mjs`의 무조건 output.length=3 단언이 실패했다.
- 분류·원인: Verification Gap/검증기 구현 결함. [durable result guard](https://github.com/winterholic/Application-Intent-Protocol/blob/8337e384edbb56c08b409ca3c4acbb4690c2ad76/spikes/spike-v6-transport/src/durable.rs#L684)는 수락 당시 deployment/contract digest가 현재 정의와 다르면 결과 노출을 거부한다. 이 guard는 최초 durable 구현 cf28f0f부터 존재한다. 영속 보존을 다른 배포에서의 접근 허용과 같게 취급한 gate가 경계를 빠뜨렸다. 최초 실패 artifact에는 어느 지문이 달랐는지는 없어 추정하지 않는다.
- 영향·수정: 검증 도구가 배포 전후 DB의 job id/owner/operation/state/input/output/deployment/contract/attempt를 대조하고, 현재 DB marker·생성 계약과 일치하면 원래 결과를 요구하며 불일치하면 정확한 거부 코드와 output 부재를 요구한다. 새 배포 job의 결과 조회와 백업 복원 뒤 이전 job 결과 조회도 추가했다. runtime·인가 guard·마이그레이션은 수정하지 않았다. 소비처 검색에서 oldJobReadable은 이 도구 한 곳뿐이어서 보고 형식을 보존/접근/거부별 필드로 바꿨다.
- 회귀: 기존 단언을 함수로 추출한 뒤 실제 거부 응답에서 RED(2 failed/5 passed)를 확인했다. 변경 후 `node --test product/tests/upgrade-drill.test.mjs product/tests/release-gate-postgres-toolchain.test.mjs`는 14 passed/0 failed/skipped다. changed deployment, changed contract, 잘못된 오류, 거부 응답의 output 노출, DB output/state/binding 변조, unchanged binding의 결과를 구분한다. node --check 및 git diff --check exit0.
- 후속 실행: [수정 bd54d91](https://github.com/winterholic/Application-Intent-Protocol/commit/bd54d919dad3a1a83b79220b207d9e37cc0d5510)은 main에 push·원격 SHA 대조했다. release43820299455의 upgrade-drill은 ok:true/exitCode:0이다. public contract fingerprint는 unchanged:true지만 기존 job result는 WIRE.DEPLOYMENT_CHANGED로 비노출됐고 DB snapshot 보존을 통과했다. 두 지문 중 어느 값이 달랐는지는 상세 digest를 출력하지 않아 raw 값 비교로 확정하지 않으며, 이 응답을 기존 guard가 거부한 범위로 해석한다. binary-only rollback은 WIRE.SCHEMA_MISMATCH로 안전 거부했고 백업 복원은 rows/idempotency/undone write 재실행/oldJobResultRestored 모두 true다. 새 job의 실제 결과 조회 단언도 지나갔다.

## 세 앱의 선언형/확장 경계와 호환 영향

| 앱 | 선언형 | 공식 확장 | 미검증/호환 경계 |
|---|---|---|---|
| FaceReview | fixture read/transition/bundle로 CRUD·관계; TS SDK read/apply/bundle. 운영 .aip 정의 미확인. | JSON DTO read extension, myEmotionSummary write extension, 추천 extension, admin job. Video.watchDetail 뒤 Video.view apply는 별도 요청. worker fixture는 Python. | GET detail 실패 때 view apply를 warning 처리하는 동작의 원본 parity 미검증. EmotionSummary의 checkpoint/new session/concurrency oracle 미실행. generic JSON envelope DSL 표준화 근거 부족. |
| FlaskBB | app.aip policy/transition/aggregate 및 Python SDK; caller ID로 Topic/Post bundle 생성·publish 가능. | activation/password email durable job. 외부 SMTP는 DB와 원자적이지 않음. production UI enqueue 미확인. | G-6 PostOrder/projection/latest parity 열림. graph-create 필요 입증 없음. exactly-once email 보장 없음. |
| Infisical | Org/Project/Environment/Folder/Secret 관계, policy, uniqueness, CRUD, bounded multi-delete, projection, secretReminder. | Python reminder worker는 DB/outbox 기록이며 mail/Slack 전송 안 함. 암호화는 별도 data-plane capability. | G2 encryption/version/rotation 미구현·runtime gate 차단. G14 recursive delete, G16 time/position gap. G11 Environment+root Folder 요청 간 중간 상태 후보. gate를 열지 않는다. |

관계·권한·상태 전이·요청 단위 SQL atomicity는 선언형 조합을 우선한다. 비관계 계산·외부 provider·장기 작업·legacy DTO는 선언된 input/output/access/effect/deadline을 가진 extension/job에서 처리한다. spike-* service dependency는 제품 경로이고 poc-runtime/Core tests는 service 실행 증거가 아니다. 이번 초안은 SDK/wire schema나 앱 선언을 바꾸지 않아 호환 변경이 없다.

## 포팅 사례의 5분류

| 분류 | 대표 사례 |
|---|---|
| Implementation Defect — 재현·수정 | Core 빈 aggregate NULL 검사/IR shape 누락(6917e476), 제품 owned READ COMMITTED 40P01 retry 누락(341208c). 각각 위 재현·원인·영향·검증 항목에 연결한다. Infisical 암호화 미구현 자체를 이번에 재현한 구현 버그로 세지 않는다. |
| Semantic Defect — 포팅 자산의 과거 사례/후보 | FlaskBB G7: nullable hidden을 생략하고 hidden != true를 평가할 때 SQL UNKNOWN과 원본 ORM 기본값이 달라진다. 명시 false 대응은 기존 자산이다. FaceReview GET summary/view 부수 효과의 원본 비교는 후보이며 이번에 동등성을 실행하지 않았다. |
| Expressiveness Gap — 문서·소스 기반 후보 | Infisical G14 recursive folder delete/G16 time arithmetic. 제한 깊이 삭제로 대체하면 원래 업무 의미와 달라진다. 기존 조합과 공식 확장을 먼저 검토하며 DSL 추가는 하지 않았다. |
| Abstraction Defect — 동등성 미검증 후보 | FlaskBB G6 UUID/PostOrder mapping과 Infisical G11 Environment/root Folder ID 전달. 내부 id를 원본 순서나 복합 생성 원자성으로 간주하면 안 된다. 전체 oracle 실행 전에는 확정 제품 결함으로 올리지 않는다. |
| Verification Gap — 직접 확인 | Core 첫 수정의 합성 표현식/Boolean 반례 누락, 교착의 raw SQLSTATE 누락, exact 필터의 0 tests 가능성, baseline revision/명령 누락, service와 generated SDK/원본 oracle의 공백. Core 15개 회귀·SQLSTATE 서버 로그·실행 건수 점검으로 일부 보완하고 나머지는 잔여 위험으로 둔다. |

## 실행 검증 기록 — 과거 실패와 후속 결과

| revision / run | 환경·명령 | 결과 | 원문·범위 |
|---|---|---|---|
| A f1c6696, Core pre-fix | CARGO_TARGET_DIR=/tmp/aip-audit-a/target cargo test -p aip-sema --test aggregate_empty_nullability --no-fail-fast -- --nocapture | failed, exit 101, 2 failed | A audit §4; required min/grouped min RED, Core PoC. |
| Core 6917e476 | CARGO_TARGET_DIR=/tmp/aip-audit-a/target cargo test -q -p aip-sema --test aggregate_boolean_nullability_review --test aggregate_nullability_review --test aggregate_empty_nullability --test conformance --test core_ir --locked --no-fail-fast | passed, exit 0; aggregate 5/5 each, conformance 3/3, core_ir 29/29 | parent-confirmed state JSON; Core only. |
| C initial local | cargo test -p aip-cli --no-fail-fast; local initdb | DB test skipped at DSN guard; initdb failed shmget ... Operation not permitted | C audit §7–8; nested sandbox attempt only. |
| CI 43796184726, head 57655d1fdf63ce830ad8b356a37aef345c7a10f0 | Ubuntu 24.04 + PG17; cargo test -p aip-cli --locked --no-fail-fast; cargo test --manifest-path spikes/spike-v2-read/Cargo.toml --locked --test security_authorization --no-fail-fast; same command with --test negated_policy; readiness psql SELECT 1 ×3 | passed 51 + 2 + 1; readiness passed 3. mysql job skipped by audit-ref condition. | [Actions 43796184726](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43796184726), C audit §8. Not full service policy matrix. |
| B/D historical baseline | baseline log가 원 revision/전체 invocation을 보존하지 않음; /tmp/aip-baseline-engines.log에 기록된 named test outputs | 환경·전체 명령은 감사 원문에 없음. extension tests: write_extension_atomic 1, write_extensions_http 2, write_extension_commit_classification 12, job_retry 1, job_breaker 2, outbound_webhook 1, outbox 2가 passed로 기록됨. 여기서는 현재 CI 결과로 승격하지 않는다. | [B/D 감사 §5](https://github.com/winterholic/Application-Intent-Protocol/blob/e9011f31199a552260baefb1c7cd4e768668831b/plan-docs/reviews/2026-11-16-semantic-audit-bd-tx-ext.md#5-실행-흔적과-미검증-범위); 원본 log /tmp/aip-baseline-engines.log. 전체 명령/revision은 보존되지 않아 미확인. |
| CI 43797649277, head b91f80dc4bca339468ff06eb315328e3657538ab | PG17; V3 latest_effect_values/latest_nullable_projection 및 V6 audit_bd_idem_race 각 named test | latest 2 suite 각 1 passed, corrected 12연결 idem race 1 passed. 같은 run의 lock-order는 1 failed로 run 전체 실패를 보존한다. | /tmp/aip-orch-proof-668d5d6c/bd-43797649277의 세 원문 로그를 부모가 대조했다. |
| CI 43806410294, head 22683321 | PG17; cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --locked --target-dir target --test audit_bd_lock_order -- --exact --nocapture; latest 두 named suite와 V6 audit_bd_idem_race | lock-order 0 passed/1 failed, retryable WIRE.CONFLICT 및 PostgreSQL 40P01. latest_effect_values/latest_nullable_projection와 idem race는 각각 1 passed/0 failed다. 서비스 matrix는 traverse fixture 누락으로 데이터 단언에 미도달했다. | [Actions 43806410294](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43806410294), /tmp/aip-orch-proof-668d5d6c/semantic-43806410294의 각 원문. 초안의 latest 실패 표기는 교정했다. |
| CI 43808490417, test commit 0518f31, workflow head abc41c0 | GitHub-hosted Ubuntu + PG17; cargo test -p aip-cli --test service_flow --locked service_data_semantics_hold_across_http_and_database_state -- --exact --nocapture; psql -X "$AIP_TEST_DATABASE_URL" -Atc 'SELECT 1' readiness | passed 1, failed 0, filtered 1; readiness printed 1 | [Actions 43808490417](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43808490417), /tmp/aip-orch-proof-668d5d6c/service-43808490417/service-data-semantics.log. HTTP+DB subset, generated SDK not executed. |
| CI 43809336539, head 6a4019e51443c73f74916380c418e07e5652eb4b, fix 85371aa | GitHub-hosted PG17; commands: cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --locked --target-dir target --test audit_bd_lock_order -- --exact --nocapture; cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --locked --target-dir target --test latest_effect_values -- --nocapture; cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --test bundle_http -- --nocapture; cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --test absence_serializable_http -- --nocapture; cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --test write_extension_atomic -- --nocapture; cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --test commit_unknown_wire -- --nocapture | passed 18; failed/ignored/filtered 0; each readiness passed; 40P01 actor and owner outcome recorded | [Actions 43809336539](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43809336539), /tmp/aip-orch-proof-668d5d6c/retry-43809336539. Recovery for this conflict, not deadlock-free. |
| CI 43810795517, head f2cad274a5871781ff66b0bc23db4b35636736a1 | Ubuntu + PG17.12; cargo test -p aip-cli --test service_flow --locked -- --nocapture | completed/success; identity/data/policy 3 passed/0 failed/0 ignored/0 filtered out; readiness 1 | [Actions 43810795517](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43810795517), /tmp/aip-orch-proof-668d5d6c/policy-43810795517/service-flow.log. generated SDK 호출은 아님. |
| Retry/policy main integration | git ls-remote; git patch-id --stable; git rev-parse commit:path | retry 341208c 제품 diff 일치; 정책 68df3be 테스트 blob 일치; main 반영 확인 | 부모의 영속 계약 revision29 및 .aip-harness/state/aip-bounded-owned-retry-main-20261116.json, aip-policy-service-main-20261116.json. |
| main 68df3be 전체 CI 8개 | GitHub API run/job 조회 및 job logs, release artifact 원문 다운로드 | DB contracts, Linux worker, cgroup, external brokers, object storage, app packages 6개 success. Verify43811829023와 release43811829006 failure: 둘 다 audit_bd_lock_order.rs import 순서 3줄로 Rust formatting exit1. 이후 runtime 전체·패키지/설치/업그레이드/rollback은 실행되지 않음. | [Verify](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43811829023), [release gate](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43811829006). 현재 Run outbox ci-evidence의 job-133115075550.log 및 release-43811829006/logs/full-regression.log. |
| main 8337e384edbb56c08b409ca3c4acbb4690c2ad76 | cargo fmt --all --check; git diff --check; 단일 파일 3+/3- 확인; 일반 push 및 ls-remote | 로컬 fmt/diff exit0; 원격 SHA 일치. 후속 Verify43815434509는 success. release43815434522는 전체회귀·build/package/smoke PASS 뒤 upgrade job 결과 검증기에서 실패했으며 아래 행에 별도 기록했다. | [포맷 수정](https://github.com/winterholic/Application-Intent-Protocol/commit/8337e384edbb56c08b409ca3c4acbb4690c2ad76). 기존 실패를 취소하거나 green으로 바꾸지 않았다. |
| main8337e38 worker CI43815434485 | V6 cargo test --locked --target-dir target --no-fail-fast --test job_retry --test job_breaker --test triggers --test outbox --test otlp --test encryption_gate --test encryption_e2e --test bundle_http | worker-jobs job133126299447 success. 해당 8개 suite는 각각 1/2/3/2/9/3/1/1 passed, 모두 failed/ignored/filtered 0. 같은 run의 crypto-compile-repair job133126301306은 skipped이며 통과로 세지 않는다. | [Linux worker jobs](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43815434485). 전체 job에는 다른 검증도 있지만 이 행은 명시 suite만 센다. |
| main8337e38 brokers CI43815434511 | cargo test -p aip-execution --features external-brokers --test brokers_live --locked -- --ignored; cargo test -p aip-service --features external-brokers --locked broker_worker_recovers -- --ignored | live brokers 2 passed/0 failed, worker recovery 1 passed/0 failed/47 filtered. 뒤 별도 test binaries의 0 tests는 추가 통과 건수로 세지 않는다. | [External brokers](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43815434511). Kafka/RabbitMQ CI fixture 경계다. |
| main8337e38 기타 완료 CI | cgroup43815434626 / object storage43815434543 / app packages43815434606의 run/job/head와 원문 회수 | 모두 success. cgroup 명시 실행 14+2 passed, 각각59/71 filtered. Object storage 로그에는 unit 2 ignored가 있어 전체0 ignored라고 하지 않는다. app packages Node 검사는73+1+1 passed/0 failed/0 skipped. | [cgroup](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43815434626), [object storage](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43815434543), [app packages](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43815434606). app packages 성공은 전체 release install/upgrade/rollback 성공과 별개다. |

| main8337e38 Verify43815434509 / job133126299705 | gh run view 및 job logs API 원문 직접 회수 | completed/success. compiler/runtime/product/worker/transport/SDK 검증 단계와 HTTP→Node/Python→MySQL 단계 success. focused UI diagnostic build와 failure-log 단계는 skipped로 유지한다. | [Verify](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43815434509); 현재 Run ci-evidence/verify-133126299705.log. |
| main8337e38 release43815434522 / job133126299590 | scripts/release-gate.sh --baseline 4c27040c6d4a2a2931321e1980842be79512bdb0 --ref 8337e384edbb56c08b409ca3c4acbb4690c2ad76; PG17.12/Linux/rustc1.100.0/Node26.13.1/Python3.12.3 | full-regression·release-build·package·artifact-checksums·artifact-binary·artifact-smoke·baseline-build PASS, upgrade-drill FAIL at new-job-result/WIRE.DEPLOYMENT_CHANGED. 두 rollback은 해당 실행에서 미도달. | [release](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43815434522); artifact14203662861의 summary 및 logs/upgrade-drill.log. 전체 실패를 보존한다. |
| mainbd54d91 검증기 교정 | node --test product/tests/upgrade-drill.test.mjs product/tests/release-gate-postgres-toolchain.test.mjs; node --check; git diff --check; 일반 push/ls-remote | 14 passed/0 failed/skipped, 문법·diff exit0, main SHA 일치. 자동 Verify43820299403/release43820299455의 후속 완료 원문은 아래 행에 기록했다. | [수정](https://github.com/winterholic/Application-Intent-Protocol/commit/bd54d919dad3a1a83b79220b207d9e37cc0d5510), .aip-harness/state/aip-upgrade-job-fence-20261116.json. |

| mainbd54d91 최종 Verify43820299403 / job133141602209 | GitHub 완료 API와 job logs 직접 회수 | completed/success. compiler/runtime/product/worker/transport/SDK 및 real HTTP→Node/Python→MySQL 단계 success. focused UI diagnostic build·failure-log 단계는 skipped로 구분한다. | [Verify](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43820299403); 최종 Run ci-evidence/verify-133141602209.log. |
| mainbd54d91 최종 release43820299455 / job133141602723 | scripts/release-gate.sh --baseline 4c27040c6d4a2a2931321e1980842be79512bdb0 --ref bd54d919dad3a1a83b79220b207d9e37cc0d5510; Linux x86_64/PG17.12/rustc1.100.0/Node26.13.1/Python3.12.3 | completed/success; 13 PASS, ALL STEPS PASSED. full-regression 원문 말미 AIP core verification passed. release build·package·checksums·binary·설치 smoke·upgrade 및 두 rollback 경로를 검증했다. suite 내부 skip/ignore/filter까지 모두 0이라는 뜻은 아니다. | [release](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43820299455); 최종 Run ci-evidence/release-artifact/summary.txt 및 logs/*.log. |
| 같은 release의 설치 smoke와 upgrade-drill | artifact-smoke.log / upgrade-drill.log JSON 직접 파싱·assert | smoke ok:true/package packed+installed, decimal/safe에서 Node+Python read/replay/status/revoke/operation/job 확인. upgrade ok:true/exitCode0, storedResultPreserved:true/resultReadable:false/refused:DEPLOYMENT_CHANGED, binary rollback refused:SCHEMA_MISMATCH, restore 4필드 모두 true. | 같은 실행의 immutable source와 실제 결과다. 원본 앱 parity·운영 IdP·전체 권한 비간섭의 증거로 확대하지 않는다. |

## 우선순위와 최종 판단

| 우선순위 | 남은 확인 | 이유 |
|---|---|---|
| P1 | generated SDK에서 service 정책 wire 보존 및 고위험 조합 | CI43810795517의 service HTTP/PG 직접 검증은 확보했다. generated TS/Python SDK 호출과 모든 조합은 아직 미검증이다. |
| P1 | hidden-only row 비간섭: to-many, aggregate, filter/sort/page 및 callerFilter | aggregate source policy 자동 상속 없음. where/sourceAccess와 aggregate access가 선택한 실제 집합 확인 필요. |
| 확인 종료 | mainbd54d91의 교정된 upgrade/rollback gate | Verify43820299403/release43820299455 success와 실제 summary/drill/smoke 원문을 확인했다. 추가 실행을 요구하는 잔여 항목으로 두지 않는다. |
| P2 | FaceReview production definition/facts/actor/access 및 GET detail/view 실패, EmotionSummary repeat/new-session/concurrency parity | 원본 oracle 비교·실행 전. |
| P2 | Infisical G11 중간 실패; FlaskBB G6 order/projection parity | source/DB oracle 재현 전에는 제품 결함으로 승격하지 않는다. |
| P2 | worker/service actor와 role/tenant/ownership race | 적용 시점과 actor semantics가 열려 있다. |
| P3 | generic JSON envelope/graph-create/새 DSL primitive | 두 번째 독립 앱 수요 및 호환 영향 입증 전 보류. |

**현재 신뢰할 수 있는 범위:** Core 6917e476의 NULL 의미 회귀, 서비스 PG17 HTTP+DB identity/data/policy 3건, owned retry와 인접 회귀 18건, engine latest·12연결 멱등 replay의 명시 assertion은 실행 증거가 있고 관련 변경은 main에 반영됐다. 같은 소스의 기존 검증은 재사용하되 서로 다른 revision의 결과를 현 main 전체 성공으로 합산하지 않는다. mainbd54d91 Verify와 release full-regression·release package·실제 SDK 설치 smoke·upgrade/rollback도 통과했다. 이 판정은 검증한 선언·assertion·fixture 경계다. 모든 앱 parity, 모든 transaction 조합 및 이번 정책 회귀의 generated SDK 전체 검증은 남아 있다.

**가장 위험한 미검증 의미 경계:** hidden row가 aggregate·to-many·sort·page를 통해 결과에 영향을 주지 않는다는 service policy 비간섭이다. aggregate source policy가 자동 상속되지 않아 명시 where/sourceAccess와 public access가 실제 /read/SDK에서 의도한 집합을 선택하는지 확인해야 한다. tenant 자동 보증 및 FaceReview 운영 definition parity도 열려 있다.

## 범위·자원 기록

초안 013b45d는 문서 하나만 추가했다. 복원 후 부모가 오류를 교정했고 기존 전용 worktree에서 테스트 import 정렬만 8337e38로 main에 일반 push했다. dirty release c4a0f469와 다른 작업자의 캐시는 유지했다. 새 clone/target/DB/browser/container는 만들지 않았다. 기존 소유 자원은 .aip-harness/state/aip-semantic-lead-review-20261116.json 및 aip-rustfmt-lock-order-20261116.json에 기록했다. 후속 bd54d91용 worktree /private/tmp/aip-upgrade-job-fence-20261116은 사전 여유450560000000 bytes/예산128MiB로 등록한 뒤 사용했다. 커밋의 원격 main 보존과 clean 확인 후 git worktree remove exit0·path 부재를 대조했다. 삭제 전 논리 크기는32608KiB이며 물리 회수량으로 환산하지 않는다. 새 build cache/DB는 만들지 않았다. 상세 영수증은 aip-upgrade-job-fence-20261116.json이다. 최종 Run outbox의 ci-evidence에는 bd54d91 Verify/release API observation·두 job 원문·release artifact 원문과 SHA256 manifest를 보존한다. 앞선 Run의 증거를 보호 경계 밖에서 다시 읽거나 복제하지 않고 기존 검토 결과를 재사용했다. Core·C·B/D worker의 마지막 keychain_mismatch는 해당 worker Run 실패이며 AIP 제품 실패로 세지 않는다.

## 보강한 검증과 남은 방법론 공백

발견 결함의 최소 회귀를 먼저 RED로 재현하고 기존 suite를 재사용했다. Core에서는 required assignment·합성 산술·binding·insert·grouped shape·Boolean SQL 호환성을 15개 회귀로 고정했다. 서비스에서는 입력 삽입 순서 불변, 거부 후 DB 불변, 동일 JWT 다음 요청의 정책 변경을 deterministic assertion으로 넣었다. 병렬 요청의 동일 key 단일 효과/11회 replay와 실제 교착 후 원자 상태는 조합 검증이다. 이를 임의 생성 property suite나 원본 앱 차등 시험으로 부르지 않는다.

FaceReview·FlaskBB·Infisical 원본과 동일 시나리오의 HTTP+DB+인가+부수 효과 차등 실행은 이번에 수행하지 않았다. 정책 비간섭의 생성형 property, 조건부 효과의 경합 시점, Money·field deletion·service actor 등 미검증 항목은 위 matrix와 우선순위에 그대로 남긴다. 새 DSL이나 특수 업무를 추가하지 않고 재현된 공통 결함과 기존 제품 release gate를 우선했다. 세 원본 앱을 동시에 실행해 같은 fixture로 비교하는 차등 시험과 임의 생성 property suite는 수행하지 않았으며 이번 완료 근거에 넣지 않는다. 이 두 방법론의 부재를 환경 실패나 원본과의 동등성으로 포장하지 않는다. 후속은 위 P1 권한 비간섭·SDK 정책 경로부터 시작하고 원본 parity/조건 경합을 P2로 검증한다.

## P0-1 시도 — 2026-11-17, e48ee7a

**완료되지 않음.** 이 cycle에서는 정책 동작 변경이나 회귀 테스트를 작성하지 않았다. 서비스 HTTP/PostgreSQL baseline이 필요하지만 이번 Codex command sandbox에서 전용 PostgreSQL 인스턴스를 띄울 수 없었고, 기존 검증 DB의 인증 정보도 제공되지 않았다. 실패 테스트 없이 제품 코드를 먼저 수정하지 않았다.

**시작 상태와 환경.** cycle `cycle/p0-policy-noninterference-20261117`, HEAD `e48ee7aea4f3ec7036798439ca193b31581346af`; 초기 tracked working tree clean. `.aip-harness/check.py` 시작 출력은 free bytes `450560000000`, 기존 checkout `target` absent, `.aip-harness` 32768 bytes였다. Codex nested command sandbox의 OS는 macOS이며 `/opt/homebrew/opt/postgresql@17/bin`의 PostgreSQL 17.11을 확인했다. Docker API는 `docker ps`에서 `dial unix .../docker.sock: no such file or directory`로 사용할 수 없었다. shell의 PGHOST/PGPORT만으로 시도한 `aip_test_ci` 접속은 port 58480에서 `fe_sendauth: no password supplied`로 실패했다. 이 접속의 DB 상태는 미확인이다.

**전용 PostgreSQL 시작 실패.** 이번 작업 소유 경로 `.aip-harness/tmp/p0-policy-20261117/pgdata`에서 `initdb`가 bootstrap 중 `FATAL: could not create shared memory segment: Operation not permitted`, `DETAIL: Failed system call was shmget(...)`로 중단됐고 data directory를 자동 제거했다. 첫 실행 뒤 SysV 설정을 `mmap`으로 지정한 두 변형에서도 같은 syscall이 거절됐다. 이 결과는 해당 nested command sandbox에서 실행된 initdb에 한정된다. 다른 엔진이나 권한 경로를 사용해 재시도하지 않았다. DB 또는 스키마는 생성되지 않았다.

**분리된 경로 판정.** 서비스 경로는 `crates/aip-cli/tests/service_flow.rs`의 `SERVICE_POLICY_APP` 및 `service_policy_default_deny_redaction_and_hidden_target_invariants`이고, 제품 service 실행이 `aip-cli`/service runtime을 통해 HTTP와 PostgreSQL을 함께 사용한다. 필수 명령은 `cargo test -p aip-cli --test service_flow --locked -- --nocapture`; 이번에는 실제 실행하지 않았다. compile-only 명령 `env -u PGHOST -u PGPORT -u PGDATABASE -u PGUSER -u PGPASSWORD -u PGSERVICE -u PGSERVICEFILE AIP_TEST_DATABASE_URL='postgres://winterholic@127.0.0.1:58567/aip_test_p0_policy_20261117' AIP_TEST_ADMIN_URL='postgres://winterholic@127.0.0.1:58567/postgres' CARGO_TARGET_DIR=.aip-harness/tmp/p0-policy-20261117/target CARGO_INCREMENTAL=0 cargo test -p aip-cli --test service_flow --locked --no-run`은 exit 0, test executable 생성으로 끝났다. 이 compile 결과는 HTTP/DB assertions 실행 증거가 아니다.

Core PoC 경로는 독립 `spikes/spike-v1-fixture` sema이며 service 제품 엔진과 구별했다. `CARGO_TARGET_DIR=.aip-harness/tmp/p0-policy-20261117/target CARGO_INCREMENTAL=0 cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --test advisories --locked -- --nocapture`는 `2 passed; 0 failed; 0 ignored`였다. 확인된 회귀는 `callerFilter read` + source rows policy + aggregate where 누락 경고, 그리고 explicit where 및 FaceReview fixture에서 해당 경고가 사라지는 경우다. 이는 Core DSL 검사이며 service 결과의 증거가 아니다.

`cargo fmt --all --check`는 exit 0, `env -u PGHOST ... CARGO_TARGET_DIR=.aip-harness/tmp/p0-policy-20261117/target CARGO_INCREMENTAL=0 cargo clippy -p aip-cli --all-targets --locked -- -D warnings`는 exit 0이었다. lint는 컴파일/정적 검사이며 PostgreSQL 실행을 의미하지 않는다.

**집계 계약과 호환성.** `spikes/spike-v1-fixture/src/parser.rs`, `ast.rs`, `sema.rs:1641-1665,1715-1766`, `syntax_card.rs:113-127`, `codes.rs:630-645` 및 `product/tests/fixtures/facereview-mysql/facereview.aip`를 확인했다. 선언의 `sourceAccess`는 필수이며 source 행 접근을 결정한다. `where`는 집계 원본 행을 별도로 제한한다. `callerFilter none`은 caller filter를 받지 않고 `read`는 요청 filter를 허용하며, `groupKey`는 per-row 결과를 만든다. standalone aggregate는 source `rows read`를 자동 상속하지 않는 기존 계약이다. 따라서 숨김 행 변화가 결과에 영향을 주지 않아야 하는 집계는 aggregate 자체의 `sourceAccess`/`where`가 그 불변식을 명시해야 한다. `callerFilter read` + 정책 source + 빠진 `where`는 현재 advisory이지 rejection이 아니다. 데이터 집합 전체에 의도적으로 공개한 aggregate는 `sourceAccess` 정책이 이를 허용할 때 의도된 예외이며, 모두 차단하면 FaceReview의 guest/user 공개 Video `watchCount`·`commentCount`와 기존 extension aggregate 경로가 호환성 영향을 받는다. 이 영향은 코드 선언으로 확인했으며 실제 HTTP 결과는 미검증이다.

**미검증.** 실제 서비스 baseline, 다른 사용자/조직 숨김, count/min/max/avg, 필드 은닉과 filter/sort side channel, page count/cursor/order, to-one/to-many 조합, row+field 숨김 교차, 정책 변경 다음 요청 재평가, 응답과 DB snapshot 및 hidden-only mutation 불변, 변경 후 제품 회귀·fmt·clippy·RED/GREEN은 모두 미실행이다. DB-backed tests는 skip되거나 성공 처리되지 않았으며 신규 test assertion도 없다. 서비스와 aggregate 경계에 대한 위 판정은 소스 코드 검토이고 실행 결과를 뜻하지 않는다.

**자원.** 별도 Cargo target `.aip-harness/tmp/p0-policy-20261117/target`는 이 task용으로만 만들었고 마지막 측정 크기는 1972240 KiB, 디스크 free는 440000000 KiB였다. 25 GB peak budget 안에 있으며 후속 P0-1 compile cache로 보존한다. postgres data dir는 initdb 실패가 자동 제거해 현재 없다. 서비스 process나 database는 남아 있지 않다. Task record는 `.aip-harness/state/p0-policy-20261117.json`에 기록했다. 다음 단계는 일반 macOS Terminal 등 실제 PostgreSQL을 실행할 수 있는 승인된 환경에서 이 test tree를 이어서 검증하는 것이다.
