# V8. 인증 실험 결과

> 상태: 검증 필요 (2026-10-04). 의제 5(인증·인가) 중 "요청의 actor를 서버가 어떻게 믿는가"를 V6 전송에 붙여 작게 실행한 기록이다. 로그인 방식·OIDC·세션 정책 결정이 아니다.
> 위치: `../../spikes/spike-v6-transport/src/auth.rs`, `tests/v8_auth.rs`. V6의 시험용 `x-spike-actor` 헤더는 제거했다.

## 실제 실행 출력

명령: `cd spikes/spike-v6-transport && cargo test --offline -q`

```text
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.30s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
```

첫 줄은 V6 e2e(토큰으로 바꾼 뒤 7개 그대로 통과), 둘째 줄은 V8 인증 테스트 2개다.

고장 주입: 서명 검사를 건너뛰면 변조 토큰·다른 키 토큰이 통과해 실패. 검증 실패 토큰을 익명으로 낮추면 변조·다른 키·깨진 토큰 사례가 실패. 복원 후 통과.

## 1. 구조

- payload JSON = `{sub: actor, iat, exp}`. 토큰은 `segment = base64url(payload JSON)`과 `base64url(HMAC-SHA256(서버 키, segment의 바이트))`를 `.`으로 잇는다. 서명 입력은 첫 번째 인코딩 문자열의 바이트다.
- 서버 키는 기동할 때 무작위 32바이트로 만든다. 코드·설정 파일에 두지 않는다. 서명 비교는 상수 시간.
- 요청의 actor는 `authorization: Bearer <토큰>`에서만 온다. 토큰이 없으면 익명, 있는데 검증에 실패하면 익명으로 낮추지 않고 거부한다.
- 시험에서는 서버가 돌려준 발급기가 로그인 제공자 역할을 한다. 실제 신원 확인은 범위 밖이다.

## 2. 결과

| 사례 | 결과 |
|---|---|
| 정상 토큰(회원 1) | 회원 1의 알림만 보임 |
| 토큰 없음 | 익명(알림 0개) |
| payload의 회원만 2로 바꾼 토큰 | `UNAUTHENTICATED` |
| 다른 서버 키로 서명한 토큰 | `UNAUTHENTICATED` |
| 만료 토큰 | `TOKEN_EXPIRED` |
| 형식이 깨진 토큰 | `UNAUTHENTICATED`(익명 결과도 주지 않음) |
| 옛 `x-spike-actor: 2` 헤더 | 무시되고 익명 |
| authorization 헤더 두 번 | `BAD_REQUEST` |

V8 당시 V6의 멱등 키는 (actor, key)로 저장했다. signed -1과 익명 sentinel의 충돌이 [r9](../reviews/codex-v8-r9.md)에서 발견됐고, [V10](V10-session-recovery-results.md)은 인증 actor와 익명 principal namespace를 분리한 뒤 실제 HTTP/DB로 재생 차단을 확인했다.

## 3. 하지 않은 것

실제 로그인·OIDC 연동, 키 교체·여러 서버 공유 키, 토큰 폐기(로그아웃 즉시 무효화), 대리 접속(관리자가 다른 사용자로 행동), tenant, 갱신 토큰, 브라우저 저장 위치(쿠키·CSRF). 기존 PoC에는 HMAC 토큰·대리 접속·tenant 구현이 있으며([04](../04-current-state.md) §2) 이번 실험과 비교하지 않았다.

## 4. 독립 검토와 V9 후속 검증

Luna high가 단위 토큰 검증과 실제 HTTP/PG 인증 테스트를 따로 실행했다. 검토 범위에서 인증 우회는 찾지 못했고, HTTP 헤더·Bearer 입력 상한 누락을 확인했다. 초과 입력 3사례는 본문 전 응답 없이 대기해 실패했으며, V9에서 상한을 적용한 후 통과했다.

[V9 결과](V9-cache-lifetime-results.md)에는 세션 잔여 수명 이하의 캐시 재사용과 읽기 처리 중 만료 거부를 추가했다. 메인/Sol/Luna의 실제 검토 범위는 [독립 기록](../reviews/codex-v9-sol-review.md)을 따른다. `iat` 미검증을 현재 신뢰 발급기 범위의 인증 우회로 분류하지 않았다. 발급 TTL 비정상 입력·키 교체·실제 로그인/refresh 정책은 남은 과제다. [V10](V10-session-recovery-results.md)은 같은 actor 새 토큰 교체와 pending 복구를 별도로 검증했다.

## 변경 이력

- 2026-10-04 V8 서명 토큰 인증 실행 결과.
- 2026-10-04 Luna high 독립 인증 검토, V9 입력 크기·세션 수명 보강 연결.
- 2026-10-04 HMAC 서명 입력 설명을 실제 인코딩 segment와 맞춤. V10 토큰 교체·복구와 signed -1/익명 분리 연결.
