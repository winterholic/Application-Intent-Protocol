# 전송 계층 실험 정확성 검토 (r7)

기준일: 2026-10-04. 범위: V6 전송 서버·클라이언트·e2e, 재사용 V3 apply_in/checks_in 및 V5 캐시. 인증·프로세스 격리는 제외한다. 기준은 창시자 통합 지시의 프로토콜·DD-11·DD-13, RK-08·RK-09, r5 R5-02다.

원본 코드와 결과 문서는 수정하지 않는다. 저장소 산출물은 이 파일 하나다. 사용자 지시의 임시 복사본은 /private/tmp 아래에 두고, V2 기본 schema와 V6 테스트 schema를 검토 전용 이름으로 바꾼다. git·.env·비밀 저장소는 사용하지 않는다. 발견은 확인 즉시 append하며 실행 한계도 기록한다.

## 기존 테스트 대조

임시 복사본의 V2 기본 schema는 `aip_r7_25952_default`, V6 테스트 schema는 `aip_r7_25952`로 치환했다. 코드 동작은 바꾸지 않았다. `spikes/spike-v6-transport/target/`을 빌드 출력 경로로 사용했다.

실행: 임시 `spikes/spike-v6-transport`에서 `PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="/Users/winterholic/development/projects/aip/spikes/spike-v6-transport/target" cargo test --offline -q -- --nocapture`.

```text
ℹ tests 4 / ℹ pass 4 / ℹ fail 0
test result: ok. 1 passed; 0 failed; finished in 0.30s
```

기존 정상 사례인 태그 무효화, 커밋 뒤 응답 유실 복구, 동일 키 replay·다른 요청 거부, actor별 상태 분리 및 최종 회원/outbox 각 1건 단언은 재현됐다. 이 테스트는 미커밋 중 상태 조회, 오류 응답, 서버 파서의 경계 입력을 포함하지 않는다(`tests/v6_e2e.rs:47-57`, `client/e2e.test.ts:9-55`).

## R7-01 · P1 · Q1/Q2: 본문 불일치의 오류 응답이 유실되면 다른 요청의 성공 결과로 복구한다

문제: 같은 actor·키에 다른 요청을 보내면 서버는 `IDEMPOTENCY_MISMATCH`를 반환한다. 그 응답이 유실되면 클라이언트는 이전 요청의 저장 결과를 이번 요청의 성공으로 반환한다. 이번 요청은 실행되지 않았는데 성공했다고 알린다. 다음 단계 전에 상태 복구에도 요청 일치 검사를 적용해야 한다.

근거: `spikes/spike-v6-transport/src/lib.rs:51-66`은 `/apply`에서만 본문을 비교한다. `/status`는 키로 result만 조회한다(`:107-115`). 클라이언트 `client/transport.ts:28-35`는 `st.ok`만 보고 `recovered:true`로 반환한다. 응답 유실 손잡이는 성공·오류를 구별하지 않고 연결을 끊는다(`src/lib.rs:155-158`). 인증과 무관하게 같은 actor에서 발생한다.

실행: 임시 fixture의 actor 1 알림 10·11은 모두 미확인. `apply(MemberAlarm.read, ids:["10"], key:"wrong-result")` 성공 → 같은 키로 ids:["11"] 요청 → 정상 응답 대조는 MISMATCH → 같은 요청에 `dropResponse:true`. 서버·클라이언트 로직은 원본이며 fixture만 추가했다.

```text
MISMATCH direct={code:"IDEMPOTENCY_MISMATCH",ok:false}
lost-response={changed:[10],ok:true,replayed:true,tags:["MemberAlarm"],unchanged:[],recovered:true}
requested-id11-db=f
```

명령: 임시 V6에서 `AIP_R7_CASE=mismatch PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR=<원본 V6 target 경로> cargo test --offline -q --test r7_cases -- --nocapture` → `1 passed; 0 failed`. 해당 단언은 잘못된 성공 반환과 DB 미변경을 확인한다. 키를 재사용한 호출 자체는 허용된 API이며, 문서도 다른 요청 거부를 명시한다(`V6-transport-results.md:43`, `:57`). 키 전용 status의 결과를 무조건 요청 복구로 간주해서는 안 된다. status에 저장 요청 지문을 포함·검증하거나, 동일 본문 `/apply` replay 경로로 확인한다.

