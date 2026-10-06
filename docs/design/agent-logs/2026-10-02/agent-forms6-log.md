# forms6 log
## 진행
- [탐색] 구조 파악: Subscribe/Migration은 IR 폼으로만 존재, aip-pg/lib.rs에서 E602. Engine::query가 QueryPlan 실행을 담당. 계획: Subscribe를 pg에서 ir::Query로 바꿔 기존 query 계획 재사용(SubscriptionPlan{query, reads}), 런타임 Hub(LISTEN/NOTIFY + WS), Migration은 plan.migrations + _aip_migration.
- [A/B 컴파일 쪽 완료] IR: Subscribe.output(Shape), Subscribe::as_query(), Migration::digest(), CORE_IR 0.10. aip-plan: Program.subscriptions/migrations. aip-pg: plan/subscribe.rs(구독=내부 list query + SQL에서 읽는 테이블 도출, NOTIFY 트리거 DDL, migration은 pin cross). 계약 subscriptions 절 + TS subscribe. 레지스트리 코드 4개(MIGRATION_CHANGED/FAILED, SUBSCRIPTION_LIMIT/TOO_LARGE). saas에 LiveProjectTasks, backfill_done_notes 추가. tsc 4개 예제 통과.
- [A 런타임+e2e 작성] subscribe.rs(Hub/Session), migration.rs, subscribe_e2e 4개, migration_e2e 3개 통과. 다음: negative control, 골든 갱신, 문서
- [A negative control] (1) wake()가 reads 무시 -> a_change_wakes_only... 실패 / (2) 동일 결과 dedupe 제거 -> subscriptions_end_to_end 실패 / (3) 재조회 오류 시 구독 유지 -> 3개 실패 / (4) engine.run_subscription의 open_session 제거 -> impersonation 테스트 실패 / (5) auth 게이트 제거 -> connections_must_authenticate 실패. 전부 원복+touch 후 5 passed.
- [B negative control] advisory lock 제거 -> two_processes... 실패(second starter MIGRATION.FAILED), digest 비교 제거 -> migrations_run_once... 실패. 원복+touch 후 3 passed.
- [A control 6] NOTIFY 트리거 DDL 제거 -> subscribe_e2e 실패(스냅숏만 오고 갱신 없음). [B control 3] migration의 cross pin 제거 -> a_migration_reaches_every_tenant 실패(TENANT.MISMATCH). 원복+touch.

