# 오래된 원격 브랜치 정리 기록 (2026-11-21)

2026-10월부터 11월 초까지 만들어진 원격 브랜치 중 `origin/main`(0c5d108d)에 없는 수정·기능이 남았는지 하나씩 대조한 기록이다. 브랜치를 merge하지 않았고, 삭제하지도 않았다. main에 아직 없고 현재 구현보다 나쁘지 않은 것만 골라 옮겼다.

## 결론

- 반영: 1건. `spikes/spike-v6-transport/tests/audit_bd_idem_race.rs`(같은 멱등 키 12개 동시 요청 시험). 원본은 `audit/semantic-c-policy-20261116`의 b91f80dc.
- 퇴보라서 제외: 1건. `work/sprint-read-access-declared`의 900f8325(읽기 확장 `ctx.data.read`에 `read.<Resource>` 선언 강제).
- 보류: 1건. `codex/crypto-target-binding-red-20261107`. main에서 실제로 실패하는 반증 시험이 있으나 고치려면 rekey API 설계 변경이 필요하다.
- 그 밖의 브랜치: 이미 main에 들어 있거나(PR squash 포함) 현재 구현이 대체했거나 일회성 검증·스냅샷이다. 66개 전부 아래 표에 있다.
- main보다 앞선 커밋이 있는 원격 브랜치는 66개였다. 나머지 61개는 main에 완전히 포함돼 비교 대상이 아니다.

## 방법

1. 이 저장소의 `remote.origin.fetch`는 main만 받도록 되어 있어서 `git fetch origin '+refs/heads/*:refs/remotes/origin/*'`로 전체 브랜치를 받았다.
2. `git rev-list origin/main..<브랜치>`가 0이 아닌 브랜치 66개를 골랐다. `git cherry`는 PR squash와 cherry-pick을 같은 변경으로 보지 못해서 쓰지 않았다.
3. `git merge-tree --write-tree`로 병합 결과를 만들어 main과 트리가 같은지(`EMPTY`), 추가분만 남는지(`NET`), 충돌인지(`CONFLICT`)를 나눴다.
4. 충돌 브랜치는 판정을 자동으로 못 하므로 merge-base 이후 추가된 줄이 main의 같은 파일에 이미 있는지 세었다(표의 `main 포함률`). 포함률이 낮은 파일은 따로 열어 main의 구현과 비교했다.
5. 반영 후보는 main 위에 올려 시험을 돌리고, 코드를 일부러 망가뜨려 실패(RED)를 본 뒤 복원해 통과(GREEN)를 확인했다.

한계: 포함률은 줄 단위 일치라서 main이 같은 일을 다르게 구현했으면 낮게 나온다. 그런 경우는 기능 단위로 main의 구현을 찾아 근거에 적었다. 판정이 정적 대조(실행 없음)인 곳은 근거 칸에 그렇게 썼다.

## 판정 표

판정: 반영 / 이미 대체됨 / 퇴보 / 불필요 / 보류(결함 후보, 별도 수정 필요).

### 1. 지정 브랜치

