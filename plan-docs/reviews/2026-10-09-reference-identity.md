# Ref 대상의 명시적 identity

ID 없는 내부 관계 resource는 `exists`의 원본으로 유효하다. 그러나 `Item.link: Link?`의 대상 `Link`에 `id`가 없으면 V1은 정의를 받으면서 V2는 `REFERENCES link(id)`를 생성해 PostgreSQL SQLSTATE 42703으로 실패한다. Ref가 사용하는 identity의 존재를 의미 검사에서 확인한다.

## 설계 관문

1. 원칙 1·4: 실행 불가능한 관계를 DB 배포 전에 진단한다.
2. 새 문법이나 관계를 추가하지 않는다. Ref 대상에는 명시적인 id가 필요하고, 그 타입은 기존 자기 resource의 non-null Id 규칙을 따른다.
3. 앱 작성자가 DB의 존재하지 않는 열 오류를 추적하는 부담을 줄인다. 숨은 ID 생성이나 새 설정을 요구하지 않는다.
4. 서버가 Ref의 대상 identity를 검증하며, 기존 FK 집행은 유지한다.
5. 공통 의미 검사에서 A/E/H 다섯 작성 형식을 동일하게 검사한다.
6. 기존 Ref와 Id를 재사용한다. Ref와 달리 FK를 생성하지 않는 비-primary typed ID의 저장 규칙은 바꾸지 않는다.
7. ID 없는 내부 관계를 전체 금지하지 않고 FK를 저장하는 Ref 필드의 대상으로 사용하는 지점만 거부한다. 기존 `exists`·정확히 한 행 update와 predicate에 `this` 행을 전달하는 내부 관계 사용은 유지한다. Predicate의 resource 매개변수 자체는 저장 필드가 아니므로 이 제한을 적용하지 않는다.
8. 최종 Id wire·쓰기 조합·작성 형식은 OPEN이다. 암묵적 identity나 호출자 CRUD를 새로 도입하지 않는다.

검증은 다섯 작성 형식의 필수·nullable Ref와 자기 참조를 ID 없는 대상에서 거부하고, ID 없는 내부 관계를 exists로 읽는 정상 사례와 명시적 ID가 있는 관계를 대조한다. 유효한 기존 fixture의 facts·DDL 지문과 순환 Ref 회귀를 유지한다.
