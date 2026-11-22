# Track E 결과: 진단·확장·구조 (2026-11-22)

브랜치 `stab/e-20261122`. 최종 통합 기준은 fetch한 `origin/main` `0e3f0c80`; G9·G1 커밋을 이 기준 위로 재적용(rebase)하고 미커밋 R1 변경을 복원했다. 통합 SHA와 D 인계 시점은 7절. 환경: `CARGO_TARGET_DIR=/tmp/aip-stab-e-target`, 이번 Run에서는 PostgreSQL을 초기화하거나 DB 테스트를 시도하지 않았다. 이전 작업 카드가 기록한 별도 Codex 검증 샌드박스의 PG17 `shmget` 권한 거부는 재시도·우회하지 않았다.

## 한눈에

| 항목 | 상태 |
|---|---|
| E1 기계 필드와 설명 필드 구분 명세, 가산 변경(G2·G3·G6, G1은 선택 필드로 표현) | 완료. 명세는 [`diagnostics-fields.md`](diagnostics-fields.md) |
| E1 G4 `schema` 필드, 장기 호환성 약속, G5, G10 | 구현하지 않음. 소유자 결정으로 분리 → [`decisions/e.md`](decisions/e.md) D-E1, D-E2, D-E5, D-E6 |
| E1 G9와 문구로만 갈리는 코드 | 완료: 쓰기 확장·job의 비공개 전이 접근을 `DEF.ACCESS_TARGET_NOT_EXPOSED`로 분리. FlaskBB workflow 전환은 F1에 요청(7절) |
| E2 권한 선언과 `ctx.data.*` 비교표, R1 | 비교표 완료. 읽기 확장 `access read.<Resource>` 옵트인과 v4 worker 제한 구현. 무선언 확장은 기존 계약·동작 유지 |
| E3 큰 모듈 4개 분석, 진단 출력 분리 | 분석은 아래 3절. 분리는 `crates/aip-service/src/diagnostics.rs`만. 나머지는 후보로만 |
| E-08 진단 시험이 PG 없이 | 완료(`AIP_TEST_NO_DB=1`) |

## 1. 변경

| 커밋 | 파일 | 목적 |
|---|---|---|
| `3aa98afa` | `spikes/spike-v1-fixture/src/diag.rs`, `enrich.rs` | `located`, `anchor`, `scopePath`를 `Diag::json`에 추가(기존 필드 불변). `enrich`가 `scope`를 조각 배열로 넘김 |
| 〃 | `spikes/spike-v1-fixture/src/codes.rs`, `crates/aip-cli/src/registry.rs` | 레지스트리 `Info`에 `status`, `since`, `replaced_by`, `severity()`, `json()`. `aip diagnostics --json`에 `severity`·`status` |
| 〃 | `spikes/spike-v11-dev-checks/src/lib.rs` | `Advisory.span`(G2). 컴파일러 경고의 앵커가 `diag::anchor`를 쓴다 |
| 〃 | `crates/aip-service/src/diagnostics.rs`(새), `lib.rs` | 진단 출력을 모듈로 분리. 경고에 `line`·`column`·`explain`, `read_bounded` 실패에 `reason` |
| 〃 | `crates/aip-test-support/build.rs` | `AIP_TEST_NO_DB=1`(E-08) |
| 〃 | `crates/aip-cli/tests/service.rs` | 정확한 키 집합 비교를 최소 계약 검사로 교체(G3), 새 시험 6개 |
| 〃 | `spikes/spike-v1-fixture/tests/diagnostic_fields.rs`, `published_codes.txt`, `diagnostics.rs` | PG 없는 시험 9개, 게시된 코드 삭제 방지, 폐기 코드는 "발생지 없음" 검사에서 제외 |
| 〃 | `spec/definition-diagnostics.md` | **골든 갱신 1회**(`AIP_UPDATE_GOLDEN=1 … --test diagnostics`). 머리글 한 문단뿐이고 코드 표는 불변 |
| `6b1e05a3` | `crates/aip-service/src/diagnostics.rs`, `lib.rs` | `report`, `output_error`, 종료 코드 결정(`finish`)을 같은 모듈로. 동작 불변 |
| `4fbe131b` (원본 `c649603f`, rebase 후) | `spec/definition-diagnostics.md`, `spikes/spike-v1-fixture/src/{codes,sema}.rs`, `diagnostic_fields.rs` | G9 전용 코드. 쓰기 확장과 job 경로에 적용 |
| `06e5f2d8` (원본 `7e4684e8`, rebase 후) | `spikes/spike-v1-fixture/src/{ast,extract,hlit,limits,parser,sema}.rs`, 진단 시험 | G1에서 위치를 알 수 있는 정의 진단의 실제 좌표 채움. 0:0 + `located:false`는 위치를 모르는 경우 유지 |
| `753be26f` | `sema.rs`, v4 worker, 진단·worker 시험, `ctx.d.ts`와 E 결정·명세 문서 | R1 opt-in `reads` 계약과 실행 시 목록 제한. 미선언 확장의 동작은 그대로 |
| 이 커밋 | `plan-docs/stabilization-20261122/**`, `CHANGELOG.md` | 명세, 결정, E2 표와 스캔 스크립트, 이 문서, baseline 행 갱신 |

