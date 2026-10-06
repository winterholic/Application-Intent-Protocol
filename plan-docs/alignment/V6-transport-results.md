# V6. 전송 계층 실험 결과

> 상태: 검증 필요 (2026-10-04). V5 다음 단계로 남았던 "전송 계층·응답 유실 복구"(V5 §8, Codex r5 R5-02)를 최소 범위로 실행한 기록이다. 와이어 형식·HTTP API·인증 결정이 아니다.
> 위치: `../../spikes/spike-v6-transport/`. Rust 최소 HTTP 서버(V2 읽기·V3 쓰기 재사용) + TS 클라이언트(V5 캐시 재사용) + 로컬 PostgreSQL `aip_v6` schema(테스트 끝에 삭제).

## 실제 실행 출력

명령: `cd spikes/spike-v6-transport && cargo test --offline -q -- --nocapture`

```text
ℹ pass 7
ℹ fail 0
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.30s
```

Rust 테스트가 서버를 띄우고 node e2e 테스트 7개(r7 반영 3개 포함)를 실행한 뒤, DB에서 "회원 생성 1건·outbox 1건"을 직접 확인한다.

고장 주입: 서버의 멱등 키 조회를 끄면 node 테스트 2개가 실패했다(같은 키 재요청이 재실행됨). 복원 후 통과.

## 1. 구조

```text
TS 클라이언트 ── POST /read {query} ──────────▶ plan_read + execute → {rows, deps}
   (V5 캐시)  ── POST /apply {request, key} ──▶ 한 트랜잭션: 키 조회 → 전이 실행 → 사후조건 → 키·결과 기록 → 커밋
              ── POST /status {key, request} ─▶ 같은 요청의 기록된 결과, 다른 요청이면 MISMATCH, 없으면 NOT_FOUND(조회 시점 기준)
```

- 쓰기 응답에는 바뀔 수 있는 resource 태그(전이 대상 + 효과 대상)가 들어 있고, 클라이언트 캐시가 그 태그로 무효화한다.
- 멱등 키는 actor별이고, 쓰기와 **같은 트랜잭션**에 결과와 함께 기록한다. 키가 있으면 커밋됐다. 키가 없다는 것은 조회 시점에 커밋된 결과가 없다는 뜻일 뿐, 진행 중일 수 있다(r7 R7-02).
- 같은 키 동시 요청은 키 단위 advisory 잠금으로 하나씩 처리한다.

## 2. 결과

| 사례 | 결과 |
|---|---|
| 알림 목록 읽기 → 알림 읽음 쓰기 → 다시 읽기 | 쓰기 응답 태그 `["MemberAlarm"]`로 알림 목록만 다시 읽고 바뀐 값을 받음. 모집 목록 캐시는 유지 |
| 승인 쓰기의 응답 유실(서버는 커밋 후 연결을 끊음) | 클라이언트: 캐시 비움·저장 보류 → **같은 키·같은 본문으로 재요청** → 저장된 결과로 확정(r7 뒤. 처음에는 `/status`로 확정했으나 R7-01·02 때문에 바꿈) |
| 같은 키로 다시 보냄 | 저장된 결과(`replayed: true`), 재실행 없음 |
| 새 키로 다시 보냄 | `INVALID_STATE`(이미 승인). 처음 한 번만 실행됐다는 뜻 |
| DB 최종 | 회원 생성 1건, outbox 1건 |
| 승인 뒤 모집 목록 | 다시 읽음(승인 태그 `ClubMember`가 목록의 정책 deps와 겹침) |
| 같은 키에 다른 요청 | `IDEMPOTENCY_MISMATCH` |
| 보낸 적 없는 키 상태 | `NOT_FOUND` |
| 다른 actor가 같은 키 조회 | `NOT_FOUND`(남의 결과를 받지 않음) |

## 2.1 Codex r7 반영

[codex-v6-r7](../reviews/codex-v6-r7.md): P1 3, P2 2. 기존 e2e는 재현됐고 같은 키 중복 커밋은 없었다.

