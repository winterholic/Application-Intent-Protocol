# V3. 표준 쓰기 실험 결과

> 상태: 검증 필요 (2026-10-04). [V3 실험 설계](V3-standard-write-plan.md)의 V3-1(단일 표준 전이 쓰기, Codex r2b 반영), V3-2(일괄 승인 W0/W1/W2 비교, Codex r3 반영), V3-3(관리자 위임 W2/W0 비교) 결과다. 쓰기 모델 결정이 아니다.
> 위치: `../../spikes/spike-v3-write/`. V1 facts와 V2 정책 SQL 생성기를 재사용한다. 로컬 PostgreSQL의 `aip_v3_spike`·`aip_v3_r2b` schema를 테스트 시작에 만들고 끝에 지운다.

## 실제 실행 출력

명령: `cd spikes/spike-v3-write && cargo test --offline -q -- --nocapture`

```text
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.86s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.97s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.66s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.68s
```

순서대로 `tests/v3_1.rs`, `tests/v3_2.rs`(V3-2 승인 비교, §6), `tests/v3_r2b.rs`(Codex r2b 재발 방지), `tests/v3_r3.rs`(Codex r3 재발 방지)다.

고장 주입 확인. 아래 장치를 하나씩 무력화했을 때 각각 테스트가 실패했고, 복원 후 통과했다.

| 무력화한 장치 | 실패한 단언 |
|---|---|
| 대상 행 `FOR UPDATE` 잠금 | T2가 잠금을 기다리지 않고 changed, T1은 CONFLICT |
| UPDATE의 from 조건 재검사(잠금도 없음) | 무잠금 경쟁에서 두 요청 모두 changed |
| 누락 대상 검사 | 남의 알림이 섞인 요청이 성공하고 내 알림만 바뀜 |
| allow 정책 | 학생이 모집을 마감 |

## 1. 추가한 실험 문법 (V1 spike)

`true`/`false` 리터럴, 전이의 `repeat unchanged|reject`(기본 reject), `expose apply <전이> { target id, where; bulk maxRows N }`. facts에는 `transitions.<이름>.repeat`, `exposeApply.<이름> = { target, bulkMaxRows }`가 생긴다. 의미 검사 규칙: 전이가 있어야 함, target은 id/where만, bulk maxRows 필수·1 이상, where 대상이면 같은 resource의 expose read filter 허용 목록이 있어야 함.

## 2. 실행 의미와 결과

알림 fixture: C B.5의 MemberAlarm에 `expose read { select id, isChecked; filter isChecked.eq; … }`를 더했다. where 대상이 읽기 filter 허용 목록을 따르게 하려고 넣은 것이다. bulk 상한은 시험을 위해 3으로 낮췄다.

| 상황 | 결과 | 설계 표 대응 |
|---|---|---|
| 내 알림 1,2 | changed [1,2] | |
| 다시 1,2,3(이미 읽음) | unchanged [1,2,3] (`repeat unchanged`) | 재시도 멱등 |
| 내 알림 + 남의 알림 | `MISSING_TARGET`, 내 알림도 그대로 | 일부 변경 성공 응답 없음 |
| 없는 id, 익명 | `MISSING_TARGET` (안 보이는 행과 구분 안 함) | 존재 추론 차단 |
| 같은 id 두 번 | `DUPLICATE_TARGET` | |
| id 4개 > 상한 3 | `BULK_LIMIT` | 조용한 절단 없음 |
| where "내 미읽음 전부"가 4개 | `BULK_LIMIT`, 아무것도 안 바뀜 | |
| where 미읽음 3개 | changed [5,6,7], 남의 알림은 대상 밖 | where ∧ 행 정책 ∧ allow ∧ from. 이미 목표 상태인 행은 대상 아님(r2b 후) |
| where에 filter 허용 목록 밖 필드(member) | `FILTER_NOT_ALLOWED` | 변경 개수로 숨은 값 추론 차단 |
| 모르는 키, 공개 안 된 동작 | `UNKNOWN_KEY`, `NOT_EXPOSED` | |
| 학생이 보이는 모집을 마감 | `FORBIDDEN` | read 가시성 ≠ write 권한 |
| 관리자가 타 학교 모집·없는 모집 마감 | 둘 다 `MISSING_TARGET` | |
| 마감은 where 대상 계약 밖 | `TARGET_NOT_ALLOWED` | |
| 관리자 마감 후 같은 요청 재시도 | `MISSING_TARGET` | §3 발견 |
| 같은 알림 동시 2요청, 잠금 있음 | T1 changed, T2는 약 400ms 기다린 뒤 unchanged | |
| 같은 알림 동시 2요청, 잠금 없음 | 한쪽 changed, 다른 쪽 `CONFLICT`. 두 번 바뀌지 않음 | UPDATE의 from 재검사. from과 to 상태가 겹치지 않는 전이에서만 성립(R2B-10) |

