# forms5 log

## 0. 시작 관찰
- 골든 기준선 사본: scratchpad/base5/{pg,cli}.
- 로컬 PG 17.11, pg_trgm/unaccent 확장 사용 가능(pg_available_extensions 확인).
- Search 형식은 sema(check.rs)가 선언 자체(엔티티/필드)를 검사하지 않고, `r.rank` 는 엔티티 필드가 아니라 E103. QuerySource::Call 은 현재 aip-pg 가 "search queries are not available yet" 로 거부.
- 설계 문서 근거(검색): 08-coverage-catalog.md:119 `E05 | 전문 검색 | search 형식 (postgres tsvector 또는 search ext)`, :120 `E06 | 한국어 형태소 검색 | search ext capability | ⚠ (엔진 선택 미결)`, :449-455 예제(`select { id title rank: r.rank }`), 09-verification-log.md:99 OI-08.
- 구현: IR(QuerySource::Search, Node::SearchRank, I122/E316/W603, 0.8), sema(check/to_core), aip-pg(schema.searches, ddl, list_sql), contract(guarantees.search), 예제 saas(search 2개), e2e search_e2e.rs 2개 통과(1차). 변이 스크립트: scratchpad/negctl5.py + mut_b.json
- Part C 구현(1차): IR Node::SigningSecret + E317/E208 규칙(0.9), sema check/to_core, aip-plan Outbound, aip-pg plan/outbound.rs(이벤트별 핸들러가 _aip_outbound_delivery 적재), ddl(_aip_outbound_delivery/_aip_outbound_endpoint + url CHECK), aip-runtime outbound.rs(reqwest+rustls, SSRF 검사, 서명, 재시도, 비활성), Engine.secret/outbound 설정, CLI --allow-private-webhook-targets, 예제 saas(Endpoint/AddEndpoint/outbound), e2e outbound_e2e.rs 5개 통과. 변이 10건 모두 e2e 실패(mut_c.json). Part B 변이 6건 모두 실패(mut_b.json).

