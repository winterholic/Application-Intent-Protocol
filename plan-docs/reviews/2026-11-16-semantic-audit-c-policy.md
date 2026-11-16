# AIP 권한·정책 의미 감사 (C)

- 기준: `origin/main` at `f1c6696` (2026-11-16), branch `audit/semantic-c-policy-20261116`
- 범위: 제품 정의의 정책 작성 → 사실 변환 → `aip service` 실행 → HTTP/SDK 오류·결과
- 결론: 이번 실행에서 **새로 확인된 결함은 0건**이다. 제품 PostgreSQL 테스트를 실행하지 못했으므로 이 결론은 코드 경로 정적 추적과 가능한 비DB 테스트 범위에 한정한다. 기존 정책 보안 회귀의 현재 동작은 미검증이다.
- 결함 수정은 하지 않았다. 재현 조건·RED 출력을 확보하지 않은 상태에서 테스트를 결함 증거로 추가하지 않았다.
- 후속 DB 검증: 작업자 소유 PostgreSQL 초기화가 Codex command sandbox의 System V 공유 메모리 `Operation not permitted`로 막혔다. 아래 후속 실행 기록에 따라 DB 연결 및 보안 회귀는 여전히 미실행이다.

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

상태 용어: **정적 일치**는 현재 코드 표현이 계약과 맞는다는 뜻, **실행 확인**은 이 Run의 시험 출력이 있는 경우, **테스트 공백**은 현재 제품 DB 경로의 실행을 못 한 경우다. 이번에는 아래 권한 항목 중 실DB 실행 확인은 없다.