실행 순서: 한 트랜잭션에서 행 정책을 만족하는 대상 행만 `SELECT … FOR UPDATE`로 잠그며 allow·from·이미 to 상태 여부를 함께 계산한다. 이때 정책이 읽는 다른 행(예: managerOf의 ClubMember)은 `FOR SHARE`로 잠근다. 판정이 모두 통과해야 `UPDATE … WHERE id = ANY(…) AND <from>`을 실행하고, 바뀐 행 수가 판정과 다르면 전체 취소(`CONFLICT`)한다. where 대상은 상한+1개까지만 잠근다.

## 3. 드러난 것

- **전이 결과가 행 정책 밖으로 나가면 재시도가 unchanged가 아니라 MISSING_TARGET이 된다.** C B.1의 모집 행 정책은 `active(this)`(게시 중)를 요구한다. 마감하면 관리자에게도 안 보인다. 실제 아리아리는 관리자가 자기 동아리의 마감 모집을 관리 화면에서 본다. 읽기 행 정책에 관리자 분기가 필요하고, `repeat unchanged`는 전이 뒤에도 보이는 행에서만 의미가 있다.
- where 대상 쓰기에 읽기 filter 허용 목록을 요구하자 MemberAlarm에 expose read가 필요해졌다. 쓰기 계약이 읽기 계약에 기대는 구조다.
- 잠금 없이도 UPDATE의 from 재검사만으로 이중 변경은 막힌다(from과 to가 겹치지 않는 전이 한정). 잠금은 결과를 CONFLICT 대신 unchanged로 만들어 재시도 경험을 낫게 한다.

## 4. spike가 정한 규칙

| ID | 규칙 | 열린 점 |
|---|---|---|
| V3-R1 | id 대상의 안 보이는 행은 없는 행과 같은 `MISSING_TARGET`. 안 보이는 행은 잠그지 않아 다른 트랜잭션이 잠가도 기다리지 않는다 | 몇 개가 없는지는 알려 줌. 오류 상세 공개 수준은 제품 정책 후보 |
| V3-R2 | 보이지만 allow 거짓이면 `FORBIDDEN` | 이미 보이는 행이라 존재 정보는 새로 드러나지 않음 |
| V3-R3 | where 대상은 행 정책·allow·from을 모두 만족하는 행 집합. 권한 없는 행·이미 목표 상태인 행은 오류 없이 대상 밖. `repeat`는 id 대상에만 의미 | "보이는데 권한 없는 행"이 섞였는지 호출자는 모름. id 집합과 정책 교차 집합의 '모두'는 다른 의미 |
| V3-R4 | where 대상 조건은 expose read filter 허용 목록 + eq만 | 범위 연산은 미지원 |
| V3-R5 | 대상 판정과 변경은 READ COMMITTED 트랜잭션 + 대상 행 `FOR UPDATE` + 정책 의존 행 `FOR SHARE` + from 재검사. 판정 근거가 된 권한 행은 커밋까지 바뀌지 않아, 진행 중 쓰기 뒤에 회수가 반영된다 | 행의 **부재**에 기대는 정책(`not exists`)은 행 잠금으로 보호되지 않는다(새 행 삽입). SERIALIZABLE 비교는 미실행 |
| V3-R6 | 쓰기 요청 전체 기한 2s + 문장별 statement_timeout. 커밋 직전 남은 기한이 300ms 미만이면 rollback 후 `DEADLINE_EXCEEDED`(rollback 보장). 커밋을 보낸 뒤 기한을 넘기면 `COMMIT_UNKNOWN`(커밋됐을 수 있음, 상태 조회 필요) | 쓰기 budget 위치 미정. 2s·300ms는 spike 값. 재시도용 멱등 키·상태 조회 API는 미구현(R3-05) |

## 5. 독립 검토와 반영

