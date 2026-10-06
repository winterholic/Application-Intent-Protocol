# V3. 표준 쓰기 실험 설계

> 상태: 실험 설계 (2026-10-04). [E §4](E-technical-risks.md#4-작은-실험의-선후-관계) V3를 두 단계로 나눈 계획이다. 문법·쓰기 모델 결정이 아니다. 결과는 별도 결과 문서에 적는다.

## 1. 왜 두 단계로 나누나

E §4 V3 범위는 "알림 read, 승인/위임 W0/W1/W2 대조"다. 한 번에 만들면 쓰기 엔진, 조합 해석기, 불변식 집행이 한꺼번에 검증 대상이 된다. 작은 단위로 나눠 실수를 빨리 드러낸다.

| 단계 | 범위 | 확인할 것 |
|---|---|---|
| V3-1 | 단일 표준 전이 쓰기. 내 알림 읽음(C B.5), 모집 마감(EQ-08) | target id/where, bulk 상한, atomic, 누락·중복 ID, 이미 바뀐 행, read 가시성 ≠ write 권한, 동시 실행 잠금 |
| V3-2 | 아리아리 일괄 승인을 W0/W1/W2로 각각 표현 | 대상 전부 존재, 같은 동아리, 운영진 권한, 중복 회원 금지, 회원 생성 값의 출처, 알림 시점, rollback |

## 2. V3-1 실험 문법 (spike 가정)

```text
resource MemberAlarm {
  fields { id: Id; member: Member; isChecked: Bool }
  rows read when member = actor
  transition read {
    allow member = actor
    from isChecked = false
    to isChecked = true
    repeat unchanged
  }
  expose apply read { target id, where; bulk maxRows 500 }
}
```

V1 문법에 더하는 것: Bool 리터럴, `repeat unchanged|reject`, `expose apply <전이> { target …; bulk maxRows N }`.

호출자 요청:

```json
{ "apply": "MemberAlarm.read", "target": { "ids": ["1", "2"] } }
{ "apply": "MemberAlarm.read", "target": { "where": [{ "field": "isChecked", "op": "eq", "value": false }] } }
```

## 3. V3-1 실행 의미 후보

| 상황 | 후보 의미 | 이유 |
|---|---|---|
| id 대상 중 안 보이는 행 | 없는 행과 같은 `MISSING_TARGET`, 전체 실패 | 존재 추론 차단. 일부만 바꾸고 성공 응답 금지(C B.5) |
| id 대상이 보이지만 allow 거짓 | `FORBIDDEN`, 전체 실패 | 이미 보이는 행이라 존재 정보 추가 노출 없음. read 가시성 ≠ write 권한 |
| where 대상 | (호출자 조건) ∧ 행 정책 ∧ allow ∧ from인 행만 | "내 미읽음 전부" 같은 집합 요청. 정책 밖 행은 대상 집합에 들어오지 않음 |
| 대상 수 > bulk maxRows | `BULK_LIMIT`, 전체 실패 | 조용한 절단 금지 |
| 같은 id 두 번 | `DUPLICATE_TARGET` | 의도 불명확 |
| from 거짓이고 이미 to 상태 | `repeat unchanged`면 unchanged로 성공, `reject`면 `INVALID_STATE` | 재시도·중복 클릭 멱등 |
| from 거짓이고 to도 아님 | `INVALID_STATE`, 전체 실패 | |
| 동시 실행 | 대상 행을 `FOR UPDATE`로 잠그고 판정 후 UPDATE | 두 요청이 같은 행을 동시에 바꿔도 판정이 최신 상태 기준 |
| 응답 | `changed`, `unchanged` id 목록 | 부분 성공 표현 없음(atomic) |

## 4. V3-2 승인 시나리오

아리아리 일괄 승인 요구(C B.6): 상태 변경, 회원 생성, 같은 동아리, 운영진 권한, 중복 회원 금지, 알림.

| 후보 | 표현 | 서버가 막아야 할 반례 |
|---|---|---|
| W0 | 이미 공개된 `approveOnly` 전이 + `ClubMember` 생성을 atomic 묶음 | 지원자 A를 승인하며 지원하지 않은 B를 회원으로 생성. 각 연산 단독 정책만으로는 통과한다. commit 불변식("회원은 승인된 지원이 있어야 함")을 선언하면 막히는지 확인 |
| W1 | 대상 Apply 목록 + 항목별 단계(require, create from item, transition) | 생성 값이 호출자 리터럴이 아니라 서버가 읽은 대상 행에서 오는지. 호출자가 require를 빼도 서버 필수 조건이 적용되는지 |
| W2 | 서버 정의 `transition approve { … create ClubMember {…}; notify … }` + `expose apply approve { sameScope recruitment.club }` | 다른 동아리 지원 섞기, 이미 회원, 권한 없는 운영진, 일부 누락 ID |

### 4.1 아리아리 원본 동작 (코드 읽기, 실행 안 함)

`ariari-backend/.../recruitment/apply/ApplyService.java` 95~124행 `approveApplies`와 `MemberAlarmService.saveAlarms`를 읽었다.

| 요구 | 원본 동작 | V3-2에서 볼 것 |
|---|---|---|
| 대상 전부 존재 | `findAllByIdsWithClub(ids)` 결과만 처리. 없는 id는 조용히 빠지고 나머지만 승인되는 것으로 보임 | 누락 id가 있으면 전체 실패 |
| 빈 목록 | 컨트롤러(`ApplyController` 59~66행)가 `InvalidListParamException`으로 거부. 단, 목록은 있으나 모든 id가 없으면 서비스의 `applies.get(0)`에서 예외가 날 것으로 보임 | 명시 오류 |
| 같은 동아리 | 첫 지원의 동아리와 비교. 권한 검사보다 먼저 실행 | 권한 없는 사람이 동아리 일치 여부를 알 수 없게 |
| 운영진 권한 | `isClubManagerOrHigher` | allow 정책 |
| 이전 상태 | REFUSAL만 거부. 이미 APPROVE면 기존 회원 검사에서 걸림 | from 상태를 명시 |
| 중복 회원 | 조회 후 저장. `ClubMember`에 유일 제약 없음(논리 삭제 엔티티) | 동시 승인에서 중복 생성 여부. DB 유일 제약 또는 잠금 |
| 알림 | `@TransactionalEventListener`(커밋 뒤) + `REQUIRES_NEW`로 MemberAlarm 저장. 저장 실패 시 승인은 유지되고 재시도 없음 | 커밋과 알림 기록의 관계(outbox) |

"보임"으로 적은 항목은 실행으로 확인하지 않았다(확인 필요).

세 후보에 같은 반례 표를 돌려 "안전하게 막히는가"와 "서버에 써야 하는 정의 양"을 함께 기록한다. W1이 안전해지려면 서버가 허용 단계와 값 출처를 선언해야 하는데, 그 선언이 W2와 얼마나 가까워지는지가 핵심 관찰이다.

## 5. 하지 않는 것

외부 알림 전달(outbox 행 기록까지만), worker 확장(V4), 와이어·SDK, 마이그레이션.

## 변경 이력

- 2026-10-04 V3 실험 설계 초안.
