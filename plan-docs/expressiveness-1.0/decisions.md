# P0 결정표 (기준 4e132564)

상태: 설계 확정안(소유자 승인 전). 범용 언어화(무제한 식, 루프, 사용자 정의 함수)는 모든 항목에서 기각한다. 각 결정은 [PRINCIPLES §4](../../docs/PRINCIPLES.md) 관문 1~8에 아래 한 줄로 답한다. 공통 답: 원칙 1·2를 실현하고(서버 코드 없이 흔한 쓰기를 선언), 호출자 표현 범위는 서버가 선언한 슬롯 안에서만 넓어지며, 서버 최종 권한·정책 평가는 그대로다. 같은 의도를 두 방식으로 쓰지 않도록 각 항목에 "유일 경로"를 적었다.

## 0. 공통 의미 계약 (A~D가 함께 지킨다)

| ID | 계약 | 재사용 코드 |
| --- | --- | --- |
| C0-1 값 출처 | 쓰기에 들어가는 값은 다섯 출처뿐이다. `Lit`(호출자 리터럴, fields 타입·range로 검증), `Null`, `Item`(서버가 읽은 대상 행 경로, 기존 compose), **`Created(i)`**(P0-A, 같은 묶음 앞 연산이 만든 행의 id), **`Arith`**(P0-B, 잠근 행 값 ± 한 항). `Created(i)`는 `bundle_ops` 판정 뒤 실행 시점에 `Src::Lit(id, cast)`로 해석해 기존 경로에 넣고, `Arith`는 전이 facts의 대입 형태로만 존재한다. 다른 출처를 새로 만들지 않는다 | `spike-v3-write/src/lib.rs:1065` `enum Src`, `create_row`(:1119), `input_sql`(:1439) |
| C0-2 타입 | 대입은 `same_target` 규칙을 따른다. nullable 값은 non-null 필드에 못 들어간다. `Ref<X>`에는 `X`를 만든 연산의 id만 들어간다. 판정은 트랜잭션 전에 facts로 끝낸다(DB에 닿기 전 거절) | sema.rs `same_target`, v3 `compose_plan`(:1696)의 `WIRE.TYPE_MISMATCH` 판정 |
| C0-3 권한 | 새 출처로 만든 값도 기존 검사를 모두 거친다. `Created(i)` 참조도 `ensure_refs_readable`(E4 규칙)과 `expose create allow`를 다시 평가한다. 호출자가 만들었어도 읽을 수 없는 행이면 `WIRE.MISSING_TARGET`이다. 우회 경로를 만들지 않는다 | v3 `ensure_refs_readable`(:1075), `create_row` allow |
| C0-4 멱등 | 묶음 전체가 바깥 `/apply` key 하나에 묶인다. 저장 요청은 참조를 기호(`{"created":0}`) 그대로 저장하므로 재전송은 같은 요청으로 판정되고 저장 결과(만든 id 포함)를 재생한다. 확장 안 `ctx.data.*`의 key는 형식만 검사하고 무시한다(바깥 key가 덮는다) | v6 `apply_attempt`(lib.rs:314, `aip_idem`), v4 `ctx_request`의 `check_ctx_key` |
| C0-5 잠금 순서 | 연산은 배열 순서로 실행하고 대상 행은 `ORDER BY id`로 잠근다. 키를 바꾸지 않는 전이는 `FOR NO KEY UPDATE`를 쓴다. 키 변경·삭제와 기존 자식 행 갱신·삭제 효과는 `FOR UPDATE`를 쓴다. 생성이 가리키는 행을 뒤의 명시적 `target.ids` 전이가 갱신하면, 기존 참조 가시성 조회에서 전이에 필요한 잠금을 먼저 잡는다. 산술은 잠근 행 값에서 같은 `UPDATE` 문으로 계산한다 | v3 `judge_query`, `row_lock`, `bundle_reference_locks`, `update` |
| C0-6 롤백 | 어느 연산이든 실패하면 트랜잭션 전체가 롤백되고 멱등 결과를 남기지 않는다. 서버가 소유하는 선언형 SQL 트랜잭션은 재시도 가능한 직렬화 실패·교착을 최대 3회 재실행한다. 동기 쓰기 확장은 서버에서 재실행하지 않는다. 확장이 `WIRE.CONFLICT`를 그대로 반환하면 호출자가 같은 키로 다시 보낸다 | v3 `before_commit_rejection`, `retry_owned_transaction` |
| C0-7 호환 | 기존 정의의 facts·계약 지문·실행 digest를 바꾸지 않는다. 새 facts 키는 그 기능을 쓴 정의에만 나온다. 리터럴 정수 증감은 계속 `{"increment": n}`으로 낸다. 구버전 서버는 새 wire 키를 `WIRE.UNKNOWN_KEY`로 거부한다(fail-closed, 이번 세션 실행 확인) | v5 `contract_module_with_apply`: 지문은 `write_binding` 접미부 전에 계산 |
| C0-8 오류 코드 | 새 WIRE 코드를 만들지 않는다. 형식·범위는 `WIRE.BAD_VALUE`, 타입은 `WIRE.TYPE_MISMATCH`, 허용 안 된 출처는 `WIRE.VALUE_SOURCE_NOT_ALLOWED`, 모르는 키는 `WIRE.UNKNOWN_KEY`. DEF는 기존 `DEF.ARITH_NOT_ALLOWED`, `DEF.TYPE_MISMATCH`, `DEF.UNSUPPORTED`에 메시지만 더한다 | `spec/wire-errors.md`, codes.rs |
| C0-9 MySQL | 1.0 범위 밖. `crates/aip-migrate/src/mysql_write.rs`는 새 facts 형태(`arith`, `refs`, 중첩 compose)를 만나면 `WIRE.UNSUPPORTED`로 거부하는 가드만 넣는다. 기존 `keys()` 검사가 새 wire 키를 이미 `UNKNOWN_KEY`로 막는다 | mysql_write.rs:1180 `keys`, :704 `increment` |

