# 생성 후보 행의 생략 필드와 NULL

V1은 nullable 필드를 읽는 `expose create allow`를 받는다. 그러나 V3는 제공된 값만 후보 행 `v`에 투영해 생략된 `archived` 같은 필드를 읽을 때 SQLSTATE 42703을 낸다. JSON의 명시적 null도 nullable 타입인데 리터럴 검사에서 거부한다. 값이 하나도 없는 생성은 빈 SQL 열 목록을 만들어 실행하지 못한다.

## 설계 관문

1. 원칙 1·4: 선언한 생성 권한식과 실제로 삽입할 행의 값을 일치시킨다.
2. 기존 W0/W1의 공개 생성 계약 안에서 nullable 생략·null과 빈 값을 실행한다. 새 HTTP 생성 API를 열지 않는다.
3. 앱마다 nullable 필드의 의미를 우회하기 위한 상수나 서버 코드를 추가하지 않게 한다.
4. 허용 필드·타입 검사·생성 allow·check·트랜잭션을 유지한다. 필수 필드의 명시적 null은 BAD_VALUE다. 생략 필드의 후보 값은 현재 DB 의미인 NULL이며 서버가 생성하는 id는 후보 값으로 위조하지 않는다.
5. 공통 create_row를 고쳐 리터럴 묶음과 서버 출처 조합의 권한 평가를 함께 보완한다.
6. 새 타입이나 기본값 문법을 도입하지 않는다. PostgreSQL의 실제 테이블 행 타입으로 NULL을 표현해 스칼라·Ref별 SQL 타입 매핑을 중복 구현하지 않는다.
7. 후보 SELECT에 생략된 비-id 필드를 추가하되 INSERT의 지정 열은 제공된 열만 유지한다. 제공 값이 없을 때는 PostgreSQL의 열 목록 없는 INSERT SELECT로 기존 DB 기본값을 사용한다.
8. 작성 형식·W0/W1/W2·최종 CRUD·기본값 문법은 OPEN이다. 자동 생성 id를 allow에서 평가하는 계약은 이 수정으로 새로 정하지 않는다. W1의 Ref 상수 금지도 그대로 유지한다.

실제 PostgreSQL에서 Bool·Int·Text·Email·Time·Date·Decimal·Enum·Ref·typed ID의 생략/명시적 null, 빈 생성, W1 출처 생성, 익명·권한 거부, 필수 값 null 거부와 묶음 전체 rollback을 대조한다. 생성 SQL에서 권한식이 읽는 생략 필드가 사라지지 않는지 확인한다.
