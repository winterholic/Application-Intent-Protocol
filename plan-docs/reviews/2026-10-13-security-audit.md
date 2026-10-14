# 2026-10-13 보안 감사: 위협 모델과 발견 목록

> 상태: 진행 중인 감사의 살아 있는 기록이다. 소유자가 직접 요청한 자기 프로젝트의 방어적 점검이며, 수치와 상태는 기록 시점 값이다. 앞선 점검은 [2026-10-07 패턴 커버리지·보안 점검](2026-10-07-pattern-security-audit.md)이고 이 문서는 그 뒤를 잇는다. 설계 결정이 필요한 항목은 [OPEN]으로 표시하고 임의로 정하지 않았다.

2026-10-14 후속: 일반 수정은 선언한 patch 필드만 values에서 받으며 expect 조건·행 인가 뒤 transaction에서 실행한다. 기본키·모르는 필드·타입/범위·hidden/Ref expect를 거부한다. [일반 수정 계약](../../product/GENERAL-UPDATE.md), `tests/general_update.rs`, `product/tests/general-update.test.mjs`. 아래 표의 고정 to만 받는 설명은 감사 당시 기록이다.

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
| 속성 수준 인가·mass assignment(API3) | `/apply`는 `{apply,target}`만 받고 `to` 값은 계약에 고정. create/bundle/compose는 2026-10-13에 `/apply`로 연결됨(D-A26, 같은 멱등·계약 지문·커밋 시점 검사). O-01·O-02는 조치 완료 |
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
| S-25 | 낮음 | 제품 | 새로 추가된 OAuth client credentials 설정(`OAuthConfig`)이 `Debug`를 derive해 로그에 `client_secret`이 찍힐 수 있음 | 단위 테스트 | Debug를 직접 구현해 secret을 `<redacted>`로 | `aip-execution/tests/integrations.rs` | e145643 |
| S-26 | 낮음 | PoC | Text에 NUL이 들어가면 DB 오류가 `INTERNAL`로 나옴, CSV export 셀이 수식으로 실행됨, 읽기에 `statement_timeout` 없음, 제약 위반 메시지가 DB 원문 | 단위 테스트, e2e 전체 | NUL 거부, 수식 접두 셀에 `'`, 읽기 10초 timeout(`UNAVAILABLE`), 일반 문구 | `validate.rs`·`jobs.rs` 단위 테스트, `poc-runtime` e2e 전체 통과 | f7dccb3 |
| S-27 | 정보 | 제품 | `aip-execution`에 `hmac`·`base64`·`url`이 추가된 뒤 `spike-v3-write`·`spike-v4-worker`·`spike-v5-sdk`의 `Cargo.lock`이 갱신되지 않아 `verify.sh`가 spike-v3 단계에서 `--locked` 오류로 멈추고 CI의 v4 단계도 같은 오류를 낼 상태였음 | 실행(`bash tools/verify.sh`, exit 101) | lock 3개 오프라인 갱신(추가만), verify.sh에 v4·v6 포함 | `tools/verify.sh` 전체 | b7c2b7c |
| S-28 | 중간 | 제품 | O-19: trigger 대상 operation을 인증된 사용자가 `/jobs/enqueue`로 직접 넣어 HMAC 검증을 우회 | 실제 PG: 위조 body enqueue가 `ok:true`로 QUEUED | 사용자 경로에서 webhook·schedule·outbox·message 대상 operation이면 `ACCESS_DENIED`(`Triggers::targets_operation`). trigger 경로(`submit_as_service`)는 그대로 | `spike-v6-transport/tests/trigger_guard.rs`, `tests/triggers.rs` | 9feb20b |
| S-29 | 중간(가용성) | 제품 | O-25: `check`·정원 제약이 있는 resource마다 모든 `/apply`가 `xmin = pg_current_xact_id()` 전체 스캔(인덱스 불가) | 실제 PG: 관련 없는 `Ticket.close` 한 번이 `Allocation`을 seq scan 2번(`pg_stat_user_tables.seq_scan` 증가) | 트랜잭션 로컬 통계 `pg_stat_xact_all_tables`로 변경 없는 테이블을 먼저 거르고, 변경이 있어 보이면 기존 xmin 검사로 확정(결과 동일). `track_counts`가 꺼져 있으면 거르지 않음 | `spike-v3-write/tests/commit_check_scope.rs`(스캔 0회, 직접·effect 경유 `CHECK_FAILED`·`INVARIANT_VIOLATED` 동일) | 72c8557 |
| S-30 | 중간 | 제품 | O-20: 이벤트 key ledger(`aip_event_keys`)는 발행한 모든 이벤트 1행씩 영구 보존, `aip_outbox_dispatch`·`aip_outbox` 정리 없음, 만료 job 정리는 분당 100건(상한 없는 서비스 주체의 생성 속도보다 느림), job 입출력 바이트 quota 없음 | 테스트 4개가 수정 전 실패: ledger 1300행 유지, 만료 job 250건 중 150건 잔존, 900kB 입력 90개 전부 수락 | key는 재생 창 뒤 10000 cursor까지만 보존(100개마다 정리), 디스패치 30일 뒤 `aip_outbox_dispatch`와 해당 `aip_outbox` 행 삭제(미전달 행은 유지), 정리는 한 번에 1000건씩 최대 10배치, principal별 보존 payload 64MiB 상한(`QUOTA_EXCEEDED`, 재전송 key는 제외) | `aip-execution/tests/events.rs`, `spike-v6-transport/src/durable.rs`(collector 테스트), `tests/job_quota.rs` | b68fa7b |
| S-31 | 정보 | 제품 | `write_extension_lifetime`이 macOS에서 `owned workers report PIDs`로 실패(이전 감사의 "worker가 PID 파일을 쓰지 못함"). 원인: macOS 기본 worker 샌드박스(O-13, 63d3f9c)가 임시 디렉터리 쓰기와 spawn을 막는데 이 fixture는 PID 로그를 쓰고 busy loop를 돌림 | 실행 | fixture를 `listen_with_prototype_launch`로 샌드박스 없이 실행(다른 lifetime fixture와 같은 방식). 보안 약화 아님: 테스트 전용 opt-in | `spike-v6-transport/tests/write_extension_lifetime.rs` | 760472e |
| S-32 | 중간 | 제품 | O-05: 실시간·worker·업로드 슬롯이 서버 전역 semaphore라 인증된 한 principal이 전부 점유 가능, rate limit은 설정하지 않으면 꺼짐 | 실제 서버(전역 3슬롯): `hog` principal의 세 번째 WebSocket이 `authenticated`(수정 전) -> 다른 principal이 `SERVER_BUSY`가 될 상태 | principal별 상한(실시간 4·worker 2·업로드 2, `limits.per_principal_*`)을 전역 슬롯과 함께 획득·Drop 반환, RAG·object control 포함. `rate_limit` 생략 시 principal·경로 분류별 분당 600회 기본, `null`로 명시적 해제 | `spike-v6-transport/tests/principal_slots.rs`, `slots.rs` 단위 테스트, `aip-service` 설정 테스트 | 9e2a4dd |
| S-33 | 중간 | 제품 | O-22: 런타임과 migration이 같은 DB 역할이고 `serve`가 시작할 때 CREATE를 해서 서버 process가 사실상 schema 소유자(DROP·ALTER·트리거 해제 가능), 최소 권한 가이드 없음 | 실행: 권한 없는 역할에서 `CREATE SCHEMA IF NOT EXISTS`가 `permission denied for database`(이미 있어도), 수정 전에는 별도 URL 설정 불가(알 수 없는 키 거부) | `migrate_database_url_env`(init·migrate·adopt·provision용, 생략 시 기존 URL), `aip service provision`(인프라 테이블을 migration 역할로 생성), 서버는 권한 거부 + 테이블 존재면 진행·없으면 `provision` 안내와 함께 시작 거부, `product/DATABASE-ROLES.md` | `aip-cli/tests/least_privilege.rs`(문서의 권한만 가진 역할로 serve·read·apply·principal bind 동작, DROP·ALTER·CREATE·TRUNCATE·감사 UPDATE/DELETE 거부) | 84ce236 |
| S-34 | 중간 | 제품 | O-13: macOS 기본 worker 격리는 네트워크만 차단하고 `start_new_session` 손자 프로세스·파일 읽기·쓰기는 못 막음. `stop()`은 직계 자식만 정리. 별도로 Linux에서 `spike-v4-worker`가 컴파일되지 않았고(`cgroup.rs` 가시성), cgroup 메모리 상한이 스왑이 있는 호스트에서 OOM 대신 스왑으로 새며, `verify.sh`가 Linux에서 macOS 전용 격리 모드를 요구했음 | 테스트: 수정 전 macOS에서 같은 그룹 자식·`setsid` 손자·drop 경로·이중 fork 데몬 4종 모두 `descendant … survived the worker`. Linux는 CI 로그(E0365·E0603·E0624, `Unknown cgroup controller`, `memory-limited worker did not exit`, `MacNetDeny and MacSandbox require macOS`) | worker를 process group leader로 시작하고 stop·drop·읽기 실패·전송 기한 경로에서 부모 링크를 따라 후손 전체와 그룹을 SIGSTOP 뒤 SIGKILL(`reap.rs`). macOS `MacSandbox`(기본 `auto`): 네트워크·모든 파일 쓰기·`fork`/spawn 거부, `/Users`·`/Volumes`·임시 디렉터리 읽기 거부 뒤 extension·의존성·runtime 경로만 허용(Seatbelt는 더 구체적인 연산 `file-read-data`로 허용 규칙을 써야 deny를 덮음). Linux bwrap(PID namespace)·cgroup은 기존 구현을 유지하고 `memory.swap.max=0`을 추가, cgroup 모듈 가시성·systemd Delegate 목록·scope 안 하위 cgroup 이동 수정, CI에 teardown 테스트 추가. 한계: 이미 init에 입양된 이중 fork 데몬은 `Isolation::None`에서 찾을 수 없다(`#[ignore]` 테스트로 기록; MacSandbox는 fork 금지, bwrap은 PID namespace). macOS 프로파일은 deny 목록이라 `/etc` 등 시스템 경로 읽기는 열려 있다. 카운터 파일을 쓰는 fixture 3곳은 새 샌드박스가 막아 `Isolation::None`·`mac_net_deny`를 명시 | `spike-v4-worker/src/cleanup_tests.rs`(손자 3종 + bwrap 손자 + ignored 데몬), `isolation.rs`(macOS 샌드박스: 읽기·쓰기·fork·spawn·network, Node 실행, 경로 이스케이프) | 63d3f9c, ed70ed4, 1a06646, 43e3551, ac85e16, 9f2ae38 |
| S-35 | 낮음 | 제품 | O-04·O-06: 인증 조회가 서버 풀과 admission 밖의 사설 연결을 쓰고, JWKS 파일 모드는 토큰마다 읽고 파싱, HTTPS 모드는 서명 검증 전 알 수 없는 kid(발신자가 고름)가 만든 갱신 실패가 5초 동안 캐시된 키의 토큰까지 `Unavailable`로 만들고 갱신이 캐시 mutex 안에서 실행됨 | 단위 테스트 수정 전 실패: `a cached key was blocked by someone else's failed refresh`, `an IdP outage right after the TTL must not log everyone out`, `an unchanged file stamp must not trigger a re-read`, 풀 테스트 `lookup must use the shared pool left: None right: Some(Unavailable)` | 캐시된 키는 다른 사람의 갱신 실패와 무관하게 검증하고(IdP 장애 시 최대 5분 stale), 갱신은 single flight로 캐시 락 밖에서, 파일 모드는 크기·mtime·inode가 바뀔 때만 다시 읽음. `Authenticator::use_pool`로 서버 풀을 넘겨 요청별 principal 조회가 풀 크기·획득 기한·연결 재사용을 공유(durable 재검증은 job이 이미 풀 연결을 쥐므로 사설 연결 유지). 한계: admission(rate limit)은 principal을 알아야 하므로 인증 조회보다 뒤라 조회 flood를 principal별로 막지는 못하고 풀 상한과 획득 기한으로만 묶인다 | `aip-auth/src/idp_tests.rs`(mock IdP), `aip-auth/tests/jwt.rs` | 7cf0c56, 43e3551 |
| S-36 | 낮음 | 제품 | O-07: owner 해시에 폐기 기준(`min_iat`)이 들어가 revoke 뒤 같은 subject·actor를 다시 연결하면 기존 파일·job·event가 고아가 됨 | 단위 테스트(`owner_survives_revoke_and_rebind`: 해시 두 값이 다름) | owner 해시의 해당 자리를 고정 0으로. 폐기된 적 없는 principal의 기존 해시와 같은 값이라 저장된 데이터는 그대로 접근된다(그 테스트도 고정). 폐기 전 토큰은 `iat` 검사로 여전히 거부 | `spike-v6-transport/src/durable.rs` `owner_identity_tests` | a94f8c4 |
| S-37 | 낮음 | 제품 | O-08: rate limiter 행이 10만 개에 닿으면 신규 principal이 503. 1일 넘게 지난 만료 행만 회수했음 | 실제 PG 테스트 2개(만료 행 10만 개, 활성 행 10만 개) 모두 `rate scope quota exceeded` | 끝난 윈도는 상태가 없으므로 매 검사가 만료 행 128개를 회수하고, 모두 활성이면 가장 먼저 리셋되는 128개를 축출한 뒤 삽입 | `aip-execution/tests/rate_limit.rs` | dab8de4 |
| S-38 | 낮음 | 제품 | O-10: 응답을 쓰는 동안 DB 풀 연결(과 배포 fence의 shared 잠금)을 보유 — 느리게 읽는 클라이언트가 풀 슬롯과 migration을 쓰기 timeout 동안 붙잡음 | 실제 PG·한 연결 풀·수신 버퍼 4KiB 클라이언트: 다음 요청이 `DB_UNAVAILABLE` | 응답을 쓰기 전에 `release()`(JSON 응답과 managed file mount 모두) | `spike-v6-transport/tests/response_connection.rs` | 8b98ba6 |
| S-39 | 낮음 | 제품 | O-14: RAG corpus 용량이 owner 합산이라 `client_index`에서 한 principal이 corpus를 채우면 다른 사람의 색인이 막히고, 검색은 corpus 한도까지를 메모리에 올림 | 단위 테스트(`alice is at her share`) | `max_owner_chunks`(principal 하나의 chunk 수): 저장소가 owner의 다른 문서 chunk와 합쳐 검사(자기 문서 교체는 이전 chunk를 빼고 셈), 검색 scan 한도도 이 값. `client_index:true`면 모든 corpus에 명시하지 않으면 시작하지 않음(용량은 운영자 결정). `max_corpus_chunks`는 전체 백스톱으로 남아 `예상 사용자 수 × max_owner_chunks` 이하여야 몫이 보장된다 | `aip-execution/tests/rag_engine.rs`, `aip-service/src/rag.rs` | 3008dd0 |
| S-40 | 낮음 | PoC | O-16: aip-pg가 63바이트 초과 식별자를 검사하지 않아 PostgreSQL이 조용히 자르며 이름 충돌 | 테스트(`must be rejected: []`) | 생성한 DDL의 큰따옴표 식별자(문자열 리터럴 제외)가 63바이트를 넘으면 `POC.E603`. 코드 레지스트리·`spec/diagnostics.md`·emission site 스냅샷 갱신 | `aip-cli/tests/identifier_length.rs`, `aip-pg/src/lib.rs` 단위 테스트 | 959f6c9, db0ef89 |
| S-41 | 낮음 | PoC | O-17: evolve가 public 스키마에서 `%__%` 이름의 트리거·`%__%_fn` 함수를 프로그램이 모르면 모두 삭제해 다른 앱 객체를 지울 수 있음 | 실제 PG e2e: 다른 테이블의 `audit_log__stamp` 트리거가 삭제됨(count 0, 기대 1) | 정리 대상을 old·new 프로그램이 관리하는 테이블의 `<table>__` 네임스페이스로 한정 | `aip-cli/tests/evolve_e2e.rs` | e6577a3 |
| S-42 | 낮음 | 제품 | O-21: 실행 중 job의 heartbeat가 한 번 `DB_UNAVAILABLE`을 받으면 worker가 종료되고 job이 처음부터 재시작(PENDING http effect가 있으면 영구 `EFFECT_UNKNOWN`) | 실제 PG: 5초 job 도중 배포 잠금을 3초 독점 → 수정 전 attempt 2(기대 1) | 마지막 성공 heartbeat 후 12초까지는 `DB_UNAVAILABLE`을 견디고 계속 시도(lease 30초). lease 상실·배포 변경·principal 폐기는 즉시 중단 | `spike-v6-transport/tests/job_heartbeat.rs` | 9bb4bdf |
| S-43 | 낮음 | 제품 | O-23: `/ready`가 인증 없이 전역 gate로 직렬화되어 검사가 DB를 기다리는 2초 동안 뒤 요청이 줄을 서서 flood가 LB의 정상 probe를 막음 | `aip_migrate_meta`를 잠가 검사를 멈추면 두 번째 probe가 2.001초 대기 | 한가하면 매번 새 검사, gate가 바쁘면 직전 응답(최대 10초 전)을 즉시 반환. 응답이 없던 runtime만 기다림. 내부 listener로만 probe를 노출하는 방법을 운영 문서에 명시 | `spike-v6-transport/tests/health.rs` | 9ebd3c6 |
| S-44 | 낮음 | 제품 | O-24: migration journal `plan_digest UNIQUE`가 A→B→A→B의 두 번째 적용을 롤백시킴, Time↔Date 변환이 세션 TimeZone에 의존 | 실제 PG: `journal 기록 실패`, 20:00Z가 Seoul 세션에서 `2026-10-14`(기대 `2026-10-13`), Date→Time이 `2026-10-12T15:00:00`(기대 `…13T00:00:00`) | UNIQUE 제거(새 배포), 기존 배포는 drift 검사 뒤·새 구조 digest 기록 전에 apply가 제약을 삭제(구형 UNIQUE journal 단위 테스트), plan·apply 트랜잭션에 `SET LOCAL TimeZone TO UTC`. `requiresReview`는 재현되지 않음: apply는 항상 검토한 plan digest와 현재 DB·정의가 같을 때만 적용하므로 우회 경로가 없고, digest 복사를 "검토"로 볼지는 설계 결정이라 바꾸지 않음 | `aip-migrate/tests/migration_history.rs`, `aip-migrate/src/lib.rs` `old_journal_tests` | b8f8290 |
| S-45 | 낮음 | 제품 | O-28: 쓰기 판정의 `now`가 멱등·행 잠금 대기(최대 2초) 전에 고정되어 `allow due >= now`가 마감 뒤에도 통과 | 실제 PG: 700ms 뒤 마감인 행을 1.3초 잠그면 수정 전 `Applied { changed: [1] }` | `Knobs.fresh_now_after_lock`(HTTP apply·job 쓰기·쓰기 확장에서 켬): 행 잠금 후 DB 시계를 다시 읽고 250ms 넘게 지났으면 신선한 시각으로 재판정하고 update·effects에도 같은 시각 사용(고정 시계 호출자는 그대로). 미재현: 비-`maxRows` update effect 락 순서(교착이 계획 의존)와 advisory 키 32비트 공유는 코드를 바꾸지 않았다 | `spike-v3-write/tests/now_after_lock.rs` | b5d5fa5 |
| S-46 | 낮음 | 제품 | O-29: TS 클라이언트가 응답 크기·content-type을 검사하지 않고(프록시의 HTML 오류 페이지가 SyntaxError), 응답 유실 뒤 pending 쓰기를 버릴 방법이 없고, `WriteUnsettled` 뒤 키 없이 `apply`를 다시 부르면 새 키로 중복 쓰기 | Node 테스트 수정 전 4개 실패(SyntaxError, `discardPending is not a function`, 새 키) | JSON 호환 media type만 수용(누락·text/plain은 허용), content-length와 스트리밍 바이트 수를 `maxResponseBytes`(기본 32MiB)와 비교해 파싱 전에 `CLIENT.PROTOCOL_ERROR`, `discardPending(key)`, 같은 본문의 미확정 쓰기가 있으면 키 없는 `apply`가 그 키를 재사용. 미처리: 배포 artifact의 로컬 경로와 서명 없는 `SHA256SUMS` | `spikes/spike-v6-transport/client/hardening.test.ts` | 17f284c |
| S-47 | 낮음 | 제품 | O-30: `__proto__` 이름이 생성 descriptor·SDK 객체에서 own key가 아닌 prototype 설정으로 해석됨 | 정의 검사 테스트(`field: ()` — 거부되지 않음) | 정의 검사기가 정확히 `__proto__`인 enum·resource·operation·predicate·필드·transition·extension·매개변수 이름을 `DEF.RESERVED_NAME`으로 거부. `_proto`·`proto__` 등은 허용 | `spike-v1-fixture/tests/reserved_names.rs` | d984f81 |
| S-48 | 낮음 | 제품 | O-31: `*/2` 같은 일·요일 필드를 OR로 평가(Vixie는 `*`로 시작하면 AND), webhook 본문이 JSON escape 뒤 1MiB를 넘으면 영구 실패인데 503+`Retry-After` | 단위 테스트(`0 0 */2 * mon` 마지막 슬롯이 10-13, 기대 10-05), webhook 테스트 수정 전 `503 WIRE.DB_UNAVAILABLE` | 일·요일 필드가 `*`로 시작하면 둘 다 일치해야 함. job 입력 한도 초과를 `WIRE.PAYLOAD_TOO_LARGE`로 매핑해 webhook은 413(재시도 안내 없음). 미착수: job key의 deployment digest 때문에 배포 중 provider 재전송이 새 job이 될 수 있다는 점은 유일 키 `(owner,operation,contract_digest,deployment_digest,key)` 변경과 기존 배포 index migration이 필요한 설계 변경이라 바꾸지 않았다 | `spike-v6-transport/src/schedule.rs` 단위 테스트, `tests/webhook_oversize.rs` | 47ed6f6 |
| S-49 | 정보 | 제품 | O-15: `rustls-pemfile`(RUSTSEC-2025-0134, 유지보수 중단)을 `spike-v2-read`가 직접 사용, `rsa`(RUSTSEC-2023-0071)는 jsonwebtoken 경유로 수정 버전 없음, `cargo audit`·`cargo deny` 미설치 | `cargo audit` 설치 후 실행: 루트 workspace에서 `rsa` 1건, spike lock 8개와 prototype은 이상 없음 | `rustls-pemfile`을 `rustls-pki-types::pem`으로 대체(모든 lock에서 삭제만 발생, CA 번들 테스트 통과). `rsa`는 RS256 검증만 하고 개인키 연산이 없어 `deny.toml`에 근거와 함께 예외로 등록(재검토 조건 명시). `deny.toml`(yanked 거부, 미관리 workspace 의존성 거부, 알 수 없는 registry·git 거부, 라이선스 허용 목록), `tools/dependency-check.sh`(10개 manifest의 audit·deny + `npm audit`, 모두 통과), `.github/workflows/dependencies.yml`(의존성 변경 시와 매주 월요일). `npm audit` 0건, Python SDK는 표준 라이브러리만 사용 | `tools/dependency-check.sh` | 86ad333, 9b9b053 |
| S-50 | 정보 | 제품 | (운영): 실배포 미검증 항목: TLS reverse proxy, 다른 OS, 외부 IdP, 대형 backfill | (아래 6.2) | TLS proxy: 실제 Caddy 뒤에서 Origin·CORS·Authorization·`x-request-id`·1MiB 본문 한도·SSE 무버퍼 keepalive·WebSocket(Origin 필수·101·첫 프레임 인증) 16개 검사(`product/tools/proxy-check.mjs`, 서비스 결함은 발견되지 않음). IdP: mock HTTPS IdP로 discovery·JWKS 회전·kid 교체·위조 kid 검증(실제 Auth0·Keycloak·Entra ID는 **확인 못함: 계정 없음**, `product/IDP-RUNBOOK.md`). 대형 backfill: 합성 1200만 행에서 이전 방식은 20초 statement 한도에서 취소되며 읽기를 20.06초 막았고, 상수 backfill을 `ADD COLUMN … DEFAULT`로 바꿔 적용 0.81초·최대 읽기 대기 0.76초. 다른 OS: `verify.yml`이 ubuntu·PG 17 서비스 컨테이너에서 전체 검증을 이미 실행하고 있었고 Linux 컴파일 오류를 고쳐 녹색으로 만듦 | `product/tools/proxy-check.mjs`, `aip-auth/src/idp_tests.rs`, `aip-migrate/tests/online_backfill.rs`(+ `large_backfill.rs` 측정) | d898d2c, b331099, 002a620, e69d7b9 |