## P0-A. bundle 단계 간 타입 검증 참조

| 항목 | 결정 |
| --- | --- |
| 선택안 | create 연산에 선택 키 `refs`를 더한다. `{"create":"OrderLine","values":{"qty":3},"refs":{"order":{"created":0}}}`. apply 연산의 대상도 `{"target":{"created":0}}`로 앞에서 만든 행을 가리킬 수 있다. 출처는 `created: <앞 연산 번호>` 하나뿐이다 |
| 규칙 | ① `i`는 현재 연산보다 앞(0 ≤ i < 현재)이어야 한다. ② 연산 `i`는 create여야 하고 `id` 열이 있어야 한다. ③ 필드 타입은 `Ref<연산 i의 resource>`(nullable 허용). ④ 필드는 `expose create fields`에 있어야 하고 `values`와 `refs`에 동시에 올 수 없다. ⑤ apply 대상 참조는 연산 i의 resource가 전이의 resource와 같아야 한다. ⑥ 모두 트랜잭션 전 `bundle_ops` 단계에서 판정한다 |
| 불변식 | E1의 "주문은 줄이 1개 이상"은 부모 resource의 `check`가 커밋 시점(`run_checks`)에 평가되므로 같은 묶음에서 줄을 만들면 지킬 수 있다. 이 동작은 **확인 필요**(test-plan A-S3이 확정한다). 실패하면 이 문장을 결정표에서 지운다 |
| 기각 | ① 연산 결과에 대한 식·변수(`$0.total + 1`): 범용 언어화. ② 호출자가 UUID를 정해 부모·자식에 같이 쓰기: 숫자 Id에 안 되고 id 선점 공격면이 생긴다. ③ 중첩 생성(`create Order { lines: [...] }`): bundle과 같은 의도를 두 방식으로 쓰게 된다. ④ 서버가 패턴별로 선언하는 `expose create Order with lines`: 서버 작성량 증가. 불변식만 필요하면 1.1 후보 |
| 재사용 | `bundle_run`(:1603)이 `out.created_rows`를 이미 모은다. 해석 결과를 `Src::Lit(id, cast)`로 바꿔 기존 `literal` 뒤 경로(`ensure_refs_readable`, `create_row`)에 넣는다. 타입 판정은 compose의 `WIRE.TYPE_MISMATCH` 문구를 따른다. apply 대상은 `TargetSpec::Ids`로 바꾼다 |
| 와이어 | 요청: create 연산 키 `refs`(선택), apply `target.created`(선택). 응답 변화 없음(`created[]` 순서는 create 연산 순서). 구서버는 `UNKNOWN_KEY` |
| SDK(TS) | `bundle` 빌더를 더한다: `client.bundle(b => { const o = b.create("Order", {...}); b.create("OrderLine", {qty: 3}, { order: o }); })`. 핸들 타입 `Created<"Order">`가 `Ref<Order>` 필드에만 맞는다(컴파일 시 타입 오류). 원시 요청 형식도 그대로 받는다. ref 필드 메타데이터는 생성 binding의 **지문 밖 접미부**에 둔다(C0-7) |
| SDK(Python) | `bundle(operations)`가 `refs`를 그대로 보낸다. 도우미 `aip_sdk.created(i)`만 더한다. 정적 타입은 P1 |
| 호환·보안 | 기존 묶음 요청과 지문 불변. 공격면: 앞 연산의 id만 가리키므로 호출자가 볼 수 없는 기존 행을 지정하는 새 통로가 없다. 생성한 행이 호출자에게 안 보이면 C0-3대로 `MISSING_TARGET` |
| 멱등·잠금·롤백 | C0-4~6 그대로. 만든 행은 INSERT로 잠기고 대상 잠금 순서는 연산 순서다. 크기 상한 `BUNDLE_MAX_OPS` 20, `BUNDLE_MAX_ROWS` 1000 유지 |
| 유일 경로 | 부모·자식 원자 생성은 bundle `refs` 하나. compose의 `item`은 "기존 대상 행에서 값 복사"로 역할을 나눈다 |

