# 설계 결정 기록

> 최상위 기준은 [2026-10-03 창시자 통합 지침](../plan-docs/sources/founder-integrated-directive-2026-10-03.md)이다. 아래는 이전 구현 결정 이력이며 최신 기준에 충돌하면 원문이 이긴다. 현재 25개 질문/의제 상태 정본은 [B](../plan-docs/alignment/B-decision-reclassification.md).

상태 정의(철학 문서 §13):
- **Accepted**: 사용자가 명시적으로 결정한 원칙
- **Proposed**: 검토 중인 설계안. 구현돼 있어도 사용자가 확정하지 않았으면 여기다
- **Open**: 아직 해결되지 않은 문제
- **Rejected**: 검토 후 채택하지 않은 방식

각 항목은 근거 문서를 가리킨다. 구현됐다는 사실은 상태를 바꾸지 않는다.

## Accepted

| ID | 결정 | 근거 |
|---|---|---|
| D-A1 | AIP는 Backend Framework, Frontend Library, Communication Protocol의 성격을 함께 가진다 | 철학 문서 §1, §14 |
| D-A2 | 애플리케이션의 의도를 선언하고 Runtime이 검증·실행한다. 반복 백엔드 구현·유지보수 부담을 줄이는 것이 목적이다 | 철학 문서 §1, `docs/origin/design_draft.md` |
| D-A3 | 언어별로 다른 의미 체계를 만들지 않는다. 의미 표현(IR 등)은 목적을 위한 내부 도구이며, "IR 중심" 자체는 목표가 아니다 | 철학 문서 §4, 창시자 정정 10-02(`PRINCIPLES.md` §3) |
| D-A4 | JS/TS·Python 생태계와 AIP 고유 정형 표현을 함께 지향한다. 독립 .aip 작성 여부는 미정 | 통합 지침 §2 원칙 3·§5.3 |
| D-A5 | 같은 의미에 대한 표현 다양성을 제한하고, AI가 해석하기 쉬운 정형 표현을 우선한다 | 철학 문서 §2 |
| D-A6 | 표준으로 못 덮는 로직을 JS/TS·Python 공식 확장으로 자연스럽게 작성한다. 확장마다 특별 예외 승인 절차를 요구하지 않는다 | 통합 지침 §2 원칙 7·§4.3. 기존 ‘일반 수단 금지’ 표현은 표준 우선 방향으로 한정 |
| D-A7 | 서버가 최종 권한을 가진다. 클라이언트가 보낸 권한 규칙·실행 계획을 신뢰하지 않는다 | 철학 문서 §8 |
| D-A8 | SPR(Specification, Presentation, Runtime)을 아키텍처 모델로 검토·발전시킨다(잠정 명칭) | 철학 문서 §3 |
| D-A9 | 런타임·컴파일러 구현 언어는 Rust | 사용자 09-30 "Rust로 가자", `07-architecture-and-roadmap.md` ADR-001 |
| D-A10 | 장난감 수준이 아닌 실제 프레임워크. 백엔드에서 작성할 법한 모든 케이스를 문법으로 커버하는 것을 목표로 검증·보충을 반복한다 | 사용자 09-30 지시 |
| D-A12 | 호출자는 서버가 허용한 최종 계약 안에서 필요한 데이터와 동작을 표현해 요청한다. 백엔드에 매번 API를 추가하지 않는 것이 목표다. 요청의 자유와 실행 권한을 분리한다 | 창시자 정정 10-02(`PRINCIPLES.md` §1·§2) |
| D-A13 | 표준 범위 밖에서는 JS/Python을 자연스럽게 활용한다 | 창시자 정정 10-02 |
| D-A14 | 선언에 귀속된 선택적 구조화 설명. 자연어 의미 검증 비필수, 실행 권한/보안 판단 근거 금지 | 통합 지침 §2 원칙 5·§6. OI-N1은 스키마 상세만 남음 |
| D-A15 | 모든 설계·구현 단위는 `PRINCIPLES.md` §4 관문을 통과해야 한다 | 창시자 정정 10-02 |
| D-A11 | 장기 일관성과 완성도를 단기 구현 속도보다 우선한다 | 철학 문서 §0, §12 |

## Proposed

> 10-02 감사 이후 아래 Proposed 항목 중 `.aip` 문법이나 서버 선언 전용 모델을 전제로 한 것(L3 형식 문법, 진단 코드 체계의 F 규칙 등)은 OI-C1·OI-L1 결정 뒤 다시 검토한다. 구현돼 있다는 이유로 유지하지 않는다.

