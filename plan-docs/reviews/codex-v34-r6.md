# 조합 쓰기·쓰기 확장 정확성 검토 (r6)

- 검토일: 2026-10-04. 정상 사용과 경계 사례를 문서·코드·로컬 실행으로 대조한다. 프로세스 격리는 범위 밖이다.
- 원본 수정은 이 파일만 허용한다. 변형과 추가 테스트는 임시 복사본에서 수행한다. `.env`·비밀 저장소 및 git 명령을 사용하지 않는다.
- 발견은 검증 직후 아래에 추가한다. P1=다음 단계 전 수정, P2=권장, P3=기록.

## 실행 환경·기준 실행

- 임시 복사본: `/private/tmp/aip-r6-56xwn867/`. V1·V2·V3·V4의 src/tests/fixture/workers/extensions와 Cargo 파일만 복사했다. V2의 기본 schema 및 모든 명시 schema를 `r6_aip_r6_56xwn867_…`로 바꿨다.
- 실행 명령(각 spike): `PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`. V1: 16개 통과(7+5+4), V3: 5개 통과.
- 최초 V4 전체 실행은 `tests/v4_1.rs`의 범위 밖 MacNetDeny 경로에서 Node·Python 모두 `WORKER_FAILED`가 나와 중단됐다. 이 실패를 쓰기 정확성 발견으로 분류하지 않는다. 임시 복사본에서 DB 직접 연결·격리 대안 사례만 제외하고 동일 명령으로 재실행한다. 쓰기 구현은 변경하지 않는다.

## 발견 F01 — P1: selfRow의 `by`와 `sameScope`가 달라도 V1이 받아 다른 범위의 자기 행을 변경한다 (Q1·Q4)

- 잘못된 점: `sameScope`의 값이 `by` 필드의 값이라는 관계를 의미 검사에서 보장하지 않는다. 서버 선언이 정상적으로 로드되지만 다른 동아리의 자기 행에 단계가 적용됐다.
- 영향·범위: 기본 위임 선언(`sameScope club; selfRow member by club`)에서는 재현되지 않았다. 새 compose 계약에 범위 경로를 확장하면 ‘대상과 같은 범위의 자기 행’이라는 설명을 믿을 수 없다. 남의 행 변경이 아니라 **다른 동아리의 자기 행 변경**이다.
- 다음 단계 전: `sameScope`와 `by`를 동일한 정규화 경로로 제한하거나 자기 행에서 같은 scope 표현식을 다시 평가하라. ID를 text로 바꾸어 서로 다른 resource의 식별자를 비교하는 방식도 제거해야 한다.

근거: `spikes/spike-v1-fixture/src/ast.rs:143`은 대상과 같아야 하는 범위 필드라고 정의한다. `src/parser.rs:388`·`:390`은 두 항목을 독립 저장한다. `src/sema.rs:820`·`:825`는 actor 참조·by 참조·sameScope 존재만 검사한다. `spikes/spike-v3-write/src/lib.rs:626`에서 대상 `sameScope`를 text로 만들고, `:647`·`:648`에서 자기 행의 `by` 컬럼과 직접 비교한다. 두 경로 일치나 참조 타입 일치를 확인하지 않는다.

임시 복사본 실제 실행: V3-3 fixture를 바탕으로 `transition mark { allow member = actor; from role = MANAGER; to role = MEMBER }`와 `expose compose { bulk maxRows 1; sameScope club.school; transitions mark; selfRow member by club }`를 선언했다. `load_str(..., Form::A)` 성공. 대상 id=3은 club=10, school=1이며, actor=1의 별도 행 id=6은 club=1, MANAGER이고 해당 동아리에는 별도 ADMIN이 있다. 요청은 다음과 같다.

```json
{"compose":"ClubMember","targets":{"ids":["3"]},"steps":[{"transition":"mark","on":"self"}]}
```

`cargo test --offline -q --test r6_self r6_self_boundaries -- --nocapture` 출력:

```text
R6 mismatched scope V1 accepted=true
R6 mismatched scope | ... | OK | ...3:club=10:MEMBER,...6:club=1:MEMBER,7:club=1:ADMIN
```

대상 club=10의 자기 행 id=1(ADMIN)은 그대로이고 club=1의 자기 행 id=6이 MANAGER→MEMBER로 커밋됐다. 요청·계약은 V1을 거쳤고 런타임 facts의 scope는 직접 변조하지 않았다.

## 발견 F02 — P1: 쓰기 확장의 기한은 커밋 검사·커밋을 덮지 않아 기한 뒤 변경과 OK가 남는다 (Q2·Q3·Q4)

- 잘못된 점: 확장의 `done` 수신까지만 전체 기한을 검사하고 이후 `checks_in`·`tx.commit()`은 제한 없이 기다린다. 1s 선언에서 약 1.25s 뒤 커밋과 성공 응답을 확인했다.
- 영향·범위: 일부 행만 커밋된 것은 아니다. 위임 두 행 모두 **기한 뒤에 커밋**된다. V3 표준 쓰기의 ‘커밋 전 여유 부족→rollback / 커밋 전송 뒤 시간 초과→COMMIT_UNKNOWN’과 의미가 다르다.
- 다음 단계 전: V3와 공통 기한 제어를 사용하되 검사·커밋 전 남은 기한도 확인하고, 커밋 전송 이후 결과 미확정 계약을 쓰기 확장에도 적용해야 한다.

근거: `spikes/spike-v4-worker/src/write.rs:18`에서 기한을 만들고 `:53`에 ctx 작업 timeout, `:70`에 done 도착 기한을 검사한다. 그러나 `:87`·`:88`은 남은 기한 검사·timeout·커밋 전송 표시가 없다. V3의 `spikes/spike-v3-write/src/lib.rs:79`·`:82`·`:94`·`:99`에는 그 제어가 있다. `plan-docs/alignment/V4-official-extension-results.md:142`의 ‘하나라도 어기면 rollback’은 done 수신까지의 기한 조건으로 한정해야 하며, 전체 쓰기 기한 보장으로 읽으면 실행 근거보다 강하다.  `:161`는 커밋 중 결과 미확정 미구현을 이미 인정한다.

실행: V4 기본 위임 계약의 deadline=1s, input=`{"target":"3","self":"1"}`, actor=1. 확장 함수가 700ms 기다린 뒤 기존 `delegate`를 호출한다. 임시 schema의 `club_member`에 `AFTER UPDATE DEFERRABLE INITIALLY DEFERRED` 트리거를 추가하고 NEW.id=1일 때만 `pg_sleep(0.5)`를 수행했다. 문장 기한(1s)보다 짧은 커밋 지연이지만 요청 전체 기한은 넘는다.

```text
R6 Node commit exceeds deadline | OK {"done":true} | elapsed=1281ms | 1:MEMBER,2:MANAGER,3:ADMIN
R6 Python commit exceeds deadline | OK {"done":true} | elapsed=1250ms | 1:MEMBER,2:MANAGER,3:ADMIN
```

명령: `PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q --test r6_write r6_write_boundaries -- --nocapture`. 트리거·확장 변형·테스트는 임시 복사본 및 임시 schema에만 두었다. 네트워크 단절에 의한 실제 커밋 결과 유실은 확인 못함: 이번 실행은 지연만 주입했다.

## 발견 F03 — P2: 끝난 호출의 ctx가 다음 정상 쓰기 호출을 오염시켜 rollback시킨다 (Q2·Q3·Q4)