| 브랜치 | main에 없는 변경 요약 | 판정 | 근거 |
|---|---|---|---|
| work/sprint728-emotion-summary | 마이페이지 감정 요약을 원본과 대조하는 Python oracle 시험(1239줄), `slice728_summary.py` 포팅, 당시 언어가 Mongo access를 못 선언하던 공백을 보이는 시험 | 이미 대체됨 | main은 쓰기 확장 `User.myEmotionSummary`로 구현했다(`docs/DECISIONS.md` D-P-EMOSUMMARY, `slice14e_mypage.py`, `facereview.aip`의 `access ... mongo.user_watch_stats`). 브랜치가 보이려던 언어 공백이 이미 닫혔다. 추가 1463줄 중 main에 같은 줄 0%(다른 구현) |
| work/wbs723-slice13c-python-frame | Slice13-C watch 포팅 4커밋과 Python worker 입력 프레임 한도 정렬(84fb1f5a) | 이미 대체됨 | 프레임 한도 수정은 main에 있다: `spikes/spike-v4-worker/workers/worker.py`의 `MAX_FRAME_BYTES = 1024 * 1024`, `StreamReader(limit=MAX_FRAME_BYTES - 1)`, `tests/python_frame_limit.py`가 브랜치와 같은 blob. watch 포팅은 main의 `slice13f_watch.watchDetail`, `slice13d_recommend.watchRecommended`, `slice14k_watch`가 대체 |
| work/wbs723-sprint13c-watch | 위와 같은 Slice13-C watch 포팅 4커밋(프레임 수정 제외) | 이미 대체됨 | 위 행과 같은 근거. 추가 1841줄이 main에 0%이지만 같은 화면을 main의 다른 확장이 구현 |
| work/sprint-uuid-guest-guard | 3커밋. 256a9013(Uuid actor가 자기 비공개 Mongo 문서 읽기), f4270062(guest가 `actor_uuid`로 소유권을 얻지 못하게 함), 900f8325(읽기 확장 `ctx.data.read`에 `read.<Resource>` 선언 강제) | 앞 둘은 이미 대체됨, 셋째는 퇴보 | 앞 둘은 main에 같은 내용이 있다: `capability.rs`의 `actor_uuid`·`private_user_id`·`resolve_actor_uuid`, `lib.rs`의 `invoke_with_host_as`, 시험 `uuid_guest_guard.rs`·`principal_uuid_mongo.rs`. 셋째가 퇴보인 이유는 아래 "퇴보 상세" |
| codex/crypto-target-binding-red-20261107 | rekey 대상 바인딩 반증 시험 5개와 시험용 `rekey_target_column_with_page_hook`(19줄) | 보류 | 아래 "보류 상세" |

### 2. 충돌로 자동 판정이 안 된 브랜치

