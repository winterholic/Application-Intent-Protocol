# `@aip/sdk` 배포 빌드 계약

이 문서는 운영 앱이 TypeScript SDK를 로컬 tarball로 설치할 수 있게 하는 빌더의 검수 기준이다. 공개 registry publish는 범위에 포함하지 않는다. 패키지 API는 기존 typed transport와 cache를 그대로 재사용하며 새 API 복제본이나 spike 소스 이동을 만들지 않는다.

## 8개 검수 관문

1. **공개 API**: `product/sdk/index.ts`는 `connect`, `WriteUnsettled`, generic SDK 타입과 durable contract 타입을 공개한다. `connect`는 base URL, 운영 JWT access token, 생성된 binding, 선택적 fetch 구현을 공통 typed transport에 전달한다. Durable descriptor가 있을 때 `jobs`, `files`, `events` client를 붙인다.
2. **동일 구현 재사용**: 빌드 입력은 제품 entry와 기존 spike의 TypeScript 의존 closure다. SDK 로직을 복사하거나 spike 원본을 이동·수정하지 않는다.
3. **완전한 산출물**: 로컬 TypeScript compiler로 JavaScript와 declaration을 함께 만들고, package에 필요한 runtime/type 의존 closure를 모두 담는다. 산출물의 상대 import가 package 밖으로 나가지 않는다.
4. **설치 가능한 패키지 메타데이터**: tarball은 이름 `@aip/sdk`, 버전 `0.1.0`, ESM 형식이며 root `exports`가 JavaScript와 TypeScript declaration을 가리킨다. 빌드는 `npm publish`를 실행하지 않는다.
5. **오프라인 소비자**: `npm pack --offline` 후 별도 소비자 디렉터리에서 `npm install --offline`이 가능하고, 설치된 root export를 Node가 import할 수 있다.
6. **운영 인증과 전송 통합**: ephemeral RSA key로 만든 JWT access token을 설치 SDK가 전달하고 서비스가 검증한다. 테스트는 세션 확인, typed read, 두 번째 read의 공통 cache hit, typed write, write 뒤 cache 무효화, 멱등 키 재생을 실제 service endpoint로 확인한다.
7. **엄격한 타입 공개**: 설치된 package declaration으로 NodeNext strict consumer가 통과하고, 계약에 없는 field/enum value를 사용하면 음성 대조 typecheck가 실패한다.
8. **출력 소유권과 실패 처리**: `--out <fresh-directory>`만 허용한다. 기존 디렉터리·파일·symlink를 거부하고 내용을 보존한다. 빌드 중 실패하면 이번 실행이 만든 출력 디렉터리만 정리한다.

운영 통합 확인은 임시 owned PostgreSQL schema와 임시 JWKS 파일을 사용한다. principal 폐기 뒤 같은 JWT가 `/session`에서 거부되는지 확인하고, 테스트가 초기화한 schema만 정리한다. JWT 개인키는 메모리에서 생성·사용하고 출력하거나 파일로 저장하지 않는다.

## 실행

저장소 루트 `aip/`에서 다음과 같이 실행한다.

```sh
npm ci --prefix spikes/spike-0-ts --ignore-scripts --no-audit --no-fund
node product/tools/build-sdk.mjs --out /tmp/aip-sdk-0.1.0
node --test product/tests/sdk-package.test.mjs
node --test product/tests/package-boundaries.test.mjs
cargo build -p aip-cli --locked
node --test product/tests/service-smoke.test.mjs
node product/tools/package.mjs
```

첫 명령은 lockfile에 기록된 로컬 compiler와 타입 의존성을 설치한다. SDK 소비자에게 이 빌드 의존성이 필요하지는 않다. Compiler가 없거나 실행할 수 없으면 builder는 출력 디렉터리를 만들기 전에 `SDK_TOOLCHAIN_MISSING`과 설치 명령을 반환한다. 초기 의존성 설치에는 registry 또는 준비된 npm cache가 필요하며, 설치된 도구를 사용하는 pack/install 검사는 offline으로 실행한다.

