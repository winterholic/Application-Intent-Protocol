# 세션 토큰 실험 정확성 검토 (r9)

기준일: 2026-10-04. 저장소 산출물은 이 파일 하나다. 원본 코드·결과 문서는 수정하지 않고 git, .env, 비밀 저장소는 사용하지 않는다. 실행 변형은 /private/tmp의 임시 복사본에서만 수행하고, V2 기본 schema와 V6/V8 시험 schema도 검토 전용 이름으로 바꾼다. 발견은 확인 즉시 append한다.

기준: `plan-docs/sources/founder-integrated-directive-2026-10-03.md`의 서버 최종 통제·인증 절, `plan-docs/alignment/E-technical-risks.md` RK-09. V8은 실제 로그인·OIDC·폐기·키 공유를 제외한 실험(`V8-auth-results.md:3,24,43`)으로 평가한다.

## R9-01 · P3 · Q1/Q3: 문서의 서명 대상과 구현의 서명 대상이 다르다

문제: 문서는 HMAC의 입력을 payload로 쓰지만, 구현은 JSON 원문이 아니라 base64url로 인코딩한 첫 세그먼트의 ASCII bytes에 서명한다. 문서식을 JSON 원문 서명으로 구현한 다른 클라이언트/검증기는 호환되지 않는다. 다음 단계에서 wire 명세에 서명 대상을 명시해야 한다.

근거: `plan-docs/alignment/V8-auth-results.md:21`은 `base64url(HMAC-SHA256(서버 키, payload))` 및 `payload = {sub: actor, iat, exp}`라고 한다. `spikes/spike-v6-transport/src/auth.rs:45-46`은 JSON을 B64로 인코딩한 문자열에 `mac(payload.as_bytes())`를 적용하고, `:51-55`의 검증도 인코딩된 세그먼트에 먼저 서명 검증한다. 서명 검증 자체가 빠진 결함은 아니다. 아래 실행 대조에서 두 입력을 별도로 확인한다.

## 실행 격리

임시 복사본: `/private/tmp/aip-r9-n768ka2q`. V2 기본 schema `aip_r9_n768ka2q_default`, V6 `aip_r9_n768ka2q_v6`, V8 `aip_r9_n768ka2q_v8`, 추가 경계 시험 `aip_r9_n768ka2q_cases`. 기존 테스트 복사본에서는 schema 이름만 변경했다. 빌드·로그 출력은 원본 `spikes/spike-v6-transport/target/`에 둔다. 원본 비교를 위해 검토 대상 및 재사용 코드·문서의 SHA-256 목록을 임시 디렉터리에 보관했다.

## R9-02 · P2 · Q1/Q3: 서명은 확인하지만 발급 시각과 actor 값의 계약은 검증하지 않는다

문제: `{sub, iat, exp}` 형식이라고 설명하지만 검증기는 `sub`와 `exp`만 i64로 확인한다. `iat` 누락·잘못된 타입·미래 발급·exp보다 늦은 발급을 거부하는 코드가 없다. 발급기 `issue(actor, ttl)`도 actor를 임의 i64로 받는다. 다음 단계 전에 필수 claim, 시간 관계, actor ID 도메인을 기술 계약으로 정해야 한다.

근거: `spikes/spike-v6-transport/src/auth.rs:43-47,55-60`. `plan-docs/alignment/V8-auth-results.md:21`과 비교하면 문서의 형식보다 수락 범위가 넓다. `iat`를 신뢰 조건으로 쓰겠다는 명시적 정책은 아직 없으므로 미래 iat 수락을 이미 승인된 세션 정책 위반으로 단정하지 않는다. `exp <= 서버 현재 초`만 만료 검사한다. 정상 서버 키로 서명된 payload에 한정된 문제이며, 키 없는 공격자가 payload를 고쳐도 서명을 통과한다는 뜻은 아니다. 아래 추가 시험에서 실제 수락/거부 행렬을 기록한다.

## 기존 테스트 실행 근거

