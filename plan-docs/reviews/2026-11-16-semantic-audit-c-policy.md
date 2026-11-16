# AIP 권한·정책 의미 감사 (C)

- 기준: `origin/main` at `f1c6696` (2026-11-16), branch `audit/semantic-c-policy-20261116`
- 범위: 제품 정의의 정책 작성 → 사실 변환 → `aip service` 실행 → HTTP/SDK 오류·결과
- 결론: GitHub Actions PostgreSQL 17에서 실행한 `aip-cli` baseline, S-01, S-17은 모두 통과했고 이 회차에서 **새로 확인된 결함은 0건**이다. S-01은 참조 원본 field policy의 관계 traversal 은닉을, S-17은 부정 actor 정책의 suspended·anonymous 거부를 실제 DB에서 확인한다. 이 결과만으로 `aip service` HTTP/SDK의 전체 권한 matrix가 검증된 것은 아니다.
- 결함 수정은 하지 않았다. 재현 조건·RED 출력을 확보하지 않은 상태에서 테스트를 결함 증거로 추가하지 않았다.
- 로컬 후속 시도는 Codex command sandbox의 System V 공유 메모리 `Operation not permitted`로 막혔으나, 이후 기존 GitHub Actions의 작업자 격리 PostgreSQL 17 fixture에서 같은 회귀를 실행했다. 실행 내역과 남은 matrix는 §6–§8을 참조한다.

## 1. 실제 제품 경로와 실험 경로

`aip service`는 `crates/aip-cli/src/main.rs`에서 서비스 명령을 `aip_service::run`으로 넘기고, `crates/aip-service/Cargo.toml`이 다음을 직접 의존한다.

| 역할 | 실제 의존 모듈 | 확인한 경로 |
|---|---|---|
| `.aip` 읽기·의미 분석·facts 생성 | `spike-v1-fixture`, `spike-v11-dev-checks` | `crates/aip-service/src/lib.rs:566-581`의 `load`/지원 런타임 검사 |
| `/read` 계획·정책 SQL | `spike-v2-read` (`plan`, `sqlgen`) | `crates/aip-service/src/lib.rs:10`; `spike-v2-read/src/plan.rs:486-520` |
| `/apply` 정책 평가·쓰기 | `spike-v6-transport` → `spike-v3-write` | `spike-v6-transport/Cargo.toml`의 직접 의존, HTTP server 요청 전달 경로; 전이 판단은 `spike-v3-write/src/lib.rs:361-419` |
| 인증·actor 연결 및 SDK 계약 | `aip-auth`, `spike-v5-sdk`, `spike-v6-transport` | `crates/aip-service/src/lib.rs:1675-1709` |
| 공통 정책 SQL·DB 실행 도구 | `aip-execution`, `aip-migrate` | 위 서비스 및 하위 crate manifest 의존 |

`spike-*`라는 디렉터리명만으로 제품에서 제외하지 않았다. 위 모듈은 `aip service`의 런타임 Cargo 의존성이다. 반대로 `aip-cli::compile` → `aip-sema` → `aip-ir` → `aip-pg` → `aip-plan`은 `.aipoc`/PoC 런타임 경로다. `crates/aip-cli/tests/field_policy_e2e.rs`도 `aip_runtime::Engine`을 사용하므로 제품 `aip service` 권한 검증으로 세지 않았다. 제품이 직접 의존하는 crate와 PoC 전용 검사 결과를 섞지 않는다.

저장소 루트/상위에서 `CLAUDE.md`, `AGENTS.md`를 찾지 못했다. `plan-docs/STATUS.md`는 2026-11-01 V3 fixture의 `rows read` 누락 → `WIRE.MISSING_TARGET` 사례를 기록한다. 이는 아래 기본 거부 계약의 기대 동작과 일치하며, 문서 기록 자체는 제품 시험 증거로 계산하지 않았다.

## 2. 정책 평가 항목

상태 용어: **정적 일치**는 현재 코드 표현이 계약과 맞는다는 뜻, **실행 확인**은 인용한 CI test binary가 통과한 경우, **테스트 공백**은 해당 항목을 겨냥한 제품 `aip service` 경로 시험이 없는 경우다. 2026-11-16 CI에서 S-01/S-17 spike 엔진 회귀와 CLI baseline은 실행했지만, 서비스 HTTP/SDK 권한 matrix 전체를 실행한 것은 아니다.

