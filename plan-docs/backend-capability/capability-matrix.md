# Backend Capability Matrix

> 후속(2026-10-10): 이 문서의 감사 기준은 `ecf3369` 시점이다. 이후 [resource 독립 동기 operation](../../product/EXECUTION.md)을 제품에 연결했고 [durable Job 구조 제안](durable-job-proposal.md)을 구체화했다. 아래 역사적 미지원/제안 항목을 현재 구현으로 합산하지 않으며 최신 지원 범위는 후속 문서를 따른다.


> 2026-10-09 코드 감사. 정본 데이터는 [CSV](capability-matrix.csv). 기능 존재, 제품 경로 도달, 실행 증거를 구분한다. 목표 Level은 채택 승인이나 현재 지원 점수가 아니다.

원문 A~J의 최소 기능 105개를 행별로 대조했다. 범위 항목 수이며 지원율·생산성 측정치가 아니다. 일반 구현·프론트/서버/확장 역할·실행 제약·제거 후보·OSS는 CSV의 개별 열에 있다. 코드 근거 ID는 [근거 목록](sources.md), 시나리오 실행은 [검증 기록](validation.md)을 따른다.

현재 Level의 `—`는 해당 공통 계약이 제품에서 확인되지 않았음을 뜻한다. `L4(로컬)`은 관리형 파일·네트워크·durable capability를 뜻하지 않는다. 세부 PoC 기능 미확인은 제품 부재 판정과 별개다. 모든 제거 후보는 구현·비교 실험 전 가설이다.

최소 목록 밖의 본인 확인·동의/개인 데이터 수명은 [추가 기능 경계](additional-capabilities.md)에 별도로 정리했다. 105개 최소 항목에 더하거나 제품 지원율로 계산하지 않는다.

## A

| ID | 기능 | 제품 현재 지원 | 현재 Level | 목표 Level | 우선순위 |
|---|---|---|---|---|---|
| A01 | CRUD | 부분: read/create/공개 전이; 임의 필드 update/delete 없음 [P02,P03] | L1+L2+L3 | L1+L2+L3 | P1 |
| A02 | 관계 데이터 조회 | 지원: 노출 관계의 단일·다중 traverse [P02] | L1+L2+L3 | L1+L2+L3 | P2 |
| A03 | 복잡한 조건과 집계 | 부분: 조건/count/sum/min/max; avg·임의 산식 아님 [P02] | L1+L2+L3 | L1+L2+L3 | P1 |
| A04 | 검색과 필터링 | 부분: 노출 filter; 전문 검색 엔진 계약 없음 [P02] | L1+L2+L3 | L1+L2+L3 | P2 |
| A05 | 정렬과 페이지네이션 | 지원 범위 내 sort/keyset/offset [P02] | L1+L2+L3 | L1+L2+L3 | P2 |
| A06 | Bulk Operation | 부분: bounded bulk 전이·many 효과; 원자적 [P03] | L1+L2+L3 | L1+L2+L3 | P1 |
| A07 | 데이터 검증 및 변환 | 부분: scalar·nullable·Ref 타입; 일반 변환은 확장 [P01,P04] | L1+L3+L4 | L1+L2+L3+L4 | P1 |
| A08 | 트랜잭션 | 지원: DB 단위 원자적 apply [P03,P06] | L1+L3 | L1+L2+L3 | P1 |
| A09 | 낙관적·비관적 잠금 | 부분: 행·권한·advisory 잠금; 범용 version-CAS 미확인 [P03] | L1+L3 | L1+L3 | P1 |
| A10 | 캐시 | 부분: SDK deps·세션 수명 캐시 [P05] | L1+L3 | L1+L3 | P2 |
| A11 | Migration | 지원 범위 내 plan/preflight/apply/journal [P08] | L1+L3 | L1+L3 | P2 |
| A12 | 데이터베이스별 기능 차이 | PG 단일 백엔드; DB 간 차이 처리 계약 없음 [P02,P08] | — | L1+L3 | P3 |

## B