임시 V6에서 `PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="/Users/winterholic/development/projects/aip/spikes/spike-v6-transport/target" cargo test --offline -q -- --nocapture` 실행. schema 이름만 바꾼 기존 테스트 출력:

```text
ℹ tests 7 / ℹ pass 7 / ℹ fail 0
test result: ok. 1 passed; 0 failed (V6 Rust e2e)
test result: ok. 2 passed; 0 failed (V8 unit + server)
```

V6 r7 반영 회귀 3개도 실행됐다(`client/e2e.test.ts:61-110`). 이 출력만으로 만료 중 쓰기·claim 경계·actor ID 공간·만료 후 cache hit를 검증한 것으로 읽지 않는다.

## R9-01・R9-02 실행 대조 및 Q1 수락 범위

임시 `auth.rs`에 private mac을 호출하는 test module만 추가했다. 원래 issue/verify 코드는 변경하지 않았다. 모든 claim 변형은 **동일 서버 키로 새로 서명**했으며 변조한 payload에 기존 서명을 붙인 것과 구분한다. 명령: 임시 V6에서 `PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR=<원본 V6 target> cargo test --offline -q --lib r9_claim_cases -- --nocapture` → `1 passed; 0 failed`.

검토 harness의 문서 append는 처음에 임시 cwd에서 상대 경로를 사용해 FileNotFoundError가 났다. 같은 호출에서 시험은 정상 실행됐고, 문서 기록은 원본 cwd의 절대 대상 파일로 보완했다. 원본 앱 실패가 아니다.

| 실제 서명 입력/변형 | 실제 결과 |
|---|---|
| 정상 `{sub:7,iat:1791063842,exp:1791063902}` | `Ok(7)` |
| `iat` 누락, 문자열 `"tomorrow"`, null, 객체 `{}` | 모두 `Ok(7)` |
| `iat:1791067442 > exp:1791063902 > now:1791063842` | `Ok(7)` |
| 추가 `role:"admin",tenant:"other"` | `Ok(7)`; 검증 결과는 sub만 반환. tenant/role 권한 구현 증거 아님 |
| sub 문자열 `"7"`, 7.0, 누락, null, true, 9223372036854775808 | 모두 `Malformed` |
| sub -1, 0, i64 최소/최대 | 각각 해당 actor로 수락 |
| exp 누락, 문자열, 소수, null | 모두 `Malformed` |
| exp -1, 현재 초 1791063842 | `Expired` |
| exp i64 최대 | `Ok(7)`; 최대 세션 수명 제한 없음 |
| 원문 `{"sub":1,"sub":2,"exp":1791063902}` | `Ok(2)`; 마지막 값 사용 |
| 원문 `{"sub":7,"exp":0,"exp":1791063902}` | `Ok(7)`; 마지막 값 사용 |
| JSON 배열/null | `Malformed` |
| 키 순서 변경·JSON 앞뒤 공백, 변경 bytes로 다시 서명 | `Ok(7)` |
| JSON 원문 bytes에 HMAC 적용 | `BadSignature` |
| base64url 첫 세그먼트 ASCII bytes에 HMAC 적용 | `Ok(7)` |
| 서명 padding `=`, 추가 `.`, 비정규 trailing bits | `Malformed` |
| payload padding을 붙이고 정상 키로 다시 서명 | `Malformed` |
| payload 인코딩을 바꾸고 옛 서명 유지, 토큰 앞 공백 | `BadSignature` |
| 빈 토큰 / 빈 서명 | `Malformed` / `BadSignature` |
| issue(7, 0) | `Expired`; ttl 0은 유효 세션을 발급하지 않음 |

추가 발견 없음: 위 인코딩 변형에서 정상 서명 없이 actor를 바꾸는 우회는 **없음**. 중복 JSON 필드는 Value 파서의 마지막 값 수락이므로 typed claim 계약에 포함할지 정해야 한다(`auth.rs:55-56`). iat 관련 결론은 R9-02이고, exp 최대 수락은 만료 상한 정책 미정이다. `exp=now+1`의 실제 만료 전후 대조는 아래 서버 시험에서 확인한다.

