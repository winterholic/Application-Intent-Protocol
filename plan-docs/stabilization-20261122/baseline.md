# AIP 안정화 공유 기준선 (S0)

- **기준 HEAD**: `origin/main` = `596ab5dd23b9e1aa7bd0b63b48dd7c3f37821003` (2026-11-21 21:39 +0900, "ci: fix the v6 probe clippy lint and run the upgrade-drill unit tests in Verify"). 작성일 2026-11-22.
- 목적: 후속 Track A~F 작업자가 저장소 전체를 다시 조사하지 않고 쓰는 단일 기준선이다. 감사·hardening 문서의 항목을 **현재 코드·테스트·커밋**으로 재분류했다. 이미 해결된 것은 작업 대상에서 뺐다.
- 이 문서는 1.0 출시 승인이 아니다. `v1.0.0` 태그, GitHub Release, npm, PyPI 게시는 이번 안정화 범위에서 금지다. `aip --version`이 `1.0.0`인 것은 문자열 정합일 뿐 안정 릴리스 선언이 아니다.
- 이 문서가 바꾼 것: 이 파일 하나. 제품 코드, 테스트, workflow는 바꾸지 않았다.
- 작성 방식: 문서 읽기 + 코드 대조 + 표적 실행(5절). 실행하지 않은 것은 `미실행` 또는 `코드 확인만`으로 적었다. 무거운 전체 스위트(`tools/verify.sh`, MySQL, Linux bwrap, macOS Seatbelt)는 돌리지 않았다.

## 0. 읽는 법

| 분류 | 뜻 | 작업 대상인가 |
|---|---|---|
| **해결됨** | 코드와 테스트(또는 커밋)로 확인했다 | 아니오. 다시 열지 않는다 |
| **미해결 결함** | 재현했거나 코드 근거가 명확하다. 공개 계약을 바꾸지 않고 고칠 수 있다 | 예. 실제 수정 + RED→GREEN |
| **결정 필요** | 응답 값, 호환성, 공개 계약이 걸려 소유자 결정 없이 확정할 수 없다 | 선택지 정리까지. 결정 대기 때문에 독립 작업을 멈추지 않는다 |
| **범위 밖** | 1.0 지원 범위 밖(MySQL, 자동 tenant 격리, 암호화 필드, Windows 등) | 아니오. 지원 문서에 이미 있다 |
| **근거 부족** | 확인하지 못했다. 지원 근거로 쓰지 않는다 | 해당 Track이 먼저 확인한다 |

근거 표기: `파일:줄`은 이 HEAD 기준이다(줄은 바뀌므로 함수 이름도 함께 적었다). `커밋`은 `origin/main`의 짧은 SHA와 제목 일부다. `실행(이번)`은 이 작업에서 596ab5dd 소스로 직접 돌린 것이다(5절).

## 1. 한눈 요약

1. **이미 해결됨(다시 하지 말 것)**: 숨은 필드 우회 D1~D13, 공개 count의 `callerFilter read` 오라클과 공개 count 경고, 외부 저장소 create 판정 순서, `/apply` 멱등 조회 오류 무시, SERIALIZABLE 확장의 `WIRE.INTERNAL` 누출, 줄 생성 뒤 compose·`target.where` 교착, worker slot 선점, 표현력 P0-A~D(`refs`, 제한 산술, `ctx.data.create`, bundle 안 compose), Decimal sum, BOM, 앱 패키저 fail-open, 1.0.0 버전 정합, release-candidate 집계 workflow. 목록과 근거는 2.9절.
2. **미해결 결함(우선순위 순)**:
   - 독립 소비자 E2E가 과거 Core `4e132564`에 고정돼 있고 `release-candidate`에도 없다(C-01, F-01).
   - 반대 순서 bundle 교착은 이번에 재현했다. 2건 동시에 1.18초(교착 1회 후 재시도), 계약 결정이 필요하다(B-01).
   - `durable.rs:1201` `if let Ok(Some(row))`가 취소 확인 쿼리 오류를 조용히 버린다(B-02). 중복 실행은 없다고 코드로 추정했고 재현은 안 했다.
   - macOS 전용 테스트 5곳이 어떤 workflow에서도 실행되지 않는다(B-05).
   - 진단 JSON의 `line:0, column:0` 등 G1~G10 전부 미해결(E-01).
   - 어떤 workflow에서도 안 도는 테스트 파일이 있다(F-05).
3. **결정 필요**: 공개 count 기본 의미(O1~O5)와 F1(`where`+`input` count), K1~K4, R1(읽기 확장 무선언 읽기), E9 잠금 계약(A~D), 단독 집계 budget, 진단 JSON 장기 호환, 확장 ABI. 선택지는 각 문서에 있고 3절은 그 위치를 가리킨다.
4. **현재 HEAD 상태**: Verify, release gate, App packages, External brokers, Linux cgroup, Object storage, Linux worker jobs, macOS platform tests는 `596ab5dd`에서 success다. Database contracts, Deployment checks, Dependencies, FlaskBB 2종, SDK semantic e2e, Semantic conformance는 **이 SHA에서 실행되지 않았다**(경로 필터). `Release candidate`는 `596ab5dd`에서 실행된 적이 없다(마지막 `be94d064`, 2026-11-21 03:49Z). main 브랜치 보호는 없다(API 404).

## 2. 분류표

### 2.1 감사 문서 P0/P1 (`plan-docs/reviews/2026-11-21-1.0-release-audit.md`, 기준 `723fb474`)

감사는 시점 기록이다. 아래가 현재 상태다.

| ID | 감사의 문제 | 현재 분류 | 근거 |
|---|---|---|---|
| P0-1 | 버전 0.1.0 | **해결됨** | `Cargo.toml:17 version = "1.0.0"`. 실행(이번) `aip --version` → `aip 1.0.0`. 버전 일치 검사 `scripts/check-versions.mjs`는 `app-packages.yml:58` |
| P0-2 | `callerFilter read` count로 숨은 행 값 추정, 기본 `check`가 침묵 | **해결됨**(오라클). 공개 count **기본 의미는 결정 필요**(A-02) | 커밋 `9db9905c` 공개 count 오라클 차단·경고. `sema.rs:2368` `DEF.CALLER_FILTER_POLICY_REQUIRED`, `sema.rs:2386` `DEF.ADVISORY_PUBLIC_COUNT`. 테스트 `spikes/spike-v1-fixture/tests/advisories.rs::a_where_that_does_not_imply_the_row_policy_is_rejected` 외 7개, 실행(이번) 8 passed. 경고가 `--dev-checks` 없이 나오는 것은 `spike-v11-dev-checks/src/lib.rs:19` `ALWAYS_ON`, `crates/aip-cli/tests/extensions.rs:135` |
| P0-3 | 필수 CI가 출시 후보 SHA에서 안 돎, flaskbb workflow에 dispatch 없음 | **부분 해결**. 집계 workflow는 생겼고 경로 필터·보호·소비자 누락은 남음 | 해결: `release-candidate.yml`(`needs` 15개, `required` job이 전부 `success`만 통과), `flaskbb-parity.yml`·`flaskbb-post-order.yml`에 `workflow_dispatch` 있음. 미해결: F-01~F-04 |
| P0-4 | main `Verify` 실패(step 17 MySQL FaceReview 브라우저, `reload_main`) | **미해결 결함**(간헐, 원인 미확정) | `596ab5dd`의 Verify는 success. `cb37868b`의 Verify는 failure이고 로그의 실패 테스트는 `isolation::tests::runtime_probe_timeout_keeps_leader_identity_owned_until_cleanup`(`spikes/spike-v4-worker/src/isolation.rs:1296` panic, Run `44904438206`)로 감사의 step과 다르다. 두 건 모두 원인 규명 문서가 없다 → F-06. step 17은 `verify.yml:295-311`에 그대로 있다 |
| P1-1 | v11 dev-checks 테스트가 CI에 없음 | **해결됨** | `tools/verify.sh` 엔진 루프에 `spike-v11-dev-checks` 포함. 의도적 제외 `spike-v7-migrate`는 주석에 이유가 있다 |
| P1-2 | 생성 문서(`definition-syntax.md`)가 실제와 다름 | **해결됨** | 커밋 `9db9905c`. `spikes/spike-v1-fixture/src/syntax_card.rs:128-133`. 실행(이번) `to done = true; views = views + 1` → 진단 메시지가 "`to` 절의 대입은 쉼표" 안내 |
| P1-3 | 단독 집계 budget 없음, 고정 2초 | **결정 필요** | `spikes/spike-v2-read/src/plan.rs:316,1097 deadline_ms: 2000`. 선언 허용 또는 1.0 한도 문서화 → D-01 |
| P1-4 | 사용자 진단에 "spike" 문구 | **해결됨** | `sema.rs`에 사용자 메시지의 "spike" 없음(주석 123, 754뿐). 테스트 `advisories.rs::unsupported_forms_name_what_is_allowed_without_mentioning_spike` 통과(실행(이번)). crate 이름 `spike-*`는 1.1로 미룬 상태(범위 밖) |
| P1-5 | SDK 패키지가 배포 불가 형태 | **부분 해결**, 게시는 금지 | `release.yml`에 `build-sdk-ts`, `build-sdk-py`, `assemble`, `publish-*` 구조가 있고 PR·dispatch는 dry-run(`release.yml` 상단 주석). 이번 안정화는 게시 금지라 실행하지 않음 |
| P1-6 | 릴리스 위생 부재 | **부분 해결** | `CHANGELOG.md`, `SECURITY.md`, `CONTRIBUTING.md` 있음. 태그 0개(`git ls-remote --tags origin` 출력 없음). main 보호 없음(`gh api .../branches/main/protection` → 404). 보호는 저장소 설정이라 환경/소유자 몫 → F-07 |
| P1-7 | README 상태 문구 | **해결됨** | `README.md:3` 1.0 상태 박스(2026-11-21) |
| P1-8 | 구버전 호환 정책 없음 | **해결됨**(문서) | `COMPATIBILITY.md` "1.0 호환 약속과 경계". 동작은 `WIRE.CONTRACT_MISMATCH`, `product/OPERATIONS.md:99` |
| P1-9 | PG17 한정이 지원처럼 읽힘 | **해결됨**(문서) | `product/SUPPORT.md` "PostgreSQL 17 한정" |
| P1-10 | Linux 격리 worker·macOS 격리의 현재 코드 CI 근거 없음 | **부분 해결** | `worker-jobs-linux`가 RC에 포함, `macos-platform-tests.yml` 신설(596ab5dd에서 success). 단 macOS 전용 테스트 5곳이 그 job 밖 → B-05 |
| P1-11 | 업그레이드 baseline이 릴리스가 아님 | **결정 필요**(릴리스 후) | `aip-release-gate.yml:122 --baseline 4c27040c…`는 의도된 고정이다. 1.0 태그 후 갱신하는 항목이라 지금은 건드리지 않는다 |
| P1-12 | 제품 binary에 PoC 명령 노출 | **해결됨** | 실행(이번) `aip --help`에 `[1.0]`, `[PoC]` 구분 표시 |
| P1-13 | 의존성 점검을 출시 SHA에서 재실행 | **부분 해결** | `dependencies.yml`이 RC에 포함, 월요일 schedule 있음. HEAD push에서는 경로 필터로 미실행 → F-03 |
| P1-14 | 헤더 오류 코드가 문서와 다름 | **해결됨**(문서). 쓰기 확장 `/apply` 헤더 없음은 **근거 부족** | `product/SUPPORT.md` "계약 헤더 오류 코드"(경로별 표). 쓰기 확장 `/apply`는 "코드 확인만" 표기 그대로 → D-06 |

