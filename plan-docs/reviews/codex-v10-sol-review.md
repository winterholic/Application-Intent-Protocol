# V10 독립 검토와 반영

> 2026-10-04. Codex 메인 구현·판정, `gpt-6-sol high` 설계·독립 코드 비평, `gpt-6-luna high` HTTP/PG 테스트·문서 대조. Claude는 참여하지 않았다. 에이전트 합의는 창시자 승인이나 전체 안전성 증명이 아니다.

| 검토 | 근거와 메인 반영 |
|---|---|
| Sol: 갱신할 사용자 식별 | 첫 응답이 유실된 뒤에야 principal을 확인하면 원래 사용자를 고정할 근거가 없음. read/apply 전 서버 `/session` 확인, 다른 principal 교체 거부 |
| Sol: 늦은 응답·종료 경합 | cache/read/apply를 인증 세대로 분리. 같은 키의 flight도 세대별로 저장. Node gate로 옛 응답의 늦은 완료와 새 세대 복구 결과 보존 검증 |
| Sol: 최초 admission 거부 | `/session` 성공 직후 첫 `/apply`가 TOKEN_EXPIRED이면 pending 선등록 때문에 WriteUnsettled가 됨. Sol 인라인 재현 후 메인 실패 테스트로 확인. 첫 전송 거부는 확정, 이전 미확정 시도 뒤 거부는 보존하도록 수정 |
| 기존 V8 r9: 익명 sentinel 충돌 | [r9](codex-v8-r9.md)의 signed actor=-1과 익명 저장소 충돌을 실제 HTTP/DB로 재현. ID 도메인 축소 대신 principal namespace 분리 |
| Luna: 실제 서버 경로 | `/session` 토큰·본문 계약, signed -1/익명 분리, 응답 유실 후 갱신·다른 actor 거부, DB 단일 효과 테스트 작성. 구현 전 실패·구현 후 통과 |
| 메인: 실제 토큰 만료 | Luna의 만료 응답 모사를 실제 3초 토큰·대기로 대체. 커밋 성공 JSON 확인 후 응답 유실, 서버 만료 확인, 새 토큰 replay, 회원·outbox 1건씩 확인 |
| 메인: 문서 검증 누락 | 새 실험의 깨진 링크를 기존 검사기가 성공 처리한 반례 확인. alignment/reviews 자동 포함, 같은 음성 대조로 거부 확인 |

최신 Sol 코드 대조에서는 위 admission 수정 후 추가 재현 가능한 세션·멱등 분리 결함을 찾지 못했다. 독립 리뷰 범위는 코드 대조와 admission 인라인 재현이며, 실제 서버/DB 실행은 메인·Luna가 수행했다. 실행 명령과 미검증 범위는 [V10 결과](../alignment/V10-session-recovery-results.md)를 따른다.
