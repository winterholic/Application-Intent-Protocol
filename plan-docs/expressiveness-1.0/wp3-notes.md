# WP3 (P0-C 확장 create) 작업 중 알게 된 것

- **분기는 `run_write_in` 한 곳**: `ctx_create_in`이 `job_create_request` → `spike_v3_write::bundle_in` → `{"created":[{resource,id}]}`. 같은 파일의 `invoke_write_action`(MySQL·job 경로)에는 create가 이미 있었고 PG 경로(`run_write_in`)만 `ctx.create.unsupported`였다(E7).
- **`write_needs_serializable`**: access 키를 `Resource.transition`로만 읽던 것을 `create.<R>`이면 `resources[R].exposeCreate.isolation`을 보게 했다. `create`라는 이름의 resource는 없으므로 기존 전이 판정과 섞이지 않는다.
- **검사 순서**: v3 `bundle_run`은 `ensure_refs_readable`을 `create_row`의 allow 평가보다 먼저 한다. 읽을 수 없는 참조(남의 Member)를 쓰면 allow가 거짓이어도 `MISSING_TARGET`이 먼저 나온다. allow 거절을 시험하려면 참조는 읽을 수 있고(actor 본인) allow만 거짓인 입력이 필요하다(시험은 `actor.org > 0`인 org 0 멤버).
- **ALREADY_EXISTS·CONFLICT는 통과시킨다(리뷰 C-2)**: `run_write_in`도 host 경로(`invoke_write_action`)와 같은 `passed_on_refusal`을 쓴다. 첫 거절된 ctx 호출이 `WIRE.CONFLICT`(expect 불일치, 40001, 40P01) 또는 `WIRE.ALREADY_EXISTS`(유일 제약)이고 확장이 그 코드 그대로 끝나면 호출은 그 코드로 끝난다(새 WIRE 코드 없음). 로그의 `cause` 태그는 그대로 `ctx.create.conflict` 등이다. 확장이 거절을 삼키고 정상 반환하면 이전처럼 `EXTENSION_ERROR`와 원인 태그다. 확장 경로에는 서버 재실행이 없다: 40001은 `WIRE.CONFLICT`(retry later)로 나가고 호출자가 같은 key로 다시 보낸다. **결정표 C0-6 "서버가 최대 3회 재실행"은 SQL 전용 경로 얘기로 정정해야 한다**(확장 경로는 재실행 없이 `WIRE.CONFLICT`, 같은 key 재전송). `wire-errors.md`의 `WIRE.CONFLICT` 원인 문구("서버가 이미 3회 재시도했다")도 이 경로에는 맞지 않는다(문서 수정은 WP5).
- **create 실패 뒤 계속은 안 된다**: 확장 트랜잭션에 savepoint가 없어 `create_row`의 DB 오류(유일 제약 등) 뒤 트랜잭션은 aborted 상태이고 이어지는 ctx 호출은 25P02(`INTERNAL`)다. 첫 원인은 보존되어 진단은 맞다. "create 실패를 잡고 계속"하는 확장은 PG 경로에서 성립하지 않는다(문서에 적을 것, C-F4와 일치).
- **key 형식(리뷰 C-1)**: `ctx_create_in`이 `job_create_request` 뒤에 `check_ctx_key`를 부른다(`job_create_request`는 job 경로와 공유라 건드리지 않았다). 시험은 `tests/expr_ext_create.rs`의 `badKey` 케이스(`ctx.create.key_format`)이고, 검사를 빼면 `badKey: ... got Ok(...)`로 RED가 난다.
- **리뷰 C-3(참고, 영향: 문서만)**: job은 key를 장부에 남겨 재생하고 PG 확장은 검사 후 버린다. 같은 호출 안에서 같은 key로 두 번 create하면 두 행이다. 코드는 바꾸지 않았다. 확장 ctx 문서(WP5 MANAGED-EXECUTION)에 "같은 호출 안 같은 key는 중복 행"을 적어야 한다.
- **리뷰 C-4(참고, 영향: 1.0 차단 아님)**: `run_write_in`의 knobs에 `seal`이 없어 암호화 필드가 있는 resource에 `create.<R>`를 열면 평문이 들어갈 수 있다. 1.0이 암호화를 지원하지 않아 코드는 바꾸지 않았다. `ctx.data.read`의 `declares_encrypted` 거절처럼 create도 거절하는 한 줄은 후속 후보다.
- **시험 구조**: `sqlgen::set_schema`는 프로세스당 한 번이라 한 test binary 안의 `#[tokio::test]`가 둘 이상이면 스키마가 충돌한다. `expr_ext_create.rs`는 시나리오 함수 넷을 한 테스트가 순서대로 부른다.
- **바깥 key 재생(C0-4)은 v4 시험으로 못 본다**: 재생은 v6 `apply_attempt`에 있고 v6가 v4에 의존한다. serve 실행으로 확인했다(아래).
- **serve 재현 절차**(임시 앱): `/apply` 확장 호출은 `x-aip-contract: <listening 줄의 fingerprint>` 헤더가 필요하다(없으면 `WIRE.CONTRACT_MISMATCH`). 접근 토큰 수명은 800초로 냈을 때 통과했고 3000초는 `WIRE.UNAUTHENTICATED`였다(상한은 코드로 확인하지 않음, 확인 필요). `principal bind`는 actor 행이 먼저 있어야 한다(`PRINCIPAL_FAILED`). JWKS를 다시 만들면 serve를 다시 띄워야 한다.
- **미반영(범위 밖)**: `ctx_request`의 `input` 키 전달은 WP1 병합 뒤 일이다. v3 쪽 수정은 필요하지 않았다.

