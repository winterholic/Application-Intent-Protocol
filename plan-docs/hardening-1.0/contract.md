# 1.0 계약 정합성 대조

기준은 `origin/main` `0c5d108d`다. 명세, 의미 분석, 런타임, wire 오류 코드, TypeScript·Python SDK, 생성 binding, 문서의 지원 범위 표가 서로 같은 말을 하는지 **실행해서** 대조했다. 실행 환경은 소스에서 만든 `aip 1.0.0`(debug), PostgreSQL 17 컨테이너 하나, 개발 JWT, Node 26.9, Python 3.14다. 재현 스크립트는 [`probes/`](probes/README.md)에 있다.

## 요약

실행으로 확인한 것은 정의 케이스 104건(`syn1`·`syn2`·`syn4`·`syn5`)과 필터 연산 표(타입 14종 × 연산 10종, 영구 시험은 240칸), 코드 레지스트리 3종 218개(DEF 99, WIRE 110, CLIENT 9)의 양방향 대조, 실제 서버(PostgreSQL 17)에서 받은 WIRE 코드 24종, 두 SDK와 생성 binding 호출, 문서의 링크·인용 경로·```aip 예제 13개다. **확정 불일치는 8건**이다. 코드가 틀린 것 1건은 회귀 시험과 함께 고쳤고, 문서가 틀린 것 6건은 문서를 고쳤다. 나머지 1건(C8, 진단 JSON 키)은 다른 작업자 담당이라 고치지 않고 선택지만 적었다.

| # | 불일치 | 어느 쪽이 맞나 | 조치 | 커밋(제목) |
| --- | --- | --- | --- | --- |
| C1 | `spec/client-errors.md`는 SDK가 올리는 모든 오류가 `CLIENT.*` `code`를 가진다고 하고 Python은 그렇게 하지만, TypeScript의 `WriteUnsettled`·`ScopeConflict`에는 `code`가 없었다 | 레지스트리와 Python이 맞다. TypeScript 클래스에 속성이 빠져 있었다 | **코드 수정**: 두 클래스에 `code` 추가. 회귀 시험 2개 | `fix(sdk-ts): WriteUnsettled and ScopeConflict carry the CLIENT.* code…`, `docs(client-errors): say which SDK raises which CLIENT code…` |
| C2 | `product/MANAGED-EXECUTION.md`의 outbox 예제가 `expose apply`에 `bulk maxRows`를 쓰지 않아 `DEF.MISSING_ITEM`으로 거부된다 | 검사기가 맞다(명세는 `bulk maxRows` 필수) | **문서 수정** + 문서 예제 검사 시험 | `docs(managed-execution): add bulk maxRows to the outbox example…` |
| C3 | 문법 카드가 필터 연산을 "필드 타입이 지원하는 것"이라고만 하고, `min`·`max`를 "Int 필드만"이라고만 한다. 실제로는 Int에 `in`이 없고 `min`·`max`는 groupKey와 `Int?` 결과가 필요하다(`DEF.UNSUPPORTED`) | 파서가 맞다. 작성자가 어긋난 정의를 쓰게 만드는 빈칸이다 | **문서 수정**(생성 원천 `syntax_card.rs`) + 표를 파서에 묶는 시험 2개 | `docs(syntax): state the per-type filter operators and the min/max aggregate rules…` |
| C4 | `SUPPORT.md`·`GETTING-STARTED.md`가 계약 지문을 "64자리 16진수"라고 하지만 서버는 소문자만 받는다. 대문자 16진수는 `WIRE.BAD_REQUEST`다 | 서버가 맞다(생성 binding과 두 SDK도 소문자만 낸다) | **문서 수정** | `docs: lowercase contract fingerprint, README anchor, dev-checks-only effect advisory` |
| C5 | `README.md`가 `plan-docs/README.md#이번-결과물-ae`를 링크하지만 그 제목은 없다(`기존 의도 정렬 결과물 A~E (2026-10-03)`) | 대상 문서가 맞다 | **문서 수정** | `docs: lowercase contract fingerprint, README anchor, dev-checks-only effect advisory` |
| C6 | `GENERAL-UPDATE.md`가 `DEF.ADVISORY_EFFECT_NO_LOWER_BOUND`를 조건 없이 "나온다"고 하지만 `--dev-checks`에서만 나온다 | 코드가 맞다(상시 4종은 `spike-v11-dev-checks`의 `ALWAYS_ON`) | **문서 수정** | `docs: lowercase contract fingerprint, README anchor, dev-checks-only effect advisory` |
| C7 | `SUPPORT.md`가 object·RAG 헤더 오류를 "코드 확인만"이라 했으나 실행으로 확인된다. 관리형 `/jobs`·`/files`·`/events`·`/messages`도 같은 코드다 | 코드가 맞고 문서가 보수적이었다 | **문서 수정**(실행 증거로 격상) | `docs(support): header error codes for managed routes were executed…` |
| C8 | 진단 JSON 항목 키를 `COMPATIBILITY.md`와 `definition-diagnostics.md`가 `scope`·`help`를 항상 있는 것처럼 적는다. 실제로 `help`는 같은 코드가 반복되면 두 번째부터 없고, `.h.ts`·`.h.py`에는 `scope`가 없다 | 코드는 의도한 동작이다(`enrich.rs`: "수리 방법은 한 응답에 한 번"). 문서가 틀렸다 | **고치지 않음**: 진단 JSON 호환 범위는 다른 작업자 담당. [diagnostics-contract.md](diagnostics-contract.md)의 G3·G7이 같은 어긋남을 이미 다룬다. 이 대조가 더한 관측은 `help`가 같은 코드의 반복에서 빠진다는 것이다. 선택지는 아래 | |