### 3.2 열려 있는 항목(제품 경로)

| ID | 심각도 | 발견 | 상태 |
|---|---|---|---|
| O-01 | 중간, **조치 완료**(2026-10-13) | **실제 PG로 재현**: `Task.team: Team?`, `Team rows read when owner = actor`, `expose read { select id, team }`에서 member 1이 읽으면 자기 팀이 아닌 task 2의 `team: 11`도 받음. 루트 select에 Ref 필드를 직접 열면 대상 행 정책과 무관하게 원시 FK Id가 나감. traverse·Ref filter는 안 보이는 대상을 null로 취급하므로 "숨은 대상 Id 확인" 방어가 select 경로에서는 무의미해짐 | 창시자 결정("c가 좋을듯 기본은 a로하고 선언따로하도록"): 기본은 대상 resource의 행 정책을 적용해 호출자가 대상 행을 볼 수 없으면 null, 정의 작성자가 `expose read` 안 `refIds team`으로 선언한 FK만 원시 id를 non-null로 공개. 구현: `sema.rs` `expose()`가 select 종류를 `refVisible`/`refVisibleWithPolicy`로 분류(선언하면 `field`), `plan.rs`의 루트·단일 traverse·1:N select 3곳이 `guard_ref`로 대상 `rowRead`를 적용, `spike-v5-sdk` 타입·descriptor 2곳이 nullable로 생성. 필수 참조(`team: Team`)도 기본 출력은 `Id<"Team"> \| null`, 선언하면 `Id<"Team">`. 계약 지문이 바뀐다. 테스트: `spike-v2-read/tests/ref_select_visibility.rs`(감사 시나리오를 PG로 재현, 수정 전 실패 확인), `spike-v5-sdk/tests/ref_select_codegen.rs` |
| O-02 | 낮음, **조치 완료**(2026-10-13) | `transition`의 `from`/`to`/`repeat unchanged`가 field read 정책 필드를 참조하면 `/apply` 응답의 `changed`/`unchanged`·`WIRE.INVALID_STATE`로 이전 값을 추론하거나 `to` 값 식으로 값을 보이는 필드에 복사할 수 있었음 | sema가 막지 않음을 `spike-v1-fixture/tests/policy_field_transition.rs`로 확인(수정 전 컴파일 통과). 이제 `from`, `to` 값 식(참조 이동·`exists` 안쪽 포함), `repeat unchanged`가 걸린 `to` 대상에서 field read 정책 필드를 쓰면 정의 오류 `DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`(D-A22). `allow`와 `repeat unchanged` 없는 `to` 쓰기는 가능하다. `check`·`invariant` 조건은 막지 않는다(O-27과 같은 이유로 존재 추론은 허용) |
| O-03 | 낮음, **조치 완료**(2026-10-14) | `/read`·`/apply`·`/status`가 `X-AIP-Contract` 헤더 없이도 통과했다. 권한 우회는 아니지만 경로별 계약 검증이 달랐다 | CD-13: 세 경로에서 헤더가 없으면 인증 뒤 업무 DB 접근 전에 `WIRE.CONTRACT_REQUIRED`, 지문이 다르면 `WIRE.CONTRACT_MISMATCH`로 거부한다. `/capabilities`에서 지문을 얻을 수 있다. 생성 TS 클라이언트와 Python SDK는 헤더를 보낸다. 테스트: `v13_headers.rs::contract_header_is_required_on_read_apply_and_status` |
| O-04 | 완료 | 인증 조회가 admission·풀 밖의 사설 연결 | S-35로 조치(서버 풀 공유). 한계: principal별 rate limit은 인증 뒤라 조회 flood는 풀 상한·획득 기한으로만 묶임 |
| O-05 | 완료 | 슬롯 전역 공유와 rate limit 기본 꺼짐 | S-32로 조치. 남은 것: durable job 실행 슬롯(`limits.jobs`)은 principal별로 나누지 않음(활성 job 100개 상한만), 기본 600/분은 임의 선택이라 앱 부하에 맞춘 조정 필요, 새 기본값은 시작 시 인프라 schema에 limiter 테이블을 만들어야 함(O-22와 겹침) |
| O-06 | 완료 | JWKS 파일 모드의 토큰마다 읽기, HTTPS 모드의 갱신 실패가 캐시된 키를 막음 | S-35로 조치(IdP 장애 시 알려진 키는 최대 5분 stale 허용) |
| O-07 | 완료 | revoke 뒤 재바인드하면 owner 해시가 달라져 기존 데이터 접근 불가 | S-36로 조치 |
| O-08 | 완료 | rate limiter scope 10만 행 한계에서 신규 principal 503 | S-37로 조치 |
| O-09 | 완료 | 읽기 확장 ctx 호출 횟수 상한 없음 | S-20으로 조치됨(64회 초과 `CALL_LIMIT`). 이 표의 "열림" 상태가 갱신되지 않았던 것 |
| O-10 | 완료 | 응답을 쓰는 동안 DB 풀 연결 보유 | S-38로 조치 |
| O-11 | 해당 없음 | `spike-v3-write`의 `internal(e)`가 DB 오류 원문을 보낸다는 지적 | 확인 결과 `internal`에 들어가는 것은 정의 컴파일 단계 문장뿐이고 DB 오류는 sqlstate만 응답. 조치 없음 |
| O-12 | 완료 | 헤더 이름 공백 수용 | S-16으로 조치 |
| O-13 | 완료(한계 있음) | macOS worker 격리 한계와 후손 프로세스 정리 | S-34로 조치. 한계: 이미 입양된 이중 fork 데몬은 `Isolation::None`에서 못 찾음, macOS 프로파일은 deny 목록, 신뢰한 확장 전제는 그대로. Linux CI: GitHub Actions ubuntu-24.04 Verify 실행 https://github.com/winterholic/Application-Intent-Protocol/actions/runs/38410877281 (bf3af85, 성공), Linux cgroup v2 https://github.com/winterholic/Application-Intent-Protocol/actions/runs/38410885856 (성공), 배포 점검(Caddy) https://github.com/winterholic/Application-Intent-Protocol/actions/runs/38410881671 (성공), Dependencies https://github.com/winterholic/Application-Intent-Protocol/actions/runs/38409128489 (성공) |
| O-14 | 완료 | RAG corpus 용량의 owner 합산과 검색 메모리 | S-39로 조치(`max_owner_chunks`, `client_index` 시 필수) |
| O-15 | 완료 | 의존성 점검(`rsa`, `rustls-pemfile`) | S-49로 조치(`rustls-pemfile` 제거, `rsa`는 근거 있는 예외, `cargo audit`·`cargo deny` CI) |
| O-16 | 완료 | 63바이트 초과 식별자 검증 없음(aip-pg) | S-40로 조치(`POC.E603`) |
| O-17 | 완료 | evolve가 다른 앱의 트리거·함수를 삭제할 수 있음 | S-41로 조치(`<table>__` 네임스페이스 한정). 한계: 관리 테이블 위의 `<table>__…` 이름은 여전히 생성기 소유로 본다 |
| O-19 | 완료 | trigger 대상 operation의 직접 enqueue로 HMAC 검증 우회 | S-28로 조치(trigger 대상 operation은 사용자 `/jobs/enqueue`에서 `ACCESS_DENIED`; 같은 operation을 사용자가 수동 실행해야 하면 trigger 없는 별도 operation을 둬야 함) |
| O-20 | 완료 | 저장소 증가(이벤트 key ledger, outbox 디스패치, 서비스 주체 job 정리 속도, job 바이트 quota) | S-30로 조치. 한계: 이벤트 key는 재생 창 뒤 10000 cursor를 넘으면 중복 방지가 사라짐(늦은 재시도는 새 이벤트), 수신자 없는 topic의 `aip_outbox` 행과 `aip_event_streams` 행은 정리하지 않음, 출력 바이트는 제출 시점에만 합산되어 실행 중 job 최대 100개 × 1MiB만큼 넘을 수 있음 |
| O-21 | 완료 | 실행 중 job의 heartbeat가 일시 DB 오류에 worker를 종료 | S-42로 조치(12초 허용) |
| O-22 | 완료 | 런타임·migration 같은 DB 역할, 최소 권한 가이드 없음 | S-33로 조치. 한계: 기본값은 여전히 단일 역할(설정을 바꾸지 않으면 서버가 소유자), runtime 역할에도 `aip_migrate_meta` 같은 관리 테이블의 DML이 열려 있어 침해 시 배포 metadata는 바꿀 수 있음(컬럼 권한 구성은 미검증), `GRANT ... ALL TABLES`는 테이블이 늘 때마다 다시 필요 |
| O-23 | 완료 | /ready 전역 gate에 probe가 줄을 섬 | S-43로 조치(바쁘면 직전 응답) |
| O-24 | 완료(일부) | journal 순환, `requiresReview`, Time↔Date 변환 | S-44로 조치(순환·UTC). `requiresReview` 강제는 재현되지 않아 변경 없음 |
| O-25 | 완료 | `check`·정원 제약 검사가 관련 없는 `/apply`마다 전체 스캔 | S-29으로 조치. 변경된 resource에는 여전히 xmin 스캔이 돈다(그 테이블이 클 때의 비용은 남음) |
| O-26 | 중간, **조치 완료**(2026-10-13, 쓰기 확장·job 경로 제외) | 행의 부재에 의존하는 정책(`allow not exists Booking b where b.room = r.room and b.status = CONFIRMED`)은 서로 다른 행을 동시에 바꾸는 요청을 막지 못함(이중 예약) | **실제 PG 재현**: 수정 전 두 요청 모두 성공(`["OK","OK"]`, 확정 2건; HTTP 6개 방 동시 요청에서도 같음). D-A23: 다른 행의 부재에 의존하는(`not exists`) 전이·create는 SERIALIZABLE로 실행하고(양의 `exists`·참조 경로는 `FOR SHARE`가 이미 보호하므로 표시하지 않음) 40001·40P01을 최대 3회 내부 재시도. 수정 뒤 방마다 정확히 하나 성공하고 진 쪽은 `WIRE.FORBIDDEN`(재시도 가능한 `WIRE.CONFLICT`가 아님). 테스트: `spike-v3-write/tests/absence_serializable.rs`, `spike-v6-transport/tests/absence_serializable_http.rs`, `spike-v1-fixture/tests/serializable_marker.rs`. 개발 조언 `DEF.ADVISORY_ABSENCE_POLICY`. **남은 것**: 쓰기 확장·job이 `apply_in`으로 부르는 전이는 확장 트랜잭션의 격리(READ COMMITTED)를 그대로 쓴다(재시도가 확장 부작용과 맞지 않아 미착수) |
| O-27 | 낮음, **조치 완료**(2026-10-13) | 불변식 위반 코드(`WIRE.ALREADY_EXISTS`, `WIRE.INVARIANT_VIOLATED`)로 `rows read`에 가려진 행의 존재·정원 상태를 추론할 수 있음 | **결정(D-A22): 허용한다.** 유일성·정원 위반은 본질적으로 충돌 행의 존재를 알린다. 대신 오류 본문·메시지에 호출자가 볼 수 없는 행의 id·필드 값이 실리지 않음을 `spike-v3-write/tests/invariant_error_privacy.rs`로 고정했다(DB 오류의 `DETAIL`을 메시지에 넣는 변형이 이 테스트에서 실패함을 확인). 숨은 대상 행은 없는 행과 같은 `WIRE.MISSING_TARGET`이다. `spec/definition-syntax.md`에 명시 |
| O-28 | 완료(일부) | 쓰기 판정의 `now`가 잠금 대기 전에 고정 | S-45로 조치(`now`). 락 순서·32비트 advisory 키는 재현하지 못해 변경 없음 |
| O-29 | 완료(일부) | TS 클라이언트 응답 검사·pending 폐기·키 중복 | S-46로 조치. 남음: 배포 artifact의 로컬 경로와 서명 없는 `SHA256SUMS` |
| O-30 | 완료 | `__proto__` 이름 | S-47로 조치(정의 단계 거부) |
| O-31 | 완료(일부) | cron 일·요일, webhook 1MiB 초과 | S-48로 조치. 남음: job key의 deployment digest(유일 키 변경 설계 필요) |
| O-32 | 낮음 | 오늘 추가된 업무 감사 테이블(`aip_audit`)이 "append-only"라고 하지만 `BEFORE UPDATE` 트리거만 있다. `DELETE`·`TRUNCATE`는 막지 않아 런타임 역할(O-22: owner 권한)이 행을 지울 수 있고, 트리거는 owner가 끌 수 있음 | 코드 추적(daf3e63). 삭제를 막으려면 `BEFORE DELETE`·`TRUNCATE` 트리거나 별도 역할이 필요하고, 보존 기간 정책과 함께 결정해야 함 |
| O-33 | 낮음 | 감사 행은 전이의 to-필드 이전·이후 값을 영구 보존한다. 개인정보 필드가 전이 대상이면 삭제 요청(erase)과 충돌하고, bulk 요청마다 대상 행 수만큼 행이 쌓여(최대 `bulk maxRows` 10000) 인증된 사용자가 저장소를 늘릴 수 있음(O-20과 같은 유형). field read 정책 필드의 값도 기록됨(운영자 전용 테이블이라 API로는 안 나감) | 코드 추적 |
| O-34 | 정보 | 오늘 추가된 요청 추적(`trace.rs`)과 `/metrics`: 로그 줄은 route 이름·상태·오류 코드·시간만 담고 토큰·주체·본문·쿼리는 담지 않음. 인바운드 `x-request-id`는 8~64자 안전 문자일 때만 수용. `/metrics`는 토큰이 설정된 때만 열리고 상수 시간 비교. label은 고정 집합이라 카디널리티가 호출자 입력으로 늘지 않음 | 코드 읽기, 문제 없음 확인 |
| O-35 | 낮음 | 직접 노출된 listener에 응답 없는 연결 90개를 열어 두면 정상 호출이 요청 제한 시간(5초) 동안 응답을 받지 못함(연결 상한 64, 실측 5.0초 뒤 회복). 계속 다시 열면 지속됨. 서버는 loopback listener라 TLS reverse proxy가 헤더를 끝까지 받은 요청만 넘기는 배치에서는 도달하지 않음 | 실행(`aip-cli/tests/http_blackbox.rs`). 이미 [인프라 문서](../../product/INFRASTRUCTURE.md)가 연결·요청 크기 제한은 proxy가 맡는다고 적음 |
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