## R7-02 · P1 · Q1/Q2: NOT_FOUND를 종결로 간주하고 재송신 결과 미확정 중에 캐시 저장을 재개한다

문제: `/status`의 NOT_FOUND는 실행 중인 미커밋 처리에도 반환된다. 클라이언트는 재송신 전에 `resolveUnknown()`을 호출한다. 재송신까지 통신 실패하면 아직 커밋할 수 있는 원래 쓰기를 미확정으로 추적하지 않는다. 커밋 전 값을 저장하고 커밋 후에도 cache hit로 반환한다. r5 R5-02의 수명 문제가 복구 경로에 남아 있어 다음 단계 전에 수정해야 한다.

근거: `src/lib.rs:111-118`의 상태 조회는 멱등 잠금을 기다리지 않는다. `client/transport.ts:31-38`은 상태의 `code`도 확인하지 않고 `resolveUnknown()` 후 재송신한다. 두 번째 송신은 첫 try/catch의 보호 밖이다. `spikes/spike-v5-sdk/sdk/cache.ts:54-56`, `:73-76`은 unresolved가 0이면 저장하며, 종결 호출은 globalEpoch를 증가시킨다. 기준은 `codex-v5-r5.md:26-38`, RK-09(`E-technical-risks.md:20`)다.

실행: 임시 PG의 알림 40 UPDATE trigger에 `pg_sleep(0.8)`을 설정했다. 실제 첫 요청을 전송한 뒤 `pg_stat_activity.wait_event='PgSleep'`로 처리 중임을 확인하고 첫 응답 유실을 모사했다. 실제 `/status`는 NOT_FOUND. 재송신은 네트워크 실패로 모사 → `apply` reject → 커밋 전 실제 읽기 → 첫 실제 요청의 커밋 응답 대기 → 다시 읽기와 DB 직접 대조. 변경된 코드는 fixture·고장 주입 fetch wrapper뿐이다.

```text
EARLY_RESOLVE during={rows:[{id:40,isChecked:false}],cached:false,stale:false,stored:true}
after={rows:[{id:40,isChecked:false}],cached:true,stale:false} db=t
STATUS_INTERNAL apply_calls=2 status_calls=1 next_read_stored=true
```

명령: 임시 V6에서 `AIP_R7_CASE=cache ... cargo test --offline -q --test r7_cases -- --nocapture` → `1 passed; 0 failed`. 마지막 줄은 `/status`의 JSON `{ok:false,code:"INTERNAL"}`도 NOT_FOUND처럼 재송신하고 저장 보류를 해제한다는 별도 대조다. `INTERNAL`은 커밋 안 됨을 증명하지 않는다.

NOT_FOUND는 “이 조회 시점에 커밋된 결과가 보이지 않음”으로 정의한다. 미확정 상태는 검증된 저장 결과 또는 확정 롤백까지 유지하고, 동일 키 재송신 실패도 같은 미확정 작업에 묶는다. 진행 중 NOT_FOUND 뒤 재송신이 정상 종결되면 최종 태그 무효화가 있으므로 이 낡은 캐시 고착은 발생하지 않는다. 이번 재현의 필수 조건은 종결 전에 저장 보류를 풀고 이후 재송신 응답까지 실패하는 경우다.

## R7-03 · P2 · Q2: 상태 조회 자체가 통신 실패하면 미확정 캐시의 복구 수단이 apply에서 끊긴다

문제: 최초 쓰기와 `/status` 모두 통신 실패하면 `unresolved`가 남는다. 네트워크가 돌아와 읽기에 성공해도 새 결과를 계속 저장하지 못한다. 동일 쓰기를 같은 키로 다시 `apply`해 성공해도 그 정상 성공 경로에는 `resolveUnknown()`이 없다. 연결 인스턴스를 버리거나 노출된 cache 메서드를 직접 호출해야 한다. 미확정 키별 수명과 재개 API를 정할 것을 권한다.

