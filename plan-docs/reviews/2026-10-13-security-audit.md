# 2026-10-13 보안 감사: 위협 모델과 발견 목록

> 상태: 진행 중인 감사의 살아 있는 기록이다. 소유자가 직접 요청한 자기 프로젝트의 방어적 점검이며, 수치와 상태는 기록 시점 값이다. 앞선 점검은 [2026-10-07 패턴 커버리지·보안 점검](2026-10-07-pattern-security-audit.md)이고 이 문서는 그 뒤를 잇는다. 설계 결정이 필요한 항목은 [OPEN]으로 표시하고 임의로 정하지 않았다.

## 1. 범위와 방법

- 경로 구분: **제품 경로**는 `aip service`가 쓰는 `spikes/spike-v1~v7`과 `crates/aip-service·aip-auth·aip-execution·aip-migrate`다. **PoC 경로**는 `aip-cli --features poc-runtime`가 켜는 `crates/aip-runtime`과 `aip-pg`의 .aip 언어 실행기다. 같은 심각도라도 제품 경로를 먼저 고친다.
- 방법: 코드 추적, 실제 PostgreSQL 재현 테스트, 읽기 전용 독립 검토 4개 영역(읽기 엔진 인가, 쓰기·전송·세션, 실행·브로커·객체, worker 격리·공급망), OSV 조회. 독립 검토의 지적은 부모가 코드·테스트로 다시 확인한 것만 "확인됨"으로 표시했다. 확인하지 못한 것은 "미확인"으로 남겼다.
- 원칙: 반례를 먼저 실패하는 테스트로 만들고 고친다. 테스트로 재현하지 못한 항목은 고쳤다고 쓰지 않는다.

## 2. 위협 모델

### 2.1 지키려는 것

| 자산 | 설명 |
|---|---|
| 행·필드 가시성 | `rows read when`, `field X read when` 정책이 숨긴 값과 그 존재 여부 |
| 쓰기 권한 | 계약에 선언된 전이만, 허용된 대상 행에만 일어나는 것 |
| 신원 | 서명 세션·JWT·durable principal. 위조·재사용·폐기 후 사용 방지 |
| 서버 자원 | DB 연결, worker, 슬롯, CPU·메모리, 응답 크기 |
| 내부망 | 브로커·webhook·HTTP effect를 통한 내부 주소 접근(SSRF) |
| 비밀 | 서명 키, DB URL, provider 키. 로그·오류·Debug 출력에 새지 않을 것 |

### 2.2 신뢰 경계와 공격자

| 경계 | 신뢰하지 않는 쪽 | 신뢰하는 쪽 |
|---|---|---|
| 브라우저/SDK → `aip service` HTTP | 요청 본문·헤더·토큰·Origin·Id·필터 값 | 서버 계약(정의), 서버 설정, 검증된 principal |
| 서버 → PostgreSQL | 호출자가 만든 값 | 바인드 파라미터, 정의에서 온 식별자 |
| 서버 → 확장 worker(Node/Python) | 확장 코드 출력, ctx 호출 인자 | 서버가 발급한 grant와 선언된 capability |
| 서버 → 외부 HTTP/브로커 | DNS 응답, 리다이렉트, 응답 크기 | 운영자가 설정한 목적지(공개 주소 검증 뒤) |
| 정의 작성자 → CLI | 거의 신뢰함(자기 정의). 단 CLI가 abort하면 안 됨 | |

공격자 유형: ① 익명 네트워크 호출자 ② 인증된 일반 사용자(다른 사용자 데이터를 노리는 BOLA/IDOR, 숨은 필드 추론) ③ 폐기·만료된 신원 보유자 ④ 악의적·결함 있는 확장 코드 ⑤ 외부 시스템(SSRF 대상, webhook 발신자) ⑥ 같은 DB를 쓰는 다른 앱.

### 2.3 점검 관점별 상태

