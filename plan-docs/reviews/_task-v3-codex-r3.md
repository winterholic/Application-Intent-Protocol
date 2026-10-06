너의 작업 결과물은 파일 하나를 쓰는 것이다: `plan-docs/reviews/codex-v3-r3.md`.

# 쓰기 실험 정확성 검토 (r3)

이 저장소는 사용자 본인이 설계 중인 오픈소스 애플리케이션 프레임워크(AIP)다. 서버가 선언된 쓰기 정책과 업무 규칙을 트랜잭션 안에서 정확히 집행하는지, 설계 실험의 비교 결론이 실행 근거와 맞는지 검토한다.

## 배경
- 기준: `plan-docs/sources/founder-integrated-directive-2026-10-03.md`(§9 표준 쓰기/조합), `plan-docs/alignment/E-technical-risks.md`(RK-04), `plan-docs/alignment/C-syntax-proposal.md`(B.5, B.6 W0/W1/W2).
- 직전 검토: `plan-docs/reviews/codex-v2v3-r2b.md`(네가 쓴 것). 반영 기록: `plan-docs/alignment/V2-caller-read-results.md` §6, `plan-docs/alignment/V3-standard-write-results.md` §5.
- 새 실험: V3-2 일괄 승인 W0/W1/W2 비교. 설계 `plan-docs/alignment/V3-standard-write-plan.md` §4, 결과 `V3-standard-write-results.md` §6.
- 코드: `spikes/spike-v3-write/src/lib.rs`(judge·update·effects·create_row·run_checks·apply·bundle·compose), `tests/v3_2.rs`. V1 문법 확장은 `spikes/spike-v1-fixture/src/{parser,sema}.rs`(Effect, ExposeCreate, ExposeCompose, unique, check). SQL 생성은 `spikes/spike-v2-read/src/sqlgen.rs`.
- 실행: 각 spike에서 `PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`(로컬 PostgreSQL `host=localhost dbname=postgres`). V3-2는 schema `aip_v3_2`를 만들고 끝에 지운다.
- 작성자 주장(미검증으로 취급): r2b P1·P2가 고쳐졌다. W2·W0(커밋 검사 포함)·W1(서버 선언 범위)이 공통 반례에서 같은 안전한 결과를 낸다. 이 fixture에서 W1은 W2보다 서버 작업을 줄이지 못했다.

## 할 일
1. 세 spike 테스트를 직접 실행하고 명령과 출력 1~3줄을 적는다.
2. r2b 발견(R2B-01~12)의 반영을 원래 재현 입력으로 다시 확인한다. 특히 R2B-06(권한 의존 행 잠금)과 R2B-08(행별 집계 guard).
3. V3-2 질문. 근거(파일:줄)와 가능하면 실제 실행 입력·결과:
   - Q1. 세 후보의 공통 반례 결과가 정말 같은 이유로 같은가, 아니면 우연히 같은가(예: 판정 순서, 오류 코드 우선순위)?
   - Q2. 커밋 검사(xmin 기반)가 놓치는 쓰기 경로나, 정당한 쓰기를 잘못 막는 경우.
   - Q3. W1 값 출처 제한·상수 제한·단계 허용이 의도대로 동작하지 않는 정상 형식의 요청. 단계 순서·반복(같은 단계 두 번)·여러 create.
   - Q4. 전이 효과(create, notify)와 outbox의 원자성, 동시 승인 시 회원 중복·알림 중복.
   - Q5. 결과 문서 §6.4 관찰(W2가 불변식을 한 곳에 둔다, W0은 단독 정책으로 안전하지 않다, W1이 서버 작업을 줄이지 못했다)이 실행 근거보다 강한지. 다른 업무(관리자 위임, 순환 전이)에서 결론이 바뀔 가능성.
   - Q6. 창시자 결정이 필요한 것과 기술적으로 정할 것을 구분해 다음 실험(V3 확장 또는 V4 공식 확장 worker) 전에 필요한 것.
4. 발견마다 즉시 파일에 append한다. 심각도 P1(다음 단계 전 수정)/P2(권장)/P3(기록).
5. 확인하지 못한 것은 "확인 못함: <이유>". 발견이 없으면 "없음"과 확인 범위. 빈칸을 채우려고 발견을 만들지 않는다.

## 금지
- `codex-v3-r3.md` 외 파일 수정·생성 금지(`target/` 제외). 변형 실험은 임시 디렉터리 복사본에서 하고 DB는 네가 만든 임시 schema만 쓰고 지운다.
- `.env`·비밀 저장소 읽기 금지. git 명령 금지.

## 최종 응답
10줄 이하 요약 + 파일 경로.
