# 05. 실행 모델

> 상태: Proposed · 렌즈: dev-distributed-systems(타임아웃은 "모름", 2PC 회피, exactly-once 신화), dev-event-driven(아웃박스, 멱등 소비, key 단위 순서), dev-auth(인가는 매 요청)
> 기준 인프라: PostgreSQL 단일 primary. 복제본 읽기는 M5 이후.

## 1. 원칙

1. **분산하지 않을 수 있으면 분산하지 않는다.** 모든 내구성 상태(outbox, saga, 타이머, 멱등 기록, 스케줄 리더)는 애플리케이션과 같은 PostgreSQL에 둔다. 별도 워크플로 엔진, 별도 코디네이터가 없다. 그러면 "상태 기록"과 "비즈니스 변경"이 한 트랜잭션에 들어간다.
2. **타임아웃은 실패가 아니라 모름이다.** 외부 효과의 결과를 모르면 lookup(서명에 있을 때) 또는 멱등 키 재시도만 한다. 맹목 재시도는 없다.
3. **exactly-once 전달은 약속하지 않는다.** at-least-once 전달 + 멱등 처리로 "정확히 한 번 처리"를 만든다. 계약에 그렇게 적는다.
4. **2PC는 쓰지 않는다.** 여러 시스템에 걸친 원자성은 예약(reservable) + 보상(saga) + 멱등 재시도로 수렴시킨다.

## 2. Query 실행

```
1 Admission     Intent 레지스트리 조회 → 입력 디코드/검증 → 인증(actor 확정) → rate limit
2 Cache         cached면 키 계산(actor-class 포함) → hit면 반환
3 Plan select   capability와 선택 모양으로 전략 선택
4 Execute       REPEATABLE READ READ ONLY 스냅샷 1개
5 Observe       observational 효과(조회수 등) 비동기 발행, 실패해도 응답에 영향 없음
6 Respond       선택된 필드만 직렬화
```

**조회 전략** (planner가 선택, `aip explain --sql`로 확인)

| 전략 | 쓰는 경우 | 왕복 수 |
|---|---|---|
| 단일 SQL + `LATERAL` + `json_agg` | 선택 깊이 3 이하, 중첩 목록 상한이 선언됨 | 1 |
| 관계 edge별 배치 (`= ANY($ids)`) | 깊거나 넓은 선택, 큰 중첩 목록 | edge 수 + 1 |
| 파생 필드 lateral 서브쿼리 | `count`, `exists`, `latest` | 위에 포함 |

spike-0은 배치만 구현했다. PostgreSQL은 JSON 집계로 한 번에 트리를 만들 수 있으므로 기본 전략을 단일 SQL로 두고, 비용 모델이 배치를 고르면 전환한다. 둘 다 왕복 수가 행 수와 무관하다.

권한과 가시성: `allow`의 행 의존 부분과 모든 관련 엔티티의 `visible to`가 SQL 조건으로 들어간다. 중첩 선택의 각 edge에도 해당 엔티티의 가시성이 적용된다.

## 3. Command 실행

```
Phase 0  Admission
         Intent 조회, 입력 검증, 인증, rate limit
         idempotency 키가 있으면 선점 INSERT (동일 키 동시 요청은 여기서 직렬화)
         → 이미 완료된 키면 저장된 응답 반환

Phase 1  Pre-commit effects   (계획에 있을 때만)
         staged: 스테이징 업로드
         reservable: 예약 실행, saga 레코드에 "reserved" 기록 (별도 짧은 트랜잭션)
         타임아웃 → lookup으로 판정, 판정 불가면 명령 실패 + 보상 예약

Phase 2  DB transaction (기본 READ COMMITTED)
         2.1 잠금       lock_set을 (테이블 순위, id) 전역 정렬로 SELECT ... FOR UPDATE
         2.2 문맥 조회   read_set 전체 + 권한 재료 + 사전조건 재료를 한 SQL로
         2.3 권한        정책 평가. 불통과면 NOT_FOUND 또는 FORBIDDEN (disclose 설정)
         2.4 사전조건     require, lifecycle 가드
         2.5 변경        집합 연산 SQL들 (RETURNING으로 바인딩 갱신)
         2.6 불변식      즉시 제약은 문장마다, 지연 제약은 COMMIT 시
         2.7 outbox     이벤트, deferred 효과, 캐시 무효화, reservable confirm, saga 완료 표시
         2.8 멱등 기록    응답 저장
         COMMIT
         실패 → ROLLBACK, Phase 1 예약이 있으면 보상 실행 요청(saga)

Phase 3  Post-commit (비동기, dispatcher)
         outbox 행을 가져와 효과 실행. 멱등 키로 재시도, 지수 백오프, 한도 초과 시 dead letter
         이벤트는 broker extension(kafka 등) 또는 내부 구독자로 전달

Phase 4  Response
         returns 선택을 커밋된 상태로 읽어 응답
```

### 3.1 잠금 순서

- 각 엔티티 테이블에 전역 순위를 부여한다(스키마 정의 순서가 아니라 참조 그래프의 위상 정렬 + 이름, 결정적).
- 한 명령의 모든 행 잠금은 (순위, id) 오름차순으로 한 번에 획득한다.
- 집합 UPDATE 대상도 먼저 같은 순서로 잠근다.
- 결과: AIP 명령끼리의 행 잠금 순환 대기는 생기지 않는다. 인덱스 페이지나 외래키 검사로 인한 DB 내부 교착은 여전히 가능하므로(보장 불가 항목 19) 재시도 정책으로 흡수한다.

### 3.2 자동 재시도

