# Track D 결과: Parser → Sema → Runtime → SDK 의미 정합 (2026-11-22)

- 기준: `origin/main` `4f44ca87`(기준선 문서 커밋) 위의 브랜치 `stab/d-20261122`. 모든 DB 시험은 **PostgreSQL 17.11(Homebrew, 포트 55424)** 에서 실행했다.
- 이 문서는 1.0 출시 승인이 아니다. 태그, GitHub Release, npm, PyPI 게시는 하지 않았다.
- 핵심 파일(`sema.rs`, `spike-v3-write/src/lib.rs`, `spike-v6-transport/src/lib.rs`)은 수정하지 않았다. 그 파일이 필요한 결함은 9절 "핵심 파일 대기 목록"에 재현 시험과 함께 적었다.

## 1. 변경(파일·목적·커밋)

| 커밋 | 파일 | 목적 |
|---|---|---|
| `e04938c3` | `product/test-support/service-fixture.mjs`, `product/tests/semantic-path-parity.test.mjs` | D2: 같은 전이를 직접 apply, bundle, compose, 쓰기 확장 네 경로로 PG17에서 비교하는 시험과 공용 서비스 픽스처 |
| `cd4f715a` | `product/tests/semantic-wire-measure.test.mjs`, `product/tests/sdk-type-rejection.test.mjs`, `product/tests/fixtures/sdk-semantics/**` | D6·D1 budget 실측, TS 타입 거부 코퍼스 |
| `5387e7e9` | `spikes/spike-v6-transport/client/transport.ts`, `product/python/aip_sdk/__init__.py`, `spikes/spike-v6-transport/client/unsettled-codes.test.ts`, `product/tests/python_sdk_contract_test.py` | SDK 결함 수정: 실행 전 거절 코드 5개와 Python 키 재사용 |
| `8462fbe4` | `spikes/spike-v6-transport/client/transport.ts`, `spikes/spike-v6-transport/client/bundle-result-ids.test.ts` | SDK 결함 수정: Uuid 자원의 compose를 담은 묶음 결과를 형식 오류로 오판 |
| `251242a0`, `222a1e72` | `spikes/spike-v5-sdk/sdk/generic.ts`, `product/tests/sdk-contract-semantics.test.mjs`, `product/tests/sdk-type-rejection.test.mjs` | TS 타입 개선(참조 키, 중복 select, 원시 refs create)과 TS·Python PG17 여정 시험. `222a1e72`가 `bundle` 오버로드 시도를 되돌리고 필드 조합 union으로 바꿨다(아래 4절) |
| `222a1e72` | `product/tests/semantic-chain.test.mjs` | D4: 정의 → 실행 기록(감사 행) → DB → SDK 응답 |
| `d23091a3` | `product/tests/semantic-definition-matrix.test.mjs`, `product/tests/fixtures/definition-matrix.expected.json` | D1: 정의 조각별 check 결과와 PG17 런타임 결과 표(골든), 핵심 파일 대기 todo 재현 3개 |

## 2. D1: check 통과인데 런타임 미지원 / 실행 가능한데 정적 거부 (현재 HEAD 반례 탐색)

방법: 정의 조각마다 `aip service check`를 돌리고, 통과하면 `init`·`serve`로 PG17에서 요청을 보내 런타임 결과를 기록했다. 골든은 `product/tests/fixtures/definition-matrix.expected.json`이고 시험(`semantic-definition-matrix.test.mjs`)이 같은 결과를 매번 단언한다(3회 반복 일치). 43개 조각 중 **반례는 5개**다: check 통과 / 런타임 거부 2개와 실행 가능한데 정적 거부 1개는 굵게 표시했고, 일부 경로 미지원 2개(`apply-maxRows-over-bundle-cap`, `Int-unbounded-beyond-2^53`)는 판정 열에 적었다.

