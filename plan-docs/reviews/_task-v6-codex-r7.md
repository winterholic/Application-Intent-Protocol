너의 작업 결과물은 파일 하나를 쓰는 것이다: `plan-docs/reviews/codex-v6-r7.md`.

# 전송 계층 실험 정확성 검토 (r7)

이 저장소는 사용자 본인이 설계 중인 오픈소스 애플리케이션 프레임워크(AIP)다. 실험 코드가 문서의 주장대로 동작하는지 정상 사용 시나리오와 경계 사례로 검토한다. 인증·프로세스 격리는 범위 밖이다(문서에 미구현으로 명시됨).

## 범위
- `plan-docs/alignment/V6-transport-results.md`, 코드 `spikes/spike-v6-transport/`(src/lib.rs, client/transport.ts, client/e2e.test.ts, tests/v6_e2e.rs). 재사용하는 V3 `apply_in`·`checks_in`(`spikes/spike-v3-write/src/lib.rs`), V5 캐시(`spikes/spike-v5-sdk/sdk/cache.ts`).
- 기준: `plan-docs/sources/founder-integrated-directive-2026-10-03.md`(프로토콜·프론트 동기화 관련 절), `plan-docs/alignment/E-technical-risks.md`(RK-08·RK-09), `plan-docs/reviews/codex-v5-r5.md`의 R5-02.
- 실행: `cd spikes/spike-v6-transport && PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`(node, 로컬 PostgreSQL). 변형은 임시 복사본에서, DB는 코드의 기본 schema 이름도 바꾼 임시 schema만 쓰고 지운다.

## 질문 (근거 파일:줄, 가능하면 실제 실행 입력·결과)
- Q1. 멱등 키: 같은 키 동시 요청, 실패한 쓰기 뒤 같은 키 재시도, 요청 본문 비교 방식(키 순서 등), 상태 조회와 재시도 사이 경쟁에서 두 번 실행되거나 잘못된 결과를 받는 경우.
- Q2. 클라이언트 복구 흐름과 캐시: 응답 유실 외 오류(서버 오류 응답, 읽기 실패) 처리, 무효화 태그와 실제 변경의 일치, 늦은 응답.
- Q3. 서버 요청 처리의 정확성: 헤더·본문 파싱, 잘못된 JSON, 경로, 상한.
- Q4. 문서 주장 중 실행 근거보다 강한 것. 다음 단계 전에 정할 것과 창시자 결정이 필요한 것.

## 작성 규칙
- 발견마다 즉시 파일에 append한다. 심각도 P1(다음 단계 전 수정)/P2(권장)/P3(기록).
- 확인하지 못한 것은 "확인 못함: <이유>". 발견이 없으면 "없음"과 확인 범위. 빈칸을 채우려고 발견을 만들지 않는다.
- `codex-v6-r7.md` 외 파일 수정·생성 금지(`target/` 제외). `.env`·비밀 저장소 읽기 금지. git 명령 금지.

## 최종 응답
10줄 이하 요약 + 파일 경로.