## 2. RED → GREEN

| 시험 | RED (고치기 전 또는 일부러 깬 상태) | GREEN |
|---|---|---|
| `diagnostic_fields`(PG 없음) | 6개 실패: `left: Null right: true`(`located`), `Null` vs `["resource Recruitment","expose read"]`(`scopePath`), 빈 정의의 `OldReader`가 `located` 없음에서 실패 | `cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --locked --test diagnostic_fields` → `9 passed` |
| `service.rs`의 경고 위치·`reason` | 고친 코드를 잠깐 되돌려(`advisory.span.filter(\|_\| false)`, `reason` 제거) → `a_failure_that_is_not_a_rejected_definition_names_its_reason ... FAILED`, `service_check_json_keeps_success_and_failure_diagnostic_contracts ... FAILED` | 원복 후 `AIP_TEST_NO_DB=1 cargo test --locked -p aip-cli --test service` → `10 passed` |
| 레지스트리 목록 `severity`·`status` | `registry.rs` 되돌림 → `the_registry_listing_carries_severity_and_status_for_every_definition_code ... FAILED`(`severity: {"area":"def",..` 키 없음) | 적용 후 `1 passed` |
| E-08 | `PGHOST=127.0.0.1 PGPORT=55031 cargo test --locked -p aip-cli --test service`(옵트아웃 없이) → `failed to run custom build command for aip-test-support … build.rs:18` | `PGHOST … AIP_TEST_NO_DB=1 …` → 같은 시험 `10 passed`(컴파일 포함 약 24초). DB 시험은 여전히 `error: AIP_TEST_DATABASE_URL is required` 로 컴파일 실패 |
| G9 전용 코드 | 새 진단 시험에서 쓰기 확장·job 경로 모두 `DEF.ACCESS_TARGET_NOT_EXPOSED`; 이전 코드로 되돌린 job 경로는 새 시험이 실패(RED) | 원복 후 `AIP_TEST_NO_DB=1 cargo test --locked --manifest-path spikes/spike-v1-fixture/Cargo.toml --test diagnostic_fields` → `19 passed` |
| R1 선언 호환 | 무선언 facts에 `reads` 없음, `access` 동일; Node·Python이 선언 없는 확장에서 공개 resource를 읽음 | 선언한 읽기는 정상, 목록 밖 읽기는 DB 계획 전 `WIRE.ACCESS_NOT_DECLARED`; `read_extension_declared_reads` → `2 passed` |
| G1 좌표 | literal, source-size, multiple-block, wrong-binding의 표적 좌표와 이전 필드 전용 reader를 `diagnostic_fields`에서 고정 | `diagnostic_fields` 전체 `19 passed`; `cargo fmt --all -- --check` 통과 |

## 3. 측정