## 1. 명세의 문법 규칙 ↔ parser·sema

`aip service check`로 규칙마다 정의 하나를 만들어 수락·거절과 진단 코드를 봤다. 아래는 규칙 묶음별 결과이고, 규칙 하나하나는 `syn*.py`의 케이스 이름으로 찾는다. 모두 `AIP_BIN=<aip> python3 plan-docs/hardening-1.0/probes/synN.py`로 돌린다(`PASS`/`DIFF` 줄이 나온다).

| 항목 | 명세/문서 내용 | 실제 동작 | 판정 | 근거 명령 |
| --- | --- | --- | --- | --- |
| 알 수 없는 키 | 무시하지 않고 오류 | `DEF.PARSE_UNKNOWN_KEY` | 일치 | `syn1.py`(`unknown-key`) |
| `budget` | rows·depth·deadline·cost 네 값 필수, 없으면 루트로 못 읽음 | 하나 빠지면 `DEF.MISSING_ITEM`. budget 없는 `expose read`는 통과하고 루트 읽기는 `WIRE.NOT_ROOT_QUERYABLE` | 일치 | `syn1.py`(`budget-missing-cost`, `read-no-budget`), `http/app2/t7.mjs` |
| `expose apply` | `bulk maxRows` 필수 | `DEF.MISSING_ITEM` | 일치(문서 예제 1곳이 어김: C2) | `syn1.py`(`apply-no-bulk`), `node --test product/tests/doc-aip-examples.test.mjs` |
| `transition` | `from`·`allow` 필수, `to`/`patch`/`delete` 중 하나, `delete`는 `to`·`patch`·`expect`·`repeat unchanged`와 함께 못 씀 | 모두 그대로 거부(`DEF.MISSING_ITEM`, `DEF.UNSUPPORTED`) | 일치 | `syn1.py`(`trans-*`) |
| 제한 산술 | `f = f ± 항` 하나, 곱셈·두 항·0 리터럴·음수 리터럴 불가, Bool 불가, nullable 불가, `this.g`·Decimal·Int 항 | 모두 명세대로(`DEF.ARITH_NOT_ALLOWED`, `DEF.TYPE_MISMATCH`, `DEF.PARSE_EXPECTED`) | 일치 | `syn1.py`, `syn2.py`, `syn4.py`(`arith-*`, `gu-*`) |
| 전이 `input` | Int는 하한 1 이상 range 필수, 8개 이하, Int·Decimal만, compose 전이로 불가, `to f = input.p`·`repeat unchanged`와 불가 | `DEF.BAD_RANGE`, `DEF.UNSUPPORTED`, `DEF.TYPE_MISMATCH` | 일치 | `syn1.py`(`input-*`), `syn2.py`(`input-compose`), `syn4.py` |
| 숨은 필드와 전이 | 필드 읽기 정책 필드는 filter·sort·집계에 불가, 전이 `from`·`to` 값·`repeat unchanged`에 불가, `allow`와 `repeat unchanged` 없는 `to` 쓰기는 가능 | `DEF.POLICY_FIELD_NOT_FILTERABLE`, `DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`, 허용 두 경우는 통과 | 일치 | `syn2.py`(`pol-*`), `syn4.py`(`gu-policy-input`) |
| Decimal 리터럴 | 따옴표 문자열만, 소수 자릿수 `s` 이하, 정수부 `p-s` 이하, 앞자리 0·`+` 없음, `0`·`0.00`은 `DEF.BAD_VALUE` | 모두 `DEF.BAD_VALUE`. `"1.5"`(자릿수 적음)는 통과 | 일치 | `syn1.py`(`dec-*`) |
| Int 범위 | 음수 경계 허용, `lo <= hi`, Text 길이·budget·Decimal 자릿수에는 부호 불가, operation 입출력은 JS 안전 정수 | `DEF.BAD_RANGE`, `DEF.PARSE_EXPECTED` | 일치 | `syn1.py`(`int-*`, `text-neg`, `dec-neg`, `op-range-*`) |
| `X.Id`, 기본 키 | `fields` 안의 `X.Id`와 `id: Item.Id`는 `DEF.ID_FIELD_NEEDS_REFERENCE`, `id: Uuid`·`id: Member`는 가능 | 명세대로 | 일치 | `syn1.py`(`id-*`, `shared-pk`) |
| `actor` 중복, `traverse via`, cursor+nullable | `DEF.MULTIPLE_ACTORS`, `DEF.TRAVERSE_VIA_FORM`, `DEF.CURSOR_NULLABLE_SORT` | 명세대로. LIMITS E5의 오류 문구가 글자까지 같다 | 일치 | `syn1.py`(`two-actors`, `traverse-via`, `cursor-nullable`) |
| 한도 값 | `traverse limit` 1~1000, `update many maxRows` 1~1000, deadline 단위 ms·s·m | 0과 1001은 `DEF.BAD_LIMIT`·`DEF.BAD_BUDGET`, 1000은 통과, `h`는 `DEF.LEX_BAD_DURATION` | 일치 | `syn2.py`(`trav-*`, `upd-*`, `deadline-*`) |
| 필터 연산 표 | "연산 … 중 필드 타입이 지원하는 것" | 타입 14종 × 연산 10종을 직접 돌린 결과가 아래 표와 같다(Int·Decimal·Time·Date는 `eq gte lte gt lt`, Text는 `eq in prefix contains like`, Email·enum·참조는 `eq in`, Url·Bool·Json은 `eq`, Id·Uuid는 `eq in gte lte gt lt`, `isNull`은 nullable만) | **불일치(문서 빈칸)**: 표가 문서에 없었다 → C3 | `syn3.py`, `cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --test filter_ops` |
| `release` | "min·max는 Int 필드만" | groupKey 필요, 결과 `Int?` 필요(`DEF.UNSUPPORTED`). `sum`(Int)·Decimal `sum`·`count`는 명세대로 | **불일치(문서 빈칸)** → C3 | `filter_ops` 시험의 `min_and_max_…` |
| 공개 count | `where` 없는 count는 `DEF.ADVISORY_PUBLIC_COUNT` 경고(`--dev-checks` 없이), `callerFilter read`+`rows read` 원본은 `where`가 정책을 함의해야 `DEF.CALLER_FILTER_POLICY_REQUIRED` | 명세대로. 원본에 `rows read`가 없으면 `callerFilter read`여도 정책 요구는 없고 경고만 난다. 문법 카드 예제 1의 `bookmarkCount`에도 경고가 나온다 | 일치 | `syn5.py`, `docblocks.py` |
| `.aip`·`.e.ts`·`.e.py`·`.h.ts`·`.h.py` | 다섯 형식이 같은 계약 | 같은 정의 5개의 `service gen` 지문이 모두 `183ed955…2e757` | 일치 | `aip service check/gen spikes/spike-v1-fixture/fixture/recruitment.*` |
| 문서 ```aip 예제 | 복사해서 쓸 수 있는 정의 | ```aip 블록 13개 중 시험이 보는 완결 블록 11개(나머지 2개는 `extension`·`transition` 조각) 가운데 1개가 검사 실패(C2). 나머지는 통과 | 불일치 1건 → C2 | `docblocks.py`, `product/tests/doc-aip-examples.test.mjs` |
| 정의 파서 전체 시험 | | `spike-v1-fixture` 시험 248개 통과 | 일치 | `cargo test`(위 manifest) |