| 브랜치 | main에 없는 변경 요약 | 판정 | 근거 |
|---|---|---|---|
| audit/semantic-a-data-20261116 | 감사 A 문서, 집계 null 가능성 재현 시험, 서비스 데이터 의미 행렬 시험 | 이미 대체됨 | `aggregate_empty_nullability.rs`는 40줄 중 39줄이 main에 있다. 서비스 행렬 시험은 main `crates/aip-cli/tests/service_flow.rs`에 69줄 중 60줄이 있다. 남은 180줄은 감사 문서(`plan-docs/reviews/2026-11-16-semantic-audit-a-data.md`)라 기록용 |
| audit/semantic-bd-tx-ext-20261116 | 감사 B+D 문서, 교착 재시도 수정, 멱등 경쟁 시험 | 반영(시험 1건) | 교착 재시도 수정(a700467d)은 main에 있다(`spike-v3-write/src/lib.rs`의 `retry_owned_transaction` 시험). `audit_bd_idem_race.rs`는 main에 없었고 통과·RED 확인 후 반영했다. 나머지는 감사 문서 |
| audit/semantic-c-policy-20261116 | 감사 C 정책 문서, PostgreSQL 정책 시험 CI 변경, 교착 재시도 수정, `audit_bd_idem_race.rs` | 반영(시험 1건) | 위와 같은 시험의 원본 커밋(b91f80dc)이 이 브랜치에 있다. `database-contracts.yml` 변경(194줄)은 main의 워크플로가 이후 재구성돼 대체됨. 서비스 시험은 main `service_flow.rs`에 203/210줄 포함 |
| audit/semantic-policy-service-pg-20261116 | 위 브랜치와 같은 커밋 11개 | 반영(시험 1건)·나머지 이미 대체됨 | 커밋 목록이 `audit/semantic-c-policy`의 부분집합(2ecb9371·6a4019e5·85371aa5 등 동일) |
| fix/otlp-status-code-standard | OTLP `SpanStatusCode`(UNSET=0, OK=1, ERROR=2) 도입과 시험 | 이미 대체됨 | main `spikes/spike-v6-transport/src/otlp.rs`에 `SpanStatusCode`, `from_http_status`, `from_job_state`가 있다. 시험 파일은 202줄 중 199줄 포함 |
| porting/flaskbb-full-2019 | FlaskBB 이식 34커밋(인증 nbf, 대화 발신자 바인딩, exists 범위, 메일 HTTP 통합 등) | 이미 대체됨 | 추가 59358줄 중 98%가 main에 있다. 확인한 수정: 대화 발신자·작성자 바인딩이 main `porting/flaskbb/app.aip` 642~646행과 716~719행에 같은 문구로 있다. `nbf` 선택 처리는 main `aip-auth`의 `present_integer`와 `jwt.rs`의 null nbf 거부 시험으로 있다. 남은 `aip-auth/tests/common/mod.rs`·`db_guard.rs`는 main의 `aip_test_support::test_database_url!`이 대체 |
| verify/crypto-429f377-20261106 | 독립 검증 시험(KEK 평문 노출 확인, 일부 대상만 rekey하면 DEK가 폐기되는 반례)과 워크플로 | 이미 대체됨 | main은 `kek_spec_debug_and_serialization_never_leak_key_material`로 노출을 막았고, 대상 목록 누락은 `metadata_inventory_*` 시험(`feat/crypto-inventory-20261107`은 main과 트리 동일)으로 막았다. 브랜치 시험은 옛 후보의 결함을 고정하던 것이라 지금은 맞지 않는다 |
| verify/flaskbb-orm-oracle-20261107 | ORM 순서 oracle 시험, 삭제 물리 cascade 계약 시험, CI 보정 | 이미 대체됨 | 추가 907줄 중 60%가 main에 있다. 남은 `post-order-delete-contract.test.mjs`를 main에 올려 `node --test`를 돌리면 `update Topic when this.topic.last_post = this` 단언이 실패한다(main이 다른 삭제 투영 설계로 바뀜: 881ae11a 등). 옛 설계를 고정하는 시험이라 반영하지 않았다 |
| verify/storage-list-scan-20261107 | MinIO 목록 스캔 PG 검증 시험 | 이미 대체됨 | `object_ledger_scan_minio.rs` 133줄 중 130줄이 main에 있다(96%) |
| work/sprint-mongo-principal-uuid | 256a9013 하나 | 이미 대체됨 | 추가 320줄이 100% main에 있다(sprint-uuid-guest-guard 행 참조) |
| work/sprint-read-access-declared | 256a9013와 900f8325 | 앞 커밋은 이미 대체됨, 900f8325는 퇴보 | 아래 "퇴보 상세" |
| work/wbs723-slice13a-mysql-worker-bridge | Slice13-A MySQL 관계형 worker bridge 1커밋 | 이미 대체됨 | main의 `crates/aip-service/src/mysql_service.rs`, `spike-v4-worker/src/host.rs`가 이 기능의 후속이다. 추가 1005줄 중 82% 포함, 남은 줄은 이후 리팩터링으로 바뀐 `capability.rs`(75줄) 등 |
| work/wbs723-sprint13d-home | Slice13-D 홈 추천 확장·골든·HTTP 시험 10커밋, admin이 User 행 여러 개를 볼 때 본인 식별 수정(ab2d60d2) | 이미 대체됨 | main은 `slice13d_recommend.py`·`recommendation_math.py`·`recommendation_pool_builder.py`로 재구현했고 `homePersonalized`는 User 행을 읽지 않으므로 ab2d60d2의 문제(admin이 남의 id를 본인으로 착각)가 없다. Mongo 접근은 principal이 `user_id`를 강제한다 |
| work/wbs723-sprint13d-pool-builder | 추천 풀 빌더와 위 홈 확장 일부 8커밋 | 이미 대체됨 | `recommendation_pool_builder.py`가 main에 있다. 나머지는 위 행과 같다 |

### 3. 그 밖에 main보다 앞선 커밋이 있는 브랜치 (2026-10-17 ~ 10-19)

`archive/tmp-aip-*`는 정리 시점(10-19)에 미커밋 작업을 `wip: preserve uncommitted work`로 보존한 스냅샷이거나 work 브랜치의 복사본이다.