| 항목 | 판정 | 코드 경로·기존 회귀 | 남은 확인 |
|---|---|---|---|
| 기본 거부 | 정적 일치, 직접 제품 matrix 공백 | `spike-v1-fixture/src/sema.rs:1087-1097`: `rows read` 누락은 facts에서 `{"default":"denyAll"}`. `spike-v2-read/src/sqlgen.rs:563-566`: `denyAll`을 SQL `FALSE`로 내린다. S-17 CI는 `not actor.suspended`에서 suspended·anonymous를 거부한다. | S-17은 NULL fail-closed를 검증했지만 선언 누락의 기본값을 `aip service` `/read`와 `/apply`에서 대조하지 않았다. 기본 deny live matrix는 미검증. |
| 읽기·쓰기 평가 차이 | 정적 의미 구분, live 차이 시험 공백 | 읽기는 `rowRead`로 결과 행을 제한한다. 쓰기 `judge_query`는 `rowRead`, transition `allow`, `from`을 각각 검사한다(`spike-v3-write/src/lib.rs:374-415`). 읽을 수 있고 전이 권한이 없으면 `WIRE.FORBIDDEN`; 보이지 않거나 없는 id는 `WIRE.MISSING_TARGET`(`:461-480`). | 이번 CI는 같은 facts/actor/행에서 read·apply·create 및 SDK 오류를 대조하지 않았다. |
| 필드 수준 접근 제어 | 일부 실행 확인, 제품 HTTP/SDK 공백 | 작성 언어의 `field <name> read when`은 facts `fieldRead`로 변환(`spike-v1-fixture/src/parser.rs:378-383`, `sema.rs:1099-1110`). S-01 CI의 `single_reference_traverse_respects_origin_field_policy`는 실제 PostgreSQL에서 다른 사용자와 anonymous의 참조 relation 값을 null로 숨겼다. 직접 select는 CASE redaction(`spike-v2-read/src/plan.rs:550-566`); 정책 필드는 filter/sort/aggregate 사용이 제한된다. | S-01은 `spike-v2-read` 엔진 test로 통과했다. 직접 field projection의 제품 service·generated SDK 전달과 우회 반례는 미검증. PoC `field_policy_e2e`는 이번에 실행하지 않았다. |
| 참조·join·aggregate·sort를 통한 정보 노출 | to-one origin reference 일부 확인, 나머지 공백 | S-01은 참조 출발 field policy가 to-one traversal에 적용됨을 확인했다(`spikes/spike-v2-read/tests/security_authorization.rs`). `traverseMany`는 하위 row policy를 적용하도록 facts를 만든다(`spike-v1-fixture/src/sema.rs:1882,1954`). 독립 aggregate는 source의 `rowRead`를 자동 상속하지 않고 advisory를 낸다. | to-many, hidden-only row의 filter/sort/page 비간섭, aggregate와 제품 SDK 경로는 이번 CI에서 실행하지 않았다. advisory 경계는 명시적 설계 위험으로 남는다. |
| 테넌트 격리 | 개념 의미 제한, 교차 테넌트 격리 테스트 공백 | 현재 `.aip` v1 parser/AST에는 tenant 선언이 없다(`spike-v1-fixture/src/ast.rs`, `parser.rs`; `rg tenant` 결과 없음). 제품 작성자는 조직/소유자 경계를 `rows read`, transition `allow`, 관계 조건에 표현한다. 별도 `crates/aip-ir::tenant`/`crates/aip-pg::tenant`는 PoC Core IR 경로여서 `aip service` tenant 보증 근거가 아니다. | 다중 조직 owner/ref/filter/traverse/aggregate를 통합한 실DB 비간섭 시험 미실행. 구조적 tenant 보장을 한다고 광고할 근거가 없다. |
| 작업 실행 중 권한 변경 | 잠금 의도는 보임, 경합 시험 공백 | write policy context는 참조 정책 읽기를 잠그도록 설정(`spike-v3-write/src/lib.rs:376-400`, `1023-1034`, `1087-1094`). read path는 한 SQL statement 안에서 결과·가시성 정책을 평가한다. | 역할·소유권 변경과 read/apply 경합, 새 요청 경계에서 반영 여부, SERIALIZABLE predicate/부재 판정을 제품 DB에서 실행하지 못함. 트랜잭션·동시성의 전체 소유 검토는 본 작업 범위 밖이다. |
| 서비스 주체와 사용자 주체 | 의미 일부 존재, 동등성/구분 시험 공백 | facts에 actor mode가 있고 관리형 principal 또는 resource actor를 설정한다. 서비스 초기화는 `with_resource_actor`를 통해 actor binding을 연결한다(`crates/aip-service/src/lib.rs:1678-1683`). write/query 정책은 actor id를 `Ctx`에 전달한다. job/service 경로에서 사용자 actor를 보존하는지 별도 확인 필요. | user token, anonymous, managed principal, worker/service 호출별 같은 선언의 allow·audit actor·자료 접근 시험 미실행. 별도 service-role 정책 주체를 보장한다고 판정하지 않는다. |
| 권한 거부와 리소스 부재 구별 | 정적 구현 확인, live 제품 공백 | `spike-v3-write/src/lib.rs:474-480`은 안 보이는 행과 없는 행을 같은 `WIRE.MISSING_TARGET`으로 돌려 ID 존재 여부를 숨긴다. visible row + denied transition은 `WIRE.FORBIDDEN`(:461-463). | 이번 CI는 write/query semantics와 SDK HTTP 오류 전달을 실행하지 않았다. hidden vs missing 및 visible + forbidden의 제품 `/apply` 결과는 미검증이다. |

