# P1 sema 구현 노트 (진단 문구, 정책 작성 경고, Decimal sum)

기준: `feat/expr-p0b-arith-20261121` 16819e3c에서 갈라진 `feat/expr-p1-sema-20261121`. 결정은 [decisions.md](decisions.md) "P1 후보 분류"(Decimal `sum` 1.0, 정책 실수 감지 1.0)다. 소비자 앱 정의와 마찰 기록은 `test/expr-consumer-apps-20261121`의 `consumer-apps.md`(F4, F10, P-3)를 근거로 했다.

P0-D(bundle 안 compose)와 나중에 합칠 때를 위해 `spike-v3-write/src/lib.rs`, `spike-v6-transport/src/lib.rs`, `spike-v5-sdk/sdk/generic.ts`는 건드리지 않았다. v6에서는 오류 코드 문서 문자열(`wire_codes.rs` 한 줄)만 고쳤다.

## 1. `DEF.ACCESS_MISMATCH` 문구

| | 내용 |
| --- | --- |
| 실제 판정 | `access x = totalOfVisible(P)`를 aggregate에 쓸 때 `P`가 집계를 선언한 resource(`r`)와 다르거나 aggregate에 `groupKey`가 없으면 오류(sema.rs `aggregate`, `p != r.name \|\| a.group_key.is_none()`) |
| 이전 문구 | `` `visibleTotal`은 `Comment` 기준 groupKey 집계에만 사용 ``. 인자 `Comment`를 "기준"으로 읽게 해 올바른 인자(`Task`)와 반대로 읽혔다 |
| 새 문구 | `` `visibleTotal`은 `Task`를 참조하는 `Comment`의 groupKey 집계에서만 사용(인자가 `Comment`이지만 집계를 선언한 `Task`여야 함) ``. 첫 문장은 제안 문구와 같고, 괄호에 실제 틀린 이유(인자 불일치, groupKey 없음)를 이유별로 붙인다 |
| 레지스트리 | `codes.rs` 설명도 "group key를 가리키는 access"라는 틀린 서술을 `totalOfVisible(Task)`(집계를 선언한 쪽)로 고쳤다. `spec/definition-diagnostics.md` 재생성(`AIP_UPDATE_GOLDEN=1`) |
| 시험 | `tests/p1_sema.rs` `access_mismatch_says_what_the_judgement_is`, `the_registry_describes_the_actual_judgement` |

## 2. 정책 작성 경고 3종 (조언만, 정의를 거부하지 않는다)

구현은 `spike-v1-fixture/src/policy_advice.rs`다. 정규화된 facts(술어 호출을 인라인하고 `=`의 피연산자 순서를 고정)만 읽으므로 facts·digest·계약 지문은 바뀌지 않는다. `spike-v11-dev-checks`의 `ALWAYS_ON`에 넣어 `--dev-checks` 없이도 `service check`에 나온다(테넌트 누락은 노출이라 `DEF.ADVISORY_PUBLIC_COUNT`와 같이 취급했다. 기존 `ADVISORY_EFFECT_NO_LOWER_BOUND` 등은 여전히 dev-checks 전용이다).

| 코드 | 신호 | 위치 |
| --- | --- | --- |
| `DEF.ADVISORY_TENANT_UNSCOPED_READ` | 테넌트 필드가 있는 resource의 `rows read`가 그 필드를 `actor.<필드>`와 견주지 않음(`or` 가지마다 필요) | resource |
| `DEF.ADVISORY_PUBLIC_READ_TENANT_FIELD` | `rows read when true`인데 테넌트 필드가 있음 | resource |
| `DEF.ADVISORY_WRITE_WIDER_THAN_READ` | (1) `expose create` allow가 `rows read`의 AND 항을 요구하지 않음 (2) 공개된 전이의 `patch`가 테넌트 필드를 바꿀 수 있고 테넌트 resource가 `actor.<필드>`로 한정되지 않음 | create 또는 transition |

### 판정 규칙과 오탐을 줄인 장치

