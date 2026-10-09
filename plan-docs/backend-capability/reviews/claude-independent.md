> 검토 이력: Claude Code 독립 초안. **정본이 아니며 정정 전 주장도 보존한다.** 채택 판정과 실행 반례는 [상호 검증 정리](../cross-review.md)를 따른다. 읽기 전용이며 이 검토자가 테스트를 실행하지 않았다.

# AIP 백엔드 기능 포괄성 감사: Claude Code 독립 검토 (READ ONLY)

결론부터 말씀드리면, 제품 경로(`aip-service` → spike v1~v6)는 Postgres 리소스와 전이 엔진입니다. SQL 없는 계산, 외부 API 호출, 장기 job을 제품 Intent로 처리할 방법이 지금은 없습니다. PoC(`aip-runtime`)에는 job, schedule, webhook, 구독 기능이 있습니다. 다만 PoC의 실행 단위(`Step`)도 모두 SQL을 담고 있어서, PoC를 이식하더라도 SQL 편향은 해소되지 않습니다.

모든 근거는 코드를 직접 읽은 결과입니다. 테스트, 빌드, 서버는 실행하지 않았습니다(**미실행**: READ ONLY 지시).

## (1) 제품과 PoC의 실제 기능 차이

제품 조립 범위의 근거: `crates/aip-service/Cargo.toml:15-19`(v1, v2, v5, v6, v11에 직접 의존), `spikes/spike-v6-transport/Cargo.toml:11-15`(v3, v4는 v6를 거쳐 들어옴). `aip-runtime`에는 의존하지 않습니다.

| 영역 | 제품 (service + spike) | PoC (`aip-runtime` 계열) |
|---|---|---|
| 최상위 선언 | `enum`, `actor`, `predicate`, `access`, `limit`, `resource` 6종뿐 (`spike-v1-fixture/src/parser.rs:178-222`) | query, command, job, schedule, webhook, consume, subscribe, approval, flag, config 등 약 35종 (`aip-syntax/src/parser.rs:371-437`) |
| HTTP 경로 | `/read`, `/apply`, `/status`, `/session`, `/extension` (`spike-v6-transport/src/lib.rs:2-5`, `:430`) | Intent마다 endpoint 하나 + `/aip/health` (`aip-runtime/src/http.rs:1`, `:46`) |
| JS/Python 확장 | 있음. 단 read 확장은 `effect none` + 집계 접근만, write 확장은 `effect db` + 공개 전이만 허용 (`v1 sema.rs:1482-1508`, `:1523`). 그 밖의 종류는 `UNSUPPORTED` | 사용자 JS/Python 확장 없음. 내장 extension effect(`s3.put` 등)를 outbox로 처리 (`aip-plan/src/lib.rs:373-379`) |
| 확장 격리와 플랫폼 | macOS `sandbox-exec`에서만 동작하고 네트워크 전면 차단 (`v6 server.rs:320-325`, `v4 lib.rs:101-105`) | 해당 없음 |
| 장기 job | 없음. 요청 상한 5초 고정 (`v6 server.rs:15`), 확장 기본 deadline 1초 (`v6 lib.rs:532`) | 있음. heartbeat와 재개 지원. 단 DB item 스냅샷을 배치로 순회하는 구조 (`aip-runtime/src/jobs.rs:1-4`, `aip-pg/src/plan/job.rs:1-5`) |
| Outbox | 같은 트랜잭션에서 행을 쓰는 것까지만 (`v3 lib.rs:454-464`). 소비자는 `MockProvider`를 쓰는 spike뿐이며 제품 serve에 연결되지 않음 (`v4 outbox.rs:7-31`, `:52`) | dispatch와 outbound webhook을 서명, 재시도, 비활성화까지 구현 (`outbound.rs:1-25`) |
| 멱등성 | `/apply`의 key와 `aip_idem` 테이블을 같은 트랜잭션에 기록 (`v6 lib.rs:3`, `aip-migrate/src/adopt.rs:8`) | command `idempotent`, `idempotency_key` (`aip-plan lib.rs:265-266`) |
| 인증 | JWT와 principal 매핑 (`aip-service lib.rs:307-336`, `:346-350`) | HMAC 토큰, impersonation (`aip-runtime/src/auth.rs:1-7`) |
| Rate limit, 감사 | 없음 (제품 src에서 grep 결과 없음) | `rate_limits`, `audited` (`engine.rs:159-181`, `aip-plan lib.rs:267`) |
| 실시간 | 없음 | WebSocket과 LISTEN/NOTIFY 기반 구독, 매 재실행마다 권한 재검사 (`subscribe.rs:1-12`) |
| 파일 | 없음 | 업로드 staging, dev 환경은 로컬 디렉터리 (`objects.rs:1-3`, `aip-plan lib.rs:366-372`) |
| 관측성 | `eprintln` 한 줄(panic)뿐 (`v6 server.rs:170`). health, metrics 없음 | tracing 로그 (`http.rs:197`), health. metrics는 확인 못함 |
| 비용 제한 | read budget(rows, depth, deadline, cost, offset, cursor) (`v1 sema.rs:1461-1473`), 연결 64개, 확장 슬롯 4개 (`v6 server.rs:14`, `:247`) | rate limit, page 상한 |

