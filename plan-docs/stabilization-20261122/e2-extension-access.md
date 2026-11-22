# 확장의 권한 선언과 실제 `ctx.data.*` 접근 (Track E2)

- 기준: `stab/e-20261122`, 최종 통합 때 `origin/main` `0e3f0c80` 위로 rebase; 측정일 2026-11-22. 근거 문서: `plan-docs/hardening-1.0/policy.md`의 R1(커밋 `f0b1b31b`, 프로브 C7·C8), baseline E-02·E-03.
- **이 문서는 확장 ABI(ctx 호출 형태, `access` 키 규칙, 응답 모양)를 1.0 안정 계약으로 정하지 않는다.** 현재 구현이 무엇을 강제하는지의 기록이고, 확장 ABI는 확정 전이다(OI-X2).
- 결과 요약: 쓰기 확장과 job은 `ctx` 호출을 선언한 grant로 강제한다. 읽기 확장의 `ctx.data.read`는 선언 문법이 없어 `expose read`가 있는 어느 resource든 읽었다(우회는 아니다, 호출자 정책이 그대로 적용된다, R1·C7). **호환을 유지하는 opt-in 선언 `access read.<Resource>`를 구현했다**(3절): 선언하지 않은 확장은 그대로 동작하고, 하나라도 선언한 확장만 목록 밖 읽기가 거부된다. 필수로 만들지는 않았다.

## 1. 종류별로 무엇을 선언하고 어디서 강제하는가

`access` 목록은 정의의 `extension`/`operation` 블록에 쓴다. 파서·검사는 `sema.rs`의 `extension`(`:2821` 부근), 실행 중 강제는 `spikes/spike-v4-worker`다.

| 종류 | `ctx` 호출 | 선언 | 강제하는 곳 | 선언이 없으면 |
|---|---|---|---|---|
| 읽기 확장, 읽기 operation | `ctx.data.aggregate(R, name, input)` | `access R.name`(공개한 집계, 그 입력을 확장 입력에서 같은 타입으로 받아야 함 `DEF.EXT_INPUT_UNBOUND`) | `lib.rs` `serve_call`의 `"aggregate"` | `WIRE.ACCESS_NOT_DECLARED`. 입력이 묶인 값과 다르면 `WIRE.INPUT_BINDING` |
| 같은 종류 | `ctx.data.read(query)` | **선택**: `access read.R`(opt-in, 3절). 선언하지 않아도 된다 | `serve_call`의 `"read"`: 선언이 있으면 목록(`Grant.reads`), 그리고 `expose read`가 있는 resource인지(`WIRE.NOT_EXPOSED`), 질의가 공개 읽기 계약 안인지, 호출자 행·필드 정책(`plan_read_in`) | 선언이 하나도 없으면 **거부되지 않는다**(정책이 허용하는 만큼 읽힌다). 선언이 있고 목록 밖이면 `WIRE.ACCESS_NOT_DECLARED` |
| 같은 종류 | `ctx.data.apply`, `create` | 없음(읽기 확장은 쓸 수 없다) | `serve_call` | `apply`는 `WIRE.ACCESS_DENIED`, 그 밖은 `WIRE.UNSUPPORTED` |
| 같은 종류 | `ctx.mongo.*`, `ctx.redis.*` | `access mongo.<컬렉션>`, `access redis.<키 또는 접두*>` | `capability.rs`의 grant 검사 | `WIRE.ACCESS_NOT_DECLARED` |
| 읽기 확장, operation(읽기) | 호출 횟수 | — | `lib.rs` `MAX_READ_CTX_CALLS` 64 | `WIRE.CALL_LIMIT` |
| 쓰기 확장 | `ctx.data.apply(R, t, ids, ..)` | `access R.t`(`expose apply`로 공개한 전이만, 아니면 정의 오류 `DEF.UNKNOWN_SYMBOL`) | `write.rs`의 `allowed.contains(name)` | `WIRE.ACCESS_NOT_DECLARED` |
| 쓰기 확장 | `ctx.data.create(R, values)` | `access create.R`(`expose create`가 있는 resource) | `write.rs` | `WIRE.ACCESS_NOT_DECLARED` |
| 쓰기 확장 | `ctx.data.read(query)` | `access read.R` | `write.rs` | `WIRE.ACCESS_NOT_DECLARED` |
| 쓰기 확장 | `ctx.mongo/redis` | `access mongo.*`, `redis.*` | `write.rs`, `capability.rs` | `WIRE.ACCESS_NOT_DECLARED` |
| 쓰기 확장 | 호출 횟수 | — | `write.rs` `MAX_CTX_CALLS` 20 | `WIRE.CALL_LIMIT` |
| job(`operation` 종류 job) | `read`, `aggregate`, `apply`, mongo, redis | `access read.R`, `apply.R.t`, `mongo.*`, `redis.*`(값 `true`) | `lib.rs` `job_capability_granted` | `WIRE.ACCESS_NOT_DECLARED` |