## P0-B. 입력값·필드의 Int/Decimal 제한 산술

| 항목 | 결정 |
| --- | --- |
| 선택안 | 전이에 선택 블록 `input { qty: Int(1..100); amount: Decimal(12,2) }`를 더하고, 대입 한 형태만 연다: `f = f + 항` / `f = f - 항`. 항은 하나이며 `정수 리터럴(0 제외)`, `"Decimal 리터럴"`, `input.p`, `this.g`(같은 행의 non-null 숫자 필드) 중 하나다. `to` 대입과 효과 `update ... { f = f - this.total }` 대입 두 곳에서 쓴다. `from`·`allow`는 `input.p`와 비교할 수 있다(`from stock >= input.qty`) |
| 효과 update의 자기 참조 | 효과 블록의 `f = f - 항`에서 앞의 `f`는 위치로 정해지며 **효과 대상 행**의 현재 값이다. 항의 맨 이름과 `this.`는 전이 대상 행이다. 섞어 쓸 이름 해석 규칙을 새로 만들지 않는다 |
| 타입 | Int 필드 ± Int 항. Decimal(p,s) 필드 ± Decimal(p2,s2) 항(s2 ≤ s) 또는 Int 항. Decimal 항을 Int 필드에 쓰면 `DEF.TYPE_MISMATCH`. `input` 매개변수는 Int·Decimal non-null만(`?` 금지), 8개 이하 |
| 오버플로 | 결과가 Int range 밖이면 `WIRE.BAD_VALUE`(기존 `ranged` 사후 검사를 `arith`로 넓힌다). bigint·numeric(p,s) 초과는 PostgreSQL 22003이 이미 `WIRE.BAD_VALUE`로 분류된다(`write_err` :782). 감싸기(wrap)나 포화(saturate)는 없다. 실패하면 전체 롤백 |
| 정밀도 | 곱셈·나눗셈이 없으므로 반올림이 생기지 않는다. 항의 소수 자릿수가 필드보다 크면 정의 오류다. 입력 Decimal의 자릿수 초과는 기존 리터럴 검사가 `WIRE.BAD_VALUE`로 막는다 |
| NULL | 정의에서 배제한다. 대상 필드·항 모두 non-null이어야 하고 아니면 `DEF.TYPE_MISMATCH`. 그래서 실행 중 NULL 전파(`NULL + 1 = NULL`)나 숨은 기본값(COALESCE)이 생기지 않는다 |
| 기존 제약 유지 | `repeat unchanged`와 같이 못 쓴다(증감과 같음). 필드 읽기 정책이 있는 필드는 항·대상에 못 쓴다(`DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`). compose 단계에서 `input` 있는 전이는 `DEF.UNSUPPORTED`(P0-D) |
| 기각 | ① 곱셈·나눗셈·함수: 반올림·0 나누기 의미가 필요하고 범용 식이 된다(E3 `Σ 수량×단가`는 1.1 집계 후보). ② 두 항 이상 연쇄(`f = f + a - b`): 1.0은 한 항. ③ 호출자가 새 값을 계산해 `patch`로 보내기: 갱신 유실과 호출자 신뢰 문제(현재 우회의 원인). ④ NULL 전파·COALESCE: 조용한 오답 |
| 재사용 | 파서는 `additive`(parser.rs:1040)가 이미 `+`·`-`를 만든다. 효과 값 파서(parser.rs:573)만 additive를 부르게 한다. sema `counter`(:520)를 일반화하고, 입력 스코프는 aggregate `input`과 같은 `sc.input`(sema.rs:418)을 쓴다. 런타임은 `update`(:548)의 `increment` 분기 옆에 `arith`를, 값 바인딩은 `apply_input`(:1414)의 `values`·`expect` 처리와 같은 방식으로 `input`을 더한다 |
| facts | 리터럴 정수는 기존 `{"increment": n}` 유지. 새 형태는 `{"arith":{"op":"-","term":{"input":"qty"}}}` 또는 `{"term":{"path":...}}`. 전이 facts에 `input`은 선언한 경우에만 |
| 와이어 | apply 요청에 선택 키 `input: {qty: 3}`. 선언한 키는 모두 필요, 모르는 키는 `WIRE.FIELD_NOT_EXPOSED`(values와 같은 판정), 선언 없는 전이에 보내면 `WIRE.UNKNOWN_KEY`. bundle의 apply 연산과 PG 쓰기 확장 `ctx.data.apply`도 같은 키를 받는다 |
| SDK | TS `ApplyRequest`에 전이별 `input` 타입(range 주석 포함). Python 생성 helper `apply_<T>(client, ids, *, input=...)`. 지문은 input을 선언한 전이가 있는 정의에서만 바뀐다 |
| 호환·보안 | 기존 정의 facts 불변(C0-7). 서버가 범위를 선언한 값만 받고, 계산은 서버가 잠근 행에서 한다. 호출자는 증감량만 정하고 결과 값은 정하지 못한다 |
| 멱등·잠금·롤백 | 같은 key 재전송은 재실행하지 않는다(이중 차감 없음). 동시 차감은 행 잠금으로 직렬화되고 `from stock >= input.qty`는 잠근 뒤 판정해 음수가 되지 않는다. 결과 range 위반은 전체 롤백 |
| 유일 경로 | 서버가 값을 계산하는 쓰기는 이 형태 하나. 확장(patch+expect)은 이 형태로 안 되는 계산에만 쓴다 |

