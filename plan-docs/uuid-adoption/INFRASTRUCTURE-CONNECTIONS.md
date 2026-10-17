# 외부 인프라 연결 통로와 우선순위

2026-10-14 코드 기준. 사용자 원칙: 권장 기본을 유지하되 실제 앱의 흔한 선택과 외부 인프라 연결 통로를 제공한다. 이번 구현은 UUID·PostgreSQL 채택까지다. 이 표의 나머지는 구현 상태 조사와 제안이며 신규 공급자를 구현했다는 뜻이 아니다. 기존 capability-gap의 2026-10-13 집계 수치를 다시 계산하지 않는다.

| 통로 | 현재 제품 지원 | 부재·경계 | 우선순위 |
|---|---|---|---|
| PostgreSQL | URL 환경변수, TLS/CA, pool, migration/runtime 역할 분리. 이번에 UUID·외부 schema introspect/mapping/DDL 비소유 모드·drift 검사 추가 | PG15+, 단일 숫자/UUID PK. 자동 introspect의 enum/numeric/JSONB·복합키·cross-schema FK 보완 필요 | P0 구현·검증, 타입/부분 schema 선택은 P1 |
| 기타 업무 DB | `mysql` feature에 optional `mysql_async` 드라이버와 MySQL 8.4 INFORMATION_SCHEMA introspection draft 경로 추가. 격리된 2-table synthetic schema read 검증 | 쓰기·transaction·작업, 전체 FaceReview schema 대조, MariaDB 서빙 미지원. 2026-10-18: 정책 read·adopt-external·principal·serve(읽기 전용)를 실제 MySQL 8.4에서 검증. SQLite/MongoDB adapter 없음 | P1: 전체 FaceReview read/write/transaction 포팅 검증; MariaDB는 별도 gate |
| 외부 IdP | issuer/audience + RS256 access token, HTTPS/file JWKS, 영속 subject→actor bind/revoke. 숫자와 UUID 업무 actor | OIDC 로그인 UI·authorization-code/PKCE는 앱/IdP 책임. 실제 외부 IdP 계정으로 smoke는 별도 | P1: 대표 IdP 운영 예제와 테스트, 임의 claim→actor 매핑은 명시적 정책 유지 |
| 메시지 브로커 | `external-brokers` feature의 Kafka(TLS/SASL)·RabbitMQ, PG managed queue/inbox/outbox relay | NATS·SQS/SNS·Redis Streams 없음. feature 없이 설정하면 거부. 실제 브로커 환경 검증은 이번 범위 밖 | P1: 설치/운영·오류/재시도 검증; 추가 공급자는 P2 |
| Object storage | local filesystem, S3-compatible endpoint/bucket/region·서명 요청, GCS HMAC | Azure Blob, GCS native OAuth·workload identity 없음. 이번에는 실제 cloud bucket을 호출하지 않음 | P1: 공급자 운영 smoke/최소 권한; Azure·workload identity는 P2 |
| 공유 캐시·세션 저장소 | SDK 메모리 캐시, write tag 무효화·세션 경계; 서버 rate limit/작업 상태는 PostgreSQL | Redis/Memcached 서버 조회 캐시, 분산 SDK 캐시 동기화 없음 | P1: 선택적 Redis adapter, 장애 시 우회·권한/배포 버전 키 포함 |
| 메일·SMS·push | notify→PG outbox→durable job, worker의 허용된 HTTP integration으로 공급자 API 호출 가능 | native SMTP/SES/SendGrid/APNs/FCM adapter·템플릿·전송 receipt·구독해지 모델 없음. notify 자체는 외부 메일 전송이 아님 | P1: 공통 송신 계약·receipt·멱등성·suppression을 먼저, 공급자 adapter는 분리 |
| 외부 HTTP API | 사전 등록 endpoint/method, timeout/size, DNS/SSRF 검사, secret header env, OAuth2 client credentials, circuit breaker | 호출자가 임의 URL/credential을 전달하는 범용 proxy는 없음 | P1: 반복하는 provider SDK를 이 capability 경계에 연결 |
| Webhook 입·출력 | 서명 검증 입력→job, outbox/worker 연계, 서명 송신과 재시도 경계 | 공급자별 signature/freshness 표준은 각각 검증 필요 | P1: 공급자 test vector와 replay/receipt 운영 예제 |
| 검색·vector·AI provider | 제품 RAG의 PG/pgvector corpus와 embedding/answer provider HTTP 설정; typed 결과 | 일반 검색용 Elasticsearch/OpenSearch/Meilisearch adapter 없음. PoC tsvector를 제품 지원으로 합산하지 않음 | P2: 검색 계약·정책 필터와 adapter, RAG 운영 공급자 검증 |
| Scheduler/workflow | 서비스 cron→durable PG job, 자체 retry/lease/DLQ/redrive | Temporal·Airflow·Celery·외부 cron service adapter 없음 | P2: 외부 trigger/event 수신 계약; 업무 인가·멱등성 유지 |
| Realtime | 자체 WebSocket·PG event cursor/subscription | 외부 managed realtime·Redis/NATS pubsub fanout 없음 | P2: 다중 instance 전달/재연결 계약을 먼저 검증 |
| 파일·정적 hosting/CDN | local static/file mount·object 전송, TLS reverse proxy 배치 | CDN invalidation·외부 NAS 관리·managed edge deployment adapter 없음 | P2: object URL·cache-control/무효화 계약과 운영 예제 |
| 관측 시스템 | 구조화 HTTP 로그, `/health`·`/ready`, 토큰 보호 `/metrics`(Prometheus 수집 통로) | native OTLP exporter·trace propagation/managed alert sink 없음 | P1: 운영 수집 예제, OTLP는 P2 |
| Secret manager·workload identity | 환경변수 이름을 통한 DB URL/공급자 비밀 연결, CA 파일 | Vault/클라우드 secret manager 자동 rotation·IAM role credential source 없음 | P1: credential provider 인터페이스와 rotation 수명; plaintext 값은 계약에 넣지 않음 |
| TLS/ingress·배포 | loopback listener + 외부 reverse proxy, DB/HTTPS TLS 검증 | AIP가 ingress/cloud provisioning 전체를 소유하지 않음. 배포 템플릿과 운영 smoke는 별도 | P1: 대표 배치와 제한 계정 runbook |