### 2.2 감사 §6 공개 count, §7 표현력 질문

| 항목 | 현재 분류 | 근거 |
|---|---|---|
| count가 `rows read`를 상속하지 않음, sum/min/max는 상속 | **결정 필요**(계약으로 유지 중) | `spikes/spike-v2-read/src/plan.rs:119 measure`: count(`:124-126` 근처)는 `count(*)`, sum/min/max는 `FILTER (WHERE visible)`. `product/SUPPORT.md` "공개 count", `product/SDK.md` Public count contract. 옵션 O1~O5는 `plan-docs/expressiveness-1.0/decisions.md` "공개 count" 표 |
| `callerFilter read` count 오라클 | **해결됨** | P0-2 행 |
| where 없는 count 경고 | **해결됨** | `sema.rs:2379-2394` |
| `where ... input` + `callerFilter none` count가 숨은 행 수를 노출(F1) | **결정 필요** | `plan-docs/hardening-1.0/policy.md` §2 F1(선택지 a/b/c, 1.0에는 c 권고). 현재 경고 조건이 `a.where_.is_none()`뿐이라(`sema.rs:2379`) F1은 경고도 없다 → A-01 |
| Q1 `avg`, 관련 필드 정렬·필터, 다중 `groupKey`, nullable cursor | **범위 밖**(1.1 후보, SUPPORT 4번에 명시) | `decisions.md` P1 후보 표 |
| Q1 Decimal `sum` | **해결됨** | 커밋 `7e367ead` HTTP·SDK Decimal sum 진단 시험, `79f2d3e8` P1 정책 경고·Decimal sum |
| Q2 관련 행 증감 `update Member where id = author { score = score + 1 }` | **해결됨** | 실행(이번) `aip service check` → `ok:true`. 커밋 `acaed2ab` P0-B, 테스트 `spikes/spike-v3-write/tests/expr_arith.rs`, `spikes/spike-v1-fixture/tests/expr_arith.rs` |
| Q2 다른 필드 값으로 증감(`views + bonus`) | **범위 밖**(한 항, `this.g`·input·리터럴만) | `decisions.md` P0-B "기각" |
| Q3 tenant | **범위 밖**(자동 격리). 누락 조언은 부분 해결 → A-03 | `product/SUPPORT.md` 13번. `spikes/spike-v1-fixture/src/policy_advice.rs` 3종 상시 경고 |
| Q4 `to` 쉼표와 효과 `;`의 구분자 불일치 | **지원 범위 확정, 진단 개선됨** | 구분자는 `product/LIMITS.md` "구분자 규칙"에 문서화. 진단은 위 P1-2 행 |
| Q5 문법 추가가 기존 `.aip`를 깨지 않는다는 자동 보장(문법 버전, 호환 골든) | **근거 부족** | `product/SUPPORT.md` 마지막 절이 "확인하지 못했다"고 적음. 현재도 문법 버전 필드를 찾지 못함 → E-07 |

### 2.3 hardening `policy.md`

| ID | 내용 | 현재 분류 | 근거 |
|---|---|---|---|
| D1, D2 | `from`·`to` 술어 호출로 숨은 필드 읽기·복사 | **해결됨** | 커밋 `30bc9a3d`. 테스트 `spikes/spike-v1-fixture/tests/policy_hardening.rs::from_and_to_values_cannot_read_a_hidden_field_through_a_predicate`. 실행(이번) `policy_hardening` 10 passed |
| D3 | `actor.*` 경로와 `input` 이분 탐색 | **해결됨** | `20e44d6a`. `the_actor_root_is_checked_like_this_for_hidden_fields` |
| D4 | 효과 값·대상 조건 | **해결됨** | `6d2db35f`. `effects_cannot_copy_or_match_a_hidden_field` |
| D5 | compose `create ... from` 복사 | **해결됨** | `45b9b3bc`. `compose_create_sources_cannot_copy_a_hidden_field` |
| D6 | `sameScope` | **해결됨** | `1e5f6d03`. `same_scope_cannot_be_a_hidden_field` |
| D7, D8 | `expose create allow` | **해결됨** | `02a14267`. `create_allow_cannot_compare_caller_values_with_a_hidden_field` |
| D9 | `check` | **해결됨** | `2ed641d7`. `a_check_cannot_compare_a_hidden_field_with_a_caller_written_field` |
| D10 | 집계 `where`의 `input` | **해결됨** | `c0c6c349`. `aggregate_where_cannot_compare_input_with_a_hidden_field` |
| D11, D12 | 집계 guard, operation allow | **해결됨** | `da8c6afb`. `aggregate_access_guard_cannot_compare_input_with_a_hidden_field`, `operation_allow_cannot_compare_input_with_a_hidden_field` |
| (선행) | 술어 호출이 `input`을 받는 경로 | **해결됨** | `b59457b7` |
| D13 | 외부 저장소 create가 권한 판정 전에 유일 제약 오류를 냄 | **해결됨** | `0e5344ee`. 회귀 `product/tests/external-create-order.test.mjs`, 실행(이번) `ℹ pass 1`. Verify 연결 `7d2418da`, `tools/verify.sh` node 목록 |
| F1 | where+`input` count | **결정 필요** | 2.2 |
| R1 | 읽기 확장 `ctx.data.read`가 `access read.<Resource>` 선언 없이 `expose read`가 있는 어느 resource든 읽음 | **결정 필요**(결함 아님, 우회 못 찾음) | 코드 확인: `sema.rs:2794 extension`의 읽기 확장 access 루프(`:2869-2893`)는 `mongo`/`redis`와 `Resource.aggregate`만 받는다. 런타임 `spikes/spike-v4-worker/src/lib.rs:896` `"read" =>`는 계약 access를 보지 않는다(aggregate만 `:883`에서 `ACCESS_NOT_DECLARED`). 선택지 (a) opt-in, (b) 필수, (c) 경고만은 `policy.md` §2. 선택지 (b)와 같은 시도 `900f8325`는 FaceReview가 깨져 퇴보 판정(`old-branches.md`). 정적 경고는 별도 → E-02 |
| K1 | `unique`에 숨은 필드: 값 존재 확인 | **결정 필요** | `sema.rs:1913` unique는 필드 존재만 검사한다. 선택지 `policy.md` §2 → A-04 |
| K2 | 숨은 범위 필드의 리터럴 증감 | **결정 필요** | 현재 허용. (a) 거부 (b) 허용·문서화 → A-04 |
| K3 | 숨은 자식 행 때문에 부모 삭제가 `REFERENCE_CONFLICT` | **결정 필요** | `spikes/spike-v3-write/src/lib.rs:862 23503 → WIRE.REFERENCE_CONFLICT` → A-04 |
| K4 | 외부 저장소 create `allow`가 생략한 `default database` 필드를 읽음 | **결정 필요** | D13 수정 뒤에도 남음. 선택지 (a) 정의 오류 거부(1.0 권고). 현재 sema에 해당 검사 없음(`default database` 처리는 `sema.rs:293-300`뿐) → A-04 |
| C1~C8 | 다른 테넌트 id=없는 id 응답, bundle refs, 확장 ctx.data, internal 전이, 커서·정렬·필터, 멱등 key, 읽기 확장 read/aggregate | **해결됨**(정상 확인) | `plan-docs/hardening-1.0/policy-probes/policy-probes.mjs C1~C8`. 이번에 재실행하지 않았다(`미실행`) |
| C9 | 소유 스키마 create는 `allow`가 INSERT `WHERE`에 있어 권한 없으면 INSERT 미실행 | **해결됨**(구조) | `policy.md` §3 C9. 이번에 재실행하지 않음 |
| S1, S2 | 필터·정렬·집계 정의 단계 거부, 오류 본문에 행 id·값 없음 | **해결됨** | `DEF.POLICY_FIELD_NOT_FILTERABLE`, `DEF.POLICY_FIELD_NOT_AGGREGATABLE`(`sema.rs:2246`) |
| A2 요청: 권한 판정 전에 DB 제약 오류가 나오는 다른 경로 | **근거 부족** | 공개 create(소유 스키마)와 외부 create는 확인됨. **전이(`apply`) 경로**에서 권한 없는 호출자의 unique·check 충돌 순서는 시험 기록이 없다. 확장 `ctx.data.create`를 외부 저장소 위에서 돌린 실행도 없다(`policy.md` §4 "미실행") → A-05 |
| A3 | 테넌트 조건 누락 조언 | **부분 해결** | 상시 경고 3종(`DEF.ADVISORY_TENANT_UNSCOPED_READ`, `DEF.ADVISORY_WRITE_WIDER_THAN_READ`, `DEF.ADVISORY_PUBLIC_READ_TENANT_FIELD`): `spikes/spike-v1-fixture/src/policy_advice.rs`, `79f2d3e8`. 구조상 미탐: `exists`가 있는 정책은 판정 안 함(`policy_advice.rs` 머리 주석), relation·aggregate·extension을 통한 간접 접근의 커버리지는 확인 못함 → A-03 |

### 2.4 hardening `concurrency.md`와 LIMITS E1~E9

`concurrency.md`의 시험 ID(`a1~a5`, `i1~i5`, `e1~e5`, `n1`, `f1`)는 `spikes/spike-v6-transport/tests/concurrency_probe_http.rs`의 `#[ignore]` 탐색 시험이다(`--ignored --exact`로 하나씩 실행). `product/LIMITS.md`의 E1~E9는 표현력 한계 번호이고 별개다.