[codex-v2v3-r2b](../reviews/codex-v2v3-r2b.md) 쓰기 쪽 발견과 반영:

| 발견 | 내용 | 반영 |
|---|---|---|
| R2B-04 P2 | where 대상 코드가 `from OR done`이라 문서(from만)와 다름 | from만으로 통일. 이미 목표 상태만 고르면 changed [] |
| R2B-06 P1 | 대상 행만 잠가서, 판정 뒤 관리자 권한(ClubMember)을 지워도 마감 성공 | 정책 subquery에 `FOR SHARE`. 회수 DELETE가 진행 중 쓰기 커밋까지 기다림을 테스트로 확인. 잠금을 끄면 테스트 실패 |
| R2B-07 P2 | 잠금 대기 초과가 INTERNAL, 요청 전체 기한 없음 | 57014 → `DEADLINE_EXCEEDED`, 요청 전체 2s 기한 |
| R2B-09 P2 | `to internalNote = null` 실행 실패 | null 대입·재시도 unchanged 확인 |
| R2B-10 P3 | from이 목표 상태를 포함하면 재시도도 changed | 이미 목표 상태면 from보다 먼저 unchanged |
| R2B-11 P2 | 숨은 행도 잠가서 경합 시 오류 코드로 존재가 드러남 | 행 정책을 잠금 SELECT의 WHERE로. 잠긴 숨은 행도 즉시 `MISSING_TARGET`. 되돌리면 테스트 실패 |

## 6. V3-2 일괄 승인: W0·W1·W2 비교

아리아리 `approveApplies`(설계 §4.1)의 요구를 세 후보로 표현하고 같은 반례 표에 돌렸다. 엔진은 판정(잠금·정책·상태·같은 범위) → 변경 → 전이 효과 → 생성 → 커밋 검사 → 커밋 순서이고 전부 한 트랜잭션이다.

### 6.1 서버 정의

```text
// W2: 의미 있는 표준 동작. 효과 값은 서버가 대상 행에서 계산
transition approve {
  allow managerOf(actor, recruitment.club)
  from status = PENDING
  to status = APPROVE
  create ClubMember { club = recruitment.club; member = member; role = MEMBER }
  notify member "apply.approved"
}
expose apply approve { target id; bulk maxRows 100; sameScope recruitment.club }

// W0: 이미 공개된 동작의 묶음
transition approveOnly { allow …; from status = PENDING; to status = APPROVE }
expose apply approveOnly { target id; bulk maxRows 100; sameScope recruitment.club }
// ClubMember 쪽
expose create { allow managerOf(actor, club); fields club, member, role }
check hasApproval when approvedApplicant(this)   // 커밋 검사
predicate approvedApplicant(cm: ClubMember) = exists Apply where member = cm.member and recruitment.club = cm.club and status = APPROVE

// W1: 호출자 조합. 허용 단계와 값 출처를 서버가 선언
expose compose { bulk maxRows 100; sameScope recruitment.club; transitions approveOnly; create ClubMember from member, recruitment.club }

// 공통: ClubMember에 unique club, member
```

| | 서버 정의(줄) | 호출자 요청 | 알림 |
|---|---|---|---|
| W2 | 전이 6 + 공개 1 + unique 1 = 8 | `{apply: "Apply.approve", target: {ids}}` | outbox 기록 |
| W0 | 전이 4 + 공개 1 + create 1 + check 2 + predicate 2 + unique 1 = 11 | 승인 1개 + 항목마다 `create ClubMember {club, member, role}` 값을 직접 | 없음(알림 동작이 공개돼 있지 않음) |
| W1 | 전이 4 + compose 1 + create 1 + check 2 + predicate 2 + unique 1 = 11 | 대상 + 단계 `[{transition}, {create, values: {item 경로 / const}}]` | 없음 |

r3 뒤 W0·W1에 더한 것(R3-02): create allow에 `role = MEMBER`, Apply 검사 `check approvedHasMember when status != APPROVE or joined(this)`와 `predicate joined`. 이것이 없으면 승인만 하고 회원을 만들지 않거나 ADMIN으로 만드는 요청이 성공했다. 줄 수는 선언 묶음 기준이며 작성·수정 비용 측정이 아니다.

W0 요청은 대상 N개에 연산 N+1개다. 묶음 연산 상한 20 때문에 W0의 실효 일괄 승인 상한은 19명이고, W2·W1은 bulk maxRows 100이다(R3-04).