## 순서와 공통 계약

1. **P0**: 이번 UUID/기존 PG 경로를 실제 앱의 작은 slice로 닫는다. owned 기본을 유지하며 기존 schema/권한/데이터를 별도로 검증한다.
2. **P1**: DB introspect 타입·부분 매핑, 실 IdP/브로커/storage 운영 예제, 메일/알림과 Redis를 제공한다. 이들이 없어 앱마다 같은 외부 연동 코드를 다시 쓰는 비용이 큰 순서다. secret rotation·최소 권한·관측은 공급자 adapter와 함께 정의한다.
3. **P2**: 다른 DB/브로커/storage/search/workflow/realtime 공급자를 같은 계약에 추가한다. 공급자 목록 때문에 서로 다른 인가·멱등 모델을 만들지 않는다.

각 adapter는 capability 이름, 서버 설정, credential provider, timeout/size/동시성, 재시도·멱등성·미확정 결과, 관측, startup validation, 제한 역할과 실제 공급자 smoke를 제공해야 한다. SDK에서 공급자 URL이나 비밀을 직접 선택하지 않는다. 트랜잭션 밖 부작용은 outbox/effect journal과 연결하며 rollback이 외부 전송을 되돌린다고 약속하지 않는다.

## 코드 근거

- DB/IdP: `crates/aip-service/src/lib.rs`, `crates/aip-migrate/src/{lib,external}.rs`, `crates/aip-auth/src/lib.rs`.
- 브로커: `crates/aip-service/src/brokers.rs`, `crates/aip-execution/src/brokers.rs`.
- object: `crates/aip-service/src/infrastructure.rs`의 ObjectsConfig, `crates/aip-execution/src/{objects,gcs}.rs`.
- 외부 API: `crates/aip-execution/src/integrations.rs`, service의 IntegrationConfig/OAuthClientCredentialsConfig.
- 작업·trigger·notify: service `triggers.rs`, transport `durable.rs`, `crates/aip-execution/src/{messages,events}.rs`.
- 캐시·realtime·관측: `spike-v5-sdk/sdk/cache.ts`, transport의 `health.rs`·`trace.rs`·`infrastructure.rs`.
- RAG/배포 운영 경계: `product/{RAG,INFRASTRUCTURE,MANAGED-EXECUTION}.md`, `crates/aip-service/src/rag.rs`.

이 표의 상태는 공개 구현/설정 통로의 존재를 뜻한다. 이번 세션의 실환경 증거는 PostgreSQL·제한 역할·JWT fixture·설치 SDK이며, 그 결과를 cloud/IdP/Kafka/RabbitMQ 운영 계정 검증으로 확대하지 않는다.
