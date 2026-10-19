# 포팅 공통 제품 갭 조사 — WBS 8.19

2026-10-19, **조사 진행 중**. 기존 앱 포팅 기록의 미지원 서술을 현재 제품 경로와 대조한다.
구문 검사·소스 판독·DB/HTTP 동작·원본 동등성을 구분하며, 미실행 검증은 완료로 세지 않는다.
우선순위는 작업 순서이며 새로운 1.0 필수 기능이나 창시자 승인을 뜻하지 않는다.

입력은 [FlaskBB G1~G11 고정 원문](https://github.com/winterholic/Application-Intent-Protocol/blob/83f3bdb7e92c9bf1778c3cecf00a214002ec406f/porting/flaskbb/PRODUCT-GAPS.md),
[Infisical 갭](INFISICAL-GAPS.md), [FaceReview fixture](../../product/tests/fixtures/facereview-mysql/README.md)다.

| 후보 | 현재 판정과 다음 확인 | 우선순위·기존 WBS |
|---|---|---|
| 물리 삭제·참조 정리 | 제품 delete 전이는 존재한다. 파서의 `delete_effects` 회귀와 FaceReview 취소 의미를 앱별 삭제 정책·감사·FK 검증과 구분한다. 과거의 문법 부재 서술을 전체 앱 삭제 완료로 바꾸지 않는다 | 먼저 기존 증거 대조, 7.2.4·7.3·7.5 |
| 음수 Int 범위 | `Int(-1000..1000)` 거부를 재현 후 수정. 5형식 같은 실행 계약, 역전·overflow·unsigned 문맥 거부, operation 안전 정수 영역, 쓰기 값 양 끝/초과 검증. 아래 실행 범위 참조 | 작은 공통 결함부터 처리, 7.2.3 |
| Bool 정렬 | 루트·1:N 정렬의 Bool 거부를 재현 후 수정. false/true·NULL·asc/desc·id 동률·cursor 값 타입과 행 정책 회귀를 추가했다. 로컬 PostgreSQL initdb는 Codex 명령 샌드박스의 shmget 권한 거부로 차단돼 실DB 검증은 Linux CI에 남는다. MySQL은 SQL·바인딩 검사이며 실DB 동작으로 합산하지 않는다 | 작은 공통 결함부터 처리, 7.5.2 |
| 상수 기본값 | 파서는 `default database`만 지원한다. create 입력 생략·DDL/adopt·기존 행·계약 hash를 함께 정해야 한다 | 후속 계약 필요, 7.2.3·7.3·7.5.2 |
| OR·total | 현행 FaceReview 정의에 OR 필터와 total aggregate가 있다. 목록/total을 한 응답으로 주는 원본과 호출 형태가 다르다. 이번 변경에서 실DB oracle 재실행 안 함 | 기존 구현 재사용, 7.2.3 |
| 공유 기본키·JSON | 현행 fixture의 `id: User`와 Json 지원을 기존 MySQL 증거와 대조한다. Ref 정렬 거부를 공유 PK 자체 거부로 오인하지 않는다. MariaDB longtext는 별도 차이다 | 기존 구현 재사용, 7.2.3 |
| 전문검색 | contains/like의 collation 차이와 Whoosh 토큰화·순위를 구분한다. row policy·쓰기 직후 일관성·비용 상한을 포함한 실DB 비교가 남음 | 검색 트랙, 7.5.3 |
| 삭제 후 last-post | bounded update 효과와 남은 최신 행 선택은 다른 계약이다. 같은 transaction의 선택·잠금·동시 삭제·tie-break·rollback 확인이 남음 | 앱 전이 트랙, 7.5.2 |
| create 관련 행·결과 연쇄 | expose create 관련 효과와 일반 결과 id binding은 미지원 후보다. 기존 compose item 제한과 구분하고 두 요청 사이 실패를 원자 성공으로 세지 않는다 | 설계·구현 트랙, 7.3.2·7.3.3 |
| patch 대상값 인가 | 제안된 input 경로 거부만으로 가능한 모든 표현 부재를 증명하지 않는다. 변경 전/후 평가·관련 행 정책·write skew 반례가 남음 | 앱 권한 트랙, 7.5.2 |
| 메일 provider | 관리 HTTP capability는 있고 격리 worker의 직접 SMTP와 다르다. 전용 preset·템플릿·수신 동의 계약은 별도 후보 | 기존 관리 HTTP 재사용, 7.5.3 |
| EFFECT_UNKNOWN 복구 | HTTP send 오류 분류와 운영자 확정 명령의 공백을 검토한다. 전송 전 실패·부분 전송·응답 손실의 장애 주입은 이번 변경에서 미실행 | 운영 장애 트랙, 7.5.3 |
| 읽기 캐시 | TS client의 최대 1초 캐시와 동일 client 쓰기 무효화를 다른 client·외부 DB 변경 통지와 구분한다. 두 client 시간 측정이 남음 | 통합 판정, 7.4 |
| shared_id 유일성 | 앱 정의·실DB 제약·main 충돌을 함께 확인한다. 제품 기능 task를 중복 생성하지 않는다 | 기존 차단 항목, 7.5.5 |
| 암호화·blind index | 평문 임시 필드로 해결하지 않는다. SRP 책임 분리·EE dynamic secret 제외는 구현 완료가 아닌 범위 분류다 | 별도 설계·구현 게이트, 8.17·8.18 |

## 음수 범위 변경의 확인 범위

- `spikes/spike-v1-fixture/tests/signed_int_ranges.rs`: 5형식 실행 계약, 음수 양 끝·0·안전 정수 경계, 잘못된 범위와 부호 문맥을 확인한다.
- `spikes/spike-v3-write/src/lib.rs`의 `signed_range_definitions_enforce_negative_create_value_boundaries`: 실제 파서 facts를 쓰기 값 검사에 넘겨 -1000/-1/0/1000을 허용하고 범위 초과·소수·문자열·null을 거부한다.
- 실제 `aip service check/gen`으로 두 wire의 TS/Python binding 생성과 같은 계약 fingerprint, 범위 변경 시 fingerprint 변경을 확인한다. Python binding은 서버에 범위 검사를 위임한다.
- 이번 변경의 DB/HTTP/MySQL 원본 대조는 미실행이다. `user_point_history.amount`의 원본에 없는 상한을 새로 강제하지 않는다.
- 실제 실행 명령·출력은 작업별 검증 기록에 남긴다. CI 결과와 전체 WBS 완료 판정은 별개다.

남은 후보의 실제 실행 증거와 기존 WBS 대응을 모두 닫기 전까지 **8.19는 doing**을 유지한다.
