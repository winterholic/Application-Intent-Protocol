# 선언형 실행 모델 고도화 제안

> 후속(2026-10-10): 이 문서의 감사 기준은 `ecf3369` 시점이다. 이후 [resource 독립 동기 operation](../../product/EXECUTION.md)을 제품에 연결했고 [durable Job 구조 제안](durable-job-proposal.md)을 구체화했다. 아래 역사적 미지원/제안 항목을 현재 구현으로 합산하지 않으며 최신 지원 범위는 후속 문서를 따른다.


> **제안 문서. 신규 문법·API·실행 보장을 구현하거나 승인받은 결과가 아니다.** 기존 read/apply/extension과 제품의 보안·멱등·배포 계약을 유지하면서 필요한 경계를 추가하는 방향이다. [SQL 편향 감사](sql-bias-audit.md)와 [시나리오](scenarios.md)의 반례가 출발점이다.

## 설계 관문 8개

1. 원칙1·2·3·4·6·7: SQL 밖 반복 glue를 Runtime으로 옮기고 caller 조합을 유지한다. 공통 계약이 커져 유지보수가 증가할 수 있으므로 시나리오별 제거한 구현과 새 spec을 함께 측정한다.
2. 표준 동작·파일 handle·작업 수명·외부 연동의 표현 범위를 넓힌다. 임의 코드/URL/secret/권한/재시도 정책은 caller가 정의하지 않는다.
3. endpoint/DTO/queue glue는 감소 후보지만 업무 정책·provider 설정은 서버에 남는다. 화면마다 operation spec을 새로 만들게 되면 설계 실패다.
4. 서버는 actor/tenant/권한·계약 버전·입출력·효과 의존성·예산을 검증한다. caller의 capability 이름/입력/낮은 예산 요청은 전부 비신뢰 데이터다.
5. Rust는 검증·실행 경계를, JS/TS·Python은 typed adapter/특수 계산을 맡는다. 기존 두 worker의 계약 검사를 재사용하고 플랫폼 격리는 별도 증명한다.
6. L1→L2→L3→L4 선택 기준을 catalog에 제공한다. 기존 표준 read/apply로 가능한 일을 named operation으로 다시 선언하지 않는다.
7. 새 범용 IR이나 만능 workflow interpreter를 선행 도입하지 않는다. 공통 의미에 필요한 최소 계약만 정의하고 전문 executor의 의미는 숨기지 않는다.
8. 최종 작성 형식·W0/W1/W2·Id wire·DB 다중 지원·최종 라이선스·구버전 공존은 기존 Open을 유지한다. 대규모 구조 변경은 명시 승인 전 착수하지 않는다.

## Level과 실행 축

Level은 코드가 어디에 있어야 반복을 줄이는가를 나타낸다. 효과·수명·권한·저장소와 다른 축이다.

| 축 | 예 | 검증 대상 |
|---|---|---|
| 표준화 위치 | L1 primitive / L2 composition / L3 reusable spec / L4 extension | 반복 제거와 업무 고유성 |
| 데이터 의존 | 없음 / resource read / resource write | 정책·가시성·배포 fence·transaction |
| 부작용 | pure / managed external / DB / file / 조합 | 재시도 가능성·멱등·보상 |
| 실행 수명 | sync / durable / subscription·stream | deadline·lease·재개·연결 해제 |
| 보안 의존 | IdP·principal / tenant / secret / object / provider | 각 경계에서 actor·scope 재검사 |

`effect none`은 오늘 제품에서 “DB 쓰기 없음”에 가깝다. aggregate 접근이 있으므로 pure/DB 독립을 뜻하지 않는다. 위 분류를 그대로 새 enum으로 확정하지 않는다. contract validator가 필요한 dependency 집합을 산출할 수 있는지를 먼저 실험한다.

## 공통 계약에서 공유할 최소 의미