## 2. 진단 코드 목록 ↔ 실제 발생

| 항목 | 명세/문서 내용 | 실제 동작 | 판정 | 근거 명령 |
| --- | --- | --- | --- | --- |
| DEF 레지스트리 ↔ 소스 | 레지스트리의 99개 코드는 모두 소스가 낸다. 소스가 내는 코드는 모두 등록 | 소스에서 쓰는 `DEF.*` 문자열 집합과 레지스트리 집합이 같다(양방향 차집합 0) | 일치 | `python3 plan-docs/hardening-1.0/probes/codes.py` |
| 문서가 부르는 코드 ↔ 레지스트리 | 문서가 쓰는 코드는 존재 | 미등록 3개: `DEF.ADVISORY_CALLER_FILTER_POLICY`(COMPATIBILITY가 "이전 binary의 조언"이라 설명, 의도), `DEF.ADVISORY_WEAK_BLIND_INDEX`(`docs/design/13-field-encryption.md`의 **계획**, 제품 문서 아님), `CLIENT.BAD_ARGUMENT`(`product/VERIFICATION.md`가 부르는 FaceReview 앱 어댑터의 자체 코드이고 SDK 레지스트리에 없음) | 일치(설명 있는 2건) / 보고만 1건 | `python3 plan-docs/hardening-1.0/probes/doccodes.py` |
| 상시 경고 4종 | `PUBLIC_COUNT`, `TENANT_UNSCOPED_READ`, `PUBLIC_READ_TENANT_FIELD`, `WRITE_WIDER_THAN_READ`만 `--dev-checks` 없이 나온다 | `ALWAYS_ON` 4개와 같고, `ADVISORY_EFFECT_NO_LOWER_BOUND`는 dev 전용 | 일치(문서 1곳 어김: C6). 이번에 실행으로 본 것은 `PUBLIC_COUNT`(상시)와 `EFFECT_NO_LOWER_BOUND`(dev 전용)이고 나머지 3종은 기존 시험(`p1_sema.rs` 등, 248개 통과)에 맡겼다 | `syn4.py`의 마지막 줄, `syn5.py` |
| 문법 카드가 부르는 코드 | `DEF.MISSING_ITEM`, `DEF.BAD_VALUE`, `DEF.BAD_RANGE`, `DEF.ARITH_NOT_ALLOWED`, `DEF.OP_TYPE_MISMATCH` 등 | 각각 설명한 입력에서 그 코드가 난다(위 1절) | 일치 | `syn1.py`~`syn4.py` |
| `aip explain-code`·`diagnostics` | 세 영역을 함께 조회, 모르는 코드는 안내 | `WIRE.CONFLICT`, `CLIENT.WRITE_UNSETTLED`, `DEF.ADVISORY_PUBLIC_COUNT` 설명이 나오고 `DEF.NOPE`는 "unknown code", `--json`은 334개 | 일치 | `aip explain-code WIRE.CONFLICT`, `aip diagnostics --json` |
| 진단 JSON 키 | 항목 키 `code, line, column, message, scope, help, explain, severity` | `help`는 같은 코드의 두 번째부터 없다, `scope`는 `.h.*` 형식에 없다. 성공 키 집합과 `severity`는 문서와 같고 실패 JSON은 stderr | **불일치(문서)** → C8, 고치지 않음 | 아래 명령 |

