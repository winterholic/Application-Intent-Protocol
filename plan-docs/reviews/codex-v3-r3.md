# 쓰기 실험 정확성 검토 (r3)

기준일: 2026-10-04. 기준은 창시자 통합 지침 §9, E RK-04, C B.5·B.6이다. 작성자 주장은 미검증으로 출발했다. 이 문서만 작성하며 git·.env·비밀 저장소에 접근하지 않는다. 발견은 검증 즉시 append한다. 변형은 `/private/tmp` 복사본과 이 검토가 만든 별도 schema에서 실행한다.

## 직접 실행 기록

실행 전 localhost PostgreSQL 조회: 버전 `17.11 (Homebrew)`. 원본 테스트가 사용하는 `aip_v2_spike`, `aip_v2_r2b`, `aip_v3_spike`, `aip_v3_r2b`, `aip_v3_2` schema는 모두 없었다. 따라서 이번 테스트가 만드는 schema만 사용한다.

각 spike 디렉터리에서 `PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`를 직접 실행했다. 세 명령 모두 exit 0.

V1 (`spikes/spike-v1-fixture`, 합계 15 tests):
```text
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.36s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

V2 (`spikes/spike-v2-read`, 합계 5 tests):
```text
db scenarios ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.30s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
```

V3 (`spikes/spike-v3-write`, 합계 3 tests, 순서 v3_1 / v3_2 / v3_r2b):
```text
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.84s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.93s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.67s
```

원본 공통표 8개 사례×3후보, 후보별 11개 사례, 경쟁 2개 사례×W2/W1을 실행했다. 정상 승인의 회원 상태는 같지만 outbox는 W2만 2행이다. 경쟁에서는 W2/W1 각각 1건 성공·회원 1행, W2 outbox 1행이다. 원본 테스트에 없는 W0 경쟁도 아래 보조 실험으로 추가 실행했다.

## 발견 (검증 순서)

### R3-01 · P2 · Q1 · 공통표의 동일 오류는 동일 판정 순서의 증거가 아니다

문제: 복합 반례에서 W1과 W0/W2가 다른 오류를 반환한다. W1은 sameScope를 전이 allow/from보다 먼저 검사한다. 결과 문서의 ‘같은 범위 검사는 가시성·권한 판정 뒤’는 W1의 전이 권한까지 포함하면 부정확하다.
영향: 현재 fixture의 rowRead 자체가 운영진 검사라 단일 반례에서는 차이가 가려진다. read 가시성과 write 권한이 다른 정상 선언에서는 권한 없는 전이의 범위 오류가 먼저 나온다. 이번 반례들은 모두 rollback됐으며 비인가 쓰기 성공은 관찰하지 않았다.
조치: 비교 결론을 단일 반례 표 범위로 좁히고, 복합 오류 우선순위와 W1의 사전 범위 판정 계약을 고정한다. 기술적으로 비교할 문제다.

근거: `spikes/spike-v3-write/src/lib.rs:203`의 allow/from 판정 → `:218`의 누락 검사 → `:228`의 sameScope 검사. W1은 `:558`의 누락 검사 → `:562`의 sameScope 검사 → `:570`의 전이 judge다. 결과 문서 `plan-docs/alignment/V3-standard-write-results.md:140`.

임시 복사본 `tests/r3_probe.rs`의 직접 실행 입력·결과:
```text
actor=8, ids=[304(REJECT,club10),302(PENDING,club11)]: W2 INVALID_STATE / W0 INVALID_STATE / W1 NOT_SAME_SCOPE
actor=1, ids=[304,999]: W2 INVALID_STATE / W0 INVALID_STATE / W1 MISSING_TARGET
rowRead=true로만 바꾼 facts, actor=1, ids=[300,302]: W2 FORBIDDEN / W0 FORBIDDEN / W1 NOT_SAME_SCOPE
```
세 후보의 각 요청은 원본 `spikes/spike-v3-write/tests/v3_2.rs:76`의 approve_req와 같다. 마지막 변형은 V1에서 허용하는 Bool 정책과 동일한 typed facts다. 원래 Apply rowRead가 운영진 검사라는 사실(`spikes/spike-v1-fixture/fixture/recruitment.aip:87`) 때문에 기존 단일 표에서 W1의 순서 차이가 노출되지 않았다.

### R3-02 · P2 · Q3/Q5 · ‘승인 업무’ 비교에서 생성 생략과 회원 역할의 의미가 다르다

문제: check를 켠 W0·W1도 승인만 실행하고 회원을 만들지 않아도 성공한다. 또한 정상 요청의 role을 ADMIN으로 바꾸면 지원자를 관리자 회원으로 생성한다. W2는 회원 생성과 MEMBER 역할을 고정한다.
영향: 현재 allow/check는 선언대로 집행되지만, 승인 업무의 회원 생성·역할 불변식까지 동등하게 강제한 비교는 아니다. ‘승인된 지원이 있어야 회원 생성’은 ‘승인하면 반드시 MEMBER 회원 생성’의 역방향을 보장하지 않는다. 선언된 정책 우회로 단정하지 않는다.
조치: 비교할 업무의 필수 생성·역할 조건을 먼저 맞춘다. W1에는 생성 필수 여부와 필드별 허용 상수/역할 정책이 추가로 필요한지 실험한다. 자유로운 단계 생략이 의도라면 그 범위와 업무 완결성을 분리해 기록한다.

근거: fixture `spikes/spike-v3-write/tests/v3_2.rs:17`의 W2 create role=MEMBER, `:31`의 W0/W1 create allow에는 role 조건 없음, `:33`의 check는 approvedApplicant만. 엔진 `src/lib.rs:534`는 참조 필드의 상수만 막고 enum 상수는 `:409`에서 enum 전체를 허용한다. `:567`은 선택한 단계만 수행하고 `:593`의 check는 생성하지 않은 회원의 존재를 강제하지 않는다.

직접 실행(원본 check 유지):
```text
W1 steps=[{transition:approveOnly}], ids=[300]: OK [300]+0 | approved[300] members[10:2] outbox0
W0 atomic=[{apply:Apply.approveOnly,target:{ids:[300]}}]: OK [300]+0 | approved[300] members[10:2] outbox0
W1 role.const=ADMIN / W0 role=ADMIN: 둘 다 OK [300]+1 (아래 추가 기록에서 DB role=ADMIN 조회)
```

R3-02 역할 DB 확인 추가:
```text
R3 role-confirm W1 member=5 role=ADMIN
R3 role-confirm W0 member=5 role=ADMIN
```

### R3-03 · P1 · Q2/Q6 · 커밋 검사는 의존 행 변경을 보호하지 않는다

문제: 회원 생성의 hasApproval 검사가 성공한 뒤 관련 Apply 승인을 다른 트랜잭션이 철회해도, 회원 생성 트랜잭션이 승인 근거 없는 회원을 commit한다(아래 check-race). 순차적으로도 Apply 상태만 APPROVE→REJECT로 바꾸면 기존 회원의 check는 다시 평가되지 않는다.
영향: 현재 승인 전용 표에는 역방향 전이가 없어 드러나지 않는다. 동시 반례에서는 바로 이번 트랜잭션이 새로 쓴 회원의 check도 최종 commit 시점에 거짓이다. 순차 반례는 직접 수정 행에만 적용되는 사후조건을 전역 관계 불변식으로 확대할 수 없음을 보여 준다. savepoint 문제와 별개다.
조치: 다음 실험 전에 검사한 관련 행과 조건이 commit까지 유지되는 잠금/격리 방식을 구현·대조한다. 그와 별도로 check를 ‘수정된 행의 사후조건’으로만 한정할지, 의존 변경에 영향받은 기존 행까지 재검사하는 불변식으로 집행할지 기술 계약을 정한다.

근거: `spikes/spike-v3-write/src/lib.rs:358`은 검사 resource 자체의 xmin만 고른다. `spikes/spike-v3-write/tests/v3_2.rs:33`의 approvedApplicant는 Apply·Recruitment를 읽는다. Apply만 수정하면 기존 ClubMember의 xmin은 현재 트랜잭션이 아니며 검사에서 빠진다. V3-R8(`plan-docs/alignment/V3-standard-write-results.md:169`)의 제한은 기록돼 있으나 의존 행 변경으로 기존 불변식이 깨지는 결과까지 설명하지 않는다.

직접 재현: 임시 서버 선언에 정상 문법의 아래 전이만 더하고 V1 load_str 성공을 확인했다. unique·hasApproval·권한 잠금은 그대로 유지했다.
```text
transition retract { allow managerOf(actor, recruitment.club); from status = APPROVE; to status = REJECT }
expose apply retract { target id; bulk maxRows 100 }
```
actor=1이 W2로 지원 300을 승인·회원 생성한 뒤 `{apply:"Apply.retract",target:{ids:["300"]}}` 실행:
```text
R3 dependency-only => OK [300] | approved[] members[10:2,10:5] outbox1 | invalid_members=1
```
invalid_members는 member=5에 대해 같은 club·member의 APPROVE 지원이 없는 회원 행 수를 별도 SQL로 조회한 값이다. 원본 fixture에 retract가 이미 공개돼 있다는 주장은 아니다.

### R3-04 · P2 · Q1/Q5 · W0의 실효 일괄 승인 상한은 100이 아니라 19다

문제: 원본 W0 표현은 승인 1연산 + 회원별 create N연산이다. 엔진은 atomic 연산을 20개로 제한하므로 20명부터 BAD_REQUEST며, 선언 bulk maxRows 100과 다른 용량을 갖는다.
영향: 원본 공통표는 최대 2명이라 후보별 실효 처리 범위 차이를 검증하지 않았다. ‘같은 일괄 승인’ 범위와 요청량 비교에 영향을 준다.
조치: 후보별 동등 업무 상한으로 비교하거나 실효 상한을 별도로 공개한다. 묶음 연산 제한과 대상 수 제한의 관계는 기술 계약이다.

근거: `spikes/spike-v3-write/src/lib.rs:432`의 atomic 1~20개 제한, `spikes/spike-v3-write/tests/v3_2.rs:95`의 대상마다 create 추가, `:27`의 bulk maxRows 100.

직접 입력: 원본 approve_req로 ids=[400..419] 20개(모두 없는 id)를 보냈다. 형식/용량 검사와 DB 대상 검사를 분리하기 위해 없는 id를 사용했다.
```text
W2 MISSING_TARGET / W0 BAD_REQUEST / W1 MISSING_TARGET
```
W0은 DB 대상 판정 전에 21개 연산이라는 이유로 거부한다. 20명의 실제 정상 승인 데이터로 성공 한계까지 측정한 것은 아니다. 상한 산식은 루프와 검사에서 직접 확인했다.

R3-03 동시 실행 추가: 임시 엔진 복사본에서 bundle의 `run_checks` 성공 직후에만 700ms pause를 넣었다(검사·잠금·UPDATE·commit 의미는 유지). pg_stat_activity의 idle in transaction과 마지막 check SQL로 그 지점에 도달했음을 확인했다. T1은 이미 APPROVE인 지원 305를 근거로 W0 create-only 회원 9를 생성하고 check 성공, T2는 Apply.retract(305)를 commit, 이후 T1 commit.
```text
R3 check-race create=Ok(BundleResult { applied: [], created: 1 }); retract=OK [305]; invalid_members=1
```
검사 Ctx가 `lock_reads=false`(`src/lib.rs:355`, 기본값 `spikes/spike-v2-read/src/sqlgen.rs:103`)라 supporting Apply의 변경은 기다리지 않았다. 검사 시점에 참이라는 사실도 commit 시점의 관계 불변식을 보장하지 않는다. 기술적으로 동시성을 막더라도 이후 관계 변경 시 재검사 범위는 별도로 필요하다.

## r2b 원래 입력 재확인

임시 복사본 `/private/tmp/aip-r3-guvdsc0r`에서 원래 요청·정책 변형을 다시 실행했다. source/fixture 파일을 복사했고 새 테스트만 임시 디렉터리에 생성했다. 원본 테스트의 단언 결과와 별도로 각 executor 결과를 출력했다.

명령(읽기, exit 0):
`PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="/Users/winterholic/development/projects/aip/spikes/spike-v2-read/target" cargo test --manifest-path /private/tmp/aip-r3-guvdsc0r/spike-v2-read/Cargo.toml --offline -q --test r3_read r3_original_read_inputs -- --nocapture`
```text
R3 R2B-08-guard-false actor=Some(2) => Ok([Object {"bookmarkCount": Null, "id": Number(100)}, Object {"bookmarkCount": Null, "id": Number(102)}])
R3 R2B-08-guard-true actor=Some(1) => Ok([Object {"bookmarkCount": Number(3), "id": Number(100)}, Object {"bookmarkCount": Number(1), "id": Number(102)}])
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.15s
```

명령(쓰기, exit 0):
`PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="/Users/winterholic/development/projects/aip/spikes/spike-v3-write/target" cargo test --manifest-path /private/tmp/aip-r3-guvdsc0r/spike-v3-write/Cargo.toml --offline -q --test r3_write r3_original_write_inputs -- --nocapture`
```text
R3 R2B-07 lock 2002ms => DEADLINE_EXCEEDED
R3 R2B-06 original id100 revoked DELETE wait=698ms; worker=changed[100] unchanged[]; status=CLOSED
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 2.79s
```

| 발견 | 원래 재현 입력·이번 결과 | 판정·현재 근거 |
|---|---|---|
| R2B-01 P2 | Apply.approvedCount(clubId=10), actor=1/2/익명: Ok([2])/ACCESS_DENIED/ACCESS_DENIED | 원래 반례 해결. executor가 직접 거부 (`spikes/spike-v2-read/src/lib.rs:50`) |
| R2B-02 P2 | Recruitment rows read true/false, select id limit20: 3행/0행 | 원래 반례 해결 (`sqlgen.rs:237`). V3 allow/from의 true/false 전수는 확인 못함: 이번 독립 추가 입력은 read 정책에 한정 |
| R2B-03 P2 | select id×51 및 filter periodEnd.gte×1000 + sort id.asc×1000, limit1: 모두 DUPLICATE. internalNote 1,048,576글자: OUTPUT_TOO_LARGE | 원래 반복/출력 반례 해결 (`plan.rs:111,231,259`, `lib.rs:45`). 서로 다른 필터·정렬의 구조 크기 비용은 별도 한계이며 해결됐다고 확대하지 않음 |
| R2B-04 P2 | repeat reject + where isChecked=true: changed[] unchanged[]. 빈 where: 미읽음 2,4만 changed | from-only 반영 확인 (`spike-v3-write/src/lib.rs:182`). 원래 알림 id와 동일할 필요 없는 집합 반례이며 이전 true/false 상태를 그대로 유지 |
| R2B-05 P2 | MemberAlarm select id, filter isChecked.eq=false, actor1: id1 반환 | 원래 Bool 요청 실행 (`plan.rs:66`) |
| R2B-06 P1 | 원래 모집100·actor1, judge 뒤 pause700ms 관찰 후 ClubMember(10,1) DELETE: 698ms 대기, 마감 성공 뒤 삭제 | 원래 관계 권한 회수 반례 해결 (`sqlgen.rs:146,279`, `spike-v3-write/src/lib.rs:156`). 새 행 부재·NOT EXISTS 정책까지 일반화하지 않음 |
| R2B-07 P2 | MemberAlarm9를 다른 연결에서 FOR UPDATE로 잠근 뒤 read: 2002ms DEADLINE_EXCEEDED | 원래 오류 분류 개선 (`src/lib.rs:26,57`). 전체 기한과 commit 결과 의미의 새 반례는 R3-05 |
| R2B-08 P1 | 원래 managerSomewhere(actor) 선언, actor2의 ClubMember role=MEMBER 유지. Recruitment select id/bookmarkCount: null/null. actor1: 3/1. 기존 totalOfVisible: actor2도 3/1 | 원래 권한 guard 누락 해결 (`plan.rs:135`). false/true/기존 total 양방향 대조 |
| R2B-09 P2 | Recruitment.close의 to만 internalNote=null로 변경: 정상 changed[100], 재실행도 changed[100] | 원래 SQL 생성 실패 해결 (`sqlgen.rs:205`). 원래 close는 repeat 기본 reject이고 from=PUBLISHED가 그대로라 반복 changed가 현재 선언의 결과다. repeat unchanged의 null 재시도는 원본 v3_r2b의 clearTitle 단언으로 별도 확인 |
| R2B-10 P3 | MemberAlarm9, from=false OR true + repeat unchanged: 첫 changed[9], 둘째 unchanged[9] | done 우선 판정 반영 (`src/lib.rs:207`). 효과 포함 전이 전체의 멱등성 증거는 아님 |
| R2B-11 P2 | 잠긴 숨은 모집101 / 없는999: 1ms/0ms, 모두 MISSING_TARGET | 원래 반례 해결 (`src/lib.rs:179`) |
| R2B-12 P3 | 익명 not(club.school=actor.school): []; club.school!=null: id100,101 | 실행 의미는 기존과 동일. 문서 V2-R3가 UNKNOWN/IS NULL 구분으로 수정됨 (`V2-caller-read-results.md:70`). 코드 주석 `sqlgen.rs:254`에는 ‘거짓 취급’ 표현이 남아 있으나 추가 기능 결함으로 세지 않음 |

전체 수용 facts·모든 동시 실행 경로가 안전하다는 판정이 아니다. r2b P1 2건은 원래 입력 범위에서 재발 없음.

### R3-05 · P1 · Q4/Q6 · commit 도중 기한 초과는 rollback을 보장하지 않는다

문제: 요청은 DEADLINE_EXCEEDED로 반환하지만 DB에는 승인·회원·outbox가 모두 commit되고, 쓰기 전체 기한 2초 뒤에 commit이 끝날 수 있다.
영향: ‘기한을 넘기면 tx drop으로 rollback’이라는 현재 설명은 commit을 이미 보낸 경우 틀린다. 결과를 실패로 취급해 재시도하거나 V4 worker의 기한 이후 쓰기 차단 근거로 사용하면 실행 결과를 잘못 판단한다. DB의 세 효과 자체는 원자적으로 함께 commit되며, 부분 commit을 관찰한 것은 아니다.
조치: 다음 단계 전에 commit 시작 전 남은 기한과 commit 결과 불확실성을 분리한다. 실패/rollback과 commit 결과 미확정을 구분하는 응답·상태 조회·멱등 키 후보를 검증한다. timeout future 취소만으로 이미 보낸 COMMIT 취소/rollback을 보장하지 않는다.

근거: `spikes/spike-v3-write/src/lib.rs:60`의 전체 timeout, `:62`의 drop→rollback 주석, `:401`의 `tx.commit().await`. 트랜잭션별 statement_timeout은 고정 2000ms(`:371`)라 요청 전체의 남은 시간과 다르다. 같은 commit 방식이 bundle `:462`, compose `:594`에도 있다.

직접 실행: 원본 엔진 의미 그대로인 임시 복사본에서 `aip_r3_deadline_guvdsc0r`만 사용했다. ClubMember INSERT에 DEFERRABLE INITIALLY DEFERRED constraint trigger를 달아 COMMIT 중 1.1초 sleep시키고, 기존 시험용 pause_after_lock_ms=1200으로 판정 뒤 1.2초 대기했다. 각 SQL은 2초 미만이나 요청 전체는 2초를 넘는다. 요청 `{apply:"Apply.approve",target:{ids:["300"]}}`, actor=1.

명령(exit 0):
`PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="/Users/winterholic/development/projects/aip/spikes/spike-v3-write/target" cargo test --manifest-path /private/tmp/aip-r3-guvdsc0r/spike-v3-write/Cargo.toml --offline -q --test r3_deadline r3_commit_deadline -- --nocapture`
```text
R3 commit-timeout 2002ms => Err(Reject { code: "DEADLINE_EXCEEDED", msg: "쓰기 요청이 2000ms 안에 끝나지 않음" })
R3 commit-timeout final=approved[300] members[10:2,10:5] outbox1
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 2.76s
```
응답 뒤 700ms 기다린 후 같은 연결로 최종 상태를 다시 조회했다. trigger·pause는 경합 재현 장치이며 호출자 JSON의 권한/값을 변경하지 않았다. 이 실험에서 DEADLINE_EXCEEDED가 rollback이라는 주장은 재현되지 않고 반대로 commit이 확인됐다.

### R3-06 · P2 · Q3 · V1이 수용한 compose 출처가 정상 요청에서 panic을 일으킨다

문제: 서버 compose 선언에 `create ClubMember from 1, member, recruitment.club`를 넣으면 V1은 facts를 성공 생성하지만, 원래 W1 정상 승인 요청이 Result 오류 없이 panic한다.
영향: 허용 값 출처가 path라는 런타임 전제를 의미 검사에서 강제하지 않는다. 현재 원본 fixture의 두 path만 있는 경우에는 발생하지 않으며, 서버 선언을 바꿨을 때의 수용/실행 범위 불일치다. 비인가 DB 변경은 관찰하지 않았다.
조치: 출처를 path만 허용할 거면 V1에서 non-path를 명시 진단한다. 값 표현식을 허용할 거면 실행기에서 표현식 종류와 요청 표현을 일치시킨다. trusted facts에도 unwrap 대신 명시 오류 경로를 마련한다.

근거: `spikes/spike-v1-fixture/src/parser.rs:393`은 unary 표현식을 출처로 받고, `src/sema.rs:801`은 path 제한 없이 self.ex 결과를 sources에 넣는다. `spikes/spike-v3-write/src/lib.rs:469`의 path_text가 path.segs를 unwrap하며 `:524`의 find에서 리터럴 출처에도 호출된다.

원본 W1 선언의 출처 목록에 `1`만 앞에 추가한 임시 서버 소스에서 V1 load_str 성공. 원래 `{compose:"Apply",targets:{ids:["300"]},steps:[approveOnly,create ClubMember from item]}`를 보냈다. task 경계로 panic을 포착했다(요청 검사는 트랜잭션 시작 전이며 schema DDL은 실행하지 않음).
```text
R3 sources V1=OK sources=[{"lit":1},{"path":{"root":"this","segs":["member"]},"ty":"Ref<Member>"},{"path":{"root":"this","segs":["recruitment","club"]},"ty":"Ref<Club>"}]
R3 sources compose task=Err(JoinError::Panic(... "called `Option::unwrap()` on a `None` value" ...))
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.01s
```
명령: `PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="/Users/winterholic/development/projects/aip/spikes/spike-v3-write/target" cargo test --manifest-path /private/tmp/aip-r3-guvdsc0r/spike-v3-write/Cargo.toml --offline -q --test r3_sources r3_source_literal -- --nocapture` (exit 0은 panic 재현 단언이 성립했다는 뜻).

## 추가 실험 명령과 검사기 대조

승인/순서/역전이/역할/효과 실패/동시성 복합 실험:
`PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="/Users/winterholic/development/projects/aip/spikes/spike-v3-write/target" cargo test --manifest-path /private/tmp/aip-r3-guvdsc0r/spike-v3-write/Cargo.toml --offline -q --test r3_probe r3_approval_probes -- --nocapture` (exit 0).
```text
R3 notify-failure => INTERNAL | approved[] members[10:2] outbox0
R3 check-race create=Ok(BundleResult { applied: [], created: 1 }); retract=OK [305]; invalid_members=1
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 1.84s
```

r2b P1 검증이 해당 장치를 실제로 측정하는지 음성 대조했다. 같은 원래 입력 테스트 명령으로, 임시 복사본에서만 다음 장치를 껐다.

| 음성 대조 | 실제 출력·단언 | 판정 |
|---|---|---|
| R2B-06의 pause700ms 요청에 skip_policy_lock=true | `DELETE wait=0ms; worker=changed[100] unchanged[]; status=CLOSED`, wait>400 단언 실패, exit101 | 관계 잠금이 대기를 만든다는 독립 대조 |
| R2B-08 guard CASE를 `CASE WHEN guard OR TRUE THEN count END`로 변형 | actor2가 bookmarkCount 3/1을 받음, Null 단언 실패, exit101 | 원래 count 노출이 guard 집행 여부에 의존한다는 대조 |

처음 guard CASE를 count만으로 바꾸는 주입은 사용하지 않는 bind 인자 때문에 42P18이 났다. 이것을 guard 누출 대조로 채택하지 않고, bind 인자를 유지하는 OR TRUE 주입으로 다시 확인했다. 두 파일을 복원한 뒤 같은 원래 입력 테스트를 재실행했다. 쓰기 실험의 원래 null 전이는 repeat 기본 reject이므로 반복 changed가 올바른 기대다. 최초 보조 테스트의 unchanged 기대를 바로잡고 원래 입력을 유지했다. 보조 실험의 잘못된 단언을 프레임워크 결함으로 세지 않았다.

## Q1. 왜 같은 결과였는가

현재 공통표의 판정은 우연히 문자열을 맞춘 구현은 아니다. W2와 W0의 apply는 같은 `apply_op`→judge→update 경로(`spikes/spike-v3-write/src/lib.rs:386,440`)를 쓰고, W1의 transition도 같은 judge/update를 호출한다(`:570`). 빈/중복/상한 ids는 parse_ids(`:87`)를 공유하고, 회원 중복은 같은 DB unique club/member(`spikes/spike-v2-read/src/sqlgen.rs:350`)에 걸린다. 누락·권한·상태 실패는 트랜잭션을 commit하지 않아 전체 rollback되는 공통 구조다.

다만 W1은 독립 사전 검사와 순서를 갖는다(R3-01). 기존 표가 한 요청에 한 위반을 넣고 rowRead=운영진 권한이라는 fixture를 쓰므로 이 차이가 가려진다. 원본 공통표의 오류·회원 상태 일치는 성립하지만 ‘같은 이유·같은 우선순위·같은 전체 효과’로 일반화하면 성립하지 않는다. 정상 승인의 알림 결과부터 W2와 W0/W1이 다르다.

## Q2. xmin 검사 누락과 정당한 쓰기 거부

- **의존 행만 변경**: 실제 엔진 경로로 재현한 R3-03. touched resource의 xmin은 ‘관계 불변식에 영향받은 행’ 집합이 아니다.
- **검사와 commit 사이 동시 변경**: R3-03의 check-race. 현재 check SQL은 관련 행을 잠그지 않는다. 입력 대상/운영진 행 잠금과 검사 근거 행 잠금은 다르다.
- **savepoint/subtransaction**: 결과 문서에 이미 알려진 한계다. 임시 엔진 unit test에서 private run_checks를 직접 호출해 부모 트랜잭션의 invalid INSERT가 CHECK_FAILED인 양성 대조와 savepoint INSERT가 검사에서 빠지는 음성 대조를 추가 실행했다. 원본 엔진은 savepoint를 호출하지 않으므로 현재 공개 요청의 우회로로 세지 않는다. V4가 tx handle/savepoint를 허용하면 반드시 다시 설계해야 한다.
- **삭제·cascade·worker 직접 SQL**: 확인 못함: 현재 공개 쓰기에는 삭제/worker/tx handle 경로가 없다. xmin 검사에 삭제된 행 자체가 보이지 않는 구조는 정적으로 확인되지만, 그 경로의 정상 업무 입력·실행 결과를 검증하지 않았다.
- **초대**: 원본 check를 켜고 W0 ClubMember(10,9,MEMBER) create-only를 실행하면 CHECK_FAILED다. 승인 경로 없이 회원을 추가하는 정당한 초대를 허용하려면 hasApproval을 모든 회원 쓰기에 무조건 걸 수 없다. 이미 결과 문서 `:160`에 인정한 정책 범위 한계이며 새 발견으로 세지 않는다. 체크 엔진의 false positive라고 단정할 수도 없다. 현재 선언은 바로 이 행을 금지한다.
- **기존 운영진의 역할 변경**: 확인 못함: 관리자 위임의 정책·전이 fixture가 아직 없다. hasApproval을 역할 변경에도 적용할지와 기존 운영진/초대 회원의 예외는 다음 fixture에서 명시해야 한다.

## Q3. W1 정상 형식 요청의 순서·반복·여러 create

원본 check·unique를 유지한 실제 결과:

| 요청(대상300, actor1) | 결과·DB | 의미 |
|---|---|---|
| transition→create | OK, 회원1 | 원본 정상 경로 |
| create→transition | OK, 회원1 | create allow는 생성 시점, hasApproval은 commit 직전. 중간에 승인 전 회원이 존재하지만 managerOf(actor,club)가 참인 create만 실행됨 |
| transition만 | OK, 회원0 | 필수 create는 선언하지 않았다(R3-02) |
| create만 | CHECK_FAILED, 변화 없음 | 원본 후보별 표에서 직접 실행 |
| transition→transition→create | INVALID_STATE, 변화 없음 | 중복 단계 자체를 거부하지 않는다. 두 번째 approveOnly의 from=PENDING이 거짓이라 실패 |
| transition→create→create | ALREADY_EXISTS, 변화 없음 | 단계 횟수 제한이 아니라 DB unique가 두 번째 생성을 막는다 |
| member/club 상수 | VALUE_SOURCE_NOT_ALLOWED, 변화 없음 | 참조 상수 금지 확인 |
| member와 recruitment.club 경로를 서로 바꿈 | TYPE_MISMATCH, 변화 없음 | 선언된 출처라도 대상 Ref 타입이 달라 거부 |
| 미선언 출처/미허용 전이 | VALUE_SOURCE_NOT_ALLOWED/STEP_NOT_ALLOWED | 원본 테스트에서 직접 실행 |
| role.const=ADMIN | OK, ADMIN 회원1 | enum 형식 검사와 업무상 MEMBER 제한은 다르다(R3-02) |

원본 값 출처 제한의 정상 path 두 개 범위에서는 위조 참조·타입 혼동 반례가 막혔다. 새 서버 출처 표현식까지 포함하면 R3-06 panic이 있다. 반복 전이·여러 create를 언제나 구조 오류로 거부한다는 정책은 구현돼 있지 않다. 다른 합법 전이를 반복하거나 다른 생성 대상을 여러 번 쓰는 업무는 확인 못함: 이번 추가 실행은 원본 approveOnly·ClubMember에 한정했다.

## Q4. 효과·outbox·동시 승인

검증한 승인 fixture에서 **DB 원자성 반례는 없음**. 원본 W2 `[300,303]`에서 일부 회원이 이미 존재하면 상태 변경·회원·outbox 모두 변화 없이 ALREADY_EXISTS다. 임시 schema의 outbox에 `CHECK(topic <> 'apply.approved')`를 추가해 회원 create 뒤 notify INSERT를 실패시키면 INTERNAL이며 approved[]·새 회원0·outbox0으로 돌아갔다. 알림 실패도 앞선 상태 변경과 회원 생성까지 rollback된다(`src/lib.rs:285,293,398,400`). 반대로 R3-05의 commit 기한 초과에서는 세 상태가 함께 commit됐고 응답만 실패였다.

원본 동시성 2종을 W2/W1으로 실행하고, 보조 테스트로 W0도 추가했다.

| 경쟁 | W2 | W1 | W0 추가 실행 |
|---|---|---|---|
| 같은 지원300 두 요청 | OK/INVALID_STATE, 회원1·outbox1 | OK/INVALID_STATE, 회원1·outbox0 | OK/INVALID_STATE, 회원1·outbox0 |
| 같은 회원5의 지원300/306 | ALREADY_EXISTS/OK, 회원1·outbox1 | ALREADY_EXISTS/OK, 회원1·outbox0 | ALREADY_EXISTS/OK, 회원1·outbox0 |

승자 순서는 타이밍에 따라 달라졌다. W2에서는 승인된 지원이 300 또는306 하나이며 원본 출력과 보조 출력에서 확인했다. 대상 행 잠금·from=PENDING·unique가 중복 회원 및 해당 fixture의 중복 outbox를 막았다. outbox 테이블 자체에는 topic/source/source_id의 유일 제약이나 전달 멱등 키가 없다(`spikes/spike-v2-read/src/sqlgen.rs:345`). 전이를 PENDING으로 되돌렸다 재승인할 때 알림을 한 번 더 만들어야 하는지는 업무 계약이며 이번 실행으로 ‘항상 알림 정확히 한 번’을 보장하지 않는다.

W2 effects는 trusted 전이 자체의 권한을 사용하며 ClubMember.exposeCreate.allow를 호출하지 않는다(`src/lib.rs:275`). W0/W1만 create_row의 allow를 적용한다(`:334`). 보조 facts에서 exposeCreate.allow=false로 바꾸면 W2는 정상 생성, W0/W1은 FORBIDDEN이었다. 이는 V3-R10의 명시된 범위와 맞으며 별도의 우회 발견으로 세지 않는다. ‘resource의 create allow를 모든 쓰기에서 공통 집행’한다고 일반화해서는 안 된다.

확인 못함: outbox 소비자·외부 전달·worker crash·응답 유실·재전달·확장 코드 내 동시/다중 트랜잭션. 구현이 없으며 검증 범위는 PostgreSQL에 남는 outbox 행까지다.

### R3-07 · P3 · Q5 · §6.4 마지막 관찰은 W0/W1도 outbox를 남기는 것처럼 묶는다

문제: 결과 문서 §6.4 마지막 항목은 원본의 네 문제를 ‘세 후보 모두’에서 다르게 처리했다고 적고 outbox를 포함한다. 실제 W0/W1은 알림 효과를 구현하지 않아 outbox0이며, 원본 알림 방식을 대체한 후보는 W2뿐이다.
영향: 상세 표는 W2만 outbox라고 정확히 적었지만 요약을 재사용하면 후보별 기능 동등성과 실험 범위를 혼동한다. 새 안전성 결함으로 확대하지 않는다.
조치: outbox 관찰을 W2에 한정해 기록한다. 다음 비교는 알림까지 동등한 업무를 만들거나 차이를 별도 축으로 유지한다.

근거: `plan-docs/alignment/V3-standard-write-results.md:162`와 같은 문서 `:119,120,121,129`의 알림 차이. 엔진 `spikes/spike-v3-write/src/lib.rs:274`의 effects 루프는 approveOnly에 effects가 없으면 수행하지 않는다. 원본 실행의 정상 승인 출력은 W2 outbox2 / W0 outbox0 / W1 outbox0이다.

## Q5. §6.4 관찰의 강도와 다른 업무

| 문서 관찰 | 이번 실행에서 지지되는 범위 | 결론을 넓히면 안 되는 부분 |
|---|---|---|
| W2가 업무 불변식을 한 곳에 둔다 | 승인 경로의 allow/from/to/create/notify가 한 전이 선언에 있고, 호출자는 대상만 보낸다. 효과 실패 rollback 확인 | 전체 불변식의 한 장소 집행은 아니다. unique는 ClubMember 선언/DB index, hasApproval check는 ClubMember·predicate에 있다. 다른 공개 approveOnly·create 경로도 같은 facts에 존재한다(`tests/v3_2.rs:41`). global 관계 불변식과 commit 실패 의미는 R3-03/05 |
| W0은 단독 정책만으로 안전하지 않다 | 이 fixture의 create allow는 운영진 여부만 검사한다. check를 제거하면 지원하지 않은 회원 생성이 실제 성공한다 | W0라는 모델의 일반적 불가능성은 아니다. 생성 allow에 승인 근거·role을 넣거나 공개 연산을 더 정교하게 제한하면 필요한 check/허용 순서가 달라진다. 이 대안의 전체 구현·경합 대조는 확인 못함: 이번 실행 범위 밖 |
| W1이 서버 작업을 줄이지 못했다 | 현재 작성자가 쓴 선언의 구성과 호출자 요청을 놓고는 줄인 작업을 관찰하지 못했다는 제한적 표현이 타당 | 9줄 대 8줄은 작성 비용·새 기능 추가 비용을 측정한 값이 아니다. 줄 묶음/중괄호/공통 선언 재사용 기준에 따라 달라지고, 실제 수정 파일·유지보수 시간을 측정하지 않았다. W0/W1에 W2 알림 의미도 없고, 생략·role·bulk의 허용 범위도 다르다(R3-02/04/07) |

W1의 출처·단계·create allow·check를 모두 선언한 이유는 지금 후보가 순서 변경과 단계 생략을 허용하며 생성 정책도 넓게 열었기 때문이다. 같은 자유와 모든 업무 완결성을 동시에 유지하려면 추가 규칙이 필요하다는 증거다. ‘W1은 어떤 제한을 택하든 항상 W2보다 서버 작업이 많다’는 증거가 아니다. 반대로 아직 W1이 더 낫다는 실행 근거도 없다.

관리자 위임에서는 기존 역할 전이와 대상 참조 전달을 여러 화면에서 재사용하면 W1이 줄일 서버 변경이 생길 수 있다. 자기 위임·기존 권한 없는 단계·양도 후 추가 쓰기를 반드시 분리해야 한다. 순환 전이는 최종적으로 유효한 관계 교환이라도 현재 즉시 unique index(`sqlgen.rs:350`)가 중간 상태에서 먼저 거부할 수 있다. 이 문장은 현재 DDL 구조에 따른 기술적 예상이며, 해당 업무의 실행 결론으로 사용하지 않는다.

확인 못함: 관리자 위임·순환 전이의 정상/동시/실패 실행, W0/W1 알림 기능을 동등하게 더했을 때의 정의·호출자 비용, 새 화면 추가에서 W1/W2의 수정 파일/작업 시간. 결과 문서 자체도 해당 업무가 미실행이라고 적었으므로 W1 채택/기각 결정의 근거는 아직 부족하다.

## Q6. 다음 실험 전에 정할 것

**다음 단계 전 기술적으로 해결/고정할 것:**

1. R3-03: check의 대상은 직접 수정 행인지 의존 변경 영향 행까지인지 명시하고, check→commit 사이 관련 행 변경을 막는 방식 검증. 최소 사례는 승인 근거 철회와 회원 생성의 동시 실행이다. ‘최종 상태만 맞으면 중간 권한 위반도 허용’으로 풀지 않는다(E RK-04).
2. R3-05: timeout·commit 전송·응답 유실의 결과 계약. 남은 기한 설정, commit 결과 미확정, 상태 조회/멱등 후보를 실제 DB 지연에서 대조한다. worker 취소와 이미 서버가 진행한 commit을 같은 것으로 보지 않는다.
3. W1 출처 표현식 수용 범위(R3-06), 필드별 상수/역할 제한, 필수·중복 단계, 값이 전이 전/후 어느 상태에서 읽히는지, 사전 가시성/sameScope와 단계별 allow의 오류 우선순위(R3-01)를 기술 계약으로 정한다. 현재 create_row는 단계 실행 때 대상 행을 다시 읽는다(`src/lib.rs:321`).
4. W0/W1/W2에 필수 회원 생성·MEMBER 역할·알림·동일 대상 상한을 맞춘 뒤 비교하거나 각 차이를 명시한다. 서버 정의 줄 수와 별개로 ‘새 화면/업무를 추가할 때 수정한 파일·선언·코드’ 원자료를 남긴다.
5. V3 확장은 관리자 위임과 순환 전이를 먼저 좁은 fixture로 작성한다. 권한의 자체 증폭, 중간 권한, 같은 단계 반복, 동시 위임, 최종 불변식, 즉시/지연 제약 실패를 대조한다.
6. V4로 바로 가더라도 기존 tx를 노출하기 전에 savepoint·직접 SQL·삭제·관계 변경의 검사 포괄 범위와 timeout 이후 tx 수명을 고정한다. actor/tenant·허용 효과·outbox 기록은 서버가 집행하고, 외부 전달은 DB 원자성과 별도 계약으로 검증한다(E RK-05·08). 현재 기술적 관문을 창시자 승인으로 대신하지 않는다.

**업무 fixture 작성자가 명시할 규칙:** 승인 시 회원 생성을 필수로 할지, 승인된 지원을 철회하면 회원을 어떻게 할지, MEMBER만 생성할지 ADMIN 임명도 허용할지, 초대/기존 운영진이 승인 검사에서 제외되는지. 프레임워크가 임의로 고를 정책이 아니며 비교 후보 모두에 같은 요구를 넣어야 한다.

**기술 비교 이후 창시자에게 남을 수 있는 선택:** 초기 표준 쓰기에 어떤 조합 자유/업무를 먼저 제공할지, 공개 오류 상세도와 retry 경험, 부분 성공 batch를 초기 제품에 포함할지. 이는 제품 범위와 경험의 선택이다. 현재 발견을 수정하는 잠금·check 변경 집합·타입 진단·commit 불확실성 처리는 우선 기술적으로 설계/시험할 일이다. 지금 W1을 영구 기각하거나 W2를 창시자 확정으로 승격할 근거는 없다(통합 지침 `§9`, C B.6).

## 검사·정리 기록

savepoint 실험 명령(exit 0):
`PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="/Users/winterholic/development/projects/aip/spikes/spike-v3-write/target" cargo test --manifest-path /private/tmp/aip-r3-guvdsc0r/spike-v3-write/Cargo.toml --offline -q --lib r3_xmin_subtransaction -- --nocapture`
```text
R3 xmin parent write => Err(Reject { code: "CHECK_FAILED", msg: "`Probe.valid`를 만족하지 않는 행 1개" })
R3 xmin savepoint write xmin=97644 parent=97643 => Ok(())
R3 xmin committed invalid=1
```
원본 엔진은 savepoint를 쓰지 않는다. 리터럴 출처 panic과 다른 별도 경계 확인이며 신규 P1/P2로 중복 계상하지 않았다.

R3-05의 원인은 로컬 캐시의 실제 의존성 소스에서도 대조했다. tokio-postgres 0.7.18 `src/transaction.rs:54`의 commit은 done=true로 바꾼 후 COMMIT을 await하며, Drop(`:35`)은 done=true면 rollback을 보내지 않는다. 따라서 이미 commit future를 진행한 뒤 timeout으로 drop한다고 자동 rollback되는 구조가 아니다. 재현의 최종 DB 조회가 판정 근거다.

발견 합계: **P1 2건(R3-03·05), P2 4건(R3-01·02·04·06), P3 1건(R3-07)**. r2b의 원래 P1 2건 재발 없음과 새 commit/관계 검사 P1은 별개다. 원본 승인 fixture의 대상 누락 rollback·회원 unique·outbox 기록 원자성은 확인한 범위에서 성립한다. 전체 쓰기 모델 안전성/worker 안전성은 확인하지 못했다.

음성 대조 장치를 복원한 뒤 동일 명령 재실행(exit 0)의 핵심 출력:
```text
read-restored: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.15s
write-restored: R3 R2B-06 original id100 revoked DELETE wait=696ms; worker=changed[100] unchanged[]; status=CLOSED
write-restored: test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 2.76s
```

정리 확인 명령: `psql 'host=localhost dbname=postgres' -X -qAt -c "SELECT nspname FROM pg_namespace WHERE nspname IN ('aip_v2_spike','aip_v2_r2b','aip_v3_spike','aip_v3_r2b','aip_v3_2','aip_r3_approval_guvdsc0r','aip_r3_read_guvdsc0r','aip_r3_write_guvdsc0r','aip_r3_deadline_guvdsc0r','aip_r3_xmin_guvdsc0r','aip_r3_sources_guvdsc0r');"` → 출력 0행. 원본 테스트와 보조 실험이 만든 schema는 모두 정리됐다. 실패한 보조/음성 대조에서 남은 read/write schema도 복원 실행의 DDL/종료 DROP으로 정리했다. 임시 소스·로그는 저장소 밖 `/private/tmp/aip-r3-guvdsc0r`에만 있다. git 명령은 실행하지 않았다.