## Part B. search 실행: 내가 정한 세부와 근거
- 근거 인용: 08-coverage-catalog.md:119 `E05 | 전문 검색 | search 형식 (postgres tsvector 또는 search ext)`, :120 `E06 | 한국어 형태소 검색 | search ext capability | ⚠ (엔진 선택 미결)`, :449-455 `search RecruitmentSearch on Recruitment fields [title weight A, body weight B] language korean` / `from RecruitmentSearch.match(q) r` / `select { id title rank: r.rank }`, 09-verification-log.md:99 `OI-08 한국어 형태소 검색 엔진 선택`. 가중치 기본값, 일치도 표현, 빈 질의, 근사 방식, 정렬 동률은 "명세 언급 없음".
- 색인: 엔티티 테이블에 `__search_<이름>` tsvector 생성 컬럼(STORED) + GIN. 트리거가 아니라 생성 컬럼인 이유: raw SQL을 포함해 행이 바뀌는 모든 경로에서 색인이 뒤처질 수 없다. 가중치 생략은 D(PG 기본). history 엔티티는 버전 스냅숏(to_jsonb)과 versioned 변경 판정에서 이 컬럼을 뺐다(임시 DB로 확인: history data에 컬럼 없음, 편집당 version 1 증가).
- 질의: `websearch_to_tsquery`(단어 AND, or, "구절", -제외). 정렬 `rank desc, id`(id 방향은 기존 keyset 규칙대로 마지막 키 방향을 따라 desc). 명시한 `sort by`가 있으면 그것 + id.
- 일치도(rank) 표현: 엔티티 필드로 만들면 엔티티 스키마가 검색마다 달라지고 `rank` 필드를 가진 엔티티(08:358 `rank: Text position`)와 충돌한다. 그래서 IR에 `QuerySource::Search{search, query, alias}`와 `Node::SearchRank{alias}`(타입 Decimal)를 추가했다(SQL 개념 없음: 일치도는 "클수록 좋은 Decimal"). `r.rank`는 엔티티에 `rank` 필드가 있어도 일치도로 읽힌다(문법에 명시). `ts_rank`(float4)를 `round(..::numeric, 6)`로 반올림해 Decimal로 낸다: Decimal은 문자열로 전달되고(예: "0.607927"), 커서 비교가 정확하다. 검증은 validate I122(미존재 검색, 별칭 밖의 rank), analyze E316/E314, sema E103/E104/E201. CORE_IR_VERSION 0.7 -> 0.8, 단언 테스트 ir_version_is_0_8 (이후 Part C로 0.9, ir_version_is_0_9).
- keyset: 커서는 (일치도, id). 커서가 JSON 숫자로 오가며 후행 0이 사라지므로 일치도 동률 비교만 numeric 비교로 했다(기존 키 타입은 text 비교 그대로: ariari/shop 골든 불변). 일치도는 행 자신과 질의에만 의존해(ts_rank는 컬렉션 통계를 쓰지 않음) 행이 추가돼도 기존 행의 키가 안 바뀐다. e2e: 25행 동률 + 첫 페이지 뒤 4행 추가(동률 3행, 더 높은 일치도 1행) -> 중복 0, 누락 0.
- 빈 질의: 파라미터 타입 `Text(1..100, trim)`이 ""와 공백을 INPUT.INVALID로 거부(검증 오류). 구두점뿐("!!!")이거나 불용어뿐("the")은 tsquery가 비어 빈 결과(오류 아님, has_more false). 근거: 사용자가 입력은 했고 검색어로서 의미가 없을 뿐이다. 모든 행을 돌려주는 쪽이 위험(데이터 노출).
- 가시성/테넌트/총계: list_sql 공용 경로라 visible to, 소프트 삭제, 앵커 테넌트 필터가 그대로 걸린다. 쿼리 페이지 응답에는 총계가 없고 `has_more`/`next_cursor`만 있다. 모르는 사람의 검색은 `has_more=false`로 e2e 확인. 테넌트 범위 엔티티의 검색은 앵커 파라미터가 없으면 E314. `publishable` 엔티티의 검색은 E316으로 거부(읽는 쪽이 게시본 테이블이고 색인은 작업본에 있음).
- 한국어: `simple` 설정 + 질의를 `websearch_to_tsquery('simple', q)`로 만든 뒤 각 단어를 접두 일치(`'단어':*`)로 바꾼다. pg_trgm은 로컬에 있으나(확인) 쓰지 않았다: 확장 의존이 생기고 가중치와 순위는 tsvector에서만 되기 때문. DB 로케일 영향 확인: en_US.UTF-8과 C 로케일 둘 다 한글이 단어로 분리됨. 한계(e2e로 고정): "개발"로 "자바개발자"를 못 찾음(합성어), 질의 단어 AND, 어간 변화 못 찾음. 경고 AIP-W603(레지스트리 title/explain/fix), 계약 `queries.<Q>.guarantees.search`에 {language, query_syntax, order, match, approximate} 노출. 영어 등은 PG 어간 처리.
- 언어 목록: english, korean, german, french, spanish, italian, portuguese, dutch, russian, swedish, norwegian, danish, finnish, hungarian, romanian, turkish. 그 밖은 E316.