W0 호출자는 지원자의 회원 id와 동아리 id를 알아야 생성 값을 보낼 수 있다. 이 fixture에는 Apply 읽기 계약이 없어서 실제 화면은 그 값을 얻을 방법부터 필요하다.

### 6.2 공통 반례 결과 (세 후보 모두 같은 결과)

| 반례 | 결과 | DB 상태 |
|---|---|---|
| 정상 승인 [300,301] | OK | 승인 2, 회원 2명 생성. W2만 outbox 2 |
| 같은 동아리 다른 모집 [300,305] | OK | 승인 2, 회원 2명 |
| 다른 동아리 섞기(양쪽 운영진) | `NOT_SAME_SCOPE` | 변화 없음 |
| 일반 회원이 승인 | `MISSING_TARGET`(지원 행 자체가 운영진에게만 보임) | 변화 없음 |
| 일부 없는 id | `MISSING_TARGET` | 변화 없음(원본은 없는 id를 빼고 나머지만 승인하는 것으로 보임) |
| 이미 회원 포함 | `ALREADY_EXISTS`(unique) | 변화 없음 |
| 거절된 지원 | `INVALID_STATE` | 변화 없음 |
| 빈 목록 | `BAD_REQUEST` | 변화 없음 |
| 같은 지원 동시 승인(W2·W1) | 하나 OK, 하나 `INVALID_STATE` | 회원 1명 |
| 다른 지원으로 같은 회원 동시 승인(W2·W1) | 하나 OK, 하나 `ALREADY_EXISTS` | 회원 1명 |

같은 범위 검사는 가시성·권한·상태 판정 뒤에 한다. 권한 없는 호출자는 동아리 일치 여부를 알 수 없다(원본은 동아리 검사가 권한 검사보다 먼저). r3 전에는 W1만 범위 검사를 단계 판정보다 먼저 해서 복합 위반에서 다른 오류가 났다(R3-01). 지금은 세 후보가 같은 우선순위다(거절됨+다른 동아리 → 모두 `INVALID_STATE`, 행 정책이 넓을 때 권한 없는 동아리 섞임 → 모두 `FORBIDDEN`).

### 6.3 후보별 반례

| 반례 | 결과 |
|---|---|
| W0 승인 + 지원하지 않은 회원 생성 | check 있음 `CHECK_FAILED` / **check 없음 OK(지원 안 한 회원이 생김)** |
| W0 회원 생성만 | check 있음 `CHECK_FAILED` / check 없음 OK |
| W0 일반 회원이 회원 생성 | `FORBIDDEN`(create allow) |
| W1 member를 상수로 | `VALUE_SOURCE_NOT_ALLOWED`(참조 필드는 상수 금지) |
| W1 선언 안 된 출처 경로 | `VALUE_SOURCE_NOT_ALLOWED` |
| W1 허용 안 된 전이 | `STEP_NOT_ALLOWED` |
| W1 승인 단계 빼고 생성만 | check 있음 `CHECK_FAILED` / **check 없음 OK(승인 안 된 지원자가 회원이 됨)** |
| W1 생성 뒤 승인(순서 바꿈) | OK. check는 커밋 시점에 평가 |
| W0·W1 승인만(회원 생성 없음) | `CHECK_FAILED`(approvedHasMember, r3 후) |
| W0·W1 role ADMIN으로 생성 | `FORBIDDEN`(create allow의 role = MEMBER, r3 후) |

고장 주입: unique 제거 → 이미 회원 사례가 세 후보 모두 중복 회원 생성. 같은 범위 검사 제거 → 다른 동아리 섞기가 세 후보 모두 성공. 커밋 검사 제거 → W0 위조·W1 생성만 사례 성공. W1 출처 제한 제거 → 상수 member는 커밋 검사가, `recruitment.club.school`은 학교 id가 회원 id 자리에 들어가 unique 위반으로 겨우 막힘(타입 혼동). 모두 복원 후 통과.

### 6.4 관찰