- 잘못된 점: 이전 호출 토큰은 올바르게 거부하지만 그 거부를 현재 호출의 `poisoned`에 기록한다. 서로 다른 호출의 실패를 분리하지 못했다.
- 영향·범위: Node·Python 동일. 만료 ctx의 쓰기나 다음 호출의 일부 커밋은 없었다. 다음 정상 위임 전체가 잘못 실패하며 세 번째 호출은 정상이다. 기존 테스트는 700ms 기다려 뒤늦은 호출을 의도적으로 실행하므로 이 간섭을 다루지 않는다.
- 권장: 만료/다른 호출 메시지는 원래 call에 오류를 돌려주고 현재 트랜잭션 실패 기록에 넣지 말라. 현재 토큰의 ctx 실패만 현재 호출을 오염시켜야 한다.

근거: `spikes/spike-v4-worker/src/write.rs:41`·`:43`은 토큰 불일치를 거부하지만 `:58`·`:59`에서 출처 구분 없이 오염시키고 `:76`·`:77`에서 현재 호출을 실패시킨다. 완료 메시지만 `:69`에서 invoke ID를 구분한다. `workers/worker.mjs:30`·`:43` 및 `workers/worker.py:81`·`:82`는 이전 비동기 함수를 계속 실행시킨다. `tests/v4_3_write.rs:95`는 700ms 뒤에만 다음 호출을 시도한다.

실행: 첫 호출은 deadline=50ms이며 200ms 뒤 ctx로 `resignAdmin(self)`를 시도한다. timeout 반환 직후 같은 worker에서 deadline=1s인 다음 확장을 실행해 350ms 뒤 정상 delegate를 수행한다. DB는 첫 timeout 뒤 초기 역할 그대로이며, 다음 함수의 ctx 쓰기 두 개가 성공해도 이전 토큰의 오류 때문에 최종 rollback된다.

```text
R6 Node old call timeout | DEADLINE_EXCEEDED | elapsed=53ms | 1:ADMIN,2:MANAGER,3:MEMBER
R6 Node next valid delegate hit by old ctx | EXTENSION_ERROR(TOKEN_EXPIRED) | elapsed=354ms | 1:ADMIN,2:MANAGER,3:MEMBER
R6 Python next valid delegate hit by old ctx | EXTENSION_ERROR(TOKEN_EXPIRED) | elapsed=354ms | 1:ADMIN,2:MANAGER,3:MEMBER
```

별도 병렬 재현: `Promise.all`/`asyncio.gather`로 두 ctx apply를 보낸 뒤 첫 대상 행 잠금 대기 중 80ms timeout. 남은 과거 ctx 메시지가 다음 호출에서 처리돼 역시 `EXTENSION_ERROR(TOKEN_EXPIRED)` 및 전체 rollback. 세 번째 깨끗한 위임은 두 언어 모두 `OK`, `1:MEMBER,2:MANAGER,3:ADMIN`이었다.

## 발견 F04 — P2: Python에서 대기 중 ctx coroutine을 취소하면 worker가 종료된다 (Q2·Q3·Q4)

- 잘못된 점: 일반 `asyncio` task cancellation으로 ctx Future가 취소됐는데 worker가 그 Future에 결과를 설정해 `InvalidStateError`를 일으키는 경로가 있다.
- 영향·범위: 해당 위임은 rollback되어 일부 쓰기가 남지 않았다. 같은 worker의 다음 호출도 `WORKER_FAILED`가 되어 재시작이 필요하다. 정상 병렬 await는 Node·Python 모두 정상이다. 이 취소 실험은 Python 언어 동작만을 평가하며 Node의 별도 취소 API는 확인 못함: spike ctx에 대응 API가 없다.
- 권장: 완료/취소된 Future에 대한 reply 처리를 구분하고, cancellation이 서버 ctx 작업의 취소인지 응답 대기만의 취소인지 ABI에 명시하라.

근거: `spikes/spike-v4-worker/workers/worker.py:38`·`:42`가 서버 응답용 Future를 task에서 await한다. `:76`·`:78`·`:80`에는 취소 상태 확인이나 예외 처리 없이 결과를 설정한다. `src/write.rs:35`·`:92`는 worker 종료 후 트랜잭션을 rollback한다.