| ID | 내용 | 현재 분류 | 근거 |
|---|---|---|---|
| a2, a3 | 줄 생성 → compose·`target.where` 전이 교착 | **해결됨** | `78894ef4`. 회귀 `spikes/spike-v3-write/tests/bundle_lock_order_compose_where.rs`. 실행(이번) 1 passed |
| a4 | 줄 생성 → `target.ids` 전이(대조군) | **해결됨** | `dc68e4e6`, `824cc0e1`. `spikes/spike-v3-write/tests/expr_bundle_lock_order.rs` |
| **a1 = LIMITS E9** | 서로 다른 순서로 같은 상품들을 건드리는 bundle 교착 | **결정 필요**, 현재 동작은 **재현됨** | 실행(이번) `PROBE_N=2 PROBE_ROUNDS=3 … a1_opposite_order_two_product_bundles`: `round=0 elapsed=17ms`, `round=1 elapsed=1179ms`, `round=2 elapsed=1180ms`, 매번 `tally={"OK": 2}`. 교착 1회 + 재시도로 둘 다 성공하되 1초 이상 든다. 3건 이상은 `concurrency.md` 3.1표(기한 초과가 흔함, 이번에 재측정 안 함). 선택지 A~D와 권고(B 검토, A 먼저)는 `concurrency.md` §5. 잠금 코드는 `spikes/spike-v3-write/src/lib.rs:2011 bundle_reference_locks`, `:2284 compose_exec`, `:2392 apply_in` → B-01 |
| a5, f1 | 실패 시 부분 커밋 없음, 동시 12건 혼합 | **해결됨**(통과) | `concurrency.md` 3.2표. 이번에 재실행하지 않음 |
| i1~i4 | 같은 key 동시 재전송, 지문 불일치, 응답 유실 후 재생, 배포 fence | **해결됨**(통과) | `concurrency.md` 3.4. 영구 시험: `spikes/spike-v6-transport/tests/audit_bd_idem_race.rs`(`06cc9c66`), `commit_unknown_wire`, `write_extension_replay_backpressure` |
| i5 | SERIALIZABLE `/apply`에서 `WIRE.INTERNAL` 없음 | **해결됨**(통과) | `concurrency.md` 3.4, 30라운드 |
| e1~e4 | 확장 동시 중복, 경합, unique create 경합, SERIALIZABLE | **해결됨** | e4 결함은 `cff81a17`로 수정. 회귀 `spikes/spike-v6-transport/tests/write_extension_serializable_conflict.rs` |
| e5 | `WORKER_BUSY` 포화에서 완료 key 재생·`/status` | **해결됨** | `84d4e318`. 회귀 `db_connect_lifetime`, `db_query_lifetime`(Linux 게이트 해제 `cb37868b`) |
| n1 | 상·하한 근처 동시 증감 | **해결됨**(통과) | `concurrency.md` 3.3 |
| 4.3 | 확장 요청이 slot보다 DB 연결을 먼저 잡음 | **해결됨** | `84d4e318`. `SATURATED_REPLAY_BUDGET = 500ms`(`spikes/spike-v6-transport/src/lib.rs:136`)는 이 작업이 정한 값이라 소유자 조정 대상 → B-07 |
| 8.4 | `/apply` 멱등 조회 `if let Ok(Some(row))`가 오류를 버림 | **해결됨** | `cb37868b`. `spikes/spike-v6-transport/src/lib.rs:405-413`이 `match`로 분류한다(연결 끊김은 `lost_before_commit`, 그 밖은 `classify_write_error`). 회귀 `spikes/spike-v6-transport/tests/apply_idem_lookup_error.rs`, 실행(이번) 1 passed |
| **durable `if let Ok(Some(row))`** | 같은 모양이 durable job 취소 확인에 남음 | **미해결 결함**(코드 근거, 미재현) | `spikes/spike-v6-transport/src/durable.rs:1201`. `FOR UPDATE` 조회가 오류(`lock_timeout=2s`, 40001, 40P01, 연결 끊김)를 내면 `if let`이 실패한 분기를 건너뛰고 일반 완료 분기(`:1236` 이후 `guard`, `store.fail/retry_after`)로 간다. 트랜잭션은 이미 중단 상태이므로 후속 문장이 실패하고 `:1273`에서 `job_outcome_rejected`를 stderr로만 남기고 커밋이 실패한다 → 취소 확인이 유실되고 job은 lease 만료까지 `CANCEL_REQUESTED`로 남는다고 **추정**한다. 중복 실행·부분 커밋은 없다고 읽힌다. 같은 함수 `:1191-1192`의 `let Ok(..) else { return }`와 `:1232` `let _ = tx.commit()`(취소 확인 커밋 결과 버림)도 같은 계열(결과 미보고) → B-02 |
| `/status` 조회 오류 | `Ok(Err(_)) | Err(_)` → `WIRE.DB_UNAVAILABLE` | **근거 부족** | `spikes/spike-v6-transport/src/lib.rs:556`. 비연결 오류도 재시도 가능으로 보이는 코드다. 의도인지 확인 못함 → B-03 |
| LIMITS E1 | 부모·자식 원자 생성 | **해결됨** | bundle `refs`. 테스트 `spikes/spike-v3-write/tests/expr_refs.rs`, `spikes/spike-v6-transport/tests/expr_refs_http.rs`, 실행(이번) expr-apps `PASS 64` |
| LIMITS E2 | 가변량 증감 | **해결됨** | `expr_arith.rs`(v1, v3), 같은 실행 |
| LIMITS E3 | 계산 집계 | **범위 밖**(확장 우회, 1.1 후보) | `product/LIMITS.md` E3 |
| LIMITS E4, E5, E6 | 참조 읽기 가능 조건, cursor+nullable, `from` 필수 | **범위 밖**(문서화된 한계와 우회) | `product/LIMITS.md` |
| LIMITS E7 | PG 쓰기 확장 원자 생성 | **해결됨** | 커밋 `27f87d10`, `3a6edf5f`. 테스트 `spikes/spike-v4-worker/tests/expr_ext_create.rs` |
| LIMITS E8 | 확장 입력 배열 없음 | **범위 밖**(`Json` 우회) | `product/LIMITS.md` E8 |

### 2.5 hardening `contract.md`(C1~C11)와 `diagnostics-contract.md`(G1~G10)

| ID | 현재 분류 | 근거 |
|---|---|---|
| C1 TS `WriteUnsettled`·`ScopeConflict`의 `code` | **해결됨** | `410a7828`, `4ac4f1b4` |
| C2~C7 문서 불일치 | **해결됨** | `8a6a19fe`, `a6616fcd`, `8c2f08b0`, `2c859502` |
| C8 진단 JSON 키(`scope`·`help`가 항상 있다는 서술, `help`는 같은 코드 반복 시 생략) | **결정 필요** | G3, G7로 이어짐 → E-01 |
| C9 앱 패키저 check 출력 파싱 실패 시 fail-open | **해결됨** | `58cf329e`. 회귀 `product/tests/app-package.test.mjs` |
| C10 UTF-8 BOM 형식 간 불일치 | **해결됨** | `bf72da53`. `spikes/spike-v1-fixture/tests/bom.rs`. 실행(이번) BOM 붙은 `.aip`가 `ok:true` |
| C11 `aip-cli` 시험 빌드가 DB 환경변수 없이는 컴파일 불가 | **해결됨**(개발 루프 한정, Track E: `AIP_TEST_NO_DB=1`, 시험 DSN이 없을 때만 가드를 건너뛰고 DB 시험은 여전히 컴파일되지 않음). | `crates/aip-test-support/build.rs:13`. `PGHOST`·`PGPORT` 같은 `PG*`가 있어도 중단한다(이 작업 셸에도 설정돼 있었다, 5절). 개발 루프 비용이라 E-08 |
| G1 위치 없는 진단이 `line:0, column:0` | **부분 해결 / 결정 필요**(Track E: 0:0은 유지하고 `located:false`를 추가해 표현. 1:1 변경과 좌표 채우기는 `decisions/e.md` D-E3) | 실행(이번) 빈 `.aip` → `"column":0,"line":0`. `Span::default()` 사용처: `sema.rs:182,283`, `extract.rs:69,120-121`, `hlit.rs:217-218`, `limits.rs:12` |
| G2 경고에 `line`·`column` 없음(`anchor` 문자열만) | **해결됨**(Track E: 컴파일러 경고에 `line`·`column`·`explain` 추가. `--dev-checks`의 선언 단위 경고는 위치 정보가 없어 그대로). | `spikes/spike-v11-dev-checks/src/lib.rs`(advisory 변환), 실행(이번) 경고 키는 테스트가 `anchor, code, message, severity`로 고정(`crates/aip-cli/tests/service.rs:33`) |
| G3 정확한 키 집합 비교가 가산 변경을 막음 | **해결됨**(Track E: `service.rs`의 `contract` 최소 검사로 교체, 알 수 없는 필드 허용 시험 포함). | `crates/aip-cli/tests/service.rs:28,33,49,55` |
| G4 스키마 버전 표지 없음 | **결정 필요**(1.0 전에 넣을지, `decisions/e.md` D-E2) | `diagnostics-contract.md` §8 |
| G5 정의 오류가 아닌 실패 코드 62종이 영역 접두 없음 | **결정 필요**(`decisions/e.md` D-E5. Track E는 파일 읽기 실패에 `reason`만 추가) | `diagnostics-contract.md` §7 G5 |
| G6 레지스트리에 `severity`·`status`·`since` 없음 | **부분 해결**(Track E: `severity`·`status`와 선택 `since`·`replacedBy` 추가, 게시된 코드 삭제 방지 시험. `since` 값은 이력이 없어 어느 코드에도 채우지 않음). | `spikes/spike-v1-fixture/src/codes.rs`, 실행(이번) 없음 확인(`retired|since|replacedBy` 0건) |
| G7 `COMPATIBILITY.md` 진단 JSON 표가 선택 필드를 필수처럼 서술 | **미해결**(문서, 소유자 승인 필요) | `diagnostics-contract.md` §7 |
| G8 `DEF.SOURCE_TOO_LARGE`가 파일 경로로는 도달 불가로 보임 | **근거 부족**(추론) | `diagnostics-contract.md` §7 |
| G9 저장소 소비자가 `message` 부분 문자열로 분기 | **미해결 결함**(Track E: `sema.rs` 대기 + 소유자 결정 `decisions/e.md` D-E4, 적용 가능한 패치 `decisions/e-g9-ready.patch`) | `.github/workflows/flaskbb-post-order.yml:213`(`"expose apply" in d["message"]`). 전용 코드 추가 후 코드 비교로 전환 |
| G10 `BrokenPipe`를 종료 0으로 처리 | **결정 필요**(낮음, `decisions/e.md` D-E6) | `crates/aip-service/src/lib.rs:1787 Err(e) if e.kind() == BrokenPipe => ExitCode::SUCCESS` |

### 2.6 hardening `crypto-rekey.md`, `old-branches.md`

| 항목 | 현재 분류 | 근거 |
|---|---|---|
| rekey 스키마 그림자 | **해결됨** | `a149ee32` |
| rekey ref 범위·CAS의 ref 열 부재 | **결정 필요**(공개 API 변경, 권고 A) + **범위 밖**(암호화 필드는 1.0 지원 밖) | `crates/aip-execution/tests/encryption_rekey.rs:530 #[ignore]`, `crates/aip-execution/src/rekey.rs:343`. 어떤 workflow도 이 ignored 시험을 돌리지 않는다 → E-05 |
| 오래된 브랜치 66개 정리 | **해결됨**(반영 1, 퇴보 1, 보류 1, 나머지 이미 포함) | `old-branches.md`. 반영분 `audit_bd_idem_race.rs`(`06cc9c66`) |

### 2.7 소비자 E2E와 CI (Track C, F1이 직접 쓰는 항목)

