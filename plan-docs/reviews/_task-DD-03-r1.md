# Codex 과제: DD-03 독립 비판 검토 (1라운드)

너는 AIP 설계의 독립 비판자이자 공동 설계자다. 동의하려고 읽지 말고 틀린 곳을 찾으려고 읽어라.

## Target
작업 루트 `/Users/winterholic/development/projects/aip`
1. 기준: `plan-docs/sources/founder-intent-handoff-2026-10-03.md`, `plan-docs/sources/design-operating-rules-2026-10-03.md`, `docs/PRINCIPLES.md`
2. 검토 대상: `plan-docs/decisions/DD-03-read-execution-timing.md` (초안, 네 DD-01·DD-02 토론 전에 쓴 것이라 그 합의를 덜 반영했을 수 있다)
3. 일관성 기준: `plan-docs/decisions/DD-01-read-composition.md` v3.1(특히 C.1 실행 하위안 2-R/2-M/2-RM, C.3 I1~I7, F3), `DD-02-write-scope.md` v3.1
4. 근거(읽기만): 아리아리 프론트 `/Users/winterholic/development/projects/ariari/ariari/ariari-frontend/app/api/`(요청 모양이 정적으로 드러나는지), AIP PoC `crates/aip-pg`(컴파일 시점 SQL 고정, E3 재사용 주장), `docs/DECISIONS.md` OI-C2
- `docs/origin/*`, `docs/design/00~10`은 철학 근거로 쓰지 마라.

## Change
`plan-docs/reviews/DD-03-codex-r1.md` 하나. 한국어. 구성:
1. 사실 검증(DD-03의 코드·PoC 주장)
2. 철학 정합성(원문 PART I §1 "서버가 모든 요청 형태를 사전 정의하는 구조 경계", §2, §4, PART V)과 DD-01·DD-02 v3.1과의 불일치
3. 대안 비판(2-R/2-M/2-RM 비교 공정성, 빠진 대안, "승인"이 사람 검토로 되살아나 서버 작업이 형태만 바뀌는 위험, 개발 모드와 운영의 의미 불일치 위험)
4. 보안·운영 비판(manifest 위조·재생, 버전 불일치, 동적 호출자 경로, 캐시)
5. F 질문 비판(DD-03 F1이 창시자 질문인지, DD-01 F3과 중복인지)
6. 이탈 방지 점검 재평가
7. 수정 제안(P0/P1/P2, 절 명시, 5개 이상)
8. 결론과 추천

## Constraints
이 파일 외 수정 금지. 코드·git·비밀 파일 금지. 미결정 확정 금지. 확인 못 한 주장은 `확인 필요`.

## Ownership
편집 파일: `plan-docs/reviews/DD-03-codex-r1.md` 하나.

## Observable acceptance
8절, 수정 제안 5개 이상, worker_done 요약에 "가장 중대한 지적 3가지".
