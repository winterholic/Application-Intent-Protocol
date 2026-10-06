# 06. Extension 모델

> 상태: Proposed · 목표: 각 의존성이 자기 **문법, 타입, 효과 서명, 검증 규칙, 계획 훅, 런타임 어댑터**를 함께 가져오되, Core의 안전 모델은 우회할 수 없게 한다.

## 1. Extension이 제공하는 것

| 요소 | 내용 | 검증 주체 |
|---|---|---|
| manifest | 이름, 버전, 요구 Core 버전, 요구/제공 capability, 필요한 비밀값·설정 | Core (부팅 시) |
| types | `Ext` 타입(예: `redis.Counter`, `s3.Object`, `payments.AuthRef`), 저장 표현, 비교 가능 여부 | Core 타입 검사기 |
| effects | 효과 서명(`03-ir.md` 7절): 일관성 등급, 멱등성, lookup, 보상/확정, 가역성, capability | Core 효과 분석기 |
| forms | L3 문법 형식과 **하강 규칙** (L3 → L2/L1) | 하강 결과를 Core가 다시 검사 |
| rules | 추가 정적 규칙 (오류/경고 추가만 가능, Core 규칙 해제 불가) | Core가 실행 |
| planner hooks | capability 선언, 비용 모델 조각 | planner |
| runtime adapter | 효과의 실제 실행 | 런타임 샌드박스 |
| conformance kit | 서명대로 동작하는지 확인하는 테스트 묶음 | CI |

## 2. 안전 경계: 왜 우회할 수 없는가

1. **Forms는 IR을 만들 뿐 실행하지 않는다.** 하강 결과는 일반 Core IR이고 Core analyzer가 처음부터 다시 검사한다. extension이 "이 형식은 권한 검사를 생략한다"는 IR을 만들면 E-ALLOW-MISSING으로 실패한다.
2. **Rules는 더하기만 한다.** extension 규칙은 진단을 추가할 수 있지만 Core 진단을 억제할 수 없다.
3. **Adapter는 서명에 적힌 capability만 받는다.** 1st-party 어댑터는 Rust 네이티브(신뢰, 코드 리뷰 대상), 3rd-party 어댑터는 WASM 컴포넌트로 실행하고 호스트가 네트워크 대상, 비밀값, 파일 접근을 capability 단위로 중개한다. `http.post` capability가 `api.stripe.com`으로 제한되면 다른 호스트로 나갈 수 없다.
4. **서명 위반은 conformance kit가 잡는다.** "멱등이라 선언했는데 같은 키 두 번에 두 번 부작용"은 정적으로 못 잡는다(보장 불가 45번). 대신 extension 배포 조건으로 conformance kit 통과를 요구한다.

WASM 컴포넌트 모델(wasmtime)의 capability 중개가 이 수준으로 가능한지는 M4에서 기술 스파이크로 확인한다(확인 필요). 불가능하면 3rd-party 어댑터는 별도 프로세스 + 제한된 RPC로 대체한다.

## 3. 형식(form) 정의 방식

형식은 **선언적 문법 조각 + 하강 템플릿**으로 정의한다. 임의 코드를 컴파일 시점에 실행하지 않는 것이 기본이다.

```aip-form
// redis extension 안의 형식 정의 (개념 스케치). extension manifest 언어이며 앱 문법이 아니다
form counter
  syntax  field <name>: counter via redis dedupe by <key: ClientKey> within <dedupe: Duration>
                         [window <window: Duration>]
  lower {
    field <name>: External(redis.CounterSpec { dedupe: <dedupe>, window: <window> })
    on query touch <name>:
      effect redis.counter_hit(entity_id, <key>)        // observational
    schedule sync_<name> every 5m:
      update <Entity> e set e.<name>_snapshot = redis.counter_read(e.id)   // 선택: DB 스냅샷
  }
```

하강 템플릿으로 표현할 수 없는 복잡한 형식(inventory 예약처럼 여러 테이블을 생성하는 것)은 **결정적 WASM 하강기**를 허용한다. 입력 AST → 출력 IR, I/O 없음, 시간 제한. 출력은 여전히 Core가 검사한다.