C8 재현: 같은 정의에 `Int(5..1)`을 두 번 쓰고 `aip service check`를 돌린다. 두 번째 `DEF.BAD_RANGE`에 `help`가 없다.

```sh
printf 'actor Member\nresource Member { fields { id: Id; name: Text } rows read when id = actor }\nresource P { fields { id: Id; v: Int(5..1); w: Int(9..2); owner: Member } rows read when owner = actor }\n' > /tmp/multi.aip
aip service check /tmp/multi.aip   # stderr: diagnostics[1]와 [2]의 키 집합이 다르다
```

## 3. WIRE 코드 목록 ↔ 서버 응답 ↔ TS·Python SDK 오류 타입

| 항목 | 명세/문서 내용 | 실제 동작 | 판정 | 근거 명령 |
| --- | --- | --- | --- | --- |
| WIRE 레지스트리 ↔ 소스 | 110개 코드, 소스가 내는 코드는 모두 등록 | 양방향 차집합 0 | 일치 | `codes.py` |
| 서버가 실제로 낸 코드 | 레지스트리에 있는 코드 | 이번 실행에서 24종을 직접 받았다: `CONTRACT_REQUIRED`, `CONTRACT_MISMATCH`, `BAD_REQUEST`, `FIELD_NOT_EXPOSED`, `FILTER_NOT_ALLOWED`, `UNKNOWN_KEY`, `UNKNOWN_RESOURCE`, `NOT_EXPOSED`, `ROWS_EXCEEDED`, `COST_EXCEEDED`, `BULK_LIMIT`, `BAD_VALUE`, `TYPE_MISMATCH`, `VALUE_SOURCE_NOT_ALLOWED`, `MISSING_TARGET`, `INVALID_STATE`, `NOT_ROOT_QUERYABLE`, `ALREADY_EXISTS`, `INVARIANT_VIOLATED`, `OUTPUT_INVALID`, `NOT_AVAILABLE`, `IN_LIST_EXCEEDED`, `OFFSET_EXCEEDED`, `NOT_FOUND`. 모두 등록된 코드 | 일치 | `http/app/t1.mjs`, `t6.mjs`, `t9.mjs`, `http/app2/t7.mjs`, `t10.mjs` |
| 실패 본문 | `requestId`는 `x-request-id`와 같다, `retry`는 레지스트리 값, 성공 본문에는 두 필드 없음, 대부분 HTTP 200 | 헤더 검사 실패 12건 모두 `requestId`가 `x-request-id` 헤더와 같고 성공 본문 키는 `contractFingerprint, deps, maxAgeMs, ok, rows`. `retry`는 서버가 레지스트리에서 찾아 싣는다(`lib.rs:618`) | 일치 | `http/app/t6.mjs`, `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test error_body` |
| 계약 헤더 | 없음 → `CONTRACT_REQUIRED`, 형식 오류 → `BAD_REQUEST`, 다른 지문 → `CONTRACT_MISMATCH` (`/read`·`/apply`·`/status`) | 세 경로 모두 그대로. 대문자 16진수는 `BAD_REQUEST` | 일치(문서 표현만 보정: C4) | `http/app/t6.mjs` |
| 관리형 경로의 헤더 | object·RAG·쓰기 확장은 `CONTRACT_MISMATCH` | `/objects/*`, `/rag/*`, `/jobs/*`, `/files/*`, `/events/*`, `/messages/*` 모두 `CONTRACT_MISMATCH`, 헤더가 맞으면 설정 없는 서버는 `NOT_AVAILABLE`. 쓰기 확장 `/apply`는 확장이 있는 서버가 없어 실행 못 함 | 일치(C7), 일부 미실행 | `http/app/t9.mjs` |
| 확정 거부의 전달 | `/read` 거부는 예외, `/apply` 거부는 결과 객체(두 SDK) | TS·Python 모두 그렇다. 예외에 `code`, `retry`, `requestId`(TS) / `request_id`(Python) | 일치 | `http/app/t2.ts`, `t3.py` |
| 미확정 쓰기 | 통신 실패·`COMMIT_UNKNOWN`·`INTERNAL`이면 같은 key로 재전송, 끝내 못 정하면 `WriteUnsettled`, `retry`·`requestId` 보존 | TS: 통신 실패에 같은 key로 3번 시도한 뒤 `WriteUnsettled`, `COMMIT_UNKNOWN` 응답의 `retry`(`check-first`)와 `requestId` 보존, 목록 밖 `retry` 문자열은 노출 안 함. Python은 `WriteUnsettled` 코드를 소스로만 확인 | 일치 | `http/app/t4.ts` |
| 미확정·세션 오류의 `code` | 레지스트리: `CLIENT.WRITE_UNSETTLED`, `CLIENT.SCOPE_CONFLICT`를 SDK가 올린다 | Python은 `code`가 있고 TypeScript는 없었다 | **불일치(코드)** → C1, 고침 | `node --test spikes/spike-v6-transport/client/session.test.ts` |
| SDK 쪽 "실행 전 거부" 집합 | 두 SDK가 같은 규칙 | TS `NOT_EXECUTED`와 Python `_NOT_EXECUTED` 13개가 같고 모두 등록된 코드다 | 일치 | 소스 비교(`transport.ts:46`, `aip_sdk/__init__.py:50`) |
| CLIENT 코드 어느 SDK가 올리나 | "TypeScript 클라이언트와 Python SDK가 올리는" | `INVALID_TOKEN`·`PROTOCOL_ERROR`·`WRITE_UNSETTLED`·`SCOPE_CONFLICT`는 둘 다, 나머지 5개는 TypeScript만 | **불일치(문서 빈칸)**: 어느 쪽이 올리는지 안 적혀 있었다. 문서에 추가 + 시험 | `cargo test --manifest-path spikes/spike-v6-transport/Cargo.toml --test client_codes` |
| 빈 토큰 | `""`·공백·`undefined`는 `CLIENT.INVALID_TOKEN`, `null`/`None`은 익명 | TS 세 경우와 Python 두 경우 모두 `CLIENT.INVALID_TOKEN`, 익명 `session()`은 `actorId: null` | 일치 | `http/app/t2.ts`, `t3.py` |
| protocol 경고 | Python `discover()`와 TS `protocolWarning()`, 이 서버는 `protocol: "aip-protocol/1.0"` | 같은 major는 경고 없음, 다른 major·형식 오류는 문구. `discover()`에 경고 없음 | 일치 | `http/app/t2.ts`, `t3.py` |
| SDK 식별 | 두 SDK가 `x-aip-sdk`를 보내고 서버가 로그에만 쓴다 | 요청 로그에 `"sdk":"typescript/1.0.0"` 12줄, `"sdk":"python/1.0.0"` 14줄 | 일치 | `serve.log` 집계 |

