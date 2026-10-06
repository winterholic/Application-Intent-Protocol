# Codex 과제: DD-11 독립 비판 검토 (1라운드)

너는 AIP 설계의 독립 비판자이자 공동 설계자다. 동의하려고 읽지 말고 틀린 곳을 찾으려고 읽어라. 억지로 문제를 만들지는 마라.

## Target
작업 루트 `/Users/winterholic/development/projects/aip`
1. 기준: `plan-docs/sources/founder-intent-handoff-2026-10-03.md`(의제 14, PART I §1), `plan-docs/sources/design-operating-rules-2026-10-03.md`, `docs/PRINCIPLES.md`
2. 검토 대상: `plan-docs/decisions/DD-11-client-cache-invalidation.md`
3. 일관성(에이전트 합의 문서): `plan-docs/decisions/DD-01-read-composition.md`(I5, canonical), `DD-02-write-scope.md`(불변식, 전이), `DD-07-bulk-success-unit.md`, `DD-08-request-wire-format.md`, `DD-06-engine-and-runtime.md`
4. 근거(읽기만): 아리아리 프론트 `/Users/winterholic/development/projects/ariari/ariari/ariari-frontend/app/`에서 `invalidateQueries` 전체(34곳으로 적었으나 미검증, 다시 세어라)와 해당 쓰기의 백엔드 서비스 코드(`/Users/winterholic/development/projects/ariari/ariari-backend/src/main/java/com/ariari/ariari/domain/`), AIP PoC `crates/aip-pg/src/writes.rs`, `crates/aip-runtime/src/subscribe.rs`

## Change
`plan-docs/reviews/DD-11-codex-r1.md` 하나. 한국어:
1. 사실 검증
2. E1 수행(가능한 만큼): `invalidateQueries` 각 호출을 백엔드 실제 변경과 대조해 빠뜨린 무효화·과잉 무효화 찾기. 표로
3. 철학 정합성과 DD-01~08 불일치
4. 대안 비판(M1/M2/M3, 빠진 대안), 변경 집합이 존재를 노출하는 경로, 확장 동작·연쇄 삭제·불변식 repair가 만든 변경을 엔진이 다 아는가
5. F1이 창시자 질문이 맞는지, 비전문가가 답할 수 있는지
6. 이탈 방지 점검 재평가
7. 수정 제안 P0/P1/P2
8. 결론과 추천

## Constraints
이 파일 외 수정 금지. 코드·git·비밀 파일 금지. 미결정 확정 금지. 확인 못 한 주장은 `확인 필요`.

## Ownership
편집 파일: `plan-docs/reviews/DD-11-codex-r1.md` 하나.

## Observable acceptance
8절, worker_done 요약에 "가장 중대한 지적 3가지".
