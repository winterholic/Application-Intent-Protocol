# 07. 아키텍처와 로드맵

> 상태: Proposed · 구현 언어는 사용자 결정 대기(ADR-001)

## 1. ADR-001: Core 구현 언어

**결정 요청**: 컴파일러와 런타임을 Rust로 시작할 것인가.

**권고: Rust로 시작한다.** 단 semantics 검증은 언어와 독립된 conformance suite가 담당한다.

| 기준 | Rust | TypeScript(Node) | Java/Kotlin |
|---|---|---|---|
| `aip run` 단일 바이너리 배포 | ○ | △ (런타임 필요) | △ (JVM 필요, GraalVM native는 제약 많음) |
| WASM 샌드박스 호스트 (extension, escape hatch) | ○ wasmtime이 Rust 네이티브 | △ | △ |
| IR 같은 대수적 자료형과 전수 매칭 | ○ enum + match | △ (discriminated union, 전수성 약함) | △ (sealed class) |
| 컴파일러 생태계 (파서, LSP, 증분 계산) | ○ rowan, chumsky/lalrpop, tower-lsp, salsa | ○ | △ |
| PostgreSQL/Redis/Kafka 드라이버 성숙도 | ○ tokio-postgres/sqlx, redis-rs, rdkafka | ○ | ○ |
| 사용자 숙련도 | △ (Python/Java 중심) | ○ | ○ |
| semantics 변경 비용 | 높음 | 낮음 | 중간 |

**semantics 변경 비용이 높다는 약점을 줄이는 방법**: 의미는 코드가 아니라 **IR 명세 + conformance suite**(Reality Test와 커버리지 카탈로그의 케이스별 기대 진단/기대 동작)가 소유한다. 구현 언어를 바꾸더라도 suite가 남는다. spike-0(TypeScript)은 참고용으로 보존한다.

**뒤집을 조건**: M1 종료가 계획 대비 2배 이상 지연되면 런타임은 TypeScript로 먼저 검증하고 컴파일러만 Rust로 유지한다.

**환경 확인**: 이 머신에 `rustc`/`cargo`가 설치되어 있지 않다(2026-09-30 확인). rustup 설치가 선행 작업이다.

## 2. 저장소 구조 (목표)

```
aip/
  spec/                   언어 명세 (grammar, IR 스키마, 진단 코드 목록)
  conformance/            케이스별 .aip + 기대 진단(.expect.json) + 기대 런타임 시나리오
  crates/
    aip-syntax/           lexer, parser, CST(rowan), formatter
    aip-hir/              이름 해석, L3 하강, 타입 검사
    aip-ir/               Core IR 타입, 직렬화, 버전
    aip-analyze/          정적 규칙, obligations, authority graph
    aip-plan/             query/command planner, explain
    aip-runtime/          HTTP(axum), 실행기, dispatcher, saga, 타이머, 스케줄
    aip-ext-api/          extension trait, 효과 서명, WIT 인터페이스
    aip-contract/         계약 생성, diff, 클라이언트 코드 생성기
    aip-cli/              aip check/lower/plan/explain/verify/migrate/run/diff/gen
    aip-lsp/              에디터 지원
  extensions/
    postgres/ redis/ kafka/ s3/ http/ mail/ payments/ auth-oidc/ notify/
  clients/
    ts/core/  ts/react/   @aip/client, @aip/react
  examples/
    ariari/               아리아리 전체 포팅 (M6)
    commerce/             Case 13-15
  spikes/
    spike-0-ts/           iteration 1 TypeScript 스파이크 (보존)
  docs/
```

## 3. 마일스톤

각 마일스톤은 **종료 조건**이 증거로 확인될 때만 닫는다.

### M0. 명세와 conformance 골격
- 산출: `spec/grammar.md`(EBNF), `spec/diagnostics.md`(진단 코드 전수), `conformance/` 케이스 파일(Reality Test 15 + 커버리지 카탈로그 전 항목의 예제)
- 종료 조건: 카탈로그의 모든 항목이 "표현 예제 있음 / L3 형식 필요 / escape hatch / 범위 밖" 중 하나로 판정되어 있고, 모든 예제가 명세 문법만 사용한다(수동 검증 → M1부터 파서로 자동 검증).

### M1. 컴파일러 프런트엔드
- 파서, 포매터, 이름 해석, 타입 검사, L3 하강(lifecycle, unique/exactly one/no overlap, erase, retain, counter, cached)
- 진단 JSON 형식(위치, 코드, 원인, 제안 패치)
- 종료 조건: conformance 전 예제 파싱 성공. 진단 기대값 케이스 100% 일치. `aip lower`로 하강 결과 확인 가능.