테스트는 임시 디렉터리에서 offline pack/install과 독립 소비자를 구성하고 종료 시 임시 파일을 제거한다. 빌드 결과물은 호출자가 소유하는 fresh output 디렉터리에만 기록된다.

`package.mjs`는 `product/dist/<UTC timestamp>/`를 새로 만들며 root `target/debug/aip`, 설치 가능한 SDK tarball, 비밀 없는 설정 예시, 정본 app/Node/Python extension 예제와 artifact/source SHA-256 manifest를 담는다. `--out <fresh-directory>`로 별도 위치를 지정할 수 있다. artifact README에는 worker 기본 off, `Event.confirm` opt-in, 현재 macOS `MacNetDeny` 제약을 적는다.

패키징은 시작 시 소스 목록과 해시, 기존 실행 파일의 해시를 기록한다. SDK 빌드와 파일 복사 뒤 입력을 다시 확인하고, 복사한 실행 파일·예시의 해시도 비교한다. 입력 추가·삭제·수정이나 실행 파일 교체를 감지하면 변경 경로와 `SOURCE_CHANGED`를 반환하고 이번 실행의 출력을 정리한다. 필수 소스나 Cargo manifest가 없으면 SDK 빌드 전에 `SOURCE_MISSING`을 반환한다. `package-boundaries.test.mjs`는 독립 임시 저장소에서 이 경계를 재현하며 공용 DB와 `target/`를 사용하지 않는다.

이 검사는 workspace 잠금이나 빌드 provenance를 제공하지 않는다. 검사 사이에서 변경됐다가 원복된 소스는 감지하지 못할 수 있으며, 기존 실행 파일이 현재 소스에서 빌드됐다는 보증은 별도 검증이 필요하다.

## resource 독립 operation

생성 binding에 operation이 있으면 같은 `connect`가 `.operation(name, input)`을 제공한다. `OperationBinding`과 `OperationDescriptors`를 package root에서 export한다. 입력·출력·범위·계약 지문과 세션 교체 검사를 공통 transport에 연결한다. 실제 문법·인증·DB 의존·실패 경계는 [EXECUTION](EXECUTION.md)을 따른다. 설치 SDK smoke는 Node/Python과 safe/decimal 조합에서 Unicode 계산과 범위 밖 입력 거부도 확인한다.

CLI나 compiler 변경 뒤 service smoke·패키징 전에 위 `cargo build`를 실행한다. package 도구는 기존 실행 파일을 복사하므로 SDK 소스만 최신이라고 해서 생성 계약도 최신인 것은 아니다.

## 읽기 질의 모양

TypeScript와 Python SDK는 같은 JSON 질의를 서버에 보낸다. TypeScript는 생성 binding이 모양을 타입으로 검사하고, Python은 `dict`를 그대로 보내므로 서버가 검사한다. 모양을 알면 `.d.ts`를 읽지 않아도 된다.

```ts
const page = await client.read({
  read: "TodoList",                                    // expose read가 있는 resource
  select: [
    "id", "title",                                     // 공개한 필드
    { owner: { select: ["id", "name"] } },             // N:1 관계: 관계 이름을 키로 하는 객체와 그 안의 select
    { items: { select: ["id", "done"], limit: 20 } },  // 1:N 관계: select와 limit(정의 상한 이하). 자식 안의 관계는 열리지 않는다
  ],
  filter: [{ field: "done", op: "eq", value: false }], // 항상 {field, op, value}의 배열. 정의의 `filter done.eq`로 연 연산만 쓴다
  sort: [{ field: "createdAt", dir: "desc" }],         // {field, dir}의 배열. dir은 "asc"(기본) 또는 "desc"이고 field는 `sort`로 연 것
  limit: 20,                                           // 정의의 budget rows 이하
});
page.rows;                                             // [{ id, title, owner: {...}, items: [...] }]
```

```python
rows = client.read({
    "read": "TodoList",
    "select": ["id", "title", {"owner": {"select": ["id", "name"]}}, {"items": {"select": ["id", "done"], "limit": 20}}],
    "filter": [{"field": "done", "op": "eq", "value": False}],
    "sort": [{"field": "createdAt", "dir": "desc"}],
    "limit": 20,
})   # Python은 rows 목록을 돌려준다(TypeScript는 {rows, cached, stale})
```