근거: `client/transport.ts:30-32`의 상태 조회 오류는 바깥으로 전파되고 `:40`의 정상 쓰기 완료는 태그 무효화만 한다. V5 `cache.ts:62-66`, `:73-74`는 unknown 호출마다 카운트를 올리고 별도 resolve 호출에만 내린다. `connect`는 캐시 객체를 노출하므로 수동 복구 가능성 자체를 부정하지 않는다(`transport.ts:14`).

실행: 첫 `/apply`와 `/status`에 TypeError 주입 후 fetch 복원, 실제 서버 `/read`를 두 번 호출.

```text
STATUS_NETWORK_FAILURE later_reads_stored=false,false cache_size=0
```

같은 cache phase 명령으로 실행했다. 이 경우 잘못된 cache hit는 없음. 문제는 캐시 재개와 실패한 쓰기 상태 확인이 호출자에게 구조화되어 제공되지 않는 점이다. opts.key를 생략한 호출은 생성 키를 실패 결과에서 돌려주지도 않는다(`transport.ts:23-24`, `:31`), 따라서 실패 후 키 기반 복구를 호출자에게 맡기려면 키도 공개해야 한다.

### 서버 오류 응답의 현재 경계

`{ok:false,code:"COMMIT_UNKNOWN"}` JSON 응답을 fetch 대역으로 반환하면 `apply`가 그대로 반환하고 기존 cache hit를 유지했다(`SERVER_UNKNOWN result={ok:false,code:"COMMIT_UNKNOWN"} cached_read=true`). 코드 근거는 `transport.ts:26-30`, `:40`: throw일 때만 unknown으로 전환한다. **확인 못함: 실제 V6 커밋에서 COMMIT_UNKNOWN 오류 응답 생성.** V6는 V3의 기한 wrapper 대신 `apply_in`·`checks_in`과 직접 commit을 사용한다(`src/lib.rs:72-93`; V3 `src/lib.rs:72-101`, `:691-704`). 문서가 쓰기 기한과 서버 응답 연결을 미구현으로 명시한다(`V6-transport-results.md:63`). 현재 회귀로 별도 P1을 만들지 않으며, 다음 단계에서는 전송 실패·확정 실패·미확정 JSON 응답의 공통 분류를 먼저 정해야 한다.

## R7-04 · P1 · Q3: 1MiB 초과 본문을 거부하지 않고 잘라서 쓰기를 실행한다

문제: 선언 Content-Length가 상한을 넘으면 서버가 앞 1MiB만 읽는다. 앞부분이 유효한 JSON이면 뒤쪽의 잘못된 데이터는 무시하고 쓰기를 커밋한다. “본문 1MiB 상한”을 요청 수락 상한으로 읽을 수 없다. 다음 단계 전에 잘못된 프레이밍·초과 본문은 처리 전에 거부해야 한다.

근거: `src/lib.rs:143`의 `v.parse().unwrap_or(0).min(1 << 20)`, `:149-155`의 잘린 본문 JSON 파싱·handle 호출. 기준은 `V6-transport-results.md:58`의 V6-R3다. 메모리 할당을 1MiB로 제한하는 좁은 의미는 맞지만, 요청 전체 길이나 JSON 전체의 유효성을 보장하지 않는다.

실행: 원본 TCP 서버에 `POST /apply`, actor 1, Content-Length `1048577`. `{"request":{"apply":"MemberAlarm.read","target":{"ids":["20"]}},"key":"oversize"}` 뒤 공백으로 정확히 1048576 bytes를 채우고 마지막 `X`를 추가했다. 전체 본문은 잘못된 JSON이지만 잘린 앞부분은 유효하다. 실제 DB의 알림 20이 변경됐다.

```text
OVERSIZE declared=1048577 invalid_full_json=true db_id20=true
result={changed:[20],ok:true,tags:["MemberAlarm"],unchanged:[]}
```

명령: 임시 V6에서 `AIP_R7_CASE=http ... cargo test --offline -q --test r7_cases -- --nocapture` → `1 passed; 0 failed`. 첫 실행은 검토 harness의 URL 이름 충돌로 중단됐다. 이를 고친 뒤 한 실행은 raw TCP 수신의 ECONNRESET으로 중단됐다. TCP 대조에서 reset과 이미 받은 응답을 함께 수집하도록 harness를 고친 최종 실행은 위 결과와 DB 변경을 단언했다. 원본 서버 오류와 검토 harness 오류를 구분한다. 길이 초과는 clamp 대신 명시적으로 거부하고, 전체 본문 파싱 성공 이후에만 handle을 호출한다.