## 4. 1st-party extension (Core와 동시 개발)

사용자의 방향대로 "라이브러리로 선언하기로 한 외부 의존성"을 초기부터 실제로 만든다. 각 extension은 Reality Test 케이스로 검증한다.

| extension | 제공 형식/효과 | 일관성 등급 | 검증 케이스 | 로컬 테스트 인프라 |
|---|---|---|---|---|
| `aip-postgres` | 저장소 어댑터, 제약 하강(unique, partial unique, EXCLUDE, CHECK, deferred trigger), planner, 마이그레이션, outbox/saga/timer 저장소 | local | 전부 | 로컬 PostgreSQL 17 (설치 확인됨) |
| `aip-redis` | `counter`, `cached` 저장소, `rate limit`, 분산 락 없음(원칙상 제공 안 함) | observational, deferred | 8, 9, 15 | Redis (`redis-server` 설치 확인됨) |
| `aip-kafka` | `emit ... to kafka topic` 전달, 외부 이벤트 구독(`consume`) + 멱등 처리 | deferred | 15 | Redpanda 또는 Kafka (설치 필요, 확인 필요) |
| `aip-s3` | `Upload` 처리, `s3.put`(staged), presigned 다운로드, 고아 GC | staged | 1, 7 | MinIO (설치 필요, 확인 필요) |
| `aip-http` | 선언형 외부 HTTP 효과 정의(엔드포인트, 멱등 키 헤더, lookup, 재시도, 서명 검증된 웹훅 수신) | 선언에 따름 | 14 | 로컬 mock 서버 |
| `aip-mail` | 템플릿 메일, 발송 멱등, 반송 웹훅 | deferred | 12 | Mailpit 등 (설치 필요) |
| `aip-payments` | `authorize/capture/void/refund/lookup` 표준 서명, PG 어댑터(토스페이먼츠, Stripe), fake PG | reservable 또는 irreversible+refundable (PG별) | 13, 14, 15 | fake PG + 샌드박스 키 |
| `aip-auth-oidc` | OIDC 로그인(카카오, 구글), 세션/토큰 회전, `actor` 바인딩 | local | 전부 | mock IdP |
| `aip-notify` | 앱 내 알림 저장/읽음 처리, `notify` 형식 | deferred | 2, 7, 12 | PostgreSQL |

PG마다 결제 모델이 다르다는 점은 **서명을 다르게 선언**해서 드러낸다. 토스페이먼츠의 승인 방식이 authorize/capture 분리를 지원하는지는 확인 필요이며, 지원하지 않으면 그 어댑터의 `charge`는 `irreversible + refundable`로 선언되고 컴파일러가 효과 배치를 바꾼다. 서로 다른 PG를 같다고 가장하지 않는다.

## 5. 선언 예시 (사용자 관점)

```toml
# aip.toml
[app]
name = "ariari"
definition = "app.aip"

[extensions]
postgres = "0.1"
redis = "0.1"
s3 = "0.1"
mail = "0.1"
auth-oidc = { version = "0.1", providers = ["kakao"] }

[runtime]
trusted_proxies = ["10.0.0.0/8"]
```

```aip
use postgres
use redis
use s3
use mail
use auth

actor Member via auth.oidc(kakao)
```

`use`가 없는 extension의 형식이나 효과를 쓰면 컴파일 오류다. `aip.toml`에 있는데 환경변수(`REDIS_URL` 등)가 없으면 부팅 거부다.

## 6. 버전과 호환

- extension은 Core IR 버전 범위를 선언한다.
- 효과 서명 변경은 breaking이다(예: `idempotent`였던 효과가 아니게 됨). extension 메이저 버전을 올려야 한다.
- 한 앱에서 같은 extension의 두 버전은 공존하지 않는다.

## 7. 미결정

1. 3rd-party extension 배포 채널(레지스트리, 서명).
2. WASM 하강기의 결정성 검증 방법.
3. extension 간 의존(예: `aip-notify`가 `aip-mail`을 쓰는 경우)의 선언 형식.
