# 2026-10-07 패턴 커버리지·보안 점검

> 상태: 초안. 제품 경로(`aip service`, spikes v1~v7)의 "정의 문법만으로 조립되는 패턴" 범위와 보안·안정성을 실행으로 점검한 기록이다. 다른 세션의 제품 통합 작업과 별개 관점이다. 수치는 감사 시점 값이다.

## 1. 점검 방법

- 일반 웹 백엔드 패턴 72개를 제품 정의 문법(`spikes/spike-v1-fixture`)으로 직접 작성해 `aip service check`로 검사하고, 상당수는 로컬 PostgreSQL에서 v2 read·v3 apply로 실행했다.
- 인증(JWT·JWKS·principal), read 권한 우회(select/filter/sort/traverse/aggregate), SQL 생성, 입력 상한·DoS, 멱등·동시성, 운영 누설·CORS·loopback·worker 격리를 프로브로 실행했다. 프로브는 저장소 밖에 두었고 임시 schema·role은 모두 지웠다.
- 각 수정은 먼저 실패하는 테스트로 결함을 재현한 뒤 고쳤다.

## 2. 이번에 고친 결함

| 구분 | 결함(재현) | 수정 | 회귀 테스트 |
|---|---|---|---|
| 권한 | `field X read when` 정책 필드를 filter/sort 허용 목록에 넣으면 sema가 받아들임. 비관리자도 filter 결과 유무·정렬 순서로 가려진 값을 추론 | sema `POLICY_FIELD_NOT_FILTERABLE` 진단, 계획 단계(plan.rs)에서도 같은 거부 | `spike-v1-fixture/tests/sema_fixes.rs`, `spike-v2-read/tests/audit_fixes.rs` |
| 권한 | `exists R where team = this.team`에서 `this`가 안쪽 행으로 재바인딩돼 조건이 항상 참. check/allow가 조용히 무력화 | `THIS_IN_EXISTS` 진단. 바깥 행 값은 predicate 인자로 넘긴다 | `sema_fixes.rs` |
| 정합성 | `limit … atMost 2`(또는 시간·actor 의존 조건)가 check는 통과하고 `service init`에서 `UNSUPPORTED_SCHEMA`. v3도 이 형태를 집행하지 않음 | sema `UNSUPPORTED_INVARIANT`로 check 단계에서 거부 | `sema_fixes.rs`, `spike-v7-migrate/tests/v7.rs` |
| 정합성 | `Int(0..10)` 범위가 선언만 되고 DDL CHECK가 없어 11·-1 저장 | Int 범위도 `CHECK (col BETWEEN lo AND hi)`로 생성 | `audit_fixes.rs` |
| DoS | `/apply` `target.ids`의 O(n²) 중복 검사가 bulk 상한 검사보다 먼저 돎. 인증 없이 12만 id로 debug 32초 CPU, 요청 기한이 끊지 못함 | 상한 검사를 먼저 수행 | `spike-v3-write/tests/v3_1.rs` |
| DoS | apply `where`의 같은 조건 반복에 상한 없음 | `DUPLICATE_FILTER`로 거부(조건 수 ≤ 허용 목록 크기) | `v3_1.rs` |
| 누설 | 멱등 key에 NUL이 있으면 `INTERNAL "멱등 잠금 실패(sqlstate 22021)"` | 제어 문자 key는 `/apply`·`/status`·WRITE 확장에서 DB 전에 `BAD_REQUEST` | `spike-v6-transport/tests/key_validation.rs` |
| 오류 분류 | DB CHECK/NOT NULL 위반(23514/23502)이 `INTERNAL`+sqlstate | `BAD_VALUE`로 분류 | 코드 경로(spike-v3-write `write_err`) |
| 안정성 | 요청 task 하나의 panic이 accept 루프 전체를 종료(서비스 중단) | panic은 기록하고 listener 유지. 저장된 멱등 결과 파싱 실패도 panic 대신 `INTERNAL` | 도달 가능한 panic은 찾지 못함 |

## 3. 이번에 채운 패턴

| 패턴 | 정의 | 요청 | 안전 규칙 |
|---|---|---|---|
| 외래키로 거르기("이 글의 댓글") | `filter post.eq` | `{field:"post", op:"eq", value:"7"}` | 값은 Id wire 규칙. rows read 정책 뒤에 AND |
| 값 목록(IN) | `filter status.in` | `value: ["OPEN","HIDDEN"]` | Text·Enum·Id·Ref만. 최대 50개, 빈 배열 거부, 같은 값은 한 번 바인딩 |
| null 여부 | `filter parent.isNull` | `value: true` | nullable 필드만. 정책 필드는 거부 |

