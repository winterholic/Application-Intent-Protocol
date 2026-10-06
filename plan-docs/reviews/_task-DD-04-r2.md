# Codex 과제: DD-04 2라운드 (v2 재검토, 합의 확인 겸)

작업 루트 `/Users/winterholic/development/projects/aip`. 네 1라운드 `plan-docs/reviews/DD-04-codex-r1.md`의 15개 제안을 Claude가 모두 수용해 `plan-docs/decisions/DD-04-authoring-model.md`를 v2로 다시 썼다(B.0 체크리스트와 동등 예시, B-run/B-data/E 분리, C1 철회, C.3 신뢰 경계, 네 추천 E를 1순위 검증 후보로). 기준은 `plan-docs/sources/` 두 파일, `docs/PRINCIPLES.md`, `plan-docs/00-rules.md` §4, DD-01~03 v3.1.

## Change
`plan-docs/reviews/DD-04-codex-r2.md` 하나(한국어, 간결하게):
1. 1라운드 제안 15개 반영 여부(반영/부분/미반영/새 문제), 행 근거
2. B.1~B.4가 B.0의 9개 항목을 정말 같은 의미로 담았는지 항목별 대조
3. v2의 새 모순, 과잉 설계, 미결정 사항 임의 확정(특히 E 추천이 창시자 F1·F2를 선취하는지)
4. F1~F3이 비전문가 창시자가 답할 수 있는가
5. 잔여 수정 제안(P0/P1/P2)
6. 결론: "에이전트 간 합의 도달" 또는 "미합의 N개(목록)". 사소한 표현은 "선택적 다듬기"로

## Constraints
이 파일 외 수정 금지. 코드·git·비밀 파일 금지. 억지로 문제를 만들지 마라.

## Ownership
편집 파일: `plan-docs/reviews/DD-04-codex-r2.md` 하나.

## Observable acceptance
6절, 15개 전부 판정, worker_done 요약 첫 문장에 "합의 도달" 또는 "미합의 N개".