### 발급기 산술 경계 · P3 · R9-02 보강

`issue(7,i64::MAX)`는 debug 시험에서 `attempt to add with overflow`로 panic하며 catch_unwind로 포착했다(`auth.rs:45`). 검증기의 exp 최대 수락과 별개다. 현재 공개 HTTP 발급 경로는 없으므로 원격 서비스 중단 결함으로 승격하지 않는다. 실제 발급 API를 붙일 때 ttl 타입·상한·checked arithmetic과 실패 응답을 정해야 한다. 확인 못함: release 빌드의 동일 입력 결과는 실행하지 않았다.

## R9-03 · P2 · Q2/Q3: actor -1과 익명이 같은 멱등 키 공간을 사용한다

문제: 서명된 sub=-1은 정상 actor로 수락하지만 멱등 저장소는 익명도 -1로 치환한다. 이 actor의 성공 쓰기는 토큰 없는 `/status`와 `/apply` replay로 조회할 수 있다. `V8-auth-results.md:39`의 “다른 사용자의 쓰기 결과를 키로 조회하는 경로도 함께 닫혔다”는 주장은 actor 값 도메인을 제한하지 않으면 성립하지 않는다. 다음 단계 전에 익명과 모든 유효 actor의 키 공간을 구조적으로 분리하거나, -1을 actor 발급·검증·데이터 모델에서 일관되게 금지해야 한다.

근거: `auth.rs:43-47,56-60`은 -1을 발급·수락한다. `src/lib.rs:55,65,110-113`은 `None`을 -1로 대체한다. `:65-72`의 replay는 현재 호출자에 대한 apply 정책 검증(`:75`)보다 먼저 반환한다. 기준은 통합 지시 `:132-134,498-500`의 서버 통제 및 `V6-transport-results.md:44,70`의 actor별 키 분리다. 기존 fixture만으로는 양수 actor 1·2밖에 대조하지 않는다(`tests/v6_e2e.rs:45`, `client/e2e.test.ts:55-58`).

실행: 임시 schema에 member(-1), MemberAlarm(id=3,member=-1)를 만들었다. 원래 `issue(-1,60)`로 발급해 `MemberAlarm.read ids:["3"]`, key=`minus-one` 쓰기를 수행했다. 이어 같은 key·본문을 토큰 없이 보냈다.

```text
authenticated_write={changed:[3],ok:true,tags:["MemberAlarm"],unchanged:[]}
anonymous_status={changed:[3],ok:true,replayed:true,tags:["MemberAlarm"],unchanged:[]}
anonymous_apply={changed:[3],ok:true,replayed:true,tags:["MemberAlarm"],unchanged:[]}
```

새 키로 보낸 익명 apply는 `MISSING_TARGET`이었다. 따라서 정상 익명 쓰기 권한으로 받은 결과가 아니라, 저장된 인증 actor 결과의 replay다. **제한**: 정상 로그인 제공자가 실제로 -1을 발급한다는 근거는 없다. 현재 실험의 공개 발급기는 이를 허용하고 DB/검증기도 거부하지 않으므로 조건부 P2다. 키 없는 공격자가 서명을 위조하거나 일반 양수 actor 결과를 읽는 우회로 승격하지 않는다. 추가 서버 시험 명령과 종료 결과는 아래 실행 기록에 남긴다.

## R9-04 · P2 · Q2/Q3: 재요청의 인증 실패를 원래 쓰기의 확정 실패로 취급한다

문제: 유효 토큰으로 이미 실행 중인 쓰기의 응답이 유실되고 재요청 시점에 토큰이 만료되면 클라이언트는 `TOKEN_EXPIRED`를 반환하며 pending을 삭제한다. 원래 쓰기는 계속 실행되어 커밋할 수 있다. 재요청의 인증 실패는 원래 트랜잭션 rollback의 증거가 아니므로, 재인증 후 동일 actor·key·본문으로 결과를 확인할 때까지 미확정을 보존해야 한다.