### 기본 거부/존재 노출 결정

`rows read`가 없는 자원은 전부 deny-all이며, id 기반 쓰기 대상 탐색도 같은 row policy를 WHERE에 두어 행 잠금·대기부터 하지 않는다. 이를 권한 누락 오류로 바꾸거나 읽기 기본값을 열어서는 안 된다. 따라서 `rows read` 선언을 잊은 fixture의 `WIRE.MISSING_TARGET`은 기본 거부의 부작용이 아니라 기대되는 정책 결과다. 존재 여부를 감추는 경계는 row visibility다. 서버가 해당 행을 보여 주는 actor에게 action만 거부할 때 `WIRE.FORBIDDEN`을 주는 계약은 별도다.

## 3. 2026-10-13 보안 감사 조치 유지 여부

권한 C와 직접 관련된 완료 조치는 S-01(참조 traverse 정책), S-14(필드 읽기 정보흐름; 기존 감사는 PoC로 표기), S-17(부정 Bool의 NULL fail-open)이다. 현 main source에는 S-01·S-17 수정을 반영한 생성 경로가 있고 회귀 source는 `spikes/spike-v2-read/tests/security_authorization.rs`, `tests/negated_policy.rs`다. S-14의 `crates/aip-cli/tests/field_policy_e2e.rs`는 PoC라 제품 유지 증거로 재사용할 수 없다.

초기 로컬 회차에서는 지정 검증 명령이 strict DSN guard에서 막혔다. 후속 GitHub Actions run `43796184726`에서는 아래 세 명령이 실행됐다. S-01/S-17 회귀 source는 통과했으나 spike 엔진 test이며 제품 서비스 HTTP/SDK 전체 조치가 유지됨을 단독으로 증명하지 않는다.

| 요청된 검증 | 명령 | 결과 |
|---|---|---|
| 제품 baseline | `cargo test -p aip-cli --locked --no-fail-fast` | CI exit 0. 19 test binary, 51 passed, 0 failed, 0 ignored. baseline 통과는 각 policy matrix assertion이 존재한다는 뜻이 아니다. |
| S-01 | `cargo test --manifest-path spikes/spike-v2-read/Cargo.toml --locked --test security_authorization --no-fail-fast` | CI exit 0. 2 passed. PostgreSQL에서 origin field policy traversal 1건과 반복 select reject 1건. |
| S-17 | `cargo test --manifest-path spikes/spike-v2-read/Cargo.toml --locked --test negated_policy --no-fail-fast` | CI exit 0. 1 passed. Active/suspended/anonymous row visibility가 `[1, 0, 0]`. |
| S-14 | `cargo test -p aip-cli --features poc-runtime --test field_policy_e2e --no-fail-fast` | **이전 로컬 회차에서 실행 시작 전 중단.** PoC 전용이며 `aip-test-support` build script가 동일 DSN 오류로 exit 101. 이번 CI에는 포함하지 않았고 제품 범위 회귀로 계산하지 않음. |

