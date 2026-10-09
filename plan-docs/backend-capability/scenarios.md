# 여섯 백엔드 시나리오 대조

2026-10-09. **`실제 제품 문법`은 현재 parser/runtime가 받는 일부 동작, `PoC 실제 문법`은 별도 root engine, `가상 제안`은 미구현 설명용 계약이다.** 아래 전체 업무 계약의 JSON/YAML은 가상이며 최종 SDK나 DSL을 결정하지 않는다. 기존 검증 범위는 [validation](validation.md), 외부 코드 근거는 [sources](sources.md).

| 시나리오 | 현재 제품으로 가능한 부분 | PoC 실행 근거 | 아직 제품에서 증명 못한 핵심 |
|---|---|---|---|
| A 전자상거래 | 재고/전이·관계·불변조건·멱등 쓰기 | shop_e2e, Saleor 파생 제품 probe | 실제 결제·알림 delivery·durable 보상 |
| B 승인 | 상태 전이·권한·bounded bulk | ariari_approval | 승인 전문 계약·예약 만료·감사·전달 |
| C SaaS | predicate·집계·정수 건수 제한 | saas_e2e/negative controls | 모든 executor의 tenant pin·계량 quota·초대 발송 |
| D 연동 | 타입 검사·로컬 변환·DB 저장 | outbound_e2e | 일반 HTTP/OAuth/secret broker·전체 sync flow |
| E 협업 | 표준 조회·SDK 캐시 | subscribe_e2e | 제품 subscription·reconnect/replay |
| F SQL 없는 작업 | SQL 없는 동기 JS/Python 계산 | T01; PoC CSV Job | resource 비종속 file/compute durable lifecycle |

## A — 전자상거래

### 1. 기존 구성과 현재 근거

전통 구현에는 주문 Controller/입력 DTO, 재고 Repository/락, 가격 Service, 결제 adapter, webhook signature/dedupe, outbox/worker, 환불·재고 복구 saga, 알림 service와 status API가 필요하다. Saleor는 상품-기본 variant 순환 관계를, InvenTree는 재고·비동기 업무의 실용적 반례를 제공한다. 현재 제품의 상품 probe는 상품 관계와 활성화 원자성만 검증한다. 전체 Saleor checkout이나 PSP를 실행한 결과가 아니다.

### 2. 프론트 표현

**실제 제품 요청의 일부**: `POST /apply`의 body. 이 상품 활성화 계약은 현재 제품 테스트에 있다. Id 예시는 DecimalString 후보다.

```json
{"key":"activate-101","request":{"apply":"ProductVariant.activate","target":{"ids":["101"]}}}
```

**가상 제안, 미구현 전체 checkout 호출**: 표준 동작으로 풀 수 없는 재고·가격·결제 경계를 재사용 L3 계약으로 묶는다.

```json
{"capability":"commerce.checkout","input":{"cart":"cart-42"},"key":"checkout-42-v1"}
```

### 3. 서버 Specification

**실제 제품 문법 발췌**, `oss_product_variants.rs`의 ProductVariant 내부:

```aip
transition activate {
  allow product.owner = actor
  from status = DRAFT
  to status = ACTIVE
  repeat unchanged
  update Product where id = product { defaultVariant = this.id }
  notify product.owner "product.default_changed"
}
expose apply activate { target id; bulk maxRows 1 }
```

이는 DB 전이/알림 의도 기록까지다. **가상 제안, 미구현 전체 서버 계약**:

```yaml
capability: commerce.checkout
input: { cart: CartId }
authorize: cart.owner == authenticatedActor
price: authoritativeCartQuote
steps: [reserveStock, requestPayment, reconcilePayment, confirmOrCompensate, notify]
transaction: reserveStockAndRecordEffectLocally
payment: registeredProvider
idempotency: principalTenantContractAndInput
compensation: releaseReservationOrRefundAccordingToObservedPayment
```

### 4. Runtime 표준 책임

L1 타입·권한·멱등·DB transaction·outbox/inbox·retry/deadline·작업 상태, L2 공개 조회/동작 조합, L3 주문 불변조건·보상 순서. 현재 제품은 타입·권한 검사, DB 원자성, 멱등 복구와 outbox 기록까지 연결되어 있다. delivery/inbox·업무 retry·Job 상태는 연결되지 않았다.

### 5. 확장 필요 부분

