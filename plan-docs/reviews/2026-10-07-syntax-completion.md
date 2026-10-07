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

## 조사 중인 제약

`atMost N`은 경쟁 쓰기의 그룹 잠금과 커밋 검사, 초기화·migration의 일관성이 필요하다. 단순 count 검사만으로 지원을 선언하지 않는다. 구현 결정과 설계 관문은 조사 결과를 확인한 뒤 추가한다.