## (2) A~J 판정 (제품 기준)

| 영역 | 현재 | 근거와 메모 | 적정 Level |
|---|---|---|---|
| A 데이터 | **부분** | read 조합, 전이 create/update, 유일성, check, invariant, 비관적 잠금 (`v3 lib.rs:265`, `:566`), migrate는 있음. 캐시는 실험값(`READ_CACHE_MS`, `v6 lib.rs:23`). 낙관적 잠금은 미검증 | L1·L2. 쓰기 정책은 L3 |
| B 보안 | **부분** | JWT와 access predicate는 있음. 테넌트 전용 구문이 없음(`v1 src`에서 `tenant` grep 0건). API key, rate limit, audit, 위임은 미지원 | L1 + L3 |
| C 업무 로직 | **부분** | 전이(from, to, allow, repeat, effects)로 상태 머신은 표현됨 (`parser.rs:440-524`). 보상 작업과 예외 분기 없음. 확장 기본 실행 시간 1초 | L3. 특수 로직은 L4 |
| D 비동기 | **미지원** | job, schedule, cancel, retry 경로 없음 | L1 런타임 + L3 선언 |
| E 이벤트 | **부분 미만** | `notify`가 outbox 행만 기록하고 소비자는 미연결 | L1 |
| F 외부 연동 | **미지원** | 확장 네트워크 차단, secret 주입 경로 없음 | L1 capability + L4 |
| G 파일 | **미지원** | 해당 코드 없음 | L1 저장 + L4 변환 |
| H 실시간 | **미지원** | 해당 코드 없음 | L1 |
| I 운영 | **부분 미만** | 배포 fence와 digest는 강함 (`v6 lib.rs:475-499`). 로그, metrics, health, quota 없음 | L1 |
| J AI 친화 | **부분** | 5개 작성 형식(`v1 main.rs:10`), 타입 TS 계약과 지문 (`v5 lib.rs:337-343`), dev-checks. 런타임 카탈로그 없음. `docs.summary`는 facts에만 남고 SDK에서는 grep 0건 (`v1 sema.rs:178-180`) | L1 |

Level 1~4 구분 자체는 적절하다고 봅니다. 다만 현재 제품의 L4는 "집계 읽기"와 "공개 전이 호출"만 허용합니다(`worker.mjs:18-23`). 그래서 L4로 넘어가야 할 외부 호출이나 계산이 실제로는 들어갈 곳이 없습니다. 효과 종류(none, db, external, long)를 Level과 별도의 축으로 두지 않으면 분류가 무너집니다.

## (3) 구조적 문제와 반례

1. **Intent가 resource, 곧 테이블에 종속됩니다.** 확장은 `resource` 안에만 선언할 수 있고(`parser.rs:541`), 실행할 때도 `facts.resources[r].extensions[e]`로 찾습니다(`v6 lib.rs:531-532`).
   - 반례: "PDF 두 개를 병합해 달라"는 요청은 가짜 resource를 만들어야만 표현할 수 있습니다.
2. **DB 없이는 실행이 불가능합니다.** 모든 경로가 DB 연결과 배포 fence 조회를 거칩니다. `effect none`인 확장도 예외가 아닙니다(`v6 lib.rs:471-506`).
   - 반례: DB 장애 중에는 순수 계산 요청도 `DB_UNAVAILABLE`로 실패합니다.