실행: id=3 잠금을 100ms 잡아 두고, Python 확장이 `asyncio.create_task(ctx.data.apply(...makeAdmin...))`를 만든 뒤 10ms 후 `task.cancel()`을 호출한다. `CancelledError`를 잡은 뒤 resignAdmin을 요청한다. DB 잠금 해제 뒤 다음 결과를 얻었다.

```text
R6 Python cancel pending ctx | WORKER_FAILED | elapsed=107ms | 1:ADMIN,2:MANAGER,3:MEMBER
R6 Python next after ctx cancel | WORKER_FAILED | elapsed=0ms | 1:ADMIN,2:MANAGER,3:MEMBER
```

첫 DB 실행에서는 `Worker`의 비공개 stderr_tail 때문에 예외 이름을 읽지 못했다. 아래 별도 worker 프로토콜 실행에서 stderr와 종료 코드로 원인을 추가 검증했다.

F04 추가 검증(같은 검토 실행): Python worker를 PATH만 있는 환경으로 실행하고 `parallelCancel`을 invoke한 뒤 makeAdmin/resignAdmin call을 받은 다음 취소된 첫 call에 정상 reply를 보냈다. 실제 stderr 마지막 줄은 `asyncio.exceptions.InvalidStateError: invalid state`, exit=1이었다. DB 없이 worker 통신만으로 원인을 재현했으므로 위 예외 이름의 미확인 상태를 해소한다. 로그: 임시 복사본 `logs/python-cancel-stderr.log`.

## 발견 F05 — P2: 결과 문서의 비교 문구와 현재 실험 범위가 일부 어긋난다 (Q3)

- `plan-docs/alignment/V3-standard-write-results.md:197` 제목의 “W1 표현 불가”는 selfRow 추가 뒤 현재 구현과 맞지 않는다. 같은 문서 `:238`·`:244`와 재실행된 `tests/v3_3.rs`는 W1 위임 성공을 보여 준다.
- `:168`은 서버 정의를 “9 vs 8”로 비교하고 관리자 위임을 아직 시험하지 않았다고 쓴다. 현재 `:122`·`:124`의 W1 승인 정의는 11이며 §7에 위임 실험이 있다. 줄 수는 `:124` 스스로 밝힌 대로 선언 묶음 수이며 실제 수정 비용이나 유지보수 비용 측정으로 바꾸어 읽을 수 없다.
- `:166`의 “업무 불변식을 전이 한 곳에 둔다”는 W2 효과값·순서를 서버 전이에 모았다는 한정으로 읽어야 한다. 승인 unique, 위임 oneAdmin·keepsAdmin은 여전히 별도 선언이다(`spikes/spike-v3-write/tests/v3_3.rs:13`·`:15`·`:32`).
- `plan-docs/alignment/V4-official-extension-results.md:157`의 “W0과 같은 의미”는 **같은 순서의 공개 전이를 동일 actor로 한 트랜잭션에서 실행했을 때의 업무 결과**로 한정해야 한다. F02의 전체 기한 차이, F03의 연속 호출 간섭, F04의 Python 취소 차이는 현재 같지 않다. W0은 `spikes/spike-v3-write/src/lib.rs:498`에서 20개 상한이 있지만 쓰기 확장 ctx loop에는 같은 횟수 상한이 없다(`spikes/spike-v4-worker/src/write.rs:31`). 아래 25회 호출의 추가 검증으로 이 횟수 차이를 실제 실행에서도 확인했다.
- 다음 문서 갱신에서 제목·숫자·미실험 목록을 현재 코드와 맞추고, 두 업무의 조건부 관찰과 일반 안전성·표현력·TS 지원·유지보수 우열을 구분하는 것을 권장한다. 기존 `V3 :170`·`:246`의 “두 업무만으로 표준을 정하지 않는다”는 적절한 한정이다.