| 항목 | 현재 분류 | 근거 |
|---|---|---|
| 독립 소비자(taskshare, stockdesk) 검증이 과거 Core에 고정 | **미해결 결함** | `.github/workflows/independent-consumer-rc.yml:20 ref: 4e1325644b267f2b3177a327258c0882ee5711c2`, `:63 name: independent-consumers-4e132564`. `verification/1.0-consumers/run.py`가 `assert revision == "4e1325644b…"`(`git rev-parse HEAD` 단언)와 `aip-sdk-1.0.0.tgz`, `application_intent_protocol_sdk-1.0.0-*.whl`, `version == "aip 1.0.0"`를 하드코딩. 이 SHA는 현재 main의 조상(`git merge-base --is-ancestor` 참)이고 HEAD보다 104커밋 뒤라(`git rev-list --count 4e132564..origin/main`), 그 사이의 hardening 수정을 하나도 검증하지 못한다 → C-01 |
| 그 workflow의 실행 조건 | **미해결 결함** | 트리거는 `push: branches: [verify/1.0-independent-consumer-20261121]`와 `workflow_dispatch`뿐. `release-candidate.yml`의 `needs`에 없다 → F-01 |
| 소비자 실행 결과와 대상 SHA의 일치 기록 | **미해결 결함** | `run.py`는 SHA를 하드 고정 단언할 뿐 결과 파일에 SHA를 적지 않는다. 증거 로그 이름도 고정 SHA → C-02 |
| 업그레이드 시험의 baseline 고정(`4c27040c…`) | **해결됨**(의도) | 업그레이드 시험의 의도적 baseline. 최신 검증과 섞이지 않는다 |
| `release-candidate`가 동일 SHA에서 필요한 검증을 모두 하는가 | **부분 해결** | 맞는 점: 모든 job이 `workflow_call`로 호출자 SHA에서 돌고, `required`가 `skipped`·`cancelled`를 실패로 본다(`release-candidate.yml` "A skipped or cancelled workflow is not a pass"), `aip-release-gate`는 `--ref "$GITHUB_SHA"`. 틈: (1) 독립 소비자 미포함, (2) HEAD에서 RC 실행 이력 없음, (3) `release.yml`의 태그 게시 조건은 같은 SHA의 `aip-release-gate` 성공만 보고 RC `required` 성공은 요구하지 않음(`release.yml` verify job), (4) main 브랜치 보호·필수 체크 없음 → F-01~F-04, F-07 |
| main push에서 경로 필터로 안 도는 workflow | **미해결 결함**(구조) | `596ab5dd`에서 실행된 8개 외 미실행: `database-contracts`, `deploy-checks`, `dependencies`, `flaskbb-parity`, `flaskbb-post-order`(경로 필터), `sdk-semantic-e2e`, `semantic-conformance`(PR 전용, push 트리거 없음) → F-03 |
| 같은 SHA에서 `tools/verify.sh` 이중 실행 | **근거 부족**(중복이나 의도 가능) | `verify.yml`이 `bash tools/verify.sh`를 돌리고, `aip-release-gate.yml`의 `release-gate.sh`가 `full-regression`으로 다시 돌린다(`scripts/release-gate.sh:94`). RC는 둘 다 부른다. 감사 기록상 `full-regression`은 약 1223초 → F-04 |
| 단일 재현 진입점 | **미해결** | 최신 HEAD 필수 검증을 한 명령으로 재현하는 스크립트 없음(`tools/verify.sh`는 PG·mypy·tsc 필요한 Core 일부) → F-02 |

### 2.8 표현력 P0 설계(`plan-docs/expressiveness-1.0/decisions.md`)

| 항목 | 현재 분류 | 근거 |
|---|---|---|
| P0-A bundle `refs`·`target.created` | **해결됨** | `583c4e07`, `expr_refs.rs`, 실행(이번) expr-apps `합계: PASS 64 / FAIL 0 / SKIP 0` |
| P0-B 제한 산술 | **해결됨** | `acaed2ab`, `e76efbab`, `16819e3c`, `expr_arith.rs` |
| P0-C 확장 `ctx.data.create` | **해결됨** | `27f87d10`, `expr_ext_create.rs` |
| P0-D bundle 안 compose | **해결됨** | `583c4e07`, `expr_compose_bundle.rs`, `spikes/spike-v6-transport/tests/expr_compose_bundle_http.rs` |
| worker ctx 타입(d.ts, Protocol) | **해결됨** | `3a6edf5f` |
| 정책 작성 경고 3종 | **해결됨** | `79f2d3e8` |
| 주문·재고, 다중 사용자 소비자 시나리오 | **부분 해결** | `product/tests/fixtures/expr-apps/{orders,saas,community}`가 Verify의 "Run expressiveness consumer apps on PostgreSQL" step에서 매 push 실행(`verify.yml:199-206`). 항목별(재고 동시 차감, 외부 결제 대역, 여러 조직·공유 권한) 커버리지를 표로 대조한 문서는 없다 → C-03 |

### 2.9 재작업 금지 목록(이미 해결됨, 한 줄 요약)

숨은 필드 D1~D13(`30bc9a3d`, `20e44d6a`, `6d2db35f`, `45b9b3bc`, `1e5f6d03`, `02a14267`, `2ed641d7`, `c0c6c349`, `da8c6afb`, `b59457b7`, `0e5344ee`) · 공개 count 오라클·경고(`9db9905c`) · `/apply` 멱등 조회(`cb37868b`) · SERIALIZABLE 확장 `INTERNAL` 누출(`cff81a17`) · 줄 생성 뒤 교착(`78894ef4`, `dc68e4e6`, `824cc0e1`) · worker slot 선점(`84d4e318`) · 완료 재생 fence(`7cdfed93`) · 표현력 P0-A~D(`acaed2ab`…`583c4e07`, 병합 `be94d064`) · Decimal sum(`7e367ead`, `79f2d3e8`) · BOM(`bf72da53`) · 앱 패키저 fail-open(`58cf329e`) · rekey 스키마 그림자(`a149ee32`) · macOS 전용 시험 중 3곳의 Linux 게이트 해제와 macOS job 신설(`cb37868b`, 단 B-05 참조) · `external-create-order`, `upgrade-drill` 단위 시험의 Verify 연결(`7d2418da`, `596ab5dd`) · v6 probe clippy(`596ab5dd`) · 1.0.0 버전·`release-candidate.yml`.

## 3. 미해결 작업 목록 (Track별)

"위치"는 추정 수정 위치다. `결정`은 소유자 결정이 먼저 필요하다는 뜻이고, 결정 전에도 할 수 있는 작업을 `선행 가능`에 적었다.

### Track A: 정책·권한·테넌트

| ID | 항목 | 분류 | 추정 수정 위치 | 선행 가능한 작업 / 비고 |
|---|---|---|---|---|
| A-01 | F1: `where`에 `input`이 있는 count가 숨은 행 수를 노출(`callerFilter none`) | 결정 | `sema.rs:2379` 경고 조건(`a.where_.is_none()`)을 `input` 포함으로 확장, `tests/advisories.rs` | 응답 값이 안 바뀌는 선택지 (c) 경고 확장은 호환 영향이 없다. (a) 거부는 `recruitment.aip` `approvedCount` 같은 기존 정의를 깬다(`policy.md` F1) |
| A-02 | 공개 count 기본 의미(O1~O5) | 결정 | 의미를 바꾸면 `plan.rs:119 measure`, `sema.rs:2197 aggregate`, `syntax_card.rs`, SDK 생성 계약 | 이번 안정화에서 임의 변경 금지. PG17 반례 매트릭스(sourceAccess 유무, 다른 테넌트 포함, 같은 정의의 count/sum 범위 차이)를 `crates/aip-cli/tests/service_flow.rs`(기존 `service_policy_aggregate_noninterference_http_postgres`)에 더하면 결정 근거가 된다 |
| A-03 | 테넌트 조언의 미탐 정리 | 근거 부족 | `spikes/spike-v1-fixture/src/policy_advice.rs`, `spikes/spike-v11-dev-checks/src/lib.rs` | `exists` 정책, relation·aggregate·extension 경유 접근을 반례 정의로 `check`해 오탐·미탐 표를 만든다. 자동 tenant 격리는 범위 밖 |
| A-04 | K1~K4(`unique`·범위 증감·FK 거부·외부 create `allow`의 default 필드) | 결정 | K1·K4: `sema.rs:1913`, `:293-300`. K3: `spikes/spike-v3-write/src/lib.rs:862`. K4 런타임: `create_row`(`:1236`) | 권한 있는 호출자의 진단을 훼손하는 "코드 통일"은 금지. 정의 단계 제한(K1 a, K4 a)이 응답 코드를 안 바꾼다. 저장소 정의 68개와 FaceReview·uuid 입양 정의 영향 확인 필요 |
| A-05 | 권한 판정 전 DB 제약 오류가 나오는 다른 경로 | 근거 부족 | `spikes/spike-v3-write/src/lib.rs` `apply`(`:287`)의 `judge`와 write 순서 | **전이 경로**에서 권한 없는 호출자가 unique·check 충돌 값을 보내는 반례를 PG17에서 먼저 만든다(`plan-docs/hardening-1.0/policy-probes/h.mjs` 재사용). 확장 `ctx.data.create` on 외부 저장소도 미실행 |

### Track B: 동시성·트랜잭션·멱등

| ID | 항목 | 분류 | 추정 수정 위치 | 선행 가능한 작업 / 비고 |
|---|---|---|---|---|
| B-01 | 반대 순서 bundle 잠금 교착(E9 남은 부분) | 결정(재현됨) | `spikes/spike-v3-write/src/lib.rs:2011 bundle_reference_locks` 계획 단계, 선택 B면 `(resource, id)` 선잠금 | 선택지 A(문서)는 즉시 가능(`product/LIMITS.md` E9에 측정값). `a1`이 그대로 회귀 시험이다. 성공률, 교착 수, p50/p95/p99, deadline 초과율을 재는 하네스는 `concurrency_probe_http.rs`의 `PROBE` 출력에 있고 p95/p99는 없다 |
| B-02 | `durable.rs:1201` 취소 확인 쿼리 오류 무시 + `:1191-1192` `let Ok else return` + `:1232` `let _ = commit` | 미해결 결함(코드 근거) | `spikes/spike-v6-transport/src/durable.rs:1189-1275` | 오류 주입(예: `lock_timeout`, 40001/40P01, 연결 끊김)으로 `CANCEL_REQUESTED` job의 최종 상태를 확인하는 RED 시험부터. `apply_idem_lookup_error.rs`의 뷰 기반 주입 기법을 재사용할 수 있다 |
| B-03 | `/status` 비연결 오류 → `DB_UNAVAILABLE` | 근거 부족 | `spikes/spike-v6-transport/src/lib.rs:556` | 의도 확인 후 문서화 또는 분류 |
| B-04 | 확장 `ctx.data.read/apply/create`, bundle·compose, COMMIT_UNKNOWN 경로의 오류 무시·변환 전수 | 근거 부족 | `spikes/spike-v6-transport/src/write_extension.rs`, `spikes/spike-v4-worker/src/write.rs`, `spikes/spike-v3-write/src/lib.rs` | 이번 grep(`if let Ok`, `.await.ok()`, `let _ = …execute/query`)은 `durable.rs:1201` 외 쿼리 오류 무시를 찾지 못했다. 그러나 `match`로 오류를 다른 코드로 바꾸는 경로는 grep으로 못 본다 |
| B-05 | macOS 전용 테스트가 어떤 workflow에서도 실행되지 않음 | 미해결(CI 공백) | 파일은 6절. Linux에서 가능하면 `#![cfg(target_os = "macos")]` 제거(`Isolation::None`), 불가능하면 `.github/workflows/macos-platform-tests.yml` 추가 | `macos-platform-tests.yml`은 3개 명령만 돈다(`isolation::tests`, `v4_1`, `service_flow` 한 시험). `cb37868b` 이전 목록(`concurrency.md` 8.1)에는 이 5곳이 없다 |
| B-06 | `concurrency_probe_http.rs` 18개가 CI에 없음 | 미해결(CI 공백) | 안정 결과가 확인된 것(`f1`, `i1`~`i5`, `n1`, `e1`~`e5`)만 필수 경로 후보. `sqlgen::set_schema`가 프로세스 전역이라 시험마다 별도 프로세스 필요 | 비싼 탐색(`a1` 반복, `PROBE_ROUNDS` 큰 값)은 별도 경로로 분리 |
| B-07 | `SATURATED_REPLAY_BUDGET = 500ms` | 결정 | `spikes/spike-v6-transport/src/lib.rs:136` | 작업이 정한 값. DB가 500ms보다 느리면 포화 요청은 재생 대신 `WORKER_BUSY` |
| B-08 | 격리 모드 worker, 외부 저장소 위 확장, MySQL 경로에서 동시성 | 근거 부족 / MySQL은 범위 밖 | — | `concurrency.md` §6 "미검증" |

