너의 작업 결과물은 파일 하나를 쓰는 것이다: `plan-docs/reviews/codex-v2v3-r2b.md`.

# 데이터 접근 프레임워크 설계 실험 정확성 검토 (r2b)

이 저장소는 사용자 본인이 설계 중인 오픈소스 애플리케이션 프레임워크(AIP)다. 서버가 선언된 접근 정책을 SQL로 정확히 집행하는지, 설계 실험의 주장이 실행 근거와 맞는지를 검토한다. 정상 사용 시나리오와 경계 사례 위주로 본다.

## 배경
- 기준 문서: `plan-docs/sources/founder-integrated-directive-2026-10-03.md`, `plan-docs/alignment/E-technical-risks.md`(RK-01~04, §4), `plan-docs/alignment/C-syntax-proposal.md`(EQ-01~11, B.5).
- 직전 검토 `plan-docs/reviews/codex-v1v2-r2.md`는 R2-01~03 작성 후 중단됐다. 그 세 건은 반영됐다(`spikes/spike-v1-fixture/tests/v1_r2.rs`).
- 결과 문서: `plan-docs/alignment/V2-caller-read-results.md`, `plan-docs/alignment/V3-standard-write-results.md`.
- 코드: `spikes/spike-v2-read/`(src/sqlgen.rs·plan.rs·lib.rs, tests/v2.rs), `spikes/spike-v3-write/`(src/lib.rs, tests/v3_1.rs). V1 facts는 `spikes/spike-v1-fixture/`.
- 실행: 각 spike에서 `PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`. 로컬 PostgreSQL `host=localhost dbname=postgres`를 쓴다. V2는 schema `aip_v2_spike`, V3는 `aip_v3_spike`를 만들고 지운다. SQL 확인: spike-v2-read에서 `cargo run --offline -q --example show_sql`.
- 작성자 주장(미검증으로 취급): V2는 행·필드·관계·집계 정책을 SQL로 정확히 집행하고 계약 밖 요청을 거부한다. V3-1은 대상 판정과 변경을 원자적으로 하고 동시 요청에서 이중 변경이 없다.

## 할 일
1. 세 spike 테스트를 직접 실행하고 명령과 출력 1~3줄을 적는다. DB 연결이 안 되면 그 사실과 오류 한 줄을 적고 코드 읽기로 진행한다.
2. R2-01~03 반영을 원래 반례로 다시 확인한다.
3. V2·V3-1 정확성 질문. 각 항목에 근거(파일:줄)와 가능하면 실제 실행한 입력·결과:
   - Q1. 정책 집행 정확성: 정책상 보이지 않아야 할 행·필드 값·관계·집계 값이 응답에 포함되는 정상 형식의 요청이 있는가? 응답의 차이(null 표현, 오류 코드, 관계 null, 개수)로 정책 판정 결과가 드러나는 경우도 기록한다.
   - Q2. 정책 식을 SQL로 옮기는 과정의 의미 차이: NULL 처리, predicate 인라인 시 변수 범위, exists 별칭, 중첩 subquery 별칭 재사용.
   - Q3. 자원 한도: rows/depth/cost/deadline 검사가 의도대로 동작하지 않는 요청, statement_timeout 밖 비용(계획 시간, 결과 크기).
   - Q4. 값 처리: 요청 값이 매개변수가 아니라 SQL 텍스트로 들어가는 경로, 식별자가 요청에서 오는 경로가 있는가.
   - Q5. V3-1 쓰기: 원자성, 누락·중복 대상, where 대상 범위, 동시 실행, repeat unchanged, 오류 코드 일관성.
   - Q6. 결과 문서 주장 중 실행 근거보다 강한 것. V2-R1~R8·V3-R1~R6 중 창시자 지침과 충돌하거나 창시자 결정이 필요한 것.
   - Q7. V3-2(승인 W0/W1/W2 비교)로 가기 전에 고칠 것.
4. 발견마다 즉시 파일에 append한다. 심각도 P1(다음 단계 전 수정)/P2(권장)/P3(기록).
5. 확인하지 못한 것은 "확인 못함: <이유>". 발견이 없으면 "없음"과 확인 범위. 빈칸을 채우려고 발견을 만들지 않는다.

## 금지
- `codex-v2v3-r2b.md` 외 파일 수정·생성 금지(`target/` 제외). 변형 실험은 임시 디렉터리 복사본에서 하고, DB는 위 두 schema 또는 네가 만든 임시 schema만 쓰고 지운다.
- `.env`·비밀 저장소 읽기 금지. git 명령 금지.

## 최종 응답
10줄 이하 요약 + 파일 경로.