읽기 확장의 `read`만 선언이 선택이다. 쓰기 확장의 `read.R`는 선언 대상(필수)이다.

## 2. 실제 코드와의 비교 (텍스트 스캔)

`e2/scan_extensions.py`가 정의의 facts(`access`)와 구현 코드의 `ctx.data.*`·`ctx.mongo/redis` 호출을 비교한다. 전체 표는 [`e2/scan-result.md`](e2/scan-result.md). 저장소의 정의 35개, 확장과 operation 99개(읽기 26, 쓰기 28, job 45)를 봤다. 텍스트 스캔이라 증거일 뿐이고(계산된 이름, 공용 모듈의 호출은 놓친다) 21건은 구현 파일이나 함수를 이 저장소에서 찾지 못했다.

| 관찰 | 건수 | 대상 |
|---|---|---|
| 읽기 확장이 선언 없이 `ctx.data.read` | 4 | FaceReview(`product/tests/fixtures/facereview-mysql/facereview.aip`)의 `homeSearch`(Video), `watchDetail`(Video, VideoLike), `watchRecommended`(Video), `pipeline`(VideoRequest). R1이 든 `homeSearch`·`myRecentVideos` 중 `myRecentVideos`는 이번 스캔에서 `ctx.data.read` 호출을 찾지 못했다(공용 모듈 경유일 수 있어 확인 필요) |
| 선언되지 않은 접근을 쓰는 쓰기 확장 | 2 | 의도한 음성 시험: `mysql-worker-e2e/app.aip`의 `undeclared`(`Video.reset`), `porting/flaskbb/tests/fixtures/topic-move-internal-grant-repro.aip`의 `moveToForum`. 서버가 `ACCESS_NOT_DECLARED`로 막는 것을 보이는 고정물이다 |
| 선언되지 않은 집계를 부르는 읽기 operation | 1 | 의도한 음성 시험: `spike-v6-transport/tests/fixtures/operations.aip`의 `ctxProbe`(`Hidden.total`) |
| 선언했지만 코드에서 못 본 grant | 6 | 쓰기 확장의 mongo·redis grant 4건(`effects.track`, `myEmotionSummary`, `recordFrame`, `startWatching`)과 `verification/1.0-consumers/apps/stockdesk{,-1.1.0-src}`의 `place`의 `read.Order`. 공용 모듈이나 계산된 이름일 수 있다 |

해석: 쓰기 확장과 job은 선언이 곧 실행 권한이고 선언 밖 호출은 실제로 막힌다(음성 시험 3건이 그것을 고정한다). 읽기 확장의 `ctx.data.read`는 FaceReview 4건이 모두 선언 없이 읽는다. 권한 확대는 아니지만 "이 확장이 무엇을 읽는가"가 정의에 적히지 않는다.

## 3. 읽기 확장 무선언 읽기(R1, E-02)

### 3.1 위험의 크기

`policy-probes.mjs`의 C7·C8(PostgreSQL 17)이 가려진 필드, 다른 테넌트 행, `expose read` 없는 resource, 루트 없는 resource, 계약 밖 질의, 선언 안 한 집계, 쓰기 연산을 모두 공개 `/read`와 같이 거부하거나 가리는 것을 보였다. 위험은 우회가 아니라 계약에 적히지 않는 의존이다. MySQL 방언은 코드 읽기로만 같은 경로(`plan_read_in`)라고 확인했다(`policy.md` 한계 항목).

