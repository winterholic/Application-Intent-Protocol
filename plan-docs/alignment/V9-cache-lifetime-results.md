# V9. 캐시 수명·미확정 쓰기 복구 결과

> 상태: 작은 실험 검증, 기술 후보(2026-10-04). 제품 TTL·세션 갱신·실시간 동기화 결정이 아니다. [계획과 설계 관문](V9-cache-lifetime-plan.md), [독립 검토](../reviews/codex-v9-sol-review.md).
> 본 `crates/`는 보존했다. 변경은 V5 SDK와 V6 전송 spike 및 관련 검증·문서에 한정한다.

## 확인한 동작

| 경계 | 실행 결과 |
|---|---|
| 세션 만료와 캐시 | 서버가 검증한 `exp`의 잔여 밀리초 이하만 캐시. 만료 뒤 재조회는 `TOKEN_EXPIRED` |
| 시간 의존 읽기 | 고정 NOW 제거, DB `clock_timestamp()`로 현재 시각 전달. 마감 모집 제외. 현재 planner의 now 매개변수가 있으면 `maxAgeMs=0`, 저장하지 않음 |
| 비시간 읽기 | 응답 `maxAgeMs=min(1000ms 실험 상한, 세션 잔여 수명)`. 익명 읽기는 실험 상한만 적용 |
| 왕복시간·클라이언트 시계 | 요청 시작 단조 시각 + 서버 수명으로 만료 계산. TTL 누락·비정상 값·수명보다 느린 응답은 미저장 |
| 다른 연결의 데이터 변경 | 알림 캐시 hit 확인, 별도 연결의 쓰기, 상한 뒤 재조회로 실제 변경 확인 |
| 다른 연결의 권한 회수 | Permission을 회수한 직후에는 기존 ManagerNote 캐시가 남는 것을 확인. 상한 뒤 조회는 정책 재평가로 빈 목록. DB granted=false 직접 확인 |
| 같은 미확정 키 반복 | pending은 키당 하나, 확정 뒤 캐시 저장 재개 |
| 응답 유실 뒤 인증 실패 | 만료·인증 실패는 이전 쓰기의 롤백 증거가 아님. pending 보존 |
| 같은 키 동시 호출 | 진행 중 전송과 결과 공유. 다른 본문은 기존 복구 요청을 덮어쓰지 않고 `IDEMPOTENCY_MISMATCH` |
| 입력·응답 계약 | 최초 JSON 고정. 호출자 객체·오류 객체 변경이 재시도 본문을 바꾸지 않음. JSON 직렬화 실패는 전송 전 오류. 잘못된 성공 응답은 미확정 유지 |
| 인증 입력 크기 | 요청/헤더 라인 8KiB, 헤더 총량 16KiB, Bearer 1KiB 실험 상한. 초과 입력은 본문 대기·DB 접근 전에 `BAD_REQUEST` |

`IDEMPOTENCY_MISMATCH`의 근거를 구분했다. 같은 pending 키에 다른 요청을 보내려는 클라이언트 호출은 원래 pending을 보존한다. 서버가 재요청에 MISMATCH를 응답하면 해당 JSON이 그 키의 성공 결과로 기록되지 않았다는 증거이므로 그 요청의 미확정은 풀 수 있다. 이 해석은 현재 성공 기록·키 보존·actor 분리·잠금 직렬화에 의존한다.

## 재현과 검증

구현 전 Node 캐시 반례 4개, 복구 반례 3개, 같은 키 동시 호출·직렬화 반례 2개, 잘못된 응답 반례 1개가 각각 실패했다. 기존 객체 변조 방어는 통과 대조로 유지했다. 헤더 초과 3사례는 응답 없이 2초를 넘겼고, 유한 수명 HTTP 계약도 정보 누락으로 실패했다. 구현 후 같은 테스트를 다시 실행했다.

```bash
node --test spikes/spike-v5-sdk/sdk/cache.test.ts spikes/spike-v5-sdk/sdk/lifetime.test.ts spikes/spike-v6-transport/client/recovery.test.ts
# tests 17, pass 17, fail 0
/Users/winterholic/.cargo/bin/cargo test --offline --manifest-path spikes/spike-v6-transport/Cargo.toml --test v9_lifetime -- --nocapture
# Node tests 4, pass 4, fail 0; Rust 1 passed
/Users/winterholic/.cargo/bin/cargo clippy --offline --manifest-path spikes/spike-v6-transport/Cargo.toml --all-targets -- -D warnings
# Finished dev profile
spikes/spike-0-ts/node_modules/.bin/tsc --strict --noEmit --target ES2023 --lib ES2023,DOM --module nodenext --allowImportingTsExtensions spikes/spike-v6-transport/client/transport.ts spikes/spike-v5-sdk/sdk/cache.ts
# exit 0
```

V1부터 V7까지 독립 crate 전체를 순서대로 실행해 실패 0건을 확인했다. 각 실험은 전용 schema를 사용하고 종료 시 삭제한다. 새 Node 사례도 V5/V6의 cargo test에서 실행하도록 연결했다.

## 보장 경계와 다음 과제

- 1000ms는 비교 실험값이며 제품 기본값이 아니다. 다른 연결의 변경·권한 회수는 그동안 오래된 값이 반환될 수 있다. 즉시 동기화·구독·연결이 끊긴 동안 놓친 변경 복구는 미구현이다.
- 읽기는 조회 시점의 스냅샷이다. 서버가 응답을 만든 뒤 네트워크 이동 중 기한이 지난 행은 한 번 반환될 수 있다. 미저장과 응답 시점 권한 재검증은 다른 보장이다.
- `now` 탐지는 현재 생성기가 값을 반드시 매개변수화하는 경로에 의존한다. 일반식의 다음 변화 시점을 추측하지 않았다. 이후 명시적 시간 의존성과 데이터 유효 시각을 비교할 수 있다.
- V9 당시 클라이언트 토큰은 고정이어서 만료 후 pending을 같은 actor 새 연결에 수동 재송신해야 했다. 이후 [V10](V10-session-recovery-results.md)에서 서버 principal 확인·같은 actor의 `replaceSession`·pending 자동 재송신을 실행했다. 실제 refresh-token 제공자와 영속 복구는 남는다.
- 헤더 크기 상한만으로 느린 연결·접속 수·토큰 발급 TTL의 비정상 입력까지 검증한 것은 아니다. 키 교체·OIDC·Cookie/CSRF·tenant는 V8의 범위 밖으로 남는다.
- 본 crates 통합과 최종 문법·공개 프로토콜 확정은 진행하지 않았다.