근거: `src/lib.rs:190-198`은 요청 시작 시 인증 후 쓰기를 실행한다. 인증 완료된 기존 요청을 exp 도달 시 취소하거나 commit 전에 재검증하는 분기는 없다. 클라이언트 `client/transport.ts:7,31-42,57-66,69-74`는 COMMIT_UNKNOWN·INTERNAL·통신 실패만 미확정으로 다룬다. 이전 요청이 미확정이어도 재요청의 TOKEN_EXPIRED/UNAUTHENTICATED를 `settle()`로 종결한다. token은 connect의 immutable 인자여서 같은 인스턴스의 `retryPending()`에 새 토큰을 전달할 API도 없다(`:20,25-28,69-74`). r7 반영 문서 `V6-transport-results.md:53`의 “확정 응답 전까지 미확정 유지”보다 실제 범위가 좁다.

실행: 임시 fixture의 알림4 UPDATE에 1.7초 pg_sleep trigger를 설치했다. 초 경계에서 ttl=1 토큰 발급 → 실제 첫 `/apply` 전송 → fetch wrapper가 1.1초 뒤 원래 응답 유실을 모사 → 만료된 토큰으로 실제 재요청. 원래 서버·클라이언트 코드는 변경하지 않았다. 첫 요청의 실제 응답을 별도로 보관해 종료 뒤 확인했고 DB도 직접 대조했다.

```text
EXPIRY_WRITE_CLIENT result={code:"TOKEN_EXPIRED",ok:false,recovered:true} pending=[] retryPending=[]
EXPIRY_WRITE_ORIGINAL {changed:[4],ok:true,tags:["MemberAlarm"],unchanged:[]}
EXPIRY_WRITE_DB checked=true idem_count=1
```

동일 actor의 새 토큰으로 **동일 key와 본문을 수동 재전송**하면 `ok:true,replayed:true,changed:[4]`를 받았다. 따라서 서버의 멱등 결과는 살아 있고 중복 commit은 없었다. 문제는 클라이언트가 원래 결과를 잃고 `recovered:true`의 인증 오류를 확정 결과처럼 반환하며 복구 목록을 지우는 점이다. `TOKEN_EXPIRED`가 처음 요청의 인증 단계에서 반환된 경우에는 쓰기 미실행이므로, 최초 인증 거부와 이전 요청 미확정 이후의 인증 거부를 구분해야 한다. 서버가 실행 중 만료를 허용할지는 별도 정책이며 이번 발견의 필수 원인은 클라이언트 종결 판정이다.

확인 못함: 실제 키 교체/다른 서버 재시도로 생긴 UNAUTHENTICATED에서 같은 race를 실행하지 않았다. 해당 오류도 같은 settle 분기를 탄다는 코드 대조만 확인했다. 이번 실험은 기동별 키라 키 교체 자체는 범위 밖이다(`V8-auth-results.md:43`).

## R9-05 · P3 · Q2/Q3: 만료와 인증 오류는 기존 읽기 캐시를 지우지 않는다

문제: 서버가 TOKEN_EXPIRED로 거부한 토큰도 기존 connect 인스턴스의 cache hit에서는 데이터를 반환한다. 다음 단계에서 만료·로그아웃·계정 변경 시 기존 캐시의 표시/제거 정책을 정해야 한다. 서버를 통과한 새 데이터 조회라는 주장과 로컬 보관 데이터 반환을 구분해야 한다.

근거: `client/transport.ts:19-23,48-53`은 token 문자열을 캐시 scope로 설정할 뿐, exp나 인증 오류를 cache lifecycle에 연결하지 않는다. `spikes/spike-v5-sdk/sdk/cache.ts:39-49`는 setActor 변경 때 캐시를 지우지만 cache hit는 네트워크/검증 없이 반환한다. 기준 `E-technical-risks.md:20`은 로그아웃/계정/tenant 변경의 분리·제거를 요구하고 `:61`은 시간 의존 만료를 미착수로 명시한다. V8도 로그인·폐기 정책 결정이 아님을 명시하므로 이미 확정된 세션 정책 위반이나 서버의 인증 우회로 판정하지 않는다.