| 관점(OWASP API 2023 대응) | 제품 경로 상태 |
|---|---|
| 객체 수준 인가 BOLA(API1) | `/apply` 대상 행에 rowRead를 WHERE로 걸고 없는 행과 안 보이는 행을 구분하지 않음. 관계 확장·필터·정렬·집계는 S-01과 2026-10-07 항목으로 막음 |
| 인증(API2) | RS256 고정, iss·aud·exp·nbf·kid·jku 검사, 매 요청 DB에서 principal 재확인. 남은 것은 O-06 |
| 속성 수준 인가·mass assignment(API3) | `/apply`는 `{apply,target}`만 받고 `to` 값은 계약에 고정. create/bundle은 HTTP에 없음. O-01·O-02는 [OPEN] |
| 자원 소비(API4) | 본문 1MiB·헤더·IN 50·limit·cost·깊이·deadline·연결 상한. S-07·S-08 보강. O-04·O-05 남음 |
| 기능 수준 인가(API5) | 경로별 세션 요구, `/capabilities`는 세션 필요 |
| 민감 흐름(API6) | 멱등 키는 (principal,key) 범위 |
| SSRF(API7) | DNS 고정·리다이렉트 금지·프록시 무시·공개 주소만. S-02·S-05·S-11로 보강 |
| 설정 오류(API8) | CORS 정확 일치·`null`·`*` 거부·Vary, 쿠키 인증 없음(CSRF 해당 없음) |
| 인벤토리(API9) | 제품 경로와 PoC 경로를 이 문서에서 구분 |
| 안전하지 않은 외부 API 사용(API10) | 브로커 응답 크기·timeout, webhook 서명 상수 시간 비교 |

## 3. 발견 목록

심각도는 영향×도달 가능성의 감사자 판단이다. "재현"은 실패하는 테스트로 확인했다는 뜻이다.

### 3.1 조치 완료