## 4. 생성 binding(TS·Python) ↔ 서버가 받는 요청 모양

`aip service gen`(`--wire decimal`)으로 TS와 Python binding을 만들어 설치 SDK와 함께 실제 서버에 보냈다. 정의는 `probes/http/app/app.aip`(주문·줄·상품·Decimal 합계·compose)다.

| 항목 | 명세/문서 내용 | 실제 동작 | 판정 | 근거 명령 |
| --- | --- | --- | --- | --- |
| `refs` | create의 선택 키 `refs:{field:{created:N}}`, N은 요청의 연산 위치, 앞 연산만 | TS 직접 연산·`b.create()` 핸들, Python `created(0)` 모두 통과. 앞이 아닌 연산(`created:5`)은 `WIRE.BAD_VALUE`, 타입이 다른 연산은 `WIRE.TYPE_MISMATCH`, `values`와 `refs` 중복은 `WIRE.BAD_VALUE` | 일치 | `http/app/t1.mjs`, `t2.ts`, `t3.py`, `t6.mjs` |
| `created` 응답과 재생 | `created:[{resource,id}]`, 같은 key는 `replayed:true`와 저장된 `created` | 두 SDK와 raw HTTP 모두 같은 모양. 재생은 행을 다시 만들지 않는다 | 일치 | `t1.mjs`, `t3.py` |
| `target:{created:N}` | apply의 대상으로 앞 create의 id | 통과. `ids`와 함께 쓰면 `WIRE.BAD_VALUE`, compose가 만든 행을 가리키면 `WIRE.VALUE_SOURCE_NOT_ALLOWED` | 일치 | `t1.mjs`, `t6.mjs` |
| compose in bundle | 원소 `{compose,targets,steps}`, compose가 든 묶음의 응답에만 `ops:[{kind,applied:[s,e),created:[s,e)}]` | 응답 `ops:[{"applied":[0,1],"created":[0,1],"kind":"compose"},{"applied":[1,2],"created":[1,1],"kind":"apply"}]`. compose 없는 묶음은 `ops` 키 없음 | 일치 | `t1.mjs`(`bundle compose`, `bundle plain`) |
| 크기 상한 | 묶음 연산 20, compose 단계 10, 넘으면 `WIRE.BULK_LIMIT` | 20개 통과, 21개 `WIRE.BULK_LIMIT`, 단계 11개 `WIRE.BULK_LIMIT`. 생성 binding의 `bundleLimits`는 `{maxOps:20, maxRows:1000, maxSteps:10}` | 일치 | `t6.mjs`, `binding.ts` |
| compose 단계 `input` | 단계 안에서 `input` 공급 불가 | `transition` 단계의 `input` 키는 `WIRE.UNKNOWN_KEY` | 일치 | `t6.mjs` |
| `input`(Int) | 선언한 키는 모두 필수, 모르는 키는 `WIRE.FIELD_NOT_EXPOSED`, 선언 없는 전이에 보내면 `WIRE.UNKNOWN_KEY`, 범위 밖·문자열·1.5는 `WIRE.BAD_VALUE` | 모두 그대로. 재고 부족은 `WIRE.INVALID_STATE` | 일치 | `t1.mjs` |
| `input`(Decimal 문자열) | 문자열 필수, 0 이하는 `WIRE.BAD_VALUE` | `"1.25"` 통과, 숫자 `1.25`·`"1.255"`(자릿수 초과)·`"0.00"`·`"-1.00"`은 `WIRE.BAD_VALUE` | 일치 | `t1.mjs`, `t3.py` |
| Decimal 읽기·합계 | Decimal은 JSON 문자열, `sum`은 `Decimal(p',s)`, 합이 p'를 넘으면 `WIRE.OUTPUT_INVALID` | `price`·`lineSum`이 `"5.00"`·`"15.00"`, `Decimal(6,2)` 합계에 `999.99`×11을 쌓으면 `WIRE.OUTPUT_INVALID` | 일치 | `t1.mjs`, `http/app2/t7.mjs` |
| 생성 TS 타입 | 계약에 없는 create·필드·전이·값 출처는 타입에서 막힌다 | `tsc --strict`에서 positive 7건 통과, `@ts-expect-error` 5건(숫자 Decimal, 없는 전이, `input` 누락, 잘못된 `refs` 필드, 비공개 create) 모두 오류 | 일치 | `http/app/tc.ts`(`tsc -p`) |
| 생성 Python helper | `create_<r>()`, `apply_<r>_<t>(…, input=…)`, TypedDict | `create_note`, `apply_product_restock` 호출 성공, `client.create`의 `result["created"][0]["id"]` | 일치 | `t3.py` |
| 읽기 질의 모양 | `filter`는 `{field,op,value}` 배열, 객체는 `WIRE.BAD_REQUEST`, `direction`은 `WIRE.UNKNOWN_KEY`, 점 표기 select는 `WIRE.FIELD_NOT_EXPOSED`, `in` 상한 50, 열지 않은 연산은 `WIRE.FILTER_NOT_ALLOWED`, `offset`·`after`는 연 resource만 | 모두 그대로. `in` 50개 통과·51개 `WIRE.IN_LIST_EXCEEDED`, `offset` 101은 `WIRE.OFFSET_EXCEEDED`, 연 적 없는 `offset`·`after`는 `WIRE.UNKNOWN_KEY` | 일치 | `t6.mjs`, `http/app2/t10.mjs` |
| 정렬·필터 의미 | ASC는 NULL 마지막·DESC는 NULL 처음, contains는 대소문자 구분, like는 ILIKE, `%`·`_`는 contains에서 리터럴 | 모두 그대로(`pple` 매치, `PPLE` 불일치, `like %PPLE%` 매치) | 일치 | `http/app2/t7.mjs` |
| 제약 | `unique` → `WIRE.ALREADY_EXISTS`, limit+invariant → `WIRE.INVARIANT_VIOLATED`, `repeat reject`·`repeat unchanged` | 그대로. 오류 본문에 행 id·값 없음 | 일치 | `t7.mjs` |
| TS SDK 번들 | SDK가 `connect`·`WriteUnsettled`·`protocolWarning` 등을 공개 | 루트 export는 `SDK_IDENTITY, SDK_PROTOCOL, SDK_VERSION, WriteUnsettled, connect, connectEventWebSocket, createInfrastructureClient, createRagClient, protocolWarning, readSse`. `ScopeConflict`는 루트에 없다 | 일치(선택지 아래) | `http/app/t2.ts` |

