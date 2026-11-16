# AIP 의미론 안정화 결과 — 초안

- 작성 기준일: 2026-11-16 (KST)
- 상태: 초안. 정책 CI, retry의 main 통합, 통합 후보의 최종 전체 검사는 대기 중이다.
- 근거: A 데이터 감사(audit/semantic-a-data-20261116의 commit c1059eaa8d25e4ac6fc05e5880c7f06db7be536a에서 plan-docs/reviews/2026-11-16-semantic-audit-a-data.md를 git show로 읽음), C 정책 감사, B/D 트랜잭션·확장 감사, 원본 .aip-harness/state/aggregate-empty-nullability-20261116.json 및 task handoff.

## 결론

현재 제품 aip service에서 확인한 범위는 서비스 HTTP+PostgreSQL 데이터 회귀 1건, 정책 엔진 회귀 S-01/S-17 3건, 소유 SQL transaction 재시도·인접 회귀 18건이다. Core PoC의 aggregate-nullability 수정 및 관련 compiler/IR/PG 회귀는 별도 경로로 main에 들어갔다. 제품 서비스 회귀는 null/미선택·partial patch·Decimal·관계 선택·동률 정렬·빈 count와 삽입 순서 불변식을 다룬다. 이 결과는 해당 assertion의 증거이지 서비스 전체, 모든 SDK 언어, 앱 parity 또는 무교착의 보증이 아니다.

가장 위험한 미검증 경계는 서비스 /read 집계가 row visibility를 자동 상속하지 않는 점과 hidden row 변경이 aggregate·to-many·filter/sort/page 결과를 바꾸지 않는지에 대한 실제 서비스 비간섭 검증이다. tenant 불변식과 운영 FaceReview 정의/인가/facts 일치도 확인하지 않았다.

## 기준 revision과 실행 경로

| 구분 | 기준 및 상태 | 해석 |
|---|---|---|
| live main | handoff 기준 0518f31d5c028795036d7189bafe14ab1da1399a; 초안 push 직전 refs/heads/main 관측 68df3be2019d62827f5ec46c7422fa49e3f70dad | handoff SHA에는 Core 교정 6917e476e2037f649a6620ce422cbf0a9ebfb5cb와 service data test 0518f31이 있다. push 전 git ls-remote에서 main ref가 68df3be로 전진한 것을 확인했으나 해당 commit 내용은 조회하지 않았다. retry 통합 여부는 미확인이다. service data CI는 abc41c0 계열에서 실행됐다. |
| release checkout | c4a0f469c6921b645d6f2ac17cec5d5cfec0fa57, dirty | 원래 /Users/winterholic/development/projects/aip/aip checkout이다. 기존 dirty/untracked를 보존했다. 초안에서 편집·reset·stash·빌드·테스트하지 않았다. |
| 현재 감사 브랜치 | audit/semantic-bd-tx-ext-20261116, 시작 HEAD e9011f3 | 문서 전용 커밋 d9096b9, e9011f3를 기존 이력으로 유지한다. 한 커밋으로 합치지 않는다. |
| PR / release | handoff 14:28 KST 기록: open PR [], GitHub releases [] | 그 시각의 기록이며 이번 초안 작성 중 재조회하지 않았다. |
| 제품 서비스 | aip service serve → aip-service → authored .aip facts → V6 HTTP/transport → V3 write 또는 V2 read → DB/SDK contract | service가 spike-* 엔진 crate를 런타임 의존성으로 직접 쓴다. 이름만으로 PoC라고 하지 않는다. 실행 증거는 해당 HTTP/DB harness 범위다. |
| Core PoC | aip-cli poc-runtime / syntax → sema → IR → plan → PG | aggregate-nullability 수정은 이 경로 전용이다. service facts/엔진은 Core compiler를 호출하지 않으므로 Core green을 service 결함 수정 증거로 세지 않는다. |
| 문서 전용 | docs/design/02-language.md, 04-safety-matrix.md, 05-execution-model.md, 13-field-encryption.md의 Proposed 내용 | 현행 제품 지원 계약으로 올리지 않는다. DSL 기능 확장이나 설계 문서만으로 실행 지원을 주장하지 않는다. |