F05 추가 검증: 25개의 `ClubMember.touch`를 순차 await하는 확장을 V1의 `access ClubMember.touch`로 선언했다. W0의 동일 25개 atomic apply는 `BAD_REQUEST`, 역할은 그대로였다. Node 확장은 15ms, Python 확장은 13ms에 `OK {"done":true}`, 자기 행 id=2는 MANAGER→MEMBER가 됐다. 확장에 20회 상한이 없다는 정적 대조를 실행으로 확인했다. 반복 touch의 결과가 changed인 것은 `from true`·기본 repeat reject인 별도 전이 fixture의 동작이며 기본 위임을 반복 성공시킨 결과가 아니다.

## Q1: 정상 요청 및 단계 순서·반복의 확인 범위

**기본 위임 계약에서는 추가 발견 없음.** F01은 V1이 승인한 다른 서버 선언에서의 범위 혼동이다. 기본 계약을 유지한 채 호출자가 남의 자기 행이나 다른 동아리의 자기 행을 지정해 성공한 요청은 찾지 못했다. self ID는 요청에서 받지 않고 서버 actor로 조회하며(`spikes/spike-v3-write/src/lib.rs:646`·`:648`), 조회 결과가 한 행이어야 하고(`:651`), 각 단계는 `judge`의 가시성·allow·from을 다시 거친다(`:658`, `:201`·`:241`).

| 실행 입력·상황 | 실제 결과 | 커밋된 역할 |
|---|---|---|
| target=3, makeAdmin(target) → resignAdmin(self), actor=1 | OK | 1 MEMBER, 3 ADMIN |
| self 대신 target=3에 resignAdmin | FORBIDDEN | 초기 상태 유지 |
| resignAdmin(self) → makeAdmin(target) | MISSING_TARGET | 초기 상태 유지 |
| makeAdmin만 / resignAdmin(self)만 | INVARIANT_VIOLATED / CHECK_FAILED | 초기 상태 유지 |
| makeAdmin 연속 2회 | INVALID_STATE | 초기 상태 유지 |
| 정상 위임 단계 뒤 resignAdmin(self) 반복 | INVALID_STATE | 초기 상태 유지 |
| makeAdmin(self) | INVALID_STATE | 초기 상태 유지 |
| 정상 위임 2회 반복을 한 compose로 제출 | MISSING_TARGET | 초기 상태 유지 |
| actor가 동아리 10과 11의 ADMIN인 상태에서 target=3(club=10) 위임 | OK | club=10의 자기 행만 내려놓고 club=11 ADMIN 유지 |
| 같은 club의 actor 자기 행이 2개인 비정상 DB 입력 | MISSING_TARGET | target 변경도 rollback |
| 같은 actor가 두 대상에게 동시 위임(W0/W1/W2) | 각각 한쪽 OK, 다른 쪽 CONFLICT | 해당 club ADMIN 한 명 |

기본 사례는 `spikes/spike-v3-write/tests/v3_3.rs:97`·`:128`·`:160`의 재실행이며, 반복·두 동아리 소속·자기 행 중복은 임시 `tests/r6_self.rs` 추가 실행이다. 전체 가능한 단계열을 전수 열거한 것은 아니다. scope 필드 변경 전이, 여러 target을 가진 selfRow 조합, 순환 전이의 일반 의미는 확인 못함: 현재 위임 fixture는 bulk=1이고 역할만 변경한다.

## Q2: 원자성·접근 제한·연속 호출·병렬 쓰기·기한의 판정

**일부 행만 커밋되거나 선언 밖 ctx 전이가 실행되어 남은 사례는 없음**, 아래 확인 범위에 한정한다. **기한 뒤 쓰기는 F02로 있음**. 실패한 호출의 미완료 메시지가 다음 호출을 실패시키는 F03, Python 취소 후 종료 F04도 있다.