L4는 PSP adapter, 세금·쿠폰 등 프로젝트 고유 가격 산식이다. HTTP timeout·secret 로딩·dedupe·queue status까지 각 adapter가 반복 구현하지 않도록 표준 broker가 담당하는 방향이다.

### 6. 남는 애플리케이션 코드

상품·재고·가격 정책, 결제수단별 특수 mapping, 환불 가능 조건, 고객 알림 template. 업무의 금액·재고 정본을 frontend로 옮기지 않는다.

### 7. 보안·트랜잭션 및 실패 경계

재고 예약과 효과 제출은 local commit. 결제는 별도 효과이며 timeout은 unknown이다. provider 성공 뒤 ack 유실은 같은 효과 key로 조회/재시도한다. 중복 webhook, 취소와 결제 경합, 재고 복구/환불 실패는 각각 검증해야 한다. 현재 shop PoC는 서명·중복·재고 경합의 일부를 시험했고 실제 PSP settlement·환불은 **미실행**이다. 상품 defaultVariant가 동일 상품에 속하는지는 FK+unique만으로 보장되지 않는 별도 불변조건이다.

### 8. 제거한 중복과 제거 후보

현재 제품의 활성화 예시는 별도 Controller/DTO/Service 없이 선언+표준 apply로 관계 갱신·권한·rollback를 처리했다. 전체 checkout의 webhook/queue/saga glue 제거는 **제안 가설**이며 아직 구현·유지보수 감소를 측정하지 않았다.

## B — 업무 승인

### 1. 기존 구성과 현재 근거

신청 DTO/Controller, 검증 Service, 조직·승인자 조회, 단계별 승인/반려, 만료 scheduler, 알림 worker, audit interceptor가 필요하다. Frappe의 실제 workflow는 전이 선택·자기승인 방지·동기 task와 commit 이후 비동기 task를 분리한다. AIP 제품은 전이 조합, PoC는 quorum/기한 만료를 각각 검증했다.

### 2. 프론트 표현

**가상 제안, 미구현 승인 전문 계약**:

```json
{"capability":"approval.decide","input":{"request":"req-7","decision":"approve","expectedStage":2},"key":"decision-req7-stage2-user1"}
```

기존 전이로 충분한 단순 승인에는 새로운 named 계약을 만들 필요가 없다. 공개 전이와 허용 target을 표준 apply로 표현한다. 위 예시는 다단계/정족수/만료가 있는 경우다.

### 3. 서버 Specification

**가상 제안, 미구현 설명용 정책**:

```yaml
spec: expenseApproval
authorize: currentApproverAndOrganizationMembership
stages: [teamLead, financeWhenAmountExceedsThreshold]
selfApproval: denied
onReject: closeWithReason
onDeadline: expireOrEscalate
notifications: [stageAssigned, rejected, completed]
audit: actorTenantStageDecisionAndOutcome
```

### 4. Runtime 표준 책임

L1 상태 전이 검사·중복 결정 방지·expected stage/CAS·audit·예약·전달·재시도, L2 조건부 표준 조합, L3 단계·정족수·업무 권한. 제품의 일반 version-CAS와 audit/예약 실행은 아직 지원으로 주장하지 않는다.

### 5. 확장 필요 부분

L4는 특수 조직도·교대 근무 기반 승인자 결정과 고유 규정 계산이다. 승인 이력 insert, timeout 재시도, 메일 queue 구현은 공통화 후보다.

### 6. 남는 애플리케이션 코드

승인 단계·조건, 자기승인/대체 승인 규칙, 반려 사유 schema, 업무 template와 특수 승인자 계산. 단순 상태 변경은 기존 전이로 유지한다.

### 7. 보안·트랜잭션 및 실패 경계

투표와 단계 확정·event 기록은 같은 local transaction. 만료 직전 승인, 동일 사용자 중복 투표, 역할 회수, 반려 후 재신청은 상태/권한을 다시 평가한다. 알림 실패가 승인 commit을 없애지는 않는다. PoC approval 만료 시험과 Frappe 테스트 소스를 근거로 삼되 **제품 다단계·예약·전달 전체 E2E는 미실행**이다.

### 8. 제거한 중복과 제거 후보

현재 전이는 상태/권한/rollback 공통 코드를 맡는다. 목표 전문 계약은 승인별 scheduler·audit·중복 투표·알림 glue를 제거할 후보이며, 업무 규칙 자체가 사라지는 것은 아니다.