## P0-C. PostgreSQL 쓰기 확장의 `ctx.data.create`

| 항목 | 결정 |
| --- | --- |
| 선택안 | `run_write_in`(v4 write.rs:513)에 `op == "create"` 분기를 더한다. 확장 access의 `create.<R>` grant를 요구하고, 기존 `job_create_request`(:705)로 한 연산짜리 atomic 요청을 만들어 같은 트랜잭션의 `bundle_in`으로 실행한다. 응답은 MySQL·job 경로와 같은 `{"created":[{"resource","id"}]}`라 worker.mjs:42·worker.py:44의 `create`가 그대로 id를 돌려준다 |
| 권한 | 주체는 호출자다. 필드 허용 목록, `expose create allow`, `ensure_refs_readable`, DB 기본값, 유일 제약이 클라이언트 create와 같다(D-P-JOBCREATE와 같은 계약) |
| SERIALIZABLE | v4 write.rs:162 `write_needs_serializable`은 전이 grant만 본다. `create.<R>` grant면 `resources[R].exposeCreate.isolation`도 보게 고친다. 안 고치면 부재 의존 create가 READ COMMITTED로 돌아 D-A23을 어긴다 |
| key | `check_ctx_key`로 형식만 보고 무시한다(C0-4). 같은 확장 호출 안에서 같은 key로 두 번 create하면 두 행이 생긴다. 바깥 요청 재전송은 저장 결과를 재생하므로 확장이 다시 돌지 않는다 |
| 조합 | 확장이 돌려받은 부모 id로 자식을 `ctx.data.create`하면 확장 안에서도 부모·자식이 원자적이다. P0-A(호출자용)와 같은 `bundle_run`을 쓴다 |
| check 불일치 해소 | 현재 sema.rs:2217은 `create.<R>`를 받아들이고 런타임만 거부한다(audit E7). 이 묶음이 들어가면 불일치가 없어진다. 1.0에서 빠지면 sema가 PG 대상 write 확장의 `create.` grant를 `DEF.UNSUPPORTED`로 막는 별도 수정이 필요하다 |
| 기각 | ① 확장 전용 insert(허용 목록 밖 필드): 공개 create 계약 우회. ② 원시 SQL capability: 서버 권한 상실. ③ job과 다른 grant 이름: 같은 의도의 두 표기 |
| 와이어·SDK | HTTP 호출자 와이어 변화 없음. worker ABI는 기존 `ctx.data.create(res, values, key)` 그대로(확장 ABI 확정 전, OI-X2). 확장 ctx 타입 선언은 P1 |
| 멱등·잠금·롤백 | `ctx` 호출 실패는 기존대로 poison되어 확장이 오류를 잡아도 커밋하지 않는다. 원인 태그는 `ctx.create.<원인>`. `MAX_CTX_CALLS` 공유 |
| 문서 | LIMITS E7 삭제, SUPPORT 16 "`ctx.data.read`·`apply`만" 문구 수정 |