| ID | 제안 | 근거 / 현재 구현 |
|---|---|---|
| D-P1 | IR 층: Source AST → Core IR(`aip-ir`, Semantic IR) → 검증 → Execution Plan(`aip-plan`) → Runtime. Core IR에는 위치·SQL·HTTP·Rust 실행 객체가 없다 | 03 §1, 10 §3.1. 2026-10-01 `crates/aip-ir` v0 |
| D-P2 | Core IR 직렬화는 결정적 JSON, 첫 키 `aip_core`(버전), `digest` = canonical JSON의 sha256. 같은 의미의 프로그램은 어느 프런트엔드든 같은 digest | 10 §3.1 |
| D-P3 | 위치 정보는 Core IR 밖의 `SourceMap`(IR 경로 → 줄·열)에 둔다 | 10 §3.1 |
| D-P4 | Decimal/Money의 wire 표현은 10진 문자열. 입력은 `-?\d+(\.\d+)?`만 받는다(NaN·Infinity·지수 표기 거부). 안전 범위 정수 number도 허용. 응답도 문자열 | 10 §3.5. 2026-10-01 구현 |
| D-P7 | 효과 일관성 등급 local / staged / reservable / deferred / observational | 05 |
| D-P9 | 계약·클라이언트 타입은 Core IR에서 파생한다 | 철학 §4.1, 10 §3.2. 2026-10-02 `aip-contract` 구현 |
| D-P14 | 공개 계약에는 클라이언트가 호출에 필요한 것만 싣는다. 운영 정보(웹훅 서명 설정, 비밀 이름)는 `--operator` 출력으로 분리 | 철학 §8. 2026-10-02 구현 |
| D-P15 | 프로토콜 버전(`aip-protocol/x`)과 Core IR 버전(`aip-core/x`)을 따로 둔다 | 철학 §10. 2026-10-02 구현 |
| D-P11 | 의미 하강 탑 L3 → L2 → L1 → L0. L3 형식 추가는 가볍게, L2 개정은 무겁게 | 02 |
| D-P13 | 커스텀 바이너리 wire protocol을 만들지 않는다(HTTP/JSON + WebSocket) | 00 §5 |
| D-P16 | 이름 해석·타입 추론은 프런트엔드 책임, 해석 뒤 의미 규칙은 Core IR 분석기(`aip-ir/analyze.rs`) 책임. 모든 프런트엔드가 같은 의미 검사를 받는다 | 철학 §4, §12.4. 2026-10-02 구현 |
| D-P17 | 진단·오류 코드는 레지스트리 하나(`aip-ir/codes.rs`)가 정본, 코드 하나에 의미 하나. 공개된 코드는 의미를 바꾸지 않고 폐기 시 deprecated | 철학 §11. 2026-10-02 구현 |
| D-P18 | `tenant` 의미: (1) 테넌트 범위 행 사이 참조는 같은 테넌트만(DB 강제) (2) intent 하나는 테넌트 하나만 다룬다(파라미터 앵커, 런타임 검사) (3) 테넌트 범위 스캔에 앵커 테넌트 필터 자동 삽입, 앵커 없으면 컴파일 오류 (4) 한 트랜잭션의 쓰기는 한 테넌트(DB 트리거가 `aip.tenant`로 고정, caller 없는 문맥 포함) (5) 테넌트 간 작업은 `cross tenant`로 선언한 것만: intent는 `internal … cross tenant`, caller 없는 on/schedule/rule/consume/webhook 핸들러는 `cross tenant` 수식어. 멤버십(누가 그 테넌트에 속하나)은 `allow`의 책임으로 남긴다. (5) 트랜잭션 하나의 쓰기는 테넌트 하나(DB 트리거 고정), 항목별 문맥은 항목마다 재고정 | 08 C07, 2026-10-02 구현 |
| D-P19 | `publishable` 의미: 엔티티 테이블이 작업본, `<table>_published`가 게시본(제약 없는 스냅숏). 일반 query는 게시본 스키마로 컴파일하고 `drafts query`만 작업본을 읽는다. 생성 intent는 `PublishE`, `DiscardEDraft`. 권한은 `publishable by <조건>`, 없으면 superuser만(superuser도 없으면 AIP-E301). 삭제는 게시본도 지운다 | 08 F11(`draft/published 두 버전, Publish/Discard intent 생성`). 저장 방식과 권한은 명세 언급 없음, 2026-10-02 결정 |
| D-P20 | `consent` 의미: 나열된 intent는 호출자의 현재 버전 동의 기록이 있어야 실행(`AIP.CONSENT.REQUIRED`, 403). 기록은 `_aip_consent`에 남는 이력이고 버전을 올리면 이전 기록은 인정되지 않는다. 생성 intent `GiveXConsent`, `WithdrawXConsent`, `XConsentStatus` | 08 O03(`미동의 시 CONSENT_REQUIRED`). 403과 기록 구조는 명세 언급 없음, 2026-10-02 결정 |
| D-P21 | `impersonate` 의미: 세션 행(`_aip_impersonation`)을 서명 토큰(`aip1.<b64 target.exp.session>.<서명>`)이 가리키고, 엔진이 호출마다 세션이 열려 있는지(끝나지 않음, ttl 안, 그 target) 확인한다. 세션 안의 모든 command는 감사에 대리 대상과 운영자를 함께 남긴다. 중첩, 자기 자신, 운영자와 같은 힘을 가진 target, superuser 우회, 본인 데이터 export/erase, 동의 Give/Withdraw, approval 투표는 거부 | 08 B08(`관리자 대리 로그인, 감사 필수, 명시 승인`). 안전 규칙 전부는 명세 언급 없음, 2026-10-02 보수적으로 결정 |
| D-P22 | `search` 의미: 엔티티 테이블의 생성 컬럼(tsvector, 가중치 A~D, 생략은 D)과 GIN 인덱스. `from X.match(q) r`는 `websearch_to_tsquery`로 읽고 일치도(`ts_rank`를 소수 6자리 numeric으로 반올림) 내림차순, id 순으로 정렬한다. 일치도는 엔티티 필드가 아니라 IR 노드 `SearchRank{alias}`(Decimal)이고 쿼리 소스는 `QuerySource::Search`다. keyset 커서는 (일치도, id)이며 일치도 비교는 숫자로 한다. 가시성, 테넌트, 소프트 삭제 필터는 일반 쿼리와 같은 경로를 탄다. 단어가 없는 질의는 빈 결과, 길이 0은 파라미터 타입이 거부. `language korean`은 `simple` 설정 + 질의 단어 접두 일치로 근사하고 AIP-W603과 계약 guarantees.search로 알린다. `publishable` 엔티티의 검색은 거부(AIP-E316) | 08 E05(`postgres tsvector 또는 search ext`), E06(`엔진 선택 미결`), 09 OI-08. 가중치 기본값, 일치도 표현, 빈 질의, 접두 근사는 명세 언급 없음, 2026-10-02 결정 |
| D-P23 | `outbound webhooks` 의미: 이벤트가 디스패치될 때 조건을 만족하는 엔드포인트 행(E, `url` 필드 필수)마다 `_aip_outbound_delivery`에 전달 행을 쌓고(핸들러로 하강, 이벤트가 가리키는 테넌트의 엔드포인트만), 러너가 서명해 보낸다. 최소 한 번, 순서 없음, `AIP-Event-Id`로 dedupe. `AIP-Signature: t=,v1=`(inbound Stripe 방식과 대칭, 시도마다 t 갱신). 재시도는 첫 시도 뒤 N번, 대기 두 배, 마지막이 정확히 `over` 뒤. `disable after`만큼 성공 없이 실패하면 비활성(`_aip_outbound_endpoint`에 기록, 대기 전달 취소). 엔드포인트 비밀은 저장하지 않고 `whsec_`+HMAC(AIP_SECRET, 엔티티:id)로 파생해 생성 command의 `returns`에서 한 번만 보인다(`ep.signingSecret`, 멱등 command 금지). URL은 http(s)만, 전달 직전에 해석한 모든 주소가 공개여야 하고 확인한 주소로 연결, 사설 주소는 `--allow-private-webhook-targets`로만 | 08 H04(`고객 구독, 서명, 재시도`, 예제 `sign hmac_sha256  retry 10 over 24h  disable after 3d failing`). 헤더 형식, 비밀 보관, 재시도 곡선, SSRF 규칙은 명세 언급 없음, 2026-10-02 결정 |
| D-P24 | `subscribe` 의미: 선언된 구독만 WebSocket(`GET /aip/subscribe`)으로 열 수 있고, 결과는 id 순 목록 전체다. 갱신은 diff가 아니라 전체 재전송이고 직전과 같으면 보내지 않는다. 변경 감지는 구독이 읽는 테이블(계획 SQL에서 뽑는다)의 문장 단위 AFTER 트리거가 커밋 때 `pg_notify('aip_changed', 테이블)`하고, 런타임이 연결 하나로 듣고 그 테이블을 읽는 구독만 100ms 모아 다시 조회한다. 다시 조회는 매번 구독자로 query를 처음부터 실행한다(actor 존재, 대리 세션, allow, 가시성, 테넌트). 인증은 연결의 첫 메시지 `auth`(URL 로그에 토큰을 남기지 않는다). 계약에 `subscriptions`, TS에 `client.subscribe`. `aip-protocol/0.1` 유지, 메시지 프로토콜은 `aip-subscribe/1`. 연결, 구독, 동시 재조회, 결과 크기에 상한. 팬아웃 최적화는 OI-10 | 08 E11 "전달 시점에 가시성 재평가", `spec/grammar.md` 구독 프로토콜 | 2026-10-02 |
| D-P25 | `migration` 의미: 스키마 뒤(`aip migrate`와 `aip run` 공통) 아직 기록되지 않은 migration을 선언 순서대로 각각 한 트랜잭션에서 실행하고 `_aip_migration`(이름, 본문 digest, 시각)에 같은 트랜잭션으로 기록한다. 기록된 이름의 digest가 다르면 아무것도 실행하기 전에 `AIP.MIGRATION.CHANGED`로 실패하고, 문장이 실패하면 롤백, 미기록, `AIP.MIGRATION.FAILED`로 중단한다. 세션 advisory lock으로 동시 기동은 한 번만. actor가 없고 테넌트는 `cross tenant`로 취급한다(핀 해제, E314 검사 없음) | 08 P05 "데이터 마이그레이션/백필", `spec/grammar.md` migration | 2026-10-02 |
| D-P19 | 스키마 진화는 배포된 Core IR과 새 IR의 의미 차이로 계산한다. 데이터를 잃을 수 있는 변경(삭제, 이름 변경, 호환 안 되는 타입 변경)은 소스에 의도를 선언해야 한다(`was`, `removed field`, `removed entity`). 조임은 expand/contract로 배포를 나눈다 | 철학 §1(유지보수 부담), 2026-10-02 구현 |
| D-P20 | 계약 호환성은 IR 비교로 판정(`aip diff`), 깨짐이면 실패 종료 | 03 §10, 2026-10-02 구현 |
| D-P12 | 그 밖에 09 검증 로그의 명세 변경 SC-1~SC-5, OI 종결 결정(approval, job, versioned, webhook, rule 의미 포함) | 09, handoff §5 |