| 항목 | 판정 | 코드 경로·기존 회귀 | 남은 확인 |
|---|---|---|---|
| 기본 거부 | 정적 일치, 제품 DB 테스트 공백 | `spike-v1-fixture/src/sema.rs:1087-1097`: `rows read` 누락은 facts에서 `{"default":"denyAll"}`. `spike-v2-read/src/sqlgen.rs:563-566`: `denyAll`을 SQL `FALSE`로 내린다. `spike-v1-fixture/tests/v1.rs:153-155`, `tests/diagnostics.rs`도 facts 기본값을 고정한다. | 서비스 실제 PG `/read`와 `/apply`에서 빠진 정책이 어떤 응답·데이터 효과를 내는지 미실행. 최근 `WIRE.MISSING_TARGET` fixture는 실패가 아니라 숨은/부재 행을 합치는 계약과 부합한다. |
| 읽기·쓰기 평가 차이 | 의미가 구분됨, live 차이 테스트 공백 | 읽기는 `rowRead`로 결과 행을 제한한다. 쓰기 `judge_query`는 `rowRead`, transition `allow`, `from`을 각각 검사한다(`spike-v3-write/src/lib.rs:374-415`). 읽을 수 있고 전이 권한이 없으면 `WIRE.FORBIDDEN`; 보이지 않거나 없는 id는 `WIRE.MISSING_TARGET`(`:461-480`). | 동일 facts/actor/행에서 read·apply·create 및 SDK 오류를 한 번에 대조하는 제품 PG 시험 미실행. |
| 필드 수준 접근 제어 | 구현 경로 있음, 우회 반례 시험 공백 | 작성 언어의 `field <name> read when`은 facts `fieldRead`로 변환(`spike-v1-fixture/src/parser.rs:378-383`, `sema.rs:1099-1110`). 직접 select는 CASE redaction(`spike-v2-read/src/plan.rs:550-566`), traverse도 자식 field policy를 재적용(`:624-638`). 정책 필드는 filter/sort/aggregate 및 효과의 정보흐름 사용을 sema에서 제한한다(`spike-v1-fixture/src/sema.rs:1815-1850`, `1692-1696`). | `field_policy_e2e`는 PoC `aip-pg`를 검증하므로 제품 증거가 아니다. 제품 경로의 field-read + relation + generated SDK live case를 미실행. |
| 참조·join·aggregate·sort를 통한 정보 노출 | 대체로 방어선 있음, aggregate의 opt-in 경계는 명시적 위험 | traverse는 대상 `rowRead`, 참조 출발 필드 정책을 모두 조인 조건에 넣는다(`spike-v2-read/src/plan.rs:663-681`). `traverseMany`는 하위 행 정책을 다시 적용하도록 facts를 만든다(`spike-v1-fixture/src/sema.rs:1882,1954`). 필드 정책 열은 filter/sort 불가. 독립 aggregate는 source의 `rowRead`를 자동 상속하지 않으며 `callerFilter read`에 숨은 행 조건이 없으면 `DEF.ADVISORY_CALLER_FILTER_POLICY` advisory를 낸다(`spike-v1-fixture/tests/advisories.rs:20-38`, syntax card). | advisory를 무시하고 정의를 올릴 수 있다. 정책을 명시하지 않은 aggregate의 안전성은 제품 책임자의 선택 영역이라 자동 보장으로 간주하지 않는다. 코드상 예상된 선택이므로 이번에는 결함으로 분류하지 않았다. 공격형 PG 비간섭 테스트 미실행. |
| 테넌트 격리 | 개념 의미 제한, 교차 테넌트 격리 테스트 공백 | 현재 `.aip` v1 parser/AST에는 tenant 선언이 없다(`spike-v1-fixture/src/ast.rs`, `parser.rs`; `rg tenant` 결과 없음). 제품 작성자는 조직/소유자 경계를 `rows read`, transition `allow`, 관계 조건에 표현한다. 별도 `crates/aip-ir::tenant`/`crates/aip-pg::tenant`는 PoC Core IR 경로여서 `aip service` tenant 보증 근거가 아니다. | 다중 조직 owner/ref/filter/traverse/aggregate를 통합한 실DB 비간섭 시험 미실행. 구조적 tenant 보장을 한다고 광고할 근거가 없다. |
| 작업 실행 중 권한 변경 | 잠금 의도는 보임, 경합 시험 공백 | write policy context는 참조 정책 읽기를 잠그도록 설정(`spike-v3-write/src/lib.rs:376-400`, `1023-1034`, `1087-1094`). read path는 한 SQL statement 안에서 결과·가시성 정책을 평가한다. | 역할·소유권 변경과 read/apply 경합, 새 요청 경계에서 반영 여부, SERIALIZABLE predicate/부재 판정을 제품 DB에서 실행하지 못함. 트랜잭션·동시성의 전체 소유 검토는 본 작업 범위 밖이다. |
| 서비스 주체와 사용자 주체 | 의미 일부 존재, 동등성/구분 시험 공백 | facts에 actor mode가 있고 관리형 principal 또는 resource actor를 설정한다. 서비스 초기화는 `with_resource_actor`를 통해 actor binding을 연결한다(`crates/aip-service/src/lib.rs:1678-1683`). write/query 정책은 actor id를 `Ctx`에 전달한다. job/service 경로에서 사용자 actor를 보존하는지 별도 확인 필요. | user token, anonymous, managed principal, worker/service 호출별 같은 선언의 allow·audit actor·자료 접근 시험 미실행. 별도 service-role 정책 주체를 보장한다고 판정하지 않는다. |
| 권한 거부와 리소스 부재 구별 | 행 가시성 은닉은 정적 일치, SDK end-to-end 공백 | `spike-v3-write/src/lib.rs:474-480`은 안 보이는 행과 없는 행을 같은 `WIRE.MISSING_TARGET`으로 돌려 ID 존재 여부를 숨긴다. visible row + denied transition은 `WIRE.FORBIDDEN`(:461-463). SQL `rowRead`가 없는 대상이 숨겨지는 경우도 `MISSING_TARGET`이다. | SDK(HTTP 200 envelope 포함)가 오류 코드를 손실 없이 전달하고 못 읽는 대상을 `NOT_FOUND`로 보이는지 이번 Run에는 live test하지 않았다. 허용된 read 영역 안에서 action-forbidden을 `FORBIDDEN`으로 반환하는 것은 의도된 구분이다. |