| 브랜치 | main에 없는 변경 요약 | 판정 | 근거 |
|---|---|---|---|
| archive/tmp-aip-auth-verify-20261017 | FlaskBB 인증 포팅 11커밋(nbf 허용) | 이미 대체됨 | 추가 53651줄 중 99% main 포함. nbf는 main `aip-auth` 구현 |
| archive/tmp-aip-nbf-before-verify-20261017 | 위와 같은 11커밋 | 이미 대체됨 | 위와 같음 |
| archive/tmp-aip-nbf-after-verify-20261017 | 위 + null nbf 거부(86b30754) 16커밋 | 이미 대체됨 | main `Claims.nbf`가 `present_integer`로 null을 거부하고, `jwt.rs`가 `null_nbf`를 `is_err()`로 검증 |
| archive/tmp-aip-pool13d-20261018 | 추천 풀 Slice13-D 8커밋 | 이미 대체됨 | work/wbs723-sprint13d-pool-builder와 같다 |
| archive/tmp-aip-python-frame-fix | Slice13-C + 프레임 한도 | 이미 대체됨 | work/wbs723-slice13c-python-frame과 같다 |
| archive/tmp-aip-sprint-arm-20261018 | ARM 리눅스 hosting 1커밋 | 이미 대체됨 | `merge-tree` 결과가 main과 트리 동일(EMPTY) |
| archive/tmp-aip-sprint-principal-uuid-20261018 | Uuid principal 3커밋 | 퇴보(900f8325 포함) | work/sprint-uuid-guest-guard와 같다 |
| archive/tmp-aip-sprint13c-20261018 | Slice13-C watch | 이미 대체됨 | work/wbs723-sprint13c-watch와 같다 |
| archive/tmp-aip-sprint13d-20261018 | Slice13-D 홈 10커밋 | 이미 대체됨 | work/wbs723-sprint13d-home과 같다 |
| archive/tmp-aip-sprint728-summary-20261018 | 감정 요약 oracle 2커밋 | 이미 대체됨 | work/sprint728-emotion-summary와 같다 |
| porting/flaskbb-full-2019 | (표 2 참조) | 이미 대체됨 | 표 2 참조 |
| archive/tmp-aip-flaskbb-independent-20261018 | porting/flaskbb-full-2019 + 1커밋 | 이미 대체됨 | 98% main 포함 |
| work/sprint-arm-linux-hosting | ARM 리눅스 hosting 1커밋 | 이미 대체됨 | `merge-tree` 결과가 main과 트리 동일 |
| archive/tmp-aip-diagnose-cbaa401 | worker 런타임 점검 실패 시 stderr를 3개 분류(`permission_or_sandbox_denial` 등)로 줄여 시작 오류에 싣는 WIP | 불필요 | 시험이 없는 WIP다. main `server_start_error`는 io 오류를 종류만 보이게 하는 것을 의도했다(주석: 런타임 출력에 비밀이 있을 수 있음). 분류 문구는 진단 편의일 뿐 결함 수정이 아니다 |
| archive/tmp-aip-flash-verify-4e8e212 | 일회성 검증 시험(`AIP_FLASH_VERIFY_DATABASE_URL` 필요) | 불필요 | 특정 리비전 검토용 임시 시험 |
| archive/tmp-aip-mysql-driver-verify-8a27 | MySQL fenced transaction을 `Transaction`으로 바꾸고 bound table 집합 일치를 검사 + 펜스 검증 시험 | 이미 대체됨 | main `mysql_driver.rs`에 `start_transaction`, `canonical_bound_tables` 비교(314행), `FencedMysqlTransaction { tx, read_only }`가 있고 `tests/mysql_fence_verification.rs`가 main에 있으며 CI(`database-contracts.yml` 108~109행)가 돌린다 |
| archive/tmp-aip-slice13a-codex-verify-v2 | MySQL worker action 검증 스크립트·시험 | 이미 대체됨 | 추가 71줄 중 92% 포함 |
| archive/tmp-aip-slice13a-mget-repro | Slice13-A 브릿지 + mget 재현 | 이미 대체됨 | work/wbs723-slice13a-mysql-worker-bridge와 같다 |
| archive/tmp-aip-slice13a-mysql-operation | `test_agent_write.txt` 한 줄 | 불필요 | 임시 파일 |
| archive/tmp-aip-slice13a-verification-20261018 | transition 접두사·와일드카드 grant 확장 거부 시험 1개 | 이미 대체됨 | main `crates/aip-service/tests/mysql_write_http.rs` 438행·592행이 `Video.incremen`, `Video.incrementX`, `Video.increment*`, `Video.*`를 같은 방식으로 거부하는지 시험 |
| archive/tmp-aip-slice13a-verify-p2 | 검증 스크립트 | 이미 대체됨 | 추가 70줄 중 91% 포함 |
| archive/tmp-aip-v1-qwen-links-20261017 | 문서 링크 7줄과 `test_write.txt` | 불필요 | 임시 파일과 문서 보정 |
| archive/tmp-aip-wbs-8-1-concurrency | `absence_serializable_http.rs`의 멱등 키 집합 불변식 보조 함수와 음성 대조 | 불필요 | 기존 시험 파일의 보강 WIP다. 같은 시험 파일이 main에서 별도로 발전했고(110줄 중 82줄 포함), 동시성 시험은 다른 작업자가 main에서 올리는 중이라 겹친다. 핵심 경쟁 시험은 위 `audit_bd_idem_race`로 보강했다 |
| archive/tmp-aip-wbs82-83-recovery-20261017 | `scratch_api_outage.rs`(225줄) | 불필요 | 이름대로 임시 시나리오 스크래치다. main에 같은 줄 0%이고 이를 정식 시험으로 만든 근거 문서가 없다 |
| archive/tmp-aip-wbs84-sdk-python | Python 생성 코드의 `cast` 추가, 미지의 enum 패닉 시험 | 이미 대체됨 | main `spike-v5-sdk/src/python.rs` 54행에 `unknown enum` 패닉이 있다. 생성 코드 형식은 main에서 이후 바뀌었다 |
| archive/tmp-aip-wbs86-verify | 문서 7줄 | 불필요 | 문서 보정 |

