# 제품 문법 보완

기준은 `docs/PRINCIPLES.md`와 2026-10-07 work-history의 제품 경로다. 기존 PoC의 문법이 있다는 사실을 제품 지원으로 계산하지 않는다. 제품은 `aip service`가 사용하는 spike v1·v2·v3·v5·v6와 일반 migration이다.

## 설계 관문: 작성 형식 일치

1. 원칙 1·3·4: 이미 있는 선언을 TS/Python 객체 정의에도 연결해 반복 구현과 작성 형식별 예외를 줄인다.
2. offset·cursor·1:N traverse·unique·check·deferred invariant를 객체형에서도 표현한다. 기존 전이의 복수 대입·create/update/notify 효과와 expose apply의 sameScope, 실험 중인 expose create/compose도 같은 AST로 연결한다. 서버가 허용하는 요청 범위는 같은 텍스트 정의와 동일하다.
3. 별도 서버 코드를 추가할 필요가 줄어든다.
4. 객체를 실행하지 않고 정적 리터럴만 파싱한다. 기존 AST·의미 검사·정책·비용 제한을 함께 사용한다. 모르는 키와 잘못된 모양은 거부한다.
5. TS·Python의 기존 `define` 작성 형식을 유지한다.
6. 기존 키에 연결한다. 관계의 `via` 유무로 단일/목록 관계를 구분하고 정렬은 `field [asc|desc]`로 표현한다.
7. 새 IR이나 실행 경로를 만들지 않는다.
8. 최종 작성 형식·쓰기 조합·Id wire의 OPEN을 확정하지 않는다. 기존 W0/W1/W2 후보를 각 작성 형식에서 비교 가능하게 만들며 새 HTTP 쓰기 API는 추가하지 않는다.

검증: 같은 선언의 5개 작성 형식에서 execution facts와 digest를 대조한다. 객체형의 잘못된 budget·via·sort·unknown key, 정책 위반·limit 상한을 거부하는 음성 대조를 실행한다. 복수 effect 종류를 한 객체에 섞거나 create 출처 배열을 비우면 거부한다. 대입 parser를 공용화해 기존 단일 대입 문자열을 보존한다. 기존 정의의 facts와 DDL은 바꾸지 않는다.

작성 형식 테스트 `host_read_parity` 3개, `host_constraint_parity` 2개, `host_write_parity` 2개에서 구현 전 거부를 관측하고 구현 후 성공했다. prefix 상한의 ASCII·한글 200/201자 대조를 추가했다. 빈 prefix 허용은 V15의 명시적 기존 의미를 보존한다.

## 설계 관문: Date·Email

1. 원칙 1·4: 날짜·이메일의 공통 입력 검사와 계약 타입을 엔진으로 옮긴다.
2. 새 필드 타입과 허용된 필터·정렬을 표현할 수 있게 한다.
3. 앱별 문자열 검사와 SQL 캐스트 구현을 줄인다.
4. 필터·전이·생성 등 실제 값이 들어오는 경계에서 서버 검사를 유지한다. SQL 값은 매개변수로 전달한다.
5. JSON 문자열과 생성 SDK의 string 타입을 사용한다.
6. 날짜는 Date, 시각은 기존 Time으로 구분하며 같은 의미의 별칭을 추가하지 않는다.
7. 공통 스칼라 경로에 연결한다.
8. Decimal wire나 최종 쓰기 API는 결정하지 않는다. 기존 타입의 facts·DDL에는 변화가 없어야 한다.

검증: 실패 테스트를 먼저 실행한 뒤 5개 작성 형식·실제 PostgreSQL·쓰기 검증·SDK 타입·기존 제품 migration 회귀를 대조한다. Email의 수용 범위와 Date 캘린더 범위는 구현 결과에 명시한다.

## 정원 제약 후속

`atMost N`의 N≥2 집행과 제품 migration을 연결했다. 경쟁 쓰기의 resource 잠금·READ COMMITTED·전체 그룹 검사 및 보장 경계는 [정원 집행 설계](2026-10-07-capacity-enforcement.md)를 따른다. 기존 N=1의 facts와 DDL은 보존한다. 신규 resource·nullable 컬럼·backfill을 함께 추가하는 migration도 별도 회귀 테스트로 검사한다.

Date는 0001~9999년의 엄격한 `YYYY-MM-DD` 달력 날짜다. Email은 NUL·Unicode White_Space·복수 `@`와 비어 있거나 점 경계가 잘못된 domain을 거부하는 기본 검증이다. 전체 RFC 이메일 문법 지원을 뜻하지 않는다. Rust 서버와 TypeScript typed client가 같은 White_Space 정의를 사용한다.

