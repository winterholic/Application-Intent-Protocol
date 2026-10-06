너의 작업 결과물은 파일 하나를 쓰는 것이다: `plan-docs/reviews/codex-v1v2-r2.md`.

# V1 반영 확인 + V2 호출자 읽기 slice 비판 검토 (r2)

## 배경
- 기준 문서(우선순위 순): `plan-docs/sources/founder-integrated-directive-2026-10-03.md`, `plan-docs/alignment/E-technical-risks.md`(RK-01~03, §4 V2), `plan-docs/alignment/C-syntax-proposal.md`(EQ-01~11, SY-2·SY-3).
- 이전 검토: `plan-docs/reviews/codex-v1-r1.md`(네가 쓴 것). 반영 기록: `plan-docs/alignment/V1-semantic-fixture-results.md` §7.
- V2 결과 초안: `plan-docs/alignment/V2-caller-read-results.md`.
- 검토 대상 코드: `spikes/spike-v1-fixture/`(r1 반영분, 특히 src/host.rs·extract.rs·hlit.rs·sema.rs, tests/v1_r1.rs), `spikes/spike-v2-read/`(src/sqlgen.rs·plan.rs·lib.rs, tests/v2.rs). 본 crates/는 대상 아님.
- 실행: `cd spikes/spike-v1-fixture && PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q`, `cd spikes/spike-v2-read && PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`(로컬 PostgreSQL `host=localhost dbname=postgres`, schema `aip_v2_spike`만 생성·삭제). SQL 확인: `cargo run --offline -q --example show_sql`.
- 작성자 주장(미검증으로 취급하라): r1 P1이 모두 고쳐졌다. V2가 행·필드·관계·집계 정책을 SQL로 강제하고, 열린/닫힌 capability·select와 별개인 filter/sort 허용·depth/rows/cost/deadline·값 바인딩을 실행으로 검증했다.

## 할 일
1. 두 spike 테스트를 직접 실행하고 명령과 출력 1~3줄을 파일에 적는다.
2. r1의 P1(F01·F02·F04·F06·F09·F10·F11·F16)이 실제로 닫혔는지 각 원래 반례와 그 변형으로 다시 시도한다. 우회 반례가 있으면 적는다.
3. V2 비판. 각 질문에 근거(파일:줄)와 가능하면 실제 실행한 반례:
   - Q1. 권한 누출: 비인가 actor가 행·필드·관계·집계 값이나 그 존재를 알아낼 수 있는 요청이 있는가? (filter/sort/limit을 이용한 추론, 집계 count 차분, null 표현 차이, 오류 코드 차이, 관계 null 여부로 정책 결과 추론 등)
   - Q2. SQL 생성의 정확성: 정책 식 lowering(NULL 3치 논리, predicate 인라인 변수 섞임, exists 별칭, 중첩 subquery 별칭 z 재사용)이 의도한 의미와 다른 경우.
   - Q3. 비용 제한: rows/depth/cost/deadline 검사를 우회하거나 휴리스틱이 무의미한 경우. statement_timeout 밖 비용(계획 시간·결과 크기)도.
   - Q4. 값 바인딩·주입: 요청 값이 SQL 텍스트에 들어가는 경로, 식별자가 요청에서 오는 경로.
   - Q5. V2 결과 문서의 주장 중 실행 근거보다 강한 것, V2-R1~R8 중 창시자 지침과 충돌하거나 창시자 결정이 필요한 것.
   - Q6. V3(표준 쓰기·W0/W1/W2)로 넘어가기 전 막아야 할 것.
4. 발견마다 즉시 파일에 append한다. 각 발견에 P1(V3 전 반드시 수정)/P2(권장)/P3(기록).
5. 확인하지 못한 것은 "확인 못함: <이유>". 발견이 없으면 "없음"과 확인 범위. 빈칸을 채우려고 발견을 만들지 않는다.

## 금지
- `codex-v1v2-r2.md` 외 파일 수정·생성 금지(`target/` 제외). 변형 실험은 임시 디렉터리 복사본에서. DB는 `aip_v2_spike` schema 또는 네가 만든 별도 임시 schema만 쓰고 끝나면 지운다.
- `.env`·비밀 저장소 읽기 금지. git 명령 금지.

## 최종 응답
10줄 이하 요약 + 파일 경로.