### Track C: 소비자 E2E·표현력

| ID | 항목 | 분류 | 추정 수정 위치 | 비고 |
|---|---|---|---|---|
| C-01 | 최신 HEAD로 두 소비자 앱을 돌린다(과거 SHA 고정 해제) | 미해결 결함 | `verification/1.0-consumers/run.py`(SHA 단언, SDK 파일명 하드코딩), `independent-consumer-rc.yml:20`(`ref`를 `github.sha`/입력으로) | F-01과 한 쌍. SDK 아티팩트 이름은 `node product/tools/version.mjs npm-tarball`로 파생. 업그레이드 baseline 고정은 그대로 둔다. `run.py` 단독 실행은 cwd가 Core 체크아웃이고 docker와 release 빌드가 필요하다(미실행) |
| C-02 | 소스 SHA·빌드 엔진 SHA·SDK 아티팩트 버전·출처·결과의 일치를 결과에 기록 | 미해결 | `run.py`가 JSON 요약을 쓰고 workflow가 artifact로 올린다 | C1 요구 |
| C-03 | 신규 시나리오(주문·결제 직전, 다중 사용자 업무)와 기존 `expr-apps`의 커버리지 대조 | 근거 부족 | `product/tests/fixtures/expr-apps/{orders,saas,community}`, `verification/1.0-consumers/apps/{taskshare,stockdesk}` | 항목별 표(재고 동시 차감, 원자 생성, 가격·수량 검증, 상태 전이, 여러 조직, 공유 권한, 집계·필드 제한)를 만들고 빠진 것만 추가. 외부 결제는 대역 |
| C-04 | 표현력 분류(선언만 / 복잡 / 확장 필요 / 원자성 깨짐 / 불가) | 부분 해결 | `plan-docs/expressiveness-1.0/audit.md`(21행, 기준 `4e132564`), `consumer-apps.md` | 감사는 `4e132564` 기준이라 P0-A~D 이후 등급을 갱신해야 한다 |

### Track D: 의미 정합·SDK

| ID | 항목 | 분류 | 추정 수정 위치 | 비고 |
|---|---|---|---|---|
| D-01 | 단독 집계 budget이 고정 2초 | 결정 | `spikes/spike-v2-read/src/plan.rs:316,1097` | 정의에서 선언 허용 또는 한도 문서화 |
| D-02 | `service check` 통과인데 런타임 미지원 / 실행 가능한데 정적 거부 | 근거 부족 | `sema.rs`, `plan.rs`, `spike-v3-write` | `plan-docs/reviews/2026-11-17-core-service-semantic-matrix.md`가 출발점(일반 read first/latest projection은 Service 미지원). 이번에 현재 HEAD로 재검증 안 함 |
| D-03 | 직접 apply, bundle 내부 apply, compose, 확장 apply의 의미 일치와 예외 목록 | 부분 해결 | `plan-docs/expressiveness-1.0/decisions.md` §0(C0-1~C0-9), 코드 `spikes/spike-v3-write/src/lib.rs`, `spikes/spike-v4-worker/src/write.rs` | 예외 계약(compose 단계는 patch/expect 불가, compose 안 compose 없음)을 한 표로 모은 문서가 없다 |
| D-04 | SDK(TS, Python): nullable·Decimal·Id·input 산술·refs·확장 결과·오류 재시도 표지·지문 불일치 | 근거 부족 | `product/tests/sdk-semantic-e2e.test.mjs`, `product/sdk`, `spikes/spike-v5-sdk`, `product/python/aip_sdk` | 이 테스트는 `tools/verify.sh`에 없고 `sdk-semantic-e2e.yml`(PR 경로 필터 + RC)에서만 돈다(F-03). TS 컴파일 타임 거부 가능한 잘못된 요청 검토는 미착수 |
| D-05 | 의미 기반 회귀(재고 정확히 한 번 감소, 숨은 필드가 다른 필드로 복사 안 됨 등) | 부분 해결 | 기존: `service_flow.rs`, `expr-apps`, `policy_hardening.rs` | 정의→실행 계획→DB 상태→SDK 응답을 한 시나리오로 잇는 시험은 `sdk-semantic-e2e`가 가장 가깝다 |
| D-06 | 쓰기 확장 `/apply`에서 계약 헤더 없음의 코드 | 근거 부족 | `spikes/spike-v6-transport/src/lib.rs:1515-1518`(REQUIRED/MISMATCH 분기), object·RAG는 `:1365,1496,1505` | `product/SUPPORT.md`가 "코드 확인만"으로 남겨 둠 |

### Track E: 진단·확장·구조

| ID | 항목 | 분류 | 추정 수정 위치 | 비고 |
|---|---|---|---|---|
| E-01 | 진단 JSON G1~G10와 소유자 결정 6개 | **부분 해결**(G1 표현, G2, G3, G6 일부 완료, 나머지 `decisions/e.md`, 명세 `diagnostics-fields.md`) | G1: `diag.rs`·`Span::default()` 사용처(2.5절), G2: `spike-v11-dev-checks/src/lib.rs`, G3: `crates/aip-cli/tests/service.rs:28-55`, G4·G6: `codes.rs`·출력, G9: `flaskbb-post-order.yml:213` + 전용 코드, G7: `COMPATIBILITY.md` | 가산 변경(G2, G3, G4, G6)은 결정 전에도 가능. 위치 없음 표현·`schema` 필드는 결정 필요 |
| E-02 | R1 읽기 확장 무선언 읽기 | 결정(`decisions/e.md` D-E7, 구현은 `sema.rs` 대기, 순서는 `e2-extension-access.md` 3.3) | `sema.rs:2869-2893`(읽기 확장 access 루프), `spikes/spike-v4-worker/src/lib.rs:896`(`"read"`), SDK·문서 | 권고 (a) opt-in. 확장 ABI 확정 금지(OI-X2) |
| E-03 | 쓰기 확장의 선언 vs 실제 `ctx.data.*` 접근 비교, 쓰이지 않는 선언 경고 | **비교 완료**(`e2-extension-access.md`, `e2/scan-result.md`), 미사용 grant 경고는 결정(D-E8). | `sema.rs:2794 extension`, `spikes/spike-v4-worker/src/write.rs` | 쓰기 확장은 `ACCESS_NOT_DECLARED`로 강제됨(`policy.md` C3). 미사용 grant 경고 여부는 확인 안 함 |
| E-04 | 구조: 책임 분리 후보 | **부분 해결**(분석과 `aip-service` 진단 출력 분리 완료, `track-e.md` 4절. 다른 Track 소유 파일은 후보로만) | 8절 파일 표 | 전체 재작성 금지. 실제 결함 수정 부위 중 경계가 분명한 곳부터. 후보는 4절 |
| E-05 | rekey ref 열 선택자 | 결정(범위 밖 기능) | `crates/aip-execution/src/rekey.rs:343`, `encryption_rekey.rs:530` | 암호화 필드는 1.0 지원 밖. 우선순위 낮음 |
| E-06 | `spike-*` crate 이름 | 범위 밖(1.1) | — | 사용자 메시지는 이미 정리됨 |
| E-07 | 문법 호환 보장 장치(문법 버전 필드, 호환 골든) | 근거 부족 | `spec/definition-syntax.md` 생성 골든 외 확인 못함 | 감사 §7 Q5 |
| E-08 | 시험 빌드의 DB 요구(C11) | **해결됨**(`AIP_TEST_NO_DB=1`) | `crates/aip-test-support/build.rs:13` | 진단·도움말 시험 루프가 PG 준비에 묶인다 |

### Track F1: CI

| ID | 항목 | 분류 | 추정 수정 위치 | 비고 |
|---|---|---|---|---|
| F-01 | 독립 소비자 workflow를 RC에 연결 + 최신 SHA 사용 | 미해결 결함 | `release-candidate.yml`(`jobs`와 `required.needs`), `independent-consumer-rc.yml`(`workflow_call` 추가, `ref` 제거) | C-01과 같이 |
| F-02 | 최신 HEAD 필수 검증 재현 단일 진입점 | 미해결 | 새 `scripts/` 스크립트(가칭), `tools/verify.sh` 재사용 | 릴리스 검증은 dry-run/수동 |
| F-03 | main push에서 경로 필터로 미실행되는 workflow | 미해결 결함(구조) | 각 workflow의 `on.push.paths`, `sdk-semantic-e2e.yml`·`semantic-conformance.yml`에 push 트리거 없음 | HEAD에서 7개 미실행(1절). PR 필수 체크화는 저장소 설정(F-07) |
| F-04 | RC 안의 `tools/verify.sh` 이중 실행 | 근거 부족 | `scripts/release-gate.sh:94`, `verify.yml` | 의도된 격리(깨끗한 clone)인지 확인 후 판단 |
| F-05 | 어떤 workflow에서도 안 도는 테스트 파일 | 미해결 | 6절 | 이름 검색 기준. 글롭 호출은 확인 못함 |
| F-06 | Verify 간헐 실패 원인 | 근거 부족 | `isolation.rs:1296`(probe), `verify.yml:295-311`(step 17) | `cb37868b` Run `44904438206`, 감사 P0-4 |
| F-07 | main 브랜치 보호·필수 체크 | 환경/소유자 | 저장소 설정 | `gh api` 404. 이 세션은 설정 변경 안 함 |
| F-08 | `flaskbb-post-order.yml`의 `continue-on-error` 14개가 최종 성공을 가리는지 | 근거 부족 | `flaskbb-post-order.yml:84-262`, 마지막 집계 step `:285-288` | 감사 §10.3은 "마지막에 비영 종료로 모은다"고 기록. 이번에 재검증 안 함 |

## 4. 파일 소유권과 충돌 순서