Decimal의 자릿수·wire·migration 경계는 [별도 설계](2026-10-07-decimal-scalar.md)를 따른다. 정밀 저장과 읽기·쓰기 지원이 Decimal 산술이나 avg 지원을 의미하지는 않는다.

서버가 범위와 총 행 수를 명시한 다중 자식 갱신은 [bounded update 효과](2026-10-07-bounded-update-effects.md)를 따른다. 기존 update의 정확히 한 행 의미는 유지한다.

## 설계 관문: Bool 식을 대입 값으로 실행

1. 원칙 1·4: 의미 검사가 이미 허용하는 Bool 식을 실행에서도 처리해 별도 서버 분기를 줄인다.
2. 전이 대입과 서버가 선언한 create/update 효과에서 비교·논리·predicate·exists의 결과를 Bool 필드에 저장한다.
3. 기존 조건 표현식을 재사용하므로 앱 개발자가 같은 판단을 다시 구현할 필요가 줄어든다.
4. 기존 의미 검사·SQL 매개변수·정책 식 깊이와 작업량 상한을 그대로 적용한다. NULL인 조건 결과는 조건 평가와 같이 false로 저장한다.
5. 5개 작성 형식이 사용하는 공통 typed facts와 SQL 생성 경로에서 처리한다.
6. 새 연산자나 표현 문법을 추가하지 않는다.
7. 기존 조건 SQL 생성기를 값 위치에서도 재사용한다. 전이의 대입은 변경 전 행, 후속 효과는 변경 후 행을 본다는 기존 의미를 유지한다.
8. 최종 작성 형식과 쓰기 조합의 OPEN을 확정하지 않는다.

검증: PostgreSQL에서 비교·논리·predicate·exists 대입과 nullable 비교의 false 변환을 재현한다. 후속 create/update 효과가 변경 후 상태를 보고, 거부된 전이가 부수효과를 남기지 않는지도 대조한다.

## 설계 관문: 생성 SDK의 1:N 읽기·offset·cursor 연결

1. 원칙 1·3·4: v5가 생성한 공개 읽기 계약을 v6 typed client의 디코더와 호출 타입까지 연결해 서버 계약과 SDK 사이의 빈 구간을 없앤다.
2. 선언된 `traverseMany` 배열 결과를 검증하고, root resource에 선언된 offset/cursor 요청을 타입으로 허용한다.
3. 별도 호출 API나 실행 경로를 만들지 않고 기존 `connectTyped.read`, descriptor, 공통 generic 타입을 재사용한다.
4. 1:N 관계명·자식 선택 필드·limit를 descriptor allowlist와 대조한다. 응답 배열 크기는 요청 limit 및 descriptor 상한 이하인지 검사한다. root offset은 `maxOffset`, cursor는 계약의 `cursor:true`가 있을 때만 타입에 연다. 요청 값 범위·after와 sort의 정합성은 기존 서버 검사가 최종 책임을 유지한다.
5. wire 응답과 생성 binding 형식은 바꾸지 않는다. 1:N은 배열로 검증하고 기존 단일 traverse는 객체 또는 null로 계속 검증한다.
6. 단일 traverse와 1:N traverse의 null/빈 배열 의미를 구분한다. offset/cursor는 root query 옵션에만 적용하고 1:N 자식에는 추가하지 않는다. cursor의 `after`는 `id`를 필수로 하고 허용 sort 키를 선택적으로 받으며, 요청 sort와 정확히 일치하는 키 검사는 서버가 한다.
7. v5 `ReadDescriptors`와 `spike-v5-sdk/sdk/generic.ts`의 기존 공개 타입을 v6 runtime과 typed signature에서 공유한다. 새 facts 키·서버 분기·SQL 경로는 추가하지 않는다.
8. 최종 작성 형식, 쓰기 조합, Id wire의 OPEN을 결정하지 않는다. 새 문법도 추가하지 않는다.

검증: 구현 전 actual decoder 호출로 1:N 배열의 정상/초과 limit/잘못된 child·필드·임의 키를 재현한다. 실제 생성형 binding으로 offset/cursor 호출을 strict TypeScript에서 확인하고, 미선언 resource의 옵션과 잘못된 값은 컴파일 거부한다. 기존 단일 traverse의 object/null 결과와 facts projection은 회귀 테스트로 보존한다.