## Part C. outbound webhooks 실행: 내가 정한 세부와 근거
- 근거 인용: 08-coverage-catalog.md:179 `H04 | 웹훅 발신(고객 구독, 서명, 재시도) | outbound webhooks 형식`, :546-550 `outbound webhooks for Workspace w { events [OrderCreated, OrderCancelled] where event.order.workspace = w  sign hmac_sha256  retry 10 over 24h  disable after 3d failing }`. 지시문의 `on [events]`와 달리 정본(spec/grammar.md)과 파서는 `events [..]`이므로 정본을 따랐다. 헤더 형식, 비밀 보관, 재시도 곡선, 엔드포인트 비활성 의미, SSRF 규칙, 순서는 "명세 언급 없음".
- 엔드포인트와 URL: E의 행이 엔드포인트, `url: Url`(또는 Text) 필드가 있어야 한다(없으면 E317). 등록 시 scheme을 거르는 DB CHECK(`<table>__url_scheme`, INPUT.INVALID + WEBHOOK_URL_INVALID)와 런타임 Url 검증이 http/https 아닌 값을 막는다. 사설 주소 검사는 전달 직전(DNS 포함)에 한다: 등록 시점에만 검사하면 DNS가 나중에 바뀌어 우회된다(리바인딩). 등록 시점 DNS 검사는 안 한다.
- 큐잉: 이벤트별 Handler(`outbound_<엔티티>_<이벤트>`)로 하강해 디스패처 트랜잭션 안에서 `_aip_outbound_delivery`에 INSERT ... SELECT(엔드포인트 행 중 where 만족, url 있음, 비활성 아님, 같은 테넌트). `_aip_processed`가 핸들러 재실행을 막고 `UNIQUE(form, endpoint, event_id)`가 이중 방어. 순서 보장 없음(엔드포인트별로도), 최소 한 번.
- 테넌트: E가 테넌트 범위면 이벤트의 앵커(첫 테넌트 범위 참조)가 가리키는 테넌트의 엔드포인트만 받는다. where가 테넌트를 빠뜨려도 막힌다(e2e: where 없는 폼에서 B의 엔드포인트는 안 받음). 이벤트에 앵커가 없으면 E314.
- 요청: POST JSON `{"id":"evt_<outbox id>","type","created_at","data"}`. 헤더 `AIP-Signature: t=<unix>,v1=<hex HMAC-SHA256("<t>.<본문>")>`(inbound stripe 방식과 대칭, 시도마다 t 갱신 -> 5분 허용창으로 재전송 방지), `AIP-Event-Id`(모든 시도와 엔드포인트에서 같음, dedupe), `AIP-Event`, `AIP-Delivery-Id`, `AIP-Delivery-Attempt`. 수신자용 `verify_signature`를 공개 함수로 제공(단위 테스트: 본문 변조, 다른 비밀, 시간 초과, 타임스탬프 변조 거부).
- 비밀 보관(파생 방식 선택): 저장하지 않고 `whsec_`+HMAC-SHA256(AIP_SECRET, "aip-outbound-webhook:<엔티티>:<엔드포인트 id>")로 파생. SQL(pgcrypto hmac)과 러너(Rust)가 같은 값을 낸다(e2e: 응답의 비밀 == signing_secret()). 암호화 저장(W602 상황에서 평문 필드 대안)보다 우선한 근거: DB 사본에 비밀이 없고 키 관리가 AIP_SECRET 하나로 끝나며 회전/복호화 경로가 필요 없다. 대가: AIP_SECRET을 바꾸면 모든 엔드포인트의 비밀이 바뀐다(계약 operator에 명시), 같은 엔드포인트의 비밀은 재발급(회전) 불가(새로 등록). "한 번만": IR 노드 `SigningSecret{base}`(`ep.signingSecret`)를 `insert E{..} as ep`를 한 command의 `returns` select에서만 허용(그 밖은 E208), idempotent command는 금지(응답이 _aip_idempotency에 저장되어 비밀이 남는다. E208). e2e: DB의 모든 테이블(_aip_audit, _aip_idempotency, _aip_outbox 포함) 텍스트에 비밀이 없음, 같은 URL 재등록은 UNIQUE 충돌이라 두 번째 사본이 안 나옴, 조회에는 비밀 필드가 없음. IR CORE_IR_VERSION 0.8 -> 0.9. W501(비멱등 insert 바인딩)은 비밀을 내주는 command에서 면제.
- 재시도: 첫 시도 뒤 N번(최대 16, E317), n번째 재시도는 이벤트로부터 over*(2^n-1)/(2^N-1) 뒤(대기가 매번 두 배, 마지막이 정확히 over). retry 3 over 7s는 1s, 3s, 7s(e2e가 next_attempt_at - created_at으로 확인). 2xx만 성공, 4xx/5xx/시간 초과/연결 오류 모두 재시도, 서버가 부를 수 없는 URL(URL_REJECTED)은 재시도 없이 FAILED. 시도 10초 타임아웃, 리다이렉트 안 따름, 프록시 안 씀. 전달은 120초 임대로 클레임한 뒤 HTTP를 호출(보내다 죽으면 임대 만료 후 재시도: 최소 한 번).
- 비활성: 마지막 성공 이후 `disable after` 동안 실패만 했으면 비활성(`_aip_outbound_endpoint.disabled_at/disabled_reason`), 대기 전달은 CANCELLED, 이후 이벤트는 큐잉 안 함. 성공하면 실패 기간(failing_since) 초기화. 재활성화 수단은 아직 없음(새로 등록).
- SSRF: http/https만, 자격 증명(user:pw@) 거부, 호스트를 tokio lookup_host로 해석해 모든 주소가 공개여야 하고 확인한 주소로 reqwest `resolve`로 고정 연결(리바인딩 방지). 거부 대역: 0.0.0.0, 루프백, 10/8, 172.16/12, 192.168/16, 169.254/16(클라우드 메타데이터), 100.64/10, 192.0.0/24, 198.18/15, 문서용, 멀티캐스트, 브로드캐스트, 240/4, ::1, ::, fe80::/10, fc00::/7, fec0::/10, ::ffff:사설, NAT64/6to4/Teredo/문서용 IPv6. 운영 기본은 거부, `aip run --allow-private-webhook-targets`(Options.allow_private_webhook_targets, Engine.outbound.allow_private)로만 허용하고 경고 로그를 남긴다. 단위 테스트가 거부 19개와 허용 6개 주소를, e2e가 루프백 수신 서버 1개와 사설 URL 5개(169.254.169.254, 10.x, ::1, localhost, 192.168)의 전달 거부(URL_REJECTED, 수신 0건)를 확인.
- 새 의존성: aip-runtime에 reqwest 0.12(default-features=false, rustls-tls), url 2. 테스트에 axum 0.8(dev, 로컬 수신 서버).
- 계약: `describe`에 `outbound_webhooks`(요청 형식, 헤더, 보장: 최소 한 번, 순서 없음, 서명 검증과 5분 허용창, 비밀은 한 번, 공개 주소만), `operator`에 retry/over/백오프/disable after/비밀 파생식/사설 주소 설정. 폼이 없는 프로그램의 계약은 불변.