## 3차: v6 확인, C-4, worker ctx 타입

- **v6 확장·job PG 시험(리뷰 C-2 반영 revision `9907d1c9`)**: `commit_unknown_wire`(13), `durable_job_commit_loss`(3), `extension_unix`(1), `job_data`(1), `job_retry`(1), `write_extension_atomic`(1), `write_extension_commit_classification`(12), `write_extension_dx_http`(3), `write_extension_lifetime`(1), `write_extensions_http`(2), `write_isolation`(1), `write_session_outcome`(1): 12개 바이너리, 합계 40 passed, 0 failed, 0 ignored. host 경로(`invoke_write_action`)를 `passed_on_refusal`로 바꾼 것이 job 시험(`job_data`, `job_retry`, `durable_job_commit_loss`)을 깨지 않았다.
- **C-4 암호화 resource**: 이미 안전하지 않았다. 가드를 빼고 시험하면(`secret` 확장, `Secret.value`가 `encrypted(scope: service)`) `ctx.data.create`가 성공해 `secret` 테이블에 평문 행이 생겼다(`left rows behind [0, 0, 0, 0, 0, 1]`). `run_write_in` knobs에 `seal`이 없기 때문이다. `ctx_create_in`이 암호화 필드가 있는 resource를 `WIRE.UNSUPPORTED`(`ctx.create.unsupported`)로 거절한다(`ctx.data.read`의 `declares_encrypted`와 같은 방침). 제품 경로는 암호화 facts에 `DEF.ENCRYPTION_RUNTIME_UNAVAILABLE`로 serve 자체가 안 뜨므로(v3 `encrypted_opened.rs`) 이 거절은 라이브러리 층 방어다.
- **ctx 타입(수동 공통 타입)**: `spikes/spike-v4-worker/workers/ctx.d.ts`(Node, JSDoc `import("./ctx.js").Ctx`로 쓴다)와 `workers/aip_ctx.py`(typing Protocol, `TYPE_CHECKING` 아래에서 import)다. grant별 narrowing은 하지 않는다(SDK 생성기 소유). worker.mjs·worker.py는 고치지 않았다.
  - `apply`의 `input`(P0-B)은 d.ts에만 마지막 선택 인자로 있다. worker.mjs가 아직 전달하지 않으므로 지금 넘기면 조용히 버려진다(서버는 그 전이가 요구하는 값이 없어 거절한다). WP1 병합 때 worker.mjs·worker.py에 전달을 넣고 Python Protocol에도 같은 인자를 더해야 한다. Python은 런타임에 인자가 없어 선언하지 않았다(넘기면 TypeError).
  - `ids`는 `str` 하나를 mypy가 `Sequence[str]`로 받아 worker.py가 글자로 쪼개는 함정이 있어 Protocol은 `list[...]`로 좁혔다. 섞인 리터럴 `[10, "11"]`은 mypy가 `list[object]`로 추론해 거절하므로 변수에 `list[str | int]`를 붙여야 한다(시험 fixture가 그렇게 쓴다).
  - id는 wire에 따라 string(`decimal`, `uuid`) 또는 number(legacy)라 `EntityId = string | number`다.