- **W2는 효과 값과 순서를 전이 한 곳에 둔다.** 회원 생성 값이 대상 행에서 오므로 위조 경로가 없다. 다만 unique 같은 불변식은 여전히 resource 쪽 별도 선언이다. 알림까지 같은 트랜잭션 outbox로 남는다.
- **W0은 공개 동작 하나하나의 정책만으로는 안전하지 않다.** 운영진은 승인과 별개로 아무 회원이나 만들 수 있다. 커밋 검사(지원 승인된 회원만)를 선언하면 막히지만, 그 검사는 초대 같은 다른 정당한 회원 추가 경로도 막는다.
- **W1이 안전해지려면 서버가 허용 단계, 값 출처, 생성 allow, 커밋 검사를 모두 선언해야 했다.** r3 반영 뒤 서버 선언 묶음은 W1 11 vs W2 8이다(작성·수정 비용 측정 아님). 단계 순서·생략 자유 때문에 커밋 검사 없이는 승인 없는 회원 생성이 가능했다. 이 fixture에서 W1이 W2보다 줄여 준 서버 작업은 관찰되지 않았다. 관리자 위임 비교는 §7.
- 원본의 "누락 id 무시", "권한보다 앞선 동아리 검사", "유일 제약 없는 회원 생성"은 세 후보 모두에서 다르게 동작했다(전체 실패, 권한 먼저, unique). "커밋 뒤 별도 트랜잭션 알림"을 outbox로 대체한 것은 W2뿐이다. W0·W1은 알림이 없다(R3-07).
- 위 관찰은 이 fixture의 선언 구성에 한정된다. W1의 자유(단계 생략·순서·상수)를 유지하면서 업무 완결성을 맞추려면 규칙이 더 필요했다는 근거이고, W1이 어떤 제한을 택해도 W2보다 서버 작업이 많다는 근거는 아니다. 관리자 위임·순환 전이에서는 결론이 달라질 수 있다.

### 6.5 V3-2 규칙과 한계

| ID | 규칙 | 한계 |
|---|---|---|
| V3-R7 | 전이 효과 `create`·`notify`는 바뀐 대상 행에서 서버가 값을 계산 | 효과 순서·조건부 효과 없음 |
| V3-R8 | 커밋 검사 = 이 트랜잭션이 쓴 행(`xmin` = 현재 트랜잭션)의 **사후조건**. 검사가 읽은 근거 행은 `FOR SHARE`로 커밋까지 잠근다. 전역 관계 불변식이 아니다: 커밋 뒤 다른 쓰기가 근거를 바꾸면(예: 승인 철회) 다시 검사하지 않는다 | savepoint 안에서 쓴 행은 빠진다. 전역 관계 불변식이 필요하면 의존 변경 추적·DB 트리거 등 다른 장치가 필요(미구현, R3-03) |
| V3-R9 | W1 값은 선언된 출처 경로(타입 일치) 또는 참조가 아닌 필드의 상수만 | 경로 비교는 문자열 일치 |
| V3-R10 | W0·W1의 생성은 대상 resource의 `expose create` allow를 새 행에 대해 평가 | 생성된 행 id 반환 없음 |

### 6.6 Codex r3 반영

[codex-v3-r3](../reviews/codex-v3-r3.md): 7건(P1 2, P2 4, P3 1). r2b P1 두 건은 원래 입력에서 재발하지 않았다.

| 발견 | 내용 | 반영 |
|---|---|---|
| R3-01 P2 | W1만 범위 검사를 단계 판정보다 먼저 | 범위 검사를 단계 실행 뒤로. 복합 위반 오류가 세 후보 같음 |
| R3-02 P2 | W0·W1에서 승인만, ADMIN 생성 가능 | create allow `role = MEMBER`, Apply 사후조건 `approvedHasMember`. 서버 정의 9 → 11줄 |
| R3-03 P1 | 커밋 검사가 근거 행 변경(검사 뒤 커밋 전 철회, 커밋 뒤 철회)을 못 막음 | 검사 근거 행 `FOR SHARE`(철회가 커밋까지 기다림, 테스트). check를 사후조건으로 명시(V3-R8). 커밋 뒤 철회는 막지 않음 |
| R3-04 P2 | W0 실효 상한 19 | §6.1에 기록 |
| R3-05 P1 | 커밋 중 기한 초과를 실패로 응답했지만 실제 커밋 | 커밋 전 여유 300ms 미달이면 rollback, 커밋 전송 뒤 기한 초과는 `COMMIT_UNKNOWN`. 지연 트리거로 두 경우 테스트 |
| R3-06 P2 | compose 출처에 상수를 넣으면 panic | V1이 경로 아닌 출처 거부, 실행기 unwrap 제거 |
| R3-07 P3 | outbox 관찰이 W0·W1에도 해당하는 것처럼 묶임 | §6.4 문구 정정 |