| ID | 심각도 | 경로 | 발견 | 재현 | 조치 | 회귀 테스트 | 커밋 |
|---|---|---|---|---|---|---|---|
| S-01 | 높음 | 제품 | 출발 Ref 필드에 `field read` 정책이 있어도 단일 traverse가 대상 행과 연결 관계를 반환(관계 확장 누출) | 실제 PG, 본인·타인·익명 | lateral join에 출발 필드 정책을 AND | `spike-v2-read/tests/security_authorization.rs` | ee62971 |
| S-02 | 중간 | 제품 | `https://[::1]/` 같은 괄호 IPv6 literal이 `IpAddr`로 파싱되지 않아 vetted 목적지 검사를 건너뜀. `0.0.0.0/8`도 공개 주소로 취급 | 단위 테스트 | 괄호 제거 후 literal 검증, 첫 옥텟 0 거부 | `aip-execution/tests/integrations.rs` | 972e1a6 |
| S-03 | 낮음 | 제품 | JWKS RSA 키의 공개 지수 e=1·짝수·선행 0 허용 | 단위 테스트 | 구성 단계에서 거부 | `aip-auth/tests/jwt.rs` | c90a77b |
| S-04 | 낮음 | 제품·PoC | webhook·outbound 서명 timestamp 극단값(i64::MIN)에서 `abs()` overflow(debug panic, release wrap), Decimal 입력 `i64::MIN.abs()` overflow | 단위 테스트 | `abs_diff`, 범위 비교 | `outbound.rs`, `webhook.rs`, `validate.rs` 테스트 | ffcc9b1 |
| S-05 | 낮음 | PoC | outbound 공개 IP 정책이 6to4·Teredo·3fff::/20·5f00::/16·192.88.99.0/24를 막지 않음 | 단위 테스트 | global unicast만 허용 | `outbound.rs` 테스트 | ffcc9b1 |
| S-06 | 중간 | 제품 | 비-legacy Id wire에서 1:N traverse 자식 Id가 변환되지 않아 2^53 초과 Id가 JS에서 인접 Id로 반올림됨(다른 행을 가리킴, SafeNumber 범위 검사도 건너뜀) | 단위 테스트로 재현 | 출력 타입 `array`도 Id wire 변환 | `spike-v2-read/tests/v13_idwire.rs` | 5a3f1b0 |
| S-07 | 중간 | 제품 | 인증 프레임을 보내지 않는 익명 브라우저 WebSocket이 realtime 슬롯(기본 32)을 5초씩 점유해 인증된 사용자가 `SERVER_BUSY`를 받음 | 통합 테스트(슬롯 1개 서버) | 인증 전용 pre-auth 슬롯을 분리하고 인증 뒤 정식 슬롯으로 승격, Drop으로 반환 | `spike-v6-transport/tests/browser_websocket.rs` | 881a858 |
| S-08 | 중간 | 제품 | `accept()` 오류 한 번(EMFILE 등)이 listener와 진행 중 요청 전체를 종료 | 코드 추적(EMFILE 재현은 프로세스 한도 변경이 필요해 미실행) | 오류를 기록하고 50ms 쉰 뒤 계속 | 없음(미실행: EMFILE 재현) | 881a858 |
| S-09 | 낮음 | 제품 | ctx grant 토큰이 호출 번호와 `Instant::now().elapsed()`(수십 ns)라서 사실상 예측 가능. worker를 재사용하면 시간 초과된 이전 호출의 코드가 다음 호출의 grant를 추측할 수 있음 | 코드 추적. 현재는 요청마다 새 worker라 도달 불가 | OS 난수 128비트 | `spike-v4-worker/src/grant_tests.rs` | d2aee36 |
| S-10 | 낮음 | 제품 | 단일 traverse select에 같은 필드를 반복해도 거부하지 않아 SQL이 부풀고 PG 인수 한도에 걸림 | 테스트 | `DUPLICATE` | `security_authorization.rs` | 5a3f1b0 |
| S-11 | 낮음 | 제품 | `brokers.rs`의 공개 주소 정책이 `integrations.rs`와 갈라져 6to4·NAT64·IPv4-compatible·Teredo를 공개로 판정 | 단위 테스트 | 정책을 한 함수로 통일 | `brokers.rs` 단위 테스트 | cd7d252 |
| S-12 | 낮음 | CLI(PoC 겸용) | `aip check`가 괄호·단항 연산 중첩 500단계에서 스택 오버플로 abort, `99999999999GB`·`99999999999y`에서 산술 overflow panic(release는 조용히 wrap) | 테스트로 SIGABRT 재현 | 중첩 64단계 한도, 포화 곱셈 | `aip-syntax/tests/hostile_input.rs` | 4fdcfda |
| S-13 | 낮음 | PoC | 문자열 리터럴 안의 `$aip$`가 트리거 본문의 dollar-quote를 끝냄(기본 PG 설정에서는 마이그레이션 실패, `check_function_bodies=off`면 임의 SQL 가능성은 미확인) | DDL 출력 확인 | `$`를 `U&'\0024'`로 출력 | `aip-pg/src/names.rs` 단위 테스트 | 544300b |
| S-14 | 높음 | PoC | select의 계산 항목(`p: m.phone`)·where·정렬 키·group 키가 필드 `visible to`/`masked unless`를 우회해 숨긴 값을 드러냄(P-01·P-02 일부) | 실제 PG, 본인·타인 | 클라이언트 읽기 경로에서 필드 읽기를 정책 조건으로 감쌈(`guard_projection`). 정책식 안의 읽기는 다시 감싸지 않음 | `aip-cli/tests/field_policy_e2e.rs` | 34360be |
| S-15 | 중간 | 제품 | 서명이 유효한 토큰의 principal 조회가 요청마다 새 PG 연결을 열어 rate limit보다 먼저 연결 수를 늘림(O-04) | 코드 추적 | 조회 연결 동시 16개로 제한, 대기 5초 | 없음(미실행: 공유 DB에서 동시 연결 수 측정이 불안정) | 0b5b8df |
| S-16 | 낮음 | 제품 | 헤더 이름 뒤 공백(`Content-Length : 5`)을 수용(O-12) | 테스트 | `BAD_REQUEST` | `spike-v6-transport/tests/v8_headers.rs` | 72e191a |
| S-17 | 중간 | 제품 | `rows read when not actor.suspended` 같은 `not <Bool 경로>`가 `NOT (경로 IS TRUE)`로 컴파일되어 익명 actor(행 없음, 경로가 NULL)가 통과(fail-open). 같은 의미의 `actor.suspended = false`는 거부되어 일관되지 않음 | 실제 PG: 결과 `[1,0,1]`(기대 `[1,0,0]`) | `not <Bool 경로>`는 `경로 IS FALSE`로 컴파일. DDL 부분 인덱스 조건은 기존 의미 유지. 복합식(`not (a and b)`)과 `not exists`는 바꾸지 않았다(O-26 참고) | `spike-v2-read/tests/negated_policy.rs` | 87208a5 |
| S-18 | 낮음 | 제품 | TS 클라이언트가 redirect를 따라가고(307/308은 본문 재전송), signed URL 검증이 `//host`·`/\host`를 통과시키며 signed 헤더에 `authorization`·`cookie`가 들어갈 수 있음 | 테스트 | `redirect:"error"`, `credentials:"omit"`, 프로토콜 상대·역슬래시 URL 거부, 자격 증명 헤더 거부 | `client/infrastructure.test.ts` | 04977cd |
| S-19 | 낮음 | 제품 | Python SDK(`urllib`)가 302 redirect에서 `Authorization`을 새 origin으로 보냄, `file://`·`ftp://`·원격 평문 http 허용 | 테스트로 토큰 유출 재현 | redirect 거부, https 또는 loopback http만 허용 | `product/tests/python_sdk_test.py` | 8b6d9d8 |
| S-20 | 낮음 | 제품 | 읽기·operation 확장의 ctx DB 호출 횟수 상한 없음(O-09) | 테스트(Node·Python) | 64회 초과 시 `CALL_LIMIT` | `spike-v4-worker/tests/v4_1.rs` | 2d93c09 |
| S-21 | 중간 | 제품 | 사용자 경로 `/jobs/enqueue`가 `schedule:`·`webhook:`·`outbox:` 접두어 key를 받아 서비스 주체가 사람 subject일 때 정식 slot·배달을 `IDEMPOTENCY_MISMATCH`로 선점 가능 | 테스트 | 사용자 경로에서 예약 접두어 거부 | `spike-v6-transport/tests/durable.rs` | 75d602c |
| S-22 | 낮음 | 제품 | cron step `59/4294967295`가 release에서 wrap되어 매 분 실행, debug에서 panic | 테스트(`attempt to add with overflow`) | `checked_add` | `schedule.rs` 단위 테스트 | 75d602c |
| S-23 | 낮음 | 제품 | `expose apply bulk maxRows`에 상한이 없어 id 중복 검사 O(n²)와 `bulk + 1` overflow | 테스트 | 1~10000 | `spike-v1-fixture/tests/sema_fixes.rs` | 49dcb91 |
| S-24 | 정보 | 제품 | S-07 변경이 `spike-v6-transport` lib 테스트 빌드를 깨뜨림(`Runtime` 필드 추가). `tools/verify.sh`는 v4·v6 테스트를 돌리지 않아 못 잡았고 GitHub CI(`verify.yml`)만 잡는 구조. `hosting.rs` 임시 디렉터리 이름이 macOS 시계 해상도로 충돌해 간헐 실패 | 실행 | 필드 추가, 이름에 카운터 | 〃 | 04977cd, 2b6d977 |

