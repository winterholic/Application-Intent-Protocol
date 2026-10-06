# Codex 과제: DD-04 독립 비판 검토 (1라운드)

너는 AIP 설계의 독립 비판자이자 공동 설계자다. 동의하려고 읽지 말고 틀린 곳을 찾으려고 읽어라. 같은 방식으로 DD-01~03을 3라운드씩 토론해 합의했다. 이번은 정의 작성 방식이다.

## Target
작업 루트 `/Users/winterholic/development/projects/aip`
1. 기준: `plan-docs/sources/founder-intent-handoff-2026-10-03.md`(특히 PART I §3·§5, PART II §9.1·§9.4, PART VII §14), `plan-docs/sources/design-operating-rules-2026-10-03.md`, `docs/PRINCIPLES.md`(10-02 "새 프로그래밍 언어가 아니다"), `plan-docs/00-rules.md` §4(10-02와 10-03 기록 차이)
2. 검토 대상: `plan-docs/decisions/DD-04-authoring-model.md`
3. 일관성 기준: `plan-docs/decisions/DD-01-read-composition.md`, `DD-02-write-scope.md`, `DD-03-read-execution-timing.md`(모두 v3.1)
4. 근거(읽기만): `examples/ariari/app.aip`, `spec/grammar.md`, `crates/aip-syntax`, `crates/aip-sema`, `crates/aip-ir`(파서 이후 재사용 가능성)
- `docs/origin/*`, `docs/design/00~10`은 철학 근거로 쓰지 마라.

## Change
`plan-docs/reviews/DD-04-codex-r1.md` 하나. 한국어:
1. 사실 검증(DD-04의 코드·기록 인용)
2. 철학 정합성: 10-02와 10-03의 긴장을 DD-04가 공정하게 제시했는가. 창시자 미결정 사항을 에이전트 추천(C1)으로 몰래 기울였는가. "고유 문법 제거 금지"를 "읽는 표기로만 남김"으로 축소 해석했는가
3. 대안 비판: A/B/C1/C2/B.4 비교의 공정성, 빠진 대안, 정의 평가 시점의 보안(TS 정의 파일이 실행 코드라는 문제), AI 작성 정확도 주장 근거
4. 비전문가 창시자가 F1~F4로 실제로 고를 수 있는가(예시 B.1~B.4가 같은 내용을 공정하게 담았는가 포함)
5. 이탈 방지 점검 재평가
6. 수정 제안(P0/P1/P2, 절 명시, 5개 이상)
7. 결론과 추천(네 추천이 C1과 다르면 이유)

## Constraints
이 파일 외 수정 금지. 코드·git·비밀 파일 금지. 미결정 확정 금지. 확인 못 한 주장은 `확인 필요`.

## Ownership
편집 파일: `plan-docs/reviews/DD-04-codex-r1.md` 하나.

## Observable acceptance
7절, 수정 제안 5개 이상, worker_done 요약에 "가장 중대한 지적 3가지".