실행: ttl=1 토큰으로 회원1 알림을 읽어 캐시에 저장 → 1.1초 대기 → 같은 객체의 직접 `/read`는 TOKEN_EXPIRED → 같은 query의 `read()`는 cache hit로 기존 회원1 행 반환.

```text
CACHE_EXPIRY first_cached=false direct=TOKEN_EXPIRED after_cached=true network_calls_after_expired=0
rows=[{id:1,isChecked:true},{id:4,isChecked:false}]
```

다른 actor 토큰으로 생성한 객체는 회원2의 id=2만 반환했다. actor 간 캐시 혼합은 이 범위에서 **없음**. 같은 actor의 새 토큰도 별도 객체라 캐시 공유가 자동으로 되지 않는다. 이 분리는 유용하지만 만료/폐기에 대한 lifecycle 보장은 아니다. 빈 문자열 token을 전달하면 헤더가 생략되어 익명이 된다(`transport.ts:27`); 문서 타입은 string|null이므로 null만 익명으로 허용할지 입력 계약에 명시해야 한다.

## 추가 서버·SDK 경계 시험 실행 기록

명령: 임시 V6에서 `PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="/Users/winterholic/development/projects/aip/spikes/spike-v6-transport/target" cargo test --offline -q --test r9_cases -- --nocapture` → `1 passed; 0 failed; finished in 4.77s`.

임시 `tests/r9_cases.rs`는 원래 Keyring issue와 서버를 호출하고, 임시 `client/r9-cases.ts`는 원래 connect를 import했다. 변형은 시험 fixture·UPDATE 지연 trigger·응답 유실 fetch wrapper뿐이다. 서명/검증/serve_conn/SDK/캐시 구현은 변경하지 않았다. R9-03부터 R9-05까지 위 실행에서 단언했으며 시험 schema는 종료 시 DROP했다.

## R9-06 · P3 · Q1/Q3: 발급기 ttl 산술 overflow

앞서 R9-02 보강에 즉시 기록한 overflow를 별도 발견 번호로 정리한다. 정상 발급 API `issue(7,i64::MAX)`의 `iat + ttl_secs`는 debug 실행에서 panic했다(`auth.rs:43-45`). 현재 HTTP 발급 API나 사용자 ttl 입력 경로는 **없음**이고 실제 로그인도 제외되어 있으므로 P3다. 다음 발급 계약에서 허용 ttl·오류 반환을 정하고 checked arithmetic으로 검증할 사안이다. 확인 못함: release 실행과 실제 외부 제공자의 ttl 전달 경로는 시험하지 않았다.

## Q2 정상·거부 경로 대조: 일반 actor 사칭·검증 실패의 익명 강등 없음

범위: 원본 `serve_conn`의 정식 authorization 처리 및 `/read`, `/apply`, `/status`에 전달되는 Caller 생성 경로를 대조했다(`src/lib.rs:148-169,190-198`). body의 actor/actor_id/sub, 옛 x-spike-actor 헤더로 actor를 바꾸는 경로는 **없음**. 정식 헤더에서 verify 실패를 익명으로 낮추는 분기도 **없음**. 익명과 signed actor=-1의 키 충돌은 R9-03의 별도 발견이다.

| 실제 요청 | 결과 |
|---|---|
| 회원1 토큰 + body actor/actor_id/sub=2 | 회원1 id=1,4만 조회 |
| 회원1 토큰 + x-spike-actor:2 | 회원1 id=1,4만 조회 |
| 토큰 없이 body actor=2 / 옛 헤더 actor=1 | 각각 익명 rows=[] |
| 회원2 토큰 / 토큰 없음으로 회원1 owner-key status | 각각 NOT_FOUND |
| 깨진 토큰으로 같은 status / 만료 토큰으로 같은 status | UNAUTHENTICATED / TOKEN_EXPIRED |
| 같은 회원1의 새 토큰으로 같은 status | ok:true,replayed:true,changed:[1] |
| 같은 key, 다른 본문 status | IDEMPOTENCY_MISMATCH |
| 빈 Bearer, lowercase `bearer` scheme, authorization 중복 | 각각 BAD_REQUEST |
| 정식 Bearer에 깨진 토큰 / 다른 서버 키 토큰 | 각각 UNAUTHENTICATED |