| 측정 | 값 |
|---|---|
| 호환: 이전 바이너리(`4f44ca87`)와 이후 바이너리의 `service check` 출력 비교 | 입력 10종(빈 `.aip`, 오류 정의, 경고 정의, `.aipoc`, 없는 파일, actor 없는 정의, flaskbb, prototype, facereview, stockdesk)에 `--dev-checks` 유무를 더한 15개 호출에서 이전 출력의 모든 필드를 이후 출력과 비교. `old fields compared: 125 differences: 0`, 종료 코드 15개 모두 같음. 새 키는 `anchor`, `column`, `explain`, `line`, `located`, `scopePath`뿐(경고와 오류에서 각각). `executionDigest` 불변(`3801434d…`) |
| 리팩터 동일성 | 모듈 분리 전후 `empty`, `unknown_key`, `recruitment` 출력이 바이트 단위로 같음(`cmp`) |
| 파일 크기 | `crates/aip-service/src/lib.rs` 2790 → 2710줄, 새 `diagnostics.rs` 137줄 |
| PG 없이 도는 진단 시험 | `diagnostic_fields` 9개 0.02초, `diagnostics` 29개 0.01초(엔진 크레이트는 처음부터 PG 의존이 없었다) |
| E2 스캔 | 정의 35개, 확장·operation 99개(읽기 26, 쓰기 28, job 45) |
| 이번 통합의 PostgreSQL 비사용 검사·빌드 | 전체 `spike-v1-fixture` 시험 통과(AIP_TEST_NO_DB=1); 진단 표적 19개, v4 실제 Node/Python IPC 2개 통과. v1·v4 `cargo build --locked` 통과 |
| DB 전체 시험 제한 | v4 전체 시험은 DB 통합 시험의 `test_database_url!()`가 `AIP_TEST_DATABASE_URL is required`로 컴파일을 막음. 반복·PG 초기화 시도 안 함. 프로젝트 CI는 `.github/workflows/verify.yml`의 `core` job에서 PostgreSQL 17 서비스와 `bash tools/verify.sh`로 전체 엔진 시험을 수행 |

## 4. E3 구조 분석 (큰 모듈 4개)

수치는 `grep`으로 센 값이다. `확인 필요`로 표시하지 않은 분류는 코드를 읽고 판단한 것이다.

| 모듈 | 줄 수 | 책임이 섞인 곳 | 상태 공유·결합 | 중복 |
|---|---|---|---|---|
| `spikes/spike-v1-fixture/src/sema.rs` (A 소유) | 3291 | 한 `impl Ctx`에 식 검사(`ex :654`), 숨은 필드·정책 검사(`:887-1330`), resource·expose·aggregate·operation·extension 검사(`:1437-2930`)가 모두 들어 있다. 파일 끝에 정책 확장 한도(`validate_policy_expansion :3086`)와 `implies_policy :2971` | `Ctx`(진단 `diags`, resource 맵, `scope`)가 모든 메서드에 `&mut self`로 넘어가 분리하려면 진단 수집과 읽기 전용 view를 나눠야 한다 | `reject_hidden_*`와 `hidden_*` 짝이 입력·create·check 비교에서 같은 모양으로 반복(쌍의 정확한 수는 확인 필요), `extension`의 읽기·쓰기 분기가 mongo/redis grant 블록을 각각 복제 |
| `spikes/spike-v3-write/src/lib.rs` (B 소유) | 2424 | 단건 전이(`apply :287`~`delete_rows`), 효과(`effects :880-1127`, 약 250줄), create(`create_row :1236`), 산술 입력(`:1559-1700`), bundle(`:1757-2130`), compose(`:2140-2392`) | `static AUDIT_TABLE`(RwLock 전역, `:43`). 잠금 순서가 bundle·compose 안에 흩어짐 | `apply_inner`/`bundle_once`/`compose_once`가 같은 골격(begin → 연산 → `run_checks` → `commit`)을 각각 씀 |
| `spikes/spike-v6-transport/src/lib.rs` (B 소유) | 2279 | `serve_request`(`:1118-2175`, 약 1050줄) 한 함수가 `/read`, `/apply`, `/status`, `/session`, compose·bundle·write extension 라우팅, 인증, admission, 응답 작성을 모두 한다. 멱등(`handle_apply :337`, `apply_attempt :372`)도 같은 파일 | `static PROTOCOL_VERSION: OnceLock`. 요청 상태가 `serve_request`의 지역 변수들에 걸쳐 있다 | 응답 작성 `reply_*` 여섯 변형 |
| `crates/aip-service/src/lib.rs` (E 소유) | 2790 → 2710 | CLI enum과 설정 구조체(`:30-490`), 설정 로딩·검증(`configuration_with_mode :556`, 연동 검증 `:667-890`), 정의 로드, **진단 출력**, 명령 실행(`run`), audit·jobs·principal 명령(`:1173-1436`), `serve`(`:1585`) | 대부분 순수 함수. 전역 상태 없음 | `load`가 거부 봉투를 세 군데에서 같은 `json!`로 만들던 것, `mysql_service.rs`의 `serve`가 `report`·`server_start_error`를 따로 씀 |