### 4. 2026-11-06 이후 브랜치

| 브랜치 | main에 없는 변경 요약 | 판정 | 근거 |
|---|---|---|---|
| feat/crypto-inventory-20261107 | 암호화 인벤토리 12커밋 | 이미 대체됨 | `merge-tree` 결과가 main과 트리 동일(EMPTY) |
| ci/release-gate-dispatch-20261117 | 릴리스 게이트 dispatch 1커밋 | 이미 대체됨 | 추가 3줄 모두 main에 있음 |
| cycle/p0-conformance-20261117 | 의미 적합성 9커밋 | 이미 대체됨 | 추가 334줄 중 96%. 남은 10줄은 `core-service-semantic-matrix.md`의 문서 문구 |
| cycle/p0-conformance-20261117-rebased | 위의 rebase 12커밋 | 이미 대체됨 | 342줄 모두 main에 있음 |
| cycle/p0-policy-noninterference-20261117 | 정책 비간섭 14커밋 | 이미 대체됨 | 99%. 남은 1줄은 `product/SDK.md` |
| cycle/p1-flaskbb-g6-20261117 | FlaskBB G6 10커밋 | 이미 대체됨 | 98%, 남은 줄은 STATUS·GAPS 문구와 이전 이관 스크립트 4줄 |
| cycle/p1-sdk-e2e-20261117 | SDK 의미 E2E 7커밋 | 이미 대체됨 | 97% |
| cycle/security-rustix-20261117 | rustix 보안 갱신 | 이미 대체됨 | `merge-tree` 결과 EMPTY |
| cycle/worker-probe-20261117 | worker probe 10커밋 | 이미 대체됨 | 96%. 남은 줄은 `isolation.rs` 15줄 |
| docs/1.0-cycle-report-20261117 | 주기 보고서 | 이미 대체됨 | EMPTY |
| fix/worker-probe-budget-20261117 | probe 예산 수정 | 이미 대체됨 | EMPTY |
| test/sdk-schema-migration-20261117 | SDK 스키마 이관 시험 | 이미 대체됨 | EMPTY |
| verify/post-unhide-flaskbb-20261117 | FlaskBB unhide 검증 6커밋 | 이미 대체됨 | 99% |
| verify/topic-move-flaskbb-20261117 | FlaskBB topic move 검증 29커밋 | 이미 대체됨 | 99% |
| ci/1.0-required-checks-20261121 | 1.0 필수 검사 CI | 이미 대체됨 | 99%. 남은 3줄은 CHANGELOG |
| docs/1.0-contract-20261121 | 1.0 지원 범위·호환 계약 문서 | 이미 대체됨 | main에 같은 제목의 PR #23 squash(4e132564)가 있고 이후 문서가 더 갱신됐다 |
| feat/expr-p0c-ext-create-20261121 | 확장 create 1커밋 | 이미 대체됨 | EMPTY |
| fix/1.0-extension-sdk-dx-20261121 | 확장 SDK DX | 이미 대체됨 | 99% |
| release/1.0-dist-plan-20261121 | 배포 계획 | 이미 대체됨 | EMPTY |
| release/1.0-packaging-20261121 | 패키징 | 이미 대체됨 | 99% |
| release/1.0-packaging-followup-20261121 | 패키징 후속 | 이미 대체됨 | 99% |
| release/rc-dryrun-20261121 | RC 리허설 | 이미 대체됨 | 98%. 남은 줄은 CHANGELOG·검토 문서 |
| (표 2의) verify/crypto-429f377-20261106, codex/crypto-target-binding-red-20261107, verify/flaskbb-orm-oracle-20261107, verify/storage-list-scan-20261107, audit/semantic-* 4개 | | | 표 2 참조 |