## Negative control 결과
- Part B 상시(`search_negative_controls`, 별도 DB 5개): 테넌트 필터 끔 -> carol이 B의 행을 A 검색에서 봄. visible to 끔 -> 낯선 사람이 행을 찾음. 가중치 끔 -> 제목/노트 일치도 동일. 커서 동률 처리 끔 -> 페이지 사이 누락. match 조건 끔 -> 모든 행이 적중.
- Part B 구현 변이(scratchpad/negctl5.py, mut_b.json, 변이 뒤 전체 .rs touch): match 조건 제거, 테넌트 필터 제거, visibility 제거, 가중치 무시, 커서 동률 항상 거짓, 일치도 오름차순 -> 6건 모두 search_end_to_end 실패(각각 assert: 순서/일치 수, carol_in_a, stranger, 가중치 순서, paging 누락, 순서).
- Part C 상시(`outbound_negative_controls`): 사설 주소 검사 끔(개발 설정) -> 루프백 수신 서버가 호출됨. retry 0 -> 5xx가 최종 실패. disable after 없음 -> 계속 실패해도 활성. 
- Part C 구현 변이(mut_c.json 10건) 모두 outbound_e2e 실패: 테넌트 가드 제거(`the_event_names_the_tenant_even_without_a_condition`), where 제거(main + disable), 비활성 엔드포인트 계속 큐잉(disable), 서명을 빈 본문에 대해 계산(검증 실패), 사설 주소 허용(private_addresses), 재시도 스케줄 0(스케줄 단언), 시도마다 다른 event id(본문 id != 헤더), SQL 비밀 파생식 불일치(secret equality), 비활성 조건 불성립(never disables), 성공이 실패 기간을 안 지움(streak). 원복 후 전부 통과 확인, 변이 잔재 없음(스크립트가 매번 원복 + touch).
- 실서버 스모크(aip run --dev-auth --allow-private-webhook-targets, DB aip_smoke5, 포트 4177, python 수신 4991): SearchTasks rank "0.668720", SearchProjects(한국어) 응답, AddEndpoint의 secret으로 수신된 AIP-Signature를 별도 파이썬 hmac으로 검증 True, 전달 상태 DELIVERED/1. 끝에 pkill 했고 서버, DB, 임시 파일 모두 정리.

