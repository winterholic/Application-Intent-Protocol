# Codex 과제: DD-01 독립 비판 검토 (1라운드)

너는 AIP 설계의 **독립 비판자이자 공동 설계자**다. 다른 에이전트(Claude, coordinator)가 쓴 설계 결정 문서를 창시자의 철학 기준으로 반박하고 개선안을 낸다. 목표는 문서를 "창시자 철학을 완벽히 담은" 상태로 만드는 것이다. 동의하려고 읽지 말고, 틀린 곳을 찾으려고 읽어라.

## Target (읽을 것)
작업 루트: `/Users/winterholic/development/projects/aip`
1. 최우선 기준(창시자 원문): `plan-docs/sources/founder-intent-handoff-2026-10-03.md`, `plan-docs/sources/design-operating-rules-2026-10-03.md`
2. 원칙 정본: `docs/PRINCIPLES.md`
3. 검토 대상: `plan-docs/decisions/DD-01-read-composition.md`
4. 맥락: `plan-docs/00-rules.md`, `plan-docs/01-purpose.md`, `plan-docs/02-principles.md`, `plan-docs/topics/T01-caller-model.md`, `plan-docs/topics/T07-security.md`
5. 근거 코드(사실 검증용, 읽기만): 아리아리 백엔드 `/Users/winterholic/development/projects/ariari/ariari-backend/src/main/java/com/ariari/ariari/domain/recruitment/recruitment/` (`RecruitmentController.java`, `RecruitmentRepositoryImpl.java`, `RecruitmentListService.java`), 프론트 `/Users/winterholic/development/projects/ariari/ariari/ariari-frontend/app/api/recruitment/api.ts`, AIP PoC `examples/ariari/app.aip`(425행 근처 `RecruitmentList`)
- `docs/origin/*`, `docs/design/00~10`은 원칙이 흐려진 문서다. 근거로 쓰지 마라.

## Change (만들 것)
`plan-docs/reviews/DD-01-codex-r1.md` 파일 하나를 새로 쓴다. 한국어. 구성:
1. **사실 검증**: DD-01이 인용한 코드 사실(엔드포인트 10개, 가시성 규칙 5곳 복사, 50~53행 임의 정렬, PoC RecruitmentList 형태)을 파일·행으로 확인. 맞음/틀림/부분.
2. **철학 정합성 비판**: 창시자 원문의 어느 문장과 DD-01이 어긋나거나 축소·왜곡했는지. 특히 (a) "호출자 중심 데이터·동작 표현", "서버가 모든 요청 형태를 사전 정의하는 구조를 경계" (b) "요청의 자유와 실행 권한 분리" (c) AI First의 "불필요한 선택지 제거, 한 의도 한 구조" (d) "GraphQL 영감은 허용, 복제는 아님" (e) "자연어의 문법화"가 읽기 요청 설계와 연결돼야 하는지.
3. **대안 비판**: 대안 1·2·3 비교가 공정한가. 빠진 실질적 대안이 있는가(있으면 구체적으로 제시). 대안 2의 "새로 생기는 비용"을 과소평가했는가.
4. **보안 비판**: C.3 기술 제안(필터·정렬 기본값, 관계 탐색, 집계, 비용 상한, 존재 노출)의 구멍. 구체적 공격 요청 예시로.
5. **F 질문 비판**: 창시자에게 묻는 F1~F4가 정말 창시자 결정 사항인가. 빠진 질문, 에이전트가 기술적으로 제안했어야 할 질문.
6. **이탈 방지 점검 재평가**: 6문항을 너의 판단으로 다시 답하고, DD-01의 답과 다른 곳을 지적.
7. **구체적 수정 제안 목록**: DD-01의 어느 절을 어떻게 바꿀지 항목별로(우선순위 표시).
8. **결론**: 너의 추천 대안과 이유, Claude의 추천과 다르면 그 차이.

## Constraints
- 읽기 전용: `plan-docs/reviews/DD-01-codex-r1.md` 외의 어떤 파일도 만들거나 고치지 마라. 코드, 다른 문서, git 금지(이 폴더는 git이 아니다).
- 창시자가 승인하지 않은 것을 "확정"이라 쓰지 마라. 미결정 의제(호출자 범위, 작성 언어, 자연어 문법화 의미, 엔진 형태, DB 범위)를 임의로 정하지 마라. 추천은 된다.
- 확인하지 못한 주장은 `확인 필요`로 표시.
- `.env`나 비밀 파일을 읽지 마라.

## Ownership
네가 편집하는 파일은 `plan-docs/reviews/DD-01-codex-r1.md` 하나뿐이다. DD-01 본문 수정은 coordinator가 한다.

## Observable acceptance
- `plan-docs/reviews/DD-01-codex-r1.md`가 존재하고 위 8절을 모두 포함한다.
- 사실 검증 절의 모든 항목에 파일 경로와 행 번호가 있다.
- 수정 제안이 최소 5개, 각 항목에 DD-01의 해당 절(A~G, C.3 등)이 명시돼 있다.
- worker_done 요약에 "가장 중대한 지적 3가지"를 넣는다.
