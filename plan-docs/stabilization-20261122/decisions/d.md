# Track D 소유자 결정 대기 (2026-11-22)

공개 의미·호환성·지원 범위를 바꾸는 항목이다. 확정하지 않았고 코드도 바꾸지 않았다. 각 항목은 독립 작업을 멈추지 않는다.
근거와 시험은 [track-d.md](../track-d.md).

## D-DEC-1. `Int`의 범위: int64인가, JavaScript 안전 정수인가

- **문제**: 범위를 쓰지 않은 `Int` 필드(그리고 `Int(0..9223372036854775807)`)가 `check`를 통과하고 int64 값을 저장·반환한다. 서버는 정확한 숫자를 보내지만 TypeScript SDK는 안전 정수(2^53-1)를 넘는 값을 `CLIENT.PROTOCOL_ERROR`로 거부하고, TS 호출자는 그런 정확한 값을 쓰기로 보낼 수도 없다. Python SDK는 정확하다. 확장 입력의 `Int`는 이미 "JavaScript 안전 정수 범위"로 거부한다.
- **재현**: `product/tests/semantic-definition-matrix.test.mjs`의 `Int-unbounded-beyond-2^53`(서버 원문, TS, Python 세 결과를 한 줄에 기록).
- **선택지**
  - (a) 무범위 `Int`의 기본 범위를 안전 정수로 줄이고 DDL에 CHECK를 둔다. 장점: 모든 SDK가 같은 값을 본다. 단점: 기존 bigint 데이터와 `adopt`·`introspect`한 외부 표가 깨질 수 있다. 공개 의미 변경.
  - (b) 지금처럼 두고 문서화한다("안전 정수 밖은 Python만 정확, TS는 거부"). 장점: 변경 없음. 단점: TS 앱이 그런 값을 만나면 읽기 전체가 실패한다.
  - (c) 큰 정수를 위한 별도 타입(예: 문자열 wire)을 1.1에 둔다. 장점: 가산. 단점: 새 타입의 의미 전체를 설계해야 한다.
- **추천**: 1.0은 (b)로 문서화하고(`product/LIMITS.md`) `check`가 무범위 `Int`에 advisory를 내는 것(가산)을 검토한다. 장기는 (c).
- **코드 변경 범위**: (b)는 문서만. advisory는 `sema.rs`(핵심 파일 대기 C6). (a)는 `sema.rs`·DDL·migration.
- **기존 소비자 영향**: (a)만 있다. 안전 정수 밖 값을 이미 저장한 앱은 마이그레이션이 필요하다.

## D-DEC-2. 쓰기 확장 `/apply`에서 계약 헤더가 없을 때의 코드

- **문제**: 전이·묶음·조합·`/status`는 헤더 없음을 `WIRE.CONTRACT_REQUIRED`로, 쓰기 확장 `/apply`는 `WIRE.CONTRACT_MISMATCH`로 답한다(실측, track-d.md 6절). 메시지도 "계약 지문이 필요함"이라서 `MISMATCH`와 모순이고, 지문을 줬는데 다른 경우도 같은 문구다.
- **선택지**
  - (a) 헤더 없음을 `CONTRACT_REQUIRED`로 맞추고 메시지를 둘로 나눈다. 장점: 경로 사이 일관. 단점: 직접 HTTP로 `MISMATCH`만 처리하던 클라이언트가 바뀐 코드를 본다(두 SDK는 둘을 같게 취급한다).
  - (b) 코드는 두고 메시지 문구만 고친다. 장점: 호환. 단점: 불일치가 남는다.
- **추천**: (b)를 먼저(비파괴), `SUPPORT.md` 표에 그대로 기록. (a)는 1.1.
- **코드 변경 범위**: `spike-v6-transport/src/write_extension.rs`, `lib.rs`(B 소유, 핵심 파일 대기 C4). SDK 변경 없음.
- **기존 소비자 영향**: (a)만. `product/SUPPORT.md`는 이미 현재 값을 적었다.

## D-DEC-3. 단독 집계(`expose aggregate`)의 시간 한도