### M2. 분석기 + PostgreSQL 런타임
- obligations(읽기/쓰기/잠금 집합, 효과 배치, retry safety, authority graph)
- aip-postgres: 스키마/제약 하강, 마이그레이션 diff(파괴적 변경 게이트), 부팅 시 드리프트 검사
- 실행기: 문맥 조회, 정렬 잠금, 집합 연산, 지연 불변식, outbox, 멱등
- query planner: LATERAL+json_agg 단일 SQL, 배치 대체 전략
- `aip explain`, `aip verify`
- 종료 조건: Case 2, 3, 4, 6, 9, 10, 11 런타임 시나리오 통과. 동시성 테스트(중복 지원, 관리자 위임 경쟁, 기간 겹침 경쟁) 통과.

### M3. 내구성 효과
- dispatcher, saga 복구 루프, 타이머, 스케줄 리더, 보존 기한
- aip-http, aip-mail, aip-payments(fake PG 먼저, 이후 샌드박스 PG), aip-s3(MinIO)
- 종료 조건: Case 1, 5, 7, 12, 13, 14 통과. 장애 주입 테스트(외부 효과 타임아웃, 커밋 직후 프로세스 kill, PG 응답 유실) 후 최종 상태 수렴.

### M4. 캐시·스트림·인증
- aip-redis(counter, cached, rate limit), aip-kafka(Redpanda), aip-auth-oidc(카카오/구글), aip-notify
- WASM extension/escape hatch 기술 스파이크
- 종료 조건: Case 8, 15 통과. 캐시 무효화 경쟁 테스트 통과. OIDC 로그인 흐름 통과.

### M5. 클라이언트와 계약
- 계약 v1(보장 목록 포함), `aip diff --against deployed`, `@aip/client`, `@aip/react`(`useQuery`, `useCommand`, 구독)
- 종료 조건: 생성 클라이언트만으로 커머스 예제 프런트 시나리오 동작. breaking change 탐지 케이스 통과.

### M6. 증명: 아리아리 전체 포팅
- 아리아리 백엔드 전 기능을 AIP로 재구현, 원본 프런트(Next.js)를 생성 클라이언트로 연결
- 측정: 코드량(원본 Java vs AIP 정의 + escape hatch), 결함(원본에서 확인된 결함의 AIP 검출/차단 여부), 성능(k6로 동일 시나리오 p50/p95), LLM 생성 실험(요구사항 → .aip 생성 → 진단 → 자가수정 성공률)
- 종료 조건: `00-strategy.md` 7절 전제들을 판정하고 결과를 문서로 남긴다. 전제가 틀렸으면 그것도 결과다.

## 4. 검증-보충 루프 (상시 운영)

사용자 지시: "백엔드에서 개발할 만한 모든 케이스를 커버하고, 계속 검증해 나가면서 보충한다." 이를 다음 루프로 운영한다.

```
1 수집    새 백엔드 케이스를 커버리지 카탈로그(08)에 추가
2 표현    케이스를 명세 문법으로 작성 (conformance/<category>/<case>.aip)
3 판정    표현 가능 / 새 L3 형식 필요 / L2 개정 필요 / escape hatch / 범위 밖
4 검증    (M1 이후) 파서·분석기로 자동 확인, 기대 진단 작성
          (M2 이후) 런타임 시나리오와 동시성·장애 주입 테스트
5 보충    판정 결과에 따라 명세·형식·extension 보강, 변경은 검증 로그(09)에 기록
6 회귀    기존 케이스 전부 재실행. 하나라도 깨지면 보충을 되돌리거나 재설계
```

L2 개정은 무겁게, L3 형식 추가는 가볍게 다룬다(`02-language.md` 1절). escape hatch 판정이 한 카테고리에서 반복되면 그 카테고리에 L3 형식이 빠졌다는 신호다.

## 5. 리스크와 대응

| 리스크 | 신호 | 대응 |
|---|---|---|
| DSL 팽창 | L2 개정 요청이 분기당 2건 초과 | L3 형식으로 흡수 가능한지 먼저 검토 |
| 누수 추상화 | 성능 문제를 `aip explain`으로 설명할 수 없음 | planner hint를 1급으로, SQL 확인 경로 유지 |
| 런타임 마법 | 사용자가 "왜 이렇게 동작하는지" 물을 때 문서가 아니라 코드를 읽어야 함 | 모든 하강과 배치를 `aip lower`/`explain`으로 노출 |
| 분산 부작용 단순화 | 장애 주입 테스트에서 수렴 실패 | 해당 효과 등급 재정의 |
| escape hatch 지배 | 포팅 시 escape hatch 비율 30% 초과 | 해당 카테고리 L3 형식 설계 |
| 학습 비용 | LLM 생성 실험에서 진단 후 자가수정률이 낮음 | 진단 메시지와 제안 패치 개선 |