실행 가능한 제한 검증: `cargo test -p aip-syntax -p aip-sema -p aip-ir -p aip-plan -p aip-pg --no-fail-fast` 전체 exit 0 (IR unit 29 + codes 8, PG 4, Core IR 29, syntax 13 + hostile-input 3, sema conformance 포함). 이 명령은 PoC/compiler foundation이지 위 S 항목의 제품 실DB 회귀는 아니다. `cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --locked --no-fail-fast`도 전체 exit 0; default rowRead facts 및 caller-filter advisory의 **정적 facts 검사**를 확인했을 뿐 SQL 실행은 아니다.

초기 실패와 현재 CI 결과를 구분한다. `aip-test-support` build script의 strict DSN 오류는 앞선 로컬 command에서 DB test 시작 전 발생했다. 그 뒤 Codex nested command sandbox의 local `initdb`는 `shmget ... Operation not permitted`로 막혔다. GitHub Actions의 별도 Ubuntu 24.04 + PostgreSQL 17 SCRAM fixture에서는 readiness `SELECT 1`이 각 suite 전에 성공했고 세 명령이 통과했다. 보호 `~/.cargo` 접근 차단은 없었다.

## 4. 포팅 자산의 기존 사례: 5분류

분류는 갭의 주된 원인 기준이며, 문서 서술은 제품 동작 증거가 아니다. 아래는 자산이 기록한 과거 실패·차이와 코드/테스트를 다시 실행하지 못한 제한을 함께 적는다.

