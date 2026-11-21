# 일반·부분 수정

메모 제목·본문처럼 호출자가 준 값으로 바꾸는 CRUD update는 공개 전이에 `patch`를 선언한다. 기존 고정 `to` 전이는 그대로 쓴다. 한 번의 수정에는 타입·검증·행 인가·행 잠금·고정 전이·effect·notify·audit·commit 검사·멱등 결과가 같은 transaction에 들어간다.

```aip
resource Member { fields { id: Id } }
actor Member
resource Note {
  fields { id: Id; owner: Member; title: Text(1..200)?; content: Text?; version: Int(0..1000000); deleted: Bool }
  rows read when owner = actor
  transition edit {
    allow owner = actor
    from deleted = false
    patch title, content
    expect version
    to version = version + 1
    notify owner "note.edited"
  }
  expose apply edit { target id; bulk maxRows 1 }
}
```

`transition`에는 `patch`만 있어도 `from`과 `allow`가 항상 필수이며 빠지면 `DEF.MISSING_ITEM`이다. `from`을 생략해 모든 행을 허용하는 기본값은 없다. 조건 없이 허용하려면 `from true`를 쓴다.

`patch`에 없는 필드는 호출자가 수정할 수 없다. 필드별 타입·Text 길이(Unicode 코드 포인트)·Int 범위·Email·Date·Time·Url·Enum·정밀 Decimal은 기존 `fields` 선언에서 얻는다. Ref 수정은 생성과 같은 대상 행 가시성을 확인한다. Id·Ref 입력은 설정된 Id wire를 사용하며 JSON 값은 SQL 매개변수로 전달한다.

```ts
import { connect } from "@aip/sdk";
import { contract } from "./notes.binding.js";
const client = connect(url, token, contract);
const result = await client.apply({
  apply: "Note.edit", target: { ids: [note.id] },
  values: { title: "새 제목", content: "새 본문" },
  expect: { version: note.version },
}, { key: "edit-note-42-v3" });
```

values는 비어 있지 않은 객체다. `{title:"새 제목"}`이면 content는 유지한다. `{content:null}`은 nullable content를 비운다. undefined인 값 키는 JSON에서 누락되어 해당 필드를 유지하지만, 모든 키가 빠져 values가 비면 요청을 거부한다. 필수 필드에 null, 모르는 키, 허용 밖 필드, 잘못된 타입·길이·범위는 거부한다. 기본키와 고정 `to` 대상은 patch에 넣을 수 없다.

권장 동시성 방식은 `expect version` + 서버의 `to version = version + 1`이다. 생성 시 version 초기값(예: 0)도 정의의 create 필드와 호출 입력에 둔다. `expect title, content`처럼 기존 scalar 값으로 조건부 갱신할 수도 있다. expect는 선언된 키를 전부 요구하며 null도 NULL-safe 비교한다. `expect`를 생략한 **정의**는 행 잠금 안에서 마지막 요청 값이 이기는 일반 update 방식이다. expect가 선언된 action의 **요청**에서 expect를 생략하는 것은 오류다. version은 편집에 참여할 모든 action에서 증감시켜야 그 action과도 충돌을 감지한다.

rows read와 allow는 변경 전 행으로 평가한다. 안 보이는 행은 `WIRE.MISSING_TARGET`, 보이는 행에서 allow 거부는 `WIRE.FORBIDDEN`이다. 인가 후 잠근 행의 기대 값이 다르면 `WIRE.CONFLICT`로 transaction 전체가 rollback된다. 실제 이전 값은 오류에 싣지 않는다. `field read` 정책 필드와 Ref는 expect 대상에서 거부해 숨은 값 비교 통로를 막는다. `repeat unchanged`는 patch/expect와 조합하지 않는다. 동일 요청의 재시도는 같은 멱등 key로 저장 결과를 재생한다.

확정 conflict 뒤에는 최신 read로 다시 확인하고 수정된 values/expect에 **새 key**를 사용한다. 같은 본문만 다시 보낼 때는 같은 key를 쓴다. 통신 실패·`WIRE.COMMIT_UNKNOWN`·잘못된 성공 응답은 기존 SDK의 pending으로 남으며 원래 key·원래 본문으로 결과를 먼저 확정한다. 수정 때문에 이 복구 규칙이 바뀌지 않는다.