## Open

| ID | 문제 | 메모 |
|---|---|---|
| OI-C1 | **호출자가 무엇을 얼마나 표현할 수 있나**(읽기 조합, 쓰기 조합, 서버가 거는 경계와 비용 상한). AIP 존재 목적의 중심 | 감사 10-02 G-1, `docs/design/12-design-reset.md` |
| OI-R1 | 자체 포트 서버+공식 프론트 SDK는 확인됨. 자동 기동·추가 임베딩·host worker 연동 상세 검증 | 통합 지침 §3·§9 16. 기존 독립/임베드 양자택일 질문 대체 |
| OI-N1 | 선택적 구조화 설명의 소속·키·타입·공개/출력 상세 | 통합 지침 §6·7. 의미 자체는 답변됨 |
| OI-X2 | JS/Python 확장 계약(Extension ABI 포함) | 감사 10-02 |
| OI-C2 | (옛 D-P5) SQL을 컴파일 시점에 고정할지. 선언 경로에는 유효하지만 호출자 조합 요청(OI-C1)을 허용하면 런타임 계획 또는 빌드 시점 승인 목록이 필요하다 | 감사 10-02 |
| OI-DB1 | (옛 D-P6) PostgreSQL 전용 유지 여부와 DB별 capability 검증 층 | 감사 10-02 |
| OI-L1 | 선언형 A형·JS/TS·Python 연결 목표 아래 독립 파일/host 블록/정형 데이터 작성 방식 비교 | 통합 지침 §5. 독립 .aip 필요성은 Open, 폐기 확정 아님 |
| OI-T1 | Missing과 Null 구분. 부분 수정 명령에서 "안 보냄"과 `null`의 의미 | 현재 런타임은 같게 취급. 10 §3.4 |
| OI-T4 | `Decimal(p,s)` 자릿수 초과 입력 거부(현재 미검사) | Pass 6 |
| OI-T2 | Float 타입 제공 여부 | 현재 없음. Decimal만 |
| OI-T3 | Result/Error를 값 타입으로 둘지(현재는 intent 오류 계약만) | 철학 §6 |
| OI-IR1 | Core IR이 L2만 담을지, 위치 없는 L3도 1급으로 담을지 | 03 §10.1은 "L3 노드 없음" |
| OI-IR2 | L3 형식(approval, job, verification, grant link …)의 Core 하강. v0는 `forms`에 과도기로 담는다 | 10 §3.3 |
| OI-IR3 | Core IR 위 정적 분석(현재 AST 위). Verified IR의 사실(read/write/lock set, 의무) 표현 | 03 §8 |
| OI-IR4 | DB 제약 이름이 선언 위치(`ordinal`)에 의존한다. 멤버 순서만 바꿔도 DB 객체 이름이 바뀐다. 내용 기반 이름으로 바꾸면 기존 DB와 호환이 깨진다 | Pass 6 S3 |
| OI-D1 | lifecycle 위반이 프런트엔드 코드(E2xx)와 IR 구조 코드(I113) 두 가지로 표현된다. 한 의미 두 코드 | Pass 6 S2 |
| OI-D2 | E212가 아직 서로 다른 구문 오용 다섯 가지를 묶는다 | Pass 6 |
| OI-D3 | F 오류가 있을 때도 S 규칙을 함께 보고할지(오류 복구 하강, Unknown 오탐 방지 필요) | Pass 6 S2 |
| OI-TN1 | actor 없는 문맥(webhook, 핸들러)의 읽기에 테넌트 필터를 걸지. 지금은 쓰기만 트랜잭션 단위로 한 테넌트에 고정 | Pass 6 |
| OI-TN2 | actor를 테넌트에 묶을지(요청 단위 활성 테넌트). 지금 멤버십 확인은 앱의 `allow` 책임 | Pass 6 |
| OI-E1 | Custom Extension 단계(Pure / Effect-Declared / Unsafe)와 각 단계의 보장 범위 | 철학 §7 |
| OI-E2 | `fn … wasm` 실행 여부와 그 전까지 막을지 | 10 §3.7 |
| OI-P1 | 프로토콜 의미와 전송(HTTP/WebSocket) 분리, 프로토콜 버전 | 10 §3.6 |
| OI-P3 | 공개 계약의 writes(엔티티 목록)·effects(외부 서비스명)·구현 문구("one database transaction", "transactional outbox")를 클라이언트·AI 에이전트에게 보일지. Capability discovery에는 유용하나 내부 모델을 드러낸다 | 2026-10-02 관찰 |
| OI-P2 | 계약 버전 공존(구버전 클라이언트를 얼마나 오래 받을지). 판정 자체는 `aip diff`로 구현됨 | 09 OI-12 |
| OI-X1 | 분산 작업 보장 범위(`on failure` 보상 실행, saga) | 05, 철학 §9 |
| 09 OI-03~14 | 캐시 분할, repair 문법 문서화, inventory 일반성, 한국어 검색, 정기결제, 대규모 fan-out, 영업일, 낙관적 업데이트, relation 재귀 | `09-verification-log.md` |