1. [조치 완료 2026-10-13] O-01 Ref 직접 select의 대상 가시성(실제 PG로 재현됨): 가장 큰 인가 간극이었고 창시자 결정(기본은 대상 행 정책 적용, 선언 시 원시 id 공개)대로 구현했다. 아래는 결정 전 기록이다. 고치려면 Ref select를 정책 필드처럼 취급해 대상이 안 보이면 null로 만들어야 하고, 이는 출력 타입 nullable과 계약 지문을 바꾼다. 구현 위치는 `sema.rs`의 `expose()` kind 분류, `plan.rs`의 select 식 3곳(루트·단일 traverse·1:N), `spike-v5-sdk`의 타입 생성 2곳이다.
2. 제품 경로 가용성: O-04·O-05·O-06은 조치했다. O-20 저장소 증가와 O-25 쓰기마다 `check` 전체 스캔도 조치 상태다(해당 행의 한계 참고).
3. 권한 설계: O-19 trigger 대상 operation의 직접 호출, O-22 DB 역할 분리 없음, O-26 `not exists` 경합.
3. 격리: O-13은 조치했다(후손 프로세스 정리, macOS 파일·fork 제한). 신뢰한 확장만 쓴다는 전제와 `Isolation::None`의 이중 fork 데몬 한계는 남는다.
4. PoC 런타임은 P-01~P-03을 고치기 전에 제품으로 승격하면 안 된다.