## 골든 diff 요약 (기준선 scratchpad/base5)
- ariari.plan.json, shop.plan.json, cms.plan.json: 바이트 불변(diff -q 확인). 기존 sema_ok_* 플랜 골든도 불변. 신규: sema_ok_search.plan.json, sema_ok_outbound.plan.json.
- saas.plan.json: 의도된 변경만(검색 생성 컬럼/GIN DDL, W603 진단, notes 컬럼과 CreateTask notes 파라미터, SearchTasks/SearchProjects/AddEndpoint/ProjectEndpoints 신규 intent, Endpoint 테이블/CHECK/_aip_outbound_* 테이블, 핸들러 outbound_Endpoint_TaskCompleted, program.outbound).
- 계약/클라이언트 골든: ariari, shop, cms는 core_ir 버전 문자열 한 줄(0.7 -> 0.9)만. saas는 위 intent들과 guarantees.search, outbound_webhooks(describe) 추가.
- spec/diagnostics.md, code_sites.snap 재생성(E316, E317, W603, I122 추가, E208/E314 설명 갱신). examples/*/client/aip.ts 4개 재생성.

## 변경 파일
- IR/분석: crates/aip-ir/src/{lib,builtin,codes,validate,analyze}.rs, crates/aip-ir/tests/code_sites.snap
- 문법/프런트엔드: crates/aip-sema/src/{model,check,to_core}.rs (파서와 AST는 기존 그대로)
- 계획/백엔드: crates/aip-plan/src/lib.rs, crates/aip-pg/src/{lib,schema,ddl,plan,sqlexpr}.rs, crates/aip-pg/src/plan/outbound.rs(신규)
- 런타임: crates/aip-runtime/src/{outbound(신규),engine,dispatch,lib}.rs, crates/aip-runtime/Cargo.toml, crates/aip-cli/src/main.rs
- 계약: crates/aip-contract/src/{lib,forms}.rs
- 예제: examples/saas/app.aip, examples/{ariari,shop,saas,cms}/client/aip.ts
- 테스트: crates/aip-cli/tests/{search_e2e,outbound_e2e}.rs(신규), common/mod.rs, unavailable.rs, cms_e2e.rs(Engine 필드), crates/aip-cli/Cargo.toml(axum dev-dep), crates/aip-sema/tests/core_ir.rs(0.9), conformance/sema 신규 29개 파일(ok_search, ok_outbound, search_*, outbound_*, signing_secret_*), crates/aip-pg/tests/golden/*, crates/aip-cli/tests/golden/*
- 문서: spec/grammar.md(search, outbound webhooks 의미 절), spec/diagnostics.md(재생성), docs/DECISIONS.md(D-P22, D-P23), docs/design/09-verification-log.md

## 남은 TODO
- 비활성화된 엔드포인트를 다시 켜는 수단(지금은 새로 등록만).
- 엔드포인트별 비밀 회전(같은 엔드포인트의 새 비밀 발급). AIP_SECRET 회전 절차.
- 전달 행(`_aip_outbound_delivery`) 보관 기간 정리.
- 등록 시점의 DNS 기반 URL 검사(지금은 scheme CHECK + 전달 시점 검사).
- 앱이 엔드포인트의 비활성 여부를 읽는 방법(지금은 내부 테이블에만 있음).
- 한국어 형태소 검색(OI-08): 지금은 접두 근사.
- 확인 못함: https 대상 TLS 전달의 e2e(로컬 수신 서버가 http). rustls 빌드와 reqwest 사용은 컴파일만 확인했다.
- 확인 못함: 여러 서버 프로세스가 동시에 러너를 돌리는 경합(FOR UPDATE SKIP LOCKED 임대 클레임은 단일 프로세스 4 워커 구조에서만 실행).
- 확인 못함: 검색 인덱스의 대용량 성능(GIN 사용 여부는 EXPLAIN으로 안 봄).

## 최종 명령 출력 (2026-10-02)
- cargo fmt --all && cargo clippy -q --workspace --all-targets -> 출력        0 줄(경고 0)
- cargo test -q --workspace --no-fail-fast -> passed 131 failed 0
- ./target/debug/aip check-docs docs spec README.md -> 69 blocks: 62 parsed, 0 failed, 7 skipped (fragments with '...')
- tsc(--ignoreConfig --noEmit --strict --target es2022 --lib es2022,dom) examples/{ariari,shop,saas,cms}/client/aip.ts -> 4개 모두 exit 0
- pkill -f "aip run" -> 서버 없음(pgrep 0건)