## C — SaaS 멀티테넌트

### 1. 기존 구성과 현재 근거

조직 생성·초대·멤버십/role service, tenant middleware/query scopes, 캐시 key, queue context, usage counter, plan/quota/billing service가 필요하다. Frappe 초대 소스는 허용 role·만료·중복 수락을 검사하고, Twenty는 workspace별 token bucket와 stale-run counter 복구를 구현한다. 제품 predicate·집계·건수 상한과 PoC tenant pin은 다른 지원 수준이다.

### 2. 프론트 표현

**가상 제안, 미구현 초대/계량 계약**:

```json
{"capability":"organization.invite","input":{"organization":"org-3","email":"member@example.test","role":"member"},"key":"invite-org3-member-v1"}
```

조직별 목록 화면은 기존 공개 read의 필드·관계·filter 조합을 사용한다. 화면마다 `ListOrganizationX`라는 새 서버 계약을 만드는 방향이 아니다.

### 3. 서버 Specification

**가상 제안, 미구현 설명용 정책**:

```yaml
spec: organizationMembership
invite: { actor: organizationAdmin, assignableRoles: [member], expires: boundedDuration }
tenantBoundary: authenticatedMembershipAndAnchoredOrganization
quota: { unit: activeSeat, source: organizationPlan, reserveAtomically: true }
usage: { period: billingPeriod, key: stableConsumptionId }
applyTo: [read, write, aggregate, cache, job, file, event, subscription]
```

### 4. Runtime 표준 책임

L1 tenant anchoring·참조 검사·context 전파·멱등 초대 수락·quota 예약/해제·기간 계량, L2 조회/집계, L3 멤버십·role·요금제 정책. 기존 `count/sum/min/max`로 사용량 조회 일부는 가능하다. 현재 atMost 정수 상한이 요금제별 동적 quota/billing 전체를 대신하지는 않는다.

### 5. 확장 필요 부분

L4는 외부 IdP provisioning, 결제 adapter, 특수 과금 산식이다. query마다 tenant filter나 queue마다 tenant context를 수기로 붙이는 것은 제거 후보다.

### 6. 남는 애플리케이션 코드

조직/멤버십 모델, role 위임 정책, 요금제 단위와 계량 event 의미, 청구 규칙. tenant를 caller가 보내면 그 값을 검증하는 서버 정책이 남는다.

### 7. 보안·트랜잭션 및 실패 경계

초대 key는 조직·대상·만료·허용 role에 바인딩한다. 좌석 마지막 하나에 동시 초대, job 실행 전 권한 회수, 타 tenant 파일/구독/캐시 접근, 두 조직 소속 사용자의 교차 참조를 시험한다. 이번 PoC saas_negative_controls는 격리층을 제거하면 공격이 성공하는지까지 재실행했다. **모든 새 executor에 적용되는 제품 tenant 보장은 아직 미구현**이다.

### 8. 제거한 중복과 제거 후보

제품은 공개 read/aggregate의 호출 타입·가시성 검사를 재사용한다. PoC는 filter·참조·쓰기 tenant pin의 공통 동작을 시험했다. 제품 전 실행 경로의 tenant boilerplate와 quota counter 제거는 추가 구현·반례 검증 대상이다.

## D — 외부 서비스 연동

### 1. 기존 구성과 현재 근거

HTTP client, OAuth/secret provider, DTO validator/mapper, transaction Service, event publisher, retry/backoff와 scheduler가 필요하다. Frappe email queue 및 일반 framework의 HTTP/queue 구성은 데이터 저장 자체보다 외부 실패·권한·재시도 glue가 반복됨을 보여준다.

### 2. 프론트 표현

**가상 제안, 미구현 등록 연동 호출**:

```json
{"capability":"shipment.refresh","input":{"shipment":"shipment-8"},"key":"refresh-shipment8-v3"}
```

caller는 임의 URL·OAuth token·secret 이름을 보내지 않는다. 서버가 등록한 shipment provider와 허용 입력만 선택한다.

### 3. 서버 Specification

**가상 제안, 미구현 설명용 계약**:

```yaml
spec: shipmentRefresh
authorize: shipmentTenantMember
external: { capability: carrier.status, connection: serverRegisteredConnection }
response: validatedCarrierStatus
transform: canonicalShipmentStatus
commit: [updateShipmentIfVersionMatches, recordStatusChangedEvent]
retry: boundedRetryAfterAndDeadline
unknownEffect: reconcileBeforeRepeatingMutation
```