각 반영 장치(검사 근거 잠금, 커밋 전송 표시, 커밋 전 여유)를 끄면 `tests/v3_r3.rs`가 실패함을 확인했다.

## 7. V3-3 관리자 위임: W2·W0·W1 비교

아리아리 `ClubMemberService.entrustAdmin`(코드 읽기): 대상에게 ADMIN, 요청자는 GENERAL. 권한 검사는 요청자 ADMIN 여부뿐이고, 자기 자신에게 위임하면 ADMIN을 줬다가 바로 GENERAL로 바꿔 동아리에 관리자가 없어질 것으로 보인다. 잠금·유일 제약이 없어 동시 위임 시 관리자 2명이 될 수 있어 보인다(둘 다 실행 확인 안 함).

`tests/v3_3.rs`. 공통 선언: ClubMember에 `id`, 행 정책 `member = actor or managerOf(actor, club)`, `invariant oneAdmin per club deferred`(동아리당 관리자 최대 1명, 커밋 시점 검사), `check keepsAdmin when hasAdmin(club)`.

```text
// W2: 서버 정의 전이 + 갱신 효과
transition entrust {
  allow isAdmin(actor, club) and member != actor
  from role != ADMIN
  to role = ADMIN
  update ClubMember where club = club and member = actor { role = MEMBER }
}
// W0: 두 전이를 묶음으로
transition makeAdmin { allow isAdmin(actor, club); from role != ADMIN; to role = ADMIN }
transition resignAdmin { allow member = actor; from role = ADMIN; to role = MEMBER }
```

| 사례 | W2 | W0 |
|---|---|---|
| 정상 위임 | OK | OK(임명 → 내려놓기 순서) |
| 자기 자신에게 | `FORBIDDEN`(member != actor) | `INVALID_STATE`(이미 ADMIN) |
| 관리자 아닌 운영진 | `FORBIDDEN` | `FORBIDDEN` |
| 다른 동아리 회원 | `MISSING_TARGET` | `MISSING_TARGET` |
| W0 내려놓고 임명(순서 바꿈) | - | `MISSING_TARGET`(내려놓은 뒤 운영진이 아니라 대상이 안 보임) |
| W0 임명만(관리자 2명) | - | `INVARIANT_VIOLATED`(커밋 시점) |
| W0 내려놓기만(관리자 0명) | - | `CHECK_FAILED` |
| 위임 뒤 원래 관리자가 재위임 | `MISSING_TARGET`(더 이상 운영진 아님) | - |
| 같은 관리자가 두 사람에게 동시 위임 | 하나 OK, 하나 `CONFLICT`(교착 감지, 재시도 가능) | 같음 |

고장 주입: 지연 제약을 즉시 유일 인덱스로 바꾸면 정상 위임이 W2·W0 모두 실패했다(교대 중 잠시 관리자 2명). `keepsAdmin`을 빼면 관리자 0명이 허용됐다.

관찰:
- 교대형 불변식은 **커밋 시점 제약**이 필요했다. 즉시 제약은 정당한 교대를 막는다. 아리아리 원본은 이 불변식 자체가 DB에 없다.
- W0은 순서에 민감하다. 먼저 내려놓으면 자신의 권한이 사라져 임명이 실패한다. 호출자(또는 AI)가 순서를 알아야 한다. W2는 순서를 서버 정의 안에 둔다.
- 자기 위임 방지는 W2에서 명시 조건(`member != actor`)으로, W0에서는 상태 조건(이미 ADMIN)으로 우연히 막혔다.
- 동시 위임은 대상 행 잠금, 정책 의존 행 공유 잠금, 갱신 효과의 배타 잠금이 엇갈려 교착이 났고 PostgreSQL이 한쪽을 끊었다. 결과는 안전하지만 실패한 쪽은 재시도가 필요하다. 교착 없이 직렬화하려면 actor 행을 처음부터 배타 잠금하는 등 다른 순서가 필요하다(미실험).
- **처음 `expose compose`는 이 업무를 표현하지 못했다.** compose는 같은 대상 목록에 같은 단계를 적용하는데, 위임은 대상과 요청자 자신의 행에 서로 다른 전이를 적용한다. 그래서 `selfRow member by club`(대상과 같은 동아리의 요청자 자신 행)과 단계별 `"on": "self"`를 더했다.

