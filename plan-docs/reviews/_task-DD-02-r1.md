# Codex 과제: DD-02 독립 비판 검토 (1라운드)

너는 AIP 설계의 독립 비판자이자 공동 설계자다. 동의하려고 읽지 말고 틀린 곳을 찾으려고 읽어라. 목표는 문서가 창시자 철학을 완벽히 담는 것이다.

## Target
작업 루트 `/Users/winterholic/development/projects/aip`
1. 기준(창시자 원문): `plan-docs/sources/founder-intent-handoff-2026-10-03.md`, `plan-docs/sources/design-operating-rules-2026-10-03.md`, `docs/PRINCIPLES.md`
2. 검토 대상: `plan-docs/decisions/DD-02-write-scope.md`
3. 맥락: `plan-docs/decisions/DD-01-read-composition.md`(v3, 읽기 결정. 너와 Claude가 3라운드 토론한 결과. DD-02는 이것과 일관돼야 한다), `plan-docs/00-rules.md`, `plan-docs/topics/T01-caller-model.md`, `T07-security.md`, `T11-transaction-effects.md`
4. 근거 코드(읽기만): 아리아리 백엔드 `/Users/winterholic/development/projects/ariari/ariari-backend/src/main/java/com/ariari/ariari/` 중 `domain/member/alarm/MemberAlarmService.java`, `domain/member/member/MemberController.java`, `MemberService.java`, `domain/member/Member.java`, `domain/club/clubmember/ClubMemberController.java`, `ClubMemberService.java`. AIP PoC `examples/ariari/app.aip`(해당 command들), 이전 인계 기록의 결함 목록은 `docs/design/09-verification-log.md`와 `docs/design/01-reality-test.md`(기술 자료로만)
- `docs/origin/*`, `docs/design/00~10`은 원칙이 흐려진 문서다. 철학 근거로 쓰지 마라.

## Change
`plan-docs/reviews/DD-02-codex-r1.md` 하나를 새로 쓴다. 한국어. 구성:
1. 사실 검증: DD-02가 인용한 코드 사실(쓰기 93개, W1~W4 행 번호와 규칙, 닉네임 확인 후 쓰기와 DB unique, ADMIN 금지가 컨트롤러 48~50행, 자기 위임 시 ADMIN 0명, PoC의 해당 command와 "ADMIN 2명 DB 거부") 파일·행으로 확인
2. 철학 정합성: 원문 PART I §1(호출자가 데이터와 **동작**을 표현), §2(실행 권한 분리, 무결성), §4(AI First), §8(확장), PART VII §13(A/B/C, 특히 "C는 무제한 명령 실행이 아니다")과 어긋나거나 축소한 곳
3. 대안 비판: A/B/C 비교의 공정성, 빠진 실질 대안, C.2(불변식 선언)와 C.3(정책 쓰기 vs Command 경계 판정 규칙)의 구멍. DD-01 v3의 구조(연산별 capability 기본 거부, 층 1 불변식 I1~I7, preset, 확장 읽기, 런타임 오류 vs 개발 진단)와 일관되게 쓰기 쪽에 대응물이 필요한지
4. 보안·무결성 비판: 필드 확대, 상태 전이 우회, 일괄 쓰기(where 기반 update)의 범위 폭발, 묶음 부분 실패, 불변식 검사의 동시성(잠금·격리 수준), 멱등성, 감사. 개념 공격 요청 예시로
5. F 질문 비판: F1·F2가 창시자만 답할 수 있는 질문인지, 비전문가가 답할 수 있게 쓰였는지, 빠진 질문
6. 이탈 방지 점검 재평가(6문항)
7. 구체적 수정 제안 목록(P0/P1/P2, 해당 절 명시, 최소 5개)
8. 결론: 너의 추천 대안과 이유, Claude 추천(B + 불변식 선언)과의 차이

## Constraints
- `plan-docs/reviews/DD-02-codex-r1.md` 외 수정 금지. 코드·git 금지. 비밀 파일 금지.
- 창시자 미결정 사항 확정 금지(추천은 가능). 확인 못 한 주장은 `확인 필요`.

## Ownership
편집 파일: `plan-docs/reviews/DD-02-codex-r1.md` 하나.

## Observable acceptance
- 8절 포함, 사실 검증 항목마다 파일·행, 수정 제안 5개 이상.
- worker_done 요약에 "가장 중대한 지적 3가지".
