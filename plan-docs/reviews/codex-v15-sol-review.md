# V15 독립 검토와 메인 판정

> 2026-10-04. 단순 조사·생성기 테스트는 Luna high, 설계·독립 검증은 Sol high. 구현·실제 HTTP/DB·고장 주입·회귀는 메인. [결과](../alignment/V15-filter-value-results.md).

Sol은 V2 read와 V3 where의 scalar 검사를 공유하되 기존 Id 분기와 create literal을 보존하도록 권했다. 실제 코드를 대조해 범위를 나눴다. V2 단독 aggregate input에도 공통 검사가 적용된다는 파급을 기록했다.

Chrono와 PG 정밀도 차이는 읽기 전용 SELECT로 확인했다. 원문 `.1234565001Z`와 9자리 재직렬화의 저장 결과가 달라, UTC 초+원문 소수초를 바인딩했다. 연도 0/10000 표기는 메인이 실제 PG 테스트로 대조한 뒤 처리했다. 가짜 윤초도 PG가 수용하므로 실제 윤초 검증으로 소개하지 않는다.

Time 입력 상한 누락은 메인이 35/36바이트 실패 대조를 작성하고 복원했다. NUL 경로는 Sol의 코드 가설을 실제 V6 쓰기의 WriteUnsettled로 재현한 뒤 read/where에서 사전 거부했다.

Sol의 최신 독립 명령:

```text
cargo test --offline --manifest-path spikes/spike-v2-read/Cargo.toml --test v15_filter_values -- --nocapture
test result: ok. 6 passed; 0 failed
```

이후 추가한 prefix는 V1 Text 허용·V2 한 번 바인딩·NULL/빈 문자열·V3 eq-only를 코드로 검토하고 읽기 전용 PG SELECT에서 Unicode·와일드카드 문자·NULL을 대조했다. 메인 회귀와 충돌하지 않도록 V6 schema 테스트는 중복 실행하지 않았다. collations 전반의 바이트 동일성이나 인덱스 성능 검증은 아니다.

Luna는 타입 테스트의 SafeNumber binding, 다른 actor where 쓰기, 요청별 DB 불변 대조 누락을 지적했다. 메인이 두 binding의 정적 음성 대조, 다른 actor의 조건+id 쓰기와 전체 행 상태 snapshot을 보강했다. 다른 actor가 자기 행99를 읽는 것은 정상 정책이므로 초기 빈 배열 기대를 바로잡았다.

최종 API·Time domain·Id 표현·공식 패키지 출시와 창시자 OPEN을 에이전트 합의로 승인하지 않는다. 전체 프로토타입에서는 실행 예제와 생성 SDK, 공식 확장 경로의 통합 검증이 남는다.