| 조각 | 설명 | check | 런타임(PG17) | 판정 |
|---|---|---|---|---|
| `Int-unbounded-beyond-2^53` | Int without a range holding bigint values above the JS safe range | pass | server=[{"id":"1","n":9007199254740993},{"id":"2","n":9223372036854775807}] \| ts=CLIENT.PROTOCOL_ERROR \| python=[{"id": "1", "n": 9007199254740993}, {"id": "2", "n": 9223372036854775807}] | check 통과 / 런타임 부분 지원. 서버는 정확한 숫자를 보낸다. TS SDK는 안전 정수 밖 값을 `CLIENT.PROTOCOL_ERROR`로 거부하고, 쓰기에서는 JS가 정확한 값을 표현하지 못한다. Python은 정확하다. 결정 D-DEC-1 |
| `Int-range-bigint-bounds` | Int(0..9223372036854775807) beyond safe integer | pass | check-only | check 통과(런타임 미실행). `Int(0..9223372036854775807)`도 통과한다. 같은 JS 안전 정수 문제. 결정 D-DEC-1 |
| `Decimal-min-max` | Decimal aggregate min/max with groupKey | fail DEF.TYPE_MISMATCH | - | 정적 거부(의도). min·max는 Int 필드만(`product/SUPPORT` 집계 표와 같다) |
| `Decimal-sum` | Decimal sum per group | pass | ok rows=[{"id":"1","total":"3.35"}] | 일치. Decimal 합계는 문자열 `3.35`로 정확히 나간다 |
| `Int-min-per-group` | Int min per group with an empty group | pass | ok rows=[{"id":"1","low":3},{"id":"2","low":null}] | 일치. 빈 그룹은 null, 결과 타입 `Int?` |
| `nullable-sort-cursor` | nullable sort field together with cursor paging | fail DEF.CURSOR_NULLABLE_SORT | - | 정적 거부(의도). 문법 카드와 같다 |
| `cursor-sort-not-selected` | cursor paging with a sort field that is not selected | fail DEF.CURSOR_SORT_NOT_SELECTED | - | 정적 거부(의도). 문법 카드와 같다 |
| `traverse-depth-below-need` | budget depth 1 with a traverse that needs depth 2 | fail DEF.BUDGET_DEPTH_TOO_SMALL | - | 정적 거부(의도). 자기 정의가 쓸 수 없는 budget을 check가 잡는다 |
| `cost-too-low` | budget cost 1 on a plain read | pass | WIRE.COST_EXCEEDED | **check 통과 / 런타임 항상 거부**. 정의가 선언한 읽기가 어떤 요청에도 `WIRE.COST_EXCEEDED`다. 핵심 파일 대기 C1 |
| `deadline-1ms` | budget deadline 1ms | pass | answers within {ok, WIRE.DEADLINE_EXCEEDED}: true | 일치. 1ms 한도는 질의 속도와 경쟁한다. 한도가 적용된다는 뜻이다 |
| `rows-1-with-limit` | budget rows 1 with 3 rows present | pass | ok rows=[{"id":"1"}] | 일치. `rows 1`이면 3행 중 1행만 준다(조용한 절단이 아니라 선언한 상한) |
| `text-range-codepoints` | Text(1..3) with three emoji and with a combining mark | pass | 3 emoji=ok 4 emoji=WIRE.BAD_VALUE e+combining x3=WIRE.BAD_VALUE nul=WIRE.BAD_VALUE | 일치. Text 길이는 코드포인트 기준이다. 이모지 3개 통과, 결합 문자 6개 거부, NUL 거부 |
| `decimal-literal-forms` | Decimal(5,2) literals in to: 0, 0.00, "0.0" | fail DEF.BAD_VALUE | - | 정적 거부(의도). 숫자 리터럴 `0`은 Decimal이 아니다(문법 카드) |
| `decimal-literal-string-short` | Decimal(5,2) literal "0.5" (fewer digits than scale) | pass | ok {"changed":["1"]} \| ok rows=[{"d":"0.50"}] | 일치. `"0.5"`는 `0.50`으로 저장된다 |
| `arith-on-nullable` | f = f + 1 on a nullable Int field | fail DEF.TYPE_MISMATCH | - | 정적 거부(의도). C0-2 nullable 산술 금지 |
| `arith-overflow-range` | n = n + 1 on Int(0..2) at 2 | pass | WIRE.BAD_VALUE | 일치. 결과가 범위를 넘으면 `WIRE.BAD_VALUE`, 롤백 |
| `arith-decimal-int-term` | Decimal field += Int literal | pass | ok {"changed":["1"]} \| ok rows=[{"d":"2.50"}] | 일치. Decimal 필드에 Int 항이 정확히 더해진다 |
| `arith-this-field` | n = n + this.m | pass | ok {"changed":["1"]} \| ok rows=[{"n":8}] | 일치. `n + this.m` = 5 + 3 |
| `arith-this-field-zero-able` | n = n + this.m where m may be 0 or negative | pass | WIRE.BAD_VALUE \| ok rows=[{"n":5}] | **check 통과 / 런타임 거부**. `this.m`의 범위가 0 이하를 허용해도 check는 통과하고, 값이 0 이하이면 `WIRE.BAD_VALUE`다. 같은 항을 `input`으로 쓰면 `DEF.BAD_RANGE`로 정적 거부된다. 핵심 파일 대기 C2 |
| `input-int-range-includes-zero` | transition input Int(0..5) | fail DEF.BAD_RANGE | - | 정적 거부(의도). input Int는 하한 1 이상의 range 필수 |
| `input-in-allow-compare-hidden` | input compared with a field-read-policy field in allow | fail DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION | - | 정적 거부(의도). 숨은 필드와 input 비교는 이분 탐색 오라클 |
| `compose-with-input` | compose step naming a transition with input | fail DEF.UNSUPPORTED | - | 정적 거부(의도). compose 단계는 input 전이를 못 쓴다(C0, P0-D) |
| `apply-where-target` | apply with target where on a filter the read exposes, and on one it does not | pass | tag=ok changed=1 other=WIRE.FILTER_NOT_ALLOWED | 일치. read가 filter로 연 필드만 `target where`에 쓸 수 있다(`WIRE.FILTER_NOT_ALLOWED`) |
| `apply-maxRows-over-bundle-cap` | expose apply bulk maxRows 5000 (bundle row cap is 1000) | pass | direct=ok bundle=WIRE.BULK_LIMIT | check 통과 / 런타임 부분 지원. `bulk maxRows 5000`은 직접 apply로는 되고 묶음 안에서는 행 상한 1000 때문에 `WIRE.BULK_LIMIT`다. 정적 경고 없음. 핵심 파일 대기 C5 |
| `enum-in-filter` | enum field with filter in and a transition assigning an enum literal | pass | ok rows=[{"id":"1","s":"A"}] | 일치.  |
| `filter-isNull-nullable` | filter isNull on a nullable Text | pass | ok rows=[{"id":"1","n":null}] | 일치.  |
| `filter-like-percent` | Text filter like with a literal percent and underscore | pass | contains(%)=1 prefix(50%)=1 like(50\%%)=1 contains(_)=3 | 일치. contains·prefix는 `%`·`_`를 리터럴로 취급한다 |
| `sort-bool-desc-null` | sort by a nullable Bool, ASC then DESC | pass | asc=3,2,1 desc=1,2,3 | 일치. ASC는 false, true, NULL 순. DESC는 NULL 먼저 |
| `row-policy-null-actor` | rows read when actor = null on a non-null actor reference | pass | check-only | check 통과(런타임 미실행). actor가 null일 수 있는 정책 모양. 의미 확인은 A의 범위 |
| `traverse-many` | 1:N traverse with limit and a child row policy | pass | ok rows=[{"id":"1","kids":[{"id":"1"},{"id":"3"}]}] | 일치. 자식 행 정책이 숨긴 행은 빠지고 limit 2가 적용된다 |
| `unique-with-null` | unique on a nullable column, two NULL rows | pass | nullx2=ok xx=WIRE.ALREADY_EXISTS | 일치. NULL은 서로 다르다(PG 의미). 같은 값은 `WIRE.ALREADY_EXISTS` |
| `delete-transition` | transition delete then read | pass | ok {"changed":["1"]} \| ok rows=[{"id":"2"}] | 일치.  |
| `latest-effect-null` | update effect with latest ... when no candidate row exists (no otherwise / otherwise this.created) | pass | ok {"changed":["1"]} \| ok rows=[{"created":"2024-01-01T00:00:00+00:00","label":null}] | 일치. 후보가 없으면 NULL 또는 `otherwise` 값 |
| `shared-primary-key-sort-id` | id: Member (1:1 shared primary key) with `sort id` | fail DEF.OP_TYPE_MISMATCH | - | **정적 거부 / 실행은 가능(의도 확인 필요)**. `sort id`가 `DEF.OP_TYPE_MISMATCH`다. 같은 열은 정렬 없이 읽고 쓰는 것이 된다(다음 행). 핵심 파일 대기 C3 |
| `shared-primary-key-no-sort` | the same resource without a sort list: read and patch work | pass | ok {"changed":["1"]} \| ok rows=[{"bio":"y","id":"1"}] | 일치. 정렬 목록 없이 읽기와 patch가 된다 |
| `sort-on-reference-field` | sort by a reference field | fail DEF.OP_TYPE_MISMATCH | - | 정적 거부(의도). 참조 필드는 정렬 타입이 아니다. C3와 같은 계열 |
| `filter-decimal-in` | Decimal filter with the in operator | fail DEF.OP_TYPE_MISMATCH | - | 정적 거부(의도). Decimal 필터는 eq gte lte gt lt |
| `select-same-field-twice` | the same field selected twice in one read | pass | WIRE.DUPLICATE | check 해당 없음. 같은 필드를 두 번 select하면 서버가 `WIRE.DUPLICATE`로 거절. TS는 컴파일 오류로도 막는다 |
| `time-now-policy` | rows read when t >= now | pass | ok ids=1 | 일치.  |
| `json-field` | Json field read | pass | ok rows=[{"id":"1","j":{"a":[1,2.5,null]}}] | 일치. JSON 값을 숫자 `2.50`→`2.5`로 정규화해 돌려준다(JSON 의미 그대로) |
| `email-url-date` | Email/Url/Date scalars round trip | pass | ok rows=[{"d":"2024-02-29","e":"a@b.co","id":"1","u":"https://x.y/z"}] | 일치. 윤일 `2024-02-29` |
| `check-and-invariant` | check clause and limit/invariant at commit | pass | ok,ok,WIRE.INVARIANT_VIOLATED | 일치. 셋째 행이 `WIRE.INVARIANT_VIOLATED` |
| `read-extension-undeclared` | read extension without access declarations | pass | check-only | check 통과(런타임 미실행). 읽기 확장 무선언 읽기는 E의 R1 결정 |