## 6. 검증 기록

- 모든 조치는 먼저 실패하는 테스트로 재현했고, 수정 전 코드로 되돌려 같은 테스트가 실패함을 확인한 뒤 수정했다. 재현하지 못한 조치는 표에 "없음(미실행: 사유)"로 적었다(S-08, S-15).
- 검증은 다른 작업자의 미완성 변경이 섞이지 않도록 `origin/main`의 격리 worktree에서 실행했다. 최종 실행은 이 문서를 마지막으로 갱신한 시점의 `origin/main`이다: `bash tools/verify.sh`(종료 코드 0, 마지막 줄 `AIP core verification passed`), `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml`, `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml`, `cargo test -p aip-cli --features poc-runtime`.
- `tools/verify.sh`는 spike-v4·v6 테스트를 돌리지 않는다. 이 때문에 S-07 변경이 v6 lib 테스트 빌드를 깨뜨린 것을 verify로는 잡지 못했다(S-24). GitHub CI `verify.yml`이 v6 테스트를 돌린다.
- 읽기 전용 독립 검토 7개 영역과, 실행형 공격 시도 3개(읽기 인가, 쓰기 동시성, HTTP black-box)를 계획했다. 첫 회차는 계정 사용 한도(04:30 초기화)로 쓰기 정책 의미 행렬(`adv_policy_semantics`)만 끝났고, 나머지 셋은 후속 회차에서 실행했다(아래 6.1).