원칙: **A~E는 `.github/workflows/`와 `tools/verify.sh`를 수정하지 않는다.** 테스트 파일을 `spikes/*/tests/`나 `crates/*/tests/`에 두면 `tools/verify.sh`의 엔진 루프(`cargo test --manifest-path spikes/$engine/Cargo.toml --no-fail-fast`)와 `cargo test --workspace`가 자동으로 실행한다. 별도 연결이 필요한 시험(node·python·ignored·macOS)은 F1이 연결한다. 연결이 필요하면 PR 설명에 시험 이름과 명령을 적는다.

| 파일(줄 수 `wc -l`) | 주 소유 | 보조(좁은 영역만) | 충돌 시 순서 | 영역 분할 |
|---|---|---|---|---|
| `spikes/spike-v1-fixture/src/sema.rs` (3250) | **A** | E(진단 좌표 `:182,283`, 읽기 확장 `:2794-2900`), D | A → E → D. 한 번에 한 Track만 연다 | 숨은 필드 검사 `:580-1180`, 집계 `:2197-2420`(A). `extension` `:2794-2900`(E). 나머지 정합 수정(D) |
| `spikes/spike-v1-fixture/src/{codes,diag,enrich,extract,hlit,limits}.rs` | **E** | A(새 코드 추가) | `codes.rs`는 추가만 한다(A 먼저, E는 rebase). 골든 갱신 `AIP_UPDATE_GOLDEN=1 cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --test diagnostics`는 한 번에 한 PR | |
| `spikes/spike-v1-fixture/src/policy_advice.rs`, `syntax_card.rs` | A | E(문법 카드 문구) | A → E. `syntax_card.rs`를 바꾸면 `spec/definition-syntax.md` 골든 재생성 | |
| `spikes/spike-v11-dev-checks/src/lib.rs` | A | E(G2 앵커 좌표) | A → E | |
| `spikes/spike-v2-read/src/plan.rs` (1103) | A(count 의미가 결정되면) | D(단독 집계 budget) | 결정된 쪽만 연다. A → D | |
| `spikes/spike-v3-write/src/lib.rs` (2424) | **B** | A(`create_row :1236`, `:862`), D(의미 정합) | B → A → D. B-01은 `:2011-2400`(bundle·compose) 안에서만 | 잠금 `:2011`, `:2284`, `:2392`(B). 생성·분류 `:1236`, `:862`(A) |
| `spikes/spike-v6-transport/src/lib.rs` (2279) | **B** | D(헤더 `:1365-1518`) | B → D | 멱등·`/apply` `:337-475`, `/status` `:530-556`, slot `:1053,1616,1759`(B). 헤더 분기(D) |
| `spikes/spike-v6-transport/src/{durable,write_extension,slots,server}.rs` | **B** | — | — | |
| `crates/aip-service/src/lib.rs` (2790) | **E** | D(`gen`, `load :571`), A(`Check :1060` 출력 필드 확인만) | E → D | 진단 출력(E) |
| `spikes/spike-v4-worker/src/{lib.rs,write.rs}` | B(`write.rs`) | E(`lib.rs:896` 읽기 확장 R1) | B와 E가 영역이 다르다. 같은 파일이 아니면 병렬 | |
| `spikes/spike-v5-sdk/**`, `product/sdk/**`, `product/python/**` | **D** | C(소비자 사용 확인) | — | |
| `verification/1.0-consumers/**`, `product/tests/fixtures/expr-apps/**` | **C** | F(workflow 연결) | C가 `run.py`, F가 workflow. 한 쌍으로 같은 시점에 병합 | |
| `.github/workflows/**`, `tools/verify.sh`, `scripts/release-gate.sh`, `scripts/verify-*.sh` | **F1** | — | A~E의 연결 요청은 F1이 일괄 반영 | |
| `product/SUPPORT.md`, `product/LIMITS.md`, `COMPATIBILITY.md`, `CHANGELOG.md` | D(최종 정합) | 각 Track이 자기 절만 추가 | CHANGELOG `[Unreleased]`는 rebase 충돌이 잦으니 항목 추가만 한다 | |
| `plan-docs/hardening-1.0/*.md` | 각 Track이 자기 영역 문서 | — | 이 baseline의 분류가 바뀌면 이 문서 해당 행도 같은 PR에서 고친다 | |
| `crates/aip-test-support/build.rs`, `crates/aip-cli/tests/service.rs` | E | — | — | |

핵심 3파일(`sema.rs`, `spike-v3-write/src/lib.rs`, `spike-v6-transport/src/lib.rs`)은 동시 수정 금지다. 병렬이 필요하면 위 영역 분할을 따르고, 같은 파일을 건드리는 PR은 먼저 병합된 쪽에 rebase한다.

모듈화 후보(E-04, 실제 결함 수정과 겹치는 경계가 분명한 곳부터):

| 후보 | 현재 위치 | 경계 |
|---|---|---|
| 숨은 필드·정책 검사 | `sema.rs:580-1180` | `reject_hidden_input_compare`(`:938`), `access_hidden_reads`(`:1179`)가 자체 완결 |
| 집계 의미 검사 | `sema.rs:2197-2420`, `implies_policy :2930` | `aggregate`와 `implies_policy` |
| 확장 계약 | `sema.rs:2794-2900` | `extension` |
| bundle·compose 실행 | `spike-v3-write/src/lib.rs:2011-2400` | `bundle_reference_locks`, `compose_exec`, `apply_in` |
| 트랜잭션·멱등 | `spike-v6-transport/src/lib.rs:146-560` | `idempotency_ddl`, `handle_apply`, `apply_attempt`, `/status` |
| 이미 분리된 것 | `policy_advice.rs`, `spike-v11-dev-checks`, `write_extension.rs`, `slots.rs` | 모범 사례 |

## 5. 검증 명령과 실행 기록

### 5.1 PG17 기동 확인 (이 Mac, 2026-11-22)

이 세션 셸에는 `PGHOST=127.0.0.1`, `PGPORT=55031`이 이미 설정돼 있었다(다른 용도). `crates/aip-test-support/build.rs:13`이 `PG*`가 있으면 시험 빌드를 중단하므로 **모든 DB 시험 앞에서 `unset PGHOST PGPORT`(또는 `env -u PGHOST -u PGPORT`)** 한다. Homebrew `postgresql@17`로 전용 인스턴스를 만들었다(Docker의 `supabase_db_merrivoo`는 다른 프로젝트라 쓰지 않았다).

```
export PATH=/opt/homebrew/opt/postgresql@17/bin:$PATH LC_ALL=en_US.UTF-8
initdb -D /tmp/aip-s0-pg17 -U runner --auth=trust -E UTF8 --locale=C
pg_ctl -D /tmp/aip-s0-pg17 -o "-p 55417 -c listen_addresses=127.0.0.1 -c unix_socket_directories=/tmp -c deadlock_timeout=1s" -l /tmp/aip-s0-pg.log -w start
psql -X -h 127.0.0.1 -p 55417 -U runner -d postgres -Atc "create database aip_test_ci"
```

관측한 출력:

```
waiting for server to start.... done
server started
PostgreSQL 17.11 (Homebrew) on aarch64-apple-darwin27.0.0, compiled by Apple clang version 21.0.0 (clang-2100.3.34.2), 64-bit
CREATE DATABASE
aip_test_ci|17.11 (Homebrew)
```

첫 시도의 `initdb`는 `LC_ALL`/`LANG` 없이 실행해 `잘못된 로케일 설정` 오류로 실패했다. `LC_ALL=en_US.UTF-8`과 `--locale=C`로 해결했다. `deadlock_timeout=1s`는 `concurrency.md`의 측정 환경(`deadlock_timeout=1s`)과 맞춘 값이다.

DB 시험 공통 환경변수(CI 값은 `verify.yml`의 `AIP_TEST_DATABASE_URL`/`AIP_TEST_ADMIN_URL`, 포트 5433, 사용자 `runner`):

```
export AIP_TEST_DATABASE_URL='host=127.0.0.1 port=55417 dbname=aip_test_ci user=runner'
export AIP_TEST_ADMIN_URL='host=127.0.0.1 port=55417 dbname=postgres user=runner'
unset PGHOST PGPORT
```

CI와 같은 컨테이너 방식은 `postgres:17`(`verify.yml` services: `POSTGRES_USER=runner`, `POSTGRES_DB=aip_test_ci`, `POSTGRES_HOST_AUTH_METHOD=trust`, 포트 5433)이다. 이번에는 Docker 방식을 실행하지 않았다(`미실행`).

정리: `pg_ctl -D /tmp/aip-s0-pg17 stop`.

### 5.2 이번에 실행한 확인 (표적 테스트, 596ab5dd)

`CARGO_TARGET_DIR=/tmp/aip-s0-target`, 위 환경. "RED/GREEN" 열은 이 작업이 새로 쓴 테스트가 없으므로 해당 없음이다. 기존 테스트가 이 분류의 근거로 실제 통과하는지만 봤다.

| 분류 근거 | 명령 | 결과 |
|---|---|---|
| D1~D12 해결됨 | `cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --locked --test policy_hardening` | `test result: ok. 10 passed; 0 failed` |
| P0-2, P1-4 해결됨 | `… --test advisories` | `test result: ok. 8 passed` (`a_where_that_does_not_imply_the_row_policy_is_rejected` 외) |
| 8.4 멱등 조회 해결됨 | `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --test apply_idem_lookup_error` | `idempotency_lookup_errors_are_retried_or_reported_and_never_run_the_write ... ok`, `1 passed` |
| a2, a3 교착 해결됨 | `cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --locked --test bundle_lock_order_compose_where` | `a_line_before_a_compose_or_where_update_of_the_same_product_does_not_deadlock ... ok` |
| a1(E9) 미해결 재현 | `PROBE_N=2 PROBE_ROUNDS=3 cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --locked --features test-db --test concurrency_probe_http -- --ignored --nocapture --exact a1_opposite_order_two_product_bundles` | `PROBE a1-opposite-order round=0 elapsed=17ms tally={"OK": 2}`, `round=1 elapsed=1179ms tally={"OK": 2}`, `round=2 elapsed=1180ms tally={"OK": 2}` |
| D13 해결됨 | `AIP_BIN=/tmp/aip-s0-target/debug/aip node --test product/tests/external-create-order.test.mjs` | `ℹ pass 1`, `ℹ fail 0` |
| 소비자 앱(HEAD) | `env -u PGHOST -u PGPORT AIP_BIN=… AIP_TEST_DATABASE_URL=… node product/tests/fixtures/expr-apps/run.mjs` (먼저 `npm ci --prefix spikes/spike-0-ts --ignore-scripts --no-audit --no-fund`) | `revision: 596ab5dd23b9, aip 1.0.0, HTTP=aip service serve, SDK=@aip/sdk(tarball 설치)+python aip_sdk`, `합계: PASS 64 / FAIL 0 / SKIP 0` |
| 버전, 진단 | `aip --version`, 빈 `.aip`·BOM `.aip`·Q2·Q4 정의에 `aip service check` | `aip 1.0.0`. 빈 파일은 `"column":0,"line":0`(G1). BOM 파일은 `ok:true`. Q2 효과 산술은 `ok:true`. Q4는 쉼표 안내 메시지 |
| CI 상태 | `gh run list -R winterholic/Application-Intent-Protocol -c 596ab5dd…`, `gh api repos/…/branches/main/protection`, `git ls-remote --tags origin` | 8개 workflow success. 보호 404. 태그 없음 |