## 5. 문서의 지원 범위 표 ↔ 실제 동작

`SUPPORT.md`의 19행과 `COMPATIBILITY.md`·`LIMITS.md`·`SDK.md`의 서술 중 이 환경에서 실행할 수 있는 것을 확인했다.

| 항목 | 명세/문서 내용 | 실제 동작 | 판정 | 근거 명령 |
| --- | --- | --- | --- | --- |
| 지원 11·조건부 7·제외 1 | 요약 문장의 번호와 개수 | 표의 행 판정을 세면 지원 {1,2,4,5,6,7,8,10,11,12,14}=11, 조건부 {3,9,15,16,17,18,19}=7, 제외 {13}=1 | 일치 | 표 직접 계산 |
| 2번 스키마·타입 | Int, Text, Bool, Id/Uuid, Decimal, Time, Date, Email, Url, enum, Json, Float 없음 | 모두 `check` 통과, `Float`은 `DEF.UNKNOWN_TYPE` | 일치 | `syn2.py`(`float`, `date-type`, `json-type`) |
| 9번 count와 `rows read` | `count`는 상속 안 함, `sum`·`min`·`max`는 상속 | 호출자 42가 `Item` 1건만 볼 수 있을 때 회원 43의 `cnt`=3(숨은 행 포함), `sm`=0, `mx`·`mn`=null | 일치 | `http/app3/t8.mjs` |
| 1번 | 다섯 작성 형식, `--dev-checks` 선택 | 위 1절과 같다 | 일치 | 1절 |
| 11번 | 직렬화 실패·교착 재시도 | 동시성 반례는 다른 작업자 담당이라 하지 않음 | 하지 않음 | |
| 13번 tenant 자동 격리 | 제품 경로에 구현 없음, 명시 패턴만 | 정의에 자동 격리 구문이 없다(문법 카드에 해당 키 없음). 상시 경고 3종이 명시 패턴을 안내 | 일치 | 문법 카드, `syn2.py` |
| PoC 경로 분리 | `run`·`migrate`·`rekey`는 `--features poc-runtime`에서만, `aip --help`에 PoC 명령도 나열 | 기본 빌드에서 `aip run/migrate/rekey x.aipoc`는 `RUNTIME_FEATURE_DISABLED`, `check`는 동작, 루트 `--help`에 `[1.0]`·`[PoC]` 구분 | 일치 | `aip run x.aipoc`, `aip --help` |
| 버전·protocol 표 | `1.0.0`, `aip-protocol/1.0`, `aip-core/0.11`, lockstep | `node scripts/check-versions.mjs`가 `ok:true` | 일치 | `node scripts/check-versions.mjs` |
| 문서가 인용한 파일·커밋·앵커 | 인용한 경로와 22개 커밋, 링크·앵커 | 존재하지 않는 것은 README 앵커 1건(C5)뿐(`spikes/...`, `product/sdk/engine/`는 설명용 자리표시) | 불일치 1건 → C5 | `python3 plan-docs/hardening-1.0/probes/links.py` |
| 호환 표의 MySQL 서술(`WIRE.UNSUPPORTED` 등) | MySQL은 1.0 범위 밖이고 `input`·Decimal `sum`을 거절 | MySQL 어댑터는 이 빌드에 없다 | 미실행(범위 밖) | |
| COMPATIBILITY "구서버가 새 키를 `WIRE.UNKNOWN_KEY`로 거절" | 이전 바이너리 필요 | 이전 바이너리가 없어 실행 못 함. 선언 없는 전이에 `input`을 보내면 `WIRE.UNKNOWN_KEY`인 것만 확인 | 미실행 | |
| SDK CORS preflight(`x-aip-sdk`) | 브라우저에서 허용 목록 | CORS를 설정한 서버가 없다 | 미실행 | |