## Part A 세부와 근거 (subscribe)
- 전송: `GET /aip/subscribe` WebSocket, JSON 텍스트 프레임. 근거: 00-strategy wire는 HTTP/JSON + WebSocket(D-P13), 기존 `/aip/*` 체계. 메시지 `auth`, `subscribe{id,name,input}`, `unsubscribe{id}` / `ready`, `snapshot`, `changed`(전체 재전송), `error{id,code,...}`. diff 미채택(정확성 우선, 지시 사항).
- 인증: 연결 첫 메시지 `auth{token}`을 택함(헤더를 붙일 수 있는 클라이언트는 업그레이드 `Authorization` 헤더도 가능, `--dev-auth`는 `actor`). 근거: 브라우저 WebSocket은 헤더 불가, 쿼리 파라미터 토큰은 접속 로그와 프록시 로그에 남음. 5초 내 미인증과 인증 전 subscribe는 거부 후 종료. 토큰 만료 시 구독에 error 후 연결 종료(15초 주기 확인과 매 재조회 확인). 쿠키를 쓰지 않으므로 CSWSH 해당 없음. `auth`에 token이 없으면 익명(allow public만 열림).
- 변경 감지: 명세 08 E11 "하강: ... 전달 시점에 가시성 재평가"만 있고 NOTIFY는 지시서의 선택. 구독이 읽는 테이블마다 AFTER INSERT/UPDATE/DELETE/TRUNCATE 문장 트리거가 pg_notify('aip_changed', 테이블). 읽기 집합은 IR이 아니라 계획 SQL에서 도출(근거: 09-verification-log 신규 절). 런타임 연결 1개가 LISTEN, 구독 있는 프로그램에서만 시작, 끊기면 1초 뒤 재접속하고 전 구독 재조회. 100ms 디바운스.
- 재조회 보안: 매번 `Engine::run_subscription`(actor 존재, open_session 대리 세션 재확인, query 전체 실행 = allow, 가시성, 필드 가시성, 테넌트). 철회 시 error 후 그 구독 종료. 일시 오류(UNAVAILABLE, 직렬화 충돌)는 구독 유지하고 1초 뒤 재시도.
- 상한(기본값, `Limits`): 연결 1000, 연결당 구독 20, 전체 구독 5000, 동시 재조회 8, 결과 1000행, 메시지 64KB. 새 코드 AIP.SUBSCRIPTION.LIMIT(429), AIP.SUBSCRIPTION.TOO_LARGE(413).
- 계약/TS: 계약에 `subscriptions`(input, output, errors, guarantees)와 `transport.subscribe`(구독이 있을 때만, 없는 예제는 core_ir 줄 외 불변). TS는 `client.subscribe(name, input, onRows, onError?)`와 `Subscription.close()`. 재연결은 자동 안 함: 끊김은 onError(AIP.UNAVAILABLE)로 알리고 재구독하면 새 스냅숏(문서화됨).
- 프로토콜 버전: `aip-protocol/0.1` 유지. 근거: 계약 문서에 선택 키만 추가(이전에 jobs, outbound_webhooks를 더할 때도 올리지 않음), 구독 없는 프로그램의 계약은 불변, 기존 클라이언트 영향 없음. 메시지 프로토콜은 `ready.protocol = aip-subscribe/1`로 별도 버전.
- 구독 계획 재사용: `Subscribe::as_query()`(IR)로 내부 list query를 만들어 pg query 계획 그대로 사용. 정렬은 id 순(planner 기본). `from 파라미터`는 한 행짜리 목록으로 감쌈. IR: `Subscribe.output`, CORE_IR aip-core/0.10. 분석: 구독도 파라미터가 테넌트를 정하지 않으면 E314.
- 명세 언급 없음: 메시지 형식, 디바운스, 상한 값, 인증 방식은 전부 내가 정함(grammar.md 구독 프로토콜 절, D-P24에 기록).

## Part B 세부와 근거 (migration)
- 실행 위치: `aip_runtime::migrate`(스키마 DDL 뒤)에 붙임. `aip migrate`와 `aip run`이 이 함수를 공유하므로 둘 다에서 실행. 드리프트가 있으면 실행 안 함. 근거: 기존 구조(run이 migrate를 호출). 명세는 "배포당 1회"(08 P05)만 언급.
- `_aip_migration`은 DDL이 아니라 런타임이 `CREATE TABLE IF NOT EXISTS`(마이그레이션이 있을 때만): 모든 앱의 계획 골든이 바뀌는 것을 피하기 위함.
- 본문 digest: 본문 Stmt의 JSON sha256(`Migration::digest`). 서식과 주석은 불변(테스트). 이미 기록된 이름의 digest가 다르면 아무것도 실행하기 전에 AIP.MIGRATION.CHANGED. 실패는 롤백, 미기록, AIP.MIGRATION.FAILED(DB 메시지 포함), 뒤 migration 미실행. 새 코드 둘은 레지스트리에(startup 오류용 런타임 코드, http_status 500).
- 동시 기동: 세션 advisory lock(`pg_advisory_lock`) 하나를 잡고 전체를 처리, 끝에 unlock(풀 연결에 락이 남지 않게). 대기한 쪽은 기록을 보고 건너뜀.
- 테넌트: `cross tenant` 취급(핀 해제, E314 검사 없음). 근거: actor도 앵커도 없고 마이그레이션의 목적이 모든 워크스페이스의 행을 고치는 것이라, 기본 고정이면 두 번째 테넌트에서 항상 실패(negative control로 확인: TENANT.MISMATCH). 참조 트리거는 유지.
- 선언에서 지워진 migration의 기록은 오류 아님. `--reset`은 기록도 지움.