| 확인 범위 | 실행 근거·결과 |
|---|---|
| 순차 위임, 첫 쓰기 후 throw, 오류 삼킴, 임명만, 계약 밖 전이, 첫 쓰기 후 timeout, expired ctx 재사용 | 기존 `tests/v4_3_write.rs`를 Node·Python 모두 재실행. 정상만 위임 커밋, 나머지는 초기 역할 유지 |
| poison이 불변식과 독립적으로 작동하는가 | 임시 계약에서 oneAdmin·keepsAdmin을 모두 제거해 V1로 로드하고 DDL 생성. delegateSwallow는 두 언어 모두 EXTENSION_ERROR(FORBIDDEN), 첫 makeAdmin까지 rollback |
| 성공적인 쓰기 둘 후 잘못된 output | 같은 무불변식 fixture에서 `await delegate(...); return {done:"yes"}`. 두 언어 모두 OUTPUT_INVALID 및 초기 역할 유지 |
| worker에서 actor=3 추가 | 같은 무불변식 fixture에서 raw ctx 요청에 actor를 추가. 두 언어 모두 EXTENSION_ERROR(UNKNOWN_KEY), 변화 없음 |
| 정상 병렬 ctx apply | Node Promise.all / Python asyncio.gather로 makeAdmin(target), resignAdmin(self)를 함께 발행. 두 언어 모두 OK, 자기 행 MEMBER·대상 ADMIN. Rust는 한 트랜잭션 loop에서 받은 ctx 작업을 순차 실행한다(`src/write.rs:40`·`:53`) |
| 병렬 요청 중 첫 ctx가 잠금 대기로 timeout | 80ms 기한·id=3 잠금. 두 언어 모두 DEADLINE_EXCEEDED 및 두 행 모두 초기 상태. 다음 호출은 F03처럼 오염됨 |
| 커밋 단계 지연 | F02처럼 1s 기한 뒤 성공과 위임 커밋. 이미 알려진 `V4-official-extension-results.md:161`의 미구현 항목을 실행으로 구체화한 것이다 |
| await한 Python ctx task를 취소 | F04처럼 worker 종료 및 전체 rollback. 후속 호출도 worker 재시작 전까지 WORKER_FAILED |
| 확장의 25회 순차 apply | W0은 20회 상한으로 BAD_REQUEST, 확장은 두 언어 모두 25회 실행 및 OK. 선언 밖 쓰기 우회가 아니라 서로 다른 비용 계약 |

확인 못함: 네트워크 단절·응답 유실 중 커밋 결과, 확장이 await하지 않고 반환한 background 작업의 공식 계약, 대규모 병렬 부하·worker 재시작 자동화. 프로세스 격리와 ctx 외 직접 DB 접근은 이번 판정 범위 밖이다. `apply_in`은 자체 커밋·전체 기한·검사를 하지 않는다(`spikes/spike-v3-write/src/lib.rs:691`·`:694`). 호출 측이 실패 기록·검사·커밋·기한을 책임진다는 경계를 API 계약에 남겨야 한다. `checks_in`은 현재 트랜잭션이 쓴 행의 사후조건이며 전역 관계 불변식이 아니다(`:414`·`:424`·`:697`); savepoint는 ctx API에 없으며 별도 검증하지 않았다.

## Q3: W0/W1/W2/쓰기 확장 비교 결론의 범위

참조 기준은 `plan-docs/sources/founder-integrated-directive-2026-10-03.md:540`·`:548`·`:554`의 표준 동작 확대·원자적 조합 설계·공식 확장 요구, `plan-docs/alignment/C-syntax-proposal.md:260`·`:289`의 후보 비교, `plan-docs/alignment/E-technical-risks.md:15`·`:16`의 RK-04·RK-05다. 실행 결과를 표준 선택 승인으로 승격하지 않는다.

