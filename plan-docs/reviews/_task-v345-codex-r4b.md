너의 작업 결과물은 파일 하나를 쓰는 것이다: `plan-docs/reviews/codex-v345-r4b.md`.

# 프레임워크 실험 정확성 검토 (r4b)

이 저장소는 사용자 본인이 설계 중인 오픈소스 애플리케이션 프레임워크(AIP)다. 실험 코드가 선언된 계약대로 동작하는지, 결과 문서의 주장이 실행 근거와 맞는지 검토한다. 정상 사용 시나리오와 경계 사례 위주로 본다. 프로세스 격리(sandbox) 주제는 이번 범위에서 제외한다(직전 r4에서 이 환경의 중첩 sandbox 제한으로 확인 불가였다).

## 범위
- V4 확장 실행: `plan-docs/alignment/V4-official-extension-results.md` §1·§2·§5, 코드 `spikes/spike-v4-worker/`(src/lib.rs·outbox.rs, workers/, extensions/, tests/). 계약(입력·출력·허용 집계·기한·토큰 수명)과 outbox 전달 보장.
- V3-3 관리자 위임: `plan-docs/alignment/V3-standard-write-results.md` §7, 코드 `spikes/spike-v3-write/src/lib.rs`(update 효과, 커밋 시 제약 오류 분류), `tests/v3_3.rs`. V1 문법 `deferred` 불변식·`update` 효과(`spikes/spike-v1-fixture/src/{parser,sema}.rs`), V2 DDL(`spikes/spike-v2-read/src/sqlgen.rs` EXCLUDE DEFERRABLE).
- V5 SDK: `plan-docs/alignment/V5-sdk-results.md`, 코드 `spikes/spike-v5-sdk/`(src/lib.rs 계약 생성, sdk/aip.ts 타입 계층, sdk/type-tests.ts, tests/), V2 `plan.rs`의 `deps`.
- 기준: `plan-docs/sources/founder-integrated-directive-2026-10-03.md`, `plan-docs/alignment/E-technical-risks.md`(RK-04·05·08·09), `plan-docs/alignment/C-syntax-proposal.md`(B.4·B.6·B.7).
- 실행: 각 spike에서 `PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`(로컬 PostgreSQL, node, python3. V5는 `spikes/spike-0-ts/node_modules/.bin/tsc` 사용).

## 질문 (근거 파일:줄, 가능하면 실제 실행 입력·결과)
- Q1. 확장 계약: 정상 형식의 확장 코드가 선언되지 않은 집계 결과나 다른 입력 값의 결과를 얻거나, 선언 밖 출력이 호출자에게 전달되는 경우가 있는가? 같은 worker 안 동시 호출·늦은 응답·호출 id 처리에서 의도와 다른 결과가 나오는가?
- Q2. outbox 전달: 문서의 보장(커밋된 효과만, 재시도, 멱등 키로 한 번 효과, 동시 소비자, 격리 처리)이 코드·테스트와 맞는가? 소비자 중단 지점별 결과.
- Q3. 위임: 커밋 시 제약·update 효과·사후조건 검사가 의도대로 동작하는가? 위임 업무의 정상 변형(관리자 교대 직후 추가 작업, 다른 동아리와 동시 위임)에서 의도와 다른 결과가 나오는가? 교착 처리와 CONFLICT 분류.
- Q4. SDK 타입: 계약 밖 요청이 타입 검사를 통과하거나, 정상 요청이 거부되거나, 추론된 결과 타입이 서버 실제 응답(V2 실행)과 다른 경우. 생성기 출력이 facts의 일부 형태(nullable enum, 집계 guard, 관계 없는 resource)에서 잘못되는가?
- Q5. deps 건전성: 결과에 영향을 주는데 deps에 빠지는 쓰기가 있는가(다른 쿼리 형태: 필터, 정렬, 집계 guard, 단독 집계 포함)? SQL 문자열에서 테이블을 찾는 방식의 오탐·누락.
- Q6. 결과 문서 주장 중 실행 근거보다 강한 것. 다음 단계(쓰기 확장, SDK 캐시 런타임, W1 확장) 전에 정할 것과 창시자 결정이 필요한 것.

## 작성 규칙
- 발견마다 즉시 파일에 append한다. 심각도 P1(다음 단계 전 수정)/P2(권장)/P3(기록).
- 확인하지 못한 것은 "확인 못함: <이유>". 발견이 없으면 "없음"과 확인 범위. 빈칸을 채우려고 발견을 만들지 않는다.
- `codex-v345-r4b.md` 외 파일 수정·생성 금지(`target/` 제외). 변형 실험은 임시 디렉터리 복사본에서, DB는 네가 만든 임시 schema만 쓰고 지운다. `.env`·비밀 저장소 읽기 금지. git 명령 금지.

## 최종 응답
10줄 이하 요약 + 파일 경로.