요약:

- **check 통과 / 런타임 항상 거부**: `cost-too-low`(C1), **check 통과 / 런타임 값에 따라 거부**: `arith-this-field-zero-able`(C2). 둘 다 `sema.rs` 정적 검사의 공백이다. 재현은 `semantic-definition-matrix.test.mjs`의 `todo` 시험이다.
- **check 통과 / 일부 경로 미지원**: `apply-maxRows-over-bundle-cap`(C5), `Int` 안전 정수 밖(D-DEC-1).
- **실행 가능한데 정적 거부**: `shared-primary-key-sort-id`(C3, 의도 확인 필요). 나머지 정적 거부는 문법 카드와 일치하는 의도된 거부다(Decimal min·max, 참조 정렬, cursor와 nullable 정렬, input 하한 등).
- `Core·Service 의미 매트릭스`(`plan-docs/reviews/2026-11-17-core-service-semantic-matrix.md`)의 "일반 read first/latest projection은 Service 미지원"은 `.aip` 문법에 그 표현이 없어서 정적으로 쓸 수 없다. 이번에 재검증하지 않았다(`미실행`).

## 3. D2: 직접 apply, bundle, compose, 확장 apply 의미 일치

시험: `product/tests/semantic-path-parity.test.mjs`(3개). `Order.confirm`(권한·상태·commit 시점 check·롤백) 네 경로, `Product.take/restock/edit`(input 산술, patch, expect, Decimal) 직접·묶음·확장 세 경로, compose의 input·patch·expect 거부. 열 가지 시나리오에서 오류 코드와 최종 DB 상태를 모두 단언했고, **DB 상태는 모든 경로에서 같다**(실패한 대상은 앞 대상까지 되돌려진다).