- **시험** `tests/worker_ctx_types.rs`(3개): ① `tsc --checkJs`로 `ctx-types/typed.mjs`와 `expr-ext-create/ex.mjs`가 통과하고 `neg.mjs`가 `// ERR` 표시 줄에서만 실패. ② `mypy --strict`로 `typed.py` 통과, `neg.py`가 `# ERR` 줄에서만 실패(`AIP_MYPY`가 없으면 "미실행"을 출력하고 건너뜀). ③ 같은 typed fixture를 진짜 `worker.mjs`·`worker.py`에서 돌려 보낸 call과 돌려받은 값이 선언과 같음을 본다. 도구는 저장소에 없어 `AIP_TSC`(기본 `../spike-0-ts/node_modules/.bin/tsc`)와 `AIP_MYPY`로 지정한다. 이 세션은 `npm install typescript@7.0.2`를 `/tmp/wp3-ts`에, `pip install mypy`를 `/tmp/wp3-venv`에 따로 받아 썼다(원래 checkout의 `spike-0-ts` tsc는 `@typescript/typescript-darwin-arm64` 플랫폼 패키지가 없어 이 머신에서 실행되지 않았다).
- **패키지에 넣으려면 고칠 파일**(이번 범위 밖, 수정하지 않음): 워커 스크립트는 `spikes/spike-v4-worker/src/lib.rs`의 `include_str!`로 바이너리에 들어가므로 타입 파일은 어디에도 배포되지 않는다. Node용은 `product/tools/build-sdk.mjs`(npm 패키지 `files`·`exports`에 `ctx.d.ts` 추가, 예: `@aip/sdk/worker-ctx`)와 `product/tools/package.mjs`(소스 목록·번들 배치), Python용은 `product/python/aip_sdk/`에 `worker_ctx.py`로 복사하는 것(`__init__.py`는 수정하지 않아도 되고 `py.typed`는 이미 있음). 소스 번들 목록은 `prototype/tools/build-bundle.py`(39~40행 `workers/worker.*`)에 두 파일을 더한다. 문서는 `product/MANAGED-EXECUTION.md`(WP5).

## 4차: 검증 관찰 O-3(WORKER_BUSY)·O-4(stale 바이너리) 조사

기준: P0-C 독립 검증 `verify-p0c.md`의 O-3, O-4. 이 절은 조사 기록이고 코드는 고치지 않았다(v6 transport는 다른 작업이 고친다).

### O-3 같은 key 동시 8건 중 6건 `WIRE.WORKER_BUSY`

**판정: 의도한 backpressure(기본 한도)다. P0-C가 만든 것도 같은 key 전용 결함도 아니다. 다만 "worker가 필요 없는 요청이 worker 슬롯을 쓴다"는 경미한 설계 개선점이 있다.**

- 원인 한도: `ResourceLimits::default()`의 `workers: 4`, `per_principal_workers: 2`(`spikes/spike-v6-transport/src/infrastructure.rs:28`, `:33`). 확장 쓰기(`/apply`의 `extension`)는 `lib.rs:1691-1698`에서 전역 슬롯을, `lib.rs:1699-1706`에서 principal 슬롯을 `try_acquire`하고 못 얻으면 즉시 `WORKER_BUSY`(`wire_codes.rs:230`: retry later, "Send again after a short delay")를 돌려준다. 같은 주체의 8건 중 2건만 들어가므로 6건이 거절된다.
- 같은 key 요청이 슬롯을 먼저 잡는 순서: 슬롯 획득(`lib.rs:1691-1706`) → `write_extension::apply_with_launch`(`lib.rs:1967`) → 그 안에서 `aip_idem` claim INSERT(`write_extension.rs` `ON CONFLICT (principal,key) DO NOTHING`)가 같은 key의 소유를 정한다 → 이긴 쪽만 worker를 시작한다. 진 쪽은 이긴 쪽 트랜잭션이 끝날 때까지 유일 인덱스에서 기다린 뒤 저장 결과를 재생한다(`replayed`). 그래서 슬롯 2개는 "실행 1건 + 기다리다 재생하는 1건"이 차지한다. 확장 실행은 한 번뿐이고(안전성 유지) 재생도 정상이다.
- 재현(`/tmp/wp3-o3/run.mjs`, 800ms 걸리는 확장이 `Hit.n`을 1 올린다. 같은 PG 17, 같은 시나리오):

| 시나리오 | 4e132564 | P0-C 이후 `3a6edf5f` | 한도를 8로 올린 `3a6edf5f` |
| --- | --- | --- | --- |
| S1 같은 key 8건 | OK 1, replayed 1, BUSY 6, n=1 | 같음 | OK 1, replayed 7, n=1 |
| S2 서로 다른 key 8건 | OK 2, BUSY 6, n=2 | 같음 | OK 4, DEADLINE_EXCEEDED 4(같은 행 잠금 대기, 별개 현상) |
| S3 슬롯 2개가 느린 호출로 찬 동안 이미 저장된 key 재전송 | **WORKER_BUSY**(다른 주체는 OK) | 같음 | replayed |
| S4 단독 재전송 | replayed, n=1 | 같음 | 같음 |

  한도를 `limits: {workers: 8, per_principal_workers: 8}`로 올리면 S1이 `OK 1 + replayed 7`로 바뀌므로 원인은 한도다. 서로 다른 key(S2)도 같은 6건이 거절되므로 key 때문이 아니다. 4e132564와 결과가 같아 P0-C 이전부터의 동작이다(이번 변경은 이 경로를 건드리지 않았다).