## R7-05 · P2 · Q3: 최소 HTTP 파서가 메서드·잘못된 길이·잘못된 JSON을 구별하지 않는다

문제: POST로 기술된 쓰기 경로를 GET으로 보내도 실행한다. 잘못된 Content-Length가 먼저 있어도 뒤의 길이로 덮어써 실행한다. 잘못된 JSON을 Null로 바꿔 `/status`에서 “없는 키”로 처리한다. 실험의 좁은 정상 클라이언트는 동작하지만, 프로토콜 경계 오류와 실제 NOT_FOUND를 구분하도록 정리할 것을 권한다.

근거: `src/lib.rs:134`는 요청 줄에서 경로만 추출하고 메서드·버전을 검사하지 않는다. `:141-145`는 중복 헤더를 덮어쓰고 잘못된 길이는 0으로 바꾼다. `:153`은 JSON 파싱 실패를 Null로 바꾼다. `/status`의 키 검증은 `:109`에서 빈 문자열 대체뿐이다. 문서가 HTTP API 확정이 아니라고 명시하므로(`V6-transport-results.md:3`) 특정 HTTP 상태 코드나 query string 지원을 창시자 계약 위반으로 단정하지 않는다.

실행: 같은 http phase에서 raw TCP로 다음 요청을 보냈다.

```text
GET_APPLY result={changed:[21],ok:true,tags:["MemberAlarm"],unchanged:[]}
DUPLICATE_LENGTH invalid_first={changed:[24],ok:true,tags:["MemberAlarm"],unchanged:[]}
PARSE_INVALID apply={code:"BAD_REQUEST",msg:"key 필요",ok:false}; status={code:"NOT_FOUND",ok:false}; read={code:"BAD_REQUEST",msg:"요청은 객체",ok:false}
```

중복 길이 입력은 `Content-Length: invalid` 다음에 실제 길이를 다시 썼다. 같은 길이를 두 번 보낸 대조도 실행됐다. 잘못된 JSON 입력은 `{"key":`였다. 이를 악용한 인증 문제나 다른 프록시와의 요청 혼동은 확인 못함: 인증·프록시 연결은 검토 범위 밖이며 시험하지 않았다.

오류·정상 구분 대조: 없는 경로 `/other`와 `/apply?x=1`은 NOT_FOUND, Content-Length 없는 POST와 chunked POST는 BAD_REQUEST, 본문을 선언 길이보다 짧게 보내고 half-close하면 응답 0 bytes였다. 해당 쓰기들은 실행되지 않았다. `/apply` 키는 빈 값·101 ASCII bytes·102 UTF-8 bytes를 거부하고 100 ASCII bytes를 허용했다. 대소문자가 섞인 `Content-Length`, `X-Spike-Actor`는 정상 파싱됐다. 현재 정확한 지원 범위는 “정확한 경로 + 유효한 Content-Length + 전체 JSON”으로 문서화하고, 미지원 transfer encoding·중복/잘못된 헤더·잘못된 JSON은 별도 BAD_REQUEST로 거부하도록 권한다.

## Q1 정상·경계 대조: 추가 발견 없음

아래 범위에서 서버 멱등 처리 자체의 중복 커밋은 **없음**. R7-01의 잘못된 클라이언트 복구 결과와 R7-02의 조기 종결은 별도다. 실행은 `AIP_R7_CASE=controls ... cargo test --offline -q --test r7_cases -- --nocapture` → `1 passed; 0 failed`이며, 임시 DB를 사용했다.