### 의도된 예외 계약

| 경로 | 같은 점 | 다른 점(의도) | 근거 |
|---|---|---|---|
| 직접 apply / bundle 안 apply / compose 단계 | 오류 코드 전부(`WIRE.FORBIDDEN`, `MISSING_TARGET`, `INVALID_STATE`, `CHECK_FAILED`, `BAD_VALUE`, `CONFLICT`), 롤백, 멱등 재생 | compose 단계는 `input`·`patch`·`expect` 전이를 못 쓴다(`service check`가 `DEF.UNSUPPORTED`). compose 안 compose 없음. 묶음 안에서는 compose와 apply를 섞을 수 있다 | `decisions.md` C0, 시험 3번째 |
| 쓰기 확장의 `ctx.data.apply` | 성공 결과(변경 행), DB 상태, 멱등 재생, commit 시점 `CHECK_FAILED`와 `CONFLICT` | 안쪽 `ctx.data` 거절(`FORBIDDEN`, `MISSING_TARGET`, `INVALID_STATE`, 값 범위 `BAD_VALUE`)은 호출자에게 `WIRE.EXTENSION_ERROR` 하나로만 보인다. 확장 **입력** 검증은 확장 자신의 선언 타입으로 `BAD_VALUE`가 된다. 업무 거절은 `ctx.reject`로 `WIRE.EXTENSION_REJECTED`+`appCode` | `spec/wire-errors.md` `WIRE.EXTENSION_ERROR`, `COMPATIBILITY.md`(`CONFLICT`·`ALREADY_EXISTS`는 그대로 전달) |
| 묶음 | 감사 행의 `via`가 `apply`다(직접과 구별되지 않는다) | — | 시험 D4. 관측성 제안 D-DEC-7 |