## P0-D. compose 단절 정리

| 항목 | 결정 |
| --- | --- |
| 현재 단절 | ① compose 전이 단계는 patch/expect를 못 받는다(sema.rs:1363 `DEF.UNSUPPORTED`, 런타임 :1887). ② P0-B `input` 전이도 같은 이유로 못 받는다. ③ `/apply` 요청은 `atomic`과 `compose` 중 하나라서(v6 lib.rs:230) compose와 매개변수 전이를 한 트랜잭션에 넣을 수 없다 |
| 선택안 | **compose 입력 슬롯은 열지 않는다**(D-P-UPDATE "compose 입력 슬롯은 확장하지 않고 원자 조합은 bundle" 유지). 대신 bundle 연산 종류에 `{"compose": ...}`를 더해 compose와 apply(patch·expect·input)를 한 묶음에 넣게 한다. 중첩은 한 단계만(compose 안 compose 없음) |
| 규칙 | 행 수 = compose 대상×단계 + 다른 연산, 합이 `BUNDLE_MAX_ROWS` 이하. `bundle_needs_serializable`(:1346)은 compose 연산에 `compose_needs_serializable`을 적용하게 고친다(지금 코드는 `create` 키가 없으면 false로 떨어진다). P0-A `refs`는 compose 연산을 가리킬 수 없다(만든 행이 여럿이라 번호가 모호): `WIRE.VALUE_SOURCE_NOT_ALLOWED` |
| 기각 | ① 단계별 values(대상 N개에 같은 값 또는 대상별 값): 대상별 expect가 필요해져 compose가 두 번째 bundle이 된다. ② compose create의 참조 필드에 상수 허용: 의도적으로 막은 숨은 행 연결 경로(`VALUE_SOURCE_NOT_ALLOWED`). ③ compose 폐지: 기존 계약 파괴 |
| 재사용 | `compose_exec`를 `bundle_run`의 한 분기로 부른다. 조합은 먼저 대상을 `ORDER BY id`로 잠그고 단계 중 가장 강한 잠금 모드를 사용한다 |
| 와이어·SDK | 요청: `atomic[]` 원소로 `{compose, targets, steps}`. TS `BundleOp`에 `ComposeRequest`를 더한다. 생성 계약의 compose `transitions`에는 정의 단계에서 이미 patch/expect 전이가 없으므로 input 전이도 같은 규칙으로 빠진다. 응답은 `applied[]`·`created[]`에 compose 결과가 연산 순서대로 이어 붙는다 |
| 호환·보안 | 기존 top-level `compose` 요청 그대로. 권한은 compose 경로 그대로(대상 가시성 잠금, sameScope, selfRow) |
| 멱등·잠금·롤백 | C0-4~6. compose 대상 잠금이 그 연산 시점에 일어나므로 앞 연산이 잡은 잠금과 순서가 섞인다. 교착은 기존 40P01 재시도가 처리한다 |