| 확인 범위 | 실제 입력·결과 | 코드 근거 |
|---|---|---|
| 같은 actor·키·본문 동시 승인 | actor 1, Apply.approve ids:["300"], key:"concurrent" 두 요청. 하나만 replayed:true, 둘 다 changed:[300]. DB 회원/outbox/키 `1\|1\|1` | V6 `src/lib.rs:55-70`, `:84-93`의 트랜잭션 advisory lock → 키 조회 → 결과 기록·commit |
| 같은 키·다른 본문 동시 요청 | 알림 34·35를 같은 key:"concurrent-mismatch"로 동시 전송. 한 성공, 한 IDEMPOTENCY_MISMATCH | `:63-66` |
| 실패 뒤 동일 키·동일 본문 재시도 | 없는 알림99, key:"failed-retry" → MISSING_TARGET; status NOT_FOUND. DB에 알림99 생성 뒤 같은 요청 → ok:true, changed:[99] | `:73-89`: 오류 시 트랜잭션 drop, 성공한 결과만 기록 |
| 본문 객체 키 순서 | ids:["30"], key:"key-order" 성공 후 `{target:{ids:["30"]},apply:"MemberAlarm.read"}`로 순서 변경 → replayed:true | `:51-52`, `:65`: 파싱된 serde_json Value를 직렬화해서 비교. 이번 빌드의 객체 키 순서는 정렬됨 |
| 의미상 같은 Id 표기 | ids:["031"] 성공 뒤 같은 키로 ids:["31"] → IDEMPOTENCY_MISMATCH | 비교는 실행 계획·정규화된 Id 기준이 아니라 JSON 값 직렬화 기준. V3 `src/lib.rs:127-129`는 두 Id를 같은 정수로 변환 |
| 대상 배열 순서 | ids:["32","33"] 성공 뒤 같은 키로 ["33","32"] → IDEMPOTENCY_MISMATCH | 배열 순서는 직렬화에 남음. 객체 키 순서와 다름 |
| 진행 중 status와 재송신 경쟁 | 알림41 UPDATE에 0.8s PG trigger, 진행 중 status NOT_FOUND. 같은 키 재송신은 잠금 뒤 replayed:true; 원래 요청도 ok:true | `:62-70`, `:107-118` |
| 대상 상한·일부 누락 | 알림 ids 11개 → BULK_LIMIT(10 상한). ids:["50","999"] → MISSING_TARGET; DB 알림50은 false 유지 | V3 `src/lib.rs:121-138`, `:253-273`, `:452-459` |
| actor별 키 | 기존 e2e의 actor2로 actor1의 approve-300 조회 → NOT_FOUND | V6 `:108-111`; `client/e2e.test.ts:52-55` |

본문 비교 대상은 outer `{request,key}` 전체가 아니라 `request` 값이다. 객체 키 순서 차이는 허용되지만 배열 순서·Id 문자열 표기 차이는 허용되지 않는다. 이는 “본문이 다르면 거부” 후보 규칙의 현재 구체화다(`V6-transport-results.md:57`). 의미 동등한 요청까지 replay할지는 정책 선택이며, 현재 실행 결과만으로 잘못된 비교라고 판정하지 않는다.

## Q2 늦은 읽기·확정 실패·읽기 오류 대조: 추가 발견 없음

실행: `AIP_R7_CASE=late-read ... cargo test --offline -q --test r7_cases -- --nocapture` → `1 passed; 0 failed`.

```text
LATE_READ first_wire_id36=false fresh_id36=true late_return_id36=true fetch_reads=3
DEFINITE_WRITE_ERROR retained_cache=true write_code=MISSING_TARGET
READ_ERROR code=INTERNAL next_cached=false next_stored=true
```

첫 실제 `/read`의 본문을 수신한 뒤 클라이언트 전달만 보류 → 같은 클라이언트가 알림36 쓰기 → 새 읽기 완료 → 옛 응답 전달 순서다. 옛 wire 값은 false지만 호출자에게 전달된 늦은 결과는 재조회한 true였다. 원본 V5 `cache.ts:26-34`, `:50-59`가 관련 epoch 변경을 보고 재조회한다. r5 R5-01의 옛 구현을 현재 코드 문제로 재기록하지 않는다.