## 판정 원칙

실행 검증은 해당 assertion의 실행 결과만 뜻한다. 코드 대조는 source/계약 비교이지 실행을 뜻하지 않는다. 미검증은 실패가 아니라 실행 증거가 없는 상태다. 수량을 신뢰도 백분율로 환산하지 않는다.

## A — 데이터 의미 9개 항목

| # | 항목 | 판정 | 근거·경계 |
|---:|---|---|---|
| A1 | Missing / Null / 값 존재 | 실행 검증, 범위 한정 | service_data_semantics_hold_across_http_and_database_state가 응답 필드 부재, JSON null, patch 생략값 보존, 명시적 null 저장을 확인했다. |
| A2 | 조건부 효과와 평가 시점 | 코드 대조; 제품 실DB 미검증 | 서비스 선언·전이 경로를 대조했다. 새 service matrix는 조건 평가/경합 시점을 검사하지 않는다. conditional_update_effect는 spike 단독 test다. |
| A3 | 정렬 기준·동률 | 실행 검증, 범위 한정 | 동률 score의 id 순서와 반대 삽입 순서의 결과 불변을 service matrix에서 확인했다. 전체 cursor/page 경계는 아니다. |
| A4 | latest / first / last | 과거 실패와 후속 engine 성공 보존; service 의미 미검증 | /tmp/aip-baseline-engines.log의 latest_effect_values와 latest_nullable_projection 실패를 보존했다. 실제 left/right는 없어 과거 차이는 미확인이다. CI43809336539에서 latest_effect_values 1건은 후속 통과했으나 latest_nullable_projection 후속 실행은 기록되지 않았다. 선택된 nullable row 분기는 과거 통과, absence fallback은 과거 실패였다. service matrix는 latest를 호출하지 않았다. Core AST에는 Latest/First만 있고 Last는 없다. |
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
| B7 | idem key 재사용/결과 replay | 부분 검증; 동시 race 미검증 | 순차 replay/다른 본문 mismatch 경로가 있지만 corrected 12-connection race는 새로 실행되지 않았다. commit_unknown 13건은 이를 대체하지 않는다. |

### 제품 owned transaction retry 결함

- 재현 조건: READ COMMITTED atomic bundle 둘이 같은 행을 [30,31] / [31,30] 순으로 잠가 역순 deadlock을 만든다. CI43806410294/head 22683321에서 actor 하나가 retryable conflict였고 server log의 40P01이 확인했다.
- 5분류: Implementation 결함. 초기 failing run은 Verification 증거이고 원인은 제품 owned transaction 경로의 retry 누락이다. 원본 앱 parity 후보와 다르다.
- 근본 원인: V3 apply/bundle/compose 및 V6 owned SQL-only HTTP /apply 재시도는 SERIALIZABLE에만 제한돼 READ COMMITTED의 40P01 rollback 뒤 자동 재실행이 없었다.
- 수정: retry_owned_transaction을 owned SQL transaction에 적용해 rollback 후 정책과 현재 상태를 다시 평가한다. caller-owned bundle_in/compose_in, 외부 callback/provider, durable callback은 무조건 재실행하지 않는다. 상한·business refusal·COMMIT_UNKNOWN 경계는 유지된다.
- 수정 SHA: 85371aa (fix(write): retry owned read-committed deadlocks), CI head 6a4019e51443c73f74916380c418e07e5652eb4b. main 통합 pending.
- 검증: CI43809336539 PostgreSQL 17 — lock 1 + latest 1 + bundle HTTP 1 + serializable absence 1 + extension atomic 1 + commit-unknown 13 = 18 passed, 0 failed/ignored/filtered. 이는 한 충돌에서의 회복이며 무교착 보장은 아니다.

## C — policy 9개 항목

S-01/S-17 성공은 spike-v2-read 엔진과 PostgreSQL fixture 범위다.