### 기본 거부/존재 노출 결정

`rows read`가 없는 자원은 전부 deny-all이며, id 기반 쓰기 대상 탐색도 같은 row policy를 WHERE에 두어 행 잠금·대기부터 하지 않는다. 이를 권한 누락 오류로 바꾸거나 읽기 기본값을 열어서는 안 된다. 따라서 `rows read` 선언을 잊은 fixture의 `WIRE.MISSING_TARGET`은 기본 거부의 부작용이 아니라 기대되는 정책 결과다. 존재 여부를 감추는 경계는 row visibility다. 서버가 해당 행을 보여 주는 actor에게 action만 거부할 때 `WIRE.FORBIDDEN`을 주는 계약은 별도다.

## 3. 2026-10-13 보안 감사 조치 유지 여부

권한 C와 직접 관련된 완료 조치는 S-01(참조 traverse 정책), S-14(필드 읽기 정보흐름; 기존 감사는 PoC로 표기), S-17(부정 Bool의 NULL fail-open)이다. 현 main source에는 S-01·S-17 수정을 반영한 생성 경로가 있고 회귀 source는 `spikes/spike-v2-read/tests/security_authorization.rs`, `tests/negated_policy.rs`다. S-14의 `crates/aip-cli/tests/field_policy_e2e.rs`는 PoC라 제품 유지 증거로 재사용할 수 없다.

다만 지정 검증 명령은 이번 Run에서 실행되지 않았다. 따라서 어느 조치도 **현재 실행된 테스트로 유지 확인**했다고 쓰지 않는다.

| 요청된 검증 | 명령 | 결과 |
|---|---|---|
| 제품 baseline | `cargo test -p aip-cli --no-fail-fast` | 실패 stage: `aip-test-support` build script. 관측 오류: `AIP_TEST_DATABASE_URL is required; refusing PostgreSQL defaults`. 테스트 실행 전 중단. |
| S-01 | `cargo test --manifest-path spikes/spike-v2-read/Cargo.toml --locked --test security_authorization --no-fail-fast` | 같은 strict DB guard에서 build 실패. PostgreSQL 회귀 미실행. |
| S-17 | `cargo test --manifest-path spikes/spike-v2-read/Cargo.toml --locked --test negated_policy --no-fail-fast` | 시도함. `aip-test-support` build script가 `AIP_TEST_DATABASE_URL is required; refusing PostgreSQL defaults`로 exit 101. assertion 실행 전 중단. |
| S-14 | `cargo test -p aip-cli --features poc-runtime --test field_policy_e2e --no-fail-fast` | 시도함. PoC 전용이며 `aip-test-support` build script가 동일 DSN 오류로 exit 101. 제품 범위 회귀로 계산하지 않음. |

실행 가능한 제한 검증: `cargo test -p aip-syntax -p aip-sema -p aip-ir -p aip-plan -p aip-pg --no-fail-fast` 전체 exit 0 (IR unit 29 + codes 8, PG 4, Core IR 29, syntax 13 + hostile-input 3, sema conformance 포함). 이 명령은 PoC/compiler foundation이지 위 S 항목의 제품 실DB 회귀는 아니다. `cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --locked --no-fail-fast`도 전체 exit 0; default rowRead facts 및 caller-filter advisory의 **정적 facts 검사**를 확인했을 뿐 SQL 실행은 아니다.

환경·실패를 구분한다: 실패한 엔진은 Codex nested command sandbox의 Cargo test build이며, 명령은 위와 같다. 실패 단계는 `aip-test-support` build script이고 관측 오류는 strict DB URL 미설정이다. 이는 보호 `~/.cargo` 접근 차단이 아니므로 cache 우회는 시도하지 않았다. DB 동작은 미실행으로 둔다.

## 4. 포팅 자산의 기존 사례: 5분류

분류는 갭의 주된 원인 기준이며, 문서 서술은 제품 동작 증거가 아니다. 아래는 자산이 기록한 과거 실패·차이와 코드/테스트를 다시 실행하지 못한 제한을 함께 적는다.

