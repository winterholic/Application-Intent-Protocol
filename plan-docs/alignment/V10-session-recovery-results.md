# V10. 세션 갱신·미확정 쓰기 복구 결과

> 상태: 독립 실험 검증, 기술 후보(2026-10-04). [설계 관문](V10-session-recovery-plan.md), [독립 검토](../reviews/codex-v10-sol-review.md). 로그인 제공자·최종 공개 프로토콜·제품 인증 정책은 확정하지 않는다.
> 변경은 V6 전송 spike와 검증·연결 문서에 한정했다. 본 `crates/`는 보존했다.

## 확인한 동작

| 경계 | 실행 결과 |
|---|---|
| 최초 principal 확인 | read/apply 전에 `/session {}`을 공유 호출. 서버는 서명·만료를 검증하고 DB 연결 전에 actorId와 잔여 수명을 반환. 실패하면 apply를 전송하거나 pending을 만들지 않음 |
| actor 표현 | actorId는 정규 10진 i64 문자열. JS 숫자·비정규 문자열·범위 밖 값은 SDK가 거부. 양쪽 i64 경계값은 문자열로 유지 |
| 같은 사용자 갱신 | `replaceSession(token)`이 새 토큰을 서버에서 확인하고 같은 principal일 때 교체. pending을 최초 키·JSON으로 자동 재송신 |
| 실제 만료 뒤 복구 | 3초 토큰으로 승인 커밋 후 응답 유실, 3100ms 대기, 서버 `TOKEN_EXPIRED` 확인. pending 보존 뒤 같은 actor의 새 토큰으로 replay. DB 회원 생성·outbox 각각 1건 |
| 다른 사용자·실패 토큰 | 교체 거부, 이전 연결·pending 보존. 실패한 교체는 apply를 재전송하지 않음 |
| 인증 세대 | 교체 시 캐시 폐기. 이전 세대의 늦은 read는 `ScopeChanged`. 늦은 apply는 새 세대가 이미 확정한 같은 record의 결과만 공유 |
| 동시 갱신·같은 키 | 갱신은 직렬화. 진행 중 apply는 세대별 키로 공유하므로 옛 요청의 종료가 새 요청을 지우지 않음 |
| 첫 실행 전 거부 | 최초 전송의 명시적 admission 거부는 오류를 반환하고 pending/캐시 보류를 해제. 이전 시도가 미확정이면 같은 거부만으로 이전 커밋을 부정하지 않음 |
| signed actor와 익명 | 멱등 저장소·잠금 scope를 `actor:{id}`와 `anonymous`로 분리. signed actor=-1의 성공을 익명 status/apply로 재생하지 못함 |

principal 확인은 현재 업무 권한의 증명이 아니다. 매 read/apply의 서버 정책을 다시 평가한다. 미확정 record는 실제 전송 전부터 등록하여 진행 중 쓰기에 따른 캐시 저장도 막는다. 전송 시도 횟수는 앞선 시도를 확정하지 못했다는 보수적 근거이며 DB 실행 횟수가 아니다.

## 재현과 검증

구현 전 세션 단위 반례 6개는 `replaceSession` 부재로 실패했다. 실제 서버 테스트도 `/session` 부재와 signed -1/익명 멱등 scope 충돌로 실패했다. Sol이 발견한 최초 admission 거부는 메인이 별도 반례로 재현했고, 처음에는 Node 15개 중 1개가 실패했다. 첫 거부와 이전 유실 후 거부를 구분한 뒤 같은 테스트가 통과했다.

```bash
node --test spikes/spike-v5-sdk/sdk/cache.test.ts spikes/spike-v5-sdk/sdk/lifetime.test.ts spikes/spike-v6-transport/client/recovery.test.ts spikes/spike-v6-transport/client/session.test.ts
# tests 27; pass 27; fail 0
/Users/winterholic/.cargo/bin/cargo test --offline --manifest-path spikes/spike-v6-transport/Cargo.toml --test v10_session -- --nocapture
# Node tests 3; pass 3; fail 0; Rust 1 passed
```

V6 전체 회귀는 V9/V10 복구 단위 17개, 기존 HTTP/PG 7개, V9 수명 4개와 V10 세션 3개를 cargo에서 실행한다. 인증·헤더 Rust 테스트도 함께 실행한다. strict tsc와 Rust fmt/clippy는 해당 SDK/전송 코드에 적용한다.

마지막 전체 실행은 V6 Rust 테스트 7개, V5 Rust 테스트 4개가 실패 0건이었다. V5의 타입 음성 대조는 의도한 컴파일 오류를 확인한 뒤 성공 판정했다. V6의 Node 하위 테스트는 17·7·4·3개가 각각 실패 0건이었다. strict tsc·fmt check·clippy `-D warnings`도 exit 0이고, DB 조회는 `remaining spike schemas: 0`이었다.

기존 문서 검사기의 고정 목록은 새 실험 문서의 깨진 링크를 놓쳤다. alignment/reviews Markdown을 자동 포함하도록 바꾼 뒤 임시 새 문서의 없는 링크를 거부하는 음성 대조를 실행했다. [V0](V0-validation-results.md)는 구조 검사 범위와 의미 검증 한계를 분리한다.

## 남은 범위

- 새 토큰은 호출자가 기존 로그인 제공자에게서 받아 전달한다. refresh token 발급·저장, OIDC, 키 교체, 다중 서버, Cookie/CSRF, tenant는 구현하지 않았다.
- 최초 principal 확인 전에 이미 만료된 연결은 새 유효 토큰으로 새 연결을 만든다. 확인했던 principal이 있어야 같은 사용자 교체를 검증할 수 있다.
- pending은 연결 메모리에 있다. 브라우저 재시작·프로세스 종료 뒤 영속 복구는 별도 설계가 필요하다. 익명에서 로그인 사용자로 자동 이관하지 않는다.
- `replaceSession` 반환 목록에 없는 키는 아직 미확정일 수 있다. `pending()`을 확인하고 통신 복구 뒤 `retryPending()`으로 다시 시도한다.
- 저수준 `post()` 직접 호출은 tracked apply·pending·캐시 보장 밖이다. 업무 쓰기는 `apply()` 경로를 사용한다.
- `/status`의 NOT_FOUND는 조회 시점에 저장 결과가 없다는 뜻이며 진행 중 쓰기의 롤백 증거가 아니다.
- 멱등 테이블의 principal 열 변경은 새 spike schema에서 검증했다. 기존 배포 테이블의 마이그레이션이나 호환성은 검증하지 않았다.
- V9의 수명 기반 신선도는 유지한다. 즉시 변경 통보·정확한 다음 시간 경계·연결이 끊긴 동안 놓친 변경 복구는 남는다. 제품 TTL과 최종 SDK API는 미정이다.