### 6.1 실행형 공격 검증 결과(후속 회차)

**새 발견: 없음.** 조치가 필요한 취약점은 만들어지지 않았다. 관찰 하나(O-35, 직접 노출 시 연결 점유)와 의도 확인 두 가지(아래 "관찰")만 기록한다. 세 검증 모두 회귀 테스트로 남겼고 `bash tools/verify.sh`가 돌린다.

| 영역 | 시도한 것 | 방법 | 결과 | 테스트 |
|---|---|---|---|---|
| 읽기 인가 | 숨은 행의 값·존재, 정책 숨김 필드, 숨은 문서만 가진 그룹의 집계·1:N traverse, 숨은 대상을 가리키는 Ref를 eq·gt·lt·gte·lte·in·contains·prefix 프로브, 정렬 asc/desc×limit, offset 0~10, keyset cursor, id 직접 조회, 숨은 필드 select·filter·sort, 정책 숨김 필드의 sum 집계 정의, 없는 resource·필드 이름. 정의는 숨은 필드 외 모두 filter·sort·offset·cursor 개방 | 비간섭 오라클: 공격자가 읽을 수 없는 데이터만 다른 두 DB에서 응답이 요청 92개 모두 같아야 함 | 이상 없음(멤버 2와 익명 모두 동일, 응답 74개·거부 18개). 대조: 소유자는 20개 이상 요청에서 차이를 봄, 일부러 `rows read when true`로 누출을 심으면 같은 테스트가 실패함을 확인 | `spike-v2-read/tests/adv_read_noninterference.rs` |
| 쓰기 동시성 | 같은 행을 24명이 동시에 claim, 한 소유자의 카운터 60회 동시 증가, 같은 팀의 12개 행이 정원 3의 남은 2칸을 경쟁, 같은 행 3개를 반대 순서로 겹치는 bulk 16개, 전이와 그 전제를 없애는 전이(release)의 경쟁. 각각 3회 반복(연결마다 별도 세션, 동시 출발) | 실제 PG, 최종 상태와 응답의 일치 검사 | 이상 없음: claim 승자는 정확히 1명이고 행이 그 승자를 가리킴, 증가 누락·중복 없음(응답 OK 수 = 저장된 값), 정원 초과 없음(남은 칸 모두 사용), bulk는 한 호출자에게 전부 귀속, 교착·멈춤 없음, 거부 코드는 `INVALID_STATE`·`INVARIANT_VIOLATED`뿐 | `spike-v3-write/tests/adv_write_concurrency.rs` |
| HTTP black-box | 위조 토큰 23종(alg none 2, HS256 공개키 혼동, 서명 비트 반전, payload 교체, 다른 RSA 키, 모르는 kid, kid 경로 순회, jku, typ, 만료, nbf 미래, 수명 초과, aud·iss 불일치, sub 없음, 미연결 subject, 형식 오류 4, 100kB 토큰, NUL), Authorization 헤더 8종(없음, Basic, 스킴 없음, 중복 2, 쿼리·쿠키·X-Forwarded), 프레임·라우팅 35종(CL.TE, 이중 Content-Length, 형식 이상 Content-Length 5, chunked, obs-fold, 개행만 LF, 콜론 없음, NUL, 절대 URI, `//`·`%72ead`·`/read/`·`/x/../read`·대문자, 경로 순회, GET·PUT·TRACE·메서드 재정의, HTTP/1.0·/2 preface·/0.9, Expect, 70kB 줄, 헤더 500·30000·300000개, 2MiB 본문), 본문 14종(100000단계 중첩, 중복 키, 배열, 빈 본문, 잘못된 UTF-8, BOM, 꼬리 쓰레기, 거대·음수 limit, 키릴 유사 이름, SQL 문자열, select 100000개, `__proto__`), CORS 6개 출처×(요청, preflight), 응답 없는 연결 90개 | 실제 `aip service serve` 바이너리와 JWKS 검증 토큰, 소켓 바이트만 사용 | 이상 없음: 잘못된 토큰은 모두 `UNAUTHENTICATED`·`TOKEN_EXPIRED`·`BAD_REQUEST`로 거부(익명으로 격하되어 서빙되지 않음), 숨은 두 번째 요청으로 쓰기가 일어나지 않음(데이터 불변), 오류 본문에 DB 문자열·SQL·경로·스택·토큰·schema 이름 없음, 신뢰하지 않는 Origin은 어떤 변형도 반영되지 않음, 모든 공격 뒤 정상 호출이 계속 응답 | `aip-cli/tests/http_blackbox.rs` |

