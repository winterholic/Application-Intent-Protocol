# V15. 공개 필터와 실제 실행 범위 연결 결과

> 상태: 독립 실험 검증, 기술 후보(2026-10-04). [8관문·계획](V15-filter-value-plan.md), [독립 검토](../reviews/codex-v15-sol-review.md). 본 `crates/`·최종 API·Id 표현을 변경하지 않았다.

## 실제로 바뀐 것

V1이 공개한 Url·Enum·Time 조건을 V2 읽기와 V3 direct apply where가 같은 scalar 검사로 받는다. V5 쓰기 타입도 이 지원집합에서 생성한다. Enum은 선언된 멤버만, Url은 현재 text 저장 의미의 문자열만 받는다. URL 문법·프로토콜 정책을 새로 정하지 않았다. 기존 Id/Ref wire 분기·행 정책·필드/연산 allowlist·트랜잭션은 유지한다. 쓰기 where는 eq만 받는다.

이미 공개한 Text.prefix도 연결했다. `left(column, char_length(parameter)) = parameter`로 한글·빈 접두어·`%`·`_`·역슬래시·따옴표를 비교한다. 문자열은 SQL 패턴이 아니며 매개변수다. 인덱스 사용·모든 collation에서 바이트 동일성은 보장하지 않는다.

V2의 공통 값 검사는 기존 단독 집계 input에도 연결되지만, 새 scalar의 집계별 HTTP 왕복을 전부 검증한 것은 아니다. V3 create/compose의 기존 literal 검사에는 연결하지 않았다.

## Time 검사와 저장 의미

로컬 `chrono 0.4.45`를 clock/OS 시간대 기능 없이 사용한다. [Chrono 공식 parser](https://docs.rs/chrono/0.4.45/chrono/struct.DateTime.html#method.parse_from_rfc3339)에 앞서 기본 형태·명시 offset·ASCII·입력 연도와 기존 20..35바이트 상한을 확인한다. 불가능한 날짜·시간·offset은 SQL 전에 BAD_VALUE다. 소문자 t/z도 대조했다.

UTC 초 부분만 변환하고 소수초는 원문 그대로 바인딩한다. [PostgreSQL 17 날짜·시간 문서](https://www.postgresql.org/docs/17/datatype-datetime.html)의 저장 정밀도와 실제 DB 결과를 기준으로 했다. `.1234565001Z`를 Chrono가 9자리로 줄이면 PG의 반올림 결과가 달라지는 독립 반례를 확인했다. 9/10자리 입력의 의미는 DB 마이크로초 반올림이며 나노초 저장 보장이 아니다.

UTC 변환으로 생긴 연도 0은 PostgreSQL의 1 BC, 연도 10000은 무부호 연도로 바인딩해 기존 유효 입력을 거부하지 않는다. +23:59·연도 경계·다음 초/다음 해로 반올림되는 값을 실제 조회로 확인했다. `:60`은 기존 PG 정규화 의미를 보존하며 실제 윤초 발생 여부를 검증하지 않는다. BC/확장 연도 응답을 JS Date·Python datetime·SDK 값으로 다시 요청하는 전체 왕복은 미검증이다. 최종 Time domain·직렬화 정책 승인이 아니다.

## 실제 반례와 검사 보강

- 구현 전 V2 4개 테스트 중 1 통과/3 실패: 정상 Url 거부, 불가능한 Time 계획 수용, 큰 offset/소문자 실행 불일치. 첫 PG 테스트의 동일 timestamp 행 중복 기대는 fixture 오류로 구분해 id 조건을 추가한 뒤 다시 red를 확인했다.
- Luna high가 V5 테스트만 작성했다. 4개 중 3 통과/1 실패는 Url where가 never인 실제 미구현이었다. main이 fixture에 title.eq도 있다는 사실을 대조해 prefix-only projection 기대를 바로잡았다.
- V6 실제 HTTP baseline은 각 wire에서 3개 중 1 통과/2 실패였다. 정상 값 거부와 구 projection의 첫 쓰기 수용을 실제 DB에서 확인했다.
- Sol의 NUL 지적을 실제 HTTP로 재현했다. 읽기는 BAD_VALUE지만 Text where 쓰기는 INTERNAL을 거쳐 WriteUnsettled가 됐다. read/where 공통 검사에서 NUL을 사전 거부한 뒤 pending=[]를 확인했다. 텍스트 입력 전면 차단으로 확대하지 않는다.
- Sol이 Time 길이 상한 제거를 지적해 35바이트 수용/36바이트 거부의 red를 확인하고 기존 경계를 복원했다.
- prefix는 planner와 실제 HTTP에서 미지원 red를 먼저 관측했다. SQL LIKE 고장 주입이 처음에는 HTTP 테스트를 통과해, `%`·`_` 단독 접두어 대조를 추가했다. 보강한 테스트에서는 실제 다른 행 매치로 실패했다.

## 검증 증거

명령은 각 crate에서 `/Users/winterholic/.cargo/bin/cargo test --offline -- --nocapture`다.

| 범위 | 최신 실행 |
|---|---|
| V2 읽기 | Rust 19 통과/실패 0. 새 7개에 실제 PG Time 9조건 포함 |
| V3 쓰기 | 기존 Rust 5 통과/실패 0 |
| V5 생성 SDK | Rust 17 통과/실패 0. V15 생성기 4개 포함 |
| V6 HTTP/SDK | Rust 12 통과/실패 0. V15 safe/string 각각 Node 5 통과/실패 0 |
| 기존 HTTP 회귀 | V14 각 Node14, V13 4/1/3, V12 11, 세션3, 복구17, 기존 HTTP7, 수명4 실패 0 |
| 선택적 개발 조언 | V11 Rust9 통과/실패0, PG 정책24조합 유지 |
| 정적 타입 | V15 strict tsc 정상·8개 음성 marker·marker 제거 대조, 두 Id binding 포함 |
| 독립 검증 | Sol V2 새 테스트6 통과/실패0 및 prefix SELECT 대조. 이후 main이 prefix 포함7개·전체 회귀 실행 |

V2 기존 매개변수 테스트는 `Z` 원문 보존을 기대해 한 번 실패했다. 정규화한 UTC 값도 SQL문에 들어가지 않고 매개변수에만 존재하는 검사로 갱신한 뒤 전체 19개를 재실행했다.

V14 생성기 원본을 임시 Rust target에 연결해 V15 구 binding의 타입 바이트·지문을 직접 대조했다. 1 통과/실패0 후 임시 target을 제거했다. 고장 주입 7종은 Enum 멤버십, 달력 검사, 소수초 보존, NUL 검사, 길이 상한, 쓰기 parser 연결, 리터럴 prefix다. 모두 실제 행동 실패를 확인한 뒤 원복했다. 첫 prefix 검출 누락과 보강도 위에 기록했다.

## 남은 제품 범위

create/compose 값·nullable 조건·일반 Int 정밀도·전체 행 decoder·Python SDK·worker 계약 및 운영 격리·설치 패키지·정의 변경/재기동·영속 pending 복구·변경 통보는 이 실험으로 달성되지 않는다. 본 구현의 이름난 Intent 호출과 caller 표현 후보도 아직 통합되지 않았다.

다음은 V번호별 실험 추가 자체보다, 검증된 엔진과 생성 SDK를 한 실행 가능한 개발자 흐름으로 연결하는 것이다. 초기화→계약 생성→서버→typed read/apply→정의 변경·재생성의 실제 동작과 반복 비용을 확인한다. 기존 실험을 전체 초기 프로토타입 완성으로 표시하지 않는다.