호출자 생성은 토큰 검증 뒤 DB 연결 전이고, 경로를 handle로 넘기기 전에 적용된다. 따라서 `/apply` 및 `/status`의 멱등 replay가 검증을 건너뛰는 경로는 없다. DB 멱등 scope는 sub(actor ID), SDK 캐시 scope는 token 문자열이며 둘은 다르다(`src/lib.rs:55,110`, `client/transport.ts:19-23`). 동일 actor의 새 토큰으로 기존 키 복구는 가능하고 다른 양수 actor 키는 조회되지 않았다.

### 헤더 파서 경계: 정식 검증 실패와 구분

`authorization Bearer broken`처럼 콜론 없는 **잘못된 HTTP 헤더 줄**은 `split_once(':').unwrap_or_default()` 때문에 무시되어 익명 rows=[]였다(`src/lib.rs:155,168,191`). 이는 verify가 실패했는데 익명으로 낮춘 경우가 아니다. 정식 authorization 헤더 자체로 인식되지 않은 구문이다. 문서의 “토큰이 있는데 실패하면 거부”를 임의 바이트 형태의 인증 시도 전체에 일반화하지 않는다. 다음 최소 파서 계약에는 알 수 없는 헤더와 구문이 깨진 헤더를 구분하고 후자를 BAD_REQUEST로 처리할지 명시해야 한다. 확인 못함: 프록시/다른 HTTP parser와의 연계는 시험하지 않았다. lowercase scheme도 현재 엄격한 prefix 지원 범위로 기록하며 외부 HTTP 상호운용 준수 여부를 이번 검토로 보장하지 않는다.

## V8 고장 주입 주장의 재검증

임시 복사본에서만 두 변형을 각각 적용하고 기존 V8 테스트를 실행했다. 명령: `PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR=<원본 V6 target> cargo test --offline -q --test v8_auth -- --nocapture`. 두 변형 모두 실행 뒤 finally로 원래 코드로 복원했다.

| 주입 | 실제 결과 | 감지한 내용 |
|---|---|---|
| `auth.rs:54`의 verify_slice 호출을 생략 | exit=101, 0 passed; 2 failed | unit의 다른 키 기대 BadSignature가 Ok(7). server의 변조/다른 키 토큰이 조회 성공 |
| `src/lib.rs:194`의 서명/형식 실패를 None으로 전환 | exit=101, 1 passed; 1 failed | server의 변조/다른 키/깨진 토큰 기대 UNAUTHENTICATED가 null |

따라서 `V8-auth-results.md:17`의 고장 주입 주장은 이 범위에서 재현됐다. 시험의 양방향 대조이며 변형 코드를 원본에 적용하지 않았다. 서명 비교도 cached dependency `digest-0.10.7/src/mac.rs:168-174`에서 정확한 출력 길이 검사 뒤 `ct_eq(tag)`를 쓰는 것을 확인했다. 서명 값의 상수 시간 비교와 HTTP 처리 전체의 동일 응답 시간은 다른 주장이다. 확인 못함: 타이밍 공격 실험·난수 품질의 통계 검증은 수행하지 않았다. 서버 코드의 32-byte getrandom 호출은 확인했다(`auth.rs:30-33`).

## Q3 문서 주장에 붙일 조건과 다음 단계

