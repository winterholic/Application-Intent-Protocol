# 제품 실행기의 resource identity 경계

제품 SQL·조회·쓰기·actor 경로에서 명시적인 `id`는 해당 resource의 non-null bigint 기본키로 사용된다. 현재 V1은 다른 타입·nullable 선언도 받아 이 전제와 맞지 않는 facts를 만들 수 있다. 전이에서 `id`를 바꾸면 실행기는 변경 전 ID를 결과와 후속 효과의 대상으로 계속 사용한다. 이를 새 identity 기능으로 간주하지 않고 의미 검사 단계에서 거부한다. 기존 비공개 `ClubMember`·`RecruitmentBookmark`처럼 `exists`의 원본으로만 사용하는 관계 resource는 id를 생략할 수 있으므로, 모든 resource에 id를 강제하지 않는다.

## 설계 관문

1. 원칙 1·4·6: 실행할 수 없는 정의를 조기에 거부하고 서버가 객체 식별과 권한을 유지한다.
2. 표현 범위를 새로 넓히지 않는다. id가 명시된 경우 기존 실행기의 `id: Id` 또는 같은 resource의 `Resource.Id`만 기본키 선언으로 받는다. 비-primary typed ID 필드와 id 없는 내부 관계 resource는 이 제한의 대상이 아니다.
3. DB 생성·조회 실패를 앱 작성자가 개별 방어하지 않게 한다. 새 설정이나 API는 없다.
4. 명시적인 id 필드는 non-null 자기 타입 ID여야 한다. 전이의 기본키 변경은 의미 검사에서 거부한다. 일반 필드의 typed ID 대입·Ref 대입은 기존 규칙을 유지한다.
5. 다섯 작성 형식은 공통 sema에서 같은 제한을 적용한다. wire의 숫자·문자열 후보나 SDK brand는 바꾸지 않는다.
6. 암묵적 ID 필드나 새로운 rekey API를 만들지 않는다. 이미 실행기가 가정하는 명시적 ID 하나를 검증한다.
7. 기존 AST·typed facts·DDL을 재사용하며 유효한 기존 정의의 지문에는 변화가 없다.
8. 최종 Id wire와 작성 형식은 OPEN으로 둔다. primary ID 수정 지원 여부를 새 API 설계로 확정하는 작업이 아니라, 현재 실행기가 안전하게 처리하지 못하는 선언을 명시 거부하는 보완이다.

검증은 의미 검사를 통과하던 잘못된 명시적 ID 선언과 일반 전이의 ID 변경을 먼저 재현한다. A/E/H 다섯 형식, 같은 resource의 qualified ID 정상 선언, 비-primary typed ID 및 id 없는 내부 관계 resource의 정상 선언을 대조한다. 기존 전이·생성·관계·지문 검사를 전체 회귀한다.