| 자산 | 분류 | 권한/정책 사례와 결과 |
|---|---|---|
| FlaskBB | Implementation | `PRODUCT-GAPS.md` G-11: create 호출 하나로 조직/첫 admin 같은 관련 행을 같이 만들 수 없어 원래 초안에서 생성 후 읽을 수 없는 고아가 됐다. 포팅은 `createdBy=actor`와 별도 membership 요청으로 해결해 두 요청 사이 구간을 남긴다. |
| FlaskBB | Semantic | G-7 nullable `hidden`: NULL에 `hidden != true`는 SQL 3값 논리에서 false/unknown이어서 작성자에게도 읽히지 않고 후속 create가 `WIRE.MISSING_TARGET`이 됐다. 이는 원본 ORM 기본값 차이와 작성 필드 생략 의미가 합쳐진 사례; 생성에서 false를 명시한다. 과거 2026-11-01 `rows read` 누락은 이와 달리 의도된 denyAll로 기록된다. |
| FlaskBB | Expressiveness | G-3 `exists R where a or b` 전체 조건은 행 없는 케이스에서 기대와 달랐다. 표현 괄호로 의미를 고정. G-6 Post 순서는 UUID/date 정렬이 원본 정수 ID 순서와 달라지는 반례가 있으나 전체 parity는 open. |
| FlaskBB | Abstraction | G-6의 projection 갱신은 before pointer, hidden 후보, Topic/Forum 서로 다른 순서 기준이 요구된다. 단일 generic `latest`/효과가 이 제어 흐름을 모두 표현하지 못하는 지점이 남는다. 이 C 검토는 범위를 넘어선 데이터/트랜잭션 갭은 수정하지 않았다. |
| FlaskBB | Verification | G-6 최신 노트는 V3 guard/read-lock source를 compile·PG 미실행이라고 명시한다. 이번에는 S-01/S-17의 PostgreSQL spike regressions를 실행했지만, spike 결과만으로 실제 `aip service` 전체 포팅 parity를 주장할 수 없다. |
| Infisical | Implementation | `INFISICAL-GAPS.md` G13: 원본 job 종류 중 `secretReminder` 하나만 실제 구현, 나머지 큐 동작은 미구현. 암호화 값 버전·회전(G2) 또한 제품 기능 gate/부재가 명시돼 있다. |
| Infisical | Semantic | G7은 원본 CASL 조건 권한(경로·환경·태그)과 포팅의 역할 기반 admin/member/viewer predicate가 다름. 권한 핵심은 EE 미열람이라 비교가 불완전하다고 기록한다. 이를 같은 권한이라고 추정하지 않는다. |
| Infisical | Expressiveness | G14/G16: 임의 깊이 `folder.env` 효과 경로, 시각 산술·14일 grace 표현 부족. `hardDelete` 범위 제한/거부 등으로 포팅 격차를 드러내고 권한을 완화하지 않는다. |
| Infisical | Abstraction | G11 조직/멤버십과 환경/루트 폴더가 별도 create 요청이며 중간 고아/경쟁 경계가 남는다. create + 관련 행을 단일 작업으로 묶는 추상화가 없다. |
| Infisical | Verification | `INFISICAL-GAPS.md`는 특정 live DB 재현과 수정 전 RED 기록을 구분하고, 과거 “P1 5건 중 4건 해결” 추정은 근거 부족으로 철회했다. 2026-11-06 암호화 fixture는 시작 gate만 확인하고 새 source/실DB semantics로 확대하지 않았다. |
| FaceReview 관리자 포팅 자산 | Implementation | `porting/facereview/ui-overlay/README.md`는 관리자 7 read/6 write와 SDK adapter 경로를 분리한다. 과거 실제 admin console에서 aggregate SDK 함수 부재와 UUID durable-job input 검증 누락이 발견돼 SDK에서 보완됐다. 정책 판정 없이 UI role button 숨김만으로는 경계가 되지 않는다. |
| FaceReview 관리자 포팅 자산 | Semantic | 과거 재-reject 테스트 기대가 `WIRE.EFFECT_TARGET_MISSING`이었으나 실제 정상 응답은 from-state mismatch `WIRE.INVALID_STATE`; 허용값 기대를 코드 의미에 맞게 고쳤다. role/대상 보호는 `mysql_facereview_admin_ui.rs`가 일반 멤버 승격·관리자의 보호 role 대상 변경을 `WIRE.FORBIDDEN`으로 기대한다. |
| FaceReview 관리자 포팅 자산 | Expressiveness | admin dashboard 통계는 standalone aggregate와 read extensions로 구성한다. 집계 대상이 민감 row policy를 상속하지 않는 경계에서 정의의 `sourceAccess`/`where`가 정확해야 한다. 제품 DSL의 aggregate opt-in advisory와 연결되는 위험이다. |
| FaceReview 관리자 포팅 자산 | Abstraction | 13개 admin API와 로그인/인증 공통 경로를 adapter 층에서 합성한다. 이것은 관리자 UX 편의 계층이며 server policy는 여전히 별도 권한 계약이어야 한다. |
| FaceReview 관리자 포팅 자산 | Verification | README는 같은 SHA/실제 MySQL 및 browser evidence와 미도달·미확인 화면을 구별한다. 이 기록은 설계 자료이지 현재 main의 테스트 실행 출력이 아니다. 이 Run에는 관리자 live policy test DB가 없어 재확인 불가. |

## 5. 결함 목록·최소 재현·수정 난이도

### 확인된 결함

없음 (0건). 이 판정은 “보안 결함 없음” 보증이 아니다. CI가 S-01/S-17 회귀를 통과했으며 `aip-cli` baseline도 성공했지만, 전체 `aip service` 권한 matrix의 제품 HTTP/SDK 경로는 실행하지 않았다. 테스트된 행위에서 결함은 관측되지 않았다. 수정 전 RED를 만들 제품 결함이 없으므로 제품 재현 테스트를 새로 추가하지 않았다.

### 의미 불명확/설계 위험 (결함으로 오판하지 않도록 분리)

1. **Aggregate의 숨은 행 입력**: `callerFilter read`가 정책 있는 source에 작성되면 aggregate에서 `rows read`가 자동으로 적용되지 않는다. checker advisory를 기록하고, 명시 aggregate `where`를 권한다. 실제 악용은 source policy와 작성자가 부여한 공개 aggregate access가 함께 있어야 하므로 코드 실행 없이 exploit이라고 단정하지 않는다. 보호 목적이면 advisory를 배포 오류로 승격하거나 `sourceAccess`의 행 집합 정의를 필수로 하는 방안을 별도 설계한다. 의미/호환 영향: 중간, 소규모 checker 정책 변경 가능; 대규모 재설계는 아님. 이번에는 DSL/기대값을 바꾸지 않았다.
2. **Tenant 보증**: 현재 서비스 정의 언어에서 tenant는 명시 개념이 아니다. row/transition predicate에 조직 조건을 작성해야 한다. 제품 차원의 자동 cross-tenant invariants라고 표현하면 안 된다. 별도 DSL 추가는 본 작업 금지 범위여서 제안하지 않는다.
3. **서비스 actor**: worker/service 작업이 어떤 actor identity를 쓰는지 endpoint별 계약 확인이 필요하다. `actor_id=None`을 사용자 인증과 같다고 추정할 수 없다. 제품 live job/extension 권한 시험이 필요하다.