3. **요청과 응답만 있는 동기 모델입니다.** 요청 상한이 5초이고(`server.rs:15`), worker를 호출마다 띄웠다가 내립니다(`lib.rs:516-539`). run id, 진행률, 취소가 없고, `/status`도 실행 중인 쓰기를 증명하지 못합니다(`lib.rs:4`).
   - 비교: Temporal은 취소를 heartbeat로 전달하고, heartbeat가 없는 activity는 취소를 받을 수 없다고 문서에 명시합니다([docs.temporal.io](https://docs.temporal.io/encyclopedia/detecting-activity-failures)).
   - 비교: Hasura는 async action이 즉시 `action_id`를 반환하고, 결과는 query나 구독으로 받는 구조입니다([hasura.io](https://hasura.io/docs/2.0/actions/async-actions/)).
4. **확장 ABI가 지나치게 좁고, 운영 환경에서 쓸 수 없습니다.** ctx에는 `data.aggregate`와 `data.apply`만 있습니다. http, secret, file, progress, 취소 신호가 없습니다. 게다가 macOS가 아니면 설정 단계에서 거부되므로(`server.rs:320`), Linux 서버에서는 확장 자체를 쓸 수 없는 것으로 읽힙니다. 실행으로 확인한 것은 아닙니다.
5. **PoC 이식은 해법이 아닙니다.**
   - PoC의 `Step`은 Let과 Check까지 모두 `sql: Sql`을 담습니다(`aip-plan lib.rs:304-389`). 런타임도 "PostgreSQL에 대해 실행한다"고 스스로 밝힙니다(`aip-runtime lib.rs:1-2`).
   - PoC job도 DB item 스냅샷을 순회하는 구조라 임의 계산에는 맞지 않습니다.
   - 선언 종류가 약 35개라서 §8.2(결정의 명확성)와 긴장합니다. 과도한 범용화의 실제 사례입니다.

## (4) 재사용할 부분

- **확장 토큰 grant 모델** (`v4 lib.rs:65-74`): 서버가 actor, 접근 범위, 기한을 토큰에 묶습니다. 효과 capability를 일반화하는 기반으로 적합합니다.
- **멱등 결과 저장**: 같은 트랜잭션에 결과를 기록하는 `aip_idem` 패턴. run 기록에 그대로 확장할 수 있습니다.
- **outbox 소비 패턴** (`outbox.rs:52-93`): SKIP LOCKED, 시도 횟수, dead 처리. job queue 골격으로 쓸 수 있습니다.
- **계약 지문 검사와 배포 fence**: 새 실행 종류에도 그대로 적용할 수 있습니다.
- **PoC 코드는 동작을 이식하지 말고 의미 명세의 참고 자료로만 씁니다.** 대상은 outbound 서명과 재시도 의미(`outbound.rs:6-25`), 구독 권한 재검사(`subscribe.rs:11-12`), 필드 암호화 AAD 설계(`crypto.rs:1-11`)입니다.

## (5) 우선순위 제안과 트레이드오프 (분석과 제안만)

1. **resource에 독립적인 operation 계약.** 입력, 출력, effect 종류, deadline, 멱등성, 비용을 하나의 계약으로 묶습니다. DB fence는 effect가 db일 때만 적용합니다. 문제 1, 2를 직접 해소합니다.
   - 트레이드오프: 최상위 선언이 하나 늘어납니다. 대신 "확장을 붙이려고 만드는 가짜 resource"라는 선택지가 사라집니다.
2. **영속 run 기록.** `/status`를 run id 기준으로 바꾸고 progress, cancel, retry/backoff를 붙입니다. 위 aip_idem과 outbox 패턴을 재사용합니다.
   - 트레이드오프: worker를 상주시키거나 별도 프로세스로 운영해야 합니다.
3. **capability 기반 외부 효과.** 서버가 허용한 host와 secret 참조만 ctx로 주입합니다. 플랫폼에 중립적인 격리 방식도 함께 필요합니다.
   - 트레이드오프: 네트워크 전면 차단이라는 현재의 단순한 안전성을 포기하게 됩니다.
4. **공통 운영 계층.** rate limit, quota, audit, 구조화 로그와 metrics를 Intent별이 아니라 런타임 공통으로 둡니다.
5. **런타임 카탈로그 노출.** `docs` metadata를 포함해 AI가 사용 가능한 기능을 발견할 수 있게 합니다.

반복 구현이 얼마나 줄어드는지는 측정하지 않았으므로 수치로 보장할 수 없습니다. 새 문법 예시는 의도적으로 넣지 않았습니다.

## (6) 6개 시나리오 중 핵심 누락

- **F (데이터와 무관한 작업)**: 전 단계가 불가능합니다. 5초 상한, 진행 상태 없음, 취소 없음.
- **D (외부 서비스 연동)**: 외부 호출, secret, retry가 모두 불가능합니다.
- **A (전자상거래)**: 재고 잠금과 불변조건은 됩니다. 결제 호출, 보상 작업, 알림 실제 발송이 빠져 있습니다.
- **E (실시간 협업)**: 제품 경로가 아예 없습니다.
- **B (업무 승인)**: 예약 실행이 빠져 있습니다.
- **C (SaaS 멀티테넌트)**: 테넌트 구문과 사용량 집계가 없습니다. `limit atMost`는 행 개수 제한만 표현합니다.

## (7) 확인 못한 사항

- 테스트, 빌드, 서버 실행은 하지 않았습니다. 위 판정은 모두 코드 읽기에 근거합니다.
- 다음은 직접 읽지 않았습니다: Python worker와 Node worker의 동등성, 제품 낙관적 잠금, predicate만으로 테넌트 격리가 충분한지, PoC metrics와 cancel 유무.
- Temporal과 Hasura의 라이선스, 특허 조건은 확인하지 않았습니다.
- 공식 문서는 위 두 URL만 직접 읽었습니다. Spring, NestJS 등 나머지 프레임워크는 조사하지 않았습니다.