| 문서 주장 | 실행으로 확인된 범위/한계 |
|---|---|
| V8:21 토큰 형식 | 인코딩 첫 세그먼트 bytes를 HMAC 대상으로 명시해야 함(R9-01). payload 발급 형식과 검증 수락 스키마는 다름(R9-02) |
| V8:22 서버 키·상수 시간 | 기동별 무작위 32-byte key 생성과 verify_slice의 ct_eq 호출 확인. 키 운영·HTTP 전체 timing 안전성을 뜻하지 않음 |
| V8:23 actor는 Bearer에서만, 검증 실패 거부 | 정식 헤더 경로에서 재현. body/옛 헤더 사칭 없음. 잘못된 헤더 구문은 무시됨. 캐시 hit에는 서버 요청/검증이 없음 |
| V8:39 다른 사용자의 쓰기 결과 키 조회 경로 닫힘 | 양수 actor 1/2 대조는 맞음. 익명=-1 sentinel과 signed sub=-1 충돌은 반례(R9-03) |
| V6:53 확정 응답 전까지 미확정 유지 | 일반 네트워크 복구는 기존 e2e에서 재현. 이전 요청 진행 중 재요청 인증 거부를 확정으로 보는 예외가 있음(R9-04) |
| V6:62 미확정 결과를 스스로 확정 | 동일 유효 actor·키·본문·서비스 키 유지 조건에서 저장 결과 replay. 만료·재인증 연결은 아직 없다 |
| V8:43 하지 않은 것 | 실제 로그인, OIDC, 키 교체/공유, 폐기, tenant 등은 명시된 범위 밖. 이 검토에서도 구현이나 안전성을 확인한 것으로 승격하지 않음 |

### 다음 단계 전에 기술적으로 정할 것

1. 서명 입력 bytes, unpadded base64url 지원, claim 타입/필수 여부/추가·중복 필드 정책을 wire 계약에 명시한다. iat를 단순 발급 기록으로 쓸지 검증 조건으로 쓸지 구분하고 exp 관계·허용 clock skew·최대 ttl를 정한다.
2. actor ID 도메인과 익명 scope를 정한다. -1 sentinel 대신 인증 여부를 포함한 구별된 키 공간이 가장 직접적인 해결 후보다. 양수만 발급한다면 actor 발급·검증·DB 제약에서 같은 조건을 강제해야 한다.
3. 미확정 쓰기는 재요청의 인증 오류만으로 종결하지 않는다. 재인증된 동일 actor로 같은 key·본문을 확인하는 API와 pending 이관/유지 계약을 정한다. 새로운 key로 자동 재시도하지 않는다.
4. 세션 교체에 따른 캐시 제거·표시 정책과 늦게 도착한 응답 취급을 SDK lifecycle에 연결한다. 기존 setActor만으로 token 갱신/폐기/만료가 통합되었다고 보장하지 않는다.
5. 최소 HTTP 파서에서 깨진 헤더·정식 인증 실패·중복 인증·빈 토큰 입력을 구분한다. 대소문자와 whitespace의 지원 범위도 명시한다.

이는 기술 계약 및 실험 후속안이며 창시자 승인으로 취급하지 않는다. HMAC 자체를 다른 로그인/세션 체계로 바꾸는 결론도 내리지 않는다.

### 창시자 판단이 필요한 제품 의미

통합 지시 `:836-849` 기준으로, 다음처럼 사용자 경험이나 운영 가치가 달라지는 경우에만 기술 대조 뒤 선택을 요청한다. 이번 검토에서는 질문을 보내거나 자동 확정하지 않는다.

- 로그인/로그아웃·만료 뒤 기존 화면 데이터를 계속 보여줄지 즉시 제거할지. 여러 탭/기기까지 즉시 폐기를 약속할지. 이는 cached data 표시와 서버의 새 요청 인증을 나눠 정의해야 한다.
- 시작 시 인증된 쓰기가 세션 만료 뒤에도 commit 가능한지, commit 직전까지 유효성이 필요한지. 현재는 시작 시 검증이다. 취소/확정 결과와 사용자에게 보여줄 결과 의미가 달라진다.
- 계정 전환 중 이전 계정의 미확정 쓰기를 어느 화면에서 알리고 복구할지. 저장 결과는 이전 actor로만 확인하되 SDK에서 pending을 안전하게 보존할 제품 흐름이 필요하다.