각 위험의 재현 조건은 위 표의 source/기존 테스트 경로다. RED 출력은 없다. 현행 작성 규칙을 바꾸는 patch는 추가하지 않았다.

### 2026-11-16 격리 PostgreSQL 후속 실행

로컬 시도는 별도 이력으로 남긴다. Codex nested command sandbox에서 소유 클러스터 초기화가 막혀 같은 대상을 재시도하지 않았다. 이후 기존 `Database contracts` workflow를 audit ref에서만 보완해 실행했다. strict DB guard와 policy 기대값은 완화하지 않았다.

| 완료 기준 | 실행/관측 | 결과 |
|---|---|---|
| 전용 DB 초기화 | Codex nested command sandbox에서 `/opt/homebrew/opt/postgresql@17/bin/initdb -D /tmp/aip-audit-c-policy-pgdata -U runner --auth-local=trust --auth-host=trust --no-instructions` | **실패, initdb 단계.** 서버 시작이나 연결 전에 bootstrap이 `FATAL: could not create shared memory segment: Operation not permitted`, `DETAIL: Failed system call was shmget(...)`로 종료됐고 `initdb`가 데이터 디렉터리를 제거했다. 이는 해당 command sandbox에서 관측된 IPC 거부다. |
| 로컬 전용 DB 초기화 | Codex nested sandbox의 `initdb -D /tmp/aip-audit-c-policy-pgdata -U runner ...` | **환경 차단.** `shmget ... Operation not permitted`; initdb가 데이터 디렉터리를 제거했다. 로컬 endpoint나 DB 연결은 만들어지지 않았다. |
| CI 격리 DB readiness | workflow의 각 suite 직전 `psql -X "$AIP_TEST_DATABASE_URL" -Atc 'SELECT 1'` | **통과 3회.** 세 readiness log가 모두 `1`을 출력했다. Fixture는 GitHub-hosted Ubuntu 24.04와 PostgreSQL 17 SCRAM이었다. |
| `cargo test -p aip-cli --locked --no-fail-fast` baseline | audit workflow step | **통과.** 19 test binary에서 51 passed, 0 failed, 0 ignored. |
| S-01 `security_authorization` | `cargo test --manifest-path spikes/spike-v2-read/Cargo.toml --locked --test security_authorization --no-fail-fast` | **통과.** 2 passed, 0 failed. origin field policy 관계 traversal을 실제 DB에서 확인했다. |
| S-17 `negated_policy` | `cargo test --manifest-path spikes/spike-v2-read/Cargo.toml --locked --test negated_policy --no-fail-fast` | **통과.** 1 passed, 0 failed. active/suspended/anonymous 가시성 `[1,0,0]`. |
| S-14 PoC 대조 | `cargo test -p aip-cli --features poc-runtime --test field_policy_e2e --no-fail-fast` | **미실행.** 이 workflow에는 포함하지 않았다. 실행되더라도 `aip_runtime::Engine` 기반 PoC 결과는 제품 `aip service` 증거가 아니다. |

제품 `aip service`의 범위는 구분한다. `crates/aip-cli/tests/field_policy_e2e.rs`는 PoC라 제품 증거가 아니다. `crates/aip-service/tests/mysql_read_http.rs::mysql_read_path_serves_policy_reads_and_fences_ddl` 및 `mysql_write_http.rs::mysql_apply_is_atomic_idempotent_audited_and_fenced`는 서비스 API 행위를 다루지만 MySQL adapter 경로다. 이번 CI에서 통과한 S-01/S-17은 실제 DB를 쓴 `spike-v2-read` 회귀이며, 서비스의 인증→HTTP→SDK end-to-end 결과와 구별한다.