| 자산 | 분류 | 권한/정책 사례와 결과 |
|---|---|---|
| FlaskBB | Implementation | `PRODUCT-GAPS.md` G-11: create 호출 하나로 조직/첫 admin 같은 관련 행을 같이 만들 수 없어 원래 초안에서 생성 후 읽을 수 없는 고아가 됐다. 포팅은 `createdBy=actor`와 별도 membership 요청으로 해결해 두 요청 사이 구간을 남긴다. |
| FlaskBB | Semantic | G-7 nullable `hidden`: NULL에 `hidden != true`는 SQL 3값 논리에서 false/unknown이어서 작성자에게도 읽히지 않고 후속 create가 `WIRE.MISSING_TARGET`이 됐다. 이는 원본 ORM 기본값 차이와 작성 필드 생략 의미가 합쳐진 사례; 생성에서 false를 명시한다. 과거 2026-11-01 `rows read` 누락은 이와 달리 의도된 denyAll로 기록된다. |
| FlaskBB | Expressiveness | G-3 `exists R where a or b` 전체 조건은 행 없는 케이스에서 기대와 달랐다. 표현 괄호로 의미를 고정. G-6 Post 순서는 UUID/date 정렬이 원본 정수 ID 순서와 달라지는 반례가 있으나 전체 parity는 open. |
| FlaskBB | Abstraction | G-6의 projection 갱신은 before pointer, hidden 후보, Topic/Forum 서로 다른 순서 기준이 요구된다. 단일 generic `latest`/효과가 이 제어 흐름을 모두 표현하지 못하는 지점이 남는다. 이 C 검토는 범위를 넘어선 데이터/트랜잭션 갭은 수정하지 않았다. |
| FlaskBB | Verification | G-6 최신 노트는 V3 guard/read-lock source를 compile·PG 미실행이라고 명시한다. spike 소스/SQLite source oracle/기존 `security_authorization`만으로 실제 `aip service` 전체 포팅 parity를 주장할 수 없다. 이번에도 strict DSN이 없어 보안 PG 회귀를 재실행 못했다. |
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

없음 (0건). 이 판정은 “보안 결함 없음” 보증이 아니다. 실제 제품 DB 재현을 실행하지 못했으므로 새로 **확인된** 결함이 없다는 뜻이다. 따라서 수정 전 RED를 관측한 제품 재현 테스트도 추가하지 않았다. 이는 “결함마다 test를 추가” 조건에서 확인된 결함 집합이 빈 경우에 해당한다.

### 의미 불명확/설계 위험 (결함으로 오판하지 않도록 분리)

1. **Aggregate의 숨은 행 입력**: `callerFilter read`가 정책 있는 source에 작성되면 aggregate에서 `rows read`가 자동으로 적용되지 않는다. checker advisory를 기록하고, 명시 aggregate `where`를 권한다. 실제 악용은 source policy와 작성자가 부여한 공개 aggregate access가 함께 있어야 하므로 코드 실행 없이 exploit이라고 단정하지 않는다. 보호 목적이면 advisory를 배포 오류로 승격하거나 `sourceAccess`의 행 집합 정의를 필수로 하는 방안을 별도 설계한다. 의미/호환 영향: 중간, 소규모 checker 정책 변경 가능; 대규모 재설계는 아님. 이번에는 DSL/기대값을 바꾸지 않았다.
2. **Tenant 보증**: 현재 서비스 정의 언어에서 tenant는 명시 개념이 아니다. row/transition predicate에 조직 조건을 작성해야 한다. 제품 차원의 자동 cross-tenant invariants라고 표현하면 안 된다. 별도 DSL 추가는 본 작업 금지 범위여서 제안하지 않는다.
3. **서비스 actor**: worker/service 작업이 어떤 actor identity를 쓰는지 endpoint별 계약 확인이 필요하다. `actor_id=None`을 사용자 인증과 같다고 추정할 수 없다. 제품 live job/extension 권한 시험이 필요하다.