- 관계의 필드는 `select`에 객체로 넣는다(`{owner: {select: [...]}}`). `"owner.id"` 같은 점 표기는 필드 이름이 아니므로 `WIRE.FIELD_NOT_EXPOSED`다. 문자열 `"owner"`는 관계 필드 자체(참조한 행의 Id)다.
- `filter`는 객체가 아니라 `{field, op, value}`의 배열이다. 연산 이름(`eq`, `prefix`, `lte`, `in` 등)은 정의의 `filter <field>.<op>` 선언이 연 것만 쓸 수 있고, 열지 않은 필터는 `WIRE.FILTER_NOT_ALLOWED`다. 배열이 아닌 객체(`filter: {title: "al"}`)는 `WIRE.BAD_REQUEST`다. `in`의 값은 배열(상한 50), `isNull`의 값은 bool이다.
- 정렬 방향 키는 `direction`이 아니라 `dir`이다(`direction`은 `WIRE.UNKNOWN_KEY`).
- 다음 쪽은 `offset`(정의가 허용한 resource만) 또는 `after`(cursor를 연 resource만, 마지막 행의 id와 정렬 필드 값)로 연다.
- Id는 생성 binding의 wire를 따른다(`decimal-string-v13`은 문자열, `safe-number-v13`은 숫자).

## 익명 호출과 빈 토큰

로그인하지 않은 호출은 토큰 자리에 `null`(TypeScript) 또는 `None`(Python)을 준다. 서버의 `/session`은 그때 `{principal: {actorId: null}}`을 돌려주고, 읽기는 행 정책이 익명에게 허용한 행만 보인다.

```ts
const anonymous = connect(url, null, binding);   // 익명
const signedIn = connect(url, accessToken, binding);
```

빈 문자열(`""`, 공백만 있는 문자열)과 `undefined`(환경 변수가 설정되지 않은 경우)는 "토큰 없음"이 아니다. 두 SDK는 요청을 보내기 전에 `CLIENT.INVALID_TOKEN`으로 거절하고 메시지가 `null`/`None`을 쓰라고 말한다. 예전에는 서버의 세션 응답과 계약이 맞지 않는다는 `CLIENT.PROTOCOL_ERROR "세션 응답 계약 오류"`로 보여 원인이 가려졌다. `replaceSession`·`replace_session`에 빈 토큰을 줘도 같다. 근거 테스트: `spikes/spike-v6-transport/client/extension-rejection.test.ts`, `product/tests/python_sdk_test.py`의 `ExtensionAndTokenTest`.

## 쓰기 확장과 create 호출, 업무 거절

| 하는 일 | TypeScript | Python |
|---|---|---|
| 쓰기 확장 호출 | `client.writeExtension("Order.place", input, {key})` | `client.write_extension("Order.place", input, key=...)`, 생성 helper `write_order_place(client, input, key=...)` |
| 행 하나 만들기 | `client.bundle({atomic: [{create: "Task", values}]}, {key})` | `client.create("Task", values, key=...)`, 생성 helper `create_task(client, values, key=...)` |
| 여러 연산을 한 번에 | `client.bundle(...)`, `client.compose(...)` | `client.bundle(...)`, `client.compose(...)` |

Python의 `write_extension`과 `create`는 `apply`와 같은 키 복구를 쓴다. 응답이 유실되면 같은 key와 본문으로 다시 보내고, 끝내 확정하지 못하면 `WriteUnsettled`다. `create`는 만든 행의 id를 `result["created"][0]["id"]`로 돌려준다. `aip service gen --lang python`은 정의의 `expose create`마다 `<Resource>CreateValues` TypedDict와 `create_<resource>()`를, 공개한 쓰기 확장마다 `<Resource><Extension>WriteInput`·`WriteOutput` TypedDict와 `write_<resource>_<extension>()`을 만든다. `internal` 확장은 만들지 않는다. 생성기가 모르는 필드 타입은 `Any`로 둔다. 범위: 반환 값(`dict`)은 `apply_*` helper와 같이 정적 타입이 없고, 성공 결과의 `output`만 `WriteOutput`으로 문서화한다. TypeScript에는 `create` 전용 메서드가 없고 `bundle`로 만든다(두 SDK의 create 동작은 같다). 근거 테스트: `spikes/spike-v5-sdk/tests/python_create_write_helpers.rs`.

