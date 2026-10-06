너의 작업 결과물은 파일 하나를 쓰는 것이다: `plan-docs/reviews/codex-v8-r9.md`.

# 세션 토큰 실험 정확성 검토 (r9)

이 저장소는 사용자 본인이 설계 중인 오픈소스 애플리케이션 프레임워크(AIP)다. 서버가 요청 주체(actor)를 서명된 세션 토큰으로만 정하는 실험이 문서의 주장대로 동작하는지 정상 사용과 경계 사례로 검토한다.

## 범위
- `plan-docs/alignment/V8-auth-results.md`, 코드 `spikes/spike-v6-transport/src/auth.rs`, `src/lib.rs`(serve_conn의 토큰 처리), `tests/v8_auth.rs`, `client/transport.ts`, V6 결과 문서와 r7 반영.
- 기준: `plan-docs/sources/founder-integrated-directive-2026-10-03.md`(서버 최종 통제·인증 관련 절), `plan-docs/alignment/E-technical-risks.md`.
- 실행: `cd spikes/spike-v6-transport && PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`. 변형은 임시 복사본, DB는 코드의 기본 schema 이름도 바꾼 임시 schema만 쓰고 지운다.

## 질문 (근거 파일:줄, 가능하면 실제 실행 입력·결과)
- Q1. 토큰 형식·검증이 문서대로인가? 정상 형식이지만 의도와 다르게 받아들여지는 토큰(인코딩 변형, payload 필드 타입·누락·추가, 시간 경계)이 있는가?
- Q2. 서버 요청 처리에서 actor가 토큰 외 경로로 정해지거나, 검증 실패가 익명 처리로 이어지는 경로가 있는가? 멱등 키·캐시와의 관계.
- Q3. 문서 주장 중 실행 근거보다 강한 것, 다음 단계 전에 정할 것과 창시자 결정이 필요한 것.

## 작성 규칙
- 발견마다 즉시 파일에 append한다. 심각도 P1/P2/P3.
- 확인하지 못한 것은 "확인 못함: <이유>". 발견이 없으면 "없음"과 확인 범위.
- `codex-v8-r9.md` 외 파일 수정·생성 금지(`target/` 제외). `.env`·비밀 저장소 읽기 금지. git 명령 금지.

## 최종 응답
10줄 이하 요약 + 파일 경로.