각 위험의 재현 조건은 위 표의 source/기존 테스트 경로다. 현재 RED 출력이 없으므로 테스트 이름·실패 문자열을 결함 증거로 주장하지 않는다. 현행 작성 규칙을 바꾸지 않는 작은 patch 제안도 이번 증거만으로는 정당화되지 않는다.

### 2026-11-16 격리 PostgreSQL 후속 실행

목표는 제품 DB 권한 검증과 S-01/S-17 회귀를 작업자 소유 PostgreSQL에서 보완하는 것이었다. 저장소의 strict DSN guard와 권한 기대값은 변경하지 않았다. 부모 command 환경에서 `PGHOST`, `PGPORT` 변수가 존재함을 변수명만 확인했고, 테스트 명령에는 둘 다 `env -u`로 제거할 계획이었다. `AIP_TEST_DATABASE_URL`과 `AIP_TEST_ADMIN_URL`은 아직 전달하지 않았다.

| 완료 기준 | 실행/관측 | 결과 |
|---|---|---|
| 전용 DB 초기화 | Codex nested command sandbox에서 `/opt/homebrew/opt/postgresql@17/bin/initdb -D /tmp/aip-audit-c-policy-pgdata -U runner --auth-local=trust --auth-host=trust --no-instructions` | **실패, initdb 단계.** 서버 시작이나 연결 전에 bootstrap이 `FATAL: could not create shared memory segment: Operation not permitted`, `DETAIL: Failed system call was shmget(...)`로 종료됐고 `initdb`가 데이터 디렉터리를 제거했다. 이는 해당 command sandbox에서 관측된 IPC 거부다. |
| 명시 DSN 연결 검사 | `psql -X "$AIP_TEST_DATABASE_URL" -Atc 'SELECT 1'` | **미실행.** 초기화된 서버가 없어 시험 대상 endpoint가 없었다. 다른 실행 엔진/방법으로 우회하지 않았다. |
| `cargo test -p aip-cli --no-fail-fast` baseline | 실행 예정 | **미실행.** 전용 DB를 만들지 못해 DSN을 줄 수 없다. 앞선 회차의 strict build guard 실패는 assertion 결과가 아니다. |
| S-01 `security_authorization` | `cargo test --manifest-path spikes/spike-v2-read/Cargo.toml --locked --test security_authorization --no-fail-fast` | **미실행.** 해당 suite는 `single_reference_traverse_respects_origin_field_policy`와 반복 select reject를 포함하지만 DB 회귀 결과는 없다. |
| S-17 `negated_policy` | `cargo test --manifest-path spikes/spike-v2-read/Cargo.toml --locked --test negated_policy --no-fail-fast` | **미실행.** `negated_actor_attribute_denies_the_anonymous_caller`의 현 실행 결과는 없다. |
| S-14 PoC 대조 | `cargo test -p aip-cli --features poc-runtime --test field_policy_e2e --no-fail-fast` | **미실행.** 실행되더라도 `aip_runtime::Engine` 기반 PoC 결과로 제품 `aip service` 증거가 되지 않는다. |

DB 시도가 막힌 뒤 제품 `aip service` 경로에서 재사용할 수 있는 regression source를 다시 분류했다. `crates/aip-cli/tests/field_policy_e2e.rs`는 PoC라 제외한다. `crates/aip-service/tests/mysql_read_http.rs::mysql_read_path_serves_policy_reads_and_fences_ddl`와 `mysql_write_http.rs::mysql_apply_is_atomic_idempotent_audited_and_fenced`는 서비스 read/write API, relation/field projection 및 `WIRE.FORBIDDEN`/`WIRE.MISSING_TARGET` 예시를 포함하지만 MySQL 경로다. 따라서 이 테스트 source가 존재해도 이번 PostgreSQL 제품 경로 실행으로 승격하지 않는다. `spikes/spike-v2-read/tests/security_authorization.rs`와 `negated_policy.rs`도 회귀 구현 위치는 제공하지만, 독립 spike suite 실행 전에는 결과가 없다.