- **테넌트 필드** = actor resource가 가진 참조 필드와 이름·대상이 같은 참조 필드(`Member.org: Org`와 `Task.org: Org`). 그리고 이 정의의 어떤 정책이 그 쌍을 실제로 actor와 묶고 있어야 한다(다른 resource의 `rows read when org = actor.org`, 또는 테넌트 resource의 `rows read when id = actor.org`). 이 "관례 증거" 조건이 없으면 flaskbb의 `primary_group: Group`, `UserGroup.group`, `ForumGroupAccess.group`이 모두 공개 읽기 경고를 받는다(권한 그룹이지 테넌트가 아니다. 먼저 이 단순 규칙으로 돌려 세 건이 나온 것을 보고 조건을 넣었다).
- 행을 caller에 묶는 `owner = actor`, `id = actor.id`도 "묶임"으로 본다(단일 소유자 앱이 경고받지 않게).
- `exists`가 든 정책은 판정하지 않는다(멤버십 표로 테넌트를 표현하는 경우를 구조 비교로 따라갈 수 없다).
- (2)의 create 판정에서 다음 항은 보고하지 않는다. ① 호출자가 `expose create fields`로 값을 정하지 않는 항(DB·서버가 채우거나 caller 자신만 읽는 항). ② 참조 가시성이 이미 보장하는 항: 생성 값의 참조는 호출자가 읽을 수 있어야 하므로(`ensure_refs_readable`, v3 lib.rs:1124) `org = actor.org`는 `Org`의 `rows read`가 `id = actor.org`면 보장되고, `video.is_deleted = false`는 `Video`의 `rows read`에 같은 항이 있으면 보장된다.
- (2)의 patch 판정도 같은 이유로 테넌트 resource가 `actor.<필드>`로 한정되면(`Org`가 `id = actor.org`) 경고하지 않는다. patch 값은 `rows read`로 다시 확인하지 않지만(`judge`는 사전 상태로만 본다) 참조 가시성은 확인한다.

### 알려진 한계 (놓치는 것)

- 모든 정책에서 테넌트 조건을 한꺼번에 뺀 정의는 관례 증거가 없어 경고가 없다. 이 경우 AIP는 테넌트 개념이 없는 정의로 본다.
- 필드 이름이 달라도(`organization`과 `org`) 같은 쌍으로 보지 않는다. 대상은 같고 이름만 다른 쌍을 잡으려면 오탐이 늘었다(위 flaskbb).
- `exists` 정책, 술어 호출이 순환·미해결인 정책은 판정하지 않는다.
- allow와 `rows read`가 같은 뜻을 다르게 쓰면(예: `a != b` 대 `not a = b`) 구조 비교라 (2)가 오탐을 낼 수 있다.

### 기존 정의에서의 분류 (V-1 대상 63개)

대상: 저장소의 `.aip`·`.e/.h.ts·py` 60개(plan-docs fixtures 19, spikes 24, product·porting·prototype 17)와 소비자 앱 정의 3개(`workspace.aip`, `community.aip`, `orders.current.aip`). `service check` 출력에서 새 경고가 난 정의는 **2개, 경고 3건**이고 나머지 61개는 경고 변화가 없다. `ADVISORY_TENANT_UNSCOPED_READ`와 `ADVISORY_PUBLIC_READ_TENANT_FIELD`는 63개 모두에서 0건이다.