| 항목 | 제안 의미 | caller가 결정하는 범위 |
|---|---|---|
| capability와 계약 버전 | 서버의 공개 등록 기능·입출력·권한·provider 가용성 | 허용 기능 선택과 예상 계약 지문 |
| 입력·출력 | schema 하나에서 서버 검사·SDK 생성 | typed 값·허용된 필드/변환/동작 조합 |
| principal/tenant | 인증 provider와 정책에서 결정·검증 | tenant 선택도 멤버십 검증 후에만 |
| dependency | DB read/write, secret, file, network, clock, run store 의존의 검증 결과 | 원시 권한·provider 주소 지정 불가 |
| 예산 | server/spec 상한과 caller 요청의 최소값 | 더 낮은 시간/행/항목/파일 상한만 |
| 결과 | 확정 값 / durable 수락 handle / 계약화 실패 | 같은 결과 계약으로 UI 분기 |
| 실패 | 거부·검증·업무 실패·retryable·결과 불명·취소 요청·취소 확인 구분 | retryable이라는 이유로 원 요청 변경 불가 |
| 멱등성 | principal·tenant·contract·입력 digest·요청 key scope | 안정 key와 원 요청 유지 |
| 관측성 | request/run/effect 상관 ID, 민감정보 제거한 audit/metrics/trace | 비밀·운영 정보 임의 조회 불가 |

operation은 이 공통 계약의 설명용 용어다. 새 최상위 키워드나 endpoint 이름을 확정하지 않는다. 기존 envelope를 호환 확장할지, 별도 실행 계약으로 둘지는 타입·버전·복구 실험으로 비교한다.

## caller·서버·확장의 경계

| 기능 | caller 조합 | 서버 한 번 선언 | 확장에 남는 것 |
|---|---|---|---|
| 조회/필터/정렬·단순 변경 | 기존 공개 범위 내 자유 조합 | expose/행·필드/불변조건 | 특수 집계·알고리즘 |
| 표준 변환/파일/Job | 서버 catalog가 공개한 primitive와 bounded 조합 | 허용 입력·결과 접근·예산·provider | codec/converter/특수 계산 |
| 주문 결제/다단계 승인/초대 | 공개 동작·입력·허용 후속 조회 | 업무 불변조건·역할·전이·보상·기한 | 고유 가격 산식/승인자 계산/provider mapping |
| HTTP/mail/SMS/push | 등록 서비스·template·typed input | host/method/secret/scopes/수신자·요금 한도 | provider adapter·특수 payload |
| realtime | 공개 조회·알림 구독 선택 | subscriber 정책·회수·recovery 계약 | 특수 stream/CDC adapter |

L3는 앱마다 불필요한 named API를 다시 만드는 계층이 아니다. 표준 동작 자체는 L1/L2로 공개하고, caller가 정의할 수 없는 정책과 다단계 불변조건만 L3로 묶는다. controller/DTO는 계약에서 파생하며 별도 수기 구현을 요구하지 않는 것이 평가 관문이다.

## 전문 executor와 경계

1. **DB 실행기**: 기존 V2/V3 planner와 transaction·불변조건 검사를 유지한다. 이미 검증한 가시성/잠금/fence 의미를 그대로 사용한다.
2. **동기 worker**: 순수 계산/제한된 ctx를 유지한다. 데이터 접근 없는 호출도 인증·폐기·배포 계약 의존은 따로 검사한다. 임의로 fence를 제거하지 않는다.
3. **durable 실행기**: acceptance를 영속화한 뒤 handle 반환. queue/lease/heartbeat/checkpoint/attempt/result expiry를 관리한다. PG나 다른 run store 선택은 운영 결정이며 업무 테이블의 존재와 분리한다.
4. **효과 전달기**: local transaction에서 outbox 의도를 기록하고 commit 뒤 전달. inbox/dedupe/외부 key/재조회/재시도를 관리한다. “delivery 시도”와 “provider 효과 완료”를 분리한다.
5. **파일·stream 실행기**: file handle 소유권/tenant/크기/형식/저장소/cleanup를 관리한다. codec/converter는 제한된 adapter이고 모든 파일 처리 알고리즘을 Rust DSL로 만들지 않는다.
6. **realtime 실행기**: 구독 비용/전달시 권한/재인증/slow consumer/reconnect snapshot·gap을 관리한다. DB change query와 일반 알림 stream의 전문 의미는 따로 둔다.