## 공개 계약을 바꾸는 후보(고치지 않음)

| 항목 | 지금 | 선택지 | 호환 영향 |
| --- | --- | --- | --- |
| C8 진단 JSON의 `help`·`scope` | `help`는 같은 코드의 반복에 없고, `scope`는 `.h.*`에 없다. `COMPATIBILITY.md`는 키 집합을 시험으로 고정했다고 하되 단일 진단 한 건만 본다 | (가) 문서를 "`help`·`scope`는 있을 때만"으로 고친다. (나) 코드가 두 키를 항상 낸다 | (가)는 코드와 응답이 그대로라 호환 영향이 없다. (나)는 응답 모양과 `service.rs`의 키 집합 시험이 바뀌고 도구가 메시지 길이에 기대면 영향이 있다. 진단 JSON 호환 범위 설계가 확정된 뒤 같은 작업자가 정한다 |
| 계약 지문 대문자 | 서버·SDK·binding이 소문자만 받는다 | (가) 지금처럼 문서를 정확히 한다(이번 조치). (나) 서버가 대소문자를 구분하지 않고 받는다 | (나)는 거부하던 요청을 받는 완화라 기존 호출자에 영향이 없지만, 지문 비교를 대소문자 무시로 바꾸는 일이라 `realtime.rs`·`lib.rs`·SDK 두 곳의 정규식을 함께 바꿔야 한다. 실익이 작다 |
| TS `ScopeConflict` | `connect`의 `replaceSession`이 던지지만 패키지 루트에서 export하지 않는다. `e.name`과 `e.code`로만 구분한다 | 루트에서 export한다 | 공개 API 추가라 호환 영향은 없다. Python은 이미 `ScopeConflict`를 export한다 |
| 문서 예제 시험의 CI 연결 | `doc-aip-examples.test.mjs`는 로컬에서만 돈다. 기존 워크플로에는 바이너리를 빌드한 뒤 상시 도는 Node 시험 단계가 없다 | `verify.yml`의 제품 빌드 단계 뒤에 한 줄을 더한다 | 워크플로 변경이라 이번 범위 밖으로 두었다 |
| `CLIENT.BAD_ARGUMENT`, `DEF.ADVISORY_WEAK_BLIND_INDEX` | 레지스트리에 없는 이름을 문서가 쓴다 | 앞의 것은 FaceReview 어댑터 이름을 `CLIENT.` 아닌 앱 접두로 바꾸고, 뒤의 것은 구현될 때 등록한다 | 앞의 것은 앱 코드, 뒤의 것은 계획 문서라 제품 계약에 영향이 없다 |

## 하지 않은 것

- 동시성·권한 반례: 다른 작업자 담당이라 제외했다.
- MySQL·MariaDB, 구서버·이전 바이너리 대조, CORS preflight, 쓰기 확장 `/apply`의 헤더 오류, 외부 IdP: 이 환경에 없거나 1.0 범위 밖이라 실행하지 않았다.
- Python SDK 전체 시험(`python_sdk_test.py`)과 `service-smoke`: 이번 변경이 Python 코드와 패키징 경로를 건드리지 않아 다시 돌리지 않았다. TypeScript SDK 패키지 시험(`sdk-package.test.mjs` 5건)은 통과했다.