### 3.2 열려 있는 항목(제품 경로)

| ID | 심각도 | 발견 | 상태 |
|---|---|---|---|
| O-01 | 중간 [OPEN] | **실제 PG로 재현**: `Task.team: Team?`, `Team rows read when owner = actor`, `expose read { select id, team }`에서 member 1이 읽으면 자기 팀이 아닌 task 2의 `team: 11`도 받음. 루트 select에 Ref 필드를 직접 열면 대상 행 정책과 무관하게 원시 FK Id가 나감. traverse·Ref filter는 안 보이는 대상을 null로 취급하므로 "숨은 대상 Id 확인" 방어가 select 경로에서는 무의미해짐 | 코드 추적. 고치면 Ref 출력이 nullable로 바뀌어 SDK 타입과 계약 지문이 달라지므로 창시자 결정 필요. 권고: 정의 작성자가 FK 노출을 선언(`expose`)하거나 기본을 정책 적용으로 |
| O-02 | 낮음 | `transition`의 `from`/`to`/`repeat unchanged`가 field read 정책 필드를 참조하면 `/apply` 응답의 `changed`/`unchanged`·`INVALID_STATE`로 이전 값을 추론할 수 있음 | 미재현. sema가 이 조합을 막는지 확인 필요 |
| O-03 | 낮음 [OPEN] | `/read`·`/apply`·`/status`는 `X-AIP-Contract` 헤더가 없어도 통과(다른 경로는 필수). 권한 우회는 아님 | 설계 결정. raw 클라이언트 호환과 맞바꿈 |
| O-04 | 낮음 | 위 S-15로 연결 수는 16개로 묶였다. 인증 조회 자체는 여전히 admission 앞에서 실행되므로 풀 연결·rate limit 통합은 남음 | 부분 조치 |
| O-05 | 낮음 | 슬롯(realtime·worker·업로드)이 전역 공유라 인증된 한 명이 독점 가능. rate limit 기본 꺼짐 | 코드 추적. principal별 상한 설계 필요 |
| O-06 | 낮음 | JWKS 파일 모드가 토큰마다 파일을 읽어 전역 mutex 안에서 파싱, HTTPS 모드는 서명 검증 전 알 수 없는 kid가 5초 간격 갱신을 유발해 캐시된 키까지 5초 `Unavailable` | 코드 추적 |
| O-07 | 낮음 | owner 해시에 `min_iat`가 들어가 revoke 뒤 재바인드하면 기존 파일·job·event 접근 불가(정합성) | 코드 추적. 보안 우회 아님 |
| O-08 | 낮음 | rate limiter scope 10만 행 한계에서 신규 principal이 503 | 코드 추적, 운영 규모 의존 |
| O-09 | 낮음 | 읽기 확장 ctx 호출 횟수 상한 없음(쓰기 20, job 64) | deadline·slot이 상한 역할 |
| O-10 | 낮음 | 응답을 쓰는 동안 DB 풀 연결을 보유 | TLS proxy 버퍼링으로 대부분 완화 |
| O-11 | 해당 없음 | `spike-v3-write`의 `internal(e)`가 DB 오류 원문을 보낸다는 지적 | 확인 결과 `internal`에 들어가는 것은 정의 컴파일 단계 문장뿐이고 DB 오류는 sqlstate만 응답. 조치 없음 |
| O-12 | 완료 | 헤더 이름 공백 수용 | S-16으로 조치 |
| O-13 | 중간 | macOS 기본 worker 격리는 네트워크만 차단. 파일 읽기·쓰기와 `start_new_session` 손자 프로세스는 막지 못함. `stop()`은 직계 자식만 정리 | 문서가 "신뢰한 확장만"을 전제. Linux는 bwrap/cgroup |
| O-14 | 낮음 | RAG corpus 용량이 owner 합산이라 한 principal이 다른 principal의 색인을 막을 수 있음(`client_index:true`일 때). 검색은 owner 파티션 전체를 메모리에 올림 | 코드 추적 |
| O-15 | 정보 | 의존성: `rsa 0.9.10`(RUSTSEC-2023-0071, Marvin)은 `jsonwebtoken`을 통해 들어오고 이 코드는 검증만 하므로 개인키 연산 노출은 없음. `rustls-pemfile 2.2.0`(RUSTSEC-2025-0134)은 유지보수 중단 공지. 나머지 436개 crates.io 패키지는 OSV 조회에서 알려진 취약점 없음(`cargo audit` 미설치, OSV `querybatch`로 대체). `npm audit` 0건, install script 없음 | 2026-10-13 조회 |
| O-16 | 낮음 | 63바이트 초과 식별자 검증 없음(`aip-pg` DDL, 이름 충돌 시 인덱스 누락·제약 오류 사유 대체). `aip-migrate`는 63바이트 검증 있음 | 코드 추적 |
| O-17 | 낮음 | `evolve`가 `public` 스키마의 AIP 소유가 아닌 `__` 포함 트리거·`__…_fn` 함수를 삭제할 수 있음 | 코드 추적 (PoC 마이그레이션) |
| O-19 | 중간 | trigger(webhook·schedule·outbox) 대상 operation은 일반 job이라 `allow`가 열려 있으면 인증된 사용자가 `/jobs/enqueue`로 `{body:"위조"}`를 직접 넣어 HMAC 검증을 우회할 수 있음. 실행은 호출자 자신의 권한이지만 확장이 body 출처를 신뢰하면 업무 규칙이 깨짐 | 코드 추적. trigger 전용 operation 표식이나 `allow actor = 서비스 actor` 강제와 문서 경고 필요 |
| O-20 | 중간 | 서비스 주체 job 이력은 보존 상한 면제인데 만료 정리는 분당 100건, 이벤트 key ledger·`aip_outbox_dispatch`·`aip_outbox`는 정리가 없고 job 입출력 바이트 quota도 없음(files 10MiB·messages 64MiB와 불일치). outbox relay는 매초 topic별 전체 이력을 anti-join | 코드 추적(부하 미측정) |
| O-21 | 낮음 | 실행 중 job의 250ms heartbeat가 principal 재검증에서 한 번 `DB_UNAVAILABLE`을 받으면 worker가 종료되고, PENDING http effect가 있으면 영구 `EFFECT_UNKNOWN`. S-15의 조회 연결 상한(16)과 겹치면 포화 시 증폭 | 코드 추적 |
| O-22 | 중간 | 런타임과 migration이 같은 DB 역할을 씀. `serve`가 시작할 때 `CREATE SCHEMA/TABLE`을 하므로 서버 프로세스가 사실상 owner. 최소 권한 가이드 없음 | 코드 추적. migrate 전용 URL과 runtime 전용 URL 분리 권고 |
| O-23 | 낮음 | `/ready`가 인증 없이 전역 `Semaphore(1)`을 쓰고 `DEPLOYMENT_CHANGED`/`DB_UNAVAILABLE`을 구분해 응답. 공개 프록시에 노출되면 flood가 LB의 정상 probe를 막음 | 코드 추적. probe는 내부 listener 권고 |
| O-24 | 낮음 | migration `plan_digest UNIQUE`가 A→B→A→B 순환의 두 번째 적용을 롤백시킴. `requiresReview`는 보고만 하고 apply에서 강제하지 않음. Time→Date 변환이 세션 TimeZone에 의존 | 코드 추적 |
| O-25 | 중간(가용성) | `check`/정원 제약이 선언된 resource가 있으면 관련 없는 전이의 `/apply`마다 각 resource에 `xmin = pg_current_xact_id()` 전체 스캔(인덱스 불가). 테이블이 커지면 `statement_timeout` 2초에 걸려 전 쓰기가 `DEADLINE_EXCEEDED` | 코드 추적. 이번 트랜잭션이 건드린 resource만 검사하도록 추적 필요 |
| O-26 | 중간 | 행의 부재에 의존하는 정책(`allow not exists Booking b where b.room = r.room and b.status = CONFIRMED`)은 서로 다른 행을 동시에 바꾸는 요청을 막지 못함(이중 예약). `atMost 1` 불변식만 막음. sema가 경고하지 않음. 설계 문서 V3-R5에 한계로만 있음. `not exists`가 익명 actor에게 열리는 것은 의미상 옳지만 정의 작성자가 `actor != null`을 써야 함 | 코드 추적. sema 경고 권고 |
| O-27 | 낮음 | 불변식 위반 코드(`ALREADY_EXISTS`, `INVARIANT_VIOLATED`)로 `rows read`에 가려진 행의 존재·정원 상태를 추론할 수 있음 | 코드 추적. 설계상 허용 여부 결정 필요 |
| O-28 | 낮음 | 쓰기 판정의 `now`가 트랜잭션 전에 고정되어 멱등·행 잠금 대기(최대 2초)만큼 마감을 지난 뒤에도 `allow now < deadline`이 통과할 수 있음. 비-`maxRows` update effect의 락 순서 비고정(교착 시 `CONFLICT` 재시도). advisory lock 키가 32비트 해시를 공유 | 코드 추적 |
| O-29 | 낮음 | TS 클라이언트: 응답 크기·content-type 검사 없음, 응답 유실 뒤 `BAD_REQUEST`를 받은 pending 쓰기를 버릴 방법 없음, `WriteUnsettled` 뒤 키 없이 `apply`를 다시 부르면 새 키로 중복 쓰기. 배포 artifact의 `bin/aip`에 로컬 경로(`~/.cargo/registry`)가 들어가고 `SHA256SUMS`는 서명 없음 | 코드 추적, `strings` 확인 |
| O-30 | 낮음 | `__proto__`라는 필드·resource 이름은 생성 descriptor에서 own key가 아니라 prototype 설정으로 해석되어 해당 이름을 쓰는 읽기·확장 호출이 항상 거부됨(실패는 닫힌 방향) | 코드 추적 |
| O-31 | 낮음 | cron 일·요일 필드가 `*/2` 형태면 OR로 평가(Vixie는 AND). webhook 본문이 JSON escape 뒤 1MiB를 넘으면 영구 실패인데 503+`Retry-After`로 재시도 유도. job key에 deployment digest가 있어 배포 중 provider 재전송이 새 job이 될 수 있음 | 코드 추적 |
| O-18 | 낮음 | `predicate` 인라이닝으로 20단계 정의가 118MB DDL·RSS 1.3GB(`aip ddl`) | 재현됨(독립 검토). 제품 경로 `spike-v1`은 노드 4096 한도 |

