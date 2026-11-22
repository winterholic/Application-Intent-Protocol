# 호환 표

엔진(Core)과 SDK의 버전, 그 버전이 말하는 protocol과 형식의 대응이다. 릴리스마다 한 줄을 더한다. `scripts/check-versions.mjs`가 현재 버전의 행이 있고 protocol과 Core IR 값이 코드와 같은지 검사한다.

## 읽는 법

- **엔진·SDK 버전**: Cargo workspace 버전이 원천이다. Core(`aip`), TypeScript SDK, Python SDK는 같은 숫자를 쓴다(lockstep). Python의 사전 릴리스 표기만 PEP 440(`1.0.0rc1`)으로 다르다.
- **protocol**: `crates/aip-contract`의 `PROTOCOL_VERSION`. 계약 문서(`GET /aip/describe`)의 `protocol` 필드와 생성 binding 헤더 주석에 실린다. **major가 호환 경계다.** 같은 major 안에서는 계약 문서에 선택 필드를 더하는 변경만 허용한다. 필드 제거, 의미 변경, 오류 코드 삭제는 major를 올린다. 오류 코드 레지스트리(`spec/wire-errors.md`)는 추가가 minor, 삭제가 major다.
- **Core IR**: `crates/aip-ir`의 `CORE_IR_VERSION`. 정의가 의미 분석을 거쳐 만들어지는 Core IR의 직렬화 태그다. 배포 저널에 저장되므로 protocol과 따로 움직인다(아래 "올리지 않은 표지").
- **앱 매니페스트**: `aip.app.json`의 `$schema`. 형식 번호는 정수로 독립 증가한다.
- **패키지 형식**: 앱 패키지 `BUILD.json`의 `format`.
- **계약 지문**: `x-aip-contract`와 `/capabilities`의 `contractFingerprint`. 앱 단위 정체성이며 SemVer와 독립이다. 배포가 바뀌면 binding을 다시 생성해야 한다.

## 버전 대응

| 엔진·SDK | protocol | Core IR | 앱 매니페스트 | 패키지 형식 | 비고 |
| --- | --- | --- | --- | --- | --- |
| 1.0.0 | `aip-protocol/1.0` | `aip-core/0.11` | `app-manifest-v1.json` | 1 | 최초 안정 릴리스(초안, 소유자 승인 전) |
| 0.1.0 | `aip-protocol/0.1` | `aip-core/0.11` | `app-manifest-v1.json` | 1 | 사전 릴리스. 공개 배포 없음 |

## 1.0에서 바꾼 표지와 그대로 둔 표지

| 표지 | 1.0 | 이유 |
| --- | --- | --- |
| protocol | `aip-protocol/0.1` → **`aip-protocol/1.0`** | 0.x는 "깨질 수 있음"으로 읽히므로 1.0 선언과 맞지 않는다. 계약 문서의 `protocol` 필드와 생성 binding 헤더 주석만 바뀐다. 계약 지문은 이 문자열에서 만들어지지 않으므로 이미 배포된 앱의 지문은 그대로다. 골든 계약 4개, 골든 client 4개, 예제 binding 4개의 변경은 각각 이 한 줄뿐이다 |
| Core IR | `aip-core/0.11` **유지** | 이 태그는 Core IR의 canonical JSON에 들어가 `digest()`에 반영되고, `aip-runtime`이 `_aip_deployment`의 `digest`와 `ir` 열에 저장한다. 문자열을 바꾸면 기존 배포 전부의 digest가 달라져 "변경됨"으로 읽히고 계획을 다시 세운다. 더 나쁘게는 롤백한 옛 바이너리가 새 태그를 `POC.I101`로 거부한다. 릴리스 gate의 롤백 경로가 이 위험을 직접 건드린다. IR의 의미가 실제로 바뀔 때만 올린다 |
| 앱 매니페스트 `$schema` | `app-manifest-v1.json` 유지 | 형식이 바뀌지 않았다 |
| 패키지 형식 `format` | `1` 유지 | 형식이 바뀌지 않았다. 패키지 안의 SDK 파일 이름만 버전·이름 설정에서 파생된다 |
| 메시지 protocol(subscribe) | 유지 | 계약 문서 버전과 별개다(`crates/aip-runtime/src/subscribe.rs`) |