SDK 생성 타입은 `in`에 배열, `isNull`에 bool을 요구한다(`spike-v5-sdk/sdk/filter-op-types.ts`).

## 4. 남은 패턴 빈칸 (감사 시점)

72개 중 직접 표현 약 33, 우회 15, 불가 14, 부분 6이었다(이번 수정 전). 불가·부분 중 흔한 것을 우선순위로 적는다. 쓰기 조합(W0/W1/W2)과 최종 WRITE API는 [OPEN]이라 이 문서가 정하지 않는다.

| 우선 | 패턴 | 현재 | 비고 |
|---|---|---|---|
| 1 | 호출자가 값을 보내는 생성(글 작성) | 정의 `expose create`는 check 통과, 제품 HTTP `/apply`는 전이만 받음(`UNKNOWN_KEY create`). `bundle`은 Time·Url 값을 `BAD_VALUE` | 쓰기 조합 결정과 묶여 있어 [OPEN]. 결정 전까지 기본 CRUD의 C가 제품 경로에 없음 |
| 1 | 호출자 값 수정(patch), 삭제 | 전이는 상수 대입만, delete 없음 | 같은 [OPEN] |
| 2 | 카운터·재고 증감 | 산술식 없음(`to stock = stock - 1` 불가) | `check stock >= 0`은 표현됨. 서버 정의 전이 안의 산술은 호출자 권한을 넓히지 않음 |
| 2 | sum/avg/min/max, 임의 group by | count만 | 대시보드 통계 |
| 2 | offset 페이지네이션, cursor의 동률 처리 | offset 불가, cursor는 id gt/lt 불가로 동률 누락 | 목록 화면 기본 |
| 3 | 기본값, 계산 필드, Email 형식 | 없음 | root PoC 문법에는 일부 존재 |
| 3 | 정원 N(atMost ≥ 2) | 이번에 check에서 명시 거부로 바꿈. 집행 기능은 없음 | 잠금 기반 count 검사 필요 |
| 3 | 상수 access guard(`= true`) | check 통과 후 실행 시 `42P18` | 미사용 바인드 매개변수 타입 추론으로 추정, 확인 필요 |
| 4 | webhook, rate limit, 스케줄 실행 | 없음 | 운영 기능 |

## 5. 보안 관찰 중 고치지 않은 것

| 관찰 | 이유 |
|---|---|
| JWT `aud` 배열 안에 설정 audience가 있으면 수용 | RFC 7519 허용 형태. OPERATIONS.md "정확한 audience" 해석을 명시할지 판단 필요 |
| 유휴 연결 64개를 반복 유지하면 정상 요청 0/10 성공, IP별 상한 없음 | 제품은 loopback + reverse proxy 전제라 IP 단위 제한은 proxy 몫. OPERATIONS.md에 proxy의 연결·헤더 시간 제한 요구를 적을지 판단 필요 |
| `aip_idem` 보관·정리 정책 없음. 익명 호출이 행당 최대 1MiB 기록, 익명 전체가 한 namespace 공유 | 보관 기간·익명 쓰기 정책은 운영 결정 |
| 모든 응답이 HTTP 200, 오류는 JSON code | SDK 계약과 묶인 결정 |
| worker 확장이 detached로 만든 손자 프로세스는 기한 뒤에도 남음 | 확장 코드는 신뢰 경계 안. 격리 범위는 문서상 네트워크 차단만 |
| JWKS 갱신 실패 시 캐시 키도 쓰지 않고 5초 fail-closed, 갱신 중 mutex 대기 | 의도된 fail-closed로 보임. HTTPS 실행 확인 못함 |

인증(alg none·HS256 혼동·kid·typ·시각 경계·폐기 같은 초), actor 위조(헤더 8종·body·claims), SQL 매개변수화·prefix 리터럴 처리, JSON·본문·헤더 상한, CORS 정확 일치, loopback 강제, 로그·응답의 비밀 누설은 문제를 찾지 못했다.

## 변경 이력

- 2026-10-07: 초안. 패턴 72개·보안 6영역 점검, 결함 9건 수정, 읽기 필터 3종 추가.
