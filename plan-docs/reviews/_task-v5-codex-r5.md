너의 작업 결과물은 파일 하나를 쓰는 것이다: `plan-docs/reviews/codex-v5-r5.md`.

# 프론트 SDK 캐시·r4b 반영 정확성 검토 (r5)

이 저장소는 사용자 본인이 설계 중인 오픈소스 애플리케이션 프레임워크(AIP)다. 실험 코드가 문서의 주장대로 동작하는지 정상 사용 시나리오와 경계 사례로 검토한다.

## 범위
- V5-3 SDK 캐시 런타임: `plan-docs/alignment/V5-sdk-results.md` §6, 코드 `spikes/spike-v5-sdk/sdk/cache.ts`, `sdk/cache.test.ts`, `tests/v5_3_cache.rs`, `src/lib.rs`의 `write_tags`.
- r4b 반영 확인: `plan-docs/reviews/codex-v345-r4b.md`(네가 쓴 것)의 F01~F09. 반영 기록은 `V5-sdk-results.md` §5, `V4-official-extension-results.md` §6. 코드 `spikes/spike-v5-sdk/sdk/aip.ts`·`src/lib.rs`, `spikes/spike-v4-worker/src/lib.rs`, `spikes/spike-v2-read/src/plan.rs`.
- 기준: `plan-docs/sources/founder-integrated-directive-2026-10-03.md`(프론트 캐시·동기화 관련 절), `plan-docs/alignment/E-technical-risks.md`(RK-09), `plan-docs/alignment/D-development-experience.md`(지원자 승인과 캐시).
- 실행: 각 spike에서 `PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`. node, `spikes/spike-0-ts/node_modules/.bin/tsc`, 로컬 PostgreSQL.

## 질문 (근거 파일:줄, 가능하면 실제 실행 입력·결과)
- Q1. 캐시가 쓰기 뒤 낡은 결과를 돌려주는 정상 순서가 있는가(쓰기 태그 누락, 효과·outbox 후속 쓰기, 여러 쓰기 동시, 읽기와 쓰기 순서 교차)?
- Q2. 캐시 키·actor 처리·결과 미확정 처리가 문서와 맞는가? 과다 무효화의 정도.
- Q3. r4b 발견 F01~F09가 원래 재현 입력으로 해결됐는가? 고친 부분이 정상 요청을 새로 거부하거나 다른 의미 차이를 만들었는가?
- Q4. 결과 문서 주장 중 실행 근거보다 강한 것. 다음 단계(전송 계층, 응답 유실 복구, 시간 의존 만료) 전에 정할 것과 창시자 결정이 필요한 것.

## 작성 규칙
- 발견마다 즉시 파일에 append한다. 심각도 P1(다음 단계 전 수정)/P2(권장)/P3(기록).
- 확인하지 못한 것은 "확인 못함: <이유>". 발견이 없으면 "없음"과 확인 범위. 빈칸을 채우려고 발견을 만들지 않는다.
- `codex-v5-r5.md` 외 파일 수정·생성 금지(`target/` 제외). 변형 실험은 임시 디렉터리 복사본에서, DB는 네가 만든 임시 schema만(코드의 기본 schema 이름도 바꿔서) 쓰고 지운다. `.env`·비밀 저장소 읽기 금지. git 명령 금지.

## 최종 응답
10줄 이하 요약 + 파일 경로.