| # | 항목 | 판정 | 근거·미검증 경계 |
|---:|---|---|---|
| C1 | 정책 선언 누락 기본 거부 | 코드 대조; service 미검증 | rows read 누락은 denyAll facts→SQL FALSE. S-17은 nullable 부정식이지 선언 누락 matrix가 아니다. |
| C2 | read/write 평가 차이 | 코드 대조; 통합 미검증 | read row filtering과 write rowRead/allow/from 분리. 같은 actor/row를 /read·/apply로 대조한 service test 없음. |
| C3 | 직접 fieldRead redaction | 코드 대조; SDK 미검증 | field redaction 및 policy field filter/sort/aggregate 제한. PoC field_policy_e2e는 실행하지 않았다. |
| C4 | relation origin field policy | engine 실행, service 미검증 | CI43796184726/head 57655d1fdf63ce830ad8b356a37aef345c7a10f0, security_authorization 2 passed. PG spike to-one test이며 service HTTP/SDK는 아니다. |
| C5 | to-many/aggregate/filter/sort/page 비간섭 | 미검증; 위험 경계 | traverseMany 생성은 코드 대조. hidden-only row 불변 시험 없음. aggregate source rowRead 자동 상속 없음, advisory 있음. |
| C6 | tenant 격리 | 제품 계약 제한; 시험 없음 | .aip에 tenant 선언이 없다. 조직 조건은 rows read/allow/관계 predicate로 표현한다. 자동 tenant invariant를 주장하지 않는다. |
| C7 | 실행 중 권한/소유권 변경 | 코드 대조; 경합 미검증 | write policy 참조 잠금 의도와 read statement 평가를 봤으나 실제 경합은 실행하지 않았다. |
| C8 | service/worker와 end-user actor | 코드 대조; 등가성 미검증 | actor mode·service principal/resource actor 경로 존재. user/anonymous/managed/worker 권한·audit actor matrix는 없다. |
| C9 | hidden/missing vs forbidden 및 nullable 부정식 | 일부 engine 실행; service 미검증 | S-17 active/suspended/anonymous [1,0,0] 1 passed. service /apply의 MISSING_TARGET/FORBIDDEN 및 SDK wire 대조 없음. |

정책 이력: 초기 local command는 strict DSN guard에서 중단됐고 그 Codex nested sandbox의 initdb는 shmget ... Operation not permitted. 후속 CI43796184726에서 CLI baseline 51, S-01 2, S-17 1 passed. 이것은 현재 pending CI43810795517을 대신하지 않는다.

## D — extension/외부 효과 7개 항목

| # | 항목 | 판정 | 근거·경계 |
|---:|---|---|---|
| D1 | extension input/output contract | 실행 검증 | write_extension_atomic 1, write_extensions_http 2, write_extension_commit_classification 12 baseline 통과. 모든 provider/driver 조합은 아니다. |
| D2 | extension data access 범위 | 실행 검증, 제한 | 선언 access 경로를 atomic/HTTP tests에서 확인. 전체 driver capability 및 production definition parity는 아니다. |
| D3 | DB transaction/callback 경계 | 실행 검증, 외부 효과 분리 | extension/transition/audit/idempotency DB 경계 test 통과. rollback은 SMTP/Mongo/Redis/provider 효과를 되돌리지 않는다. owned SQL retry도 callback 전체를 재실행하지 않는다. |
| D4 | durable job retry/중복 실행 | 일부 실행 | job_retry 1, job_breaker 2 통과. 다중 worker·lease expiry·provider unknown 조합 미검증. |
| D5 | 외부 API unknown | mock 실행, 실 provider 미실행 | UNKNOWN 자동 재전송 방지 계약. job_breaker 2 및 local outbound_webhook mock 1 통과. |
| D6 | outbox/webhook 전달 | 실행 검증, 중복 허용 | outbox 2, local mock webhook 1 통과. exactly-once consumer delivery가 아니다. |
| D7 | 보상·복구 | 계약별 범위, 조합 미검증 | Mongo/Redis write extension saga log, WIRE.COMPENSATION_FAILED 및 수동 복구 경로. 일반 durable job saga compensation과 모든 provider lookup은 보장으로 세지 않는다. |

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
| Implementation | Infisical G2 encryption/G13 job, FlaskBB G11 related create intermediate state |
| Semantic | FlaskBB G7 nullable hidden의 SQL 3값 논리; FaceReview legacy GET write side effect/parity 후보 |
| Expressiveness | Infisical G14 recursive folder delete/G16 time arithmetic; FlaskBB G3 empty relation condition |
| Abstraction | FlaskBB G6 UUID/PostOrder mapping; Infisical G11 generated Environment ID 전달 |
| Verification | 정책·lock 과거 failure와 후속 green, source/compile/engine/service/SDK/oracle 검증 구분 |

