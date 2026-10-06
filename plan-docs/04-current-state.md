# 04. 현재 구현과 재사용 가능성

> 상태: 초안 v0 (2026-10-03). 원문 PART IV를 저장소 파일로 대조했다. 테스트 수치는 다시 돌리지 않았다(감사 시점 값).
> 현재 재검토 기준: [A 창시자 의도 반영표](alignment/A-founder-intent-matrix.md). 아래 감사 수치는 당시 기록이며, ‘일치/구현·테스트’ 표기는 새 조합 경로 안전성 보장이 아니다. 이번 source 대조에서는 보편 비용/시간 상한·공식 host 확장·metadata를 미확인으로 구분했다.

## 1. 지금 있는 것 (파일 기준)

```
.aip 텍스트 (독자 문법)
  → crates/aip-syntax    파서
  → crates/aip-sema      이름·타입 해석, .aip → Core IR 하강(to_core.rs)
  → crates/aip-ir        언어 독립 의미 표현, 검증(validate), 의미 규칙(analyze), diff, 진단 코드 레지스트리
  → crates/aip-pg        PostgreSQL DDL, 컴파일 시점에 고정한 SQL 실행 계획, 스키마 진화(evolve.rs)
  → crates/aip-plan      실행 계획 타입
  → crates/aip-runtime   Rust 단일 서버: HTTP POST /aip/{intent}, WebSocket 구독, 트랜잭션, outbox, job, 타이머, 웹훅
  → crates/aip-contract  공개 계약(/aip/describe), TS 클라이언트 생성, 호환성 판정
  → crates/aip-cli       aip check / run / migrate / diff / gen-ts 등
```

| 사실 | 근거 | 확인 |
|---|---|---|
| crate 8개 | `ls crates` | 10-03 확인 |
| 예제 4개: ariari 1,075줄, saas 369줄, cms 150줄, shop 115줄 | `examples/*/app.aip` | 10-03 `grep -c` 확인 |
| JS/TS 코드는 생성된 클라이언트(`examples/*/client/aip.ts`, 골든 파일)뿐 | 저장소 검색 | 10-03 확인 |
| Python 코드는 문서 검사 도구(`tools/doccheck.py`)뿐. Python 연동 없음 | 저장소 검색 | 10-03 확인 |
| extension 효과는 런타임에 하드코딩(`mail.*`, `auth.*`, `export.*`) | `crates/aip-runtime/src/dispatch.rs` 223·230행 | 10-03 확인 |
| 테스트 182 passed | 감사 보고 | 감사 시점 값, 재실행 안 함 |
| 아리아리 원본 Java 478파일 21,552줄 | 감사 보고 | 확인 필요: 테스트 포함 여부 미구분 |
| CI 없음 | 감사 보고 | 재확인 안 함 |

## 2. 원칙과 맞는 곳, 어긋난 곳

| 원칙 | 판정 | 요지 |
|---|---|---|
| 존재 목적 | 부분 일치 | 계층 코드는 사라졌지만 새 화면이 데이터를 원하면 서버 정의에 query를 하나 더 써야 한다 |
| 작동 모델 (호출자 표현) | **충돌** | 호출자는 선언된 이름과 입력만 보낸다. 조합 요청 경로가 없다 |
| 요청 자유와 실행 권한 분리 | **불일치** | 자유를 없애는 방식으로 안전을 얻었다 |
| 보안 (서버 최종 통제) | 일치 | 정책, 행·필드 가시성, 테넌트, 대리 접속, 멱등, SSRF 차단이 구현·테스트됨 |
| 프로토콜 | 부분 일치 | 형식(계약, 버전, 오류 레지스트리)은 갖췄으나 의미가 "이름 붙은 RPC"에 가깝다 |
| SPR | 구조는 일치 | Presentation이 표현하지 못하고 호출만 한다 |
| Rust 엔진 | 일치 | |
| JS/Python 생태계 | **연결 부족** | 공식 JS/TS·Python 확장·프로젝트 연동 경로 없음. `.aip` 존재 자체는 충돌이 아니며 독립 파일 여부 OPEN |
| AI 철학 (반복 흡수) | 일치 | approval, job, tenant, versioned, search, 웹훅 등 다수의 표준 형식 |
| 자연어의 문법화 | **미반영** | 문법에 해당 개념이 없다 |
| 확장 | **미반영** | Extension ABI 없음. `fn … wasm`은 실행 경로 없음 |