관찰(결함 아님):

- 토큰 없는 `/read`는 익명 actor로 처리되어 200과 정책이 허용한 행(샘플에서는 없음)을 돌려준다. 익명 접근 자체가 설계이고(S-17의 NULL actor 항목과 같은 모델), 유효하지 않은 토큰이 붙은 요청은 익명으로 격하되지 않고 거부된다.
- `aud`가 배열이고 이 API를 포함하면 수락한다(RFC 7519에서 유효). "정확한 API audience"를 단일 값으로 강제할지는 IdP 정책이다.
- 헤더 값 끝의 공백은 같은 토큰으로 본다(RFC 9110의 선택적 공백).
- 오류는 HTTP 200과 `{"ok":false,"code"}`로 온다(DD-14, F4·F13 참고).

미실행·한계: 공격은 로컬 PG 17과 macOS에서만 실행했다. Linux worker 격리(bwrap)를 거치는 공격과 TLS reverse proxy 뒤의 공격(Caddy 경유 검증은 별도 테스트가 있음)은 이번에 하지 않았다. 쓰기 동시성은 READ COMMITTED 기본 경로이고 `not exists` 의존 정책(SERIALIZABLE 경로)과 O-26은 건드리지 않았다. 두 번째 PG 인스턴스·다중 서버 인스턴스 경쟁은 미실행이다.