확정 실패 MISSING_TARGET은 변경이 없으므로 기존 cache hit를 유지했다. 읽기 JSON 오류 `{ok:false,code:"INTERNAL"}`는 code가 붙은 Error로 전파되고 캐시에 저장되지 않았으며 다음 실제 읽기는 성공했다(`transport.ts:17-20`, V5 `cache.ts:30`, `:55`). 네트워크/JSON 파싱 실패도 저장 전에 Promise가 reject되는 코드 경로다. 이어서 아래 추가 오류 대조에서 실제 연결 종료와 잘못된 JSON 응답 대역으로 검증했다. 성공·오류 HTTP 상태 자체는 `post`가 검사하지 않으며(`transport.ts:10-11`), 현 서버는 모든 응답을 HTTP 200으로 보낸다(`src/lib.rs:160`). 외부 서버·프록시의 HTTP 오류를 위한 계약은 아직 없다.

## Q1/Q2 전이 효과·사후조건·무효화 태그 대조: 추가 발견 없음

실행: `AIP_R7_CASE=tags-checks ... cargo test --offline -q --test r7_cases -- --nocapture` → `1 passed; 0 failed`. 서버·V3·캐시 로직은 원본이며, 임시 fixture에 사후조건과 update 효과 전이만 추가했다.

```text
CHECK_ROLLBACK failed={code:"CHECK_FAILED",...} db_before=f|0 retry={changed:[60],ok:true,tags:["MemberAlarm"],unchanged:[]}
EFFECT_ROLLBACK failed={code:"INTERNAL",msg:"전이 효과 실패(sqlstate P0001)",ok:false} db_before=PENDING|0|0|0
UPDATE_TAGS result={changed:[37],ok:true,tags:["ClubMember","MemberAlarm"],unchanged:[]} recruitment_cached=false internalNote=null
```

사후조건 대조는 `check hasSchool when member.school != null`을 추가한 알림60이다. member1.school을 NULL로 두면 UPDATE 뒤 검사에서 CHECK_FAILED가 발생하고 알림 false·키 기록 0으로 rollback된다. school을 복원하고 같은 키·본문을 재시도하면 성공한다. V6 `src/lib.rs:73-79`; V3 `src/lib.rs:414-432`, `:697-700`에 부합한다.

효과 대조는 Apply.approve의 outbox INSERT에 오류를 내는 임시 PG trigger를 걸었다. 승인 상태·그보다 먼저 만든 ClubMember·outbox·멱등 키가 모두 rollback되어 `PENDING|0|0|0`이었다. trigger 제거 후 같은 키 재시도는 성공하고 최종 회원/outbox는 각 1건이었다. V3 `src/lib.rs:341-363`의 create·outbox 효과와 V6 트랜잭션 경계에 부합한다.

태그 대조는 기본 알림 변경의 `[MemberAlarm]`, 승인 create 효과의 `[Apply,ClubMember]`, 추가 `MemberAlarm.demote` 전이의 ClubMember update 효과 `[ClubMember,MemberAlarm]`이다. 마지막 전이는 실제 manager 역할을 MEMBER로 바꾸고 Recruitment.internalNote 정책 의존 캐시를 무효화하여 null로 재조회했다. `write_tags`의 전이 대상 + create/update 대상(`src/lib.rs:27-40`)과 실제 DB 변경이 이 세 사례에서 맞는다. 이미 확인된 알림60을 새 키로 다시 쓰면 `changed:[],unchanged:[60],tags:["MemberAlarm"]`이며 보수적인 과무효화다. RK-09는 안전 상위집합을 요구하므로 오류 발견은 없음(`E-technical-risks.md:20`).

notify는 MemberAlarm을 직접 생성하지 않고 outbox를 기록한다(`V3 src/lib.rs:349-363`). 따라서 이 실행의 승인 tags에 MemberAlarm이 없는 것은 누락이 아니다. **확인 못함: outbox worker가 별도 트랜잭션으로 만든 알림의 통보·캐시 무효화 및 실제 외부 전달의 중복 방지.** V6에는 해당 worker 연결이 없다. 회원/outbox 각 1건을 외부 효과 exactly-once나 RK-08 전체 통과로 확대할 수 없다(`E-technical-risks.md:19`).

## 추가 오류 대조와 R7-03 보강

실행: 임시 V6에서 `AIP_R7_CASE=error-paths PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="/Users/winterholic/development/projects/aip/spikes/spike-v6-transport/target" cargo test --offline -q --test r7_cases -- --nocapture` → `1 passed; 0 failed`. 최초 명령은 잘못된 작업 디렉터리로 `no test target named r7_cases`를 반환했고, 임시 V6 디렉터리에서 다시 실행했다.