현재 matrix의 행위별 상태:

- 기본 거부(선언 누락): **미검증**. S-17은 부정식의 NULL fail-closed만 확인한다.
- 숨김 행과 부재 행: **미검증**. 제품 `/apply`의 `WIRE.MISSING_TARGET` 비교는 실행하지 않았다.
- visible + forbidden: **미검증**. 제품 `/apply`의 `WIRE.FORBIDDEN` 경로는 실행하지 않았다.
- `fieldRead`와 관계 가시성: **일부 검증**. S-01 엔진 regression은 다른 사용자/anonymous의 to-one reference를 null로 숨겼다. 제품 `/read`와 generated SDK를 통한 응답은 미검증.
- 부정 actor 정책: **검증됨 (S-17 범위)**. 실제 PG에서 active 1행, suspended와 anonymous 0행을 확인했다.

이번 CI에서 실행한 세 test command는 모두 통과했다. 제품 권한 matrix의 미검증 항목은 결함으로 단정하지 않는다. local `initdb` 실패 후 같은 local 대상은 재시도하지 않았으며, GitHub Actions는 별도 owned runner/service container에서 실행했다.

### 속성 기반 불변식 시험 가능성 및 초안

가능하다. 신규 property-testing 의존성 없이 현재 strict DB harness에서 seeded/deterministic case 생성으로 작성할 수 있다. 이번에는 property test를 실행하거나 추가하지 않았다. S-01/S-17 deterministic regressions만 실행했다.

```text
for each generated actor and hidden row r (rowRead(r, actor) != TRUE):
  baseline = service.read(actor, request)
  add_or_mutate_only_hidden_row(r)
  assert service.read(actor, request) == baseline
  assert visible relation projections / aggregates / order / page boundaries are unchanged
  before = database_snapshot()
  result = service.apply(actor, forbidden_target_or_reference)
  assert result.code in {WIRE.MISSING_TARGET, WIRE.FORBIDDEN}
  assert database_snapshot() == before
  assert generated SDK preserves the same wire code and returns no hidden value
```

생성 축은 `rowRead` 부울/NULL(익명 포함), target 존재·부재, Ref target 가시성, fieldRead, aggregate `callerFilter`/explicit where, actor role 변경 전후다. 읽기 결과의 비간섭 equality와 쓰기 snapshot 불변을 분리한다. 정책 자체가 허용한 차이는 oracle의 전제에서 빼고, 생성된 모든 정책/요청을 고정 seed와 최소 fixture로 축소할 수 있어 property 방식이 적합하다. 첫 승인 지점은 property가 아니라 위에 열거한 사례를 deterministic regression으로 고정하는 것이며, 현재 테스트를 추가/수정한 사실은 없다.

## 6. 후속 확인 순서·난이도

| 우선순위 | 확인 작업 | 수정 난이도 | 대규모 재설계 |
|---|---|---|---|
| P1 | 제품 `aip service` `/read`·`/apply`/SDK 경로에서 선언 누락 default deny, hidden vs missing, visible+forbidden, status/wire preservation을 동일 PostgreSQL fixture로 대조. | 중간 | 아니오 |
| P1 | 실제 service `/read`에서 to-many traversal, aggregate, filter/sort/page에 대한 hidden-only row 비간섭과 fieldRead 직접 projection 확인. S-01의 to-one 엔진 회귀 결과는 이미 확보했으나 이 범위는 별도다. | 중간 | 아니오 |
| P2 | role/tenant/ownership 변경이 진행 중인 apply/read와 겹칠 때 새 권한이 어느 요청부터 적용되는지, DB 격리 계약을 실제 경합 시험으로 측정. | 중간~높음 | 아니오, 현행 lock/isolation 경계 우선 확인 |
| P2 | worker/service actor와 end-user actor의 facts, audit actor, allow/rowRead 결과를 같은 operation으로 대조. | 중간 | 전용 service principal을 정의에 넣는 요구가 확인되는 경우에만 별도 검토 |

로컬 작업자 PostgreSQL DSN은 만들지 못했지만, CI runner의 임시 SCRAM DSN으로 이 절의 세 command를 실행했다. secret 또는 `.env`를 읽지 않았고 workflow의 disposable fixture credential만 사용했다.