### 4. Runtime 표준 책임

L1 HTTP broker·secret scope·timeout/429·응답 크기·schema 검사·표준 변환·local transaction·outbox·retry, L2 단계 조합, L3 허용 provider와 업무 상태 정책. 제품에는 이 외부 broker가 없다. PoC outbound webhook의 서명·URL 검사는 일부 재사용 후보다.

### 5. 확장 필요 부분

L4는 carrier별 payload mapping, OAuth handshake/refresh adapter, 고유 checksum·규격 변환. 표준 retry/timeout/SSRF 방어를 provider마다 새로 쓰지 않는 것이 목표다.

### 6. 남는 애플리케이션 코드

provider 설정·connection 수명, 외부 status→업무 status 의미, 충돌/삭제 동기화 규칙, 예외적인 외부 프로토콜 adapter.

### 7. 보안·트랜잭션 및 실패 경계

외부 응답 검증 전 DB에 저장하지 않는다. malformed response, timeout, 429, refresh token 경합, retry 중 최신 shipment 변경, commit 뒤 event 발행 실패를 구분한다. 외부 호출 동안 DB write lock을 잡는 기본 구현은 피한다. **이번 검증은 PoC outbound 실패/서명/중복·주소 검사까지이며 OAuth·carrier 실제 서비스 호출은 미실행**이다.

### 8. 제거한 중복과 제거 후보

현재 제품은 값/계약 검사와 DB 저장 원자성을 재사용한다. 외부 연동의 HTTP wrapper·retry·secret 로딩·상태 API 제거는 아직 가설이다. adapter만 작성하면 안전하다는 보장은 broker 구현 후에 검증한다.

## E — 실시간 협업

### 1. 기존 구성과 현재 근거

mutation Service, change listener/CDC, WS/SSE gateway, subscriber registry, 사용자별 ACL, reconnect cursor/snapshot, 클라이언트 동기화 코드가 필요하다. 제품 캐시 TTL는 realtime 전달 계약이 아니다. PoC는 LISTEN/NOTIFY 뒤 구독자 권한으로 전체 재조회를 실행한다.

### 2. 프론트 표현

**가상 제안, 미구현 제품 구독 요청**:

```json
{"capability":"data.subscribe","input":{"query":{"read":"Document","select":["id","title"],"where":{"workspace":"ws-2"}},"resume":"previous-or-null"}}
```

`where`의 정확한 문법을 포함해 전체 예시는 제안이다. 현재 제품 read 문법이 위 구조를 그대로 받는다는 뜻이 아니다. 표준 공개 조회를 구독 의도로 재사용하여 화면별 구독 endpoint를 만들지 않는 방향을 보여준다.

### 3. 서버 Specification

**가상 제안, 미구현 설명용 정책**:

```yaml
spec: collaborativeDocuments
query: existingPublicReadContract
deliveryAuthorization: reevaluateForSubscriber
recovery: snapshotOnGapOrExpiredResume
limits: { connections: serverBound, subscriptions: serverBound, queuedBytes: serverBound }
revocation: stopAndClearClientScope
ordering: perDocumentVersionWhenAvailable
```

### 4. Runtime 표준 책임

L1 연결·재인증·bounds·변경 dependency·전달시 가시성·gap 표시·snapshot 복구, L2 공개 조회/구독 선택, L3 문서 공유 정책. DB changes와 일반 message stream을 같은 snapshot 모델로 억지 통합하지 않는다.

### 5. 확장 필요 부분

L4는 CRDT/OT 편집 알고리즘, 특수 협업 merge, 외부 CDC adapter다. 연결 상태와 인증·fanout glue는 표준 후보지만 CRDT 전체를 새 AIP DSL로 만들 이유는 없다.

### 6. 남는 애플리케이션 코드

문서 권한·공유·버전 의미, 편집 충돌 해결, UI 상태 동기화. 데이터 조회 구독이 모든 공동 편집 의미를 대신하지 않는다.

### 7. 보안·트랜잭션 및 실패 경계

commit된 변경만 알리고 전달 직전 권한을 다시 평가한다. 연결 종료·reconnect·권한 회수·해제 뒤 늦은 이벤트·slow consumer·이벤트 유실을 검증한다. PoC tests는 인증·가시성·영향 있는 테이블 wakeup·연결 상한을 재실행했다. **durable resume cursor/replay와 실제 편집 merge는 미실행**이며 gap 없는 전달로 주장하지 않는다.

