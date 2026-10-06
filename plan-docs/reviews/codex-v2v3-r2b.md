# 데이터 접근 프레임워크 설계 실험 정확성 검토 (r2b)

기준일: 2026-10-04. 기준 우선순위는 창시자 통합 지침, E의 RK-01~04·§4, C의 EQ-01~11·B.5다. 작성자 주장은 미검증 상태에서 출발했다. 원본 수정은 이 파일에 한정하며 git·.env·비밀 저장소에 접근하지 않는다. 변형 실험은 `/private/tmp` 복사본에서 수행한다. 발견은 확인 시점에 append한다.

## 직접 실행 기록

세 디렉터리 각각에서 명령 `PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`를 직접 실행했다.

V1 (`spikes/spike-v1-fixture`, exit 0):
```text
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.17s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

V2 (`spikes/spike-v2-read`, exit 0):
```text
planner rejection cases: 19
db scenarios ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.42s
```

V3 (`spikes/spike-v3-write`, exit 0):
```text
v3-1 scenarios ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.84s
```

SQL 표시 명령: `cd spikes/spike-v2-read && PATH="$HOME/.cargo/bin:$PATH" cargo run --offline -q --example show_sql` (exit 0).
```text
params: [Some("1"), Some("ADMIN"), Some("MANAGER"), Some("PUBLISHED"), Some("2026-10-04T00:00:00Z"), Some("2026-10-09T00:00:00Z"), Some("20")]
cost: 80
```
행 WHERE, 필드 CASE, 관계 LEFT JOIN LATERAL과 대상 행 정책, count 상관 subquery를 실제 SQL에서 확인했다. localhost PostgreSQL 실행 가능. 테스트의 DDL은 실행 시작에 schema를 지우고 재생성하며 종료 정리는 하지 않는다(V2 `tests/v2.rs:135`, 끝 `:234`; V3 `tests/v3_1.rs:68`). 이 검토의 종료 단계에서 두 허용 schema를 직접 정리한다.

## 발견 (확인 순서)

### R2B-01 · P2 · Q1/Q6 · 단독 집계 거부 코드가 실행 API에 없다

문제: 비관리자·익명·없는 동아리의 집계 요청은 `ACCESS_DENIED` 오류가 아니라 성공 `Ok([null])`을 반환한다. 결과 문서의 오류 코드와 실행 API가 다르다. 값 노출은 관찰하지 않았다.

근거: `spikes/spike-v2-read/src/plan.rs:314`는 guard 실패를 SQL NULL로 만들고, `src/lib.rs:39`는 NULL을 성공 배열에 넣는다. `tests/v2.rs:201`에서만 `Value::Null => Err("ACCESS_DENIED")`로 바꿔 단언한다. `plan-docs/alignment/V2-caller-read-results.md:55,71`은 이 테스트 변환을 실제 실행 의미처럼 적는다.

직접 실행 근거: 위 V2 `db_vertical_slice`의 관리자/비관리자/익명/없는 동아리 사례가 모두 실행됐고 4 tests passed. 오류 코드를 확인하는 단계는 테스트의 로컬 변환이다.

조치: 반환 Plan에 집계 응답 종류를 명시하고 공통 executor에서 NULL guard 결과를 정해진 오류로 변환하거나, 실제 계약을 null 응답으로 기록한다. HTTP/SDK가 없으므로 최종 와이어 오류 매핑은 확인 못함: 해당 구현 없음.

R2B-01 추가 직접 재현: 임시 복사본 `/private/tmp/aip-r2b-9gmg4y3w/spike-v2-read`에서 `PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="/Users/winterholic/development/projects/aip/spikes/spike-v2-read/target" cargo test --offline -q r2b_read_probes -- --nocapture` 실행(exit 0).
```text
R2B aggregate actor=Some(1): Ok([Number(2)])
R2B aggregate actor=Some(2): Ok([Null])
R2B aggregate actor=None: Ok([Null])
```
문서의 정확한 근거 줄은 `V2-caller-read-results.md:55,71`이다.

### R2B-02 · P2 · Q2/Q7 · V1이 허용하는 Bool 조건식을 V2/V3 SQL 생성기가 거부한다

문제: 정상 서버 정책 `rows read when true`와 `rows read when false` 모두 V1 facts는 나오지만 읽기 계획은 INTERNAL로 실패한다. V3의 allow/from도 같은 `Ctx::cond`를 쓰므로 단독 Bool 조건을 지원하지 않는다. 무조건 허용/거부는 정상 정책이며, 새 Bool 지원이 비교식의 피연산자에 한정됐음을 문서가 밝히지 않는다.

근거: `spikes/spike-v1-fixture/src/sema.rs:415`가 Bool facts를 만든다. `spikes/spike-v2-read/src/sqlgen.rs:216`의 cond_inner는 lit/Bool 경로 없이 `:267`에서 오류를 낸다. `spikes/spike-v3-write/src/lib.rs:113`부터 vis/allow/from에 그대로 사용한다.

직접 실행: 위 `r2b_read_probes`에서 Recruitment 행 정책 전체를 true/false로 각각 바꾸고 원래 list 요청을 사용했다.
```text
R2B bool true V1 OK, plan=Reject { code: "INTERNAL", msg: "조건으로 쓸 수 없는 식 {\"lit\":true}" }
R2B bool false V1 OK, plan=Reject { code: "INTERNAL", msg: "조건으로 쓸 수 없는 식 {\"lit\":false}" }
```
조치: typed Bool 리터럴과 Bool 경로가 조건으로 쓰이는 경우를 SQL 생성기의 지원 범위에 포함하고, V1 수용과 실행 지원 범위를 맞춘다. 조건 true/false의 양성·음성 대조를 고정한다.

### R2B-03 · P2 · Q3/Q6 · 요청 구조 크기가 cost 계산 밖이고 중복 select는 DB 오류까지 간다

문제: `limit:1`에 같은 id 선택 51개는 cost=1로 승인되나 PostgreSQL 함수 인자 한도로 INTERNAL(54023)이 된다. 필터 1000개와 정렬 1000개 역시 cost=1이며 56,342-byte SQL과 1004개 매개변수를 만든다. cost budget이 요청 구조 비용을 제한하지 않는다.

근거: `spikes/spike-v2-read/src/plan.rs:43`는 관계·집계·필드 정책만 센다. `:98`의 select, `:192`의 filter, `:217`의 sort 반복 횟수와 일반 필드 수는 제한하지 않는다. `:250`은 각 select를 그대로 json_build_object 인자로 출력하고 출력 타입 Map은 같은 이름을 덮어쓴다. SQL 생성은 DB statement_timeout 설정(`src/lib.rs:20`) 전에 끝난다.

직접 실행: 위 `r2b_read_probes`.
```text
R2B duplicate fields: cost=1 sql_bytes=1037 result=Err(Reject { code: "INTERNAL", msg: "실행 실패(sqlstate 54023)" })
R2B 1000 filters+sorts: cost=1 params=1004 sql_bytes=56342 result=Ok([Object {"id": Number(100)}])
```
입력은 각각 `select:["id" × 51]` 및 `select:["id"], filter:[periodEnd.gte NOW × 1000], sort:[id.asc × 1000]`, 둘 다 limit=1이다. 요청 크기·선택/필터/정렬 개수·SQL/매개변수 크기를 제한하고 중복은 제거 또는 명시 거부한다. 성능 붕괴는 확인 못함: 큰 부하나 실제 메모리 상한까지 실험하지 않았다. V2-R6의 휴리스틱 한계는 이미 문서에 있지만, RK-03 관문 충족 근거로 사용할 수 없다.

### R2B-04 · P2 · Q5/Q6 · where 대상 정의가 문서의 from-only 집합과 다르다

문제: where SQL은 `from OR done`을 대상에 넣는다. 문서·계획의 `where ∧ rows ∧ allow ∧ from`과 다르며, repeat reject 전이의 이미 목표 상태인 행도 포함해 INVALID_STATE로 전체 실패한다. repeat unchanged에서는 이미 목표 상태인 행이 bulk 상한과 unchanged 목록에 포함된다. 코드가 잘못된 제품 선택인지 문서가 잘못된지는 결정이 필요하다.

근거: `spikes/spike-v3-write/src/lib.rs:134`, `:165`, `:178`; 비교 기준 `plan-docs/alignment/V3-standard-write-plan.md:45`와 `V3-standard-write-results.md:65`(V3-R3). 원래 false 필터 사례는 done=true를 제외하므로 이 차이를 검증하지 못한다.

직접 실행: 임시 복사본 `r2b_write_probes`에서 MemberAlarm repeat를 reject로 바꾸고 `target:{where:[{field:"isChecked",op:"eq",value:true}]}` 요청.
```text
R2B repeat reject {"apply":"MemberAlarm.read","target":{"where":[{"field":"isChecked","op":"eq","value":true}]}}: Err(Reject { code: "INVALID_STATE", msg: "3는 `read` 이전 상태가 아님" })
R2B where empty: Err(Reject { code: "BULK_LIMIT", msg: "대상이 상한 3을 넘음" })
```
조치: from-only 대상인지 재시도 상태를 포함한 대상인지 정하고 repeat별 bulk·빈 where·재시도 결과를 명시한다. 일부 성공이나 권한 밖 행 변경은 이 실험에서 관찰하지 않았다.

### R2B-05 · P2 · Q4/Q6 · 읽기에서 공개한 Bool 필터를 V2가 실행하지 못한다

문제: V3 알림 fixture가 `expose read ... filter isChecked.eq`를 선언하지만 같은 조건의 읽기 요청은 BAD_VALUE다. 동일 공개 필터가 쓰기 where에서는 실행된다. 읽기 목록을 쓰기 대상으로 재사용했다는 V3-R4의 계약이 타입 지원까지 일관되지는 않다.

근거: `spikes/spike-v2-read/src/plan.rs:53` check_value에 Bool이 없고 `:201`에서 호출한다. V3 `src/lib.rs:37`은 Bool을 지원한다. fixture는 `spikes/spike-v3-write/tests/v3_1.rs:19`, V1은 `src/sema.rs:873`에서 필터 의미 검사를 한다.

직접 실행: `r2b_write_probes`, `{read:"MemberAlarm",select:["id"],filter:[{field:"isChecked",op:"eq",value:false}]}`.
```text
R2B read Bool: Err(Reject { code: "BAD_VALUE", msg: "`Bool` 타입에 맞지 않는 값 false" })
```
조치: facts에 노출 가능한 타입/연산과 읽기·쓰기 값 검사를 맞춘다. 지원하지 않는 타입은 facts 단계 또는 계약 단계에서 명확히 거부한다.

### R2B-06 · P1 · Q5/Q6/Q7 · 대상 행 잠금이 관계 권한의 판정과 변경을 직렬화하지 않는다

문제: 모집 행을 잠그고 allow를 판정한 뒤, 다른 연결에서 해당 관리자의 ClubMember 행을 삭제해 commit해도 첫 요청은 모집을 CLOSED로 변경하고 성공한다. 대상 행 잠금만으로 권한에 쓰인 관계 행의 동시 변경까지 원자적으로 보호한다는 주장은 성립하지 않는다. 현재 단일 알림 동시 테스트의 이중 변경 방지는 재확인됐지만 관계 정책을 쓰는 승인/위임으로 일반화할 수 없다.

근거: `spikes/spike-v3-write/src/lib.rs:123`은 `FOR UPDATE OF t`만 걸고 `:128`에서 관계 allow를 읽는다. UPDATE(`:184`, `:191`)는 from만 재평가하며 vis/allow와 그 관계 의존성을 보호하거나 재검사하지 않는다. `spikes/spike-v1-fixture/fixture/recruitment.aip:21`의 managerOf는 ClubMember를 읽는다. E `:15`(RK-04)는 중간 권한 위반을 최종 불변식만으로 정당화하지 않도록 요구한다.

직접 실행: 임시 복사본에서 `PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="/Users/winterholic/development/projects/aip/spikes/spike-v3-write/target" cargo test --offline -q r2b_write_probes -- --nocapture` (exit 0). T1은 actor=1로 `{apply:"Recruitment.close",target:{ids:["100"]}}`, 기본 행 잠금 + 시험용 판정 후 pause 700ms. pg_locks와 pg_stat_activity의 idle in transaction을 확인하여 대상 SELECT가 끝난 뒤 T2에서 `DELETE FROM aip_r2b_write.club_member WHERE club_id=10 AND member_id=1`을 commit했다. T2는 대상 모집 행을 변경하지 않았다.
```text
R2B permission revoked while locked; worker=Ok(Applied { changed: [100], unchanged: [] })
R2B revoked final status=CLOSED
```
조치: V3-2 전에 권한 판정의 기준 시점과 관계 의존 행의 동시 변경 의미를 정하고, 관계/불변식 잠금 또는 격리·검증 방식으로 구현·검증한다. 판정 시점 권한을 작업 종료까지 인정하는 계약을 택한다면 이 결과를 명시하고 ‘최신 정책 판정’ 보장을 좁혀야 한다. UPDATE에 allow를 더하는 것만으로 모든 의존성 경쟁이 해결됐다고 가정하지 않는다. 권한 변경과 진행 중 요청의 순서를 창시자가 반드시 결정해야 한다는 근거는 없으며, 우선 기술 후보로 비교할 사항이다.

### R2B-07 · P2 · Q3/Q5/Q6 · V3 timeout은 INTERNAL이며 요청 전체 deadline도 아니다

문제: 대상 잠금 대기가 2s를 넘으면 V2와 달리 DEADLINE_EXCEEDED가 아닌 INTERNAL(57014)이다. 대상 SELECT와 UPDATE는 각각 statement_timeout을 받으므로 총 작업 시간 2s 보장이 아니고, Rust에서의 계획·대기·응답 처리도 이 제한 밖이다.

근거: `spikes/spike-v3-write/src/lib.rs:147` 고정 timeout, `:148`과 `:194`는 모든 DB 오류를 INTERNAL로 분류한다. V2 `src/lib.rs:31`은 57014를 DEADLINE_EXCEEDED로 매핑한다. V3 시험용 pause(`:149`)도 DB timeout 밖임을 실제 재현에서 확인했다. 해당 Knobs는 시험용이며 호출자 요청 키로 노출되지 않는다.

직접 실행: 별도 연결에서 `BEGIN; SELECT id FROM aip_r2b_write.member_alarm WHERE id=9 FOR UPDATE`로 잠그고 기본 Knobs로 알림 9 읽음 요청, 이후 locker ROLLBACK.
```text
R2B lock timeout 2147ms: Err(Reject { code: "INTERNAL", msg: "대상 조회 실패 Some(SqlState(E57014))" })
```
조치: 오류 분류를 일관되게 하고, 쓰기 budget과 요청 전체 deadline을 정의한다. 계획·전송·commit 시간도 측정 범위에 명시한다. V3-R6(`V3-standard-write-results.md:68`)은 ‘statement_timeout 2s’로만 읽으면 정확하다. 전체 2s 완료 보장으로 넓히지 않는다.

### R2B-08 · P1 · Q1/Q2/Q6/Q7 · 행별 집계는 sourceAccess guard를 집행하지 않는다

문제: V1이 승인한 관리자 전용 sourceAccess를 행별 bookmarkCount에 선언해도 비관리자가 총수를 받는다. 원본 fixture의 totalOfVisible scope에서는 허용된 전체 count라 문제가 드러나지 않지만, 지원하는 facts 전체에 대해 집계 정책을 집행한다는 주장은 깨진다. 요청은 정상 read/select 형식이다.

근거: `spikes/spike-v1-fixture/src/sema.rs:823`은 guard access 호출을 허용하고 groupKey와 함께 쓰는 것을 금지하지 않는다. `spikes/spike-v2-read/src/plan.rs:104`부터 행별 집계는 source/groupKey/where만 읽고 `:114`에서 sourceAccess 범위라는 주석과 달리 guard를 평가하지 않는다. 단독 집계만 `:298`부터 sourceAccess를 평가한다.

직접 실행: 임시 복사본 `r2b_read_probes`. 서버 선언에서 bookmarkCount의 `sourceAccess fixedTotalOfVisibleRecruitment`를 `sourceAccess managerSomewhere(actor)`로 바꾸고 아래 선언을 추가했다. V1 load_str가 성공해 생성한 facts를 그대로 사용했다.
```text
access managerSomewhere(a: Member) = exists ClubMember where member = a and role in (ADMIN, MANAGER)
```
actor=2는 seed에서 ClubMember role=MEMBER이며 ADMIN/MANAGER 행이 없다. `{read:"Recruitment",select:["id","bookmarkCount"],limit:20}` 결과:
```text
R2B row aggregate guard actor2: Ok([Object {"bookmarkCount": Number(3), "id": Number(100)}, Object {"bookmarkCount": Number(1), "id": Number(102)}]); sourceAccess={"args":[{"path":{"root":"actor","segs":[]},"ty":"Ref<Member>"}],"ref":"managerSomewhere"}
```
실제 SQL에 managerSomewhere/ClubMember guard가 없음을 함께 출력해 확인했다. guard 실패 때 null/거부 중 어느 응답을 택하더라도 위 실제 count 반환은 맞지 않는다.

조치: V3-2 전에 행별 집계에서도 sourceAccess를 집행한다. 이 spike를 totalOfVisible만 지원하도록 한정하려면 다른 kind/입력 계약을 V1 또는 planner에서 명시 거부한다. ‘원본 행 정책 미적용’과 ‘승인 sourceAccess 미적용’을 혼동하지 않는다.

## R2-01~03 원래 반례 재확인

원본 V1 CLI `spikes/spike-v1-fixture/target/debug/spike-v1-fixture facts <임시 파일>`을 Python subprocess로 직접 호출했다. fixture 내용만 `/private/tmp/aip-r2b-9gmg4y3w`에 생성했다.

- R2-01: 이전 문서와 같은 raw 삼중 문자열 escaped quote 안에 FAKE=aip/define 선언을 넣었다. Python `ast.parse`로 두 소스 모두 Call AST 0임을 다시 확인했다. E-Py/H-Py 둘 다 exit=1 NO_BLOCK. 근거 `spikes/spike-v1-fixture/src/host.rs:187`, 재발 테스트 `tests/v1_r2.rs:16`.
- R2-02: 원래 중복 input, 중복 output, `predicate dup(m: Member, m: Member) = m.id = m.id`를 각각 실행했다. 모두 exit=1 DUPLICATE. 현재 회귀 테스트의 m=m 변형과 별도로 원래 m.id=m.id 표현도 실행했다. 근거 `src/sema.rs:242`, `tests/v1_r2.rs:30`.
- R2-03: 원래 active→p0, p0→…→p64→p0의 65개 cycle을 실행했다. exit=1 POLICY_CYCLE. 긴 비순환 체인의 양성은 원본 `tests/v1_r2.rs:55`가 위 V1 전체 실행에서 확인됐다. 근거 `src/sema.rs:145`의 방문 집합 탐색.

직접 출력(6회 CLI 각각 exit=1; 핵심 3줄로 묶음):
```text
r2-01.e.py / r2-01.h.py: NO_BLOCK / NO_BLOCK
r2-02-input.aip / r2-02-output.aip / r2-02-predicate.aip: DUPLICATE / DUPLICATE / DUPLICATE
r2-03-cycle65.aip: POLICY_CYCLE
```
판정: R2-01~03은 원래 반례 범위에서 재발 없음. 모든 Python 어휘 규칙·모든 중복 선언·모든 그래프 규모를 보장하는 판정은 아니다.

### R2B-09 · P2 · Q2/Q5 · nullable 필드에 null을 대입하는 정상 전이가 실행되지 않는다

문제: `to internalNote = null`을 V1은 정상 facts로 내보내지만 V3는 INTERNAL로 거부한다. nullable 값 제거는 정상 쓰기 시나리오다.

근거: `spikes/spike-v1-fixture/src/sema.rs:678`은 nullable 대상의 Null 대입을 허용한다. `spikes/spike-v2-read/src/sqlgen.rs:197`의 value는 Null을 처리하지 않는다. V3 `src/lib.rs:119`의 완료 상태 판정 생성부터 실패한다.

직접 실행: 위 `r2b_write_probes`에서 Recruitment.close의 `to status = CLOSED`만 `to internalNote = null`로 바꿨다. V1 load_str 성공 후 정상 ids 요청을 실행했다(이 요청은 SQL 생성에서 먼저 실패하며 DB 행 상태/권한 판정에 도달하지 않는다).
```text
R2B null assignment V1 OK: Err(Reject { code: "INTERNAL", msg: "값으로 쓸 수 없는 식 {\"lit\":null}" })
```
조치: SQL NULL/타입 있는 null 매개변수 생성과 IS NOT DISTINCT FROM 판정을 지원하고, facts 수용과 실행 지원을 맞춘다.

### R2B-10 · P3 · Q5/Q6 · repeat unchanged와 이중 변경 방지는 from/to가 겹치지 않는 fixture 범위다

문제: 허용된 from 식이 목표 상태도 포함하면 같은 요청의 두 번째 실행도 changed다. 잠금과 from 재검사만으로 일반 전이의 멱등성을 보장한다는 문장은 강하다. 원본 알림(false→true)와 모집(PUBLISHED→CLOSED)의 상태 배타성에서는 이 반례가 발생하지 않으므로 P1로 확대하지 않는다.

근거: `spikes/spike-v3-write/src/lib.rs:163`은 done보다 from을 먼저 본다. `:191`은 from만 재검사한다. V1 `src/sema.rs:663`은 from/to 배타성을 보장하지 않는다. `plan-docs/alignment/V3-standard-write-results.md:57`의 무잠금 이중 변경 차단은 이 조건을 생략한다.

직접 실행: `r2b_write_probes`, MemberAlarm의 `from isChecked = false`를 `from isChecked = false or isChecked = true`로 바꾸고 repeat unchanged를 유지했다. 기본 Knobs, 알림 9의 동일 요청 2회:
```text
R2B overlapping from/to request0: Ok(Applied { changed: [9], unchanged: [] })
R2B overlapping from/to request1: Ok(Applied { changed: [9], unchanged: [] })
```
조치: 원본 두 전이의 배타 상태에서만 성립하는 증거로 기록한다. 일반 전이는 done 우선 여부, from/to 배타성 요구, changed의 의미를 정한 뒤 별도 검증한다. 값이 두 번 다른 값으로 바뀐 것은 아니며 같은 true를 다시 UPDATE했다.

R2B-03 추가 출력 크기 실험: Recruitment.internalNote(Text?, 길이 범위 없음)를 DB fixture에서 1,048,576글자로 설정했다. 관리자 actor=1, `select:["internalNote"],limit:1`.
```text
R2B output size: cost=2 rows=1 json_bytes=1048597
```
행 상한과 deadline 안에서 약 1MiB JSON을 성공 반환한다. `src/lib.rs:27`이 모든 결과를 Vec으로 모으고 `:42`에서 JSON을 다시 파싱하며 byte 상한을 확인하지 않는다. 이 결과는 정책 누출이 아니라 승인된 큰 필드의 출력 비용 사례다.

### R2B-11 · P2 · Q1/Q5/Q6 · 숨은 id도 잠가 오류 코드로 존재 여부가 구별된다

문제: actor=1에게 안 보이는 타 학교 모집 101을 다른 트랜잭션이 잠그고 있으면 요청이 INTERNAL(57014)이고, 없는 999는 즉시 MISSING_TARGET이다. 정상 ids 요청의 오류 코드만으로도 이 경합 조건에서 숨은 대상과 없는 대상을 구별한다. 무경합 테스트에서는 둘 다 MISSING_TARGET인 것이 맞다.

근거: `spikes/spike-v3-write/src/lib.rs:128`의 id 대상 WHERE에는 행 정책이 없고 id만 있다. 행 정책은 SELECT 출력으로 계산돼 잠금 뒤 `:156`에서 거른다. `:148`의 timeout 매핑도 차이를 만든다. V3-R1 및 결과 문서 `V3-standard-write-results.md:37,45,63`의 존재 추론 차단은 무경합 범위로 좁혀야 한다.

직접 실행: 임시 복사본 `r2b_write_probes`. T2는 `BEGIN; SELECT id FROM aip_r2b_write.recruitment WHERE id=101 FOR UPDATE`. T1 actor=1이 `{apply:"Recruitment.close",target:{ids:["101"]}}`와 동일 요청의 999 변형을 순서대로 실행했다. 이후 T2 ROLLBACK.
```text
R2B hidden/missing id=101 2151ms: Err(Reject { code: "INTERNAL", msg: "대상 조회 실패 Some(SqlState(E57014))" })
R2B hidden/missing id=999 1ms: Err(Reject { code: "MISSING_TARGET", msg: "대상 1개를 찾을 수 없음" })
```
조치: 비가시 대상이 잠금 대상에 들어가지 않도록 정책 적용 위치를 검토하고 경합 오류의 공개 의미를 일관되게 한다. 공개 id 여부를 공격자가 이미 안다는 가정은 이 판정에 필요하지 않다. 특정 id에 다른 트랜잭션의 잠금이 걸린 경계 조건이 필요하며, 평시 모든 숨은 id의 존재를 구별한다는 주장은 아니다.

### R2B-12 · P3 · Q2/Q6 · V2-R3의 ‘NULL이면 거짓’은 조건 합성 의미로는 부정확하다

문제: 경로 대 경로 `=`/`!=`는 NULL이면 SQL UNKNOWN이며 NOT을 붙여도 UNKNOWN이다. WHERE/CASE에서 TRUE가 아니라서 제외되는 것을 Bool false와 같은 값이라고 쓰면 NOT/OR 정책을 잘못 이해할 수 있다. 명시적 `= null`은 IS NULL로 번역돼 별도 의미다.

근거: `spikes/spike-v2-read/src/sqlgen.rs:230`의 null 리터럴 특별 처리, `:237`의 일반 비교, `:226`의 NOT. `V2-caller-read-results.md:67`(V2-R3)의 설명은 두 경로를 구분하지 않는다. V1 `src/sema.rs:377`은 nullable 대상의 null 비교를 허용한다.

직접 실행: `r2b_read_probes`에서 Recruitment의 active 부분을 유지하고 학교 조건만 아래처럼 각각 바꾼 익명 read 요청(select id, limit 20).
```text
R2B NOT null equality: Ok([])
R2B explicit nonnull: Ok([Object {"id": Number(100)}, Object {"id": Number(101)}])
```
첫 식은 `not (club.school = actor.school)`, 둘째는 `club.school != null`이다. 익명 actor.school이 NULL이므로 첫 식은 FALSE의 부정인 TRUE로 되지 않는다. 원본 fixture의 익명/학교 없는 회원 결과는 의도와 맞으며 새 행 노출 반례는 아니다.

조치: UNKNOWN과 최종 허용 판정을 구분하고 명시 null 비교·NOT·OR의 진리표를 계약에 적는다. null-safe equality로 바꾸는 것은 별도 의미 결정이다.

## Q1~Q7 종합 답변과 확인 한계

### Q1. 정책 집행과 응답 차이

원본 fixture에서 정책상 숨은 행/메모/관계/집계 값의 부당 반환은 없음. 확인 범위는 `spikes/spike-v2-read/tests/v2.rs:145`의 actor 1·2·3·4·익명, 상태/기한/학교 조합, `:155`의 관리자 메모, `:179` 및 `:185`의 관계 정책 변형, `:190`의 단독 집계다. **V1이 승인하는 정상 계약 변형까지 넓히면 R2B-08의 실제 count 노출이 있다.** V3의 관계 권한 경쟁은 R2B-06, 경합 오류를 통한 숨은 행 판정은 R2B-11이다.

응답으로 드러나는 정책 판정은 다음과 같다.

| 응답 관찰 | 실제 확인 및 의미 | 근거 |
|---|---|---|
| internalNote 값/null | 관리자에게 A-note, 비관리자는 null. null 자체는 값 없음/권한 없음 구별 불가. non-null이면 허용 결과를 알 수 있음 | V2 `tests/v2.rs:155,166`, `src/plan.rs:122` |
| club 객체/null | 원본 연합 Club 객체가 엄격/누락 행 정책 변형에서 null. 필수 FK 관계라는 계약을 아는 호출자는 대상 관계 정책이 TRUE가 아니었음을 추론 가능 | V2 `tests/v2.rs:181,187`, `src/plan.rs:173` |
| 보이는 행 수/정렬 | A대 {102,100}, B대 {102,101}, 익명/학교 없음 {102}. 반환 집합은 행 정책의 TRUE 결과를 드러냄 | V2 `tests/v2.rs:145` |
| bookmarkCount | 모집 100의 총수 3에는 타 학교 회원 북마크도 포함. 고정 승인 scope의 의도된 결과이며 개인 행 노출과 같지 않음 | V1 fixture `:60`, V2 `tests/v2.rs:158` |
| 단독 집계 값/null | 관리자 2, 비관리자·익명 null. 권한 결과는 구분되나 비관리자·없는 동아리·남의 동아리 사이 결과는 동일. 오류 API 주장은 R2B-01 | V2 `tests/v2.rs:190`, 임시 read probes |
| MISSING_TARGET/FORBIDDEN/INVALID_STATE | 무경합 단일 요청에서 숨은 행/없는 행은 같고 보이는 행은 쓰기 권한·상태가 구별됨. MISSING_TARGET msg는 누락 수를 반환하며 단일 id 요청은 그 id의 가시성 결과를 알 수 있음. 경합 예외는 R2B-11 | V3 `src/lib.rs:156,160,168,176` |
| changed/unchanged id | 승인된 대상의 이전/목표 상태가 구별됨. 읽기 가능한 행의 쓰기 권한을 별도로 적용 | V3 `tests/v3_1.rs:86`, `src/lib.rs:163` |

위 차이가 존재한다는 사실만으로 전부 정책 위반이라고 분류하지 않는다. 관계 null/필드 null/명시적으로 허용한 count는 이번 후보의 공개 의미다. 일반적인 count 차분 추론 안전성은 확인 못함: 고정 fixture 결과와 guard 누락 반례를 실행했으며 모든 외부 보조 정보·시간에 따른 변화까지 분석하지 않았다(E RK-02의 남은 관문).

### Q2. 정책 SQL의 의미·변수·별칭

- NULL: R2B-09·12 참조. 원본 학교 정책의 `= null`은 IS NULL, 학교끼리 `=`는 UNKNOWN을 허용하지 않는 방식으로 실행됐다. 비교식의 모든 Bool 대수에서 UNKNOWN=FALSE로 취급한다고 해석하면 안 된다.
- predicate 인라인: `sqlgen.rs:252`가 this/input을 비운 새 Env를 만들고 선언된 인자만 현재 환경에서 평가해 넣는다. 호출자 this/지역 변수의 우발적 캡처를 이 확인 범위에서는 관찰하지 않았다. 인자를 교환하는 `predicate swapped(c: Club, m: Member) = managerOf(m, c)`를 추가해 내부 메모 정책을 swapped(club,actor)로 대체한 실제 결과:
```text
R2B predicate swapped actor=Some(1): null=false text_len=1048576
R2B predicate swapped actor=Some(2): null=true text_len=0
R2B predicate swapped actor=Some(3): null=false text_len=6
```
- exists: `sqlgen.rs:260`이 `xN` 새 별칭을 만들고 Env를 복제한 다음 this만 대상 resource로 바꾼다. managerOf의 기존 exists 안에 `exists Member where id = m.id`를 추가한 변형은 actor=1에 모집 100의 메모만 반환했다. SQL에 x2/x3가 별도로 생기고 바깥 t.club_id/매개변수 m이 보존됐다. 반례 없음, 이 중첩과 원본 managerOf 범위만 확인했다.
- 중첩 ref subquery: `sqlgen.rs:142`의 z 별칭 재사용은 이 경로에서는 오류 없음. Apply 집계 where에 `recruitment.club.school.id = actor.school.id`를 더해 2중 z subquery를 실행했고 actor=1/club10의 결과 2를 유지했다. 가장 안쪽 상관은 s2.recruitment_id여서 바깥 z로 잘못 결합하지 않았다. 모든 임의 길이의 경로를 보장한 검증은 아니다.
- V1/V2 수용 범위: R2B-02·05·09와 같이 typed facts가 나온다는 사실만으로 SQL 생성 가능성을 보장하지 않는다. 회귀 cycle은 거부되지만 긴 비순환 predicate를 V2가 실행할 수 있는지는 별도 한도다(`sqlgen.rs:207`은 32). 지원하지 않는 facts를 실행 전에 명시적으로 진단해야 한다.

### Q3. rows/depth/cost/deadline

원본 요청의 limit 51→ROWS_EXCEEDED, limit 0→BAD_VALUE, depth 초과→DEPTH_EXCEEDED, cost 50 변형→COST_EXCEEDED, 400,000행/1ms→DEADLINE_EXCEEDED는 모두 실제 원본 V2 테스트에서 확인했다(`tests/v2.rs:57,80,83,223`). 반환 행 LIMIT는 실제 SQL에 바인딩돼 있으며, depth는 이번 단일 관계 구현에서 깊이 2만 허용한다. depth를 높여도 다단 관계를 지원하지 않는 것은 결과 문서의 명시된 미지원 범위(`V2-caller-read-results.md:85`)다.

실패하는 자원 통제 사례는 R2B-03(구조 크기·출력 bytes)과 R2B-07(쓰기 요청 전체 deadline)이다. filter/sort와 정책 식의 실제 비용·index·결과 bytes는 휴리스틱에 반영되지 않는다. rows는 반환 수 상한이며 스캔/집계 입력 수 상한이 아니다. 단독 집계는 `plan.rs:320`의 2s, cost=1이며 계약 budget을 검사하지 않는다. 이 공백은 작성자가 이미 적었다(`V2-caller-read-results.md:76`).

PostgreSQL 내부 계획 시간이 자동으로 statement_timeout 밖이라고 단정할 근거는 없다. PG17 공식 문서의 timeout은 서버의 Parse/Bind/Execute/Describe부터 계산하므로 DB 내부 parse/plan도 그 SQL 명령 제한에 포함되는 방향이다. 반면 Rust plan_read·JSON 요청 처리·모든 결과를 받은 뒤 JSON 파싱은 밖이다. [PG17 statement_timeout 정의](https://www.postgresql.org/docs/17/runtime-config-client.html#GUC-STATEMENT-TIMEOUT).

확인 못함: 실제 PostgreSQL 계획 시간을 분리한 계측, index 유무 비교, 1:N fan-out, 지속 동시 부하, 반환 중 네트워크 정체, 메모리 소진 한계. actor/tenant 인증은 구현이 없어 교차 tenant 정확성도 확인 못함. 로컬 `psql 'host=localhost dbname=postgres' -X -qAt -c 'SHOW server_version; SHOW default_transaction_isolation; SHOW statement_timeout;'` 결과는 아래와 같다.
```text
17.11 (Homebrew)
read committed
0
```
마지막 0은 별도 기본 세션이며 spike 트랜잭션의 SET LOCAL을 부정하는 값이 아니다.

### Q4. 요청 값과 식별자의 SQL 경로

검토 범위의 SQL 주입 반례는 없음. 요청 값은 V2 `plan.rs:201,210,290`와 V3 `src/lib.rs:104,126,189`에서 타입 검사/수치 정규화 후 Params.bind를 거친다. enum/actor/now도 `sqlgen.rs:150,181,191`에서 바인딩한다. 원본 SQL 표시 및 Time/집계 입력 주입 음성 대조를 직접 실행했다. V3 ids는 문자열을 i64로 파싱하고 숫자로 다시 직렬화한 배열 하나를 매개변수로 보낸다.

식별자 후보는 요청 read/apply resource 이름·select/filter/sort/traverse 필드 이름에서 온다. V2 `plan.rs:76,101,136,197,221`, V3 `src/lib.rs:55,101`에서 facts 또는 expose 허용 목록에 정확히 매칭된 것만 SQL에 쓰므로 임의 호출자 식별자는 들어가지 않는다. sort 방향·연산자는 고정 match다. 엄밀히는 ‘식별자가 요청에서 오지 않는다’가 아니라 ‘요청이 선택한 신뢰 선언의 식별자만 들어간다’다.

SQL 텍스트에 직접 들어가는 값도 있다: trusted facts의 식별자/JSON key, 숫자 deadline, DDL enum 리터럴·범위·schema 이름(`sqlgen.rs:33,183,305,309`). DDL 전용 inline과 set_schema는 서버/테스트 설정이며 요청 경로에 노출되지 않는다. 요청 값을 그대로 SQL 값 리터럴로 쓰는 경로는 없음. Time의 문자열 검사(`plan.rs:57`)는 달력 전체 검증이 아니며 나머지 PG cast 오류는 V2에서 BAD_VALUE가 된다. V3 타입 지원 차이는 R2B-05·09.

### Q5. V3-1 원자성·대상·동시 실행·오류

원본 fixture의 누락·중복·상한·allow/from 실패는 일부 변경 없이 거부됐다. `tests/v3_1.rs:89`의 혼합 내/남의 알림, `:91`의 없는 id, `:93`의 중복, `:97`의 where 상한, `:111`의 읽기 가능/쓰기 불가, `:121`과 `:127`의 잠금/무잠금 2요청을 직접 실행했다. `src/lib.rs:154`부터 모든 행을 판정하고 `:181` 이후에만 UPDATE하며, count 불일치는 CONFLICT로 commit하지 않는다. 이 범위의 누락·중복 성공 처리 반례는 없음.

where는 PK 행들을 선택해 중복 없고, 행 정책과 allow를 SQL WHERE에 적용한다. from-only 문서와 다른 OR done 및 bulk 의미는 R2B-04. 읽기 정책 밖으로 나간 마감 재시도는 원본에서 MISSING_TARGET이며 이미 작성자가 기록했다(`tests/v3_1.rs:117`, 결과 `:55`). 관리 화면 요구를 고치기 위해 읽기 권한을 넓히는 것이 유일한 해법이라고 확정하지 않는다. 별도 쓰기 대상 가시성/결과 재시도 계약도 비교할 수 있다.

동시성 보장은 원본 2개 전이의 같은 행 경쟁까지다. 관계 권한 경쟁은 R2B-06, from/to 겹침은 R2B-10, 숨은 행의 잠금 경합은 R2B-11, timeout 오류는 R2B-07이다. `db.transaction()`은 격리 수준을 직접 고정하지 않는다(`src/lib.rs:146`). 이 로컬 기본값은 read committed로 확인됐으므로 V3-R5는 이번 실행 환경에서는 맞으나 모든 DB 설정에서 같은 수준을 강제한다는 주장은 아니다.

확인 못함: 권한·from/불변식의 여러 관련 행 동시 변경 전수, 다중 대상 잠금의 교착, 직렬화 격리 비교, connection/commit 응답 유실, tenant 간 혼합, 삭제·생성·관계 연결 쓰기. 시험용 skip_row_lock/pause는 코드 손잡이이며 사용자 JSON의 모르는 키는 거부된다.

### Q6. 결과 주장·규칙 14개와 창시자 지침

기준 문서는 `plan-docs/sources/founder-integrated-directive-2026-10-03.md`다. §0(`:17,23`)의 FOUNDER/DIRECTION/OPEN 분리, 필수 실행 안전성 §7.2(`:451`), 표준 쓰기/조합 §9(`:550,552`), 부분 성공 후보(`:610,612`), 오류 상세도(`:686`)를 비교 기준으로 삼았다. 두 결과 문서 모두 첫머리에 검증 필요·모델 결정 아님을 명시하고 있어 규칙 자체를 창시자 승인으로 승격한 직접 충돌은 없음. 다만 실험 한계를 제외하고 보편 보장으로 채택하면 필수 안전성 원칙과 맞지 않는다.

| 규칙 | 실행 근거에 맞는 범위 / 수정·결정할 부분 | 지침과의 관계·결정 주체 |
|---|---|---|
| V2-R1 필드 null | 원본 CASE와 nullable/redactable 타입은 확인. 정책 실패와 실제 null은 섞임 | 직접 충돌 없음. 공개 응답에서 null/생략/오류를 택하는 UX 계약은 기술 비교 후 제품 결정 대상 |
| V2-R2 관계 null | 대상 rowRead 재적용은 확인. 필수 FK도 null 응답이 가능하며 정책 결과를 추론할 수 있음 | 직접 충돌 없음. SDK 타입·관계 오류 표시와 함께 제품 의미를 확정할 사항 |
| V2-R3 NULL 비교 | R2B-12의 UNKNOWN과 명시 null 비교를 구분해야 함 | 창시자 지침이 equality 의미를 정하지 않았음. 기술 계약 검증 사항이며 당장 창시자 승인 요구 사유 없음 |
| V2-R4 default limit / 초과 거부 | 원본 rows 상한과 조용한 절단 거부는 실행 근거 있음. 처리 중 스캔/출력 bytes 상한으로 확대 불가 | 서버 자원 책임과 부합. batch/cursor 제공 범위는 나중의 제품 선택 |
| V2-R5 id ASC | 생성 SQL은 확인. 고정 데이터의 tie-break만 보장, 페이지 이동 중 데이터 변화/커서 안정성은 미검증 | 직접 충돌 없음. 현재 구현이 페이지네이션 제품 계약 전체를 확정하지 않음 |
| V2-R6 cost 휴리스틱 | 구현·cost 거부는 확인. 구조/bytes/실제 DB 비용은 제한 못함(R2B-03) | 실험용 한계를 밝힌 후보는 충돌 없음. RK-03 관문 통과나 운영 안전성 보장으로 승격하면 부적절 |
| V2-R7 ACCESS_DENIED | 실제 실행 API는 Ok([null])(R2B-01). 테스트 내부 변환만 확인 | 존재 추론 차단 방향과 부합하지만 오류 계약의 근거는 부족. 후보 구현 정정 후 공개 오류 의미를 선택 |
| V2-R8 DB 원문 비공개 | query 오류를 sqlstate로 분류하는 V2 경로는 확인. 최종 와이어/로그 계약·일반 DB 실패 전수는 미검증 | 오류 상세 제한 DIRECTION(`:686`)과 부합. V3도 같은 계약인지 별도 조정 필요 |
| V3-R1 MISSING_TARGET | 무경합 숨은/없는 행은 동일. msg의 누락 수와 경합 오류 차이가 남음(R2B-11) | 후보는 직접 충돌 없음. 존재 추론을 전부 차단했다는 주장은 기각. 공개 정보 수준은 제품 정책 결정 대상 |
| V3-R2 FORBIDDEN | 보이는 모집의 학생/관리자 차이 확인. 이미 읽기 가시성이 있는 행의 쓰기 권한 결과를 알려 줌 | read≠write를 분리하므로 서버 인가 책임과 부합. 오류 상세도 계약과 함께 선택 |
| V3-R3 where 집합 | 문서 from-only와 코드 from OR done이 다름(R2B-04). from/to 밖 행은 조용히 대상 제외, id 모드와 상태 오류 의미 다름 | C B.5의 ‘일부만 바꾸고 전체 성공 금지’와 대상 집합 정의가 맞는지 먼저 명시해야 함. 특정 id 집합과 정책 교차 집합은 같은 ‘모두’가 아님 |
| V3-R4 read filter + eq | 쓰기 eq 구현은 확인, Bool read/where 불일치(R2B-05). write-only resource도 read 계약을 추가하게 함 | 서버 반복 최소화 원칙 관점에서 read 계약 재사용과 별도 write target 계약의 비용을 비교할 기술 사항. 이 결합이 창시자 요구라는 근거 없음 |
| V3-R5 READ COMMITTED + locks | 로컬 default/read와 대상 행 잠금·from 재검사 확인. 격리를 코드에서 고정하지 않으며 관계 권한 경쟁은 R2B-06 | 현재 좁은 증거는 부합. 승인·위임까지 ‘판정/변경 완전 원자성’으로 일반화하면 RK-04를 충족하지 않음 |
| V3-R6 고정 2s | 각 SQL의 statement_timeout은 확인. timeout 오류/전체 요청 deadline은 R2B-07 | 개발자 임의 코드에서도 자원 계약을 지킬 서버 책임이 기준. budget 위치·요청 시간 정의를 기술적으로 설계해야 하며 2s 자체가 창시자 결정은 아님 |

실행 근거보다 강한 결과 문장:

1. V2 `:28`의 ‘facts가 SQL 생성에 충분’은 원본 fixture에 한정해야 한다. 수용 facts 전체의 Bool/null/guard 변형은 R2B-02·08·09로 깨진다.
2. V2 `:55,71`의 ACCESS_DENIED는 실제 executor의 오류가 아니다(R2B-01).
3. V3 `:57`의 ‘from 재검사만으로 이중 변경은 막힌다’는 from/to 배타 전이 범위다(R2B-10).
4. V3 `:51,67`의 판정과 변경은 같은 트랜잭션이라는 사실과 관계 권한 변경까지 직렬화한다는 보장을 구분해야 한다(R2B-06). 같은 트랜잭션 rollback은 실제로 확인됐다.
5. V3 `:65`의 대상 집합 from-only 설명은 실제 OR done과 다르다(R2B-04). `:63`의 존재 추론 차단은 경합 사례를 포함하면 강하다(R2B-11).
6. V2 `:4`의 schema를 만들고 지운다는 설명은 시작 DDL의 삭제/재생성만 자동화돼 있다. 종료 삭제는 이 검토에서 별도로 수행한다. V3 결과 문서에는 종료 삭제 보장이 없다.
7. EQ-10은 단독 집계 접근까지만 재현됐고 `Recruitment.stats`의 공식 확장 실행/효과/출력 검증은 구현이 없어 확인 못함. V2 table에서 EQ-10 표시가 전체 공식 확장 계약 실증을 뜻하지 않도록 범위를 좁힌다.

작성자가 했다는 고장 주입 이력은 확인 못함: 이 검토는 원본 테스트와 추가 반례를 직접 실행했으며, 당시 모든 mutation을 다시 만들어 같은 실패 단언까지 재실행하지 않았다. 원본 테스트 코드에 방어 장치/단언이 존재하는 사실과 과거 mutation 이력이 실제로 수행됐다는 사실을 구분한다.

창시자에게 지금 새 승인을 요구해야 하는 확정 충돌은 없음. 공통 실행의 정책 누락·반례 차단은 기술적으로 먼저 해결한다. 이후 null/오류/재시도 공개 의미, 정책 변경과 진행 중 쓰기의 기준 시점, write 대상의 ‘모두’ UX가 제품 경험에 영향을 주면 비교 자료와 함께 결정받을 수 있다. W0만 영구 표준으로 제한하거나 W1/W2 안전성이 승인됐다고 쓰지 않는다(통합 지침 `:550,552`; C `:287`).

### Q7. V3-2 전에 고칠 것과 다음 실험의 확인 조건

**P1 관문 두 건:**

- R2B-08: 행별 집계 sourceAccess 집행 또는 미지원 facts의 명시 거부. guard false일 때 실제 count를 반환하지 않는 음성 대조와 totalOfVisible의 기존 공개 count 양성 대조를 함께 실행한다.
- R2B-06: 쓰기 권한/불변식의 관계 의존성과 동시 변경 기준을 정한다. actor/ClubMember 권한 취소, 대상 scope 변경, 동시 회원 생성의 관계 행/유일성 보호를 검증한다. 대상 행 잠금만으로 충분하다고 가정하지 않는다. 승인 실험을 확장하기 전에 수정 또는 검증 가능한 좁은 보장 계약을 마련한다.

**P2 권장 조치:** 실제 오류 API(R2B-01), Bool/nullable 값 지원(R2B-02·05·09), 요청 구조·bytes·전체 deadline(R2B-03·07), where 상태/상한 의미(R2B-04), 숨은 대상 잠금/오류 노출(R2B-11)을 작은 단위로 보완한다. P3인 from/to 배타성과 NULL 진리표(R2B-10·12)는 결과 보장 조건으로 명시한다.

V3-2 W0/W1/W2는 `V3-standard-write-plan.md:57`의 같은 반례 표를 유지한다. 특히 누락/중복/빈 id, 다른 동아리 혼합, 자기/동시 위임, 값 출처, 동시 중복 회원, 단계 사이 권한 변경, rollback과 afterCommit/outbox 실패를 같은 조건에서 대조한다. 최종 DB 불변식만 맞았다는 이유로 중간 권한 위반을 허용하지 않는다(E RK-04). 정상 승인 시나리오의 서버 정의 양과 클라이언트 표현 변경도 원자료로 비교한다. 이 검토는 V3-2 코드를 구현하거나 후보 문법을 채택하지 않았다.

## 판정

원본 V1/V2/V3-1 테스트의 성공 결과는 재현됐고, R2-01~03은 원래 반례에서 다시 거부됐다. 원본 모집·알림 fixture의 읽기 정책과 단일 행 상태 경쟁은 해당 실행 범위에서 성립한다. V1이 허용하는 정상 계약 변형의 집계 guard 누락과 관계 권한 동시 변경은 다음 단계 전 다뤄야 한다. **발견 12건: P1 2건, P2 8건, P3 2건.** RK-01~04 전체 통과·일반 정책 lowering 정확성·승인 조합 안전성은 이 테스트 수만으로 보장되지 않는다. 파일·코드 변경은 이 검토 문서만이며 코드 수정·git 작업은 수행하지 않는다.

## 종료 정리·문서 검증

허용 schema 및 임시 schema만 삭제했다. 실행 명령:
```sh
PGOPTIONS='--client-min-messages=warning' psql 'host=localhost dbname=postgres' -X -qAt -v ON_ERROR_STOP=1 -c "DROP SCHEMA IF EXISTS aip_v2_spike CASCADE; DROP SCHEMA IF EXISTS aip_v3_spike CASCADE; DROP SCHEMA IF EXISTS aip_r2b_read CASCADE; DROP SCHEMA IF EXISTS aip_r2b_write CASCADE; SELECT 'review_schema_remaining=' || count(*) FROM pg_namespace WHERE nspname IN ('aip_v2_spike','aip_v3_spike','aip_r2b_read','aip_r2b_write');"
```
직접 출력(exit 0):
```text
review_schema_remaining=0
```
임시 재현 테스트 최종 실행 결과: read probes 1 passed(4 filtered), write probes 1 passed(1 filtered). write probe 작성 중 Plan의 Debug 미구현으로 첫 컴파일이 실패했으며 출력 대상을 sql/Reject로 바꾼 임시 테스트에서 위 반례를 실행했다. 원본 코드/원본 테스트는 바꾸지 않았다.

최종 문서 검사 명령: `python3 /private/tmp/aip-r2b-9gmg4y3w/verify_review.py`. 발견 번호/심각도, Q1~Q7, 규칙 14개, 명시 파일 참조의 존재와 줄 범위, fence 균형, 원본과 임시 복사본의 소스·fixture·manifest 일치(추가 probe 테스트 제외), DB schema 잔존 수를 검사했다. 문서 검증기의 첫 실행은 repository root를 상위 디렉터리로 잘못 잡아 실패했고 경로를 고친 뒤 아래 결과를 얻었다.
```text
review_check: 12 findings (P1=2, P2=8, P3=2); Q1-Q7 and 14 rules present
source_copy_check: unchanged sources/fixtures/manifests; review_schema_remaining=0
```
이 구조 검사는 반례의 의미를 독립적으로 다시 판단하는 검토를 대신하지 않는다. 테스트 성공/실패와 실제 응답은 위 각 실행 기록에 남겼다.
