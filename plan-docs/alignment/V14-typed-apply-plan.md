# V14. 공개 전이의 생성 쓰기 타입 연결 계획

> 상태: 독립 후보 구현·실행 검증(2026-10-04). [실행 결과·한계](V14-typed-apply-results.md), [독립 리뷰](../reviews/codex-v14-sol-review.md). 아래는 실행 전 대조와 검증 기준이다. [V12](V12-typed-transport-results.md)의 typed read와 [V13](V13-id-boundary-results.md)의 Id 후보·계약 사전 거부를 이어받는다.

## 현재 간극과 범위

`connectTyped`의 read는 생성 계약으로 제한되지만 apply는 여전히 `unknown` 입력과 `any` 결과다. facts의 공개 action·target·where 값 타입을 화면에서 다시 맞춘다. 실제 V12 fixture의 `Apply.approve`는 읽기 공개가 없으므로 읽기 Contract의 resource만 확장하면 빠진다. 쓰기 타입은 별도 공개 action map으로 생성하고 같은 앱 binding에 결합하는 후보를 비교한다.

범위는 direct `/apply`의 서버 정의 공개 전이다. W0/W1 bundle/compose·worker·최종 Id 표현을 정하지 않는다. 기존 public Legacy 생성기와 서버 start의 바이트 호환을 유지하며 새 후보를 명시적으로 분리한다.

현재 V13 식별값은 공개 읽기 타입과 Id 의미만 담는다. 공개 action·target·bulk가 바뀌어도 동일할 수 있다. V14 후보에서는 읽기와 쓰기 공개 산출물을 한 번 해시하고 서버 startup도 같은 함수를 사용한다. 두 해시·헤더를 새로 만들지 않는 대신 쓰기만 바뀌어도 읽기 binding을 다시 생성하는 보수적 비용을 명시한다. 내부 권한·from/effects·sameScope 식은 실행 정책으로 남기며 전체 업무 의미 동등성을 주장하지 않는다.

## 8개 설계 관문

1. 원칙 1·2·3·4·6: facts의 공개 동작을 공통 SDK 타입으로 연결해 화면별 재기술을 줄인다. 생성기 복잡도가 늘므로 실제 오류 검출과 호출 예제로 정당화한다.
2. 서버가 이미 허용한 공개 전이만 표현한다. 타입이 호출자의 권한을 부여하거나 새 업무 동작을 만들지 않는다.
3. 화면마다 endpoint·DTO·수작업 target 타입을 추가하지 않는다. 서버 기존 exposeApply와 필드 타입을 사용한다.
4. raw 요청도 서버가 타입·대상·bulk·권한·불변식·원자성을 최종 검사한다. TS는 정적 보조 검사다.
5. TS inference와 JS 런타임 envelope 검사를 연결한다. Python SDK 지원은 이번 구현으로 주장하지 않는다.
6. 읽기 없는 공개 쓰기도 같은 direct apply 경로를 사용한다. 새로운 intent 문법이나 W0/W1 기본 선택을 도입하지 않는다.
7. action map·기존 generic core·같은 transport를 사용한다. 새 IR·쓰기 엔진·호환 협상 체계를 만들지 않는다.
8. Id 표현·최종 binding/hash API·표준 쓰기 조합 범위는 창시자 OPEN으로 유지한다. 독립 실험 결과를 승인으로 승격하지 않는다.

## 최소 검증과 반증 기준

- 공개 `Resource.transition` 이름, ids/where 허용, bulk 상한, where의 실제 허용 eq 조건과 값 타입을 생성한다. V3는 eq 외 연산자를 거부하므로 읽기 FilterItem 전체를 재사용하지 않는다.
- 읽기 공개 없는 action을 포함한다. 닫힌 action·둘 다/없는 target·허용 밖 where·잘못된 값·다른 Id wire가 tsc 오류인지 확인한다. 동적 배열 길이·중복·범위·권한은 서버 검증으로 남긴다.
- 같은 binding만으로 apply 요청과 readonly changed/unchanged Id 결과가 추론되게 한다. 실패 결과와 응답 유실 복구 정보를 실제 transport 동작대로 보존한다.
- 현재 성공 응답은 tags만 검사한다. changed/unchanged 배열과 후보별 Id 요소를 검사한 후 타입을 제공한다. 잘못된 성공 응답은 앞선 커밋 여부를 모르는 pending으로 보존하며 같은 키 정상 재생으로 복구한다.
- public action·target·where·상한 변경은 fingerprint를 바꾸고 내부 정책/docs 변경은 바꾸지 않는 대조를 만든다. 이전 binding의 첫 쓰기는 DB 전에 거부한다.
- 실제 PG/HTTP에서 ids·where·반복 unchanged·다른 actor 거부·응답 유실 재생·캐시 무효화를 검증한다. 정상/음성 tsc와 핵심 방어 고장 주입을 포함한다.

성공은 공개 계약의 중복 입력과 실제 타입 공백을 줄였다는 증거다. 개발 시간 절감률·전체 SDK 완성·전체 write surface 타입화는 별도로 측정한다.