## 4. D3: SDK(TS·Python) 계약

### 발견한 결함과 RED→GREEN

| # | 결함 | 영향 | RED | GREEN |
|---|---|---|---|---|
| S1 | 응답 유실 뒤 재시도가 `WIRE.RATE_LIMITED`, `SERVER_BUSY`, `CIRCUIT_OPEN`, `DEPLOYMENT_CHANGED`, `SCHEMA_MISMATCH`를 받으면 최종 실패로 보고하고 키를 지운다. 서버는 이 코드를 멱등 기록을 보기 전에 돌려준다(`lib.rs:696` admission 429, `:1776·:1979` 배포 fence). 앞선 시도가 커밋됐을 수 있다 | 호출자가 새 키로 다시 보내 **이중 실행**(TS·Python 둘 다) | `unsettled-codes.test.ts` 5 fail, `python_sdk_contract_test.py` 5 subtest fail | 둘 다 통과 |
| S2 | Python은 키 없이 다시 부르면 새 UUID를 만든다(TS는 같은 본문의 미확정 키를 재사용) | `WriteUnsettled` 뒤 같은 요청을 다시 부르면 **이중 실행** | `test_calling_again_without_a_key_...` fail, PG17 시험의 `l09` | 통과 |
| S3 | TS가 묶음 결과의 `applied` 행을 apply 연산에만 맞춰 id 종류를 검사한다. compose는 단계마다 행을 더해 어긋나므로 **Uuid 자원의 compose + 다른 연산** 묶음은 커밋됐는데도 형식 오류로 보고 `WriteUnsettled`로 남는다(재시도도 같은 이유로 영영 미확정) | 커밋된 묶음이 영구 미확정, 호출자가 새 키로 다시 보내면 이중 실행 | `bundle-result-ids.test.ts` 1 fail, PG17 시험 `c05`(pending 비어 있지 않음) | 서버의 `ops[i].applied` 구간으로 행을 맞춤. 통과 |

수정 위치: `spikes/spike-v6-transport/client/transport.ts`(`NOT_EXECUTED`, `validWriteResponse`), `product/python/aip_sdk/__init__.py`(`_NOT_EXECUTED`, `apply`). 공개 API는 바뀌지 않았다(미확정이던 쓰기가 미확정으로 남고, 더 이상 오판하지 않는다).

### 시험이 확인하는 것

`product/tests/sdk-contract-semantics.test.mjs`(PG17, TS·Python 같은 여정 62단계, 두 언어 결과를 단계별로 비교): nullable·optional(생략은 유지, null은 비움, 필수 필드 null 거부), Decimal 정확성(0.01×3, 자릿수 초과, 컬럼 정밀도 초과, 0 이하), Id·Uuid(숫자 id를 Uuid 자원에, 형식 오류, 남의 행, Uuid 참조 읽기), input 산술(재고, 범위, 재생, 같은 key 다른 본문), refs와 `target.created`(원자 생성 후 확정·차감, 실패 시 전체 취소, 다른 묶음의 핸들), bundle·compose(+Uuid 혼합 묶음), 확장 결과(성공 출력, `appCode`, 입력 검증), 오류 코드와 `retry`·`requestId`, 응답 유실 복구, 계약 지문 불일치(read·apply·bundle·compose·확장). TS 스크립트는 `tsc --strict`로도 컴파일한다.

