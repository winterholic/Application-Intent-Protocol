너의 작업 결과물은 파일 하나를 쓰는 것이다: `plan-docs/reviews/codex-v7-r8.md`.

# 마이그레이션 사전 검사 실험 정확성 검토 (r8)

이 저장소는 사용자 본인이 설계 중인 오픈소스 애플리케이션 프레임워크(AIP)다. 실험 코드가 문서의 주장대로 동작하는지 정상 사용 시나리오와 경계 사례로 검토한다.

## 범위
- `plan-docs/alignment/V7-migration-results.md`, 코드 `spikes/spike-v7-migrate/`(src/lib.rs, tests/v7.rs). V1 facts(`spikes/spike-v1-fixture`), V2 SQL 생성기(`spikes/spike-v2-read/src/sqlgen.rs`).
- 기준: `plan-docs/sources/founder-integrated-directive-2026-10-03.md`(마이그레이션·호환 관련 절), `plan-docs/alignment/E-technical-risks.md`(RK-10).
- 실행: `cd spikes/spike-v7-migrate && PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`(로컬 PostgreSQL). 변형은 임시 복사본에서, DB는 코드의 기본 schema 이름도 바꾼 임시 schema만 쓰고 지운다.

## 질문 (근거 파일:줄, 가능하면 실제 실행 입력·결과)
- Q1. 분류가 틀리는 정상 변경이 있는가? 특히 Safe·Breaking으로 분류돼 자동 적용 가능해지는데 실제로는 권한이 넓어지거나 데이터가 손실되는 변경(관계 대상 공개 필드 변화, traverse 추가, 집계 sourceAccess 변경, 전이 allow 변경, expose apply·create·compose 변경, budget 변경, predicate 본문 변경 등 diff가 보지 않는 facts 부분).
- Q2. 데이터 측정(정책 확대 판정, 불변식 위반 수, enum 사용 행, null 행)의 정확성과 한계.
- Q3. 문서 주장 중 실행 근거보다 강한 것. 다음 단계 전에 정할 것과 창시자 결정이 필요한 것.

## 작성 규칙
- 발견마다 즉시 파일에 append한다. 심각도 P1(다음 단계 전 수정)/P2(권장)/P3(기록).
- 확인하지 못한 것은 "확인 못함: <이유>". 발견이 없으면 "없음"과 확인 범위. 빈칸을 채우려고 발견을 만들지 않는다.
- `codex-v7-r8.md` 외 파일 수정·생성 금지(`target/` 제외). `.env`·비밀 저장소 읽기 금지. git 명령 금지.

## 최종 응답
10줄 이하 요약 + 파일 경로.