### 3.3 열려 있는 항목(PoC 런타임, `poc-runtime` feature)

PoC 런타임은 개발용이며 127.0.0.1에만 바인딩한다. 제품 경로에는 해당하지 않는다.

| ID | 심각도 | 발견 |
|---|---|---|
| P-01 | 높음(일부 조치) | 계산 항목은 S-14로 조치. 관계 경로(`c.author.phone`)의 필드 정책은 같은 경로로 감싸지만 대상 행의 `visible to`는 아직 확인하지 못함 |
| P-02 | 중간(일부 조치) | where·sort·group은 S-14로 조치. `search` 색인에 정책 필드를 넣는 경우는 남음 |
| P-03 | 낮음(일부 조치) | S-14로 숨긴 값은 cursor에 NULL로 들어간다. cursor가 서명되지 않아 위조할 수 있다는 점은 남음 |
| P-04 | 중간 | 쿼리 `where`가 `apply_visibility=false`로 컴파일되어 관계 집합 `visible to`가 빠짐 |
| P-05 | 중간 | 읽기 트랜잭션에 `statement_timeout` 없음, 중첩 관계 배열에 LIMIT 없음 |
| P-06 | 중간 | 익명 rate limit·카운터 dedupe 키를 `x-aip-client`/`X-Forwarded-For` 맨 왼쪽 값(호출자 입력)으로 정함 |
| P-07 | 중간 | 객체 다운로드가 무인증이고 업로더가 Content-Type을 정함(저장형 XSS), CSV export에 formula injection 방어 없음 |
| P-08 | 낮음 | job·timer가 실행 시점에 actor를 재검증하지 않음 |
| P-09 | 낮음 | 항상 `NoTls`로 DB 연결, DB 오류 메시지를 그대로 반환, Text에 NUL 검사 없음, `--reset`이 확인 없이 `public` 스키마 삭제, inbound `hmac-sha256` webhook에 freshness 없음 |
| P-10 | 낮음 | 서버 마스터 secret이 SQL 바인드 파라미터로 PG에 전달됨(`log_min_duration_statement` 설정 시 로그에 남을 수 있음) |