이번 회차의 행위별 상태:

- 기본 거부: **제품 PostgreSQL 실행 미검증**. 정적 근거는 §2 및 V1 fixture 검사로 제한한다.
- 숨김 행과 부재 행: **제품 PostgreSQL 실행 미검증**. 서비스 MySQL 쓰기 테스트 source에는 `WIRE.MISSING_TARGET` 회귀가 있지만 이번에는 미실행이다.
- visible + forbidden: **제품 PostgreSQL 실행 미검증**. 서비스 MySQL 쓰기 테스트 source에 `WIRE.FORBIDDEN` 회귀가 있지만 이번에는 미실행이다.
- `fieldRead`와 관계 가시성: **제품 PostgreSQL 실행 미검증**. MySQL read service 테스트와 spike S-01 source는 관련 범위를 일부 포함하나 이번 실행 증거가 아니다. 제품 PostgreSQL에서 합친 핵심 사례가 확인되지 않아 추가 테스트도 작성하지 않았다.

새로 확인된 결함은 없다. 이는 테스트가 통과했다는 뜻이 아니며, DB 연결 전 차단으로 새 결함을 찾지 못했다는 뜻이다. 확인된 결함이 없으므로 RED 테스트를 추가하지 않았다. 시작한 PostgreSQL 서버는 없어서 정리할 프로세스도 없으며, initdb가 실패한 소유 데이터 디렉터리를 제거했음을 출력에서 확인했다. 실패 후 동일 대상을 다른 실행 수단으로 재시도하지 않았다.

### 속성 기반 불변식 시험 가능성 및 초안

가능하다. 신규 property-testing 의존성 없이, 현재 strict DB harness에서 seeded/deterministic case 생성으로 먼저 작성할 수 있다. 본 Run에서는 전용 DB가 없어 실행 가능한 테스트 코드로 넣지 않고 실행 계약 초안만 남긴다.

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
| P1 | 전용 PostgreSQL DSN을 가진 Run에서 S-01·S-17 `spike-v2-read` 실제 test binary와 products `cargo test -p aip-cli --no-fail-fast` 실행. 별도 제품 `/read`·`/apply` 경로에서 default deny, hidden vs missing, visible+forbidden, SDK error preservation 최소 matrix 추가 여부 판단. | 낮음~중간 (환경 제공 시 테스트 실행은 낮음; 제품 최소 재현은 중간) | 아니오 |
| P1 | 실제 service `/read`에서 Ref traverse, traverseMany, aggregate, filter/sort와 secret fieldRead를 대상으로 hidden-only row 변형 비간섭 검증. | 중간 | 아니오 |
| P2 | role/tenant/ownership 변경이 진행 중인 apply/read와 겹칠 때 새 권한이 어느 요청부터 적용되는지, DB 격리 계약을 실제 경합 시험으로 측정. | 중간~높음 | 아니오, 현행 lock/isolation 경계 우선 확인 |
| P2 | worker/service actor와 end-user actor의 facts, audit actor, allow/rowRead 결과를 같은 operation으로 대조. | 중간 | 전용 service principal을 정의에 넣는 요구가 확인되는 경우에만 별도 검토 |

테스트용 DSN은 만들지 못했다. initdb 실패 외에 DB 접속을 시도하지 않았다. 본 문서는 `.env`, 키 또는 설정 secret을 읽지 않았다.

## 7. 실행 근거

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

`aip-cli` baseline과 v2 auth regressions가 테스트 assertion failure를 낸 것은 아니다. strict harness가 DB endpoint를 받지 못해 빌드/테스트 시작 전 중단된 것이며, 수정 전 RED 출력은 없다.
이번 후속 회차에서도 DB 초기화가 Codex nested command sandbox IPC 제한에서 중단돼 `psql SELECT 1`, 제품 baseline, S-01, S-17, S-14 test binary는 실행되지 않았다.