**업무 거절.** 쓰기 확장이 업무 규칙으로 호출을 거절하면 호출자는 `WIRE.EXTENSION_REJECTED`, 확장이 고른 업무 코드 `appCode`, 사용자에게 보일 문장 `msg`를 받는다. 확장 안에서는 `throw ctx.reject("INSUFFICIENT_STOCK", "재고가 부족합니다")`(Node) 또는 `raise ctx.reject("INSUFFICIENT_STOCK", "재고가 부족합니다")`(Python)를 쓴다. 코드 버그로 난 예외는 이 경로가 아니고 그대로 `WIRE.EXTENSION_ERROR`다. 자세한 규칙은 [관리형 실행](MANAGED-EXECUTION.md)의 "업무 거절" 절을 따른다.

```ts
const result = await client.writeExtension("Order.place", { id: "7" }, { key: "place-7" });
if (!result.ok && result.code === "WIRE.EXTENSION_REJECTED") {
  switch (result.appCode) {                        // 확장이 정한 코드로 분기한다
    case "INSUFFICIENT_STOCK": showMessage(result.msg); break;
  }
}
```

```python
from aip_sdk import extension_rejection

result = client.write_extension("Order.place", {"id": "7"}, key="place-7")
if (rejected := extension_rejection(result)):     # 업무 거절이면 ExtensionRejected, 아니면 None
    if rejected.app_code == "INSUFFICIENT_STOCK":
        show(str(rejected))                         # 확장이 준 문장
```

- 거절은 확정된 결과다. `WriteUnsettled`나 pending이 아니고 `retry`는 `never`다. 같은 요청은 업무 상태가 바뀌기 전까지 다시 거절된다.
- 쓰기는 모두 되돌려지고 멱등 기록은 남지 않는다. 같은 key로 다시 보내면 확장이 그때의 상태로 다시 판단한다(재고를 채운 뒤 같은 key로 성공할 수 있다). 거절 결과를 저장해 재생하지 않는다.
- TypeScript의 `WriteExtensionResult` 실패 변형에는 `appCode`와 개발 모드의 `cause`·`detail`이 있고, Python은 실패 결과 dict의 같은 키와 `ExtensionRejected`(`app_code`)·`AipError.app_code`가 있다.
- 확장이 실패했을 때 원인은 운영에서는 서버 로그 줄의 `cause`(`x-request-id`로 찾는다)에, 개발 모드 서버에서는 응답의 `cause`·`detail`에도 있다. 설정은 [관리형 실행](MANAGED-EXECUTION.md)의 "확장 실패의 원인" 절이다.

## 묶음(bundle)과 조합(compose)

서버가 공개한 동작(`expose apply` 전이, `expose create`, `expose compose`)을 호출자가 골라 한 요청에 묶어 원자적으로 실행한다. 일반 필드 수정은 전이에 명시한 patch 목록으로 열며 [일반·부분 수정](GENERAL-UPDATE.md)을 따른다. 선언 없는 update나 물리 delete는 열리지 않는다. 같은 `/apply` 경로를 쓰므로 멱등 키, 응답 유실 복구, `x-aip-contract` 지문 검사, 커밋 시점 `check`·불변식 재검사가 전이 하나와 같다. 전부 성공하거나 아무것도 바뀌지 않으며, 실패한 묶음은 멱등 결과를 남기지 않는다.