```text
expose compose { bulk maxRows 1; sameScope club; transitions makeAdmin, resignAdmin; selfRow member by club }
// 호출자
{ compose: "ClubMember", targets: { ids: ["3"] },
  steps: [{ transition: "makeAdmin" }, { transition: "resignAdmin", on: "self" }] }
```

  W1은 위 공통 반례 전부에서 W0과 같은 결과를 냈다(정상 OK, 자기 위임 `INVALID_STATE`, 관리자 아님 `FORBIDDEN`, 다른 동아리 `MISSING_TARGET`, 순서 바꿈 `MISSING_TARGET`, 임명만 `INVARIANT_VIOLATED`, 내려놓기만 `CHECK_FAILED`, 동시 위임 하나만 성공). 자기 행 대신 대상에 내려놓기를 적용하면 `FORBIDDEN`(resignAdmin의 allow는 자기 행만).
- 서버 정의: W2는 전이 6줄 + 공개 1줄, W0은 전이 2줄 + 공개 2줄, W1은 같은 전이 2줄 + compose 1줄. 공통 선언(불변식·검사·predicate)은 같다. 이 업무에서 W0·W1은 정의가 짧지만 호출자가 순서를 알아야 하고, 안전은 결국 커밋 시점 불변식과 사후조건이 맡는다. W2는 순서를 서버 정의 안에 둔다.
- 승인(V3-2)과 위임(V3-3)을 합치면: W1을 안전하게 만드는 선언(허용 단계·값 출처·자기 행·불변식·사후조건)은 W0과 거의 같다. W1의 이득은 "값을 서버가 읽은 대상 행에서 가져온다"(승인에서 위조 회원 차단)는 점이고, 비용은 compose 문법이 업무마다 새 개념(출처, selfRow)을 요구한다는 점이다. 어느 쪽을 표준으로 둘지는 이 두 업무만으로 정하지 않는다.

r4b([codex-v345-r4b](../reviews/codex-v345-r4b.md)) Q3: V3-3에서 발견 없음. 새 관리자의 재위임, 서로 다른 동아리 동시 위임, 교대 직후 추가 작업, update 효과 대상 0행·2행(전체 rollback)을 추가 실행해 의도와 일치했다. CONFLICT 뒤 새 요청으로 재시도하면 권한 재평가로 `MISSING_TARGET`이 될 수 있어, 자동 재시도가 업무 성공을 보장하지는 않는다.

r6([codex-v34-r6](../reviews/codex-v34-r6.md)) F01(P1): `selfRow member by club`인데 `sameScope club.school`처럼 다른 경로를 선언하면 V1이 받아들였고, 다른 동아리의 자기 행이 바뀌었다. sameScope가 `by` 필드 그 자체여야 하도록 V1에서 거부한다(`tests/v1_r2.rs`). 기본 위임 선언에서는 재현되지 않았다.

## 8. 하지 않은 것

순환 전이(여러 행이 서로 상태를 바꾸는 업무)로 W1 재평가, 삭제·관계 연결 쓰기, 쓰기 후 캐시 태그, 감사 기록, 외부 효과 전달(outbox 소비), H 형식의 전이 효과 문법.

## 변경 이력

- 2026-10-04 V3-1 실행 결과.
- 2026-10-04 Codex r2b 반영: 정책 의존 행 공유 잠금, 숨은 행 미잠금, 요청 기한, where from-only, null 대입, 재시도 우선순위.
- 2026-10-04 V3-2 승인 W0/W1/W2 비교 결과.
- 2026-10-04 Codex r3 반영: 커밋 결과 미확정 구분, 검사 근거 행 잠금·사후조건 명시, W1 오류 우선순위, W0·W1 업무 완결성 규칙, 출처 검사.
- 2026-10-04 V3-3 관리자 위임 W2·W0 비교, 지연 불변식·갱신 효과 추가.
- 2026-10-04 compose `selfRow`·단계별 `on: self`로 W1 위임 표현, 같은 반례 비교.
- 2026-10-04 Codex r6 반영: selfRow 범위 일치 검사, 제목·숫자 정정.

- 2026-10-04 후속 [V15](V15-filter-value-results.md)는 Url·Enum·Time read/where 공통 검사와 읽기 Text.prefix를 연결했다. NUL 선거부 범위는 read/where이며 create/compose·공식 패키지 통합은 남는다.
