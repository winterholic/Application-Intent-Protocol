너의 작업 결과물은 파일 하나를 쓰는 것이다: `plan-docs/reviews/codex-v34-r6.md`.

# 조합 쓰기·쓰기 확장 정확성 검토 (r6)

이 저장소는 사용자 본인이 설계 중인 오픈소스 애플리케이션 프레임워크(AIP)다. 실험 코드가 문서의 주장대로 동작하는지 정상 사용 시나리오와 경계 사례로 검토한다. 프로세스 격리 주제는 범위 밖이다.

## 범위
- W1 자기 행 조합: `plan-docs/alignment/V3-standard-write-results.md` §7(selfRow 부분), 코드 `spikes/spike-v3-write/src/lib.rs`의 `compose`(selfRow·`on: self`), `tests/v3_3.rs`, V1 `spikes/spike-v1-fixture/src/{ast,parser,sema}.rs`의 ExposeCompose.self_row.
- 쓰기 확장: `plan-docs/alignment/V4-official-extension-results.md` §7, 코드 `spikes/spike-v4-worker/src/write.rs`, `workers/`, `extensions/club.*`, `tests/v4_3_write.rs`, V3 `apply_in`·`checks_in`, V1의 write extension 의미 검사.
- 기준: `plan-docs/sources/founder-integrated-directive-2026-10-03.md`(표준 쓰기·조합, JS/TS·Python 확장), `plan-docs/alignment/E-technical-risks.md`(RK-04·RK-05), `plan-docs/alignment/C-syntax-proposal.md`(B.6·B.7).
- 실행: 각 spike에서 `PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`(로컬 PostgreSQL, node, python3). DB 실험은 임시 schema만 쓰고, 코드의 기본 schema 이름도 바꿔서 실행한 뒤 지운다.

## 질문 (근거 파일:줄, 가능하면 실제 실행 입력·결과)
- Q1. selfRow 조합에서 요청자가 자기 행이 아닌 행에 단계를 적용하거나, 다른 범위의 자기 행을 고르거나, 단계 순서·반복으로 의도와 다른 최종 상태를 만드는 정상 형식의 요청이 있는가?
- Q2. 쓰기 확장에서 일부 쓰기만 커밋되거나, 확장이 선언하지 않은 쓰기를 하거나, 기한·실패 뒤 쓰기가 남는 경우가 있는가? 같은 worker의 연속 호출, 확장 안 병렬 ctx 쓰기, 커밋 중 지연.
- Q3. 결과 문서 주장 중 실행 근거보다 강한 것. W0/W1/W2/쓰기 확장 비교 결론의 범위.
- Q4. 다음 단계 전에 정할 것과 창시자 결정이 필요한 것.

## 작성 규칙
- 발견마다 즉시 파일에 append한다. 심각도 P1(다음 단계 전 수정)/P2(권장)/P3(기록).
- 확인하지 못한 것은 "확인 못함: <이유>". 발견이 없으면 "없음"과 확인 범위. 빈칸을 채우려고 발견을 만들지 않는다.
- `codex-v34-r6.md` 외 파일 수정·생성 금지(`target/` 제외). 변형 실험은 임시 디렉터리 복사본에서. `.env`·비밀 저장소 읽기 금지. git 명령 금지.

## 최종 응답
10줄 이하 요약 + 파일 경로.