PoC 항목은 제품 경로로 승격할 때 [PoC 이식 지도](2026-10-07-poc-porting-map.md)와 함께 다시 점검한다.

## 4. 점검했고 이상 없던 영역

- 입력: 요청 값은 모두 `$n::text::T` 바인딩, 식별자와 select 키는 facts의 이름만. `contains`는 `strpos`, `prefix`는 `left()` 비교라 LIKE 와일드카드 없음. 유니코드 숫자·음수·선행 0·i64 overflow Id 거부.
- NULL actor: `x = NULL actor`는 거짓, `fieldRead`는 `CASE ... END`로 fail-closed.
- 멱등성: (principal,key) PK, advisory lock, 본문이 다르면 `IDEMPOTENCY_MISMATCH`, `COMMIT_UNKNOWN`은 연결 폐기.
- JWT: RS256 고정, `typ`, `jku/x5u/crit` 거부, iss 정확 일치, aud, 수명 상한 900초, leeway 0, JWKS 크기·redirect·timeout.
- WebSocket: Origin 필수·허용 목록, 토큰은 URL이 아니라 첫 프레임, 프레임 16KiB.
- CORS: Origin 정확 일치, 경로별 메서드·헤더 제한, `Vary: Origin`. 인증은 Bearer뿐이라 CSRF 해당 없음.
- 객체·정적 파일: UUID key, owner SHA-256 namespace, `openat`+`O_NOFOLLOW`, `nosniff`+`attachment`, 컴포넌트 검증.
- 확장 worker: `env_clear`, 구현 이름 정규식·canonicalize·`starts_with`, stdout 1MiB 줄 상한, 출력 타입 검증, 요청마다 새 worker.
- TLS: `danger_accept*` 없음, reqwest `rustls-tls`. `unsafe`는 객체 파일 접근과 cgroup뿐.
- CI: workflow `permissions: contents: read`, 액션 SHA 고정, `pull_request_target` 없음.

## 5. 남은 위험 요약

1. [OPEN] O-01 Ref 직접 select의 대상 가시성: 창시자 결정이 필요한 가장 큰 인가 간극이다.
2. 제품 경로 가용성: O-04 인증 DB 조회, O-05 슬롯 공평성, O-06 JWKS 락.
3. 격리: O-13 macOS worker 격리 한계. 확장은 신뢰한 코드만 쓴다는 전제가 깨지면 영향이 크다.
4. PoC 런타임은 P-01~P-03을 고치기 전에 제품으로 승격하면 안 된다.

## 6. 검증 기록

각 조치의 검증 명령과 출력은 커밋별 보고에 남겼다. 최종 보고는 이 절에 갱신한다.