### 3.2 구현한 것 (옵트인, 호환 유지)

`policy.md` R1의 (a)를 구현했다. 확장 ABI를 안정 계약으로 정하는 것이 아니다. 현재 동작의 기록이다.

- 문법: 읽기 확장의 `access`에 `read.<Resource>`를 쓴다(쓰기 확장과 같은 형태, 쉼표로 이어 씀). 예: `access Apply.approvedCount, read.Recruitment`.
- 검사(`sema.rs`의 `extension` 읽기 분기): 대상 resource가 없으면 `DEF.UNKNOWN_SYMBOL`, 같은 선언을 두 번 쓰면 `DEF.DUPLICATE`, resource가 `read`라는 이름이면 `DEF.AMBIGUOUS`.
- 계약: 선언이 있을 때만 확장 계약에 `reads`(정렬된 resource 이름 배열)가 생긴다. `access`(집계 binding과 mongo/redis grant)에는 섞지 않았다. 선언이 없는 정의의 facts는 바뀌지 않는다(저장소 정의 195개 `service check` 출력 비교, 차이 0).
- 실행(`spike-v4-worker`의 `serve_call` `"read"`): `reads`가 있으면 목록 밖 resource의 읽기를 DB 계획 전에 `WIRE.ACCESS_NOT_DECLARED`로 거부한다. 없으면 이전과 같다.
- SDK: 생성기는 `access` 키만 쓰므로 생성물이 달라지지 않는다(recruitment 정의로 Python·TS 생성물 비교, 지문 마스킹 후 차이 0. `reads`는 `executionDigest`에는 들어간다).
- `--dev-checks` 경고는 넣지 않았다. 정의만 봐서는 어느 읽기 확장이 `ctx.data.read`를 부르는지 알 수 없어, 선언 없는 읽기 확장 전부(저장소에 26개)에 경고를 내게 되고 집계·mongo만 쓰는 확장에는 거짓 양성이다.

시험: `spikes/spike-v1-fixture/tests/diagnostic_fields.rs`의 `a_read_extension_may_declare_the_resources_it_reads`, `a_read_declaration_is_checked_like_the_one_of_a_write_extension`(DB 불필요), `spikes/spike-v4-worker/tests/read_extension_declared_reads.rs`(Node·Python 실제 IPC, DB 없는 호스트).

### 3.3 아직 하지 않은 것

- 선언을 필수로 만드는 일((b), FaceReview 4건이 깨진다)과 FaceReview 정의가 `read.Video` 등을 선언하게 고치는 일. 후자는 `product/tests/fixtures/facereview-mysql/`(다른 Track과 겹칠 수 있는 고정물)라 하지 않았다. 선언을 더한 FaceReview 정의가 통과하는 것은 임시 파일로 확인했다(`service check` ok).
- `spec/definition-syntax.md`(문법 카드)에 `read.<R>`/`create.<R>`를 적는 일. 카드는 쓰기 확장의 두 형태도 아직 적지 않았고, 카드를 바꾸면 생성 문서 골든이 다시 바뀐다.
- 공용 SDK 문서·`product/` 문서의 설명. 이번에는 `ctx.d.ts` 주석만 고쳤다.

선택지 비교와 추천은 `decisions/e.md` D-E7이다. D-E7은 소유자 결정으로 남아 있고, 위 구현은 그 결정이 (a)일 때의 변경이다.

## 4. 쓰기 확장의 미사용 선언 경고 (E-03)

구현하지 않았다. 정의만 보고는 확장 코드가 grant를 쓰는지 알 수 없다. 코드는 정의와 별도 파일이고, 공용 모듈(`youtube_common.py`)과 계산된 이름 때문에 텍스트 스캔도 거짓 양성을 낸다(위 표의 "못 본 grant" 6건이 그 예일 수 있다). 신뢰할 수 있는 방법은 실행 중 관측이다: 호출마다 쓴 grant를 기록하고 배포 단위로 "한 번도 안 쓴 grant"를 보고하는 것이다. 이는 새 기능이고 로그·저장 범위 결정이 따르므로 `decisions/e.md` D-E8로 남겼다. 선언 안 한 접근은 이미 강제로 막히므로(1절) 안전 쪽의 결함은 아니다.