- 개선점(결함이라 부를 정도는 아님, 가용성·공정성): S3처럼 **이미 저장된 결과를 재생하는 요청은 worker가 필요 없는데도 슬롯이 차 있으면 `WORKER_BUSY`가 된다.** 클라이언트가 재시도하는 것이 멱등 key의 용도인데, 재시도가 같은 주체의 슬롯을 차지해 그 주체의 새 요청을 밀어낸다. `WORKER_BUSY`는 retry later라 계약 위반은 아니다.
- 수정안(v6 담당자용): 슬롯을 "worker를 시작하기 직전"으로 옮긴다. (a) `apply_with_launch`가 claim 성공(`Ok(Some(_)) => break tx`) 직후, `Worker::try_start_configured` 전에 슬롯을 얻게 한다(슬롯 두 개를 인자로 넘기거나 콜백). 재생·IDEMPOTENCY_MISMATCH·claim 대기는 슬롯 없이 끝난다. 단 이긴 쪽이 슬롯이 없어 BUSY로 끝나면 claim 행을 남기지 말고 롤백해야 한다(지금도 BUSY는 claim 전이라 행이 없다). (b) 더 작은 수정은 슬롯 획득 전에 `SELECT result FROM aip_idem WHERE principal=$1 AND key=$2`로 완료된 결과가 있는지 읽어 있으면 슬롯 없이 재생하는 것이다(진행 중인 같은 key는 기존대로 슬롯을 쓴다).
- 영향 범위: `lib.rs:1691-1706`(PG 경로), 같은 순서를 가진 관계형 경로(`run_relational_write_extension`, `lib.rs:813-880`: `begin_worker_write`가 worker 시작 전이고 슬롯은 그 앞 `lib.rs:1691`에서 이미 잡힌다), 세션 쓰기(`lib.rs:988-989`, `:1551-1560` RAG는 해당 없음). BUSY를 기대하는 시험은 과부하를 만들 때 서로 다른 key를 쓰는지 확인해야 한다: `write_extension_lifetime.rs:90`, `extension_lifetime.rs:373`, `db_query_lifetime.rs`, `request_lifetime.rs:141`. (a)안은 claim 대기 중인 요청이 DB 연결을 쥐고 있으므로 `connections: 64` 한도가 새 상한이 된다. 이 항목들은 읽어서 적은 것이고 시험을 돌려 확인하지 않았다.

### O-4 `cargo build`가 `Fresh`로 끝났는데 바이너리가 이전 소스

**재현됨(원인은 mtime 기반 fresh 판정).** 한 줄: cargo는 소스 파일의 **mtime**이 직전 빌드의 기준 시각보다 오래되면 내용이 달라도 `Fresh`로 본다. 그래서 새 소스를 mtime을 보존하는 방법(`cp -a`, `rsync -a`, 아카이브 풀기)으로 같은 경로·같은 `CARGO_TARGET_DIR`에 덮어쓰면 이전 소스로 만든 바이너리가 그대로 남는다. `git checkout`은 파일 mtime을 지금으로 쓰므로 이 문제가 없다.
- 재현: `write.rs`의 문자열에 `O4MARK`를 넣고 빌드 → 바이너리에 `O4MARK` 1개. `git checkout -- write.rs` 뒤 `touch -t 202001010000 write.rs` → `cargo build -p aip-cli --bin aip`가 `Finished ... 0.14s`(재컴파일 없음), 바이너리에 `O4MARK`가 그대로(stale). `touch write.rs`(지금 시각)로 다시 빌드하면 2.08s에 `O4MARK` 0개로 돌아왔다(작업 트리 clean).
- 검증 쪽 추정(확인 못함): `/tmp/aip-verify-p0c-src` 복사본을 mtime을 보존해 만들었거나 공유 target에서 같은 경로를 다른 시점의 소스로 덮었을 가능성이 크다. worktree checkout 시각만으로는 설명되지 않는다(git은 mtime을 지금으로 쓴다).
- 예방: 복사본을 쓸 때는 `touch`로 mtime을 지금으로 갱신하거나(`find . -name '*.rs' -exec touch {} +`), 소스 경로마다 별도 `CARGO_TARGET_DIR`을 쓴다. 바이너리에 P0-C 문자열이 있는지(`grep -c -a "ctx는 apply·create·read만"`)를 먼저 보는 검증 절차가 맞다.