설계상 두 언어가 다른 단계(시험에 명시): 빈 묶음(TS는 `CLIENT.PROTOCOL_ERROR`, Python은 `WIRE.BAD_REQUEST`), 확장 입력 검사(TS는 전송 전에 예외, Python은 서버 결과), 확장 응답의 지문 필드, Python의 타입 있는 거절 객체. 이 차이를 바꾸면 공개 API가 깨지므로 건드리지 않고 D-DEC-4에 적었다.

### TS 컴파일 타임 거부

`product/tests/sdk-type-rejection.test.mjs`: 올바른 요청 모음은 `tsc --strict`를 통과하고, 잘못된 요청 57개는 각각 한 줄씩 컴파일 오류를 낸다(오류 줄이 사례 줄과 정확히 일치). DB가 필요 없다. 타입은 런타임 권한을 보장하지 않는다: 이 시험은 호출 모양만 본다.

개선(이전에는 컴파일되던 잘못된 요청):

| 요청 | 이전 | 이후 | 위치 |
|---|---|---|---|
| 빌더 `create`에 참조 필드가 아닌 키를 refs로 줌(`{ qty: handle }`) | 컴파일됨(서버가 거부) | 컴파일 오류 | `generic.ts` `NoExtraKeys` |
| 같은 필드를 select에 두 번(`["id","id"]`, 관계 두 번) | 컴파일됨(`WIRE.DUPLICATE`) | 컴파일 오류 | `generic.ts` `CheckSel` |
| 원시 `refs` create에서 참조로 채운 필드를 `values`에도 줌 | 컴파일됨(서버가 거부) | 컴파일 오류 | `generic.ts` `CreateOpWithRefs`(필드 조합 union) |
| 원시 `refs` create에서 필수 필드 누락·값 타입 오류 | 컴파일됨(`Partial`) | 컴파일 오류 | 같은 곳 |

시도했다 되돌린 방식: `bundle`을 `const R` 제네릭 오버로드로 바꿨더니 원시 요청의 초과 속성 검사가 사라져 기존 `v16-type-tests.ts`가 `@ts-expect-error 미사용`으로 실패했다(`v16_bundle` 시험). 제네릭 없는 원래 시그니처를 유지하고 `CreateOpWithRefs`만 참조 필드 조합 union으로 바꿨다(참조 필드 6개 이하, 넘으면 이전 `Partial`).

한계(컴파일 타임에 거부하지 못하는 것, 타입을 바꾸면 기존 호출이 깨지는 것은 D-DEC-4): 빈 `ids` 배열, Decimal·Id 문자열의 모양(`string` 브랜드), `limit`의 범위, 원시 묶음의 `{created: n}`이 가리키는 연산의 resource.

## 5. D4: 의미 기반 회귀(정의 → 실행 기록 → DB → SDK 응답)

`product/tests/semantic-chain.test.mjs`(3개). "실행 기록"은 `aip service audit list`의 감사 행이다(`via`, `transition`, `before`, `after`, `requestKey`).

| 시나리오 | 확인 |
|---|---|
| 재고는 정확히 한 번 줄어든다 | 같은 key 재전송 `replayed`, 5개 클라이언트가 같은 key로 동시에 호출해 1건만 실행(4건 재생), 서로 다른 key 8개가 재고 10에 qty 2 → 정확히 5건 성공, 재고 0, 음수 없음, 감사 행 5개. 묶음 안의 같은 전이, 실패한 묶음은 앞 연산까지 되돌려지고 감사 행·멱등 기록이 없다. 응답 유실 후 복구가 한 번만 줄인다 |
| 숨은 필드는 다른 필드로 복사되지 않는다 | `to summary = secret`, 숨은 필드를 `from`, `repeat unchanged`, 술어 안에서 쓰는 4가지 정의를 `service check`가 `DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`으로 거절한다. 허용된 정의에서 secret을 못 읽는 bob이 다른 필드를 써도 secret 값은 그대로이고 어느 열에도 복사되지 않으며 SDK 응답은 null, 감사 행에도 값이 없다 |
| 원자 생성 일부 실패 시 전체 취소 | 묶음(주문+줄+없는 상품을 가리키는 줄) → 앞의 create 둘도 취소, 감사 행·멱등 기록 없음, 같은 key로 고친 요청을 보내면 한 번만 만들어진다. 쓰기 확장(`ctx.data.create` 2회 뒤 예외) → 두 create 모두 취소 |