## 반영: audit_bd_idem_race.rs

- 파일: `spikes/spike-v6-transport/tests/audit_bd_idem_race.rs`. 원본 커밋은 `audit/semantic-c-policy-20261116`의 b91f80dc이고 파일 내용은 바꾸지 않았다.
- 내용: 같은 멱등 키를 12개 연결에서 동시에 `/apply`로 보낸다. 정확히 1개만 실행하고 11개는 저장된 결과를 재생해야 한다. READ COMMITTED(`Counter.bump`)와 SERIALIZABLE(`Booking.confirm`) 두 경로를 모두 시험한다.
- main에 없던 이유: 기존 `commit_unknown_wire`와 `v14_apply`는 같은 키를 한 번에 하나씩만 재생했고, 동시 경쟁은 다루지 않았다. `grep`으로 main의 spike-v6 시험에서 같은 키 동시 경쟁 시험을 찾지 못했다.
- 시험 등록: `tools/verify.sh`가 `spike-v6-transport`의 모든 `tests/*.rs`를 돌리므로(81~82행) 워크플로 변경은 필요 없다. `cargo clippy ... --test audit_bd_idem_race -D warnings`와 `rustfmt --edition 2021 --check`가 통과한다.
- RED: `spikes/spike-v6-transport/src/lib.rs`의 `pg_advisory_xact_lock(hashtext(...))`를 no-op 쿼리로 바꾸면 `assertion left == right failed: read committed ... left: (1, 0) right: (1, 11)`로 실패한다(실행 1, 재생 0, 나머지는 `WIRE.INTERNAL`).
- GREEN: 원래 코드로 되돌리면 `test twelve_concurrent_requests_with_one_key_execute_once_and_replay_eleven_times ... ok`.

## 퇴보 상세: 읽기 확장의 `read.<Resource>` 선언 강제 (900f8325)

900f8325는 읽기 확장의 `ctx.data.read`를 확장 계약에 `read.<Resource>`가 있을 때만 허용한다. main에서는 다음 때문에 이 검사를 넣으면 실제 앱이 깨진다(정적 대조, 실행 안 함).