| 후보 | 이번 근거가 지지하는 결론 | 근거가 없는 일반화 |
|---|---|---|
| W0 | 승인과 위임의 공개 동작을 atomic하게 실행한다. 위임은 호출자가 순서를 정하며 전이별 권한을 매번 재평가한다 | 모든 복잡한 쓰기가 20개 묶음으로 충분함, 최종 불변식만 있으면 중간 정책을 생략해도 됨 |
| W1 | 승인에서는 선언된 대상 경로에서 값을 가져오고, 올바른 selfRow 계약에서는 자기 행/대상 행을 구분한다. 위임 순서·반복의 실패는 전체 rollback | 일반 scope 경로가 selfRow와 안전하게 합성됨(F01), 조건·분기·순환 전이를 이미 지원함, 모든 업무에서 W2보다 정의가 많거나 적음 |
| W2 | 승인 create/notify 및 위임 update의 순서·값을 서버 전이로 모아 호출을 간결하게 한다 | 업무 불변식·운영 비용을 전이 한 곳만으로 모두 해결함, 어떤 업무에서도 최선임 |
| 쓰기 확장 | JS(Node)·Python 코드가 공개 전이를 같은 actor·트랜잭션에서 호출한다. 정상 병렬 await 및 실패 rollback을 확인했다 | W0과 호출 수명·기한·상한이 동등함(F02~F04·25회 실험), TS 작성·빌드·패키지 지원까지 검증됨, 모든 복잡한 업무를 현재 ctx API로 표현 가능함 |

현재 쓰기 ctx는 apply만 제공하고 읽기/생성/삭제/외부 효과 ABI는 이 slice에서 구현하지 않았다(`spikes/spike-v4-worker/src/write.rs:46`). V1은 write access가 공개 전이인지 검사한다(`spikes/spike-v1-fixture/src/sema.rs:1171`·`:1174`). 따라서 확장 안의 조건·컬렉션 처리 자유와 데이터 접근 API의 표현 범위를 구분해야 한다. 실제 artifact는 `club.mjs`·`club.py`이고 TS 컴파일을 실행한 결과가 아니다.

## Q4: 다음 단계 전 기술적으로 정할 것과 창시자 결정

다음 단계 전 기술 작업:

1. **F01 수정 및 반례 고정:** selfRow의 범위를 `by`와 동일한 경로로 제한할지, 별도 기준으로 명시 계산할지 정하고 V1 진단과 실행기를 맞춘다. target scope를 어떤 시점에 고정하는지, 자기 행의 0/복수 매치 오류도 명세에 둔다.
2. **F02 수정 및 오류 계약 통일:** 전체 쓰기 기한의 시작/끝, checks와 commit의 예산, 커밋 전송 전 rollback, 전송 후 COMMIT_UNKNOWN을 V3·V4에서 공유한다. 결과 미확정 때 조회·멱등 재시도 계약을 함께 설계한다. DEADLINE_EXCEEDED를 ‘변경 없음’으로 사용할 수 있는 경계를 명시한다.
3. **F03·F04 권장 수정:** 과거 토큰 오류는 현재 호출을 오염시키지 않도록 분리하고, pending ctx의 성공/실패/취소·늦은 reply·worker 종료를 언어 공통 ABI로 정의한다. 병렬 ctx의 서버 실행 순서 및 미await 작업의 반환 규칙도 정의한다.
4. **호출 수·계약 경계:** 확장 ctx 호출 수와 누적 비용을 어떻게 제한할지, access가 전이 호출 자체와 서버 정의 효과의 관계를 어떻게 설명할지 정한다. 현재 선언은 공개 전이 호출 허용 목록이며 효과가 쓸 모든 resource의 별도 목록은 아니다.
5. **F05 문서 정합성 및 다음 업무 실험:** 현재 제목·숫자·미실험 항목을 고친 뒤, 순환 전이/조건·컬렉션 처리/읽기를 포함한 적격성 판단으로 W1·확장 비교를 넓힌다. 선언 줄 수와 실제 작성·수정 시간·AI의 선택 오류를 별도로 측정한다.

창시자 결정이 필요한 제품 선택:

- 초기 표준 쓰기의 중심을 W0/W1/W2 중 어디에 두고, selfRow·출처·조건·컬렉션 같은 조합 capability를 어느 단계까지 출시할지. 두 업무의 우열만으로 결정하지 않는다(`V3-standard-write-results.md:246`, 통합 지침 `:552`).
- 호출자의 단계 순서·생략·반복 자유를 어느 범위로 제공할지, 자주 쓰는 업무의 안전한 순서를 서버 동작으로 고정할지. 순서 오류의 rollback 검증과 표현 경험 선택은 별개다.
- JS/TS·Python의 초기 지원 기능표와 확장 API 범위. 공개 전이만 ctx에서 호출할 수 있는 현재 실험 규칙을 제품 제약으로 채택할지, 확장 전용 capability를 추가할지. 공식 생태계 지원 목표와 처음부터 모든 기능 동등 지원은 다른 결정이다(통합 지침 `:193`·`:580`).
- 결과 미확정·재시도·명시적인 배치 처리의 사용자 경험과 호환성 약속. COMMIT_UNKNOWN이라는 기술적 사실을 실패/성공으로 숨기는 선택은 하지 않되, 상태 조회·멱등 키의 공개 방식은 제품 계약으로 정한다.

F01~F04의 코드 수정 방법과 ABI 정합성은 우선 기술 검증 대상으로 두며, 위 제품 선택의 결론을 이 검토에서 대신 내리지 않는다.

## 실행 근거 보충

- 범위 밖 격리/DB 직접 연결 사례를 임시 복사본에서 제외한 V4 전체 명령 재실행: 읽기 확장·outbox·쓰기 확장 3개 테스트 모두 성공. 로그 `logs/v4-scope.log`. 최초 전체 실패 로그 `logs/v4.log`도 보존했으며 성공 기록으로 덮어쓰지 않았다.
- V1 추가 계약 검사 `cargo test --offline -q --test r6_contract -- --nocapture`: write의 잘못된 effect는 EFFECT_MISMATCH, 미공개 access는 UNKNOWN_SYMBOL, deadline 누락은 MISSING_ITEM. selfRow의 잘못된 actor 참조/by 원시 필드/누락 scope는 각각 TYPE_MISMATCH/TYPE_MISMATCH/MISSING_ITEM. 원본 V1 의미 검사 일부는 정상 작동하지만 F01의 두 범위 연결 조건만 비어 있음을 대조했다.
- `tests/r6_more.rs`: 불변식 없는 오류 삼킴·잘못된 출력·actor 위장 rollback 및 25회 상한 비교를 단언. `tests/r6_write.rs`: 기한 뒤 성공, 과거 ctx로 다음 호출 실패, Python 취소 종료의 재현 결과 및 역할 상태를 단언. 이 테스트들의 성공은 **발견 재현의 성공**이며 제품 안전성 통과 판정이 아니다.
- 추가 테스트 초기에는 include 대상의 Rust 내부 doc 주석과 문자열 진단 출력 형식 때문에 컴파일되지 않았다. 임시 테스트 harness만 고친 뒤 위 실행 증거를 얻었다. 구현 수정으로 결함을 숨기지 않았다. 일부 shell 후처리에서 zsh의 읽기 전용 변수 `status` 사용이 실패했으며 테스트 로그를 확인하고 `r6_test_rc`로 바꿔 추가 실행의 종료 코드를 검증했다.
- 실제 버전: rustc 1.98.1, PostgreSQL 17.11(Homebrew), Node v26.9.0, Python 3.14.7. `.env`나 secret store를 읽어 얻은 값이 아니다.


최종 확인: P1 2건(F01·F02), P2 3건(F03·F04·F05). 기준 테스트 V1 16개·V3 5개·범위 내 V4 3개, 추가 테스트 4개를 실행했다. PostgreSQL에서 `SELECT count(*) FROM pg_namespace WHERE nspname LIKE 'r6_aip_r6_56xwn867_%'` 결과는 `0`이다. 모든 실험 schema를 제거했다. 저장소의 쓰기 산출물은 이 파일 하나이며, 코드 변경·commit·push는 사용자 금지에 따라 수행하지 않았다.