| 정의 | 경고 | 분류 | 근거 |
| --- | --- | --- | --- |
| `porting/flaskbb/app.aip:716` `Message` create | WIDER: allow가 `conversation.user = actor`를 요구하지 않음 | **의도(경고 수용)** | 주석이 밝히듯 보내는 사람이 상대 우편함 행에 메시지를 쓴다(allow의 `conversation.user = from_user or to_user`). 보낸 사람이 상대 우편함 메시지를 읽지 못하는 것이 설계다 |
| `facereview-mysql/facereview.aip:265` `Video` create | WIDER: allow가 `is_deleted = false`를 요구하지 않음 | **실제(낮은 위험)** | `is_deleted`가 `expose create fields`에 있어 관리자가 삭제된 Video를 만들 수 있고 그 행은 만든 관리자에게도 안 보인다. 관리자 전용이라 위험은 낮다 |
| `facereview-mysql/facereview.aip:417` `Comment` create | WIDER: allow가 `is_deleted = false ...` 읽기 조건을 요구하지 않음 | **실제(정의 가정 노출)** | `is_deleted`는 `Bool?`에 기본값 표시가 없고 `fields`에도 없어 정의상 NULL로 생성되며 비관리자에게는 안 보인다. 운영 DB의 기본값(db-defaults.sql)이 메우는 가정에 기댄다. `is_deleted: Bool default database`로 적으면 사라지는 사안(`facereview-comments`는 그렇게 적어 경고가 없다) |

오탐으로 분류한 건은 없다. `facereview-comments`의 `video.is_deleted = false`는 처음에 경고가 났으나 `Video`의 `rows read`에 같은 항이 있어 참조 가시성이 보장하는 항이므로 "참조 가시성" 면제를 넣었다(그때가 유일한 오탐이었다).

### 소비자 앱 변형 (테넌트 조건을 뺀 정의)

`test/expr-consumer-apps-20261121`의 `saas/workspace.aip`(원본은 경고 0)를 한 곳씩 고쳐 `aip service check`로 확인했다.

| 변형 | 결과 |
| --- | --- |
| Task `rows read when org = actor.org`를 `status != DONE`으로 | TENANT_UNSCOPED_READ(Task) + WRITE_WIDER(Task create, 읽기 조건 `status != DONE` 누락) |
| Task `rows read when true` | PUBLIC_READ_TENANT_FIELD(Task) |
| Comment `rows read when body != ""` | TENANT_UNSCOPED_READ + WRITE_WIDER (Comment) |
| User `rows read when true` | PUBLIC_READ_TENANT_FIELD(User) |
| Task create allow에서 `org = actor.org`, `project.org = actor.org` 제거 (`Org`는 `id = actor.org` 유지) | 경고 없음. 생성 값의 `org`는 읽을 수 있는 `Org`여야 하므로 막힌다(consumer-apps 4절 "다른 조직 프로젝트로 생성"이 `MISSING_TARGET`) |
| 위와 같고 `Org`를 `rows read when true`로 | WRITE_WIDER(Task create) |
| Task `edit`가 `patch org`를 받음 (`Org`는 `id = actor.org`) | 경고 없음(참조 가시성이 막음) |
| 위와 같고 `Org`를 `rows read when true`로 | WRITE_WIDER(transition `edit`, 테넌트 필드를 patch로 바꿀 수 있음) |

변형 정의는 다른 브랜치의 파일이라 이 저장소에 두지 않았고, 같은 변형을 `tests/p1_sema.rs`의 작은 앱(`APP`)으로 고정했다.

### 시험 (`spike-v1-fixture/tests/p1_sema.rs`, `spike-v11-dev-checks/tests/policy_advice.rs`)

각 시험은 규칙을 하나씩 껐을 때 실패하는 것을 확인했다(RED).

| 끈 규칙 | 실패한 시험 수 |
| --- | --- |
| 테넌트 묶임 판정을 항상 통과 | 2 |
| 공개 읽기 경고 끔 | 2 |
| create 경고 끔 | 3 |
| patch 경고 끔 | 1 |
| 참조 가시성 면제 끔 | 2 |
| "호출자가 정하는 항" 조건 끔 | 1 |
| 관례 증거 조건 끔 | 1 |
| `ACCESS_MISMATCH` 이전 문구 | 1 |
| v11 `ALWAYS_ON`에서 새 코드 제외 | 1 |

## 3. Decimal `sum`