### 왜 `aip-protocol/1.0`과 `aip-core/0.11`이 함께 있나

두 표지는 다른 일을 한다. `aip-protocol`은 호출자가 읽는 계약 문서와 생성 binding의 약속이라 1.0 선언에 맞춰 올렸다. `aip-core`는 Core IR의 직렬화 태그이고 배포 저널에 저장된 digest에 들어간다. 올리면 기존 배포 전부가 "변경됨"으로 읽히고 롤백한 옛 binary가 새 태그를 거부하므로(`POC.I101`) IR의 의미가 바뀔 때만 올린다. 1.0은 IR 의미를 바꾸지 않았다. 제품 경로(`aip service`)는 Core IR이 아니라 자체 facts digest로 배포를 구분하므로 이 태그가 제품 사용자에게 보이는 일은 거의 없다.

SDK의 protocol 비교는 **경고 수준**으로 넣었다. 엔진은 `/capabilities`에 선택 필드 `protocol`(`aip-protocol/1.0`)을 싣는다(같은 major 안에서는 선택 필드만 더한다는 규칙에 맞다. 값을 모르는 호스트는 필드를 싣지 않는다). Python `Client.discover()`는 이 값의 major가 `aip_sdk.PROTOCOL`과 다르거나 형식이 올바르지 않으면 `AipProtocolWarning`을 낸다. 필드가 없는 옛 엔진은 경고 대상이 아니다. 예외를 던지지 않고 호출을 막지 않으며, 호환 판단은 계속 계약 지문이다. TypeScript 클라이언트는 `/capabilities`를 스스로 부르지 않으므로 자동 경고가 없고, 그 값을 읽는 호출자를 위해 순수 함수 `protocolWarning(serverProtocol)`을 공개한다(major가 다르면 경고 문구, 아니면 `undefined`). `package.json`의 `aip.protocol`과 `aip_sdk.PROTOCOL`은 이 SDK가 말하는 protocol을 표시하며 빌드 때 `PROTOCOL_VERSION`과 일치하는지 검사한다.

## 1.0 호환 약속과 경계

정의를 바꿔 새로 배포할 때 무엇이 호환되고 무엇이 거부되는지는 다음과 같다. 코드의 동작을 문서화한 것이며 이 문서가 새 보장을 더하지 않는다.