```ts
const done = await client.bundle({ atomic: [
  { create: "Task", values: { project: "10", author: "1", title: "write", points: 3 } },
  { apply: "Project.archive", target: { ids: ["10"] } },
] }, { key: "close-10" });
if (done.ok) done.created[0].id;           // 만든 행의 id. id 열이 없는 비공개 관계 테이블은 null
const composed = await client.compose({ compose: "Project", targets: { ids: ["11"] }, steps: [
  { transition: "archive" },
  { create: "Log", values: { owner: { item: "owner" }, name: { item: "name" } } },   // 값은 서버가 읽은 대상 행의 경로
] });
```

```python
done = client.bundle([{"create": "Task", "values": {"project": "10", "author": "1", "title": "write", "points": 3}},
                      {"apply": "Project.archive", "target": {"ids": ["10"]}}], key="close-10")
composed = client.compose("Project", ["11"], [{"transition": "archive"},
                          {"create": "Log", "values": {"owner": {"item": "owner"}, "name": {"item": "name"}}}])
```

| 항목 | 규칙 |
|---|---|
| 요청 | `{atomic:[{apply,target}\|{create,values}]}`(묶음), `{compose,targets:{ids},steps}`(조합). 생성 계약에 없는 create·필드·전이·값 출처는 타입에서 막히고 서버도 `WIRE.NOT_EXPOSED`·`WIRE.FIELD_NOT_EXPOSED`·`WIRE.STEP_NOT_ALLOWED`·`WIRE.VALUE_SOURCE_NOT_ALLOWED`로 거절한다 |
| 결과 | `{ok:true, applied:[{changed,unchanged}], created:[{resource,id}], tags}`. 같은 키 재생은 `replayed:true`와 저장된 `created`를 돌려주고 행을 다시 만들지 않는다 |
| 크기 상한 | 묶음 연산 20개, 묶음·조합이 건드리는 대상 행 1000개, 조합 단계 10개. 넘으면 `WIRE.BULK_LIMIT`. 값은 생성 계약의 `bundleLimits`에도 실린다 |
| 참조 값 | create의 Ref 값은 호출자가 읽을 수 있는 행이어야 한다(D-A17). 안 보이는 행은 없는 행과 같은 `WIRE.MISSING_TARGET`이다 |
| 타입 | TypeScript는 생성 binding의 `CreateContract`·`ComposeContract`로 값·id·`item` 출처를 검사한다. Python SDK는 정적 타입이 없고 서버가 검사한다. 성공 결과의 모양이 틀리면 둘 다 확정으로 보지 않고 같은 키로 다시 확인한다 |
| 한계 | 계약이 create·compose를 공개하지 않고 전이만 있으면 `retryPending()` 결과 타입은 전이 하나의 것 그대로다. 그 계약에서 `bundle()`을 쓰면 pending 결과를 `applied` 유무로 좁혀 읽는다. 연산 사이로 값을 넘기는 일반 조합은 없다(`compose`의 `item`만). create는 Url·Time 필드를 받지 않는다 |

## 계약 헤더와 실패 응답

생성 TypeScript binding과 Python SDK는 계약을 사용하는 요청에 `x-aip-contract`를 보낸다. `/read`, `/apply`, `/status`에서 헤더가 없으면 `WIRE.CONTRACT_REQUIRED`, 서버 지문과 다르면 `WIRE.CONTRACT_MISMATCH`다. 인증 검사가 먼저 실행된다. 직접 HTTP를 호출하는 클라이언트는 `/capabilities`에서 배포된 지문을 얻어 헤더에 넣는다. 지문을 고정한 클라이언트는 배포가 바뀌면 binding을 다시 생성해야 한다.

JSON 실패 본문은 `requestId`와 `retry`를 싣는다. `requestId`는 응답 헤더 `x-request-id`와 같으며, 운영자는 같은 ID의 요청 로그를 찾는다. 성공 본문에는 이 두 필드를 추가하지 않는다. 확정된 `/read` 거부는 TypeScript와 Python SDK에서 코드가 포함된 예외로 전달되지만, 확정된 `/apply` 거부는 두 SDK 모두 `{ok: false, code: "WIRE...", ...}` 결과 객체로 반환한다. `WIRE.COMMIT_UNKNOWN` 같은 쓰기 결과 미확정은 `WriteUnsettled`로 전달되며, 거부 결과와 구분한다. TypeScript 오류와 실패 결과, Python 예외와 실패 결과는 서버의 재시도 지침을 보존한다. Python 예외의 요청 ID 속성은 `request_id`다.