| ID | 기능 | 제품 현재 지원 | 현재 Level | 목표 Level | 우선순위 |
|---|---|---|---|---|---|
| B01 | 인증과 인가 | 부분: JWT/지속 principal/행·필드 정책 [P07,P01] | L1+L3 | L1+L3 | P0 |
| B02 | RBAC / ABAC | 부분: predicate로 role/속성 조건; 전용 policy catalog 아님 [P01] | L2+L3 | L1+L2+L3 | P0 |
| B03 | 사용자·조직·테넌트 격리 | 부분: predicate로 표현 가능; 전용 tenant 강제 없음 [P01,P03] | L2+L3 | L1+L3 | P0 |
| B04 | 리소스별 접근 정책 | 지원: rows/field/expose/allow [P01,P02,P03] | L3 | L3 | P0 |
| B05 | 세션 및 토큰 | 부분: JWT·principal 폐기·세션 수명 [P07,P06] | L1+L3 | L1+L3 | P0 |
| B06 | API Key | 제품 공통 API Key 계약 없음 [P06,P07] | — | L1+L3 | P2 |
| B07 | 위임 접근 | 제품 위임 계약 없음 [P07] | — | L1+L3 | P2 |
| B08 | Rate Limiting | 제품 공통 rate limiter 없음; 연결 상한과 다름 [P06] | — | L1+L3 | P1 |
| B09 | Audit Logging | 제품 업무 감사 계약 없음; 배포 저널은 별개 [P08,P11] | — | L1+L3 | P1 |
| B10 | 민감 정보 보호 | 부분: 가시성·redaction; 제품 저장 암호화 계약 없음 [P02,P05] | L1+L3 | L1+L3 | P1 |
| B11 | 비밀 정보 관리 | 제품 외부 secret broker 없음; worker env 제거 [P04] | — | L1+L3 | P1 |

## C

| ID | 기능 | 제품 현재 지원 | 현재 Level | 목표 Level | 우선순위 |
|---|---|---|---|---|---|
| C01 | 조건문과 분기 | 부분: predicate/from/allow/repeat; 일반 flow branch 없음 [P01,P03] | L2+L3+L4 | L2+L3+L4 | P1 |
| C02 | 반복 처리 | 부분: bounded bulk/many; 임의 loop 아님 [P03] | L2+L3+L4 | L1+L2+L3+L4 | P1 |
| C03 | 데이터 변환 | 부분: 확장 내 로컬 계산 [P04,T01] | L4 | L1+L2+L3+L4 | P1 |
| C04 | 다단계 업무 처리 | 부분: DB 전이 effects; 장기 flow 없음 [P03] | L2+L3 | L2+L3+L4 | P0 |
| C05 | 상태 머신 | 지원 범위 내 from/to/allow/repeat [P01,P03] | L3 | L1+L2+L3 | P1 |
| C06 | 업무 규칙 | 부분: predicate/check/unique/invariant/limit [P01,P03] | L2+L3 | L1+L2+L3+L4 | P1 |
| C07 | 유효성 검사 | 부분: 타입·범위·필수·참조 검증 [P01,P03,P04] | L1+L3 | L1+L2+L3 | P1 |
| C08 | 승인 및 반려 | 부분: 전이 조합; 다단계 승인 전문 계약 없음 [P03] | L2+L3 | L1+L2+L3 | P1 |
| C09 | 재시도 | 부분: apply 같은 키 재생; 업무 retry 정책 아님 [P06] | L1 | L1+L3 | P0 |
| C10 | 보상 작업 | 제품 durable 보상 계약 없음 [P03,P06] | — | L1+L3+L4 | P0 |
| C11 | 멱등성 | 지원: apply 결과 동일 tx 기록/조회 [P06] | L1+L3 | L1+L2+L3 | P0 |
| C12 | 복합 트랜잭션 | 부분: 같은 DB apply 원자적; 분산 원자성 없음 [P03] | L2+L3 | L2+L3+L4 | P0 |
| C13 | 예외 처리 | 부분: Reject·출력검사·rollback; 표준 flow catch 없음 [P03,P04,P06] | L1+L4 | L1+L2+L3+L4 | P1 |

## D

| ID | 기능 | 제품 현재 지원 | 현재 Level | 목표 Level | 우선순위 |
|---|---|---|---|---|---|
| D01 | Job Queue | 제품 durable queue 없음 [P06] | — | L1+L3 | P0 |
| D02 | Scheduler | 제품 scheduler 없음 [P06] | — | L1+L3 | P1 |
| D03 | Cron | 제품 cron 계약 없음 [P06] | — | L1+L3 | P2 |
| D04 | 지연 실행 | 제품 durable delay 없음 [P06] | — | L1+L3 | P1 |
| D05 | 장기 실행 작업 | 제품은 동기 확장만; 긴 동기 실행은 가능 [T01] | L4(동기) | L1+L3+L4 | P0 |
| D06 | 작업 상태 조회 | /status는 쓰기 멱등 결과; Job 상태 아님 [P06] | — | L1+L3 | P0 |
| D07 | 작업 취소 | 제품 취소 요청 계약 없음 [P06] | — | L1+L3+L4 | P0 |
| D08 | Retry / Backoff | 제품 Job retry 없음 [P06] | — | L1+L3 | P0 |
| D09 | Dead Letter Queue | 제품 DLQ 없음 [P06] | — | L1+L3 | P1 |
| D10 | 작업 간 의존성 | 제품 dependency 계약 없음 [P06] | — | L2+L3 | P2 |
| D11 | 병렬 실행 | 확장 내 로컬 가능; 표준 durable 병렬 없음 [P04] | L4(로컬) | L1+L2+L3 | P2 |
| D12 | 분산 작업 | 제품 분산 Job 계약 없음 [P06] | — | L1+L3+L4 | P2 |

