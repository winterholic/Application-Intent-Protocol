# AIP (Application Intent Protocol)

프론트엔드가 필요한 데이터와 동작을 정형화해 표현하면 서버가 정책과 계약에 따라 실행하는 프레임워크. 애플리케이션별 백엔드 반복 구현과 프론트·백엔드 계약의 이중 유지보수를 최소화한다.

> Describe your application once.

AIP는 백엔드 서버를 없애려는 것이 아니다. 기능마다 반복되는 Controller·DTO·Validation·Service·Repository·권한·트랜잭션·오류 처리·API 문서 작성을 표준화된 컴파일러와 런타임으로 옮겨, 애플리케이션별 백엔드 구현 코드를 극단적으로 줄이려는 것이다. 비즈니스 규칙을 정하고 바꾸는 책임은 여전히 개발자에게 있다.

## 제품 실행 경로

2026-10-05 사용자 요청으로 운영 인증·일반 마이그레이션·본 `crates/` 제품 조립을 구현 범위에 포함했다. `aip service`가 검증된 caller 정의와 기존 read/apply·worker·SDK 엔진을 사용한다. 앱마다 새 endpoint를 만드는 구조로 바꾸지 않는다.

- [제품 서비스](crates/aip-service/README.md): check/gen/init/adopt/migrate/serve/principal, 운영 설정과 실행 절차.
- [운영 인증](crates/aip-auth/README.md): RS256 API access token, 공개 JWKS, 서버의 지속 actor 연결과 폐기.
- [일반 마이그레이션](crates/aip-migrate/README.md): 변경 계획, 정형 backfill/convert, 실제 DDL 트랜잭션과 배포 저널. [기존 데이터 이관](crates/aip-migrate/ADOPTION.md)은 prototype 구조 marker를 검증한다.
- [설치 SDK](product/SDK.md): 기존 TypeScript 엔진을 `@aip/sdk`로 offline 설치하고 생성 binding과 함께 사용한다.
- [제품 검증 결과](product/VERIFICATION.md)와 [최신 macOS 실행 묶음](product/dist/2026-10-06T11-19-25-091Z/README.md): 실제 JWT·PostgreSQL·설치 SDK·두 wire·Node/Python 확장 실행.
- [정의 한계](spikes/spike-v1-fixture/PRODUCTION-LIMITS.md)와 [정책 실행 한계](spikes/spike-v2-read/PRODUCTION-RUNTIME.md): 입력·AST·predicate 확장·SQL·복제 payload의 상한을 함께 검사한다.

외부 IdP 계정·TLS reverse proxy 배포·다른 OS 검증은 별도 운영 환경이 필요하다. 현재 macOS의 실제 PostgreSQL과 통제된 HTTPS/JWT fixture를 사용한다. 대형 테이블 무중단 변경, 자동 역방향 migration, 최종 Id 표현·쓰기 조합 선택은 범위 밖이다.

## 기존 PoC 구조

```
.aip (작성 언어) ─▶ AST ─▶ 의미 분석 ─▶ Core IR (언어 독립, 결정적) ─▶ 실행 계획 (SQL 고정) ─▶ 런타임
                                              └─▶ 계약(/aip/describe) · 생성 클라이언트
```

| crate | 역할 |
|---|---|
| `aip-syntax` | lexer, parser, 진단 |
| `aip-sema` | 이름 해석, 타입, 정적 규칙, Core IR 하강 |
| `aip-ir` | Canonical Semantic IR(Core IR) 타입, 직렬화, digest, 구조 검증 |
| `aip-plan` | 실행 계획 타입 |
| `aip-contract` | 공개 계약, TS 클라이언트 생성, 호환성 비교 |
| `aip-pg` | PostgreSQL 백엔드: DDL과 실행 계획 생성 |
| `aip-runtime` | HTTP, 트랜잭션, 잠금, outbox, 멱등, 타이머, job, webhook, rule |
| `aip-cli` | `aip check / core / ddl / ir / explain / contract / gen-ts / run …` |

## 상태