| `retry` | 호출자가 할 일 |
|---|---|
| `never` | 입력·설정·계약을 고친 뒤 다시 호출한다 |
| `later` | 잠시 기다린 뒤 같은 요청을 보낸다. 쓰기는 같은 멱등 키를 유지한다 |
| `check-first` | 실제 효과나 저장된 결과를 확인한다. 미확정 쓰기는 새 키를 발급하지 않고 같은 키로 결과를 다시 확인한다 |
| `reauthenticate` | 유효한 토큰을 얻어 다시 호출한다 |

`WriteUnsettled`에는 마지막 서버 응답의 요청 ID와 재시도 지침이 남는다. 네트워크 오류만 있었으면 이 정보가 없을 수 있다. 재시도 지침은 커밋 여부를 보증하지 않는다. SDK의 멱등 키·미확정 상태 관리가 계속 적용된다. SDK는 목록 밖의 `retry`를 노출하지 않으며, 본문에 유효한 요청 ID가 없을 때는 응답 헤더의 8~64자 안전 문자(`A-Za-z0-9._-`) 값을 사용한다.

근거 테스트: `spikes/spike-v6-transport/tests/error_body.rs`, `tests/v13_headers.rs`, `client/request-id.test.ts`, `product/tests/python_sdk_test.py`.

## durable Job, file, event API

`operation job`이 생성 binding에 있으면 `connect` 결과가 `jobs`, `files`, `events`를 제공한다. API 호출 예시와 제한, 오류 경계는 [관리형 durable execution](MANAGED-EXECUTION.md)을 따른다.

```ts
if (!client.jobs || !client.files || !client.events) throw new Error("durable binding이 필요합니다");
const receipt = await client.jobs.enqueue("report", input, { key: "report-42" });
const status = await client.jobs.status(receipt.id);
if (status.error) console.error(status.error);
if (status.status === "SUCCEEDED") {
  const output = await client.jobs.result(receipt.id, "report");
}
const stored = await client.files.put("reports", "report.txt", bytes, "text/plain");
const file = await client.files.get(stored.id);
for await (const event of client.events.watch("notifications", { after: "0", signal })) {
  consume(event.cursor, event.payload);
}
```

SDK surface에는 `jobs.enqueue/status/cancel/result`, `files.put/get/delete`, `events.read/watch`가 있다. `watch`는 polling async iterator이며 SSE가 아니다. 앱 SDK는 outbound HTTP broker를 직접 노출하지 않는다. 등록 endpoint 호출은 서버 worker의 `ctx.http.call(name,input,key)`만 가능하다.

`JobReceipt.error`는 선택 필드다. 실행 중 cancel은 우선 `CANCEL_REQUESTED`이며, unresolved HTTP effect가 남으면 `FAILED`와 `WIRE.CANCELLATION_UNCONFIRMED`로 표시한다. status 응답에 `WIRE.EFFECT_UNKNOWN`이 있으면 외부 provider 처리 여부가 불확실한 상태다. 현재 owner ACL과 권한 변경의 효과는 [managed execution 경계](MANAGED-EXECUTION.md)에 설명했다.

## Python 호출 SDK

`product/python/aip_sdk`는 Python 백엔드·배치·AI agent가 AIP 서비스를 호출하는 공식 SDK다. 표준 라이브러리만 쓰며 Python 3.9 이상에서 동작한다. 서버 쪽 Python 확장(worker의 `ctx`)과는 다른 패키지다.

```python
from aip_sdk import AipError, WriteUnsettled, connect

# 생성 binding의 `export const contractFingerprint = "..."` 값을 고정하면 배포가 바뀐 것을 감지한다.
client = connect("https://api.example.com", access_token, contract_fingerprint)
identity = client.session()  # {"actorId": "..."}; verified by the AIP server, not inferred from token claims
rows = client.read({"read": "Event", "select": ["id", "checked"]})
try:
    result = client.apply({"apply": "Event.mark", "target": {"ids": ["11"]}}, key="mark-11")
except WriteUnsettled as pending:
    client.retry_pending()  # 같은 key와 본문으로 다시 확정
length = client.operation("textLength", {"text": "가😀A"})["length"]
job = client.jobs.enqueue("report", {"text": "hello"}, key="report-42")
run = client.jobs.wait(job["id"])
```

