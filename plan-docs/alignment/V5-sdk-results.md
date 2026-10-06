# V5. 프론트 SDK 실험 결과

> 상태: 검증 필요 (2026-10-04). [E §4](E-technical-risks.md#4-작은-실험의-선후-관계) V5-1(결과 타입 추론), V5-2(읽기 의존 태그의 건전성), V5-3(캐시 런타임) 실행 기록이다. SDK API·생성 방식 결정이 아니다.
> 위치: `../../spikes/spike-v5-sdk/`. V1 facts에서 TS 계약 타입을 생성하고, `spikes/spike-0-ts`에 설치된 TypeScript 7 `tsc`로 검사한다. V5-2는 로컬 PostgreSQL의 `aip_v5` schema를 쓰고 지운다. 서버·전송 계층은 없다.

## 실제 실행 출력

명령: `cd spikes/spike-v5-sdk && cargo test --offline -q -- --nocapture`

```text
tsc type-tests ok; type-neg 실패 확인:
type-neg.ts(9,24): error TS2344: Type 'false' does not satisfy the constraint 'true'.
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.18s
```

`type-neg.ts`는 일부러 틀린 기대 타입(`title: number`)을 둔 음성 대조다. 이 파일이 실패해야 타입 검사가 공허하지 않다.

## 1. 구조

```text
V1 facts → contract.ts(생성) ─┐
                              ├→ aip.ts(SDK 타입 계층) → 화면 코드의 aip.read({...})
                              │     결과 행 타입 = select에서 추론
```

`contract.ts`는 호출자 읽기 계약만 담는다: resource별 공개 필드와 출력 타입, 관계와 관계 대상의 공개 필드, filter·sort 허용 목록, 루트 조회 가능 여부, 행 상한. 서버 정책 식은 내보내지 않는다.

```ts
const rows = await aip.read({
  read: "Recruitment",
  select: ["id", "title", "internalNote", "bookmarkCount", { club: { select: ["name", "logo"] } }],
  filter: [{ field: "periodEnd", op: "gte", value: "2026-10-09T00:00:00Z" }],
  sort: [{ field: "periodEnd", dir: "asc" }],
});
// rows[number]: { id: Id<"Recruitment">; title: string; internalNote: string | null;
//                 bookmarkCount: number; club: { name: string; logo: string | null } | null }
```

## 2. 결과

| 확인 | 결과 |
|---|---|
| select에서 결과 행 타입 추론(관계 안 select 포함) | 정확히 일치 |
| 화면이 관계 필드 일부를 빼면 결과 타입에서도 빠짐 | 일치(서버 정의 변경 없음) |
| 정책으로 가려질 수 있는 필드(internalNote)·관계 | `| null` 포함 |
| 닫힌 필드(status), 관계 대상의 닫힌 필드(club.school), 계약에 없는 관계 | 컴파일 오류 |
| 허용 안 된 filter 연산, select 가능하지만 filter 불가 필드, 정렬 허용 밖 | 컴파일 오류 |
| filter 값 타입(Time 자리에 숫자) | 컴파일 오류 |
| 관계 대상 전용 resource(Club) 루트 조회 | 컴파일 오류 |

8개 거부 사례는 `// @ts-expect-error`로 두었다. 오류가 나지 않으면 tsc가 "사용되지 않은 지시문"으로 실패하므로 거부가 실제로 일어난다는 뜻이다.

## 3. 드러난 것

- 서버가 런타임에 거부하는 것(V2)의 상당 부분이 화면 코드 작성 시점 타입 오류로 앞당겨진다. 런타임 검사는 여전히 필수다(SDK 우회 요청).
- 처음 구현은 관계 안 select를 리터럴로 추론하지 못해 허용 필드 전체로 넓혀졌다. 관계 key를 `infer`로 뽑는 대신 요청 객체의 key를 순회하도록 바꿔 해결했다. 타입 계층은 TS 버전·추론 규칙에 민감하므로 tsc 검사를 테스트로 고정해야 한다.
- 계약 타입에 정책 필드 이름(internalNote)이 모든 호출자에게 보인다. 역할별 계약 투영과 필드 존재 비노출은 하지 않았다(C B.1의 열린 점, V2-R1과 연결).
- 행 상한(maxRows)은 타입에 담았지만 limit 값 검사는 타입으로 하지 않는다.

## 4. V5-2 읽기 의존 태그 (RK-09)

V2 읽기 계획에 `deps`를 더했다. 생성 SQL이 참조하는 테이블을 resource 이름으로 되돌린 집합이다. 정책 하위조회(managerOf의 ClubMember), 관계 대상, 집계 원본, actor 경로(Member)가 모두 들어간다. SDK 캐시는 쓰기가 바꾼 resource와 deps가 겹치는 조회를 무효화하는 후보다.

`tests/v5_2_deps.rs`: 모집 목록 조회(actor 1)에 대해 resource 하나씩만 바꾸는 쓰기를 하고, 결과가 바뀌면 그 resource가 deps에 있어야 한다.

```text
deps = ["Club", "ClubMember", "Member", "Recruitment", "RecruitmentBookmark"]
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.11s
```

| 바꾼 resource | 결과 변화 | deps 포함 |
|---|---|---|
| Recruitment(제목) | 예 | 예 |
| Club(logo) | 예 | 예 |
| RecruitmentBookmark(추가) | 예(bookmarkCount) | 예 |
| ClubMember(관리자 회수) | 예(internalNote가 null로) | 예 |
| Member(actor 학교 변경) | 예(보이는 행 변화) | 예 |
| School(이름) | 아니오 | 아니오 |
| Apply | 아니오 | 아니오 |
| MemberAlarm | 아니오 | 아니오 |

고장 주입: deps를 루트 resource만으로 줄이면 Club·북마크·ClubMember·Member 변경 4건이 "결과가 바뀌었는데 deps에 없음"으로 실패했다.

관찰:
- **권한 변경도 캐시 무효화 대상이다.** 관리자 권한이 회수되면 캐시에 남은 internalNote가 그대로 보일 수 있다. 화면에 보이는 데이터만 태그로 삼으면 놓친다. 정책이 읽는 resource를 deps에 넣어야 한다.
- 태그 단위가 resource라서 넓다. 다른 회원의 학교만 바뀌어도(Member) 이 목록이 무효화된다. 안전한 과다 무효화다. 행 단위 정밀화는 하지 않았다.
- 같은 쿼리라도 actor마다 결과가 다르다(V2). 캐시 키에 actor가 들어가야 하고 로그아웃·계정 전환 때 비워야 한다. 이번 테스트는 한 actor만 봤다.
- 쓰기 결과가 `COMMIT_UNKNOWN`이거나 응답이 유실되면 바뀐 resource를 모른다. 그때는 전체 무효화가 안전한 기본값 후보다(미구현).

## 5. Codex r4b 반영

[codex-v345-r4b](../reviews/codex-v345-r4b.md)의 SDK·deps 발견:

| 발견 | 내용 | 반영 |
|---|---|---|
| F01 P1 | `select: [cond ? "title" : "views"]`가 두 필드를 모두 가진 행으로 추론됨 | 고정 tuple은 항목별로 교집합, 항목이 union이면 결과도 union. 길이를 모르는 배열은 모든 필드 선택적. 타입 테스트 추가 |
| F02 P2 | 빈 select, 관계 항목의 키 두 개가 타입 검사를 통과 | `CheckSel`로 컴파일 오류 |
| F03 P1 | select에 없고 filter만 열린 필드의 값 타입이 never | 계약에 `filterFields`를 따로 생성. 변형 계약으로 tsc 확인 |
| F04 P2 | 응답 Id는 숫자, 요청 Id는 문자열만 받음 | V2가 Id filter 값으로 숫자·숫자 문자열 모두 받음, SDK 입력 타입도 둘 다 |
| F07 P3 | deps가 SQL 문자열 검색이라 literal·다른 schema 오탐, 인용 이름 누락 | 기록. 현재 생성 SQL 경로에서는 도달하지 않음. SQL 생성 경로가 늘면 생성 시 의존 집합을 직접 모으는 방식으로 바꿔야 함 |

r4b는 filter·sort·행별 집계 guard·단독 집계 원본·guard 권한 회수 6가지 추가 변경에서도 결과를 바꾼 resource가 모두 deps에 있었다고 확인했다.

## 6. V5-3 캐시 런타임

`sdk/cache.ts`(TS), `tests/v5_3_cache.rs`. Rust 테스트가 실제 facts·계획에서 읽기 deps와 쓰기 태그(전이 대상 + create·update 효과 대상)를 만들고, node 테스트 3개가 캐시 동작을 확인한다.

```text
{"alarmDeps":["MemberAlarm"],"listDeps":["Club","ClubMember","Member","Recruitment"],"writeTags":{"Apply.approve":["Apply","ClubMember"],"MemberAlarm.read":["MemberAlarm"]}}
ℹ tests 3
ℹ pass 3
```

| 사례 | 결과 |
|---|---|
| 알림 읽음 쓰기 뒤 모집 목록 | 캐시 유지(겹치는 resource 없음), 알림 목록은 다시 읽음 |
| 승인(W2, 회원 생성 효과) 뒤 모집 목록 | 무효화. 목록의 internalNote 정책이 ClubMember를 읽기 때문 |
| 다른 actor | 캐시 키가 달라 공유하지 않음 |
| 로그아웃 | 캐시 전부 비움 |
| 쓰기 결과 미확정(`COMMIT_UNKNOWN`) | 전부 비움 |
| 읽기 도중 관련 쓰기가 있었고 응답이 늦게 옴 | 그 응답은 캐시에 저장하지 않음 |

고장 주입: 늦은 응답 차단을 빼면 해당 테스트 실패. 계정 전환 비움을 빼도 처음에는 통과했다(actor가 캐시 키에 있어서 다른 사용자가 읽지는 않음). 로그아웃 뒤 메모리 비움 단언을 더한 뒤 실패로 잡혔다.

한계: 서버·전송 계층이 없어 응답 유실·재연결은 흉내 내지 않았다. 시간에 따라 바뀌는 조회(`now` 의존, 예: 마감 지난 모집 제외)는 쓰기 없이도 결과가 바뀌는데 deps로 표현되지 않는다. 만료 시간 같은 별도 규칙이 필요하다(미구현).

## 7. Codex r5 반영

[codex-v5-r5](../reviews/codex-v5-r5.md): P1 2·P2 3. r4b F01~F09의 원래 재현은 해결됐다고 확인했다.

| 발견 | 내용 | 반영 |
|---|---|---|
| R5-01 P1 | 읽는 도중 쓰기·계정 전환이 있으면 저장은 막지만 옛 결과(이전 관리자의 internalNote 포함)를 호출자에게 그대로 반환 | 관련 쓰기가 있었으면 다시 읽어 최신 값을 반환(최대 2번 더). 계정이 바뀌었으면 `ScopeChanged` 오류 |
| R5-02 P2 | 결과 미확정 뒤 한 번 비우기는 아직 진행 중인 커밋을 막지 못해, 커밋 전 값이 다시 저장됨 | 미확정 쓰기가 `resolveUnknown()`으로 풀릴 때까지 새 결과를 저장하지 않음. 상태 조회 연결은 미구현 |
| R5-03 P2 | 화면이 받은 행을 고치면 캐시 값이 바뀜 | 저장·반환 시 복제 + 깊은 freeze |
| R5-04 P1 | 관계 안 조건부 select(`cond ? "name" : "logo"`)가 두 필드를 모두 보장 | 관계 select도 tuple·union·길이 미정 규칙 적용. 타입 테스트 추가 |
| R5-05 P2 | JS 안전 정수를 넘는 bigint Id가 왕복에서 바뀜(9007199254740993 → …992, 같은 행을 못 찾음) | 미해결. Id를 문자열로 주고받을지, 숫자 범위를 제한할지 공개 계약 선택(STATUS 후속 선택에 추가) |

고장 주입: 늦은 응답 재조회, 미확정 중 저장 차단을 각각 끄면 해당 node 테스트가 실패했다. node 테스트 6/6, tsc 타입 테스트 통과.

## 8. 하지 않은 것

쓰기에 대한 apply 타입, 연결이 끊긴 동안 놓친 변경 복구, 정확한 시간 의존 조회 유효 시각, 계약 변경 diff·구버전 SDK 호환, Python SDK, IDE 자동완성 체감 측정. 실제 서버 호출·응답 유실 복구는 [V6](V6-transport-results.md), 세션 수명 이하 캐시와 시간 의존 조회 미캐시는 [V9](V9-cache-lifetime-results.md)에서 후속 실행했다. V5의 단독 구조에는 전송 계층이 없으며 공통 캐시는 V6에서도 재사용한다.

## 변경 이력

- 2026-10-04 V5-1 결과 타입 추론 실행 결과.
- 2026-10-04 V5-2 읽기 의존 태그 건전성.
- 2026-10-04 Codex r4b 반영: 조건부 select 타입, select 형식 검사, filter 전용 필드 타입, Id 입력.
- 2026-10-04 V5-3 캐시 런타임.
- 2026-10-04 Codex r5 반영: 늦은 응답 재조회·계정 전환 거부, 미확정 중 저장 보류, 행 불변, 관계 select 타입.
- 2026-10-04 V9 캐시 수명 후속 연결. 기본 cargo test에서 기존 캐시 6개와 수명 4개 Node 테스트를 함께 실행.

- 2026-10-04 [V12](V12-typed-transport-results.md)에서 공유 타입 core와 앱별 binding으로 실제 HTTP·캐시 SDK를 연결. typed read만 검증했으며 큰 Id와 typed apply는 여전히 남음.

- 2026-10-04 [V13](V13-id-boundary-results.md)에서 큰 Id의 숫자 제한·십진 문자열 후보와 첫 쓰기 계약 불일치 사전 거부를 검증. V6 HTTP read/apply만 wire 연결했으며 bundle/compose/worker·최종 Id 선택은 남음.

- 2026-10-04 [V14](V14-typed-apply-results.md)에서 별도 후보 binding의 direct 공개 전이 입력/결과·복구 타입, 두 결과 Id 배열 검사·결과 동결·공개 쓰기 지문을 연결. scalar filter 실행 공백과 조합/worker 타입·최종 API는 남음.

- 2026-10-04 후속 [V15](V15-filter-value-results.md)는 Url·Enum·Time read/where 공통 검사와 읽기 Text.prefix를 연결했다. NUL 선거부 범위는 read/where이며 create/compose·공식 패키지 통합은 남는다.