RED 확인(버그를 심어 시험이 실패하는지): 재고 가드 `from stock >= input.qty`를 `from true`로 바꾸면 첫 시험이 실패했다. 숨은 필드 정책 줄을 지우면 둘째 시험이 실패했다. 셋째 시험은 실패 단계를 성공 요청으로 바꿔 실패하는 것을 확인했다.

## 6. D5 실측: D6(쓰기 확장 `/apply` 헤더)와 D1 단독 집계 budget

`product/tests/semantic-wire-measure.test.mjs`(2개).

**D-06 분류: 실측으로 확정(코드 확인만 → 실행).** 계약 헤더가 없을 때와 틀릴 때의 코드:

| 요청 | 헤더 없음 | 대문자 16진수·짧은 값 | 다른 지문 |
|---|---|---|---|
| 전이 / 묶음 / 조합 / `/status` | `WIRE.CONTRACT_REQUIRED` | `WIRE.BAD_REQUEST` | `WIRE.CONTRACT_MISMATCH` |
| **쓰기 확장 `/apply`** | **`WIRE.CONTRACT_MISMATCH`** | `WIRE.BAD_REQUEST` | `WIRE.CONTRACT_MISMATCH` |

`product/SUPPORT.md`의 해당 줄을 "실행"으로 바꿨다. 코드 불일치는 `retry never`이고 두 SDK는 `CONTRACT_REQUIRED`·`CONTRACT_MISMATCH`를 같은 실행 전 거절로 취급한다(S1과 무관). 쓰기 확장 경로의 메시지는 "지문이 필요함"으로, 지문을 줬는데 다른 경우에도 같은 문구다. 코드를 `CONTRACT_REQUIRED`로 맞추는 일은 `write_extension.rs`·`lib.rs`(B 소유)와 공개 코드 변경 결정이 필요하다(C4, D-DEC-2). 헤더가 거절되면 멱등 기록이 남지 않음도 단언했다.

**D-01 단독 집계 budget: 고정 2초 실측 확정, 결정 필요 유지.** 원본 표를 `ACCESS EXCLUSIVE`로 잠근 상태에서: 정의가 `deadline 1s`인 `expose read`는 **1008 ms**에 `WIRE.DEADLINE_EXCEEDED`, 단독 집계(`expose aggregate`)는 **2007 ms**(여러 실행에서 2007~2011 ms)에 같은 코드다. 정의에서 단독 집계의 시간 한도를 정할 수 없다(`plan.rs:316,1097` `deadline_ms: 2000`, `lib.rs:242` `statement_timeout`). 오류 본문에는 한도 값이 없다. 선택지는 D-DEC-3.

## 7. 측정값 요약

| 항목 | 값 | 명령 |
|---|---|---|
| 읽기 `deadline 1s` 실측 | 1008 ms | `node --test --test-concurrency=1 product/tests/semantic-wire-measure.test.mjs` |
| 단독 집계 실측 | 2007 ms | 같은 시험 |
| 정의 조각 43개 check+런타임 | 3회 일치 | `node --test --test-concurrency=1 product/tests/semantic-definition-matrix.test.mjs` |

## 8. 처리 못한 항목과 이유

- 핵심 파일 수정이 필요한 6건은 9절. lead 허가 전이라 수정하지 않았다.
- 공개 계약을 바꾸는 항목은 `decisions/d.md`(D-DEC-1~7)에 선택지와 추천만 적고 확정하지 않았다.
- Python SDK는 bundle·compose의 정적 타입 helper가 없다(`SDK.md`가 P1로 명시). 추가는 가산 변경이지만 이번 범위에서 하지 않았다.
- `Int-range-bigint-bounds`, `row-policy-null-actor`, `read-extension-undeclared`는 check 결과만 기록했다(`미실행`: 런타임).
- 읽기 확장 TS SDK의 입력·출력 검증과 durable job 클라이언트는 이번에 다시 검사하지 않았다(기존 시험에 의존, `미실행`).