### 5.3 검증 명령 표 (후속 작업자용)

시간은 이번에 잰 값이 아니면 적지 않았다(`확인 필요`).

| 종류 | 명령 | 필요 조건 | 비고 |
|---|---|---|---|
| 빠른 단위(DB 불필요) | `cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --locked --test policy_hardening --test advisories` | cargo | 실행(이번) 10+8 passed. sema·진단 시험 대부분이 DB 없이 돈다 |
| 서식·lint | `cargo fmt --all --check`, `cargo clippy --manifest-path spikes/<엔진>/Cargo.toml --all-targets --locked -- -D warnings` | cargo | 엔진 목록 `tools/verify.sh`의 `for engine in spike-v1-fixture … spike-v11-dev-checks` |
| 엔진 DB 시험 | `cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --locked --no-fail-fast`, 같은 형태로 `spike-v2-read`, `spike-v6-transport` | PG17 + `AIP_TEST_DATABASE_URL`, `AIP_TEST_ADMIN_URL`, `PG*` 미설정 | spike-v6의 `v12`~`v16` 시험은 `tsc`(`npm ci --prefix spikes/spike-0-ts --ignore-scripts`)가 필요(`concurrency.md` §7). 전체는 느리다(`확인 필요`) |
| 탐색(ignored) | `cargo test … --features test-db --test concurrency_probe_http -- --ignored --nocapture --exact <이름>` | PG17, 프로세스당 1개 | `PROBE_ROUNDS`, `PROBE_N` |
| 제품 흐름 | `cargo test -p aip-cli --locked --test service_flow` | PG17 + 환경변수, cli 빌드 | 정책 6개는 `… service_policy_` 필터(`semantic-conformance.yml:66`) |
| worker(확장) | `cargo test --manifest-path spikes/spike-v4-worker/Cargo.toml --locked`, spike-v6 `write_extension_*` | node, python3. Linux는 bwrap과 `AIP_TEST_BWRAP=1` | macOS는 `Isolation::MacNetDeny`. `AIP_TEST_BWRAP=1` 없이 Linux에서 돌면 일부 시험이 **조용히 통과**한다(6.3절) |
| SDK(TS, Python) | `node --test --test-concurrency=1 product/tests/sdk-semantic-e2e.test.mjs`, `python3 -W error::ResourceWarning -m unittest product.tests.python_sdk_test` | `npm ci --prefix spikes/spike-0-ts`, `cargo build --locked -p aip-cli`, PG17 | `sdk-semantic-e2e.yml:65-77`. 이번에 `미실행` |
| 소비자 앱(최신) | `node product/tests/fixtures/expr-apps/run.mjs` | PG17(없으면 docker로 `postgres:17`을 띄움), `AIP_BIN` 또는 cargo 빌드, `tsc` | 실행(이번) PASS 64 |
| 독립 소비자 | `python3 verification/1.0-consumers/run.py` | cwd가 Core 체크아웃, docker, `cargo build --release -p aip-cli`, `npm ci` | **현재 `4e132564`를 단언**. HEAD에서는 실패한다(`미실행`, C-01) |
| 외부 create | `AIP_BIN=… node --test product/tests/external-create-order.test.mjs` | PG17, `psql`, cli 빌드 | 실행(이번) 통과 |
| 릴리스 게이트 | `bash scripts/release-gate.sh --baseline 4c27040c6d4a2a2931321e1980842be79512bdb0 --ref <sha> --out <새 디렉터리>` | docker native fixtures, PG, node, python | 내부에서 `tools/verify.sh`를 다시 돈다. 실행하지 않음 |
| Core 전체 | `bash tools/verify.sh` | `AIP_TEST_DATABASE_URL`, `AIP_TEST_ADMIN_URL`, `psql`, `mypy` 2.4.0 또는 `uv`, `tsc` | 무겁다. 이번에 `미실행` |

### 5.4 workflow별 트리거와 실행 명령

`RC`는 `release-candidate.yml`의 `needs`에 포함됨을 뜻한다. 15개 모두 `workflow_call`로 같은 SHA에서 돈다.

| workflow 파일 | main push 트리거 | 그 밖의 트리거 | 주요 명령 | RC |
|---|---|---|---|---|
| `verify.yml` | 모든 push | PR, dispatch(진단 입력 2개), call | `bash tools/verify.sh`; `node product/tests/fixtures/expr-apps/run.mjs`; `scripts/verify-mysql-worker-e2e.sh run-ci`; probe·isolation 단계(`AIP_TEST_BWRAP=1`) | O |
| `aip-release-gate.yml` | 경로 필터(`crates/**`, `product/**`, `spikes/**`, `tools/**`, `scripts/**`, `docs/**`, `.github/workflows/**` 등) | dispatch, call | `scripts/release-gate.sh --baseline 4c27040c… --ref $GITHUB_SHA`(verify.sh, release 빌드, package, artifact-smoke, upgrade-drill) | O |
| `app-packages.yml` | 모든 push | PR, dispatch | `node scripts/check-versions.mjs`; `node --test product/tests/{version-check,release-manifest,sdk-package,…}`; `app-package-e2e`; `app-deploy-live` | O |
| `brokers.yml` | 모든 push | PR, dispatch | `cargo test -p aip-execution --features external-brokers --test brokers_live -- --ignored`; `cargo test -p aip-service --features external-brokers broker_worker_recovers -- --ignored` | O |
| `cgroup.yml` | 모든 push | PR, dispatch | `isolation::startup_experiment::run_isolated_node_version_once -- --ignored`, `isolation::tests`, `spikes/spike-v4-worker/tests/cgroup-v2-ci.sh` | O |
| `object-storage.yml` | 모든 push | PR, dispatch | `cargo test -p aip-execution --lib --test cloud_storage`; `scripts/test-minio-object-storage.sh`; `object_ledger`; storage provider 단위 시험 | O |
| `worker-jobs-linux.yml` | 경로 필터(`crates/**`, `product/**`, `porting/**`, `spikes/spike-v1/v3/v4/v6/**`) | dispatch(`crypto_repair_only`), call | `cargo test … isolation::tests`(`AIP_TEST_BWRAP=1`), spike-v6 job 시험들, `node --test product/tests/service-smoke.test.mjs`, aip-migrate `explicit_parent_job_schema_upgrade…`, FlaskBB mail·sticky 시험, 암호화 시험 | O |
| `macos-platform-tests.yml` | 경로 필터(`crates/aip-cli`, `crates/aip-service`, `spikes/spike-v4-worker`, `spikes/spike-v6-transport`) | dispatch, call | `isolation::tests`; `--test v4_1`; `service_flow … operation_only_service_preserves_principal_contract_revocation_and_deployment_fences` | O |
| `database-contracts.yml` | 경로 필터(`crates/aip-test-support`·`aip-cli`·`aip-service`·`aip-migrate`, v2·v3·v6 `src`) | dispatch, call | `cargo test -p aip-test-support`, `-p aip-cli --test least_privilege`, MySQL·MariaDB 계약(범위 밖) | O |
| `deploy-checks.yml` | 경로 필터(`product/tools/proxy-check.mjs`, v6 `src`, `aip-service`, `aip-auth`, `conformance/**`) | dispatch, call | `cargo test -p aip-auth`; `node product/tools/proxy-check.mjs`; `scripts/smoke-idp-proxy.sh`; `scripts/smoke-keycloak.sh` | O |
| `dependencies.yml` | 경로 필터(Cargo·lock, `deny.toml` 등) | PR(경로), 월요일 schedule, dispatch, call | `bash tools/dependency-check.sh`(cargo audit/deny, npm audit, pip-audit) | O |
| `flaskbb-parity.yml` | 경로 필터 | dispatch, call | FlaskBB upgrade·settings 시험, `cargo test … foreign_key_refusal commit_classification`, `wire_codes` | O |
| `flaskbb-post-order.yml` | 경로 필터(+ 개별 브랜치) | dispatch, call | 원본 oracle 비교(`continue-on-error` 14개), `latest_effect_match`, `diagnostics`, post-order·topic-move·post-unhide 시험 | O |
| `sdk-semantic-e2e.yml` | **push 트리거 없음** | PR(경로 필터), dispatch, call | `cargo fmt --all -- --check`; clippy(`aip-cli`, `aip-service`, `aip-execution`); `node --test --test-concurrency=1 product/tests/sdk-semantic-e2e.test.mjs` | O |
| `semantic-conformance.yml` | **push 트리거 없음** | PR(경로 필터), dispatch, call | `cargo test -p aip-sema --test shared_semantic_fixture`; `-p aip-cli --features poc-runtime --test core_semantic_fixture`; `-p aip-cli --test service_flow` | O |
| `independent-consumer-rc.yml` | 브랜치 `verify/1.0-independent-consumer-20261121` push만 | dispatch | Core `4e132564` 빌드, `python3 verification/1.0-consumers/run.py` | **X**(F-01) |
| `release-candidate.yml` | — | dispatch, `release/**` push, `v*` 태그 push | 위 15개를 `workflow_call`로 호출, `required` job이 전부 `success`인지 확인 | — |
| `release.yml` | — | `v*` 태그 push(게시), PR 경로·dispatch는 dry-run | 버전 검사, CLI·SDK 빌드, `assemble`, `publish-*`(태그 push + `release` environment 승인 시) | — (게시 금지) |

## 6. 건너뛰는 테스트: `#[ignore]`, `cfg(target_os)`, 환경변수 조건

`git grep -E '#\[ignore'` 100건, `cfg(target_os…)` 게이트, `AIP_TEST_*` 조건을 이 HEAD에서 센 결과다. "실행"은 workflow 이름이다.

### 6.1 `#[ignore]`