**제품 서비스 통합 및 실행 검증 중(2026-10-05).** 기준은 [창시자 통합 지침](plan-docs/sources/founder-integrated-directive-2026-10-03.md)이며 결과물은 [plan-docs A~E](plan-docs/README.md#이번-결과물-ae)에 있다. 자체 포트 AIP 서버+공식 프론트 라이브러리, Rust 엔진, JS/TS·Python 확장, 선택적 구조화 설명·개발 검증이 제품 기준이다.

기존 `aip check/run/gen-ts` 등의 PoC Core IR 경로는 보존한다. 새 제품 진입점은 `aip service`이며 caller read/direct apply 정본을 사용한다. 독립 spike의 검증된 엔진을 path dependency로 재사용한다. `.aip` 파일 여부와 최종 문법·쓰기 조합 선택은 이번 조립만으로 확정하지 않는다.

이전 단계의 [실행 프로토타입](prototype/README.md)은 기존 엔진의 check/gen/init/serve와 단일 생성 SDK를 연결했다. source/package × 두 Id 후보의 정의 변경4개, READ8개·WRITE8개·실제 Chrome8개·재배치 bundle8개 흐름을 실행했다. 후속 DB driver 소유권·SQL stage 기한·실제 catalog 구조 검사·SDK 미확정 재시도 보존을 연결했다. 실제 커밋 응답과 다음 재시도 응답을 멈춰도 같은 key로 복구하며 중복 worker 실행을 막았다. 실행 중 세션이 만료돼도 이미 커밋한 WRITE의 성공 결과를 보존한다. 정의 숫자·중첩 진단, 쓰기 완료와 캐시 저장 사이의 경쟁 조건, CLI 출력 소비자 종료 처리도 보완했다. 최신 Rust175개(V1 포함)·SDK/패키지47개·builder5개는 실패0이며 명령과 한계는 [DB 후속 결과](prototype/README.md#정의-입력캐시-완료cli-출력-후속-결과)를 따른다. [최신 실행 bundle](prototype/dist/local-macos-20261005-input-cache/README.md)을 제공한다. 이 수치와 bundle은 이전 단계의 검증이다. 현재 제품 경로의 인증·일반 migration·명시적 prototype 이관·SDK 설치 결과는 위 제품 문서를 따른다. 구조 지문이 없는 구형 marker-only schema를 자동 채택하지 않는다.

별도 `spikes/`에서 V1부터 V15까지 후보를 실행했다. 최신 [V15 결과](plan-docs/alignment/V15-filter-value-results.md)는 공개 Url·Enum·Time 필터와 리터럴 접두어 검색을 실제 읽기·쓰기 검사에 연결한다. [V14 결과](plan-docs/alignment/V14-typed-apply-results.md)는 공개 전이의 입력/결과를 생성 타입에 연결하고 잘못된 성공 응답을 pending으로 보존한다. [V13 결과](plan-docs/alignment/V13-id-boundary-results.md)는 큰 Id 반올림으로 다른 행을 변경하는 반례를 재현하고 숫자 제한·십진 문자열 후보를 비교했다. 처음부터 쓰기를 호출하는 계약 불일치도 DB 전에 거부한다. [V12 결과](plan-docs/alignment/V12-typed-transport-results.md)는 앱별 생성 계약의 선택형 타입을 실제 HTTP·캐시 SDK에 연결하고 계약 불일치를 저장 전에 검사한다. [V11](plan-docs/alignment/V11-optional-checks-results.md)의 선택적 개발 조언·필수 검사 분리도 유지한다. [V10](plan-docs/alignment/V10-session-recovery-results.md)의 같은 사용자 세션 복구도 유지한다. [V9](plan-docs/alignment/V9-cache-lifetime-results.md)의 캐시 수명·시간 의존 조회 경계도 유지한다. 최종 계약 결정은 남아 있다.

PoC 현황: Rust 구현이 네 예제(`examples/ariari`, `shop`, `saas`, `cms`)를 실제 PostgreSQL에서 e2e로 실행한다. 정의는 PoC용 임시 프런트엔드인 `.aip` 텍스트로 쓰고, 백엔드는 언어 독립 의미 표현(`aip-ir`)만 읽는다. 진행 기록은 `docs/design/09-verification-log.md`, 결정 상태는 `docs/DECISIONS.md`.

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -q --workspace            # 로컬 PostgreSQL 필요(postgres://localhost/postgres)
./target/debug/aip check examples/ariari/app.aip
./target/debug/aip core examples/ariari/app.aip --digest
```

## 문서

| 문서 | 내용 |
|---|---|
| `docs/origin/` | 아이디어 원문: 설계 초안, 전체 맥락 인계, 설계 철학 인계(2026-10-01) |
| `docs/PRINCIPLES.md` | 최신 창시자 원칙과 설계 관문. 원문은 plan-docs/sources |
| `docs/DECISIONS.md` | 결정 기록 (Accepted / Proposed / Open / Rejected) |
| `docs/design/00-strategy.md` | 진단, 방침, 포지셔닝, 전제와 뒤집을 신호 |
| `docs/design/01-reality-test.md` | 실제 케이스 15개(아리아리 운영 코드 12 + 커머스 3), 확인된 결함 |
| `docs/design/02-language.md` | 의미 하강 탑(L3→L2→L1→L0), Core 구성요소, 도메인 형식 |
| `docs/design/03-ir.md` | Core IR 설계, 효과 서명, 분석 의무 |
| `docs/design/04-safety-matrix.md` | 정적/부팅/런타임/보장 불가 분류 45항목 |
| `docs/design/05-execution-model.md` | query/command 실행 단계, 잠금 순서, saga, outbox, 캐시, 시간 |
| `docs/design/06-extensions.md` | extension 계약, 안전 경계, 1st-party 9종 |
| `docs/design/07-architecture-and-roadmap.md` | ADR-001(구현 언어), 저장소 구조, M0-M6 |
| `docs/design/08-coverage-catalog.md` | 백엔드 케이스와 문법 매핑 |
| `docs/design/09-verification-log.md` | 검증 회차별 기록과 open issue |
| `docs/design/10-philosophy-alignment.md` | 설계 철학 대비 현재 구현 검토, 구조적 문제, 다음 단계 |
| `docs/design/11-founder-intent-audit.md` | 창시자 의도 대비 감사 |
| `docs/design/12-design-reset.md` | 설계 재개 계획 |
| `spec/grammar.md` | 현재 PoC `.aip` 문법 (EBNF). 최종 작성 형식 여부는 미정 |

`spikes/spike-0-ts/`는 첫 iteration의 TypeScript 스파이크다. 설계 판단의 근거로 보존하며 본 구현의 출발점은 아니다.