| retry_safety | 조건 | 직렬화 실패/교착 시 |
|---|---|---|
| AutoRetryable | Phase 1 효과 없음 | 런타임이 같은 요청을 새 트랜잭션으로 최대 N회 재실행 |
| ClientRetryWithKey | Phase 1 효과가 전부 멱등 키 보유 | 409 + `retryable: true`, 클라이언트는 같은 키로 재요청 |
| NotRetryable | 그 외 (컴파일 경고 대상) | 409 + `retryable: false` |

### 3.3 격리 수준 선택

기본은 READ COMMITTED + 명시 잠금이다. 컴파일러가 잠금이나 DB 제약으로 강제할 수 없는 불변식(예: 여러 테이블에 걸친 합계 조건)을 만나면 그 명령만 SERIALIZABLE로 올리고, `aip verify`에 이유를 적는다.

## 4. Saga (다단계 외부 효과)

별도 선언 없이, 명령에 Phase 1 효과가 있으면 컴파일러가 saga를 만든다.

```
_aip_saga(id, intent, idempotency_key, state, steps jsonb, updated_at)
state: STARTED → RESERVED(step k) → COMMITTED → CONFIRMED
                         └→ COMPENSATING → COMPENSATED
```

- 각 전이는 짧은 로컬 트랜잭션으로 기록한다.
- 복구 루프: 시작 시와 주기적으로 오래된 STARTED/RESERVED/COMPENSATING saga를 찾는다. 결과를 모르는 단계는 lookup으로 판정하고, 판정 결과에 따라 확정 또는 보상한다.
- 보상 불가능한 효과(irreversible)는 saga의 **마지막 단계**로만 올 수 있다. 컴파일러가 순서를 강제한다.

장기 실행 흐름(결제 후 웹훅 대기, 승인 대기)은 명령 하나가 아니라 **명령 + 이벤트 구독 + 타이머**의 조합으로 표현한다. 전용 `workflow` 구문 도입은 Reality Test에서 필요성이 증명될 때까지 보류한다.

## 5. Outbox와 이벤트

```
_aip_outbox(id bigserial, kind, target, key, payload jsonb, available_at, attempts, last_error, done_at)
```

- kind: `event`, `effect`, `cache_invalidate`, `confirm`, `compensate`.
- dispatcher는 `FOR UPDATE SKIP LOCKED`로 가져와 여러 인스턴스가 나눠 처리한다.
- 같은 key의 이벤트는 id 순서로 처리한다(key 단위 순서). 다른 key 간 순서는 약속하지 않는다.
- AIP 내부 구독자(`on Event`)는 `_aip_processed(subscriber, event_id)`로 멱등 처리한다. 사람이 멱등 코드를 쓰지 않는다.
- 외부 브로커(kafka extension)로 나가는 이벤트는 envelope에 `event_id, type, version, key, occurred_at`을 담는다. 외부 소비자 계약: 중복 가능, key 단위 순서.

## 6. 캐시

- 키 = intent + 입력 해시 + actor-class(선언된 분할 기준) + 계약 버전.
- 값에 **버전 스탬프**(관련 엔티티의 변경 카운터)를 함께 저장한다. 무효화는 카운터 증가 후 키 삭제. 읽기 경로는 캐시에 적재하기 전 카운터를 확인해, 무효화와 경쟁한 오래된 결과를 적재하지 않는다.
- 무효화 대상은 `QueryFacts.invalidated_by`에서 컴파일 시 도출된다.

## 7. 시간

- 타이머: `_aip_timer(id, due_at, kind, payload, claimed_by, done_at)`. 예약 만료, 보존 기한, 지연 알림.
- 스케줄: cron 식 + 타임존. 실행 권한은 `pg_try_advisory_lock(schedule_id)` 보유 인스턴스만. 실행 기록으로 놓친 실행을 판정.
- 시간 값은 전부 DB 시계(`now()`) 기준. 여러 서버의 로컬 시계로 순서를 판정하지 않는다.

## 8. 인증과 신원

- AIP 런타임은 신원을 **확인**할 뿐 발급 로직을 직접 구현하지 않는다(dev-auth의 직접 구현 금지선). `auth-oidc` extension이 카카오/구글 등 OIDC 로그인, 세션 또는 짧은 access token + 서버 저장 refresh token 회전을 제공한다.
- 인가는 매 요청 DB 상태 기준으로 평가한다. 토큰의 역할 클레임을 믿지 않는다(권한 변경 즉시 반영).
- spike-0의 `x-aip-actor` 헤더 방식은 개발 모드 전용이며, 프로덕션 설정에서 켜면 부팅이 거부된다(B 등급).

## 9. 관측성

런타임은 전체 의미를 알기 때문에 사람이 계측 코드를 넣지 않아도 다음을 기본 제공한다(OpenTelemetry).

- span: intent → phase → SQL 문 / 효과 호출 / outbox 처리
- metrics: intent별 지연, 문맥 조회 크기, 잠금 대기 시간, 정책 거절 수, require 실패 코드 분포, 재시도 수, outbox 적체, saga 상태별 수, 캐시 적중률
- 구조화 로그: 계약의 오류 형식 그대로

## 10. 배포 형태

**Standalone 런타임**을 기본으로 한다. 단일 바이너리가 정의를 컴파일해 메모리에 계획을 올리고 HTTP 서버, dispatcher, 타이머, 스케줄러를 함께 돌린다. 수평 확장은 같은 바이너리 여러 개 + 같은 PostgreSQL. 역할 분리(`aip run --role api|worker`)는 설정으로.

Embedded(기존 Java/Node 앱에 라이브러리로 붙이기)는 채택 경로로 가치가 있지만 M6 이후 검토한다. 이유: 두 개의 트랜잭션 관리자(호스트 프레임워크와 AIP)가 같은 연결을 공유하는 문제가 먼저 풀려야 한다.