- **같은 protocol major(`aip-protocol/1.x`)** 안에서는 계약 문서에 선택 필드를 더하는 변경만 허용한다. 필드 제거, 의미 변경, 오류 코드 삭제는 major를 올린다. 오류 코드 추가는 minor다(위 "읽는 법").
- **계약 지문**은 앱 단위 정체성이다. 호출자가 보낸 지문이 서버와 다르면 DB에 닿기 전에 `WIRE.CONTRACT_MISMATCH`로 거부한다(`retry: never`). 헤더가 없을 때의 코드는 경로마다 다르다([지원 범위](product/SUPPORT.md#계약-헤더-오류-코드)).
- **구버전 클라이언트와 새 배포의 동시 지원은 없다.** `aip service migrate --apply` 뒤 이전 정의를 가진 listener는 `WIRE.DEPLOYMENT_CHANGED`로 작업을 거부한다. 새 정의로 서버를 재시작하고 binding을 다시 생성해 SDK와 같은 배포를 맞춘다([변경 배포](product/OPERATIONS.md#변경-배포와-기존-데이터-이관)).
- **같은 지문이 같은 정책을 뜻하지 않는다.** 전이의 수정 허용 목록·기대 조건·입력 타입·nullable·range는 지문에 들어가지만, `allow`·`from`·effect 같은 정책 변경은 배포 facts digest로 fence한다. `aip service migrate`의 계획은 전이 변경을 `SecurityReview`로 표시한다([일반·부분 수정](product/GENERAL-UPDATE.md)).
- **롤백은 백업 복원이다.** 역방향 migration과 자동 롤백은 없다. 구 패키지를 migration된 schema에 기동하면 `WIRE.SCHEMA_MISMATCH`로 거부된다.
- **데이터베이스는 PostgreSQL 17만 지원한다.** 다른 버전과 MySQL·MariaDB는 지원 범위 밖이다([지원 범위](product/SUPPORT.md#postgresql-17-한정)).

### 1.0.0에서 달라진 정의 검사

1.0.0의 `aip service check`는 이전 개발 빌드가 통과시키던 정의 일부를 거부하거나 경고한다. 응답 값이나 계약 지문이 바뀌는 변경이 아니라 **정의를 받아들이는 기준**이 바뀐 것이다. 새 binary로 기존 정의를 먼저 검사한 뒤 배포한다.

| 코드 | 종류 | 이전 | 1.0.0 | 조치 |
| --- | --- | --- | --- | --- |
| `DEF.MULTIPLE_ACTORS` | **오류**(신규) | `actor` 선언이 둘이면 마지막 선언이 조용히 이기고, 처음 선언을 가정해 쓴 조건은 관련 없는 필드·타입 오류 여러 개로 나타났다 | `actor`가 둘 이상이면 이 오류 하나만 낸다 | `actor Resource` 한 줄만 남긴다. 호출자 종류가 둘이면 resource 하나에 역할 필드를 두고 조건에서 구분한다 |
| `DEF.CALLER_FILTER_POLICY_REQUIRED` | **오류**(신규) | 원본에 `rows read`가 있는 `callerFilter read` 집계는 `where`가 없을 때만, `--dev-checks`에서만 `DEF.ADVISORY_CALLER_FILTER_POLICY` 조언이 나왔다 | `where`가 원본의 `rows read` 정책을 함의하지 않으면 기본 `check`가 오류로 거부한다 | `where`에 원본 정책을 AND 항으로 적는다(`where owner = actor and amount > 0`). 정책이 `A or B`면 `where B`로 충분하다. 또는 `callerFilter none` |
| `DEF.ADVISORY_PUBLIC_COUNT` | 경고(신규, 오류 아님) | 경고 없음 | `where` 없는 `count`는 `--dev-checks` 없이도 경고한다. 정의는 통과한다 | 공개 값이 의도라면 그대로 둔다. 아니면 `where`에 모집단을 적는다 |
| `DEF.ADVISORY_PUBLIC_COUNT_INPUT` | 경고(신규, 오류 아님) | 경고 없음 | `where`가 호출자의 `input`을 쓰는 `count`가 원본 `rows read`를 함의하지 않고 sourceAccess guard도 같은 input을 받지 않으면 `--dev-checks` 없이도 경고한다. 정의는 통과하고 응답 값은 같다 | 공개 값이 의도라면 그대로 둔다. 아니면 `where`에 행 정책을 AND 항으로 쓰거나 guard가 같은 input을 제한하게 한다 |
| `DEF.ADVISORY_TENANT_UNSCOPED_COUNT` | 경고(신규, 오류 아님) | 경고 없음 | 테넌트 필드가 있는 원본의 `count`를 `expose aggregate`로 노출하고 `where`(`input` 없음)가 그 필드를 `actor`와 묶지 않으면 `--dev-checks` 없이도 경고한다. 정의는 통과하고 응답 값은 같다 | 전역 통계가 의도라면 그대로 둔다. 아니면 `where`에 `org = actor.org`를 AND 항으로 쓴다 |
| `DEF.ADVISORY_TENANT_UNSCOPED_READ` | 경고(신규, 오류 아님) | 경고 없음 | 테넌트 필드가 있는 resource의 `rows read`가 그 필드를 `actor.<필드>`와 견주지 않으면 경고한다(`or` 가지마다). `--dev-checks` 없이도 나온다 | 의도한 공개 읽기가 아니면 `rows read`에 `org = actor.org` 같은 조건을 넣는다 |
| `DEF.ADVISORY_PUBLIC_READ_TENANT_FIELD` | 경고(신규, 오류 아님) | 경고 없음 | 테넌트 필드가 있는 resource에 `rows read when true`면 경고한다 | 위와 같다 |
| `DEF.ADVISORY_WRITE_WIDER_THAN_READ` | 경고(신규, 오류 아님) | 경고 없음 | `expose create`의 `allow`가 `rows read`의 AND 항을 요구하지 않거나, 공개 전이의 `patch`가 테넌트 필드를 바꿀 수 있고 테넌트 resource가 `actor.<필드>`로 한정되지 않으면 경고한다 | 생성 값이 호출자에게 안 보이는 행이 되지 않는지 확인한다. 의도라면 그대로 둔다 |

`DEF.CALLER_FILTER_POLICY_REQUIRED`가 막는 것은 호출자가 `callerFilter read`의 필터로 읽을 수 없는 행의 필드 값을 한 값씩 세어 알아내는 경로다. `count`가 원본의 `rows read`를 적용하지 않는다는 **공개 count 계약과 응답 값은 1.0에서 바꾸지 않았다.** `count` 집계의 facts에 키를 더하지 않아 기존 배포의 실행 digest도 달라지지 않는다. 저장소의 `.aip` 36개(FaceReview, FlaskBB, Infisical, 예제, 엔진 fixture)를 이전 binary와 새 binary로 검사했을 때 새로 오류가 나는 파일은 없었고, 새로 나온 출력은 `DEF.ADVISORY_PUBLIC_COUNT` 경고뿐이었다. 저장소 밖의 정의는 이 보장 범위 밖이므로 위 표의 두 오류를 직접 확인한다. 정책 작성 경고 3종은 조언이라 정의를 거부하지 않으며 facts, 실행 digest, 계약 지문을 바꾸지 않는다. 저장소의 `.aip`·`.e/.h.ts·py` 60개와 소비자 앱 정의 3개(63개)에서 새로 경고가 난 정의는 2개(경고 3건)였다: `porting/flaskbb/app.aip`의 `Message` create, `product/tests/fixtures/facereview-mysql/facereview.aip`의 `Video`·`Comment` create(모두 `DEF.ADVISORY_WRITE_WIDER_THAN_READ`, `ok: true`, 종료 코드 0). 경고를 실패로 처리하는 CI가 있으면 영향을 받는다. 판정 규칙과 놓치는 경우는 [P1 sema 노트](plan-docs/expressiveness-1.0/p1-sema-notes.md#2-정책-작성-경고-3종-조언만-정의를-거부하지-않는다)다(`79f2d3e8`). 계약 전문은 [SDK의 공개 count 계약](product/SDK.md#public-count-contract-공개-count-계약)이다.

같은 변경에서 이미 거부되던 입력의 **진단만** 정확해진 것도 있다. 호환에 영향이 없다. `traverse Child via field`처럼 잘못 쓴 `via`는 `DEF.TRAVERSE_VIA_FORM`이 올바른 형태(`via Child.field`)를 안내하고, Decimal에 쓴 `0`·`0.00`은 `DEF.BAD_VALUE`가 따옴표 형식(`"0.00"`)을 안내하며, `transition`에 빠진 `from`·`allow`는 어느 것이 빠졌는지 알려 준다. `DEF.ACCESS_MISMATCH`의 문구는 실제 판정(인자 resource가 집계를 선언한 쪽이어야 하고 `groupKey`가 있어야 한다)에 맞게 고쳤다. 같은 정의를 같은 코드로 거부하고 문구만 다르다. 메시지 문자열을 매칭하는 도구가 있으면 코드(`DEF.ACCESS_MISMATCH`)로 바꾼다.

## 표현력 변경의 호환 경계

기준 `4e132564`..`be94d064`(49 커밋)가 더한 문법과 동작이다. 사용법은 [일반·부분 수정](product/GENERAL-UPDATE.md), [SDK 묶음과 조합](product/SDK.md#묶음bundle과-조합compose), [표현력 한계](product/LIMITS.md)에 있고 변경 목록은 [CHANGELOG](CHANGELOG.md)다. 이 절은 호환에 영향을 주는 것만 적는다. 코드의 동작을 문서화한 것이며 새 보장을 더하지 않는다. `aip-protocol/1.0`과 `aip-core/0.11`은 이 변경에서도 올리지 않았다(`PROTOCOL_VERSION`과 `crates/aip-ir`는 이 구간에서 바뀐 파일이 없다).

### 새 문법과 wire 추가

| 문법 | 정의에서 | wire(요청·응답) | 근거 |
| --- | --- | --- | --- |
| 결과 참조 | 정의 변경 없음 | create 연산의 선택 키 `refs`(`{"order":{"created":0}}`), apply 연산의 `target:{"created":0}`. 응답 모양은 그대로다 | `600b86ad`, `12421f9d` |
| 제한 산술 | 전이의 `input { … }`와 `to f = f ± 항`, 효과 update의 `f = f ± 항` | apply·bundle apply·확장 `ctx.data.apply`의 선택 키 `input` | `acaed2ab`, `b1bda3b2`, `c24bede4` |
| bundle 안 compose | 정의 변경 없음(`expose compose`가 있어야 한다) | `atomic[]` 원소 `{"compose","targets","steps"}`. compose가 든 묶음의 응답에만 `ops` 키가 더해진다 | `a947289e`, `952c4331`, `583c4e07` |
| 확장 `ctx.data.create` | 확장의 `access create.<Resource>`와 그 resource의 `expose create` | HTTP 호출자 wire 변화 없음. worker ABI의 `ctx.data.create(res, values, key)`와 `ctx.data.apply`의 마지막 선택 인자 `input` | `27f87d10`, `c24bede4` |

### 호환 경계

| 경계 | 내용 | 확인 |
| --- | --- | --- |
| 기존 정의의 출력 불변 | 새 문법을 쓰지 않은 정의의 facts, 계약 지문, 실행 digest, 생성 TypeScript·Python binding은 바뀌지 않는다. 새 facts 키(`arith`, `input`)는 그 기능을 쓴 전이에만 나오고 `refs`는 지문 본문에 들어가지 않는다. 리터럴 정수 증감은 계속 `{"increment": n}`이다 | 기준 바이너리와 새 바이너리의 `service check`·`core`·`service gen` 출력을 `diff -r`했다. 정의 46개(P0 묶음)와 63개(P1 묶음, [p1-sema-notes 4절](plan-docs/expressiveness-1.0/p1-sema-notes.md))에서 차이가 없었다. 예외는 아래 "출력이 달라지는 것" |
| 지문이 바뀌는 곳 | `input`을 선언한 전이가 있는 정의, Decimal `sum`(p'가 지문에 들어간다)을 새로 쓴 정의. 둘 다 새 정의다 | `decisions.md` P0-B, `spike-v5-sdk/tests/decimal_sum_codegen.rs` |
| 응답 `ops` 구간 | `ops: [{kind, applied:[시작,끝), created:[시작,끝)}]`는 **compose 연산이 든 묶음에만** 나온다. compose 없는 묶음의 응답과 저장된 멱등 결과의 재생은 이전과 글자까지 같다. 구간의 끝은 포함하지 않는다 | v6 `lib.rs` `run_write`, `expr_compose_bundle_http.rs` |
| 구서버 | 새 요청 키는 구서버에서 `WIRE.UNKNOWN_KEY`로 거절된다(fail-closed). 새 SDK와 구서버를 섞어 새 문법을 쓰면 서버가 거절한다 | `decisions.md` C0-7(구서버 거절을 그 세션에서 실행해 확인했다고 적혀 있다. 이 문서 작성에서는 재실행하지 않았다) |
| protocol | 새 키는 모두 요청의 선택 키이고 응답의 `ops`도 선택 필드라서 위 "같은 protocol major" 규칙 안의 추가다. 필드 제거와 의미 변경은 없다 | `PROTOCOL_VERSION` 불변 |
| MySQL(1.0 지원 범위 밖) | `input`·제한 산술은 `WIRE.UNSUPPORTED`. 묶음의 `refs`·`target.created`·`compose`는 기존 키 검사가 `WIRE.UNKNOWN_KEY`로 거절한다. **Decimal `sum`은 읽기 계획에서 `WIRE.UNSUPPORTED`다**(MySQL에는 Decimal 합 정규화와 초과 검사가 없다). 쓰기 확장 경로의 MySQL `ctx.data.create`는 이 변경 이전부터 있다 | `crates/aip-migrate/src/mysql_write.rs` `refuse_limited_arithmetic`, `spikes/spike-v2-read/src/plan.rs:137` |
| 새 오류 코드 | 없다. 기존 코드(`WIRE.BAD_VALUE`, `WIRE.TYPE_MISMATCH`, `WIRE.VALUE_SOURCE_NOT_ALLOWED`, `WIRE.UNKNOWN_KEY`, `DEF.ARITH_NOT_ALLOWED`, `DEF.TYPE_MISMATCH`, `DEF.UNSUPPORTED`)에 메시지를 더했다. `WIRE.OUTPUT_INVALID`의 원인 설명에 Decimal `sum` 초과가 추가됐다(오류 코드 추가·삭제 아님) | `spec/wire-errors.md` |

### 출력이 달라지는 것

위 "기존 정의의 출력 불변"의 예외다. 모두 오류가 아니라 경고 추가이거나 문구 변경이다.

- `aip service check`의 `advisories`에 정책 작성 경고 3종이 더해질 수 있다(앞 절 표). 63개 중 2개 정의(3건).
- `DEF.ACCESS_MISMATCH`와 compose 단계의 `patch`·`expect`·`input` 진단 문구가 바뀌었다(코드는 같다).
- 진단 JSON에 `severity` 키가 더해졌다(아래).

### 동작 변화

| 동작 | 변화 | 범위 | 근거 |
| --- | --- | --- | --- |
| 완료된 멱등 응답 재생 | 같은 key의 재전송이 이미 커밋된 성공 결과를 worker 슬롯을 잡기 전에 `replayed: true`로 돌려준다. 이전에는 같은 주체의 슬롯이 차 있으면 `WIRE.WORKER_BUSY`였다. 배포 fence(`WIRE.DEPLOYMENT_CHANGED` 등)는 재생 전에도 확인한다. 같은 key에 다른 본문은 `WIRE.IDEMPOTENCY_MISMATCH`다. 진행 중인 key와 성공이 아닌 자리표시 행은 재생하지 않고 기존 경로로 간다 | PostgreSQL 쓰기 확장 `/apply`(관계형 인프라가 없는 경로). 다른 경로의 재생은 이 변경으로 바뀌지 않았다 | `241ae37f`, `7cdfed93`, `write_extension_replay_backpressure.rs` |
| 정책 작성 경고(advisory) | 정의를 거부하지 않는 조언 3종이 기본 `check`에 나온다 | `aip service check` | `79f2d3e8` |
| Decimal `sum` | `release sum(f)`가 `Decimal(p,s)`를 받는다. 결과 `Decimal(p',s)`(s 같음, p' >= p)는 JSON 문자열이고 원본 `rows read`를 적용한다. 합이 p'를 넘으면 `WIRE.OUTPUT_INVALID`, `avg`는 계속 `DEF.UNSUPPORTED`, `min`·`max`는 Int 필드만, 암호화 필드 집계는 `DEF.UNSUPPORTED`다. Int `sum`은 그대로 JSON 숫자다 | PostgreSQL. MySQL은 위 표 | `79f2d3e8`, `7e367ead`, `spike-v2-read/tests/decimal_sum.rs` |
| 암호화 리소스 거절 | `ctx.data.create`는 암호화 필드가 있는 resource를 `WIRE.UNSUPPORTED`로 거절한다. 암호화가 켜진 서버는 묶음 안 `compose`와 `refs`를 `WIRE.UNSUPPORTED`로 거절하고 쓰기도 멱등 기록도 남기지 않는다(`input`은 거절하지 않는다) | 라이브러리 층. 1.0 제품 경로는 암호화 활성 정의를 `serve` 전에 거부한다([지원 범위](product/SUPPORT.md#postgresql-17-한정)) | `3a6edf5f`, `952c4331`, `dc68e4e6` |
| 행 잠금 모드 | 키를 바꾸지 않는 전이는 `FOR NO KEY UPDATE`, 키 변경·삭제·자식 행 갱신·삭제 효과는 `FOR UPDATE`. 묶음의 create가 읽는 참조 행에 뒤 전이가 필요로 하는 잠금을 미리 잡는다. 응답 값은 같고 교착 가능성만 달라진다 | PostgreSQL | `824cc0e1`, `dc68e4e6` |
| 확장의 거절 전달 | 쓰기 확장이 `WIRE.CONFLICT`·`WIRE.ALREADY_EXISTS`를 그대로 내보내면 호출자가 그 코드를 받는다(이전에는 일반 `EXTENSION_ERROR`로 보였다). 확장 경로는 서버가 재실행하지 않으므로 호출자가 같은 key로 다시 보낸다 | PostgreSQL 쓰기 확장 | `9907d1c9` |

### 진단 JSON 형식

`aip service check`의 JSON은 다음 키 집합과 타입을 시험으로 고정했다(`crates/aip-cli/tests/service.rs`, `7e367ead`). 이 시험은 키 집합을 정확히 비교하므로 키를 더하거나 빼면 시험이 깨진다.

| 경우 | 종료 코드·출력 | 최상위 키 | 항목 키 |
| --- | --- | --- | --- |
| 성공 | 0, stdout | `ok`(true), `executionDigest`(64자), `advisories`, `workers` | advisory: `code`, `anchor`(`line:`으로 시작), `message`, `severity`(`"warning"`) |
| 실패 | 1, stdout은 비어 있고 stderr | `ok`(false), `code`(`DEF.INVALID_DEFINITION`), `diagnostics` | diagnostic: `code`, `line`, `column`, `message`, `scope`, `help`, `explain`, `severity`(`"error"`) |

별도 스키마 버전 필드는 없다. 이 고정을 호환 약속(제거·타입 변경 금지)으로 문서화할지는 소유자 결정 전이다(확인 필요).

## SDK 버전 식별

- **노출**: TypeScript는 `SDK_VERSION`, `SDK_IDENTITY`(`typescript/<version>`), `SDK_PROTOCOL`을, Python은 `aip_sdk.__version__`, `SDK_IDENTITY`(`python/<version>`), `PROTOCOL`을 공개한다. 값은 Cargo workspace 버전과 `PROTOCOL_VERSION`에서 파생하며 `scripts/check-versions.mjs`가 어긋나면 실패한다.
- **전송**: 두 SDK는 모든 HTTP 요청에 `x-aip-sdk: <identity>` 헤더를 보낸다.
- **서버**: 엔진은 이 헤더를 요청 로그 줄(`"sdk":"typescript/1.0.0"`)과 OTLP span 속성(`aip.sdk`)에만 쓴다. 48자 이하의 안전한 문자(`A-Za-z0-9._/+-`)가 아니면 무시하고, 값이 있든 없든 응답과 거부 판단은 달라지지 않는다. 버전 협상이나 거부는 없다. 호환 신호는 여전히 계약 지문이다.
- **옛 SDK의 증상**: 이 헤더가 생기기 전의 SDK(개발 빌드 `0.1.0`은 날짜가 달라도 모두 같은 번호였다)는 헤더를 보내지 않고, 로그에 `sdk`가 없다. 새 binding과 옛 SDK를 섞으면 SDK에 없는 기능을 부르는 순간 `TypeError`(예: `client.session is not a function`)로 끝난다. 서버 응답 문제가 아니므로 로그에서 `sdk` 필드가 없는 호출자를 찾아 SDK를 binding과 같은 릴리스로 올린다.
- **브라우저**: 새 SDK가 보내는 `x-aip-sdk`는 CORS preflight에서 허용 목록에 있어야 한다. 이 변경 전의 엔진(`0.1.0` 개발 빌드)은 preflight를 `WIRE.BAD_REQUEST`로 거절하므로, 브라우저에서 SDK 1.0.0을 쓰려면 엔진도 1.0.0 이상이어야 한다. 서버 간 호출(Node, Python)은 CORS가 없어 영향이 없다.

## 지원 플랫폼

CLI 아카이브의 target과 최소 요구는 [INSTALL](product/INSTALL.md#지원-플랫폼과-최소-요구)을 따른다.