| 테스트(위치) | ignore 이유 | 실제 실행되는 곳 |
|---|---|---|
| `spikes/spike-v6-transport/tests/concurrency_probe_http.rs` 18개(`a1`~`a5`, `i1`~`i5`, `e1`~`e5`, `n1`, `f1`) | 탐색 시험, `sqlgen::set_schema` 프로세스 전역 | **어느 workflow도 아님**(`tools/verify.sh`는 `--ignored` 없이 실행) → B-06 |
| `spikes/spike-v6-transport/tests/socketio_transport.rs` 1개 | socket.io-client 필요 | `tools/verify.sh`(`-- --ignored`) → Verify, release gate |
| `spikes/spike-v6-transport/tests/object_ledger_s3.rs` 6개 | MinIO TLS fixture | `object-storage.yml`(`scripts/test-minio-object-storage.sh`) |
| `crates/aip-execution/tests/{cloud_storage.rs(minio), object_ledger_scan_minio.rs}` | MinIO | `object-storage.yml` |
| `crates/aip-execution/tests/cloud_storage.rs::gcs_hmac_actual_xml_api_roundtrip_when_credentials_are_configured` | 실제 GCS 자격 증명 | **없음**(자격 증명 필요, 의도) |
| `crates/aip-execution/tests/brokers_live.rs` 2개, `crates/aip-service/src/brokers.rs` 1개 | 실제 Kafka·RabbitMQ | `brokers.yml` |
| `spikes/spike-v4-worker/src/isolation.rs::run_isolated_node_version_once` | opt-in startup experiment | `cgroup.yml` |
| `spikes/spike-v4-worker/src/cleanup_tests.rs::stop_kills_double_forked_daemon` | 알려진 한계(cgroup 또는 macOS sandbox 필요) | **없음**(문서화된 한계) |
| `crates/aip-execution/tests/encryption_rekey.rs:530 rekey_ref_change_after_select_is_rejected_by_cas_scope` | 알려진 공백(ref 열 없음) | **없음**(E-05, 암호화 필드는 1.0 지원 밖) |
| `crates/aip-migrate/tests/large_backfill.rs` 3개 | 수 분 걸리는 측정 | **없음**(수동 측정) |
| `crates/aip-migrate/src/lib.rs:1652 explicit_parent_job_schema_upgrade_adds_trace_id_and_rejects_drift` | 격리된 DB와 빌드된 CLI 필요 | `worker-jobs-linux.yml`(`--ignored`) |
| `crates/aip-service/tests/slice13b_host.rs:73` | `slice13b_oracle.py`가 구동 | **없음**(이름 검색 기준, MySQL·Mongo 의존이라 범위 밖) |
| MySQL·MariaDB 계열 약 60개(`crates/aip-migrate/tests/mysql_*`, `crates/aip-service/tests/mysql_*`, `crates/aip-service/src/mysql_jobs*`, `crates/aip-cli/tests/mysql_*`) | 실제 MySQL·MariaDB 필요 | 일부는 `database-contracts.yml`(`--ignored`/`--include-ignored`), FaceReview 계열은 `scripts/verify-mysql-worker-e2e.sh`(Verify step 17)와 `scripts/facereview-e2e-suites.py`. **1.0 범위 밖** |

### 6.2 `cfg(target_os = "macos")`로만 컴파일되는 시험

| 시험(위치) | macOS 전용 이유 | 실제 실행되는 곳 |
|---|---|---|
| `spikes/spike-v4-worker/src/isolation.rs` macOS 시험 5개, `spikes/spike-v4-worker/tests/v4_1.rs:156` `MacNetDeny` 블록 | Seatbelt 자체가 대상 | `macos-platform-tests.yml`(`isolation::tests`, `--test v4_1`) |
| `crates/aip-cli/tests/service_flow.rs:293` `operation_only_service_preserves_principal_contract_revocation_and_deployment_fences` | 서비스 설정에 무격리 모드 없음 | `macos-platform-tests.yml` |
| **`spikes/spike-v6-transport/tests/extension_lifetime.rs`**(파일 전체, `extension_timeout_and_server_shutdown_reap_real_worker_children`) | `mod macos` | **어느 workflow도 아님** → B-05 |
| **`spikes/spike-v6-transport/tests/extension_unix.rs`**(`mac_net_deny_blocks_a_live_unix_socket_for_node_and_python`) | `#![cfg(target_os = "macos")]` | **어느 workflow도 아님** → B-05 |
| **`spikes/spike-v6-transport/tests/write_extension_lifetime.rs`**(`write_slots_deadlines_and_shutdown_release_children_and_uncommitted_effects`) | 같음 | **어느 workflow도 아님** → B-05 |
| **`spikes/spike-v6-transport/tests/write_session_outcome.rs`**(`committed_write_is_reported_even_when_its_admitting_session_expires_during_execution`) | 같음 | **어느 workflow도 아님** → B-05 |
| **`spikes/spike-v6-transport/tests/durable.rs:743`**(`messages_share_atomic_job_effects_and_reject_foreign_acknowledgements`) | `#[cfg(target_os = "macos")]` | **어느 workflow도 아님** → B-05 |
| `spikes/spike-v6-transport/tests/read_extensions.rs:284` 블록 | macOS에서만 probe DB 확장 서버 추가 | macOS에서만 해당 블록이 실행됨(Linux는 건너뜀) |
| `crates/aip-cli/tests/mysql_facereview_trending_job.rs:1349` | `cfg!(target_os = "linux")` 분기 | MySQL, 범위 밖 |

`concurrency.md` 8.1이 "git grep로 모두 찾았다"고 적은 목록에는 위 굵은 5곳이 없다. 이번에 `git grep -n 'cfg(target_os'`로 새로 찾은 것이다. 5곳이 `spike-v6-transport` 소속이라 이 크레이트의 Linux Verify(`tools/verify.sh`)는 컴파일 단계에서 이 파일들을 비운다(시험 0개로 통과). 아래 분기형(`#[cfg(target_os = "macos")]`/`not(...)` 쌍으로 같은 시험 본문을 둔 것)은 Linux에도 짝이 있어 건너뛰지 않는다: `grant_tests.rs:19-23`, `frame_boundary.rs:23-27`, `frame_write.rs:81-85`, `read_wire.rs:138-142`, `value_boundary.rs:57-61`, `Isolation`을 `cfg!`로 고르는 `spike-v4-worker/tests/*`.

### 6.3 환경변수·플랫폼으로 조용히 건너뛰는 시험

| 시험(위치) | 조건 | 건너뛴 결과 | 실제 실행되는 곳 |
|---|---|---|---|
| `spikes/spike-v4-worker/src/isolation.rs:1314,1331,1348`, `cleanup_tests.rs:140` | Linux에서 `AIP_TEST_BWRAP != "1"` | `eprintln!("skipped Linux bwrap integration test…")` 후 **통과로 반환** | `AIP_TEST_BWRAP=1`을 주는 단계만: `verify.yml:170,173`, `worker-jobs-linux.yml:119`, `cgroup.yml:52`. `tools/verify.sh`의 엔진 시험과 로컬 실행은 건너뛴다 |
| `spikes/spike-v4-worker/src/isolation.rs:890` `linux_bwrap_startup_diagnostics_compare_direct_sequential_and_concurrent` | `AIP_TEST_BWRAP_DIAGNOSTICS=1` | 즉시 `return`(통과) | **어느 workflow도 설정하지 않음**(진단 전용, 실질적으로 실행 안 됨) |
| `crates/aip-cli/tests/mysql_facereview_auth.rs:1212`, `mysql_facereview_admin_ui.rs:663` | `AIP_FACEREVIEW_UI_DIR`, `AIP_FACEREVIEW_ADMIN_UI_DIR` 미설정 | 브라우저 journey 생략 | `verify.yml:299-300`이 설정(MySQL, 범위 밖) |
| `porting/flaskbb/tests/mail-fixture.test.mjs:17,33`, `mail-jobs.test.mjs:187`, `porting/infisical/tests/test_project_membership.py:139` | `process.platform !== 'linux'` | skip | Linux CI(`worker-jobs-linux.yml`) |
| `product/tests/python-build-script.test.mjs:34` | python `build` 도구 없음 | skip | 도구 설치 여부에 따름(`확인 필요`) |
| `product/tests/google-storage-provider.test.mjs:95` | 공식 SDK 설치됨 | skip | `object-storage.yml`, `tools/verify.sh` |
| `product/tests/slice13d_{home,pool_builder}_oracle_test.py` | `FACEREVIEW_SOURCE_PATH` 미설정 | `SkipTest` | **어느 workflow도 설정하지 않음**(FaceReview 원본 oracle) |
| `porting/infisical/tests/{legacy_repro,test_live_db,test_principal_lifecycle,test_project_membership,test_encryption_contract}.py` | DB·바이너리 없음 | `SkipTest` | `worker-jobs-linux.yml`(Infisical 단계, 환경 구성 시) |
| `product/tests/test_facereview_first_main_browser.py:86` | Playwright Chromium 없음 | skip | `verify.yml` 브라우저 준비 단계 |

### 6.4 어떤 workflow에서도 이름이 안 나오는 시험 파일

`git ls-files`의 시험 파일(`*.test.{mjs,ts,js}`, `*_test.py`, `test_*.py`) 144개 중 `.github`, `scripts`, `tools`, `package.json`에서 이름이 한 번도 나오지 않고 Rust 시험의 `Command` 인자로도 안 나오는 것을 이름 검색으로 걸렀다. 글롭(`node --test dir/`) 호출은 못 본다.

| 파일 | 비고 |
|---|---|
| `prototype/tests/{cache-completion,extension-sdk,http-recovery,read-decoder,write-extension-sdk}.test.mjs`, `prototype/tests/test_bundle.py` | 레거시 prototype. `prototype/tests/sdk-package.test.mjs`와 `cargo test --manifest-path prototype/Cargo.toml --test cli`만 CI(`tools/verify.sh`) |
| `spikes/spike-0-ts/test/{analyzer,http,runtime}.test.ts` | `spikes/spike-0-ts/package.json`에 `"test": "node --test --test-concurrency=1 test/"`가 있으나 CI는 이 패키지를 `tsc` 제공용으로만 설치(`npm ci`) |
| `spikes/spike-v6-transport/client/{e2e,extension-rejection,uuid}.test.ts` | `v13.safe`, `v13.string`은 `v13_idwire.rs`가 `format!("v13.{phase}.test.ts")`로 실행하므로 제외했다. 나머지 3개는 Rust 호출을 못 찾았다(`확인 필요`) |
| `porting/flaskbb/tests/report-bulk.test.mjs`, `test_last_post_source_oracle.py` | FlaskBB(필수 아님) |
| `tools/test_facereview_{browser,evidence,inventory,runner,socket}.py`, `tools/test_infisical_inventory.py`, `scripts/test_finish_storage_evidence.py` | `tools/verify.sh`는 `product.tests.test_facereview_*`만 돌린다 |
| `product/tests/fixtures/facereview-mysql/reference/test_writes_runner_config.py` | FaceReview(MySQL) |

(`test_slice14k_watch.py`, `test_slice14l_frame.py`는 `tools/verify.sh:93`의 `unittest discover -p 'test_*.py'`가 돌리므로 목록에서 뺐다. 처음 이름 검색은 이 두 개를 놓쳤고 글롭을 확인해 바로잡았다.)

## 7. 이 문서의 한계

- `미실행`: `tools/verify.sh` 전체, `release-gate.sh`, `run.py`(독립 소비자), SDK semantic e2e, MySQL, Linux bwrap·cgroup, macOS Seatbelt 시험, Docker 기반 `postgres:17`(CI와 같은 방식), `policy-probes.mjs` 재실행(C1~C9).
- `코드 확인만`: R1(읽기 확장 무선언 읽기), durable.rs:1201의 결과 추정, G10의 줄 위치, 쓰기 확장 `/apply` 헤더 코드.
- 줄 번호는 `596ab5dd` 기준이다. 후속 PR로 밀린다.
- 이 문서의 분류는 이 세션의 판단이다. 소유자 결정 항목(공개 count 기본 의미, K1~K4, R1, E9 잠금 계약, 진단 JSON 장기 호환, 확장 ABI)을 이 문서가 확정하지 않는다.
- 임시 PG17 인스턴스는 `/tmp/aip-s0-pg17`(포트 55417)에 만들었다. 작업 후 `pg_ctl … stop`으로 내렸다.
