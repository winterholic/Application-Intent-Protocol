# 백엔드 기능 갭 표 (2026-10-13 재감사)

> 기준: `origin/main` `b31660d` 이후 이 회차 커밋까지의 `aip service` 제품 경로. 2026-10-09 [역사적 matrix](capability-matrix.md)의 105개 최소 항목을 현재 코드로 다시 판정했다. root `crates/aip-runtime`·`aip-pg` 등 PoC에만 있는 기능은 제품 지원으로 세지 않고 "PoC"로 표시한다.

## 판정 기준

| 판정 | 뜻 |
|---|---|
| 지원 | Spring·NestJS·Django/FastAPI·Rails에서 흔히 쓰는 방식과 같은 일을 선언 또는 서버 설정만으로 할 수 있고, 제품 경로 테스트가 있다 |
| 부분 | 핵심 경로는 있지만 흔한 변형(정책 선언, 다른 전달 방향, 운영 도구 등)이 빠졌다 |
| 확장 | 공식 JS/TS·Python 확장(`operation`/`job` worker와 `ctx`)으로 처리한다. 계약·권한·비용·트랜잭션 통제를 유지하는 경로가 있다 |
| 미지원 | 제품 경로에 없고 확장으로도 통제를 유지한 채 처리할 방법이 없다 |

"확장"은 실패가 아니다. [창시자 지침](../sources/founder-backend-capability-directive-2026-10-09.md)은 특수 로직을 DSL로 무리하게 흡수하지 말고 JS/TS·Python으로 처리하되 통제를 잃지 않게 하라고 요구한다. 그래서 확장 판정에는 그 통제가 어떻게 유지되는지를 근거 열에 적었다.

## 이번 회차에 바뀐 판정