서명 대상·claim 파서·익명 키 충돌·미확정 추적 오류의 기술 수정 자체를 창시자에게 떠넘길 필요는 없다. 인증 제공자 선택은 실제 신원 확인 통합 자료 없이 이번 실험만으로 추천/확정하지 않는다.

## 심각도 최종 판정

**P1 없음**: 확인 범위에서 일반 양수 actor 사칭이나 무서명 토큰 수락은 발견하지 않았다. 전반 안전성 보증은 아니다.

**P2 2건**: R9-03 익명/actor=-1 멱등 키 충돌, R9-04 만료 재요청으로 원래 미확정 쓰기 추적 소실.

**P3 4건**: R9-01 서명 대상 문서 불일치, R9-02 claim 수락 계약 미명시, R9-05 만료/캐시 lifecycle 공백, R9-06 발급 ttl overflow.

R9-02는 최초 기록의 P2에서 **P3로 낮춘다**. 기본 발급기는 항상 iat를 채우고, 변형은 정상 키를 가진 발급 주체가 서명해야 수락된다. 현재 iat 기반 권한/갱신 정책도 없다. 따라서 지금의 직접 인증 우회보다 다음 검증 계약의 공백으로 분류하는 것이 정확하다. R9-03도 실제 로그인 제공자가 -1을 발급하는지는 확인하지 못한 조건부 P2이고, R9-05는 문서가 시간 만료 미착수를 이미 인정한 경계다.

## 최종 실행·정리 증거

이 문서에서 별도 전체 경로 없이 쓴 `auth.rs`, `src/lib.rs`, `client/*.ts`, `tests/*.rs`는 `spikes/spike-v6-transport/` 아래 파일이다. 임시 시험만 schema 이름을 바꾸었으므로 코드 근거 줄 번호는 원본 기준이다.

고장 주입 복원 후 같은 임시 V6에서 요청된 전체 명령을 다시 실행했다. `PATH="$HOME/.cargo/bin:$PATH" CARGO_TARGET_DIR="/Users/winterholic/development/projects/aip/spikes/spike-v6-transport/target" cargo test --offline -q -- --nocapture` → exit=0. 원본과 동일한 3개 Rust 시험에 경계 unit/server 2개를 추가한 실행이다.

```text
test result: ok. 1 passed; 0 failed (추가 claim unit) / 1 passed; 0 failed (추가 server/SDK)
ℹ tests 7 / ℹ pass 7 / ℹ fail 0; test result: ok. 1 passed; 0 failed (V6)
test result: ok. 2 passed; 0 failed (V8)
```

R9-04는 원래 응답 Promise가 아직 미완료라는 단언과 인증 거부 직후의 실제 PG 대조를 추가했다. `SELECT is_checked, ...idem count..., EXISTS(...wait_event='PgSleep'...)` 결과가 `f|0|t`였다. 이 시점에 pending=[]였고 이후 원래 응답은 ok:true, 최종 DB는 `checked=true idem_count=1`이었다. 따라서 단순히 “이미 commit한 뒤 오류를 받은 경우”로 설명할 수 없다.

정리 확인: `psql -X -h localhost -d postgres -At -v ON_ERROR_STOP=1 -c "SELECT count(*) FROM pg_namespace WHERE nspname IN ('aip_r9_n768ka2q_default','aip_r9_n768ka2q_v6','aip_r9_n768ka2q_v8','aip_r9_n768ka2q_cases')"` → `0`. 시험에서 만든 schema와 trigger/function은 남지 않았다.

검토 전 SHA-256 목록과 비교한 원본 코드·문서 변경은 0개였다. `target/`을 제외한 검토 대상 디렉터리의 새 파일도 이 검토 문서 외 0개였다. `/private/tmp/aip-r9-n768ka2q` 임시 복사본을 삭제했다. 이 기록은 코드 수정·git commit/push를 수행하지 않은 읽기 및 실행 검토다.