이미 분리돼 있는 모범: `policy_advice.rs`, `spike-v11-dev-checks`, `write_extension.rs`, `slots.rs`, `durable.rs`.

**이번에 분리한 곳**: `crates/aip-service/src/diagnostics.rs`. 경계가 분명하고(JSON 값을 만들고 쓰고 종료 코드를 정하는 일뿐, 다른 모듈의 상태를 쓰지 않는다) E 소유 파일이며 이번 결함 수정(`reason`, 경고 위치, `definition_rejected` 한 곳 통일)이 그 안에서 일어났다. 옮긴 것: `error`, `error_with_reason`(새), `definition_rejected`(세 곳의 중복을 하나로), `advisory_json`, `check_report`, `server_start_error`, `json_error`, `redact_json_reason`, `report`, `output_error`, `finish`(종료 코드). 동작은 바뀌지 않았다(3절 측정).

**분리 후보로만 남긴 것**(다른 Track 소유라 건드리지 않았다):

| 후보 | 위치 | 소유 | 경계 |
|---|---|---|---|
| 숨은 필드·정책 검사 | `sema.rs:887-1330` | A | `access_hidden_reads`, `reject_hidden_input_compare` 등이 자체 완결 |
| 정책 확장 한도 | `sema.rs:3086-3280` | A | 자유 함수, `Ctx`를 쓰지 않음 |
| 확장 계약 | `sema.rs:2821-2930` | A→E→D | `extension`, `external_grant` |
| bundle·compose 실행 | `spike-v3-write/src/lib.rs:1757-2392` | B | `bundle_reference_locks`, `compose_exec` |
| 효과 실행 | `spike-v3-write/src/lib.rs:880-1180` | B | `effects`, `effect_targets_at_transition_start` |
| 트랜잭션·멱등 | `spike-v6-transport/src/lib.rs:146-560` | B | `idempotency_ddl`, `handle_apply`, `apply_attempt` |
| 요청 라우팅 | `spike-v6-transport/src/lib.rs:1118-2175` | B | `serve_request`를 경로별 함수로 |

E 소유라 다음에 할 수 있는 것: 설정 구조체와 검증(`lib.rs:30-490`, `:556-890`)을 `config.rs`로, audit·jobs·principal 명령(`:1173-1436`)을 `commands/`로, `mysql_service.rs`의 `serve`와 `serve`의 공통부 합치기. 이번에는 결함 수정과 겹치지 않아 하지 않았다.

## 5. 처리하지 못한 것과 이유

