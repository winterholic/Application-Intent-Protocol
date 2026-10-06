# 04. Safety Matrix: 무엇을 언제 보장하는가

> 상태: Proposed · 과장하지 않기 위한 문서. `aip verify`의 출력과 계약(contract)의 `guarantees` 필드는 이 표에서 생성된다.

## 1. 보장 등급

| 등급 | 뜻 | 실패 시점 |
|---|---|---|
| **S** Static Error | 컴파일되지 않는다 | `aip check` |
| **W** Static Warning | 컴파일되지만 경고. `deny` 설정으로 오류 승격 가능 | `aip check` |
| **B** Boot Validation | 서버가 뜨지 않는다 (DB 스키마, capability, 비밀값, extension 버전) | `aip run` 시작 |
| **R** Runtime Enforcement | 매 요청에서 강제된다 (DB 제약, 정책 SQL, 입력 검증) | 요청 처리 중 |
| **X** Cannot Guarantee | AIP가 보장하지 않으며 계약에 그 사실을 적는다 | 해당 없음 |

한 항목이 여러 등급에 걸칠 수 있다. 예를 들어 "관리자 1명"은 정적으로 보존 증명을 시도(S/W)하고, 증명 못 한 명령은 커밋 시 검사(R)로 막는다.

## 2. 매트릭스

