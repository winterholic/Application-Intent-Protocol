# Decimal(p,s) 제품 스칼라 설계 관문

## 목적과 경계

제품 spikes 경로에서 `Decimal(p,s)` 필드 값을 정확하게 저장하고 전달한다. 구현 범위는 standalone `.aip`, TS/Python 블록, TS/Python 정형 객체 hmap 입력, typed facts, 필드 DDL, 필터, 정렬, transition 상수와 `create`, V4 worker 입력·출력 검사, 생성 SDK, V6 typed client, migration이다. `sum`/`avg`와 산술·통화별 rounding·Money 및 최종 외부 API 결정은 이 단위에서 다루지 않는다.

JSON wire는 십진 문자열을 사용한다. 기존 PoC 결정 D-P4는 Decimal/Money 응답을 십진 문자열로 만들고 IEEE-754를 거치지 않게 한다. 검증 로그는 PostgreSQL JSONB를 Rust `serde_json::Value`로 읽는 경로에서 소수 정밀도가 사라질 수 있어 select, group, 효과 payload까지 문자열로 직렬화한 근거를 남겼다. 제품에서는 같은 손실 경계를 필드 출력, traverse, read extension, worker 결과에서 닫아야 한다. JSON 숫자 입력은 받지 않는다. TS 생성 타입은 `string`이다.

## 선언·facts

- 작성 문법: `Decimal(p,s)`.
- 정형 객체의 scalar type 표기는 `Decimal(p,s)`로 읽는다.
- 공통 typed facts는 새 facts 키를 추가하지 않고 `ty: "Decimal<p,s>"`로 canonicalize한다. `p`/`s`는 ASCII 정수 표기이며 공백·leading zero를 canonical facts에 남기지 않는다.
- 기존 facts의 shape, key set, 값, execution digest는 바뀌지 않는다. hmap 다섯 정의 형식도 동일한 typed facts와 digest를 만든다.

## 경계와 값 규칙

PostgreSQL 공식 numeric 문서는 구현 최대를 정수부 131072자리와 소수부 16383자리로 설명한다. 로컬 검증 DB는 PostgreSQL 17.11이다. 제품 파서는 DB 최대치를 언어 입력으로 노출하지 않고 bounded execution policy를 적용한다: `1 <= p <= 38`, `0 <= s <= p`. 38은 decimal128 정밀도를 뜻하지 않으며 AIP 실행기에서 입력 크기와 DDL 범위를 제한하는 제품 자원 상한이다. 이는 PostgreSQL의 표현 한계보다 훨씬 낮다. 타입의 구체적 SQL은 `numeric(p,s)`이다.

입력은 UTF-8 문자열이며 다음 정규 문법을 사용한다: `-?(0|[1-9][0-9]*)(\\.[0-9]+)?`. 허용하는 선행 부호는 `-`뿐이다. `+`, 지수 표기, NaN/Infinity, 공백, 구분자, 유니코드 숫자, leading zero, 소수점 뒤 숫자가 없는 값, `.5`, JSON number는 거부한다. 음수 0은 받아들이며 DB 저장/출력은 PostgreSQL numeric의 부호 없는 0으로 정규화한다.

정수부의 유효 자릿수는 `max(p-s, 0)`을 넘지 않는다. `p == s`일 때 정수부는 `0`만 허용하므로 `Decimal(2,2)`의 `0.99`는 유효하고 `1.00`은 거부한다. 소수부는 최대 `s`자리이며 초과 scale은 PostgreSQL의 암묵 반올림에 맡기지 않고 거부한다. `s == 0`은 소수점을 포함한 입력을 거부한다. 입력 바이트 수는 타입 상한에서 유도한 최대 41 bytes(부호 1 + 최대 38자리 + 소수점 1 + `p == s`일 때 정수부 `0` 1자리) 안에서 검증한다. 출력은 DB의 `numeric(p,s)::text`로 scale을 고정해 `1` 입력을 `1.00`처럼 반환한다.

이 규칙은 `docs/DECISIONS.md` D-P4의 손실 없는 문자열 wire 방향을 참고하되 JSON 숫자를 받지 않는 제품 후보를 검증한다. 기존 PoC의 범용 Decimal보다 필드 precision/scale 검사가 강하다. 제품 경로의 실행 규칙이며 최종 언어·wire 결정이나 PoC 경로의 자릿수 검사를 완료한 것으로 해석하지 않는다.

## 읽기·쓰기·migration

- DDL은 `numeric(p,s)`를 만든다. 기존 타입 DDL 및 기존 facts는 변경하지 않는다.
- 필터는 문자열만 받고 정밀도·scale 검증을 한 뒤 parameter로 전달한다. 값은 SQL 문자열에 삽입하지 않는다. SQL cast는 Decimal 값의 numeric 타입을 보존한다.
- 정렬은 이미 허용된 필드에 대해 PostgreSQL numeric 순서로 한다.
- transition contextual string literal은 선언한 필드 타입에 따라 검사되고 정수/소수 canonical 문자열 parameter를 통해 numeric(p,s)으로 쓴다. `create` 값도 동일한 경로와 검사를 사용한다.
- Decimal 필드 출력은 numeric을 먼저 text로 만든다. root 선택과 N:1·1:N traverse는 정확한 문자열을 반환하고, 확장·worker output 검사는 JSON Number를 거부한다. 기존 집계는 Int만 지원하며 Decimal 집계를 추가한 것은 아니다.
- SDK 입력/출력은 TS `string`; V6 typed client와 V4 worker는 같은 canonical grammar, 범위, scale 규칙을 런타임 검사한다.
- facts digest 변경은 migration 계획을 만든다. 새 컬럼은 `numeric(p,s)`다. 기존 열의 precision/scale 변경과 다른 타입에서 Decimal로 convert하는 경우 저장된 값을 새 validator로 검사한다. 초과 값·scale이 있으면 PostgreSQL의 암묵 반올림 전에 `Blocked`로 멈춘다. backfill 값도 같은 규칙으로 검사한다. nullable과 타입을 함께 바꿀 때 NULL 검사도 유지한다.

## 구현·검증 순서

1. 숫자 타입 문법, parser/hmap canonical facts, 범위·문자열 규칙에 대한 RED 테스트를 먼저 만든다.
2. scalar validator, field/DDL/filter/SQL output, sema typed literals, transition/create, SDK/worker/client, migration 테스트를 수직으로 세운다. 금액 출력이 JSON 문자열임을 PostgreSQL 실험으로 증명한다.
3. owned files에서 최소 구현 후 여러 정의 형식의 digest, 실제 PostgreSQL read/write, typed SDK compile, V4 worker, migration DB 테스트를 실행한다.
4. 제품 `aip-migrate`에서 Decimal 변환·narrowing·nullable 변경을 실제 PostgreSQL로 검사한다. 기존 migration 회귀와 함께 실행한다.

## 검증 근거

- PostgreSQL 공식 자료: [Numeric Types](https://www.postgresql.org/docs/current/datatype-numeric.html) — `numeric`의 임의 precision 지원 한계와 NaN/Infinity 지원 사실.
- 로컬 DB: `psql postgres://localhost/postgres -Atc 'SHOW server_version'` → `17.11 (Homebrew)`.
- PoC 회귀 근거: `docs/DECISIONS.md` D-P4, `docs/design/09-verification-log.md` Decimal/Money 절, `docs/design/agent-logs/2026-10-02/agent-pass7-log.md`. PoC는 정확한 Decimal/Money 응답을 문자열로 직렬화하지만 당시 `Decimal(p,s)` precision/scale 초과 검사는 남은 항목이었다.