| 항목 | 이전(2026-10-09 → 10-12 구현 후) | 현재 | 근거 |
|---|---|---|---|
| D02 Scheduler, D03 Cron | 미지원 | 지원 | 서비스 설정 `schedules`, slot별 job key로 다중 instance 중복 방지. `tests/triggers.rs` |
| E05 Webhook | 미지원 | 지원 | 수신(`hmac-sha256`·`stripe`·`standard-webhooks`)과 구독자 URL로의 서명 발신(`ctx.webhooks.deliver`), 구독 연속 실패 비활성화 정책 예제. `tests/outbound_webhook.rs`, `tests/webhook_subscriptions.rs` |
| E02 Event handler | 미지원 | 부분 | webhook·schedule·outbox topic을 job handler로 연결. 내부 event stream·message queue를 handler에 바인딩하는 선언은 없음 |
| E06 이벤트 기반 후속 작업, E07 Outbox | 미지원 / 부분 | 지원 / 지원 | 선언된 `notify` 효과의 outbox 행을 `outbox_handlers`가 행당 job 하나로 넘김. `tests/outbox.rs` |
| C04 다단계 업무, C10 보상 | 부분 / 확장(데이터 불가) | 부분 / 확장 | job이 `ctx.data.read/apply`로 업무 데이터를 읽고 전이·보상 전이를 실행. 전이와 journal이 한 transaction. `tests/job_data.rs` |
| F09 외부 데이터 동기화 | 확장(데이터 쓰기 불가) | 확장 | schedule + `ctx.http` + `ctx.data.apply`로 끝까지 처리 가능 |
| I04 Health Check | 미지원 | 지원 | `/health`(liveness), `/ready`(DB·배포 digest). `tests/health.rs` |
| B03 테넌트 격리 | 미지원 | 부분 | 제품 경로 행 정책·기본 거부·trigger 서비스 주체 확인, 생성 Ref 가시성 규칙(결정됨). [결정 문서](tenant-isolation-decision-2026-10-13.md) |
| B09 Audit Logging | 미지원 | 부분 | 전이마다 같은 문장의 감사 행. `tests/audit.rs`, [OPERATIONS](../../product/OPERATIONS.md#업무-감사-기록) |
| I02 Metrics | 미지원 | 부분 | token 보호 `/metrics`. [OPERATIONS](../../product/OPERATIONS.md#요청-metrics) |
| C09 재시도, D08 Retry/Backoff | 부분 / 부분 | 지원 / 지원 | 설정 `job_policies`(1회에서 10회, 미처리 예외 재시도 선택), 확장의 `ctx.retryLater(reason, {afterMs})`, `ctx.job.attempt`. `tests/job_retry.rs`, [재시도 정책](../../product/MANAGED-EXECUTION.md#재시도-정책) |
| D09 DLQ, I10 운영 진단 | 부분 / 부분 | 지원 / 부분 | `aip service jobs list`(payload 미출력)·`retry`(같은 run 재큐, 미확정 효과·다른 배포·만료는 거부). [OPERATIONS](../../product/OPERATIONS.md#실패-job-조회와-재처리) |
| F10 외부 실패 처리, F11 Circuit Breaker | 부분 / 부분 | 지원 / 지원 | endpoint별 `circuit_breaker`, 열린 동안 `CIRCUIT_OPEN`(journal 기록 없음, 같은 key 재전송 가능). [Circuit breaker](../../product/MANAGED-EXECUTION.md#circuit-breaker) |
| E02 Event Handler, E04 메시지 브로커 | 부분 / 부분 | 지원 / 지원 | 서비스 설정 `message_handlers`, 메시지당 job 하나(`message:<id>` key), 수락 실패는 nack 후 DLQ. [메시지 큐 소비 job](../../product/MANAGED-EXECUTION.md#메시지-큐-소비-job) |
| F02 OAuth 외부 연동 | 미지원 | 부분 | integration `oauth2` client credentials. [OAuth client credentials](../../product/MANAGED-EXECUTION.md#oauth-client-credentials) |

## A. 데이터

| ID | 기능 | 타 프레임워크 대표 구현 | AIP 현재 | 근거·남은 갭 |
|---|---|---|---|---|
| A01 | CRUD | JPA Repository, Prisma/TypeORM, Django ORM, ActiveRecord | 부분 | read/create/공개 전이. 임의 필드 update·delete는 전이나 쓰기 확장으로 표현 |
| A02 | 관계 조회 | JPA fetch join, Django `select_related`, Rails `includes` | 지원 | 노출 관계 traverse, 깊이·팬아웃 상한. 중첩 traverse는 거부 |
| A03 | 조건·집계 | Criteria/QueryDSL, Django `annotate`, Rails `group` | 부분 | count/sum/min/max. avg·임의 산식은 확장 aggregate 결합 |
| A04 | 검색·필터 | Spring Data Specification, django-filter, Ransack | 부분 | 노출 필터 연산자. 벡터 검색은 RAG. 전문 검색 계약은 제품에 없음(PoC tsvector) |
| A05 | 정렬·페이지 | Pageable, DRF pagination, Kaminari | 지원 | sort/keyset/offset, budget 상한 |
| A06 | Bulk | `saveAll`, `bulk_create`, `insert_all` | 부분 | bounded bulk 전이, 원자적. 항목별 부분 실패 정책 없음 |
| A07 | 검증·변환 | Bean Validation, class-validator, Pydantic, AR validations | 부분 | scalar·범위·nullable·Ref·정확 키. 일반 변환은 확장 |
| A08 | 트랜잭션 | `@Transactional`, `transaction.atomic`, `ActiveRecord::Base.transaction` | 지원 | apply와 쓰기 확장 ctx가 한 DB transaction. job의 각 데이터 전이도 journal과 한 transaction |
| A09 | 잠금·CAS | `@Version`, `select_for_update`, `lock_version` | 부분 | 행·정원 advisory 잠금, `from` 조건이 사실상 CAS. 범용 version 입력 없음 |
| A10 | 캐시 | Spring Cache, Django cache, Rails.cache | 부분 | SDK 캐시와 write tag 무효화. 서버 조회 캐시 없음 |
| A11 | Migration | Flyway/Liquibase, Django migrations, Rails migrations | 지원 | plan/preflight/apply/journal, adopt. 무중단 대형 변경·역방향 없음 |
| A12 | DB별 차이 | JPA dialect, Django backends | 미지원 | PostgreSQL 전용(OI-DB1 Open) |

## B. 인증·권한·보안

| ID | 기능 | 타 프레임워크 대표 구현 | AIP 현재 | 근거·남은 갭 |
|---|---|---|---|---|
| B01 | 인증·인가 | Spring Security, Passport, Django auth, Devise | 지원 | RS256 access token·JWKS, 지속 principal, 반환 직전 재검증. 실제 외부 IdP 연동 검증은 별도 |
| B02 | RBAC/ABAC | `@PreAuthorize`, CASL, django-guardian, Pundit | 부분 | predicate·allow로 표현. 전용 policy catalog 없음 |
| B03 | 테넌트 격리 | Hibernate multitenancy, django-tenants, acts_as_tenant | 부분 | 행 정책 predicate(`org = actor.org`, membership `exists`)로 표현하고 정책 없는 resource는 기본 거부. traverse·Ref filter·전이·job도 같은 정책을 쓴다. 호출자 생성의 Ref 값도 호출자가 읽을 수 있는 행이어야 한다(actor 자기 행 예외, 라이브러리 `bundle`에서 강제). 요청 단위 활성 테넌트(T2-B)와 테넌트 선언 검사는 없고 후속 후보. [결정 문서](tenant-isolation-decision-2026-10-13.md), OI-TN1·TN2 결정됨(2026-10-13) |
| B04 | 리소스 정책 | method security, DRF permissions | 지원 | rows/field/expose/allow |
| B05 | 세션·토큰 | JWT filter, session store | 부분 | 최대 900초 세션, revoke generation. refresh는 IdP 책임 |
| B06 | API Key | API key filter, DRF TokenAuthentication | 미지원 | 기계 간 호출은 IdP client-credentials JWT로 대체 가능. 제품 API key 계약 없음 |
| B07 | 위임 접근 | `SwitchUserFilter`, django-hijack | 미지원 | PoC impersonate만 있음 |
| B08 | Rate Limiting | Bucket4j, `@nestjs/throttler`, DRF throttling, Rack::Attack | 지원 | DB 공유 fixed window, principal·경로 분류. IP별 제한은 proxy |
| B09 | Audit Logging | Hibernate Envers, django-auditlog, PaperTrail | 부분 | 배포 서버의 모든 전이가 같은 SQL 문장에서 append-only `aip_audit` 행(actor·경로·key·바뀐 필드 전후 값)을 남김. 운영자 조회 CLI는 `aip service audit list`(`crates/aip-service/tests/audit_cli.rs`). 호출자용 조회 API, 쓰기 확장 요청 key, 선언 효과 행 기록은 없음. `tests/audit.rs` |
| B10 | 민감 정보 | Jackson `@JsonIgnore`, 필드 암호화 | 부분 | 필드 가시성·redaction. 저장 암호화는 PoC |
| B11 | 비밀 관리 | Spring Cloud Vault, django-environ | 부분 | 명시한 env만 읽음(`.env` 미사용). vault 연동 없음 |

## C. 업무 로직

| ID | 기능 | 타 프레임워크 대표 구현 | AIP 현재 | 근거·남은 갭 |
|---|---|---|---|---|
| C01 | 분기 | service 코드 | 부분 | predicate·allow·전이 조건. 일반 흐름 분기는 확장 |
| C02 | 반복 | service 코드 | 부분 | bounded bulk. 임의 loop는 확장(호출 상한 적용) |
| C03 | 데이터 변환 | mapper, serializer | 확장 | operation read/job에서 JS/Python 계산. 입력·출력 계약 검사 |
| C04 | 다단계 업무 | service orchestration, Celery chain | 부분 | 전이 effects, job에서 `ctx.data`·files·events·http·messages·jobs를 journal로 조합. 단계 사이 원자성은 없음 |
| C05 | 상태 머신 | Spring Statemachine, django-fsm, AASM | 지원 | from/to/allow/repeat |
| C06 | 업무 규칙 | domain service | 부분 | check/unique/invariant/limit, 특수 규칙은 확장 |
| C07 | 유효성 검사 | Bean Validation, Pydantic | 부분 | 타입·범위·필수·참조 |
| C08 | 승인·반려 | workflow 엔진, django-river | 부분 | 전이 조합. 다단계 quorum·기한은 PoC |
| C09 | 재시도 | Spring Retry, Celery `autoretry_for`, ActiveJob `retry_on` | 지원 | apply 멱등 재생. job은 `ctx.retryLater()`·인프라 실패·선택한 미처리 예외를 journal 재생으로 재시도. `tests/job_retry.rs` |
| C10 | 보상 작업 | saga, Temporal compensation | 확장 | job이 알려진 단계 실패를 받아 보상 전이를 `ctx.data.apply`로 실행. 외부 효과 불확실 시 `EFFECT_UNKNOWN`에서 정지 |
| C11 | 멱등성 | idempotency key filter | 지원 | apply·job·effect key |
| C12 | 복합 트랜잭션 | XA/분산 tx(드묾), outbox | 부분 | 같은 DB 쓰기 확장은 원자적. job 단계별 transaction, 분산 원자성 없음 |
| C13 | 예외 처리 | `@ControllerAdvice`, exception filter | 부분 | 타입화된 Reject 코드, 데이터 전이 업무 오류를 확장에서 catch 가능. 선언적 catch 없음 |

## D. 비동기 작업

| ID | 기능 | 타 프레임워크 대표 구현 | AIP 현재 | 근거·남은 갭 |
|---|---|---|---|---|
| D01 | Job Queue | Spring Batch/JobRunr, BullMQ, Celery, Sidekiq/Solid Queue | 지원 | PostgreSQL lease·epoch fencing·heartbeat |
| D02 | Scheduler | `@Scheduled`, `@nestjs/schedule`, Celery beat, solid_queue recurring | 지원 | 서비스 설정 `schedules`, 서비스 주체 실행 |
| D03 | Cron | 같은 항목 | 지원 | 5필드·매크로·IANA 시간대·DST 규칙, misfire grace(coalesce) |
| D04 | 지연 실행 | `delay`, `apply_async(countdown)`, `set(wait:)` | 지원 | delayMs 최대 24시간. 절대 시각 지정 없음 |
| D05 | 장기 실행 | batch step, Celery long task | 지원 | job deadline 최대 1시간, heartbeat |
| D06 | 상태 조회 | JobRunr dashboard, AsyncResult | 지원 | owner 범위 status/result |
| D07 | 취소 | revoke, job cancel | 지원 | CANCEL_REQUESTED와 미확정 효과 보고 |
| D08 | Retry/Backoff | `@Retryable`, BullMQ backoff, `retry_on wait:` | 지원 | 설정 `job_policies`로 job별 1회에서 10회, `retry_uncaught_errors`. backoff는 1초에서 30초 지수 고정(사용자 지정 대기 없음) |
| D09 | DLQ | SQS DLQ, Sidekiq dead set | 지원 | message queue DLQ·redrive. job은 `aip service jobs list/retry`로 FAILED run을 같은 run으로 재처리(journal 재생). 미확정 효과 확정 명령은 없음. `crates/aip-service/tests/jobs_cli.rs` |
| D10 | 작업 의존성 | Celery chain/chord, BullMQ flow | 확장 | `ctx.jobs.run` 인라인 자식(depth 8, 호출 64). 독립 lease DAG 없음 |
| D11 | 병렬 실행 | `@Async`, Celery group | 확장 | worker 내부 병렬. durable fan-out 없음 |
| D12 | 분산 작업 | 다중 worker 노드 | 부분 | 여러 instance가 lease를 나눠 가짐. 배치 위치 제어 없음 |

## E. 이벤트

| ID | 기능 | 타 프레임워크 대표 구현 | AIP 현재 | 근거·남은 갭 |
|---|---|---|---|---|
| E01 | Domain Event | `ApplicationEventPublisher`, `EventEmitter2`, Django signals | 부분 | job의 `ctx.events.publish`. apply commit이 자동 발행하지 않음 |
| E02 | Event Handler | `@EventListener`, `@OnEvent`, signal receiver | 지원 | 전이 `notify`(outbox)·webhook·schedule·메시지 큐를 서비스 설정으로 job handler에 연결. 클라이언트용 event stream은 handler 대상이 아님 |
| E03 | Pub/Sub | Redis pub/sub, ActionCable | 부분 | owner 범위 event stream·SSE·WS |
| E04 | 메시지 브로커 | Spring Kafka, `@nestjs/microservices`, Celery broker | 지원 | 선택 빌드 Kafka/RabbitMQ relay가 관리 큐로 넣고 `message_handlers`가 job으로 소비(수락과 ack 한 transaction, 실패는 nack·DLQ). `tests/message_handlers.rs` |
| E05 | Webhook | 수신: controller + 서명 검증 / 발신: 별도 라이브러리 | 지원 | 수신은 설정만으로 서명 검증·멱등 job 수락. 발신은 job의 `ctx.webhooks.deliver`(Standard Webhooks 서명, 공개 주소만, 안정 webhook-id, 실패 재전송). 구독 관리·자동 비활성화는 앱 선언이며 정책 예제와 시험이 있음(`tests/webhook_subscriptions.rs`) |
| E06 | 이벤트 기반 후속 작업 | `@TransactionalEventListener`, `on_commit`, `after_commit` | 지원 | 전이의 `notify` 효과 + `outbox_handlers`. commit 뒤 crash에도 후속 job을 잃지 않음 |
| E07 | Outbox | Debezium outbox, transactional outbox | 지원 | 전이와 같은 transaction의 outbox 행, dispatch 표시와 job 수락이 한 transaction. outbox 자동 정리 없음 |
| E08 | 중복 처리 | inbox table | 부분 | 발행·job·effect·webhook delivery key. consumer inbox 없음 |
| E09 | 순서 보장 | Kafka partition key | 미지원 | stream cursor는 단조 증가지만 전달 순서 계약은 없음 |

## F. 외부 연동

| ID | 기능 | 타 프레임워크 대표 구현 | AIP 현재 | 근거·남은 갭 |
|---|---|---|---|---|
| F01 | HTTP API 호출 | RestClient/WebClient, HttpModule, requests/httpx, Faraday | 지원 | 이름 기반 endpoint, SSRF·DNS 고정, effect key |
| F02 | OAuth 외부 연동 | Spring OAuth2 Client, authlib | 부분 | endpoint별 `oauth2` client credentials(token 캐시·만료 전 갱신·401 시 폐기, token 실패는 확정 미전송). 사용자별 authorization code·refresh token 저장은 없음 |
| F03 | 결제 | Stripe SDK + webhook controller | 확장 | `ctx.http`(멱등 key) + Stripe webhook 수신 + 상태 전이. PSP 어댑터·대사 없음 |
| F04 | 이메일 | JavaMailSender, ActionMailer | 확장 | provider HTTP API. 템플릿·수신 동의 계약 없음 |
| F05 | SMS | provider SDK | 확장 | 같은 경로 |
| F06 | Push | FCM/APNs SDK | 확장 | 같은 경로 |
| F07 | 파일 업·다운로드 | MultipartFile, Multer, Active Storage | 지원 | managed file, local·S3·GCS object, signed URL |
| F08 | Object Storage | Spring Cloud AWS, django-storages | 지원 | local/S3/GCS HMAC. 실제 클라우드 계정 검증은 별도 |
| F09 | 외부 데이터 동기화 | 스케줄 + client + repository | 확장 | schedule → job → `ctx.http` → `ctx.data.apply`. 동기화 cursor 저장 선언은 없음 |
| F10 | 외부 실패 처리 | Resilience4j, tenacity | 지원 | EFFECT_UNKNOWN 정지, Retry-After 노출, `ctx.retryLater(reason, {afterMs})`로 확장이 대기 시간을 정해 job 재시도, `CIRCUIT_OPEN`은 확정 미전송이라 같은 key로 다시 보냄 |
| F11 | Timeout/Circuit Breaker | Resilience4j, opossum, pybreaker | 지원 | endpoint timeout, endpoint별 `circuit_breaker`(연속 실패·5xx 기준, half-open 시험). process별 상태(instance 간 공유 없음). `aip-execution/tests/integrations.rs`, `tests/job_breaker.rs` |

## G. 파일

| ID | 기능 | 타 프레임워크 대표 구현 | AIP 현재 | 근거·남은 갭 |
|---|---|---|---|---|
| G01 | 파일 검증 | content-type/size validator | 부분 | 크기·quota·media type. 내용 sniffing·악성 검사 없음 |
| G02 | 이미지 처리 | Thumbnailator, sharp, Pillow, ImageProcessing | 확장 | job + `ctx.files`, 의존성은 배포 경로 |
| G03 | 문서 변환 | LibreOffice/pandoc 연동 | 확장 | 같은 경로 |
| G04 | CSV/Excel | Apache POI, exceljs, openpyxl | 확장 | 같은 경로 |
| G05 | 대용량 파일 | streaming upload, S3 multipart | 부분 | object streaming·multipart. job 파일은 767KiB |
| G06 | Streaming | `StreamingResponseBody`, `StreamingHttpResponse` | 부분 | 정적·object 전송과 event stream. job 결과 streaming 없음 |
| G07 | 파일 메타데이터 | Active Storage blob | 지원 | 이름·media type·크기·hash·만료 |
| G08 | 파일 접근 제어 | signed URL, 권한 필터 | 부분 | owner 범위·signed URL. 행 정책과 연결된 파일 ACL 없음 |
| G09 | 저장소 추상화 | django-storages, Active Storage service | 지원 | local/S3/GCS 선택 |

## H. 실시간

| ID | 기능 | 타 프레임워크 대표 구현 | AIP 현재 | 근거·남은 갭 |
|---|---|---|---|---|
| H01 | WebSocket | Spring WebSocket, `@nestjs/websockets`, Channels, ActionCable | 지원 | `/events/socket` |
| H02 | SSE | `SseEmitter`, `@Sse` | 지원 | `/events/subscribe` |
| H03 | 실시간 알림 | ActionCable broadcast | 부분 | owner stream. 다른 사용자에게 보내는 fan-out 없음 |
| H04 | 데이터 변경 전달 | Hotwire Turbo Streams, Channels group | 부분 | 읽기 결과 구독: SSE·WebSocket에 `query`를 주면 snapshot 뒤 결과가 바뀔 때 전체를 다시 보냄(구독자로 재조회해 가시성 재평가, 같으면 안 보냄). 이 프로세스의 커밋이 즉시 깨우고 다른 쓰기는 poll이 찾음. 호출 SDK helper·인스턴스 간 즉시 신호·fan-out 최적화는 없음. [설계 메모](h04-data-subscription-2026-10-13.md), `tests/data_subscription.rs` |
| H05 | 구독·해제 | channel subscribe | 부분 | stream 구독. 이름 있는 subscription 등록 없음 |
| H06 | 사용자별 전달 | `convertAndSendToUser` | 부분 | owner 범위만 |
| H07 | 재연결 | client reconnect + replay | 부분 | cursor 재개, 보존 범위 밖은 CURSOR_EXPIRED |

## I. 운영

| ID | 기능 | 타 프레임워크 대표 구현 | AIP 현재 | 근거·남은 갭 |
|---|---|---|---|---|
| I01 | Logging | Logback MDC, pino, structlog, lograge | 부분 | JSON 이벤트 로그, 요청마다 한 줄 접근 로그와 `x-request-id`(71cfba5). 상관 ID를 job·worker 로그로 넘기는 MDC식 전파는 없음 |
| I02 | Metrics | Micrometer/Actuator, prom-client, django-prometheus | 부분 | token 보호 `/metrics`(Prometheus text), route·status·code별 요청 수와 지연 histogram. job·pool·outbox 내부 metric 없음. `tests/metrics.rs` |
| I03 | Tracing | OpenTelemetry | 부분 | 요청 ID와 `traceparent` 수신·전파(로그 줄의 `traceId`, 요청 중 외부 HTTP 호출에 같은 trace id)는 기본 빌드에 있다. span export는 없고 선택 패키지 후보(설계 메모는 [INFRASTRUCTURE](../../product/INFRASTRUCTURE.md#opentelemetry-exporter-설계-메모)). durable job 단계 호출은 trace를 잇지 못함(job 행 `trace_id` 저장이 필요한 schema 변경). `crates/aip-execution/tests/trace_context.rs`, `spikes/spike-v6-transport/tests/trace_propagation.rs` |
| I04 | Health Check | Actuator health, `@nestjs/terminus`, django-health-check, `/up`(Rails 7.1) | 지원 | `/health`, `/ready` |
| I05 | Configuration | `application.yml`, ConfigModule, settings.py | 지원 | 알 수 없는 필드 거부, 시작 시 검증 |
| I06 | Feature Flag | Unleash/OpenFeature, Flipper | 미지원 | 정적 설정 스위치만 |
| I07 | 장애 복구 | 재시도·재처리 도구 | 부분 | lease·epoch·COMMIT_UNKNOWN·EFFECT_UNKNOWN. provider 대사는 앱 책임 |
| I08 | 실행 비용 제한 | 서버 timeout·pool | 부분 | deadline·budget·동시성·cgroup. tenant 공정성 없음 |
| I09 | Resource Quota | 앱 코드 | 부분 | 행 limit, owner별 job·file·object 상한 |
| I10 | 운영 진단 | Actuator, admin | 부분 | `/capabilities`, migrate plan, readiness, 운영자 job 조회(`aip service jobs list`, payload 미출력). 감사 행·outbox 조회 CLI 없음 |
| I11 | 버전·호환성 | API versioning | 부분 | 계약 지문·배포 fence·journal |

## J. 계약 탐색

| ID | 기능 | 타 프레임워크 대표 구현 | AIP 현재 | 근거·남은 갭 |
|---|---|---|---|---|
| J01 | 계약 탐색 | springdoc OpenAPI, `@nestjs/swagger`, drf-spectacular, FastAPI `/openapi.json` | 부분 | `/capabilities`는 operation과 infrastructure. resource read/apply 전체 런타임 카탈로그 없음 |
| J02 | 기능 카탈로그 | OpenAPI tags | 부분 | operation descriptor와 available 표시 |
| J03 | 선언 메타데이터 | OpenAPI description | 부분 | docs facts. SDK·catalog 노출 범위 미확인 |
| J04 | 타입 호출 검증 | OpenAPI codegen, tRPC | 지원 | 생성 TS binding, 서버 입출력 검사 |
| J05 | 자동 발견 | OpenAPI discovery | 부분 | operation만 런타임 발견 |
| J06 | 선택 최소화 | 프레임워크 관례 | 부분 | 다섯 작성 형식·쓰기 모델 Open |
| J07 | 선택적 정적 분석 | lint | 지원 | dev-checks와 필수 sema 분리 |
| J08 | 선언 일관성 | 컴파일러 | 부분 | 이름·타입·binding·지문 검사 |
| J09 | 중복 구현 탐지 | 없음(일반 도구) | 미지원 | analyzer 없음 |
| J10 | AI 생성 로직 검증 | 테스트 | 부분 | facts·타입·SDK·실제 DB/worker 테스트 |

## 집계

이 집계는 범위 확인용이다. 항목 무게가 달라 지원율이나 생산성 지표로 쓰지 않는다.

| 판정 | 개수 |
|---|---|
| 지원 | 37 |
| 부분 | 48 |
| 확장 | 12 |
| 미지원 | 8 |

## JS/TS·Python 확장 경로 점검

탈출구가 통제를 잃지 않는지 경로별로 확인했다.

| 경로 | 계약 | 권한 | 비용 | 트랜잭션 |
|---|---|---|---|---|
| `operation read` (동기 계산) | 입력·출력 정확 키·범위 검사 | allow 사전·사후 평가, 반환 직전 인증 재검사 | deadline 30초 이하, worker slot, 본문 상한 | DB·외부 효과 capability 없음 |
| 쓰기 확장 (`extension write`) | 같은 검사 | 공개 전이만, 호출자 권한 | 호출당 ctx 20회, deadline | 서버가 연 한 transaction, ctx 실패 시 commit 안 함 |
| durable job (`operation job`) | 입력·출력 검사, 결과 typed 검증 | 선언 capability만, 매 호출 principal·배포·lease·allow 재검사 | deadline 1시간, attempt당 64회, depth 8, owner quota | file·event·message·데이터 전이는 각자 journal과 한 transaction. HTTP는 PENDING intent 후 전송 |
| `ctx.data.read/apply` (이번 회차) | `/read` planner·`expose apply` 재사용 | job 주체의 행·필드 정책. 보이지 않는 행은 `MISSING_TARGET` | 64회 상한과 read budget·bulk 상한 | 전이·deferred 검사·journal이 한 transaction, 재시도는 journal 재생 |
| schedule·webhook·outbox trigger (이번 회차) | job 입력 계약 검사 | 운영자가 bind한 서비스 주체, 비활성 시 수락 중단 | 기존 owner quota | job 수락은 한 transaction, 같은 slot·delivery는 한 job |

남은 통제 공백:

- trusted worker의 파일시스템은 macOS에서 격리되지 않는다. Linux는 Bubblewrap으로 격리한다. 확장 코드는 서버 배포자가 신뢰하는 코드라는 전제를 유지한다.
- job의 여러 데이터 전이는 각자 commit된다. 여러 전이를 원자적으로 묶어야 하면 전이 effects나 동기 쓰기 확장으로 표현해야 한다.

## Python 경로 점검 (2026-10-13 후속)

원칙 7은 표준 밖 로직을 JS/TS·Python 공식 확장으로 처리하라고 한다. Python 경로를 쓰임새별로 코드와 테스트로 확인했다.

| 경로 | 상태 | 근거 |
|---|---|---|
| 서버 확장(예외 로직) | 지원 | `spikes/spike-v4-worker/workers/worker.py`가 `operation read`·쓰기 확장·durable job을 실행하고 `ctx.data`·`http`·`files`·`events`·`messages`·`jobs`를 Node와 같은 계약으로 제공. v4·v6 suite와 service smoke의 Python worker 실행 |
| 정의 작성 | 지원 | 다섯 작성 형식 중 `.h.py`(host 블록)와 `.e.py`(정형 데이터). `spike-v1-fixture`의 형식 일치 테스트 |
| 확장 작성 편의 | 부분 | worker가 주입하는 `ctx`에 타입 stub·설치형 모듈이 없다. 오류는 `except Exception as e: e.code`로 다룬다. JS도 같은 수준이다 |
| 호출 측 SDK | 부분(이번에 추가) | 이전에는 TypeScript `@aip/sdk`뿐이었다. `product/python/aip_sdk`를 추가했다. 계약 지문 고정, 쓰기 복구, 세션 교체, operation, job·file·event를 제공한다. 생성 binding 기반 정적 타입과 read 캐시, SSE/WebSocket helper, wheel 배포는 없다 |
| 서버 bridge 어댑터 | 부분 | `product/providers/google_storage.py`(GCS ADC) 하나 |

의존성 패키지화 작업자가 "Python SDK 패키지가 없다"고 보고한 것은 호출 측 SDK를 가리킨다. 서버 확장 경로는 처음부터 있었다. 다음 단계는 `aip service gen --lang python`으로 TypedDict binding을 생성해 정적 타입을 맞추는 것과, 작업자 A의 패키지 구조에 맞춘 wheel 배포다.

## 이번 회차 설계 선택의 근거

[PRINCIPLES 설계 관문](../../docs/PRINCIPLES.md#4-설계-관문-모든-설계구현-단위가-통과해야-한다)과 [2026-10-13 심층 감사 지침](../sources/founder-deep-audit-directive-2026-10-13.md) §7(문법 증가 통제)에 대한 답이다.

| 선택 | 새 문법 여부 | 근거 | 대안과 기각 이유 |
|---|---|---|---|
| job 데이터 접근을 `access read.<Resource>`, `access <Resource>.<transition>`으로 선언 | 기존 `access` 문법 재사용. 다섯 작성 형식 parser 변경 없음 | 쓰기 확장의 `Resource.transition` 접근과 같은 표기. 실행은 기존 `/read` planner와 `expose apply`를 그대로 써서 권한 의미가 하나다 | 임의 SQL·ORM ctx: 서버 정책 우회 위험으로 기각. 별도 `data` 영역 3단 표기: parser 변경 비용 대비 이점 없음 |
| 스케줄·webhook·outbox handler를 서비스 설정에 둠 | 설정 필드만 추가 | 서비스 주체, secret 환경 변수, provider 경로는 운영 사실이다. Celery beat, solid_queue `recurring.yml`, Sidekiq-cron도 설정으로 둔다. 업무 의미(입력 계약·권한·효과)는 기존 `operation job` 선언이 그대로 책임진다 | `operation job` 안의 `schedule` 문법: 같은 job을 환경마다 다른 주기로 돌리기 어렵고 서비스 주체를 선언에 넣게 되어 기각. 다만 정의만 보고 trigger를 알 수 없는 단점이 있어 `/capabilities` 노출 여부는 후속 결정 |
| 업무 감사를 선언 없이 모든 전이에 기록 | 없음 | 지침 §7: 1) 기존 문법으로는 감사 행을 같은 transaction에 남길 수 없다(`notify`+job은 commit 뒤 별도 transaction). 2) 조합으로 만들면 앱마다 같은 감사 job을 반복한다. 3) 모든 전이 경로(`/apply`·쓰기 확장·job)가 `apply_in_with_wire` 하나를 지나므로 한 곳에서 전부 덮는다. 4) 정의 작성자가 배울 개념이 없다. 5) 전이 의미는 그대로다. 6) AI가 생성할 것이 없다. 7) 변형(조회·보존·제외 필드)은 운영 설정·CLI로 붙일 수 있다 | 전이마다 `audit` 선언: 빠뜨리기 쉽고 감사가 기본이 아니게 되어 보류. 앱 테이블 trigger: 주체·경로·요청 key를 알 수 없어 기각 |
| 전이 후속 작업을 기존 `notify` 효과 + 릴레이로 처리 | 새 효과 없음 | `notify`는 이미 같은 transaction의 outbox 행이었다. 소비자만 없었으므로 개념을 늘리지 않고 연결만 추가했다 | 전이에 `then job` 효과 추가: 개념 하나가 늘고 `notify`와 책임이 겹쳐 보류 |
| webhook·outbox 입력을 `{ body: Text }`, `{ event: Text }`로 고정 | 없음 | 외부 payload 구조는 provider마다 달라 선언 타입으로 고정할 이득이 적다. 파싱은 확장 책임으로 둔다 | payload를 typed 입력으로 매핑: 매핑 문법이 필요해 보류 |

## 남은 갭의 지침 §4 분류

| 항목 | 분류 | 메모 |
|---|---|---|
| B03 테넌트 격리, B09 감사, D08 retry 정책, E05 발신 webhook, F11 breaker, I02·I03 관측 | C. 범용 기능이지만 누락 | 반복성이 높아 내장 후보 |
| H04 데이터 변경 push, E09 순서 보장 | D. 구조적 한계에 가까움 | 행 변경 감지와 가시성 재평가 비용, owner 단위 stream 구조와 충돌 |
| A01 임의 update/delete, C08 다단계 승인 | B. 기존 동작 조합 | 전이·bulk·job 조합으로 표현. 반복 패턴이 확인되면 선언 후보 |
| F03 결제, F04·F05·F06 메일·SMS·push, G02·G03·G04 파일 처리 | E. 프로젝트별 확장이 적절 | provider·포맷이 다양함. `ctx.http`·`ctx.files`·`ctx.data` 경로로 처리 |
| B06 API key, B07 위임 | C와 E 경계 | IdP 기능(client credentials, token exchange)으로 대체 가능한지 먼저 확인 |

## 우선순위 높은 남은 갭

| 순위 | 항목 | 이유 | 권장 형태 |
|---|---|---|---|
| 1 | B03 테넌트 격리 | SaaS의 기본 요구. 현재는 predicate 관례에 의존 | [결정 문서](tenant-isolation-decision-2026-10-13.md): 결정됨(2026-10-13). 현행 유지(T1-A·T2-A) + 생성 경로 Ref 가시성 규칙 구현. 활성 테넌트(세션 claim, T2-B)는 후속 결정 후보 |
| 2 | B09 Audit | 규제·운영 요구 | 기본 기록과 조회 CLI(`aip service audit list`)는 구현(아래 설계 선택). 남은 것: 쓰기 확장 key, 효과 행 |
| 3 | I02·I03 Metrics·Tracing | 운영 가시성 | metrics와 `traceparent` 수신·전파는 의존성 없이 기본 빌드에 구현. 남은 것: 내부 상태 metric, job 단계로의 trace 연결(schema 변경), OpenTelemetry exporter(선택 패키지, 설계 메모만) |
| 4 | ~~D08·C09 retry 정책~~ | 2026-10-13 구현(`job_policies`, `ctx.retryLater(reason, {afterMs})`, `ctx.job`) | 남은 것 없음 |
| 5 | ~~E05 발신 webhook~~ | 2026-10-13 구현(ctx.webhooks.deliver), 구독 resource 예제와 연속 실패 자동 비활성화 정책(앱 선언 + 시험, [MANAGED-EXECUTION](../../product/MANAGED-EXECUTION.md#구독-관리와-연속-실패-자동-비활성화)) | 남은 것 없음 |
| 6 | ~~F11 circuit breaker, F02 client credentials~~ | 2026-10-13 구현 | 남은 것: 사용자별 OAuth token 저장(저장 암호화 B10과 함께) |
| 7 | ~~H04 데이터 변경 push~~ | 2026-10-13 PoC subscribe의 재조회·가시성 재평가 모델을 SSE·WebSocket에 이식([설계 메모](h04-data-subscription-2026-10-13.md)) | 남은 것: 호출 SDK helper, 인스턴스 간 즉시 신호(커밋 뒤 `pg_notify`), 같은 읽기의 재조회 공유(fan-out), 변경 resource 필터 |

선택 의존성 메모(작업자 A 구조 참고): 이번 회차의 schedule·webhook·outbox 릴레이·health·`ctx.data`는 추가 외부 서비스 없이 PostgreSQL만 쓰는 core 기능이라 기본 빌드에 둔다. cron 시간대 계산에 `chrono`(이미 transitive)와 `chrono-tz`를 transport에 직접 의존으로 추가했다. 3번 metrics·tracing exporter와 5번 발신 webhook의 HTTP client는 선택 패키지 후보다.