```text
ACTUAL_READ_DB_ERROR code=INTERNAL next_stored=true
READ_FAILURE drop-response error=TypeError cache_size_before_recovery=0 next_stored=true; invalid-json error=SyntaxError cache_size_before_recovery=0 next_stored=true
STATUS_FAILURE_THEN_SUCCESS retry_ok=true next_read_stored=false
```

첫 대조는 검토 전용 schema의 member_alarm 테이블을 잠시 다른 이름으로 변경해 실제 서버 `/read` 실행에 DB 오류를 발생시킨 뒤 복원했다. INTERNAL을 호출자에게 전달하고 캐시를 오염시키지 않았다. 둘째는 `/read` fetch에 서버의 기존 `x-spike-drop-response:1` 손잡이를 주입해 실제 연결을 끊었다. 셋째는 실제 서버 응답을 받은 후 잘못된 JSON `{broken-json` Response로 바꾸었다. 모두 실패 시 캐시 크기 0, fetch 복원 후 stored:true였다.

마지막은 R7-03의 추가 재현이다. 최초 쓰기·상태 조회 모두 통신 실패를 주입 → 통신 복원 → 같은 키·같은 알림13 쓰기는 실제 서버에서 ok:true → 읽기는 계속 stored:false. 미확정 카운트를 단순 unknown 호출 횟수로 관리하기보다 복구 가능한 작업 키별로 관리해야 한다는 근거다.

## Q4 문서 주장과 실행 근거의 경계

| 문서의 표현 | 이 검토에서 확인한 범위·정정할 점 |
|---|---|
| `V6-transport-results.md:26`, `:30`의 NOT_FOUND·키 없음이면 커밋 안 됨 | 조회 시점에 완료된 결과가 없다는 좁은 뜻은 맞다. 진행 중 요청의 최종 rollback이나 향후 커밋 불가능을 뜻하지 않는다. 실제 진행 중 NOT_FOUND 뒤 같은 원래 요청이 커밋했다. R7-02와 정상 STATUS_RACE 대조가 근거다. |
| `:31`의 같은 키 동시 요청 직렬화 | 동일 actor·키 승인 두 요청과 다른 본문 두 요청으로 확인했다. 원래 e2e 4개에는 동시 사례가 없다. 이 검토가 실행 근거를 보강했다. |
| `:38`, `:49`의 응답 유실 복구·미확정 결과 스스로 확정 | 기존 손잡이는 이미 처리가 끝난 뒤 응답만 끊는다(`src/lib.rs:155-158`). 그 좁은 사례는 맞다. 처리 중 유실·복구 통신 실패·본문 불일치 응답 유실까지 일반화하면 R7-01·R7-02·R7-03에 어긋난다. V3 COMMIT_UNKNOWN의 실제 연결은 별도 미구현이다. |
| `:29`, `:59`의 변경 가능 resource 태그 | 전이·create·update 대상의 안전한 상위집합으로 이번 세 fixture에서 맞다. changed/unchanged 행별 정확 무효화나 후속 outbox worker의 알림 생성 통보를 보장하지 않는다. |
| `:58`의 본문 1MiB 상한 | 할당·읽는 길이만 제한한다. 초과 요청 거부나 전체 JSON 검증은 R7-04에서 반례가 나왔다. |
| `:17`, `:41`의 회원 생성·outbox 1건 | DB 내부 효과의 원자성과 멱등 결과 기록에 대한 근거다. 외부 알림 실제 전달·중복 전달·worker crash 복구 등 RK-08의 나머지 조건을 입증하지 않는다. |
| `:50`, `:63`의 다른 클라이언트 변경 통보·재연결 복구·쓰기 기한 미구현 | 명시된 한계로 유지한다. 구현되지 않은 기능 자체를 이번 회귀 발견으로 집계하지 않았다. |

다음 단계 전 기술적으로 정할 것:

1. 상태 조회에 원래 요청 일치 검증을 연결하고, NOT_FOUND·오류·처리 중·확정 성공·확정 실패를 구별한다. 상태 조회와 멱등 재송신의 경쟁은 같은 키를 유지한 채 처리한다.
2. 미확정 작업을 키별로 추적한다. 재송신·상태 조회 실패 이후에도 저장을 보류하고, 복구 API와 생성 키 공개 방식·종결 시점을 정한다. 서버 JSON 오류 응답과 fetch 예외가 같은 결과 분류를 쓰도록 한다.
3. 본문 초과를 파싱 전에 거부한다. 메서드·길이·transfer encoding·중복 헤더·JSON 파싱 오류와 응답 envelope를 명시한다. spike의 수동 TCP 파서를 다음 프로토콜 구현에도 쓸지, 검증된 HTTP 구현으로 바꿀지는 기술 선택이다.
4. 멱등 저장 요청의 비교 기준을 명시한다. 현재 객체 순서 정규화와 배열·Id 표현 보존을 계약으로 삼을지, 실행 의미 기준으로 정규화할지 결정하고 계약 버전 변화 시의 replay 의미를 정한다.

창시자 결정이 필요한 제품 정책:

- 미확정 쓰기 중 화면이 어떤 상태를 보여 주고 언제 사용자 재시도를 허용할지. 캐시 저장 금지는 화면에 커밋 전 값을 반환하지 않는 보장과 다르다(V5 `cache.ts:54-56`). 자동 복구 횟수·대기 상한과 “결과 확인 중”의 사용 경험을 선택해야 한다.
- 멱등 키 보관·만료 후 재시도 보장 기간, 확정 실패 결과도 보관할지. 지금은 성공만 기록한다(`V6-transport-results.md:57`). 기간 만료 뒤 같은 키 요청을 새 작업으로 볼지에 따라 사용자 재시도 계약이 달라진다.
- 다른 사용자·기기 변경의 화면 데이터 신선도와 재연결 복구 수준. 주기 재조회·포커스 재조회·실시간 통보의 선택은 DD-11의 재조회 기본 방향(`founder-integrated-directive-2026-10-03.md:660-666`)과 DD-13의 상태 동기화 책임(`:668-680`) 안에서 결정한다. 이 검토가 실시간 기능을 필수로 정하지 않는다.

공식 프론트 라이브러리가 프로토콜 인터페이스라는 기준(`founder-integrated-directive-2026-10-03.md:225-238`)은 유지한다. HTTP API의 최종 형식, 초기 프론트 프레임워크나 별도 인증 설계는 이번 파일에서 결정하지 않는다.

## 판정

**P1 3건(R7-01·R7-02·R7-04), P2 2건(R7-03·R7-05), 별도 P3 발견 없음.** 다음 단계 전 핵심 수정은 요청 일치를 확인하는 복구, 미확정 작업의 종결 수명, 초과 본문 거부다. 기존 정상 e2e는 재현됐으며 서버의 같은 키 중복 커밋은 실행한 범위에서 없었다. 코드 수정은 수행하지 않았다.

확인 못함: 실제 COMMIT 송신 뒤 DB 연결 장애에 의한 결과 미확정, 실제 프록시의 HTTP 오류 응답, 외부 효과 전달 및 후속 worker 캐시 통보, 실제 UI의 상태 표시. 인증·격리는 명시적으로 범위 밖이다.

## 최종 검증·정리

보고서의 발견 ID·심각도 집계(P1 3·P2 2), Q1~Q4와 기존 시험·추가 7개 경계 그룹의 기록, 코드 블록 쌍을 검증했다. 지정 검토 원본 6개 파일과 V2 schema 구현 총 7개 파일을 임시 복사본과 대조하여 schema 치환 외 차이 0건이었다.

```text
review structure: P1=3 P2=2 P3=0; Q1-Q4 present
source comparison: 7/7 unchanged (temporary schema substitutions excluded)
execution record: baseline plus 7 boundary groups present
```

임시 schema 잔존 확인 명령: `psql -X -h localhost -d postgres -At -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM pg_namespace WHERE nspname IN ('aip_r7_25952','aip_r7_25952_default','aip_r7_25952_cases')"` → `0`. 모든 시험 schema와 그 안의 trigger·function이 제거됐다. 임시 복사본 `/private/tmp/aip-r7-4_irp44r`도 삭제했다. 재현 입력·순서·실행 출력은 이 파일에 남겼다.