현재 MacNetDeny만으로 Linux 격리나 파일 접근 제어를 보장할 수 없다. 외부 효과는 worker 네트워크를 전면 개방하기보다 Rust broker를 통해 서버 allowlist·secret·예산을 적용하는 실험을 우선한다. 확장 코드가 네트워크를 직접 써야 하는 provider는 별도 신뢰·격리 등급과 동일 계약 검증이 필요하며 기본값으로 열지 않는다.

## 트랜잭션·재시도·취소

| 상황 | 제안되는 보장 | 보장하지 않는 것 |
|---|---|---|
| DB 상태 변경 + event/job 제출 | 같은 DB를 쓸 때 local tx + durable outbox/acceptance의 atomicity 검증 | 외부 결제·메일까지 rollback |
| 외부 timeout | unknown 효과로 분류, provider key로 조회/멱등 재시도 | timeout이면 결제되지 않았다는 결론 |
| retry | 효과 단계별 안정 key, attempt·총 기한·backoff·jitter·재시도 불가 오류 | 모든 예외를 자동 반복 |
| cancel | 요청 영속화→worker 협력 신호→중단/정리 확인 | 이미 실행된 외부 효과 회수 |
| 보상 | 도메인 spec의 별도 멱등 동작, 재시도·운영 복구 상태 | 일반 DB rollback과 같은 원자성 |
| queue redelivery | lease generation/fencing와 inbox/dedupe로 중복 영향 제한 | 모든 외부 시스템에서 exactly-once |
| 배포 중 run | 저장한 계약 버전/step·adapter 버전과 호환 정책 검증 | 최신 정의로 과거 작업을 무조건 재해석 |

DB transaction 안에서 외부 I/O를 기다리는 기본 조합은 피한다. “transaction 전에 결제”도 자동 정답은 아니다. 예약→외부 호출→확정/보상처럼 도메인이 요구하는 순서와 실패 계약을 L3가 정의해야 한다. 일반적인 외부 사전조회까지 일률 금지할 필요는 없으므로 DB/외부 조합 자체보다 잠금·효과·복구 경계를 검사한다.

## AI First 적용

Catalog는 단순 이름 목록이 아니라 실제 available/disabled, input/output, 허용 조합, 효과·수명, 필수 정책, 예산, 실패·복구, 현재 계약 지문, 선택적 설명을 제공한다. 내부 host/secret/SQL/운영 정보는 공개 목록에서 분리한다. 목록에 보인다는 사실이 호출 권한이 되지는 않는다.

AI의 선택 순서는 “기존 표준 → 표준 bounded 조합 → 재사용 서버 업무 spec → 특수 extension”이다. 여러 형식/쓰기 모델의 최종 선택을 이 감사에서 확정하지 않는다. 정적 분석은 선언 간 의존·효과·입출력·비용·provider 누락을 검사하고, 문법적 유사성을 같은 업무로 단정하는 중복 탐지기는 선택 기능으로 둔다. 설명 metadata가 없거나 AI가 잘못 해석해도 필수 검사는 동일하다.

## 구현 전에 통과할 실험 관문

- **순수 작업**: business SQL 0, resource table 추가 0인 같은 JS/Python 계산. 인증/폐기/계약 mismatch/예산 위반이 모두 거부되는지 확인. PG 없는 배포 자체가 필요하면 저장소 독립 인가 의미까지 검증한다.
- **durable 파일 작업**: HTTP 연결 해제 뒤 진행, worker crash 재개, 중복 제출, 취소와 완료 경합, 만료 결과 접근, 다른 tenant 거부. 업무 SQL 없이도 유용해야 한다.
- **주문/외부 효과**: commit 직전·직후 crash, provider 성공 후 ack 유실, 429/timeout, 중복 webhook, 보상 실패. 같은 재고/금액 불변조건을 유지한다.
- **realtime**: 권한 회수·연결 중단·유실·구독 해제·재연결 snapshot. 순서/보관 보장을 지원하지 않으면 gap을 명시한다.
- **중복 제거**: 전통 구현과 AIP에서 수기 파일·계약·실패분기·반복 glue를 동일 요구사항으로 비교. 단순 LOC나 선언 수만으로 생산성 향상을 확정하지 않는다.