출처: `../docs/design/11-founder-intent-audit.md` §B. 이 표는 그 판정을 다시 본 것이고, 10-03 원문 기준으로 바꿀 점은 하나다. 감사는 `.aip`를 "충돌"로 봤지만 10-03 원문은 고유 문법 자체를 잘못이라고 단정하지 않는다. 충돌의 실체는 "JS/Python 생태계에서 자연스럽게 쓰는 경로가 없다"는 것이고, `.aip`가 존재한다는 사실 자체는 아니다(T02).

## 3. 왜 어긋났나 (원문 PART V)

1. 기준 문서가 이미 방향을 굳혔다. `../docs/origin/design_draft.md` §3 "CancelOrder는 서버에서 사전에 정의되고 검증된 capability", `../docs/origin/full_context_handoff.md` §0 "GraphQL 비슷한 query language로 이해하면 잘못".
2. 그 문장을 근거로 에이전트가 D-P8("클라이언트 임의 쿼리 없음")을 결정 기록에 넣었다.
3. 이후 작업 우선순위를 원칙표가 아니라 에이전트 판단으로 정했다. IR, 진단 레지스트리, 스키마 진화, 암호화가 품질은 높지만 존재 목적보다 앞서 갔다.

## 4. 재사용 가능성 (D8 자료 초안)

판정은 `[추천]`이다. 최종 처분은 T01·T02·T03 결정 뒤에 정한다.

| 자산 | 위치 | 새 모델에서의 가치 | 추천 |
|---|---|---|---|
| 서버 권한 층 (정책, 행·필드 가시성, 테넌트, 대리 접속, 감사) | `aip-pg/src/plan.rs`, `ddl.rs`, `aip-runtime/src/engine.rs` | 조합 Intent도 같은 정책을 통과시키면 된다. 호출자 자유를 넓힐수록 가치가 커진다 | 유지, 조합 경로에 맞게 확장 |
| 효과 등급, outbox, job, 타이머, 웹훅 내구성 | `aip-runtime` | 호출자 모델과 무관하게 필요 | 유지 |
| 언어 독립 의미 표현과 검증 | `aip-ir` | 작성 방식이 여럿이 되든 하나로 바뀌든 의미를 한 곳에 모으는 후보. 단 T08에서 필요성을 다시 판단 | 보류 |
| 스키마 진화, 계약 호환 판정 | `aip-pg/src/evolve.rs`, `aip-contract/src/compat.rs` | 유지보수 최소화에 직접 기여 | 유지 |
| 진단·오류 코드 레지스트리 | `aip-ir/src/codes.rs` | AI First의 "구조화된 오류" | 유지, 코드 체계는 재검토 |
| 컴파일 시점 SQL 고정 | `aip-pg` | 선언 Intent 경로에는 유효. 조합 Intent에는 런타임 계획이나 빌드 시점 승인이 필요 | 보류(T10) |
| `.aip` 파서와 문법 | `aip-syntax`, `spec/grammar.md` | T02 결정에 달림 | 보류 |
| 생성 TS 클라이언트 | `aip-contract` tsgen | 호출 방식이 "이름 + 입력"에 고정 | 수정 필요(T01·T06) |
| 단일 서버 런타임 | `aip-cli` `aip run` | 자체 포트 AIP 서버+프론트SDK 제품 구조에 부합 | 유지 후보. 설치·기동·worker·배포 재검증 |
| e2e 검증 방식 (실제 PG, 골든 비교, negative control) | `crates/aip-cli/tests` | 설계와 무관하게 유효 | 유지 |
| 아리아리 재구현 정의 | `examples/ariari/app.aip` | 실서비스 시나리오 자료로 가치. 원본 결함 목록과 함께 회귀 기준 | 유지(자료로) |

## 5. 다시 확인해야 할 것

- `cargo test -q --workspace` 현재 결과 (설계 단계라 급하지 않음)
- 필드 암호화: 작업 중 사용량 한도로 멈췄고 검증 로그가 없다. 인용 전에 재검증
- 아리아리 Java 줄 수의 테스트 포함 여부

## 변경 이력

- 2026-10-03 초안 v0
- 2026-10-03 최신 반영표 연결, .aip 자체 충돌 판정 해소·독립 서버 제품 기준 반영.