| 층 | 변경 |
| --- | --- |
| sema | `release sum(f)`가 Decimal(p,s) 필드를 받는다. 집계 타입은 null이 아닌 `Decimal(p', s)`이고 s는 같아야 하며 p' >= p다(소수 자릿수가 다르면 반올림이 필요하고 p' < p는 정의만으로 넘칠 수 있어 정의 단계에서 `DEF.TYPE_MISMATCH`). `avg`는 `DEF.UNSUPPORTED` 그대로, `min`·`max`는 Int 필드만(`DEF.TYPE_MISMATCH`). 암호화 필드 집계는 `DEF.UNSUPPORTED`(암호화는 Decimal에만 가능해 Int 때는 필요 없던 검사). 필드 읽기 정책 필드는 기존대로 `DEF.POLICY_FIELD_NOT_AGGREGATABLE` |
| facts | 새 키 없음. 집계 `ty`가 `Decimal<p',s>`로 나올 뿐이라 기존 정의의 facts·지문은 같다 |
| v2 플래너(PG) | `measure`가 `coalesce(sum(col) FILTER (WHERE <원본 rows read>), 0)`을 내고(Int와 같은 `rows read` 적용, `::bigint` 없음), `measured_subquery`가 Decimal이면 `CASE WHEN abs(v) >= 10^(p'-s) THEN v/(v-v) ELSE v::numeric(p',s) END`로 감싼다. 합이 안 들어가면 SQLSTATE 22012가 나고 `rows_of`가 `WIRE.OUTPUT_INVALID`로 바꾼다. 22003(캐스트 오류)을 쓰지 않은 이유는 Int sum의 bigint 초과(`WIRE.BAD_VALUE`, 동작 유지)와 구분이 안 되고 메시지가 번역될 수 있어서다. 이 읽기 계획 어디에도 다른 나눗셈은 없다. 응답은 `numeric::text`라 JSON 문자열이다("0.00"처럼 자릿수 유지). 단독 집계(groupKey 없음, guard access) 경로는 결과 한 개를 `serde_json::from_str`로 읽어서 `50.70`이 숫자 50.7로 깨지는 결함이 시험(RED)에서 나왔고, Decimal 합계는 `to_json(value::text)`로 문자열로 내보내게 고쳤다 |
| MySQL | `WIRE.UNSUPPORTED`로 거절(`measure`의 Decimal sum 분기). MySQL 읽기는 Decimal 합계 정규화가 없다. 이번 범위 밖 |
| SDK | 코드 변경 없음. 생성 계약은 aggregate `ty`로 타입을 정하므로 `total: string`과 읽기 descriptor `Decimal<14,2>`가 이미 나온다. p'는 계약 지문에 반영된다(`spike-v5-sdk/tests/decimal_sum_codegen.rs`) |
| 문서 | `wire_codes.rs`의 `WIRE.OUTPUT_INVALID` 설명(→ `spec/wire-errors.md` 재생성), `syntax_card.rs` `release` 행(→ `spec/definition-syntax.md` 재생성) |

실제 PG 17.11 시험(`spike-v2-read/tests/decimal_sum.rs`): 0.10+0.20+100.50-50.10 = `"50.70"`(이진 부동소수라면 틀릴 값), 빈 집합 `"0.00"`, 숨겨진 행(1000.00)은 합에서 빠지고 `count`는 그대로 5(공개 count의 비상속은 바꾸지 않았다), `Decimal(12,2)` 최댓값 `9999999999.99`는 `"9999999999.99"`로 나오고 0.01을 더하면 `WIRE.OUTPUT_INVALID`(DB 세부 없음), 같은 데이터가 `Decimal(14,2)`에는 `"10000000000.00"`, 숨겨진 큰 행만 있으면 넘치지 않음, Int sum은 JSON 숫자 그대로. 단독 집계도 `"50.70"`(문자열)로 나온다. RED: `FILTER` 제거(합이 `1050.70`), 비교를 `>=`에서 `>`로(경계값 10^10이 `WIRE.BAD_VALUE`), 22012 매핑 제거(`WIRE.BAD_VALUE`), 단독 집계의 문자열화 제거(`Number(50.7)`) 모두 이 시험만 실패했다.

## 4. V-1 (기존 정의 facts·계약 지문·digest 불변)

기준 바이너리(16819e3c)와 새 바이너리로 위 63개 정의에 `aip service check`와 `aip service gen`(ts, python)을 돌려 출력을 `diff -r`했다.