| 항목 | 이유 |
|---|---|
| 읽기 확장 정적 경고·문법 카드·공용 SDK/product 문서 | R1 선언을 코드만으로는 실제 `ctx.data.read` 호출과 대조할 수 없어 전 확장 경고는 거짓 양성이 된다. 문법 카드와 공용 설명은 별도 골든·문서 변경이며 이번 범위에서 하지 않았다(`e2-extension-access.md` 3.3절) |
| 장기 호환성 약속, `schema` 필드, 비정의 코드 62종, BrokenPipe | 소유자 결정(`decisions/e.md`) |
| `--dev-checks` 경고(`predicate:`, `resource:`)의 `line`·`column` | 실행 facts에 위치가 없다. 위치를 싣는 일은 `sema.rs`의 facts 생성과 `spec`·digest에 닿으므로 하지 않았다 |
| `DEF.NON_LITERAL`, `DEF.H_SHAPE` 등 소비자 없는 포괄 코드 분리 | 소비자가 없는 코드 공간 확대는 의미 고정 비용만 늘린다(`diagnostics-fields.md` 7절) |
| `COMPATIBILITY.md` 진단 JSON 표(G7) | 소유자 승인 필요. 이번에 고치지 않았다 |
| E-05, E-06, E-07 | 범위 밖 또는 근거 부족(baseline 그대로) |

## 6. CI 경로

게시용 workflow는 수정하지 않았다.

- `cargo test --locked --manifest-path spikes/spike-v1-fixture/Cargo.toml`은 PostgreSQL 없이 전체 컴파일러 시험을 돈다. `tools/verify.sh`의 엔진 루프는 정상 CI에서 같은 크레이트를 실행한다.
- v4 표적 `read_extension_declared_reads`는 PG 없이 실제 Node·Python worker IPC와 DB 계획 전 거부를 확인한다. v4 전체 DB 시험은 아래 7절에 기록한 CI 경로에서 확인한다.
- F1은 `.github/workflows/flaskbb-post-order.yml:183`의 `d["code"] == "DEF.UNKNOWN_SYMBOL" and "expose apply" in d["message"]` 검사를 `d["code"] == "DEF.ACCESS_TARGET_NOT_EXPOSED"`로 바꾸고, 출력 sentinel도 새 코드를 반영해야 한다. workflow 소유자는 F1이므로 이 브랜치에서는 수정하지 않았다(D-E4).
- `plan-docs/stabilization-20261122/e2/scan_extensions.py`는 선택 도구다(필수 아님).

## 7. 통합 SHA와 소유권 인계

- 최종 main 기준: fetch 결과 `origin/main` = `0e3f0c80bc3b24a3b35b31e0655048fc13c30365`. 원래 G9 `c649603f`와 G1 `7e4684e8`을 이 기준 위로 자기 변경만 rebase해 각각 `4fbe131b`, `06e5f2d8`이 됐다.
- R1·표적 시험·E 결정·명세 변경을 포함한 **통합 SHA: `753be26fbc9e8f576169e7fce3ffbdfe5e3cb417`**. 이 브랜치가 원격에 push된 시점부터 `sema.rs`의 G9·G1·읽기 확장 구역은 D가 이어서 수정할 수 있다. 본 문서는 이 SHA를 기록한 별도 후속 문서 커밋이다.
- F1 연동 요청은 6절과 같다. `.github/workflows/flaskbb-post-order.yml`은 현재 line 183에서 메시지 부분 문자열과 `DEF.UNKNOWN_SYMBOL`을 함께 비교한다. G9 적용 후 해당 fixture의 기대 코드는 `DEF.ACCESS_TARGET_NOT_EXPOSED`; workflow 파일은 F1 소유라 건드리지 않았다.
- 로컬에서 확인한 DB 비사용 검사·빌드: `AIP_TEST_NO_DB=1 cargo test --locked --manifest-path spikes/spike-v1-fixture/Cargo.toml` 통과; v4 표적 시험 2개 통과; 양 크레이트 `cargo build --locked` 통과. v4 전체 시험은 DB URL compile-time 요구로 미실행. `.github/workflows/verify.yml` `core` job은 disposable PostgreSQL 17을 제공하고 `bash tools/verify.sh`에서 v1/v4 전체 엔진 시험을 실행한다. CI 결과는 이 문서 작성 시점에 미확인이다.