- 의존성: OSV 조회로 crates.io 438개 중 취약점 2건(O-15), `npm audit` 0건. `cargo audit`·`cargo deny`는 설치되어 있지 않다.

### 6.2 운영 문제 점검 회차(2026-10-13, 후속)

macOS(로컬 PostgreSQL 17)와 GitHub Actions ubuntu-24.04(PostgreSQL 17 서비스 컨테이너)에서 실행했다. 항목마다 수정 전 실패를 먼저 확인했고 각 행의 "재현"에 적었다.

- 낮은 심각도 O-항목은 위 표의 S-34 이후 행과 같다. 실패 출력은 각 행에 있다. 재현할 수 없던 것은 "확인 못함"으로 남겼다(O-24의 `requiresReview` 강제, O-28의 락 순서·advisory 키).
- 의존성: `cargo audit`(루트 workspace는 `rsa` 1건만, 나머지 lock 9개는 이상 없음), `cargo deny check`(10개 manifest 모두 통과), `npm audit`(0건), Python SDK는 표준 라이브러리만 사용.
- Linux: GitHub Actions ubuntu-24.04 Verify 실행 https://github.com/winterholic/Application-Intent-Protocol/actions/runs/38410877281 (bf3af85, 성공), Linux cgroup v2 https://github.com/winterholic/Application-Intent-Protocol/actions/runs/38410885856 (성공), 배포 점검(Caddy) https://github.com/winterholic/Application-Intent-Protocol/actions/runs/38410881671 (성공), Dependencies https://github.com/winterholic/Application-Intent-Protocol/actions/runs/38409128489 (성공)
- 실행하지 못한 것: 실제 외부 IdP(**확인 못함: 계정 없음**), nginx(미설치), 두 번째 PG 인스턴스·다중 서버 인스턴스, 다른 OS(macOS·Linux 외).
- 관찰한 불안정 테스트: main의 Verify 한 번이 `admission::tests::simultaneous_connections_share_one_principal_route_quota`(`Unavailable`)로 실패했고 같은 커밋의 재실행은 통과했다. 12개 연결이 같은 새 scope를 동시에 만들 때 `lock_timeout='100ms'`에 걸리는 타이밍 문제로 보이며, 로컬에서 6회 연속 통과했다. 원인을 확정하지 못했고(`확인 필요`) 이 회차에서 고치지 않았다. 같은 기간 Object storage·External brokers workflow는 각각 MinIO 컨테이너 시작 실패(exit 125)와 `rdkafka-sys` 빌드의 `curl/curl.h` 누락으로 실패했으며 이 회차의 변경과 무관하다.