- **문제**: 정의가 시간 한도를 정할 수 없고 고정 2초다(실측 2007 ms, 같은 조건의 `deadline 1s` 읽기는 1008 ms). 오류 본문에 한도가 없다.
- **선택지**
  - (a) 집계 선언에 선택 키 `deadline`(기본 2s)을 허용한다. 장점: 가산, 기본 동작 불변. 단점: 새 문법 키와 지문 입력.
  - (b) 1.0 한도로 문서화한다(`product/LIMITS.md`). 장점: 변경 없음.
- **추천**: (b)를 지금(문서), (a)는 `plan.rs` 소유가 정해진 뒤 1.1.
- **코드 변경 범위**: (a)는 `spike-v2-read/src/plan.rs:316,1097`, 문법·sema·`syntax_card.rs`·골든.
- **기존 소비자 영향**: (a)는 없다(기본 2s 유지).

## D-DEC-4. TS SDK의 타입 엄격화 중 기존 호출을 깨는 것

적용한 개선은 모두 유효한 기존 호출을 깨지 않는다(track-d.md 4절). 아래는 **깨질 수 있어서 적용하지 않았다.**

| 후보 | 이득 | 깨지는 호출 | 추천 |
|---|---|---|---|
| `ids`를 비어 있지 않은 tuple로 | 빈 배열을 컴파일 타임에 거부 | 길이를 모르는 `Id[]` 변수를 넘기는 모든 호출 | 하지 않음(서버가 `WIRE.BAD_REQUEST`로 막는다) |
| Decimal·Id를 `${number}` 같은 템플릿 리터럴 타입으로 | 리터럴 오타를 거부 | `string` 변수를 넘기는 호출 | 하지 않음 |
| `writeExtension`이 입력 오류도 결과 객체로 돌려주기 | 예외·결과 이중 모양 해소 | 입력 오류를 `catch`하는 호출 | 1.1, 변경 로그 필요 |
| Python의 확장 입력 사전 검증 | TS와 같은 전송 전 거절 | 서버 결과(`ok:false`)를 기대하던 호출 | 하지 않음 |
| 빈 묶음 오류 코드 통일(TS `CLIENT.PROTOCOL_ERROR`, Python `WIRE.BAD_REQUEST`) | 일관 | 둘 중 하나를 기대하던 호출 | 1.1 |

## D-DEC-5. 공개 count와 단독 집계 외 항목

D에서 새로 결정할 것은 없다(A의 공개 count 결정과 E의 R1 결정을 따른다).

## D-DEC-6. 확장 안쪽 거절의 노출

- **문제(관찰)**: 쓰기 확장이 `ctx.data.apply`에서 받은 `FORBIDDEN`·`MISSING_TARGET`·`INVALID_STATE`·`BAD_VALUE`는 호출자에게 `WIRE.EXTENSION_ERROR`로만 보인다. 확장 작성자는 `ctx.reject`로 업무 코드를 직접 줘야 한다. 이는 의도된 정보 은닉이고 `spec/wire-errors.md`에 있다(track-d.md 3절의 예외 표).
- **선택지**: (a) 그대로(추천). (b) 정책과 무관한 상태·검증 코드만 전달한다. 단점: `MISSING_TARGET`·`FORBIDDEN` 구분이 호출자에게 새면 존재 오라클이 된다(A 영역).
- **추천**: (a). 문서 정합만 유지.
- **기존 소비자 영향**: (b)는 `EXTENSION_ERROR`를 기대하던 호출이 바뀐다.

## D-DEC-7. 감사 기록이 경로를 구별하지 않는다

- **문제(관찰)**: 묶음 안의 전이도 감사 행의 `via`가 `apply`다. 묶음·조합·확장은 직접 호출과 구별되지 않는다.
- **선택지**: (a) `via`에 `bundle`, `compose`, `extension`을 더한다(가산이지만 `aip service audit` 출력의 값이 늘어남). (b) 그대로.
- **추천**: (a)를 1.1에 검토. 이번 안정화에서는 하지 않는다.
- **코드 변경 범위**: `spike-v3-write`(감사 `via`)와 `aip-service` 출력, B·E와 조율이 필요하다.