## Rejected

| ID | 방식 | 근거 |
|---|---|---|
| D-R1 | 첫 TS PoC 수준(주문 도메인 장난감)을 출발점으로 삼기 | 사용자 09-30 "장난감 만들라고 준 아이디어가 아냐" |
| D-R2 | "한계라서 못 한다"로 표현력을 줄이는 결론 | 사용자 09-30 |
| D-R3 | 모든 작업을 하나의 DB 트랜잭션으로 원자적이라 가장하기 | 철학 §9, 00 전략 |
| D-R4 | 기존 프레임워크를 얇게 감싸 AIP라 부르기, 의미 모델 없는 API 생성, 언어별 다른 의미 | 철학 §12.3 |
| D-R5 | (옛 D-P8) 클라이언트는 선언된 intent만 호출하고 조합 요청은 일절 허용하지 않는다 | 창시자 원칙 "호출자가 원하는 데이터·동작을 표현"과 충돌. 무제한 요청도 아니므로 경계는 OI-C1에서 설계 |
| D-R6 | ⚠️ 이전 기각 이력: 독자 .aip를 정본 작성 언어로 삼기 | 10-03 통합 지침 §5.3으로 기각을 재개방. 현재 형식 여부는 OI-L1/Open이며 과거 행을 독립 파일 금지 근거로 쓰지 않는다 |