여러 수정이나 생성·고정 전이를 원자적으로 묶을 때는 `client.bundle({atomic:[{apply,target,values,expect,input}, ...]}, {key})`를 사용한다. 앞 create가 만든 행을 뒤 연산에서 쓰는 `refs`·`target.created`와 `compose` 연산을 같은 묶음에 넣는 방법은 [SDK 묶음과 조합](SDK.md#묶음bundle과-조합compose)에 있다. `compose`의 전이 단계에는 입력 값 슬롯이 없어 `patch`·`expect`·`input`이 있는 전이를 넣으면 정의 오류(`DEF.UNSUPPORTED`)다. compose 단계 안쪽에 값을 공급하는 대신, 같은 묶음의 별도 `apply` 연산에 `values`·`expect`·`input`을 넣는다. 새 update endpoint는 없다.

## 입력량으로 증감(`input`과 제한 산술)

재고 차감처럼 호출자가 정한 양만큼 숫자 필드를 늘리거나 줄일 때는 호출자가 새 값을 계산해 `patch`로 보내지 않는다(읽고 계산해 쓰는 사이의 갱신 유실과 호출자 신뢰 문제). 전이에 `input` 블록을 선언하고 자기 필드에 항 하나만 더하거나 뺀다.

```aip
resource Product {
  fields { id: Id; sku: Text(1..40); stock: Int(0..1000000); price: Int(0..1000000) }
  rows read when actor != null
  transition take {
    input { qty: Int(1..1000) }
    allow actor != null
    from stock >= input.qty
    to stock = stock - input.qty
  }
  expose apply take { target id; bulk maxRows 1 }
}
```

```ts
await client.apply({ apply: "Product.take", target: { ids: [productId] }, input: { qty: 3 } }, { key: "take-1" });
```

규칙은 다음과 같다. 정의 문법의 전체 표는 [정의 문법](../spec/definition-syntax.md)이다.

- 대입은 `f = f + 항`과 `f = f - 항` 두 형태뿐이다. 항은 하나이며 양수 정수 리터럴, 양수 Decimal 문자열 리터럴(`"0.50"`), `input.p`, 같은 행의 non-null 숫자 필드(`this.g`) 중 하나다. 곱셈·나눗셈·함수·두 항 이상(`f = f + a - b`)은 없다. `to f = input.p`처럼 입력을 그대로 대입하는 형태는 `DEF.UNSUPPORTED`다(값을 그대로 쓰려면 `patch`).
- 같은 대입을 효과 `update ... { f = f - input.qty }`의 값으로도 쓴다. 이때 앞의 `f`는 효과 대상 행의 현재 값이고, 항의 이름과 `this.`는 전이 대상 행이다.
- `input`의 매개변수는 Int 또는 Decimal이고 null을 받지 않으며 8개 이하다. 선언한 키는 요청에 모두 있어야 한다. Int는 하한 1 이상인 range가 필요하다(없거나 미만이면 `DEF.BAD_RANGE`). 항이 0 이하가 되는 요청은 서버가 DB에 닿기 전에 `WIRE.BAD_VALUE`로 거절한다. 0 정수 리터럴, 0 이하 Decimal 리터럴은 `DEF.ARITH_NOT_ALLOWED`다.
- 타입: Int 필드에는 Int 항, `Decimal(p,s)` 필드에는 소수 자릿수가 s 이하인 Decimal 항이나 Int 항이다. Decimal 항을 Int 필드에 쓰면 `DEF.TYPE_MISMATCH`다. 대상 필드와 항은 non-null이어야 한다.
- `from`·`allow`에서는 `input.p`와 비교할 수 있다(`from stock >= input.qty`). 읽기 정책이 있는 필드와 `input.p`를 한 비교식에 쓰면 숨은 값 비교 통로가 되므로 `DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`이다. `repeat unchanged`와는 함께 쓸 수 없다(`DEF.UNSUPPORTED`).
- 계산은 서버가 잠근 행에서 같은 `UPDATE` 문으로 한다. 동시 차감은 행 잠금으로 직렬화되고 `from`은 잠근 뒤 판정하므로 음수가 되지 않는다. 결과가 필드의 Int range나 Decimal 자릿수를 벗어나거나 실행 시점의 값이 NULL이면 `WIRE.BAD_VALUE`이고 트랜잭션 전체가 롤백된다(감싸기·포화는 없다). 효과로 줄이는 Decimal 대상에 하한 `check`가 없으면 `DEF.ADVISORY_EFFECT_NO_LOWER_BOUND` 조언이 나온다.
- 요청의 `input`에 모르는 키는 `WIRE.FIELD_NOT_EXPOSED`, `input`을 선언하지 않은 전이에 보내면 `WIRE.UNKNOWN_KEY`다. 같은 key 재전송은 다시 계산하지 않는다(이중 차감 없음). `input`은 `bundle`의 `apply` 연산과 PostgreSQL 쓰기 확장의 `ctx.data.apply`(일곱 번째 인자)도 받는다.
- 생성 계약의 `input` 타입(range 주석 포함)은 TypeScript `ApplyRequest`와 Python 생성 helper의 `input=` 키워드에 나온다. `input`을 선언한 전이가 있는 정의의 계약 지문은 바뀌지만, 선언하지 않은 기존 정의의 facts·지문·digest는 그대로다. 구서버는 `input` 키를 `WIRE.UNKNOWN_KEY`로 거절한다.
- PostgreSQL 전용이다. MySQL 쓰기는 `input`·제한 산술을 `WIRE.UNSUPPORTED`로 거절한다.

호환 경계 전체는 [COMPATIBILITY](../COMPATIBILITY.md#표현력-변경의-호환-경계)에, 실행 근거(주문·재고 앱의 `take`)는 [소비자 앱 실행 기록](../plan-docs/expressiveness-1.0/consumer-apps.md)에 있다. 곱셈이 필요한 계산(`Σ 수량×단가`)은 선언으로 표현하지 못하며 [표현력 한계 E3](LIMITS.md#e3-계산-집계-불가)의 확장 경로를 쓴다.

다섯 작성 형식은 동일하다. 객체형 TS/Python은 `transitions: {edit: {from:"deleted = false", allow:"owner = actor", patch:["title","content"], expect:["version"], to:"version = version + 1"}}`를 쓴다. 다른 host 선언 구조는 기존과 같다. 독립 `.aip`와 TS/Python 내 선언 블록도 위 문법을 그대로 사용한다.

```sh
aip service gen notes.aip --out notes.binding.ts
aip service gen notes.aip --lang python --out notes_binding.py
```

```python
from aip_sdk import connect
from notes_binding import CONTRACT_FINGERPRINT, NoteId, apply_note_edit
client = connect(url, token, CONTRACT_FINGERPRINT)
result = apply_note_edit(client, {"ids": [NoteId(note_id)]},
                         {"title": "새 제목", "content": None},
                         {"version": 3}, key="edit-note-42-v3")
```

Python 3.11 이상에서 생성 TypedDict의 values 필드는 `NotRequired`이고 nullable 값은 `T | None`이다. expect의 필드는 필수다. helper는 설치형 Python SDK의 apply에 위임한다. 현재 Python 생성은 공개 apply 입력과 helper만 제공한다(전체 read/operation/job의 정적 binding은 별도 범위). TypeScript는 같은 `connect`·`apply`·`bundle` API를 쓴다.

수정 허용 목록·조건 목록·입력 타입·nullable·range는 공개 계약 지문에 들어간다. 기존 action은 새 선언이 없으면 이전 생성 계약과 같다. 일반 migration은 transitions 변경을 `SecurityReview`로 표시하며 필드 타입·range 변경도 기존 비교에 포함한다. 같은 지문이 정책의 동등성을 뜻하지는 않는다. allow/from/effect 정책 변경은 배포 facts 지문으로 fence한다.

검증은 `cargo test --manifest-path spikes/spike-v3-write/Cargo.toml --test general_update --target-dir target`와 `node --test product/tests/general-update.test.mjs`다. 제한 산술은 `spikes/spike-v1-fixture/tests/expr_arith.rs`(정의 검사)와 `spikes/spike-v3-write/tests/expr_arith.rs`(PostgreSQL 실행)가 다룬다. 이 문서를 갱신하며 두 시험을 다시 실행하지는 않았다. 후자는 포팅 notes 복사본을 새 schema에서 초기화하고 실제 JWT·설치 SDK·Python 생성 helper·커밋 응답 유실·계약 불일치를 검증한다. 원본 Memoir와 기존 포팅 schema는 수정하지 않는다.