## 9. 핵심 파일 대기 목록

lead가 sema.rs·v3·v6 수정 허가를 보내면 이 순서로 고친다. 재현 시험은 이미 있다(`todo`는 지금 실패하고 고치면 통과한다).

| ID | 결함 | 파일(추정) | 재현 | 공개 의미 변경? |
|---|---|---|---|---|
| C1 | `budget cost`가 자기 읽기를 영원히 못 담는 정의를 check가 통과시킨다(`WIRE.COST_EXCEEDED`만 나온다) | `sema.rs` budget 검사 | `semantic-definition-matrix.test.mjs` "a budget cost the read can never fit..." (todo) | 아니오(더 일찍 거부, 기존 정의가 `service check`에서 새로 거부될 수 있음: COMPATIBILITY 항목 필요) |
| C2 | `to f = f + this.g`의 항 필드 범위가 0 이하를 허용해도 통과하고 런타임에 `WIRE.BAD_VALUE`. `input`은 `DEF.BAD_RANGE`로 정적 거부 | `sema.rs` 산술 항 검사 | 같은 파일 "a transition term from a field..." (todo) | 아니오(정적 거부 추가) |
| C3 | 공유 기본키(`id: Member`)의 `sort id`가 `DEF.OP_TYPE_MISMATCH` | `sema.rs` 정렬 가능 타입 | 같은 파일 "a shared primary key can be sorted by id..." (todo) | **의도 확인 필요**(허용하면 가산) |
| C4 | 쓰기 확장 `/apply`의 헤더 없음이 `CONTRACT_MISMATCH`이고 메시지가 "필요함" | `write_extension.rs`, `lib.rs`(B) | `semantic-wire-measure.test.mjs` 1번(현재 값 고정) | **예**(공개 코드 변경, D-DEC-2) |
| C5 | `bulk maxRows`가 묶음 행 상한(1000)보다 커도 정적 경고가 없고 묶음에서만 `WIRE.BULK_LIMIT` | `sema.rs` 또는 `v3 lib.rs` | 매트릭스 `apply-maxRows-over-bundle-cap`(현재 값 고정) | 경고 추가면 아니오 |
| C6 | `Int` 무범위가 int64를 받는다(TS SDK는 안전 정수 밖을 거부) | `sema.rs`·DDL | 매트릭스 `Int-unbounded-beyond-2^53` | **예**(D-DEC-1) |

## 10. F1에 연결 요청할 테스트

workflow는 수정하지 않았다. 아래를 `sdk-semantic-e2e.yml` 같은 PG17 job에 연결해 달라. 모두 `cargo build -p aip-cli` 뒤에 실행하고 DB 환경(`AIP_TEST_DATABASE_URL`, `AIP_TEST_ADMIN_URL`)이 필요하다. `sdk-type-rejection`과 TS 단위 시험은 DB가 필요 없다.

```
node --test --test-concurrency=1 product/tests/semantic-path-parity.test.mjs
node --test --test-concurrency=1 product/tests/sdk-contract-semantics.test.mjs
node --test --test-concurrency=1 product/tests/semantic-chain.test.mjs
node --test --test-concurrency=1 product/tests/semantic-wire-measure.test.mjs
node --test --test-concurrency=1 product/tests/semantic-definition-matrix.test.mjs
node --test product/tests/sdk-type-rejection.test.mjs
node --test spikes/spike-v6-transport/client/unsettled-codes.test.ts spikes/spike-v6-transport/client/bundle-result-ids.test.ts
python3 -m unittest product.tests.python_sdk_contract_test
```

`tools/verify.sh`의 `node --test` 목록(`:101-141`)에 마지막 두 줄과 `sdk-type-rejection`을 더하면 된다. 이 시험들은 `spikes/spike-0-ts`의 `npm ci`(tsc)와 `target/debug/aip`를 쓴다.

## 11. 검증 기록(마지막 실행)

아래는 이 브랜치 마지막 커밋 기준이다. 명령과 출력은 최종 보고에도 적었다.