| 발견 | 내용 | 반영 |
|---|---|---|
| R7-01 P1 | 같은 키·다른 요청의 MISMATCH 응답이 유실되면 클라이언트가 `/status`로 이전 요청의 성공을 "복구" | `/status`도 요청 일치 확인. 복구는 같은 본문 재요청으로만 |
| R7-02 P1 | `/status`의 NOT_FOUND(진행 중일 수 있음)를 롤백으로 보고 미확정을 풀어 커밋 전 값이 캐시에 남음 | 복구는 같은 키 재요청: 서버가 키 단위로 직렬화하므로 진행 중이면 끝난 뒤 결과를 받음. 확정 응답 전까지 미확정 유지 |
| R7-03 P2 | 재요청까지 통신 실패하면 복구 수단이 없음, 생성 키도 안 알려 줌 | `WriteUnsettled(key)` 오류, `pending()`, `retryPending()`. 서버 JSON `COMMIT_UNKNOWN`·`INTERNAL`도 미확정으로 처리 |
| R7-04 P1 | 1MiB 초과 본문을 잘라서 쓰기 실행 | 읽기 전 `PAYLOAD_TOO_LARGE` |
| R7-05 P2 | GET·잘못된/중복 길이·잘못된 JSON을 구분 없이 처리 | POST만, 길이 1회·숫자만, transfer-encoding 거부, JSON 오류 `BAD_REQUEST` |

고장 주입: 초과 본문 거부, `/status` 요청 일치 검사를 각각 끄면 e2e가 실패했다.

## 3. 드러난 것

- "쓰기 결과 미확정"은 멱등 키를 쓰기와 같은 트랜잭션에 기록하면 클라이언트가 스스로 확정할 수 있었다. V3의 `COMMIT_UNKNOWN`·V5 캐시의 `resolveUnknown()`이 이 경로로 연결된다.
- 전송 계층이 생기자 쓰기 응답이 캐시 무효화의 입력이 됐다. 같은 클라이언트의 쓰기만 보인다. 다른 사용자·다른 기기의 쓰기는 이 구조로 알 수 없다(실시간 통보 또는 주기 재조회 필요, 창시자 선택 "화면 데이터 신선도"와 연결).

## 4. spike가 정한 규칙

| ID | 규칙 | 열린 점 |
|---|---|---|
| V6-R1 | (V8 뒤) actor는 서버 서명 토큰에서만 | [V8](V8-auth-results.md). 실제 로그인·키 교체는 범위 밖 |
| V6-R2 | 멱등 키는 (actor, key), 요청 본문이 다르면 거부, 결과는 쓰기와 같은 트랜잭션에 기록 | 키 보관 기간·정리, 실패 결과도 기록할지(지금은 성공만) |
| V6-R3 | 연결 하나에 요청 하나, POST·정확한 경로·유효한 content-length·전체 JSON만, 본문 1MiB 초과는 거부 | keep-alive·스트리밍·압축 없음 |
| V6-R4 | 쓰기 응답에 변경 가능 resource 태그 | 다른 클라이언트의 변경 통보 없음 |

## 5. 하지 않은 것

실제 로그인·refresh-token 발급, 다른 클라이언트 변경 통보, 재연결 시 놓친 변경 복구, 쓰기 기한(COMMIT_UNKNOWN)과 서버 응답 연결, keep-alive·성능, Python 클라이언트. 인증은 [V8](V8-auth-results.md), 캐시 수명·미확정 복구·헤더 상한은 [V9](V9-cache-lifetime-results.md), 서버 principal 확인·같은 actor의 새 토큰 교체 후 pending 복구는 [V10](V10-session-recovery-results.md)에서 후속 검증했다.

## 변경 이력

- 2026-10-04 V6 전송 계층 spike 실행 결과.
- 2026-10-04 Codex r7 반영: 같은 키 재요청 복구, 상태 조회 요청 일치, 미확정 추적·재시도 API, HTTP 경계.

- 2026-10-04 [V12](V12-typed-transport-results.md)에서 생성 읽기 타입을 공통 transport/cache에 연결. 성공 read fingerprint·외부 envelope 검사와 typed facade의 캐시 주입 차단을 검증. 최종 wire/호환·전체 decoder는 남음.

- 2026-10-04 [V13](V13-id-boundary-results.md)에서 큰 Id의 숫자 제한·십진 문자열 후보와 첫 쓰기 계약 불일치 사전 거부를 검증. V6 HTTP read/apply만 wire 연결했으며 bundle/compose/worker·최종 Id 선택은 남음.

- 2026-10-04 [V14](V14-typed-apply-results.md)에서 별도 후보 binding의 direct 공개 전이 입력/결과·복구 타입, 두 결과 Id 배열 검사·결과 동결·공개 쓰기 지문을 연결. scalar filter 실행 공백과 조합/worker 타입·최종 API는 남음.

- 2026-10-04 후속 [V15](V15-filter-value-results.md)는 Url·Enum·Time read/where 공통 검사와 읽기 Text.prefix를 연결했다. NUL 선거부 범위는 read/where이며 create/compose·공식 패키지 통합은 남는다.