## 공개 count `rows read` 비상속: 소유자 결정 항목(변경하지 않음)

현재: `release count`는 `rows read`를 적용하지 않고, `where` 없는 count는 `DEF.ADVISORY_PUBLIC_COUNT` 경고, `callerFilter read`는 where가 정책을 함의해야 한다(이번 세션 fx `s15`에서 경고 확인).

| 안 | 내용 | 응답 값 | 기존 정의 | 안전성 | 비용 |
| --- | --- | --- | --- | --- | --- |
| O1 현행 유지 | 경고만 | 불변 | 불변 | 숨은 행 수 노출 가능(경고로 알림) | 0 |
| O2 명시 공개 표기 | `release count public`만 비상속, 표기 없는 count는 `rows read` 상속 | **바뀜**(표기 없는 정의) | 표기 추가 필요 | 높음 | 중 |
| O3 전부 상속 | 모든 count에 `rows read` | **바뀜** | 공개 지표가 줄어듦 | 높음 | 소 |
| O4 추가형 opt-in | 새 표기 `release count visible`(상속) 추가, 기본 불변 | 불변 | 불변 | 쓰는 정의만 높음 | 소 |
| O5 단계 전환 | 1.0.x에서 `public` 표기를 받아들이고 경고 문구로 안내 → 1.1에서 기본을 상속으로 전환(O2) | 1.0 불변, 1.1 바뀜 | 1.1 전에 표기 | 높음(1.1부터) | 중 |

권고: 1.0에는 O4 또는 O5의 1단계(둘 다 응답 값 불변)만. 기본값 전환(O2·O3)은 응답 값이 바뀌므로 소유자 결정 전에는 하지 않는다. 결정할 것: ① 1.0에 O4 표기를 넣을지, ② 1.1에서 기본 전환(O5 2단계)을 약속할지, ③ 전환 시 지문을 바꿔 구 클라이언트를 끊을지.

## P1 후보 분류

빈도·효과·위험은 상/중/하. 1.0은 "P0 뒤 1.0 출시 전", 1.1은 그 뒤.

| 후보 | 빈도 | 효과 | 위험 | 분류 | 이유 |
| --- | --- | --- | --- | --- | --- |
| Decimal `sum` | 상(금액) | 상 | 하(정확 합, 반올림 없음) | **1.0** | 결과 정밀도만 정하면 된다(출력 Decimal(p',s), 초과는 `WIRE.OUTPUT_INVALID`) |
| `avg` | 중 | 중 | 중(반올림 규칙·Int avg 타입) | 1.1 | 반올림 소유자 결정 필요 |
| 계산 집계 `Σ a×b` | 중 | 상(E3) | 중 | 1.1 | 곱셈 의미를 P0-B 밖에서 정해야 한다 |
| 관계 필드 필터·정렬 | 상 | 상 | 상(관련 행 정책·필드 정책 오라클, 조인 비용) | 1.1 | 정책 상속 설계가 먼저 |
| nullable cursor | 중 | 중 | 중(NULL 포함 keyset 인코딩) | 1.1 | offset 우회 있음 |
| 다중 집계(다중 groupKey·release) | 중 | 중 | 중 | 1.1 | |
| 정책 실수 감지(테넌트 조건 누락, allow가 rows read보다 넓음 등 조언) | 상 | 상(보안) | 하(조언만) | **1.0** | 자동 tenant 격리가 없으므로(SUPPORT 13) 누락 감지가 유일한 안전망 |
| Python 정적 바인딩(TypedDict read·bundle) | 중 | 중 | 하 | 1.1 | P0의 Python은 원시 dict + 도우미로 충분 |
| worker ctx 타입(access grant별 d.ts·Protocol) | 상(확장 작성자) | 중 | 하 | **1.0** | P0-C로 ctx 표면이 넓어져 오타·grant 누락을 컴파일 시 잡는다 |
| 진단·기계판독 계약(진단 JSON 스키마·버전 고정) | 중(AI·CI) | 중 | 하 | **1.0** | 1.0 공개 시점에 형식을 고정해야 뒤에서 깨지 않는다 |