### 8. 제거한 중복과 제거 후보

PoC는 gateway에서 매번 같은 인가/query 실행 코드를 재사용했다. 제품에는 아직 연결되지 않았으므로 제품 realtime endpoint·reconnect boilerplate 제거는 제안이다. frontend 편집 상태 코드까지 제거했다고 주장하지 않는다.

## F — 업무 SQL 없는 파일·계산 Job

### 1. 기존 구성과 현재 근거

upload Controller, MIME/크기 검사, object adapter, converter/외부 계산 worker, queue producer, progress/status/cancel endpoints, 결과 접근/expiry cleanup가 필요하다. InvenTree export는 작업 안에서 user/plugin/output와 request를 다시 구성한다. AIP T01은 **텍스트 계산만** 실제 실행했으며 파일 변환 engine은 실행하지 않았다.

### 2. 프론트 표현

**실제 제품 transport 요청**, T01에서 Node/Python 모두 검증:

```json
{"extension":"Compute.count","input":{"text":"가😀A"}}
```

생성 binding이 있을 때 현재 SDK 표현은 `client.extension("Compute.count", {text: "가😀A"})`다. T01 자체는 생성 SDK 실행이 아니라 V5 계약 지문+V6 HTTP로 호출했다.

**가상 제안, 미구현 file Job**:

```json
{"capability":"document.convert","input":{"file":"authorized-file-handle","format":"pdf"},"key":"convert-file-version1-pdf"}
```

### 3. 서버 Specification

**실제 제품 문법**, T01 전체 정의:

```aip
actor Compute
resource Compute {
  fields { id: Id }
  extension read count {
    input { text: Text }
    output { length: Int }
    effect none
    deadline 9s
    implementation "compute.count"
  }
}
```

이 정의는 현재 계산을 붙이려고 actor/resource까지 선언해야 하는 제약을 드러내는 실험이다. 운영 사용자 모델을 Compute로 바꾸라는 제안이 아니다. ctx access 없이 계산할 수 있지만 transport는 DB clock을 읽고, 제품 서비스는 추가로 운영 principal·배포 fence가 필요하다.

**가상 제안, 미구현 managed Job 계약**:

```yaml
capability: document.convert
input: { file: AuthorizedFileHandle, format: AllowedFormat }
execution: durable
worker: registeredConverter
fileScope: [readInput, writeOwnedResult]
limits: { bytes: bounded, pages: bounded, cpu: bounded, elapsed: bounded }
result: { type: AuthorizedFileHandle, expires: configuredRetention }
cancellation: cooperativeWithCleanup
```

### 4. Runtime 표준 책임

L1 파일 handle·입출력·queue acceptance·lease/progress/cancel/result·expiry·권한·비용, L2 표준 변환 조합, L3 허용 변환/파일 정책. durable 상태저장소는 필요하지만 업무 resource/비즈니스 SQL은 필요하지 않은 모델을 검증해야 한다.

### 5. 확장 필요 부분

L4는 문서 converter·image codec·특수 외부 계산이다. converter에 queue/status/멱등/tenant/결과 만료를 다시 구현하게 하면 표준화가 부족하다.

### 6. 남는 애플리케이션 코드

변환 엔진 adapter·허용 format·조직별 보관/요금 정책. 특별한 문서 의미/레이아웃 보정은 확장에 남는다.

### 7. 보안·트랜잭션 및 실패 경계

입력 파일 권한과 quota 예약 후 Job 수락을 영속화한다. 결과 저장과 완료 표시의 crash 경계, 중복 제출, worker kill, 재시도 중 취소, 취소 후 임시 파일 cleanup, 다른 tenant 결과 요청을 검증해야 한다. 현재 실험은 순수 계산·DB 연결 실패·선언 deadline까지다. **장기 file Job·변환 codec·외부 계산·진행·취소 전체는 미구현/미실행**이다.

### 8. 제거한 중복과 제거 후보

현재 확장은 수기 HTTP Controller/입출력 validator 없이 로컬 계산을 호출한다. 가짜 resource와 PG 의존도 함께 남는다. 목표는 queue/status/cancel/file lifecycle glue를 공통화하는 것이며, 파일 처리 알고리즘과 durable 저장소를 없애는 것은 아니다.
