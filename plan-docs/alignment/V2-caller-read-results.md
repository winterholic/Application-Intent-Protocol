# V2. 호출자 읽기 수직 slice 실험 결과

> 상태: 검증 필요 (2026-10-04, Codex r2b 반영). [E §4](E-technical-risks.md#4-작은-실험의-선후-관계) V2의 실행 기록이다. planner·와이어·SDK 설계 결정이 아니다.
> 위치: `../../spikes/spike-v2-read/`. 입력은 [V1](V1-semantic-fixture-results.md) typed facts뿐이며 본 `crates/`를 쓰지 않는다. 로컬 PostgreSQL 17의 `aip_v2_spike`·`aip_v2_r2b` schema를 테스트 시작에 만들고 끝에 지운다.

## 실제 실행 출력

명령: `cd spikes/spike-v2-read && cargo test --offline -q -- --nocapture`

```text
planner rejection cases: 19
db scenarios ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.48s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
```

두 번째 줄은 `tests/v2_r2b.rs`(Codex r2b 반례 재발 방지)다.

고장 주입 확인: (1) 관계 대상 행 정책 재적용과 필드 정책 CASE를 제거하면 `db_vertical_slice`가 실패했다. (2) 행 정책 없는 resource의 `denyAll`을 TRUE로 바꾸면 처음에는 통과했다. 노출 경로에 정책 없는 resource가 없었기 때문이다. Club 행 정책을 지운 변형 사례를 추가한 뒤 같은 고장이 실패로 잡혔다. 복원 후 4개 통과.

## 1. 흐름

```text
.aip/E/H 정의 → V1 typed facts(execution)
호출자 요청(JSON 트리) ─┐
                        ├→ plan_read: 허용목록·budget·값 타입 검증 → 매개변수 SQL 1개 + 출력 타입
facts ──────────────────┘
→ execute: READ ONLY 트랜잭션, statement_timeout = budget deadline, TimeZone UTC
```

planner는 facts만 읽는다. V1 AST·소스에 접근하지 않는다. 이 fixture와 r2b 변형(단독 Bool 조건, null 대입, guard 행별 집계) 범위에서 facts가 SQL 생성에 충분했다. V1이 받아들이는 모든 facts에 대한 보장은 아니다. 부족했던 점은 §4에 적었다.

호출자 요청 예:

```json
{ "read": "Recruitment",
  "select": ["id", "title", "periodEnd", "bookmarkCount", "internalNote",
             { "club": { "select": ["id", "name", "logo"] } }],
  "filter": [{ "field": "periodEnd", "op": "gte", "value": "2026-10-09T00:00:00Z" }],
  "sort": [{ "field": "periodEnd", "dir": "asc" }],
  "limit": 20 }
```

생성 SQL 구조: 행 정책은 WHERE, 필드 정책은 `CASE WHEN … THEN 열 END`, 관계는 대상 행 정책을 다시 건 `LEFT JOIN LATERAL`, 행별 집계는 상관 subquery `count(*)`. predicate(`active`, `managerOf`)는 인라인되고 `exists`는 `EXISTS (SELECT 1 …)`가 된다. 요청 값·enum 값·actor·현재 시각은 모두 `$n` 매개변수다.

## 2. 결과

| 확인 | 결과 | 관련 |
|---|---|---|
| 학교·익명·상태·기한 행 정책 | 회원 1·2(A대) {102,100}, 회원 3(B대) {102,101}, 학교 없는 회원·익명 {102} | EQ-02, RK-02 |
| 내부 메모 | 동아리 10 관리자에게 그 행만 값, 나머지 null. 비관리자는 전부 null | EQ-05 |
| 북마크 총수 | 다른 학교 회원 북마크까지 포함한 전체 count(100→3). 북마크 개인 행 직접 조회는 `NOT_EXPOSED` | EQ-06, RK-02 |
| 관계 + 대상 정책 재적용 | Club 행 정책을 좁힌 변형에서 안 보이는 Club은 null. 정책 없는 Club은 전부 null | EQ-04, V1-R1 |
| 열린/닫힌 capability | `club.logo` 제거·추가는 요청만 변경(통과). `club.school`, `status`는 `FIELD_NOT_EXPOSED`(서버 정의 변경 필요) | RK-01, SY-2 |
| select 권한 ≠ filter/sort | 선택 가능한 internalNote filter, title·bookmarkCount sort, 허용 안 된 연산 모두 거부 | SY-3 |
| 깊이·행·비용 | 관계 안 관계 `DEPTH_EXCEEDED`, limit 51 `ROWS_EXCEEDED`, cost 50 변형 `COST_EXCEEDED` | EQ-07, RK-03 |
| 기한 | 400,000행 부하에서 1ms 변형 `DEADLINE_EXCEEDED`(sqlstate 57014), 기본 2s는 성공 | RK-03 |
| 단독 집계 접근 | 관리자만 값. 비관리자·남의 동아리·없는 동아리·익명 모두 실행기(`execute`)가 같은 `ACCESS_DENIED` 반환 | EQ-10 중 집계 접근 부분. `stats` 확장 실행은 V4 |
| 값 주입 | Time 자리 SQL 조각·필드 이름 주입·집계 입력 `10 OR 1=1` 거부. 요청 값이 SQL 문자열에 없음을 단언 | RK-03 |
| 모르는 키 | 요청·filter의 모르는 키 `UNKNOWN_KEY` | SY-6 |
| 형식 무관 | A facts와 H-Py facts에서 같은 SQL·매개변수 | SY-1 |
| 불변식 DB 집행 | facts에서 만든 partial unique index가 같은 동아리 두 번째 게시를 23505로 거부, 초안은 허용 | EQ-09, V1-R4 |

## 3. spike가 정한 규칙

| ID | 규칙 | 열린 점 |
|---|---|---|
| V2-R1 | 필드 정책 불충족 값은 null. 출력 타입에 `redactable: true`, nullable 표시 | null이 "값 없음"과 "권한 없음"을 섞는다. 별도 표시·필드 생략·요청 거부 중 선택 필요 |
| V2-R2 | 안 보이는 관계 대상은 null, 출력 타입 nullable | 관계 필수 필드가 정책 때문에 null이 될 수 있음을 SDK 타입에 반영해야 함 |
| V2-R3 | 경로끼리 `=`/`!=`는 SQL 3치 논리. 한쪽이 NULL이면 UNKNOWN이고, 최종 WHERE·CASE에서 TRUE가 아니면 허용하지 않는다. `not (a = b)`도 UNKNOWN이라 허용되지 않는다. 명시적 `x = null`은 `IS NULL`로 따로 번역한다 | 익명 actor가 학교 행에 매칭되지 않는 근거. null-safe 비교를 택할지는 별도 의미 결정(R2B-12) |
| V2-R4 | limit 미지정은 budget rows, 초과는 잘라내지 않고 거부 | 조용한 절단보다 명시 거부를 택함 |
| V2-R5 | 항상 `id ASC`를 마지막 정렬 키로 추가 | 페이지 안정성. cursor 페이지네이션은 미구현 |
| V2-R6 | 비용 = limit × (1 + 관계 + 집계 + 필드 정책 수) | 자리표시 휴리스틱. 실제 DB 비용·index 유무와 무관 |
| V2-R7 | 단독 집계 guard 실패와 대상 없음은 같은 `ACCESS_DENIED` | 존재 추론 차단 방향 |
| V2-R8 | DB 오류 원문은 호출자에게 보내지 않고 sqlstate 분류만 | PoC의 `db.message()` 노출 부채와 반대 방향 |

## 4. 드러난 공백

- C의 단독 집계 노출(`expose aggregate approvedCount`)에는 budget이 없다. spike는 고정 2s를 썼다. 집계 노출에도 deadline·cost 위치가 필요하다.
- V1 facts는 필드 선언 순서를 보존하지 않는다(BTreeMap). DDL 열 순서가 알파벳순이 됐다. 의미에는 영향이 없지만 열 순서를 기대하는 SQL·seed는 깨진다.
- 같은 경로 subquery가 반복 생성된다(Club.school 2회). 성능·EXPLAIN은 측정하지 않았다.
- 비용 상한은 실행 전 휴리스틱과 statement_timeout 두 단계뿐이다. 실행 중 행 수 상한·출력 크기 상한·동시 요청은 미검증(RK-03 일부).
- `actor` 인증, 대리 접속, tenant는 없다. actor id를 직접 넣는다.

## 5. 하지 않은 것

- 와이어 형식·HTTP 서버·SDK·타입 생성 없음. 출력 타입은 JSON 서술까지만.
- cursor 페이지네이션, 집계 그룹화·호출자 집계 조건, 다단 관계, 역관계(1:N) 탐색 없음.
- 성능·index·동시성 측정 없음. 400,000행은 기한 시험용 부하다.
- 쓰기(V3)·worker(V4)·캐시(V5) 없음.

## 6. 독립 검토와 반영

[codex-v1v2-r2](../reviews/codex-v1v2-r2.md)는 V1 반례 3건 작성 뒤 OpenAI 콘텐츠 필터로 중단됐다. 표현을 정확성 검토로 바꾼 [codex-v2v3-r2b](../reviews/codex-v2v3-r2b.md)가 V2·V3-1을 실제 DB로 재현하며 검토했다(12건: P1 2, P2 8, P3 2). 판정은 "원본 fixture 범위에서 성립, 수용 facts 변형과 동시성에서 공백". 창시자 승인이 필요한 확정 충돌은 없다고 봤다.

| 발견 | 내용 | 반영 |
|---|---|---|
| R2B-01 P2 | ACCESS_DENIED는 테스트 안 변환이고 실행기는 `[null]` 성공 | Plan에 `null_is_denied`, 실행기가 `ACCESS_DENIED` 반환 |
| R2B-02 P2 | `rows read when true/false` 같은 단독 Bool 조건이 SQL 생성에서 INTERNAL | Bool 리터럴·Bool 경로 조건 지원(`IS TRUE`) |
| R2B-03 P2 | select·filter·sort 반복이 cost 밖, 중복 select는 DB 오류, 1MiB 출력 통과 | 중복 `DUPLICATE`. 출력 1MiB 상한 `OUTPUT_TOO_LARGE`(spike 고정값, budget 위치 미정) |
| R2B-05 P2 | 읽기 Bool filter 미지원 | 값 검사에 Bool 추가 |
| R2B-08 P1 | 행별 집계가 guard sourceAccess를 집행하지 않음 | guard면 `CASE WHEN guard THEN count END`, 출력 타입 redactable. 비관리자 null·관리자 count·totalOfVisible 불변 테스트 |
| R2B-09 P2 | null 값 SQL 생성 미지원 | `NULL` 생성 |
| R2B-12 P3 | V2-R3 설명 부정확 | §3 V2-R3 문구 정정 |

반영 뒤 `tests/v2_r2b.rs`로 고정했고, R2B-08 수정을 되돌리면 이 테스트가 실패함을 확인했다. 쓰기 쪽 발견은 [V3 결과](V3-standard-write-results.md)에 있다.

## 변경 이력

- 2026-10-04 V2 spike 실행 결과 초안.
- 2026-10-04 Codex r2b 반영: 실행기 거부 코드, Bool·null, 요청 중복·출력 상한, 행별 집계 guard 집행, 주장 범위 축소.

- 2026-10-04 후속 [V15](V15-filter-value-results.md)는 Url·Enum·Time read/where 공통 검사와 읽기 Text.prefix를 연결했다. NUL 선거부 범위는 read/where이며 create/compose·공식 패키지 통합은 남는다.