## 7. 초기 로컬 실행 근거 (후속 CI보다 이전)

```text
$ cargo test -p aip-cli --no-fail-fast
thread 'main' panicked at crates/aip-test-support/build.rs:13:9:
AIP_TEST_DATABASE_URL is required; refusing PostgreSQL defaults

$ cargo test -p aip-syntax -p aip-sema -p aip-ir -p aip-plan -p aip-pg --no-fail-fast
test result: ok. 29 passed; 0 failed (aip-ir)
... aip-syntax 16 passed; aip-pg 4 passed; overall exit 0

$ cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --locked --no-fail-fast
test result: ok. 2 passed; 0 failed (advisories)
... all spike-v1-fixture test binaries passed; overall exit 0

$ cargo test --manifest-path spikes/spike-v2-read/Cargo.toml --locked --test security_authorization --no-fail-fast
AIP_TEST_DATABASE_URL is required; refusing PostgreSQL defaults

$ cargo test --manifest-path spikes/spike-v2-read/Cargo.toml --locked --test negated_policy --no-fail-fast
AIP_TEST_DATABASE_URL is required; refusing PostgreSQL defaults

$ cargo test -p aip-cli --features poc-runtime --test field_policy_e2e --no-fail-fast
AIP_TEST_DATABASE_URL is required; refusing PostgreSQL defaults

$ env -u PGHOST -u PGPORT /opt/homebrew/opt/postgresql@17/bin/initdb -D /tmp/aip-audit-c-policy-pgdata -U runner --auth-local=trust --auth-host=trust --no-instructions
FATAL: could not create shared memory segment: Operation not permitted
DETAIL: Failed system call was shmget(...)
initdb: removing data directory "/tmp/aip-audit-c-policy-pgdata"

$ git status --short --branch
## audit/semantic-c-policy-20261116...origin/main
```

이 출력들은 9aed53d에서 기록한 **초기 로컬 회차**의 결과다. 이후 검증은 아래 GitHub Actions 회차를 따른다.

## 8. GitHub Actions 격리 PostgreSQL 실행 결과

- Workflow: [Database contracts](https://github.com/winterholic/Application-Intent-Protocol/actions/runs/43796184726)
- Run ID: `43796184726`; event: `workflow_dispatch`; branch: `audit/semantic-c-policy-20261116`
- head SHA: `57655d1fdf63ce830ad8b356a37aef345c7a10f0`
- fixture: GitHub-hosted `ubuntu-24.04`, isolated PostgreSQL 17 SCRAM service at the workflow's explicit non-default port. Three per-suite `psql SELECT 1` readiness logs all output `1`.
- run conclusion: `success`. PostgreSQL test job `postgres-runtime-role` succeeded. Unrelated `mysql-and-mariadb-fences` job was `skipped` by the audit-ref-only condition.

| Command | 결과 |
|---|---|
| `cargo test -p aip-cli --locked --no-fail-fast` | exit 0; 19 test binaries, 51 passed, 0 failed, 0 ignored. |
| `cargo test --manifest-path spikes/spike-v2-read/Cargo.toml --locked --test security_authorization --no-fail-fast` | exit 0; 2 passed, 0 failed. `single_reference_traverse_respects_origin_field_policy` passed for owner/other/anonymous; repeated-select validation passed. |
| `cargo test --manifest-path spikes/spike-v2-read/Cargo.toml --locked --test negated_policy --no-fail-fast` | exit 0; 1 passed, 0 failed. Active, suspended, anonymous visible counts were `[1, 0, 0]`. |

Logs artifact `policy-c-postgres-43796184726-1` contains each command log and its pre-test readiness result. `gh run view 43796184726 --log` could not write its CLI cache at `/Users/winterholic/.cache/gh/run-log-43796184726-1794798419.zip` (`operation not permitted`); that log-view command was not retried. The supported `gh run download` artifact retrieval succeeded and is the evidence used above.

The passing runs confirm the targeted S-01/S-17 engine regressions and successful CLI baseline on real isolated PG17. They do not cover service `/read` + `/apply` + SDK as a combined policy contract, default missing-policy behavior, write error distinction, cross-tenant isolation, aggregates, or concurrent role change. No assertion failed, so this run produced no confirmed defect and no RED regression to add.