## 실행 검증 기록 — 과거 실패와 후속 결과

| revision / run | 환경·명령 | 결과 | 원문·범위 |
|---|---|---|---|
| A f1c6696, Core pre-fix | CARGO_TARGET_DIR=/tmp/aip-audit-a/target cargo test -p aip-sema --test aggregate_empty_nullability --no-fail-fast -- --nocapture | failed, exit 101, 2 failed | A audit §4; required min/grouped min RED, Core PoC. |
| Core 6917e476 | CARGO_TARGET_DIR=/tmp/aip-audit-a/target cargo test -q -p aip-sema --test aggregate_boolean_nullability_review --test aggregate_nullability_review --test aggregate_empty_nullability --test conformance --test core_ir --locked --no-fail-fast | passed, exit 0; aggregate 5/5 each, conformance 3/3, core_ir 29/29 | parent-confirmed state JSON; Core only. |
| C initial local | cargo test -p aip-cli --no-fail-fast; local initdb | DB test skipped at DSN guard; initdb failed shmget ... Operation not permitted | C audit §7–8; nested sandbox attempt only. |
| CI 43796184726, head 57655d1fdf63ce830ad8b356a37aef345c7a10f0 | Ubuntu 24.04 + PG17; cargo test -p aip-cli --locked --no-fail-fast; cargo test --manifest-path spikes/spike-v2-read/Cargo.toml --locked --test security_authorization --no-fail-fast; same command with --test negated_policy; readiness psql SELECT 1 ×3 | passed 51 + 2 + 1; readiness passed 3. mysql job skipped by audit-ref condition. | [Actions 43796184726](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43796184726), C audit §8. Not full service policy matrix. |
| B/D historical baseline | baseline log가 원 revision/전체 invocation을 보존하지 않음; /tmp/aip-baseline-engines.log에 기록된 named test outputs | 환경·전체 명령은 감사 원문에 없음. extension tests: write_extension_atomic 1, write_extensions_http 2, write_extension_commit_classification 12, job_retry 1, job_breaker 2, outbound_webhook 1, outbox 2가 passed로 기록됨. 여기서는 현재 CI 결과로 승격하지 않는다. | [B/D 감사 §5](2026-11-16-semantic-audit-bd-tx-ext.md#5-실행-흔적과-미검증-범위); 원본 log /tmp/aip-baseline-engines.log. 전체 명령/revision은 보존되지 않아 미확인. |
| CI 43806410294, head 22683321 | PG17; cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --locked --target-dir target --test audit_bd_lock_order -- --exact --nocapture; same args with --test latest_effect_values -- --nocapture; same args with --test latest_nullable_projection -- --nocapture | audit_bd_lock_order failed 0 passed/1 failed with retryable WIRE.CONFLICT; PostgreSQL server recorded 40P01. latest assertions failed in this run. | [Actions 43806410294](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43806410294), /tmp/aip-orch-proof-668d5d6c/semantic-43806410294. Latest actual left/right absent from log. |
| CI 43808490417, test commit 0518f31, workflow head abc41c0 | GitHub-hosted Ubuntu + PG17; cargo test -p aip-cli --test service_flow --locked service_data_semantics_hold_across_http_and_database_state -- --exact --nocapture; psql -X "$AIP_TEST_DATABASE_URL" -Atc 'SELECT 1' readiness | passed 1, failed 0, filtered 1; readiness printed 1 | [Actions 43808490417](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43808490417), /tmp/aip-orch-proof-668d5d6c/service-43808490417/service-data-semantics.log. HTTP+DB subset, generated SDK not executed. |
| CI 43809336539, head 6a4019e51443c73f74916380c418e07e5652eb4b, fix 85371aa | GitHub-hosted PG17; commands: cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --locked --target-dir target --test audit_bd_lock_order -- --exact --nocapture; cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --locked --target-dir target --test latest_effect_values -- --nocapture; cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --test bundle_http -- --nocapture; cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --test absence_serializable_http -- --nocapture; cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --test write_extension_atomic -- --nocapture; cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --target-dir target --test commit_unknown_wire -- --nocapture | passed 18; failed/ignored/filtered 0; each readiness passed; 40P01 actor and owner outcome recorded | [Actions 43809336539](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43809336539), /tmp/aip-orch-proof-668d5d6c/retry-43809336539. Recovery for this conflict, not deadlock-free. |
| CI 43810795517, head f2cad274 | Fresh service_flow all 3 tests; PostgreSQL CI | pending; not checked | [Actions 43810795517](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43810795517). Do not infer success. |
| Retry main integration | Core worker-owned task | handoff said pending; current main ref advanced to 68df3be before push, but commit content was not inspected, so integration remains unverified | Handoff owner 76e831a6 / Run 11475faf. No worker status or CI was queried or awaited. |
| Final integration whole checks | final candidate revision | pending / not run | No final full check was performed. |

## 우선순위와 최종 판단

| 우선순위 | 남은 확인 | 이유 |
|---|---|---|
| P1 | CI43810795517과 service /read·/apply·SDK default deny, hidden/missing vs forbidden, fieldRead/wire 보존 | 결과 pending; 실제 assertions/범위를 대조해야 한다. |
| P1 | hidden-only row 비간섭: to-many, aggregate, filter/sort/page 및 callerFilter | aggregate source policy 자동 상속 없음. where/sourceAccess와 aggregate access가 선택한 실제 집합 확인 필요. |
| P1 | retry commit main SHA 및 integration candidate 전체 검사 | branch green과 main integration/final check는 다르다. |
| P2 | FaceReview production definition/facts/actor/access 및 GET detail/view 실패, EmotionSummary repeat/new-session/concurrency parity | 원본 oracle 비교·실행 전. |
| P2 | Infisical G11 중간 실패; FlaskBB G6 order/projection parity | source/DB oracle 재현 전에는 제품 결함으로 승격하지 않는다. |
| P2 | worker/service actor와 role/tenant/ownership race | 적용 시점과 actor semantics가 열려 있다. |
| P3 | generic JSON envelope/graph-create/새 DSL primitive | 두 번째 독립 앱 수요 및 호환 영향 입증 전 보류. |

**현재 신뢰할 수 있는 범위:** Core compiler nullability 회귀는 6917e476 및 관련 tests가 통과했고 main에 반영됐다. 제품은 PG17 HTTP+DB data matrix 1건, retry regression 18건, policy spike engine 3건의 기존 CI 결과를 각 harness 범위 안에서 신뢰할 수 있다. 이는 서로 다른 revision의 제한된 성공이다. 모든 앱 parity, 모든 transaction 조합, 모든 generated SDK wire 결과가 검증된 것은 아니다.

**가장 위험한 미검증 의미 경계:** hidden row가 aggregate·to-many·sort·page를 통해 결과에 영향을 주지 않는다는 service policy 비간섭이다. aggregate source policy가 자동 상속되지 않아 명시 where/sourceAccess와 public access가 실제 /read/SDK에서 의도한 집합을 선택하는지 확인해야 한다. tenant 자동 보증 및 FaceReview 운영 definition parity도 열려 있다.

## 범위·자원 기록

이 초안은 감사 branch에 Markdown 하나만 추가한다. 제품 코드, DSL, tests, workflow, main, dirty release checkout은 수정하지 않았다. CI 재실행이나 worker 결과 조회·대기는 하지 않았고 clone, target, DB, browser, container도 새로 만들지 않았다. 기존 /tmp/aip-audit-a/target(4.5G)은 부모 등록 cache라 보존했다. 사용자용 HTML 및 share_file은 후속 확정 단계 대상이다.
