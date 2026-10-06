# V4. 공식 확장 worker 실험 결과

> 상태: 검증 필요 (2026-10-04). [E §4](E-technical-risks.md#4-작은-실험의-선후-관계) V4 중 V4-1(읽기 확장), V4-2(외부 효과 outbox 전달), V4-3(쓰기 확장) 실행 기록이다. Extension ABI·worker·전달 구조 결정이 아니다.
> 위치: `../../spikes/spike-v4-worker/`. V1 facts와 V2 집계 실행기를 재사용한다. 로컬 PostgreSQL의 `aip_v4` schema를 테스트 시작에 만들고 끝에 지운다.

## 실제 실행 출력

명령: `cd spikes/spike-v4-worker && cargo test --offline -q -- --nocapture`

```text
Node | 정상: 관리자 자기 동아리 | OK {"approvedApplicants":1}
Python | 기한 초과 | DEADLINE_EXCEEDED (2003ms)
Node | 네트워크 차단 worker: DB 직접 연결 OK {"approvedApplicants":0}, ctx 경로 OK {"approvedApplicants":1}
Python | 네트워크 차단 worker: DB 직접 연결 OK {"approvedApplicants":0}, ctx 경로 OK {"approvedApplicants":1}
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.89s
```

고장 주입: 입력 바인딩·접근 목록·토큰 폐기·출력 검사를 하나씩 끄면 각각 해당 사례가 실패했고, 복원 후 통과했다. 입력 바인딩과 접근 목록은 끄더라도 뒤에서 집계 guard(`ACCESS_DENIED`)와 단독 집계 노출 검사(`NOT_EXPOSED`)가 막았다. 방어가 겹쳐 있다는 뜻이고, 이 fixture에서 각 장치가 단독으로 막는 경로는 시험하지 못했다.

## 1. 구조

```text
호출자 → Rust 서버 invoke("Recruitment.stats", {clubId})
  ├ 입력 계약 검사(선언된 키·타입만)
  ├ ctx 토큰 발급: actor, 허용 집계(access), 집계 입력 고정값(binding), 기한
  ├ worker(Node 또는 Python, 별도 프로세스)에 {invoke, impl, input, token}
  │    확장 코드: ctx.data.aggregate("Apply", "approvedCount", {clubId})
  │    → worker가 서버에 {call, token, op, name, input}
  ├ 서버: 토큰 유효·기한·허용 집계·입력 고정값 확인 → V2 집계 실행(actor는 토큰에서)
  ├ worker → {done, output}
  └ 출력 계약 검사(선언된 키·타입만) → 호출자에게. 끝나면 토큰 폐기
```

통신은 stdin/stdout JSON 줄이다. worker 환경 변수는 PATH만 남기고 지운다. 확장 코드는 C B.7 예시 그대로다.

```ts
export async function stats(input, ctx) {
  const r = await ctx.data.aggregate("Apply", "approvedCount", { clubId: input.clubId });
  return { approvedApplicants: r.value };
}
```

## 2. 결과 (Node·Python 같은 결과)

| 사례 | 결과 | 관련 |
|---|---|---|
| 관리자가 자기 동아리 stats | `{approvedApplicants: 1}` | EQ-10 |
| 다른 동아리 관리자가 자기 동아리 | `{approvedApplicants: 2}` | |
| 일반 회원·익명 | 확장 안 ctx 호출이 `ACCESS_DENIED` → 확장 실패 | 집계 guard |
| 확장이 동아리 id를 11로 바꿔 요청 | `INPUT_BINDING` | EQ-10 "동아리 ID만 바꿔 다른 동아리 집계를 읽지 못해야" |
| 계약 access에 없는 집계 요청 | `ACCESS_NOT_DECLARED` | RK-05 |
| ctx 요청에 `actor: 3` 끼워 넣기 | `UNKNOWN_KEY` | actor는 토큰에서만 |
| 출력 타입 위반(`"many"`), 선언 안 된 출력 키 | `OUTPUT_INVALID` | 출력 검사 |
| 입력 타입 위반, 선언 안 된 입력 키 | `BAD_VALUE` | |
| 기한(2s) 초과 확장 | 약 2003ms에 `DEADLINE_EXCEEDED` | |
| 기한 초과한 호출의 ctx를 다음 호출에서 재사용 | `TOKEN_EXPIRED` | 토큰 수명 |
| 느린 호출의 늦은 응답 뒤 정상 호출 | 정상 | 호출 id로 구분 |
| worker 비정상 종료 | `WORKER_FAILED`, 재기동 뒤 정상 | |
| worker 환경의 DB 관련 변수 | 0개 | |
| **worker에서 로컬 DB 포트로 직접 연결** | **성공** | **격리 없음(§3)** |
| 같은 연결, worker를 macOS `sandbox-exec`(네트워크 차단)로 기동 | 연결 실패, ctx 경로 호출은 정상 | 격리 대안 1(§3) |

## 3. 드러난 것

- **프로세스 분리와 환경 변수 제거만으로는 확장이 서버 정책을 우회하지 못한다고 말할 수 없다.** 확장은 로컬 PostgreSQL 포트에 직접 연결할 수 있었다. 이 개발 DB는 로컬 접속을 비밀번호 없이 허용하므로, 확장 코드가 DB를 직접 읽을 수 있는 상태다. 통합 지침 Q7("확장은 서버 정책을 우회해서는 안 된다. 격리 기술과 실행 방식은 검증 대상")에 대해, 기본 worker는 **ctx 경로 안의 규칙만** 강제한다.
- 격리 대안 1: macOS `sandbox-exec`로 worker의 네트워크를 막으면 DB 직접 연결은 실패하고 ctx(stdin/stdout) 호출은 그대로 동작했다(Node·Python). ctx 설계가 네트워크 없는 worker와 맞는다는 근거다. 다만 `sandbox-exec`는 macOS 전용이고 Apple이 사용 중단 예정으로 표시한 도구다. Linux 배포에서는 네트워크 네임스페이스·seccomp·컨테이너 같은 다른 수단이 필요하고, 확장이 외부 API를 불러야 하는 경우(SC-08)와 충돌한다. 외부 호출을 허용하면서 DB만 막으려면 DB 쪽 계정·인증 분리가 필요하다. 이 비교는 미실행이다.
- 같은 worker 프로세스 안의 호출들은 메모리를 공유한다. 한 호출의 ctx를 전역 변수에 붙잡을 수 있었고, 서버의 토큰 폐기만이 재사용을 막았다. 토큰은 호출 사이 격리 수단이 아니라 수명 제한 수단이다.
- 기한 초과 시 worker를 죽이지 않고 토큰만 폐기했다. 확장 코드는 기한 뒤에도 계속 돌 수 있다(읽기라 데이터 변경은 없음). 쓰기 확장에서는 취소·트랜잭션 수명 계약이 별도로 필요하다(V4-2).
- Python worker가 처음에는 호출마다 모듈을 다시 실행해서 Node와 동작이 달랐다(전역 상태). 모듈 캐시로 맞췄다. 언어별 worker가 같은 의미를 갖는지는 테스트로 고정해야 한다.

## 4. spike가 정한 규칙

| ID | 규칙 | 열린 점 |
|---|---|---|
| V4-R1 | ctx 토큰 = actor·허용 집계·집계 입력 고정값·기한. worker가 보낸 actor 같은 값은 받지 않음 | 토큰 생성은 예측 가능(실험용) |
| V4-R2 | 집계 입력은 확장 계약 binding대로 확장 입력 값에 고정 | 계산된 입력(다른 동아리 목록 등)은 표현 못함 |
| V4-R3 | 출력은 선언된 키·타입과 정확히 일치해야 함 | 중첩 구조·목록 출력 미지원 |
| V4-R4 | 호출이 끝나면(성공·실패·기한 초과) 토큰 폐기 | worker 프로세스는 계속 실행 |
| V4-R5 | worker 환경 변수는 PATH만. 선택적으로 macOS 네트워크 차단 | 파일 격리 없음. 플랫폼별 격리 수단·외부 API 허용 범위 미정 |

## 5. V4-2 외부 효과 전달 (RK-08)

V3-2의 W2 승인은 알림을 같은 트랜잭션의 `aip_outbox` 행으로 남긴다. 별도 소비자가 그 행을 외부 공급자에게 보낸다(`src/outbox.rs`, `tests/v4_2_outbox.rs`). 공급자는 멱등 키를 받는 시험용 대역이다.

```text
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
```

| 사례 | 결과 |
|---|---|
| rollback된 쓰기 | outbox 행 없음, 전달 0 |
| 정상 3건 | 3건 전달, 효과 3 |
| 공급자 일시 실패 2회 | 1회차 1 전달·2 실패, 2회차 2 전달. 효과 3 |
| 공급자 호출 뒤 전달 기록 전 중단 | 다음 소비에서 재전달(호출 2회), 멱등 키(outbox id)로 효과 1 |
| 소비자 둘이 동시에 40건 | 전달 40, 호출 40, 효과 40(`FOR UPDATE SKIP LOCKED`) |
| 항상 실패하는 수신자 | 3회 시도 뒤 격리(dead), 다른 2건은 전달 |

고장 주입: 행 잠금을 빼면 두 소비자가 같은 행을 보내 호출 80·효과 40(멱등 키가 효과 중복만 막음). 멱등 키를 호출마다 바꾸면 중단 뒤 재전달이 효과 2가 된다. 복원 후 통과.

보장 범위: DB 커밋과 외부 효과는 원자적이지 않다. 이 구조가 보장하는 것은 "커밋된 outbox만 전달을 시도, 커밋된 실패 3회까지 자동 재시도 뒤 격리(dead), 전달에 성공하면 같은 멱등 키의 중복 효과 억제"다(r4b F06으로 좁힘). 격리된 행은 공급자가 회복돼도 자동으로 다시 보내지 않는다. 시도 횟수는 커밋된 소비 트랜잭션 기준이라, 공급자 호출 뒤 기록 전에 중단된 시도는 세지 않는다(4번 중단해도 attempts 0). 공급자가 멱등 키를 지원하지 않으면 중단 뒤 재전달은 중복 효과가 된다. 아리아리 원본(커밋 뒤 별도 트랜잭션에서 알림 저장, 실패 시 재시도 없음)과 비교하면 유실이 재시도로 바뀌고 중복은 공급자 계약에 의존한다.

| ID | 규칙 | 열린 점 |
|---|---|---|
| V4-R6 | 효과는 쓰기 트랜잭션 안 outbox 행. 전달은 별도 소비자 | 전달 지연·순서 보장 없음 |
| V4-R7 | 멱등 키 = outbox 행 id | 공급자별 멱등 지원 여부 |
| V4-R8 | 커밋된 실패 3회 뒤 격리 | 중단된 시도는 세지 않음. 백오프·재처리 도구 없음 |

## 6. Codex 검토와 반영

r4([codex-v4-r4](../reviews/codex-v4-r4.md))는 Codex 샌드박스 안에서 `sandbox-exec`를 중첩 실행할 수 없어 격리 대안을 재검증하지 못했고(R4-01, worker stderr 폐기로 원인이 안 보인다는 지적 → stderr를 서버가 보관하도록 수정) 콘텐츠 필터로 중단됐다. 격리를 범위에서 뺀 r4b([codex-v345-r4b](../reviews/codex-v345-r4b.md))의 V4 발견:

| 발견 | 내용 | 반영 |
|---|---|---|
| F05 P1 | ctx 집계가 DB 잠금을 기다리면 선언 기한(100ms)을 넘겨 450ms에 성공 응답 | ctx 작업을 호출 전체 기한 안에서만 기다림, DB 문장 기한을 남은 시간 이하로, 기한 뒤 도착한 완료는 실패. 같은 잠금 재현에서 Node 102ms·Python 101ms에 `DEADLINE_EXCEEDED`. 수정을 되돌리면 원래 문제 재현 |
| F06 P2 | outbox 보장 문구가 구현보다 강함 | §5 보장 범위·V4-R8 정정 |
| F08 P2 | 선언된 nullable 출력 키를 생략해도 통과 | 선언된 키는 값이 null이어도 있어야 함 |
| F09 P2 | V1이 받는 enum 출력을 V4가 항상 거부 | enum·Time·Url·Ref 검사 추가. enum 출력 정상 통과 |
| F10 P3 | 확장 access가 caller용 집계 공개(`expose aggregate`)를 요구 | 기록. 확장 전용 access와 caller 직접 공개를 나눌지는 기술 계약으로 열어 둠 |

r4b는 선언되지 않은 집계·다른 입력 값·선언 밖 출력 키가 ctx 경로를 통과한 사례는 없다고 판정했다.

## 7. V4-3 쓰기 확장

관리자 위임(V3-3)을 JS·Python 확장으로 구현했다(`extensions/club.*`, `src/write.rs`, `tests/v4_3_write.rs`).

```text
extension write delegate {
  input { target: ClubMember.Id; self: ClubMember.Id } output { done: Bool }
  access ClubMember.makeAdmin, ClubMember.resignAdmin    // 공개 전이만
  effect db  deadline 1s  implementation "club.delegate"
}
```

```ts
export async function delegate(input, ctx) {
  await ctx.data.apply("ClubMember", "makeAdmin", [input.target]);
  await ctx.data.apply("ClubMember", "resignAdmin", [input.self]);
  return { done: true };
}
```

서버가 트랜잭션을 열고 ctx 쓰기를 그 안에서 실행한다. 각 ctx 쓰기는 V3의 같은 판정(행 정책·allow·from·잠금)을 actor 권한으로 거친다. 커밋 조건: 기한 안 성공 반환, 이 호출의 ctx 쓰기 실패 없음, ctx 쓰기 20번 이하, 출력 계약, 커밋 검사. 하나라도 어기면 rollback. 커밋 검사·커밋도 같은 기한 안이며, 커밋 전 남은 시간이 300ms 미만이면 rollback, 커밋을 보낸 뒤 기한을 넘기면 `COMMIT_UNKNOWN`(V3와 같은 계약).

| 사례(Node·Python 같은 결과) | 결과 | 역할 상태 |
|---|---|---|
| 정상 위임 | OK | 위임됨 |
| 관리자 아닌 운영진 | 첫 ctx 쓰기 `FORBIDDEN` → 확장 실패 | 그대로 |
| 첫 쓰기 뒤 확장이 예외 | 확장 실패 | 그대로(첫 쓰기 rollback) |
| ctx 오류를 잡아 삼키고 성공 반환 | 실패(`FORBIDDEN` 기록) | 그대로 |
| 임명만 하고 성공 반환 | `INVARIANT_VIOLATED`(커밋 시) | 그대로 |
| 계약 밖 전이(`Recruitment.close`) | `ACCESS_NOT_DECLARED` | 그대로 |
| 첫 쓰기 뒤 기한(1s) 초과 | `DEADLINE_EXCEEDED` | 그대로 |
| 끝난 호출의 ctx를 붙잡아 다음 호출에서 쓰기 | `TOKEN_EXPIRED`, 아무것도 안 씀 | 그대로 |

고장 주입: "ctx 쓰기 실패 뒤 성공 반환 차단"을 끄면 그 사례가 커밋 단계 불변식 위반으로 바뀌어 테스트가 실패했다(이 경우에는 불변식이 뒤에서 막았지만, 불변식이 없는 업무라면 일부만 쓴 상태가 커밋될 수 있었다).

관찰: 쓰기 확장은 "서버 정의 전이를 부르는 순서 있는 코드"다. 같은 순서의 공개 전이를 같은 actor로 한 트랜잭션에서 실행했을 때 W0과 같은 업무 결과를 낸다. 안전은 전이별 정책과 커밋 시점 불변식·사후조건이 맡고, 확장 코드는 순서와 업무 판단만 맡는다. 확장이 DB에 직접 쓰는 경로는 제공하지 않았다(격리 한계는 §3).

### 7.1 Codex r6 반영

[codex-v34-r6](../reviews/codex-v34-r6.md):

| 발견 | 내용 | 반영 |
|---|---|---|
| F02 P1 | 확장 완료 뒤 커밋 검사·커밋이 기한 밖이라, 1s 선언에서 약 1.25s에 커밋 + 성공 응답 | 커밋 전 여유 검사, 검사·커밋에 남은 기한, 커밋 전송 뒤 초과는 `COMMIT_UNKNOWN`. 지연 트리거로 재현, 수정을 되돌리면 원래 문제 재현 |
| F03 P2 | 기한이 지난 이전 호출의 늦은 ctx 쓰기 거부가 다음 정상 호출을 실패시킴 | 이 호출의 토큰인 ctx 실패만 현재 호출을 오염. 같은 순서 재현에서 다음 호출 정상 |
| F04 P2 | Python에서 ctx 대기를 취소하면 늦은 응답 처리 중 worker가 죽음 | 이미 끝난(취소된) 대기에는 응답을 버림. ctx 대기 취소는 응답 대기만 취소하고 서버 쪽 쓰기는 취소하지 않는다(ABI로 명시 필요) |
| F05 P2 | 문서 문구 불일치, 확장 ctx 쓰기 횟수 무제한(W0은 20) | 문구 정정, ctx 쓰기 20번 상한(`CALL_LIMIT`) |

`tests/v4_r6.rs`에 고정했다.

## 8. 하지 않은 것

쓰기 확장의 결과 미확정(커밋 중 기한) 처리, 효과 전달을 공식 확장(worker)으로 구현, 파일 격리·Linux 격리 수단, 동시 호출 부하, worker 기동 비용 측정, SDK에서 확장 호출.

## 9. 프로토타입 연결 전 값·프레임·grant 수명 정합

[구현 전 관문과 실행 경계](../../prototype/README.md)를 먼저 기록하고 기존 V4에 공통 검사를 연결했다. 이 사전 경계 변경은 V6 HTTP·생성 SDK 연결을 포함하지 않았다. 후속 READ 연결은 §10을 따른다. V4의 Legacy Id·기존 권한과 쓰기 판정은 유지한다.

### 값 경계

`check_record`의 Text/Url/Time/Bool/Int/Enum은 V2 scalar parser를 사용한다. Text/Url의 NUL과 불가능한 날짜·시간·offset을 입력 BAD_VALUE, 출력 OUTPUT_INVALID로 거부한다. nullable 키의 존재·Enum 멤버십·정확한 키 검사는 유지하며 원래 worker 문자열을 변환하지 않는다. URL 문법·최종 Time/Id domain을 결정하지 않았다.

Luna의 내부 테스트는 준비 단계의 잘못된 Rust 오류형 경로를 바로잡은 후1 pass/3 fail로 기존 NUL·달력·소문자 t 차이를 재현했다. 실제 Node/Python invoke에서도 잘못된 값이 그대로 반환됐다. 공유 검사 후 정상 윤년·fraction/offset·소문자 t/z와 nullable null은 원문 그대로 통과하고 잘못된 입력/출력은 계약 오류가 된다. Time 출력 반례는 짧은 문자열 대신 기존 검사도 받아들이던 `2026-02-29T12:34:56Z`로 보강했다.

### 수신 프레임과 시작 실패

`WorkerLimits`는 stdout/stderr 프레임 바이트 한도를 명시하며 실험 기본은1MiB/16KiB다. 프레임 크기는 newline·CRLF를 포함한다. `BoundedLines`는 누적 전에 한도를 검사하고 UTF-8·EOF trailing frame·부분 읽기 취소 복원을 처리한다. read/write가 같은 helper를 사용하며 초과·오류·stdout EOF는 worker protocol 폐기와 child 종료 요청으로 이어진다. 이후 호출을 다시 실행하지 않는다. 실패한 쓰기는 기존 트랜잭션 롤백 경로를 따른다.

stderr는 최근20개의 제한된 줄을 보관한다. 초과·잘못된 진단 스트림에는 고정 marker를 남기고 나머지를 sink로 비워 pipe 대기를 막는다. 이 경우 이후 stderr 진단도 버린다. 이는 worker의 CPU·전체 메모리·파일 격리를 제공하는 기능이 아니다. stderr drain task는 Worker가 소유하고 stop/Drop에서 종료한다.

`try_start_with`는 0 한도를 InvalidInput, 없는 실행 프로그램을 NotFound Result로 반환한다. 기존 start/start_with의 실험 wrapper는 expect 동작을 유지한다. 프로그램 부재 검사는 전역 환경을 바꾸지 않고 별도 자식 테스트의 빈 PATH로 Node/Python을 실행했다. 없는 extension 디렉터리를 spawn 실패로 혼동하지 않았다. Tokio1.53.1 실제 소스의 kill()은 wait()도 호출하므로 기존 stop의 wait 부재라는 결함은 성립하지 않았다.

실제 두 언어에서256바이트 stdout 한도 초과는 WORKER_FAILED, 후속 호출도 WORKER_FAILED다. stderr4096바이트 출력 뒤 정상 echo는 유지하고 고정 오류 진단은107바이트였다. 쓰기 worker가 id11을 먼저 변경한 뒤 초과 응답을 내면11/99는 false로 롤백되고, 새 worker의 정상 호출은11만 true로 커밋됐다. 최초 테스트의 SqlState 문자열화 오류와 Legacy rowRead 없는 fixture의 MISSING_TARGET은 테스트 준비 오류로 추적·수정했으며 runtime 결함으로 기록하지 않는다.

### 읽기 grant 수명

새 Luna의 독립 코드 검토에서 완료 Grant의 actor·now·입력 복사본이 HashMap에 계속 남는 경로를 확인했다. 실제 Node/Python72회 반복은 각각72개 보관, BrokenPipe 전송 실패는1개 보관으로 테스트 실패했다. 완료·실패 뒤 Grant를 제거하고 최근 만료 토큰 문자열만 최대64개 보관한다. 최근 토큰은 TOKEN_EXPIRED, 오래된/없는 토큰은 TOKEN_INVALID로 모두 거부한다. 권한·업무 입력 복사본은 만료 목록에 넣지 않는다.

외부에서 취소된 읽기 호출은 다음 유효 읽기 호출 시작에서 정리한다. 끝나지 않는 worker 함수를 호출한 future를 취소한 뒤 정상 read를 실행해 두 언어에서 새 호출 성공·Grant 빈 map·이전 ctx 거부를 확인했다. worker의 취소된 작업 자체를 즉시 중단하는 정책은 별도다.

### 실행과 음성 대조

- `cargo test --offline -- --nocapture`: Rust23개 실패0. 내부 값4·framing7·grant3, 새 실제 경계3·startup2(일반 실행 helper1개 포함), 기존 읽기·outbox·쓰기·r6 회귀4개다. startup parent는 helper를 Node/Python 환경으로 각각 실제 재실행한다.
- `cargo fmt -- --check`, `cargo clippy --offline --all-targets -- -D warnings`: exit0. 기존 fmt 차이·미사용 변수와 explicit drop의 clippy scope 경고를 같은 수명의 lexical block으로 정리했다. 실제 lock을 await 동안 유지하는 버그라고 과장하지 않는다.
- 고장 주입7종은 shared scalar 우회, stdout cap 제거, stderr cap 제거, spawn panic, 완료 grant 정리 제거, send 실패 정리 제거, 외부 취소 후 정리 제거다. 모두 행동 FAILED를 확인한 뒤 원본 바이트를 복원하고 전체23개를 다시 실행했다.

stdout cap 제거 시 실제11=true로 커밋돼 rollback 테스트가 실패했다. stderr cap 제거 시4125바이트 오류 진단으로 실패했다. 로그는 `aip-worker-boundary-controls-tnb9cc2l`의 소유 임시 디렉터리에 보존했다. worker HTTP/typed SDK 연결, 모드별 Id, 파일·운영 격리, 일반 Int 정밀도와 전체 제품 설치는 여전히 미검증이다.

## 변경 이력

- 2026-10-04 V4-1 읽기 확장 실행 결과.
- 2026-10-04 worker 네트워크 차단 대안(macOS), V4-2 outbox 전달 결과.
- 2026-10-04 Codex r4·r4b 반영: ctx 작업 기한, 출력 키 필수, 타입 검사 범위, outbox 보장 문구, worker stderr 보관.
- 2026-10-04 V4-3 쓰기 확장.
- 2026-10-04 Codex r6 반영: 쓰기 확장 커밋 기한·결과 미확정, 이전 호출 간섭 분리, Python 취소, ctx 쓰기 상한.

- 2026-10-04 프로토타입 연결 전 scalar 공유·bounded 프레임·Result 시작·읽기 Grant 정리와 실제 Node/Python 롤백/취소 회귀.


## 10. 프로토타입 공식 READ 확장 연결

[프로토타입의 구현 전 8개 관문과 실행 예시](../../prototype/README.md#공식-read-확장-연결의-구현-전-관문)를 기록한 뒤 V4 read만 additive wire 경로로 연결했다. `invoke`와 write checker/Legacy apply는 유지한다. `invoke_with_wire`·`validate_read`는 선택한 Id 표현을 입출력과 Grant의 ctx 집계 planner에 적용한다. 집계는 V1 의미 검사상 Int이므로 DB row Id encoder를 덧붙이지 않았다. 최종 worker 출력은 wire 규칙으로 검증하고 원문을 반환한다.

V5의 추가 generator family는 공개 READ 이름·입출력·Enum/nullable descriptor로 생성 타입/지문을 만들며 기존 family 산출물은 유지한다. V6 `/extension`은 opt-in 설정·서명 세션·필수 지문·입력 선검사 뒤 configured DB와 호출별 worker를 실행한다. SDK의 같은 connect에 보충 확장 메서드를 제공하며 표준 caller read/apply를 유지한다. SDK는 Id/값·정확한 키·세대/지문을 검증하고 결과를 동결한다. 연결 후 binding 변조와 getter 입력 반례는 실제2개 실패 후 wire/snapshot 고정으로 교정했다.

V4 전체25개·V5 전체20개·V6 전체21개·CLI22개, Rust88개 실패0이다. SDK 경계9개·패키지4개, 기존 read/apply 정의 변경·재기동4개 조합, 새 source/package × safe/decimal × Node/Python8개 실제 SDK/HTTP/ctx 집계도 실패0이다. unknown extension·wrong wire·readonly marker 제거3종도 의도된 TS 진단으로 실패했다. 실제 TCP/Unix socket 양성 대조는 무격리1·MacNetDeny0을 확인했다.

실제 PID로 CPU loop timeout·shutdown·동시4개/5번째 BUSY·슬롯 반환 뒤 정상 호출·child 종료를 확인했다. cooperative 비동기 대기는 kill guard를 꺼도 자연 종료해 잘못된 음성 대조였고, EOF에 반응하지 않는 CPU loop로 강화한 뒤 Node/Python timeout/shutdown에서 child 생존 때문에 실패했다. 인증·필수 지문·pre-DB 입력·네트워크·모듈 경로·동시 상한·child 종료·SDK 출력의 guard8종을 껐을 때 각각 행동 테스트가 실패했으며 소스 복원 뒤 전체 회귀를 재실행했다.

이 연결은 macOS 로컬의 신뢰한 READ demo 코드에 한정한다. 파일·후손 프로세스 격리, DB 실행을 포함한 HTTP 전체 기한, 다른 OS, WRITE HTTP/typed SDK·멱등 복구, TS worker 빌드 전달·Python SDK·운영 인증·배포·최종 ABI/API는 미결이다. JS SDK의 확장 Int는 안전 정수만 받으며 서버의 i64 전체를 정밀하게 표현한다는 보장은 하지 않는다. §8의 SDK 호출 부재는 당시 기록이며 후속 READ에 한해 이 절에서 연결했다.


## 11. 내장 bootstrap·브라우저·입력 전송 기한 후속

[프로토타입 후속 검증](../../prototype/README.md#브라우저read-decoder로컬-bundle-후속-검증)은 Node/Python trusted bootstrap을 binary에 포함해 checkout의 절대 worker 경로를 제거했다. Python은3.11+ safe-path로 cwd의 json/asyncio shadowing을 거부하며 모듈 sibling import 계약을 확대하지 않는다. 실제 Chrome와 재배치 로컬 bundle은 각각6개 흐름으로 표준 read/apply와 공식 READ 집계를 실행했다. 현재 host의 private 실험이며 운영 파일/후손 격리·다른 OS의 제품 설치 증거는 아니다.

WRITE 입력 전송의 별도 후속에서는 실제 Node/Python 확장이 ctx 쓰기 뒤 CPU 루프에 들어가면 다음 큰 입력이 stdin pipe에서 선언 기한을 넘어 대기하는 오류를 재현했다. 기존 호출의 expires를 초기 invoke와 ctx reply 전송에도 적용하고 취소된 부분 JSON protocol을 폐기·child 종료 요청한다. 커밋 전 전송 실패는 DEADLINE_EXCEEDED이고 기존 COMMIT_UNKNOWN 분류·늦은 ctx 거부를 유지한다. DB 연결·트랜잭션 시작·rollback cleanup 전체의 절대 시간 한도를 보장한 것은 아니다. 구체 테스트·최신 수치는 프로토타입 결과를 따른다.


## 12. 호출자 transaction과 WRITE prototype 연결

[WRITE 구현 관문·명령·최신 결과](../../prototype/README.md#write-공식-확장의-실행-결과)에 따라 기존 write 실행 본체를 caller-owned prepare와 Legacy 자동 commit에서 공유했다. prepare의 Err는 caller가 전체 transaction을 rollback/drop해야 한다. 선택 wire는 V2 Id parser/encoder·V3 apply_in_with_wire를 사용하며 ctx reply를 JS parse 전에 변환한다. Node/Python·safe/decimal·i64::MAX decimal Id·잘못된 입력/출력 rollback을 실제 PG로 검증했다.

V6는 확장 효과·출력 검사·기존 principal/wire/key의 멱등 결과를 같은 transaction에 담는다. replay는 worker를 띄우지 않으며 concurrent same-key도 한 번 실행한다. 실제 commit 응답 기한 초과는 COMMIT_UNKNOWN이고 뒤이은 replay가 결과를 확정한다. SDK는 생성 WRITE input/output descriptor와 지문을 pending 확정/캐시 무효화 전에 검사한다. 운영 로그인·최종 API/ABI·쓰기 조합·일반적인 worker 파일/후손 격리를 이 연결로 확정하지 않는다.

정본 Event.confirm·installed SDK·Chrome와 relocated bundle에서 Node/Python의 두 wire WRITE를 실행했다. CPU loop 뒤 기한·동시4개 상한·슬롯 반환·child 종료·shutdown 후 row lock 반환도 확인했다. argv 기록 helper의 부분 파일 race는 순차 지연 기록 RED 뒤 완료 marker로 test만 보완했다. §9~11의 WRITE 연결 부재와 예전 테스트 수는 당시 기록이며 최신 범위/수치는 prototype의 후속 결과를 따른다.


### SQL 대기와 불명 쓰기 후속 경계

[프로토타입 DB 결과](../../prototype/README.md#db-수명과-구조-검증-결과)는 V6가 driver task를 요청과 함께 소유하고 WRITE transaction BEGIN·key lock·replay·worker·결과 INSERT·commit을 선언 기한으로 감싸는 후속이다. commit 시작 전은 DEADLINE_EXCEEDED, 시작 뒤는 COMMIT_UNKNOWN이다. 이전 시도가 미확정인 SDK의 기한/충돌 재시도는 pending을 보존하며 실제 PG 성공 commit 응답 정지·다음 BEGIN 응답 정지 뒤 같은 key 재생으로 검증했다. V4 standalone/legacy 경로의 전체 SQL 수명 보장으로 확대하지 않는다. 운영 격리·최종 ABI/API·WRITE 조합·HTTP 전체 절대 기한은 여전히 미정이다.

유효 세션으로 시작한 WRITE가 실행 중 만료돼도 이미 커밋한 성공 결과는 그대로 보고한다. 실제 Node/Python × 두 wire의4개 지연 worker case에서 DB 효과·idem1개·생성 SDK 결과를 확인했고 다음 요청의 만료 토큰은 거부됐다. 이 처리는 prototype transport의 결과 보고 경계이며 V4 단독 invoke의 인증 제공자나 최종 세션 정책을 새로 정하지 않는다.

후속 [입력·캐시·CLI 결과](../../prototype/README.md#정의-입력캐시-완료cli-출력-후속-결과)는 SDK의 WRITE 완료 뒤 옛 READ 재저장 반례를 닫았다. 공통 cache의 두 wire×표준/공식 WRITE4개와 actor/resource epoch2개를 소스·설치형 SDK에서 확인했고 기존 unknown/TTL/재조회 의미를 유지한다. V4 worker의 격리·최종 ABI나 write 조합 정책을 추가로 확정한 것은 아니다.