익명 클라이언트는 토큰에 `None`을 준다. 빈 문자열은 `CLIENT.INVALID_TOKEN`으로 거절된다("익명 호출과 빈 토큰" 절).

`session()` validates the access token through `/session` and returns the already bound AIP actor id. An anonymous client returns `{"actorId": null}`. It never creates or binds an actor; resource reads and writes still pass through normal server authorization.

The official TypeScript SDK exposes the same `client.session()` check and returns `{ actorId: string | null }`; the result is cached for the current token generation and is revalidated after `replaceSession()`.

| 기능 | TypeScript SDK와의 관계 |
|---|---|
| 계약 지문 | 고정한 지문을 `x-aip-contract`로 보내고 응답 지문과 비교한다. 지문을 주지 않으면 `/capabilities`가 알려 준 값을 쓴다. 이 경우 배포 변경 감지는 고정했을 때만 의미가 있다 |
| 쓰기 복구 | 통신 실패·`WIRE.COMMIT_UNKNOWN`·`WIRE.INTERNAL`이면 같은 key·같은 본문으로 다시 보낸다. 재시도에서 실행 전 거부를 받으면 확정으로 보지 않고 `WriteUnsettled`를 던진다. 같은 key에 다른 본문은 `WIRE.IDEMPOTENCY_MISMATCH` |
| 세션 교체 | `replace_session(token)`은 같은 principal일 때만 바꾸고 미확정 쓰기를 다시 확정한다. 다른 사용자면 `ScopeConflict` |
| 쓰기 확장·create | `client.write_extension(name, input, key=...)`, `client.create(resource, values, key=...)`. TypeScript의 `writeExtension`·`bundle`과 같은 키 복구를 쓴다. 업무 거절은 `extension_rejection(result)`로 읽는다. 위 "쓰기 확장과 create 호출, 업무 거절" 절 |
| 관리 API | `jobs.enqueue/status/result/cancel/wait`, `files.put/get/delete`, `events.read/watch`. SSE·WebSocket helper는 없다 |
| 타입 | `service gen --lang python`은 read Row/ReadResult, apply 입력·helper, create 값·helper, 쓰기 확장 입력·출력·helper, operation 입력·출력·helper 및 JobState/JobRun 타입을 생성한다. read query와 일반 `Client.operation()`은 동적 API다. 생성된 `call_*`와 capability를 발견한 동적 `Client.operation()`은 공통 scalar descriptor로 반환 객체의 키·타입·범위를 검사한 뒤 TypedDict 또는 dict로 돌려준다. nullable 출력도 키가 있고 값이 None이다. 계약 불일치는 `CLIENT.PROTOCOL_ERROR`이며 값 자체는 오류에 싣지 않는다 |
| 캐시 | read 캐시가 없다 |

설치는 `pip install ./product/python`이다(PyPI 게시는 아직 없음). 새 binding과 같은 revision의 SDK를 함께 설치한다. 클린 wheel 빌드·설치와 타입 검사는 `python3 -W error::ResourceWarning -m unittest product.tests.python_sdk_test`, 실제 서비스 검증은 `node --test product/tests/service-smoke.test.mjs`다. 타입 검사는 mypy 2.4.0 또는 uv가 필요하며 도구가 없으면 실패한다. smoke는 TypeScript 소비자가 커밋한 쓰기를 Python 소비자가 같은 key로 재생하는지까지 확인한다.


## UUID Id

