# 확장의 권한 선언과 실제 `ctx.data.*` 접근 (Track E2)

- 기준: `stab/e-20261122`(origin/main `b818613d` 위), 측정일 2026-11-22. 근거 문서: `plan-docs/hardening-1.0/policy.md`의 R1(커밋 `f0b1b31b`, 프로브 C7·C8), baseline E-02·E-03.
- **이 문서는 확장 ABI(ctx 호출 형태, `access` 키 규칙, 응답 모양)를 1.0 안정 계약으로 정하지 않는다.** 현재 구현이 무엇을 강제하는지의 기록이고, 확장 ABI는 확정 전이다(OI-X2).
- 결과 요약: 쓰기 확장과 job은 `ctx` 호출을 선언한 grant로 강제한다. **읽기 확장의 `ctx.data.read`만 선언 없이 `expose read`가 있는 어느 resource든 읽는다.** 이는 우회가 아니다(호출자 정책이 그대로 적용된다, R1·C7). 이번에는 코드를 바꾸지 않았다. 이유는 3절.

## 1. 종류별로 무엇을 선언하고 어디서 강제하는가

`access` 목록은 정의의 `extension`/`operation` 블록에 쓴다. 파서·검사는 `sema.rs`의 `extension`(`:2821` 부근), 실행 중 강제는 `spikes/spike-v4-worker`다.

| 종류 | `ctx` 호출 | 선언 | 강제하는 곳 | 선언이 없으면 |
|---|---|---|---|---|
| 읽기 확장, 읽기 operation | `ctx.data.aggregate(R, name, input)` | `access R.name`(공개한 집계, 그 입력을 확장 입력에서 같은 타입으로 받아야 함 `DEF.EXT_INPUT_UNBOUND`) | `lib.rs` `serve_call`의 `"aggregate"` | `WIRE.ACCESS_NOT_DECLARED`. 입력이 묶인 값과 다르면 `WIRE.INPUT_BINDING` |
| 같은 종류 | `ctx.data.read(query)` | **선언 문법이 없다**(`access read.R`는 `DEF.UNKNOWN_SYMBOL`) | `serve_call`의 `"read"`: `expose read`가 있는 resource인지(`WIRE.NOT_EXPOSED`), 질의가 공개 읽기 계약 안인지, 호출자 행·필드 정책(`plan_read_in`) | **거부되지 않는다.** 정책이 허용하는 만큼 읽힌다 |
| 같은 종류 | `ctx.data.apply`, `create` | 없음(읽기 확장은 쓸 수 없다) | `serve_call` | `apply`는 `WIRE.ACCESS_DENIED`, 그 밖은 `WIRE.UNSUPPORTED` |
| 같은 종류 | `ctx.mongo.*`, `ctx.redis.*` | `access mongo.<컬렉션>`, `access redis.<키 또는 접두*>` | `capability.rs`의 grant 검사 | `WIRE.ACCESS_NOT_DECLARED` |
| 읽기 확장, operation(읽기) | 호출 횟수 | — | `lib.rs` `MAX_READ_CTX_CALLS` 64 | `WIRE.CALL_LIMIT` |
| 쓰기 확장 | `ctx.data.apply(R, t, ids, ..)` | `access R.t`(`expose apply`로 공개한 전이만, 아니면 정의 오류 `DEF.UNKNOWN_SYMBOL`) | `write.rs`의 `allowed.contains(name)` | `WIRE.ACCESS_NOT_DECLARED` |
| 쓰기 확장 | `ctx.data.create(R, values)` | `access create.R`(`expose create`가 있는 resource) | `write.rs` | `WIRE.ACCESS_NOT_DECLARED` |
| 쓰기 확장 | `ctx.data.read(query)` | `access read.R` | `write.rs` | `WIRE.ACCESS_NOT_DECLARED` |
| 쓰기 확장 | `ctx.mongo/redis` | `access mongo.*`, `redis.*` | `write.rs`, `capability.rs` | `WIRE.ACCESS_NOT_DECLARED` |
| 쓰기 확장 | 호출 횟수 | — | `write.rs` `MAX_CTX_CALLS` 20 | `WIRE.CALL_LIMIT` |
| job(`operation` 종류 job) | `read`, `aggregate`, `apply`, mongo, redis | `access read.R`, `apply.R.t`, `mongo.*`, `redis.*`(값 `true`) | `lib.rs` `job_capability_granted` | `WIRE.ACCESS_NOT_DECLARED` |

