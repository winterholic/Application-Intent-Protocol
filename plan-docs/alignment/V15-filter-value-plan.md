# V15. 공개 필터 값과 실행 검사 정합 계획

> 상태: 독립 후보 구현·실행 검증. [실행 결과·한계](V15-filter-value-results.md), [독립 검토](../reviews/codex-v15-sol-review.md). 아래는 구현 전 비교 기준과 8관문이다. [V14 결과](V14-typed-apply-results.md)의 never where 값을 실제 처리 범위에 연결했다.

## 구현 전 확인한 경계(V14 기준)

V1은 Ref를 제외한 타입의 eq를 허용하며 Url·Enum·Time도 포함한다. V5는 읽기에서 string·enum union을 생성한다. V2는 Url/Enum 값을 거부하고 Time 문자열의 겉모양만 검사한다. V3 where는 Bool·Int·Text와 후보 Id/Ref만 처리해 정상 Time/Url/Enum도 BAD_VALUE다. V14는 이 미구현 값을 never로 표시했으며 공개 언어 제한으로 확정하지 않았다.

현재 잘못된 Time where가 PG INTERNAL을 거쳐 pending이 되는 것은 아니다. V3가 SQL 전에 거부한다. V2의 DB 값 변환 오류(SQLSTATE 22)는 BAD_VALUE로 매핑된다. Time을 얕은 검사만 복사해 V3에 추가하면 V3의 DB 오류 분류(INTERNAL)와 SDK pending이 연결될 수 있으므로 그 반례를 검증한다. 코드를 읽은 조건부 경로이며 실제 재현은 아직 하지 않았다.

## 8개 설계 관문

1. 원칙 1·2·3·4·6: 선언·SDK 타입·필수 서버 검사가 같은 요청을 받아야 한다. 타입 확대가 실행 권한 확대로 바뀌지 않게 기존 allowlist를 유지한다.
2. 이미 공개된 Url/Enum/Time 조건을 실제 실행한다. Ref.eq 불허나 null 조건의 새 표현을 임의로 변경하지 않는다.
3. resource마다 mapper/parser를 추가하지 않고 facts·타입·wire를 받는 공통 값 검사 후보를 사용한다.
4. Enum 멤버십·Time의 실제 날짜/offset·타입·Id domain은 서버가 확인한다. SQL은 매개변수로 전달하고 원자성·권한·비용을 유지한다.
5. JS Date의 ISO 출력과 Python datetime 문자열을 실제 fixture로 대조한다. Python SDK 구현으로 표시하지 않는다.
6. 같은 scalar 의도를 read와 where에서 다른 변환으로 쓰게 하지 않는다. Url은 현재 text DDL의 문자열로 취급하며 URL 문법 정책을 새로 확정하지 않는다.
7. 기존 facts·planner·write parser를 연결한다. 새 IR·타입 계층·오류 엔진을 만들지 않는다.
8. Id 공개 표현·표준 조합의 OPEN을 유지한다. Time parser와 URL 제한 후보를 창시자 승인·최종 사양으로 표시하지 않는다.

## 최소 검증과 반증 기준

- 실제 선언에서 Url/Enum/Time eq를 노출하고 유효 값의 read/filter/where 결과와 실제 변경 행을 대조한다. 닫힌 필드·연산·다른 actor와 원자성 검사는 유지한다.
- Enum 비멤버·잘못된 타입·2월 30일·잘못된 offset·형식/범위 밖 Time을 필수 검사에서 거부한다. 실패는 BAD_VALUE와 DB 불변이며 INTERNAL/pending으로 숨기지 않는다.
- V2와 V3 where에 같은 parser를 쓰되 Legacy/V13 Id 규칙을 불필요하게 바꾸지 않는다. null 값 조건과 create/compose literal의 별도 지원 범위를 기록한다.
- V5 where의 never를 실제 지원 타입으로 연결한다. 정상/음성 tsc·생성 공개 지문 변경·이전 binding 첫 쓰기 거부를 확인한다. 지원 확대가 타입에서 드러나야 한다.
- Time의 UTC/offset·소수초와 DB 바인딩의 의미를 비교하고 caller 변환 없이 같은 값을 사용한다. 버전/파서 선택은 로컬 의존성과 공식 근거를 확인한 뒤 진행한다.
- 적합성 검사 생략·read/where 검사 불일치·오류 분류 회귀를 고장 주입으로 검출한다. 기존 typed apply·Id·세션·캐시·개발 조언 회귀를 실행한다.

prefix는 V1/V5가 공개하지만 V2 planner가 거부하던 공백이며, 아래 관문을 거쳐 같은 자율 작업에서 연결·검증했다. 리터럴 prefix와 SQL wildcard를 혼동하지 않는다. V3 create/compose의 Url·Time 값 미지원은 별도로 남긴다. 실행 후보를 최종 제품 정책 승인으로 표시하지 않는다.

## 연속 구현: 이미 공개된 Text.prefix의 8개 관문

1. 원칙 1·2·3·4·6에 따라 선언·생성 타입과 실행 범위를 일치시킨다.
2. V1과 SDK가 이미 공개한 Text.prefix만 실행한다. 쓰기 where는 기존 eq만 허용한다.
3. 화면마다 검색 endpoint를 만들지 않고 요청의 기존 filter를 사용한다.
4. 필드·연산 allowlist와 행 정책·상한을 유지한다. 값은 매개변수이며 `%`, `_`, 역슬래시는 문자 그대로다.
5. JS/TS·Python JSON 문자열의 Unicode 접두어를 그대로 전달한다.
6. regex·LIKE pattern 같은 별도 요청 표현을 추가하지 않는다. 빈 접두어도 일반 문자열 의미로 대조한다.
7. 현재 planner에서 `left(column, char_length(parameter)) = parameter`를 사용한다. 새 IR을 만들지 않는다. 인덱스 최적화는 실측 전 보장하지 않는다.
8. 최종 검색 API·확장·Id 표현·쓰기를 창시자 선택으로 확정하지 않는다. 리터럴 접두어와 정책의 실제 PG/HTTP 결과만 검증한다.