`id: Uuid` resource와 그 Ref/X.Id는 safe/decimal 모두 정규 UUID 문자열이다. TypeScript는 리소스별 Id 조건부 타입, Python `gen --lang python`은 NewType(str)와 선택형 Row 타입을 생성한다. read/filter/after/create/apply/수정 입력·pending 복구에 같은 값을 전달한다. 숫자 Id의 V13 규칙은 그대로다. 잘못된 종류의 성공응답은 pending으로 보존한다. [UUID·실 DB 채택 검증](../plan-docs/uuid-adoption/DESIGN.md)을 참고한다.

## Aggregate authorization contract

An aggregate is a separately exposed capability. `sourceAccess` determines which source rows the aggregate may read; the source resource's ordinary `rows read` policy is not uniformly inherited by every measure. The current service contract is intentionally asymmetric:

- `count` counts the source rows selected by `sourceAccess` and the aggregate's declared `where`. It does **not** apply source `rows read`. Treat a count as an explicit disclosure: callers may infer changes in that declared population. Use a restrictive `sourceAccess` and/or `where` when the complete population must not be public.
- `sum`, `min`, and `max` apply source `rows read` as a SQL `FILTER`, in addition to `sourceAccess` and `where`. Their values therefore ignore rows hidden by the source resource's row policy. Standalone aggregate calls use this same measure path.
- `groupKey` scopes a per-row aggregate. `callerFilter none` rejects a request filter; `callerFilter read` allows only the declared filter capability. A caller filter is not a substitute for the server's `sourceAccess` or `where`.

These rules define the noninterference boundary. Hidden source-row changes leave caller-scoped counts, filtered value aggregates, ordinary reads, and traversed relations unchanged when their declared predicates exclude those rows. A count intentionally authorized over a broader population is an explicit public exception: changes to that population can change the count. Field-hidden values remain unavailable for filtering and sorting, and rejection responses do not disclose the value or matching rows. The denial code/message/retry payload stays the same as hidden values change; `requestId` is intentionally unique correlation metadata for each request.

For example, the service policy regression declares `ownVoteCount` with `sourceAccess callerVotes(actor)` and `where owner = actor`, but no source `rows read` filter is added to count. `publicVoteCount` has no `where`, so it counts every source vote permitted by `sourceAccess`. `minOwnVote` and `maxOwnVote` omit `where`; their source Vote `rows read when owner = actor` is applied by the measure. The tests compare both same-organization and cross-organization hidden vote changes, including the full `votes` relation response.

### Standalone counts in FaceReview

The repository's standalone count call sites include `Record.publicVoteCount` in `crates/aip-cli/tests/service_flow.rs`; its declaration deliberately has no `where` and `callerFilter none`, and the test asserts that it counts all rows authorized by `sourceAccess`. This is the explicit broad-count example and public exception in the policy regression.

The FaceReview product extension calls `User.totalCount` and `VideoRequest.pendingCount` without a request filter in `extensions/slice14h_dashboard.py`. Both declarations have an aggregate `where`; `User.totalCount` uses `callerFilter read` and `VideoRequest.pendingCount` uses `callerFilter none`. `extensions/slice13b_lists.py` does not call a standalone total: its comment records that it pages IDs because that extension cannot supply a caller filter to `Video.totalCount`. The `slice13e` reference oracle calls `User.totalCount`, `VideoRequest.totalCount`, `Video.totalCount`, and `Comment.totalCount`, whose declarations each have explicit admin/source `where` and `callerFilter read`; oracle calls include both filtered totals and a few unfiltered requests.

FaceReview has no identified standalone count call whose declaration omits both `where` and `callerFilter`. Its grouped per-row `User.watchCount`/`commentCount` and `Video.likeCount` intentionally omit `where` and `callerFilter none`; they are selected in reads rather than called standalone and deliberately expose the counts allowed by their broad `sourceAccess`.

Changing `count` to inherit source `rows read` by default would alter the established API contract for the broad `Record.publicVoteCount` example and grouped FaceReview `watchCount`, `commentCount`, and `likeCount`, and could silently reduce existing totals or conflict with an aggregate `where` that already defines the intended population. This change is not made here. Prefer explicit `sourceAccess`, `where`, and `callerFilter` declarations; a future fail-closed default would require migration diagnostics and review of all aggregate declarations and callers before rollout.