- 수락되는 정의 전부: `executionDigest`, `workers`, 생성된 ts·python 바인딩과 계약 지문이 같다(차이 0).
- 달라진 출력은 이것뿐이다. 모두 오류가 아니라 경고 추가이거나 이번 묶음이 일부러 바꾼 문구다.

| 정의 | 차이 |
| --- | --- |
| `porting/flaskbb/app.aip` | 경고 +1: `DEF.ADVISORY_WRITE_WIDER_THAN_READ`(`Message` create) |
| `product/tests/fixtures/facereview-mysql/facereview.aip` | 경고 +2: `DEF.ADVISORY_WRITE_WIDER_THAN_READ`(`Video`, `Comment` create). `ok: true`, 종료 코드 0 |
| `plan-docs/expressiveness-1.0/fixtures/s07a_decimal_sum.aip` | 기준에서도 거부되던 정의. 오류 3개(`TYPE_MISMATCH` 2, `ACCESS_MISMATCH`)에서 `ACCESS_MISMATCH` 하나로 줄었다(Decimal sum이 열려 타입 오류 둘이 사라짐). 남은 하나는 groupKey 없는 집계에 `totalOfVisible`을 쓴 것으로 새 문구로 나온다 |
| `facereview-mysql` python 생성 | 기준과 같은 `python.rs:56` panic이며 로그의 스레드 번호만 다르다(이 묶음 이전부터) |

기존 시험이 새 경고를 고정한 곳 하나를 고쳤다: `spike-v1-fixture/tests/advisories.rs`의 FaceReview 시험이 "공개 count 세 건뿐"이라고 단정하고 있어 WIDER 두 건을 목록에 더했다.

## 5. 검증 명령(2026-11-21, `CARGO_TARGET_DIR=/tmp/aip-target-p1sema`, PG 17.11 전용 컨테이너 `aip-p1sema-20261121-pg` 포트 59317, DB `aip_test_p1sema`)

| 명령 | 결과 |
| --- | --- |
| `cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml`(전체) | 통과 245, 실패 0 |
| 같은 crate `--test diagnostics`, `--test p1_sema`, `--test advisories` | 29, 18, 8 통과 |
| `cargo test --manifest-path spikes/spike-v11-dev-checks/Cargo.toml` | 통과 16, 실패 0 |
| spike-v2-read `--test aggregate_measures aggregate_row_policy traverse_aggregate decimal_scalar decimal_sum v2 audit_fixes`(PG) | 통과 13, 실패 0 |
| spike-v5-sdk `--test decimal_sum_codegen` | 통과 2 |
| spike-v6-transport `--test wire_codes` | 통과 9 |
| `cargo test -p aip-cli --test extensions` / `--test service_flow service_check` | 통과 9 / 통과 1 |

## 6. 이 묶음 밖에서 본 것 (고치지 않음)

- `s07a_decimal_sum.aip` 같은 groupKey 없는 Decimal 합은 `totalOfVisible`로는 여전히 거부된다(`access allowAll(a: Member) = true`처럼 guard access를 써야 한다). guard access를 쓴 단독 Decimal 합은 PG 시험에 포함했다(`decimal_sum.rs`의 `allTotal`).
- HTTP(`aip service serve`)·TS/Python SDK 클라이언트를 거친 Decimal sum 종단 시험은 하지 않았다. v2 계획+PG와 생성 계약 시험까지만 했다(`미실행`).
- 공개 `count`의 `rows read` 비상속은 바꾸지 않았다(소유자 결정 대기).
- MySQL의 Decimal sum은 `WIRE.UNSUPPORTED`다. 필요하면 `mysql_rows.rs`에 Decimal 정규화와 초과 검사를 더해야 한다.
- 합칠 때 겹칠 곳: `spec/wire-errors.md`(한 항목), `spec/definition-*.md`(생성 문서이므로 재생성). P0-D가 건드리는 v3·v6 `lib.rs`와는 겹치지 않는다.