| # | 문제 | S | W | B | R | X | 비고 |
|---|---|---|---|---|---|---|---|
| 1 | 타입 불일치, 모르는 필드/이름 | ● | | | | | |
| 2 | Intent에 권한 선언 없음 | ● | | | | | `allow` 필수 |
| 3 | 불가능한 정책 (항상 false) | | ● | | | | 술어 단순화로 탐지. SMT 도입은 미결정 |
| 4 | 항상 참인 정책이 명령에 붙음 (`allow public` 아닌데 사실상 공개) | | ● | | | | |
| 5 | 자원과 무관한 권한 검사 (IDOR) | ● | | | | | 엔티티 파라미터의 권한은 그 엔티티에서 계산. "다른 id로 권한 확인" 표현 불가 |
| 6 | 같은 자원의 정책 이중 구현 | ● | | | | | `visible to`는 엔티티당 하나 |
| 7 | 권한 관계를 바꾸는 명령의 자기 상승/관리자 소멸 | | ● | | ● | | authority graph 분석 + 불변식 런타임 검사 |
| 8 | 존재 여부 노출 (404 vs 403) | | | | ● | | 기본 NOT_FOUND 통일. `disclose existence`로 명시 해제 |
| 9 | 관계 경유 암묵 로딩 (N+1) | ● | | | | | 모든 읽기는 계획된 조회. 루프 없음 |
| 10 | 비싼 조회 계획 (집계 정렬, 거대 문맥 조회) | | ● | | | | `aip plan` 비용 추정 |
| 11 | 인덱스 없는 필터/정렬 | | ● | ● | | | 시작 시 실제 인덱스와 대조 |
| 12 | 무제한 목록/입력 | ● | | | | | `Set/List max`, `page` 필수 |
| 13 | 상태 전이 위반 | ● | | | ● | | 선언과 다른 전이는 S, 실제 현재 상태는 R |
| 14 | 도달 불가 상태/죽은 전이 | | ● | | | | |
| 15 | unique/at-most-one 경쟁 | | | | ● | | DB 제약. 경쟁 조건 없음 |
| 16 | at-least-one (관리자 0명) | | ● | | ● | | 보존 증명 실패 시 경고, 커밋 시 지연 검사 |
| 17 | 기간 겹침 | | | ● | ● | | EXCLUDE capability를 부팅 시 확인 |
| 18 | check-then-act 경쟁 | ● | | | | | 컴파일러가 잠금 집합을 계산하므로 사람이 잠금을 빠뜨릴 수 없음. 잠금 없이 검사만 하는 표현 불가 |
| 19 | 명령 간 교착 | | | | ● | ● | 명령 내 잠금 순서는 전역 정렬로 보장. DB 내부(인덱스 페이지 등) 교착은 보장 불가, 재시도로 흡수 |
| 20 | 직렬화 실패 자동 재시도의 안전성 | ● | | | | | retry_safety 판정. 커밋 전 비멱등 외부 효과가 있으면 자동 재시도 금지 |
| 21 | 커밋 전 비가역 외부 효과 | ● | | | | | E-EFFECT-ORDER |
| 22 | 재시도되는 비멱등 효과 | ● | | | | | E-EFFECT-RETRY-UNSAFE. 멱등 키 또는 lookup 필수 |
| 23 | reservable 효과의 보상 누락 | ● | | | | | 서명에 compensation 필수 |
| 24 | deferred 효과 영구 실패 시 도메인 처리 없음 | | ● | | | | W-EFFECT-NO-FAILURE-PATH |
| 25 | 이벤트 중복 전달 | | | | | ● | at-least-once. 계약에 명시, AIP 소비자는 자동 멱등 처리 |
| 26 | 이벤트 전역 순서 | | | | | ● | key 단위 순서만 약속 |
| 27 | 이벤트 스키마 불일치 | ● | | | | | 같은 이벤트 한 형태 |
| 28 | 캐시 무효화 누락 | ● | | | | | 읽기/쓰기 집합에서 자동 도출 |
| 29 | 무효화 후 오래된 값 재적재 | | | | ● | | 버전 스탬프 |
| 30 | 캐시 신선도 (TTL 내 stale 읽기) | | | | | ● | 선언한 TTL만큼의 stale은 허용된 계약 |
| 31 | 사용자별 결과의 공용 캐시 | ● | | | | | E-CACHE-PERSONALIZED |
| 32 | 개인정보 참조의 삭제 정책 누락 | ● | | | | | E-ERASE-INCOMPLETE |
| 33 | 외부 시스템에 남은 개인정보 (PG, 메일 로그) | | | | | ● | 외부 시스템 정책 영역. extension이 `holds personal data` 선언 시 경고만 |
| 34 | 지원되지 않는 DB capability | ● | | ● | | | 선언된 adapter 기준 S, 실제 서버 버전/확장 설치는 B |
| 35 | 스키마 드리프트 (DB가 정의와 다름) | | | ● | | | 부팅 시 비교, 불일치면 기동 거부 |
| 36 | 파괴적 마이그레이션 | ● | | | | | 컬럼 삭제/타입 축소는 명시 승인(`migration allow drop`) 없이는 오류 |
| 37 | 클라이언트/서버 계약 불일치 | | | ● | ● | | 클라이언트가 계약 해시를 보냄. 호환 불가면 명확한 오류 |
| 38 | 배포 간 breaking change | | ● | | | | `aip diff --against deployed` |
| 39 | 입력 검증 (타입, 범위, 형식, 크기) | | | | ● | | 계약에서 파생. 수동 검증 코드 없음 |
| 40 | 출력 데이터 과다 노출 | ● | | | | | 선택되지 않은 필드는 직렬화 경로 자체가 없음 |
| 41 | 비밀값 누락 | | | ● | | | extension이 요구하는 비밀을 부팅 시 확인 |
| 42 | 스케줄 중복 실행 (다중 인스턴스) | | | | ● | | Postgres advisory lock 리더 |
| 43 | 비즈니스 규칙 자체의 오류 (잘못된 요구사항) | | | | | ● | AIP는 선언된 규칙을 지킬 뿐, 규칙이 옳은지는 모른다 |
| 44 | escape hatch 함수의 논리 오류 | | | | | ● | 순수성과 자원 한도만 보장 |
| 45 | 외부 시스템의 서명 위반 (멱등이라더니 아님) | | | | | ● | extension conformance 테스트가 줄이지만 보장은 못 함 |
| 46 | 같은 식을 비교 양변에 사용 (자기 비교) | | ● | | | | W-SELF-COMPARE. 아리아리 `isHigherRoleTypeThan(m, m)` 유형 |
| 47 | 참조 경로 불일치 (대댓글 부모가 다른 글) | | | | ● | | 복합 FK |
| 48 | 커밋 전 외부 객체 삭제, 고아 객체 | ● | | | | | 필드 소유권 + deferred 정리 |
| 49 | 전역 관리자 우회의 불일치 적용 | ● | | | | | `superuser` 선언은 하나, 전 intent에 일관 적용 |
| 50 | 동적 스키마 기준 미고정 | ● | | | | | E-DYNSCHEMA-UNPINNED |

## 3. `aip verify` 출력 예시 (목표 형태)

```
CancelOrder
  authorization      static    allow: order.customer = actor or actor.role = ADMIN
  existence          runtime   hidden (NOT_FOUND for unauthorized)
  state transition   static    Order.status: PENDING|PAID|PREPARING -> CANCELLED (lifecycle ok)
  invariants         runtime   Product.stock >= 0 (CHECK), Coupon.status lifecycle
  locks              static    order(1) -> coupon(0..1) -> product(n, id order)
  effects            static    refund: deferred, keyed(order.id)   OrderCancelled: outbox -> kafka "orders"
  retry              static    client-retry-with-key (auto retry: yes, no pre-commit effects)
  cache              static    invalidates MyOrders[actor=order.customer], OrderDetail[order]
  not guaranteed               refund permanent failure handling (W-EFFECT-NO-FAILURE-PATH)
```

이 보고서의 요약본이 계약의 `guarantees`로 클라이언트와 에이전트에게 전달된다. 클라이언트 개발자와 LLM은 "이 명령은 재시도해도 되는가", "이 이벤트는 중복될 수 있는가"를 코드가 아니라 계약에서 읽는다.