## E

| ID | 기능 | 제품 현재 지원 | 현재 Level | 목표 Level | 우선순위 |
|---|---|---|---|---|---|
| E01 | Domain Event | 부분: notify 제한 형식; 일반 payload event 없음 [P03,P09] | L3(제한) | L1+L2+L3 | P0 |
| E02 | Event Handler | 제품 handler 연결 없음 [P06,P09] | — | L1+L3+L4 | P1 |
| E03 | Pub/Sub | 제품 공통 pubsub 없음 [P06] | — | L1+L3 | P1 |
| E04 | Kafka·RabbitMQ 등 메시지 브로커 | 제품 broker adapter 없음 [P06] | — | L1+L3+L4 | P2 |
| E05 | Webhook 발행 및 수신 | 제품 webhook 경로 없음 [P06] | — | L1+L3+L4 | P1 |
| E06 | 이벤트 기반 후속 작업 | 제품 outbox 기록 이후 경로 미연결 [P09] | — | L1+L2+L3 | P0 |
| E07 | Outbox Pattern | 부분: 같은 tx notify insert; 제품 consumer 없음 [P03,P09] | L1+L3(기록) | L1+L3 | P0 |
| E08 | 이벤트 중복 처리 | 제품 delivery dedupe 없음; apply idem과 별개 [P06,P09] | — | L1+L3 | P0 |
| E09 | 이벤트 순서 보장 | 제품 delivery 순서 계약 없음 [P09] | — | L1+L3 | P2 |

## F

| ID | 기능 | 제품 현재 지원 | 현재 Level | 목표 Level | 우선순위 |
|---|---|---|---|---|---|
| F01 | HTTP API 호출 | 제품 broker 없음; 확장 network deny [P04,P06] | — | L1+L3+L4 | P0 |
| F02 | OAuth 기반 외부 서비스 연동 | 제품 외부 OAuth 계약 없음 [P04] | — | L1+L3+L4 | P2 |
| F03 | 결제 시스템 | 제품 결제 capability 없음 [P04,P06] | — | L1+L3+L4 | P1 |
| F04 | 이메일 | notify 기록만; 제품 발송 없음 [P09] | — | L1+L3+L4 | P1 |
| F05 | SMS | 제품 SMS capability 없음 [P04,P09] | — | L1+L3+L4 | P2 |
| F06 | Push Notification | 제품 push capability 없음 [P04,P09] | — | L1+L3+L4 | P2 |
| F07 | 파일 업로드 및 다운로드 | 제품 JSON만; 관리형 file route 없음 [P06] | — | L1+L2+L3 | P1 |
| F08 | Object Storage | 제품 storage capability 없음 [P06] | — | L1+L3+L4 | P1 |
| F09 | 외부 데이터 동기화 | 제품 sync 계약 없음 [P04,P06] | — | L1+L2+L3+L4 | P1 |
| F10 | 외부 API 실패 처리 | 제품 외부 실패 모델 없음 [P04] | — | L1+L3+L4 | P0 |
| F11 | Timeout / Circuit Breaker | 부분: worker deadline/DB timeout; 외부 breaker 없음 [P04,P06,T01] | L1(일부) | L1+L3 | P0 |

## G

| ID | 기능 | 제품 현재 지원 | 현재 Level | 목표 Level | 우선순위 |
|---|---|---|---|---|---|
| G01 | 파일 검증 | 관리형 file validator 없음 [P06] | — | L1+L3+L4 | P1 |
| G02 | 이미지 처리 | 제품 표준 이미지 계약 없음; 로컬 확장 가능 [P04] | L4(로컬 제한) | L1+L2+L3+L4 | P2 |
| G03 | 문서 변환 | 제품 관리형 문서 변환 없음 [P04,P06] | L4(로컬 제한) | L1+L3+L4 | P1 |
| G04 | CSV / Excel 처리 | 제품 표준 파일 처리 없음 [P06] | — | L1+L2+L3+L4 | P1 |
| G05 | 대용량 파일 처리 | 제품 1MiB JSON; 대용량 경로 없음 [P06] | — | L1+L3+L4 | P1 |
| G06 | Streaming | 제품 bounded JSON만 [P06] | — | L1+L3+L4 | P2 |
| G07 | 파일 메타데이터 | 제품 managed metadata 없음 [P06] | — | L1+L3 | P1 |
| G08 | 접근 제어 | 행 정책은 있음; file executor 없음 [P01,P06] | L3(데이터) | L1+L3 | P0 |
| G09 | 저장소 추상화 | 제품 storage executor 없음 [P06] | — | L1+L3+L4 | P2 |