## negative control 결과
- subscribe: wake가 reads 무시 -> a_change_wakes_only... 실패 / 동일 결과 dedupe 제거 -> subscriptions_end_to_end 실패 / 재조회 오류에도 구독 유지 -> 3개 실패 / open_session 제거 -> impersonation 테스트 실패 / 인증 게이트 제거 -> connections_must_authenticate 실패 / NOTIFY 트리거 DDL 제거 -> 4개 실패.
- migration: advisory lock 제거 -> two_processes 실패(second starter AIP.MIGRATION.FAILED) / digest 비교 제거 -> migrations_run_once_in_order 실패 / cross pin 제거 -> a_migration_reaches_every_tenant 실패(AIP.TENANT.MISMATCH). 전부 원복 후 touch, 재실행 통과.
- 실서버 스모크: `aip run examples/saas/app.aip --dev-auth --reset`(migration 적용 로그 확인) + Node 전역 WebSocket으로 ready, snapshot [], changed [task] 수신. 종료 후 pgrep 0.

## 변경 파일
- IR/분석/레지스트리: crates/aip-ir/src/{forms,lib,analyze,codes}.rs, crates/aip-ir/tests/code_sites.snap
- 프런트엔드: crates/aip-sema/src/to_core.rs(subscribe_shape, rows_shape), crates/aip-sema/tests/core_ir.rs
- 플랜/백엔드: crates/aip-plan/src/lib.rs(Subscription, Migration, CHANGE_CHANNEL), crates/aip-pg/src/lib.rs, crates/aip-pg/src/plan.rs, crates/aip-pg/src/plan/subscribe.rs(신규)
- 런타임: crates/aip-runtime/src/{subscribe,migration}.rs(신규), engine.rs(actor_exists, run_subscription), http.rs(AppState::new, 라우트), lib.rs(migrate가 마이그레이션 실행, pub use Pool), auth.rs(verify_until), Cargo.toml(axum ws, futures-util)
- 계약/TS: crates/aip-contract/src/{lib,tsgen}.rs
- CLI: crates/aip-cli/src/main.rs(출력 문구), tests/{subscribe_e2e,migration_e2e}.rs(신규), tests/{cms_e2e,unavailable}.rs(AppState::new), Cargo.toml(dev: tokio-tungstenite, futures-util), tests/golden/*(계약, 클라이언트), crates/aip-pg/tests/golden/{saas,sema_ok_subscribe_migration}.plan.json
- 예제/적합성: examples/saas/app.aip, examples/*/client/aip.ts(4개 재생성), conformance/sema/{ok_subscribe_migration,tenant_subscribe_without_anchor}.{aip,expect}
- 문서: spec/grammar.md, spec/diagnostics.md(재생성), docs/DECISIONS.md(D-P24, D-P25), docs/design/09-verification-log.md
- 골든 diff 요약: ariari/shop/cms 실행 계획 골든 바이트 불변(diff -rq 확인, 변경은 saas.plan.json과 신규 1개). ariari/shop/cms 계약과 클라이언트 골든은 core_ir 두 줄만. saas는 subscriptions 절, transport.subscribe, TS subscribe 런타임 추가.

## 남은 TODO
- 클라이언트 자동 재연결과 백오프(문서화만).
- 변경 행으로 거르기, 같은 입력 구독 공유(OI-10, 범위 밖).
- 검색(`from X.match`)을 읽는 구독은 AIP-E601(계획 없음).
- 스키마를 이미 가진 DB에는 알림 트리거가 `--reset` 없이 생기지 않음(스키마 변경이 아직 --reset뿐).
- 확인 못함: FK cascade 삭제가 자식 테이블의 문장 트리거 NOTIFY를 내는지 e2e로 검증 안 함(구독이 읽는 테이블의 직접 변경만 e2e).
- 확인 못함: 수천 구독 규모의 부하와 재조회 비용 측정 안 함.
- 확인 못함: 듣는 연결 끊김 후 재접속 시 전 구독 재조회 동작은 e2e로 검증 안 함(코드 경로만 있음).

## 최종 명령 출력
- `cargo fmt --all && cargo clippy -q --workspace --all-targets` -> 출력 0줄(경고 0)
- `cargo test -q --workspace --no-fail-fast` -> passed=142 failed=0
- `./target/debug/aip check-docs docs spec README.md` -> 69 blocks: 62 parsed, 0 failed, 7 skipped (fragments with '...')
- `npx tsc --ignoreConfig --noEmit --strict --target es2022 --lib es2022,dom ../../examples/<app>/client/aip.ts` -> ariari cms saas shop 모두 tsc=0
- `pkill -f "aip run"` 후 `pgrep -f "aip run" | wc -l` -> 0