읽기 확장의 `read`만 비대칭이다. 쓰기 확장의 `read.R`가 선언 대상인 것과 다르다.

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

### 3.2 이번에 구현하지 않은 이유

호환을 유지하는 개선은 R1의 (a) opt-in이다. 읽기 확장에서도 `access read.<Resource>`를 받아들이고, 하나라도 선언한 확장에서만 목록 밖 읽기를 거부한다. 이 변경은 세 군데를 함께 바꿔야 의미가 있다.

1. `sema.rs`의 `extension` 읽기 분기: `read.<Resource>`를 받아 facts의 `access`에 싣는다. **`sema.rs`는 Track A 병합 완료 메시지 전에는 건드리지 않는 규칙이라 지금 수정할 수 없다.**
2. `spike-v4-worker`의 `serve_call` `"read"`: 선언이 하나라도 있으면 목록으로 거른다. 선언은 facts에 생겨야 하므로 1 없이는 실행 경로가 없다. 런타임만 먼저 넣으면 정의로는 켤 수 없는 코드가 된다.
3. SDK 타입·문서·`--dev-checks` 경고: 선언이 없는 읽기 확장에 "읽는 resource를 선언하라"는 경고는 선언 문법이 생긴 뒤에야 조치 가능하다. 문법 없이 경고만 내면 고칠 방법이 없는 소음이다(R1의 (c)가 어렵다고 적은 이유와 같다).

### 3.3 sema.rs 해제 뒤의 구현 순서 (옵트인, 호환 유지)

1. `sema.rs:2896`의 읽기 분기 루프(`for (rn, an) in &x.access`)에서 `rn == "read"`를 쓰기 분기와 같은 방식으로 받는다(대상이 `expose read`가 있는 resource인지 확인, 중복은 `DEF.DUPLICATE`). facts에는 별도 키(예: `reads`)로 싣고 `access`(집계 binding 맵)와 섞지 않는다. 섞으면 `Grant.access: HashMap<String, Map<..>>`의 값 타입과 어긋난다.
2. `serve_call`의 `"read"`: `reads`가 비어 있지 않으면 `name`이 목록에 없을 때 `WIRE.ACCESS_NOT_DECLARED`.
3. 시험: (가) 선언 없는 기존 읽기 확장은 동작 불변(`spike-v4-worker/tests/read_wire.rs`, FaceReview `homeSearch`), (나) 선언한 확장이 목록 밖 resource를 읽으면 거부, (다) 선언한 resource는 공개 `/read`와 같은 결과(C7과 같은 질의).
4. 계약 지문은 `reads`를 선언한 정의에서만 바뀐다(기존 정의는 불변). 확인 필요: 이 값이 `executionDigest`에 들어가는 경로.
5. `--dev-checks`에 "읽기 확장이 `read.*`를 선언하지 않음" 경고를 더한다(A 소유 v11의 좁은 영역, 같은 시점).
6. `ctx.d.ts`와 SDK 문서에 "읽기 확장의 `read.<R>` 선언은 선택, 하나라도 선언하면 강제"를 적는다. 확장 ABI를 확정하는 문구는 쓰지 않는다.

선택지 비교와 추천은 `decisions/e.md` D-E7.

## 4. 쓰기 확장의 미사용 선언 경고 (E-03)

구현하지 않았다. 정의만 보고는 확장 코드가 grant를 쓰는지 알 수 없다. 코드는 정의와 별도 파일이고, 공용 모듈(`youtube_common.py`)과 계산된 이름 때문에 텍스트 스캔도 거짓 양성을 낸다(위 표의 "못 본 grant" 6건이 그 예일 수 있다). 신뢰할 수 있는 방법은 실행 중 관측이다: 호출마다 쓴 grant를 기록하고 배포 단위로 "한 번도 안 쓴 grant"를 보고하는 것이다. 이는 새 기능이고 로그·저장 범위 결정이 따르므로 `decisions/e.md` D-E8로 남겼다. 선언 안 한 접근은 이미 강제로 막히므로(1절) 안전 쪽의 결함은 아니다.