- 현재 언어는 읽기 확장에서 `read.<Resource>`를 선언할 수 없다. `spikes/spike-v1-fixture/src/sema.rs`의 읽기 확장 access 처리는 `mongo`·`redis`가 아니면 `Resource.aggregate`만 찾아서 `access read.Video`는 `DEF.UNKNOWN_SYMBOL`이 된다. 브랜치 자신의 시험 머리말도 "현재 언어는 읽기 확장에서 `read.<Resource>`를 선언할 수 없으므로 수동 facts의 표기만 허용"이라고 적었다.
- main의 FaceReview 읽기 확장은 선언 없이 `ctx.data.read`를 부른다. `extension read homeSearch`(access는 `mongo.video_distribution`뿐)는 `slice13b_lists.py` 36행 `_read_rows`로, `extension read myRecentVideos`는 `slice14e_mypage.py` 66행 `_videos`로 읽는다. 검사가 들어가면 이 확장들이 `WIRE.ACCESS_NOT_DECLARED`로 실패한다.

따라서 이 검사를 넣으려면 읽기 확장에 `read.<Resource>` 선언 문법을 먼저 더해야 한다. 이건 범위 밖이라 하지 않았다. 읽기 확장의 `ctx.data.read`가 무선언인 지금 상태는 행·필드 정책이 호출자 기준으로 적용되는 한 권한 확대는 아니지만, 설계상 선언 없는 접근이라는 점을 남긴다.

## 보류 상세: codex/crypto-target-binding-red-20261107

브랜치는 rekey 대상 바인딩 반증 시험 5개와, 시험이 쓰는 `rekey_target_column_with_page_hook`(페이지 선택 직후 훅, 운영 진입점은 no-op)을 더한다. main 위에 올려 `AIP_TEST_DATABASE_URL`(PostgreSQL 17)로 돌렸다.

- 브랜치 그대로는 컴파일되지 않는다. `std::sync::Barrier`의 `wait()`를 `tokio::select!`와 `.await`로 쓰기 때문이다(`E0277: BarrierWaitResult is not a future`, 6건). `tokio::sync::Barrier`로 바꾸면 컴파일된다.
- 컴파일 후 결과:
  - `rekey_schema_shadow_changes_only_the_intended_schema` 실패: `["intended frame not on active DEK", "decoy frame bytes changed"]; intended_frame_changed=false; decoy_frame_changed=true`. `search_path` 앞에 같은 이름 테이블이 있는 다른 스키마가 있으면 rekey가 그쪽을 바꾼다. `rekey.rs`가 대상 테이블을 `quote_ident(&target.table)`로만 쓰고 배포 스키마로 한정하지 않는다.
  - `rekey_ref_scope_ignores_other_refs_and_preserves_their_frame_bytes` 실패: `["report includes rows outside Team 42"]; scanned=12 failed_count=1`. ref 범위 rekey가 범위 밖 행까지 스캔하고 실패로 센다. API에 ref 열 지정이 없다(`rekey_existing_api_accepts_ref_scope_without_a_ref_column_selector` 시험 이름이 이를 짚는다).
  - `rekey_ref_change_after_select_is_rejected_by_cas_scope`는 `cargo test` 전체 실행이 600초 안에 끝나지 않았다. 원인은 확인하지 못했다(PG에서 rekey 연결이 클라이언트 대기 상태였다). 따로 실행하는 확인은 하지 않았다.
  - `rekey_checkpoint_identity_does_not_bind_deployment_schema`, `rekey_existing_api_accepts_ref_scope_without_a_ref_column_selector`는 통과.
- 판정 이유: 시험 2개가 main에서 실패하므로 시험만 가져오면 CI가 빨개진다. 고치려면 스키마 한정과 ref 열 선택 API가 필요해 선별 반영이 아니라 설계 변경이다. 반영하지 않고 worktree 변경은 되돌렸다.
- 후속: 별도 작업으로 rekey 대상 테이블을 배포 스키마로 한정하고 ref 범위 선택자를 추가한 뒤 이 시험을 가져온다. 운영 경로에서 `search_path`가 어떻게 고정되는지(`aip_runtime::rekey`)는 확인하지 못했다. 확인 필요.

## 범위 밖에서 본 것

- 이 저장소의 `remote.origin.fetch`가 main만 받도록 설정돼 있어, 평소 `git fetch`로는 다른 브랜치가 갱신되지 않는다. 이번에 설정은 바꾸지 않았다.
- 위 "퇴보 상세"의 읽기 확장 무선언 `ctx.data.read` 문제.
- 위 "보류 상세"의 rekey 반증 결과 2건.
