# V12. 생성 계약과 실제 프론트 SDK 연결 계획

> 상태: 아래 후보를 독립 spike에서 구현·실행했다. [결과·한계](V12-typed-transport-results.md)와 [독립 검토](../reviews/codex-v12-sol-review.md)를 따른다. [V5](V5-sdk-results.md)의 선택형 Row 추론과 [V6](V6-transport-results.md)·[V10](V10-session-recovery-results.md)의 HTTP/캐시/세션 경로를 연결한다. 제품 프로토콜·출시 API를 확정하지 않는다.

## 현재 증거

- V5 `sdk/aip.ts`는 특정 생성 `contract.ts`를 import하며 `client(send).read`가 선택형 배열을 반환한다.
- V6 `client/transport.ts`는 cache만 재사용한다. `connect().read(query:unknown)`의 envelope.rows는 unknown이고 실제 e2e 호출부에도 추가 cast가 있다.
- V6 fixture는 MemberAlarm을 추가하므로 V5에 체크인된 특정 Contract를 직접 import해서는 이 서버의 계약을 표현하지 못한다.

## 설계 관문

1. 원칙 1·2·4·6: 화면마다 타입 단언·결과 mapper를 다시 쓰는 일을 줄인다. 타입 단언만 추가하고 실제 계약 연결을 검증하지 않으면 원칙 4와 긴장한다.
2. 호출자가 표현하는 read/apply 범위는 유지한다. 생성 계약 아래 선택형 rows를 추론하고 캐시 envelope 정보도 보존한다.
3. 기존 서버 facts 한 곳에서 앱별 계약을 생성한다. 화면별 endpoint나 중복 타입 선언을 추가하지 않는다.
4. 서버 raw JSON의 정책·타입·비용 검사는 유지한다. 생성 계약 식별값의 일치는 권한이나 응답 내용 전체 검증을 대신하지 않는다.
5. V5 타입 연산을 생성 Contract에 대해 일반화하고 V6 cache/세션/쓰기 복구를 재사용한다. 이번 TS slice를 Python 클라이언트 검증으로 표시하지 않는다.
6. 앱의 생성 계약을 한 번 결합하는 후보를 비교한다. V5 fixture 편의 adapter와 공유 타입 core를 유지하며 화면별 wrapper를 늘리지 않는다.
7. V1 facts→V5 생성물→V6 실행 경로를 연결한다. 새 IR·독자적 타입 시스템을 만들지 않는다.
8. fingerprint 표현·최종 API·Id 표현·구버전 호환 기간은 기술 후보로 남긴다. 본 `crates/` 통합이나 기존 인증 정책 변경은 하지 않는다.

## 후보와 반례

- 앱별 생성 Contract를 명시 결합한 SDK read가 선택형 rows와 cached/stale/stored 정보를 반환한다. 캐시가 실제로 동결하는 값에 맞는 읽기 전용 결과 타입을 검토한다.
- MemberAlarm을 포함한 V6 facts에서 계약을 생성하고, 캐시 miss/hit의 결과 필드를 추가 cast 없이 읽는다. 닫힌 필드/filter/sort/root는 tsc에서 거부된다.
- 관계 선택·조건부 select union·길이를 모르는 배열의 optional 필드가 V6 envelope에서도 V5와 같은 의미를 유지한다.
- 런타임 계약 일치를 주장하는 후보는 매 read 응답의 fingerprint를 캐시 저장 전에 대조한다. `/session`에서 한 번 확인하는 것만으로는 서버 교체 뒤의 다른 계약을 검출하지 못한다.
- fingerprint 누락/불일치/다른 서버 계약은 명시 오류이며 저장하지 않는다. fingerprint 일치는 행 내용의 구조 검증이나 호환성 증명이 아니다.
- V5 타입·V6/V9/V10 전송/캐시/세션 회귀, 실제 HTTP/PG, tsc 정상·음성 대조와 고장 주입을 수행한다.

실험은 공개 읽기 타입 산출물의 SHA-256을 식별값 후보로 사용했다. 내부 정책만 변경한 경우 동일하고 공개 필드 변경은 달라지는지 실행 대조했다. 코드 대조와 테스트는 Luna high가, 최소 연결·보장 경계·실제 우회 독립 리뷰는 Sol high가 맡았다. 최종 fingerprint 표현과 API 승인은 남는다.