## H

| ID | 기능 | 제품 현재 지원 | 현재 Level | 목표 Level | 우선순위 |
|---|---|---|---|---|---|
| H01 | WebSocket | 제품 WS 경로 없음 [P06] | — | L1+L3 | P1 |
| H02 | Server-Sent Events | 제품 SSE 없음 [P06] | — | L1+L3 | P2 |
| H03 | 실시간 알림 | 제품 outbox 기록만 [P09] | — | L1+L2+L3 | P1 |
| H04 | 실시간 데이터 변경 전달 | 제품 SDK TTL는 realtime 아님 [P05,P06] | — | L1+L2+L3 | P1 |
| H05 | 구독 및 해제 | 제품 subscribe contract 없음 [P06] | — | L1+L2+L3 | P1 |
| H06 | 사용자별 이벤트 전달 | 제품 per-user delivery 없음 [P09] | — | L1+L3 | P0 |
| H07 | 연결 및 재연결 관리 | 제품 durable reconnect 없음 [P06] | — | L1+L3 | P1 |

## I

| ID | 기능 | 제품 현재 지원 | 현재 Level | 목표 Level | 우선순위 |
|---|---|---|---|---|---|
| I01 | Logging | 부분: 기동/정지 report·panic 로그; 요청 공통 로그 부족 [P11] | L1(일부) | L1+L3 | P1 |
| I02 | Metrics | 제품 metrics endpoint 없음 [P06,P11] | — | L1+L3 | P1 |
| I03 | Distributed Tracing | 제품 공통 trace 계약 없음 [P11] | — | L1+L3 | P2 |
| I04 | Health Check | 제품 health route 없음 [P06] | — | L1+L3 | P1 |
| I05 | Configuration | 지원 범위 내 명시적 service 옵션 [P11] | L1+L3 | L1+L3 | P2 |
| I06 | Feature Flag | 제품 공통 feature flag 없음 [P01,P06] | — | L1+L3+L4 | P2 |
| I07 | 장애 복구 | 부분: commit unknown/idem·배포 journal [P06,P08] | L1+L3(일부) | L1+L3 | P0 |
| I08 | 실행 비용 제한 | 부분: read budget/연결·frame·worker 제한; 실행 전체 deadline은 별도 [P01,P04,P06,T01] | L1+L3 | L1+L3 | P0 |
| I09 | Resource Quota | 부분: 정수 atMost 행 제한; 범용 quota 없음 [P01,P03] | L1+L3(행 수) | L1+L3 | P1 |
| I10 | 운영 진단 | 부분: migration plan/preflight; 작업 진단 없음 [P08,P11] | L1(일부) | L1+L3 | P1 |
| I11 | 버전 및 호환성 관리 | 부분: 계약 지문/배포 fence·migration [P05,P06,P08] | L1+L3 | L1+L3 | P0 |

## J

| ID | 기능 | 제품 현재 지원 | 현재 Level | 목표 Level | 우선순위 |
|---|---|---|---|---|---|
| J01 | 정형화된 계약 탐색 | 부분: 생성 SDK/정적 descriptor; runtime describe 없음 [P05,P06] | L1+L3 | L1+L3 | P1 |
| J02 | 표준 기능 카탈로그 | 제품 공통 backend capability catalog 없음 [P05,P06] | — | L1+L3 | P1 |
| J03 | 선언 메타데이터 | 부분: 선택 docs facts; SDK 노출 전체 미검증 [P01,P05] | L3 | L1+L3 | P2 |
| J04 | 타입 기반 호출 검증 | 지원 범위 내 생성 TS·서버 입력/출력 검사 [P01,P04,P05] | L1+L3 | L1+L2+L3 | P0 |
| J05 | 사용 가능한 기능의 자동 발견 | 부분: 정적 binding만; runtime catalog 없음 [P05,P06] | L1(정적) | L1+L3 | P1 |
| J06 | 코드 생성 시 불필요한 선택 최소화 | 부분: 다섯 작성 형식·여러 쓰기 모델 미결정 [P01,P05] | L1(일부) | L1+L2+L3 | P1 |
| J07 | 선택적 정적 분석 | 지원 범위 dev-checks + 필수 sema 구분 [P01] | L1+L3 | L1+L3 | P2 |
| J08 | 선언 간 일관성 검사 | 부분: 이름·타입·binding·지문 검사 [P01,P05] | L1 | L1+L3 | P1 |
| J09 | 중복 구현 탐지 | 제품 공통 duplication analyzer 없음 [P01] | — | L1+L3 | P3 |
| J10 | AI가 생성한 로직의 검증 가능성 | 부분: facts/type/SDK/실제 DB·worker tests [P01,P04,P05,T01] | L1+L3 | L1+L2+L3+L4 | P0 |
