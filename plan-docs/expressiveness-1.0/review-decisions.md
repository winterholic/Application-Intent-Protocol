# P0-A~D 결정표 독립 검토 (기준 a4eba452, 코드 4e132564)

검토 방식: 결정표가 인용한 코드를 직접 읽고, 의심 지점은 `/tmp`의 임시 PostgreSQL 17과 임시 crate로 실행해 판정했다. 제품 코드는 수정하지 않았고 커밋하지 않았다. 문장의 그럴듯함이 아니라 코드와 실행 출력으로 판정했다. 읽기만 한 것은 "코드 읽기"로, 실행한 것은 "실행"으로 구분해 적었다.

## 0. 먼저 전달할 것 (1.0 차단)

P0-B와 P0-C 구현 작업자는 이미 착수했으므로 이 절만 먼저 읽으면 된다. 번호는 아래 본문과 같다.

| 번호 | 묶음 | 지적 | 한 줄 수정안 |
| --- | --- | --- | --- |
| B-1 | P0-B | `input`의 Decimal은 부호 제약이 없고 Int도 `range`를 안 쓰면 음수가 들어간다. `balance = balance - input.amount`에 `"-100.00"`을 보내면 잔액이 늘어난다(실행으로 확인). | 항은 양수만 받는다. input Int는 `range` 하한 1 이상을 정의 필수로, Decimal은 런타임에서 `항 > 0` 아니면 `WIRE.BAD_VALUE`로 거절한다. `this.g` 항도 같은 런타임 가드를 둔다 |
| B-2 | P0-B | `allow`에는 field read 정책 필드를 읽는 것이 허용되어 있다(sema.rs:1149는 `allow`에 `reject_hidden_reads`를 안 건다). 호출자가 고른 `input.p`를 `allow`에서 가려진 필드와 비교하게 열면 `FORBIDDEN` 대 성공으로 가려진 값을 이분 탐색할 수 있다. | `allow`, `from`, effect `when` 어디서든 `input.*`과 field read 정책 필드를 같은 비교식에 쓰면 `DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`. `reject_hidden_reads`를 `allow`와 `when`에도 걸되 input이 섞인 식만 대상으로 한다 |
| D-1 | P0-D | bundle 응답의 캐시 무효화 `tags`가 compose 연산을 모른다. `bundle_tags`(v6 lib.rs:255-277)는 `atomic` 안에서 `apply`·`create`만 본다. | `bundle_tags`가 `op["compose"]`와 그 `steps`(transition, create)를 top-level compose와 같은 규칙으로 센다 |
| D-2 | P0-D | `bundle_needs_serializable`이 compose 연산에서 false를 낸다(결정표 서술이 맞다. 실행으로 확인). 고치지 않으면 부재 의존 compose가 READ COMMITTED로 돈다. | `op.get("compose")` 분기를 더해 `compose_needs_serializable(facts, op)`를 호출한다 |

수정 권장(1.0 출시 전에 반영 권고)은 B-3, B-4, B-5, C-1, C-2, A-1, D-3, D-4와 test-plan 정정 T-1~T-5다. 나머지는 참고다.

## 1. 결정표가 인용한 코드와 동작 주장 대조

판정: 맞음 / 틀림 / 보정(대체로 맞으나 문구·위치 보정). 근거는 파일:줄이다.

| # | 결정표 주장 | 판정 | 근거 |
| --- | --- | --- | --- |
| 1 | `bundle_run`이 `out.created_rows`를 모은다 | 맞음 | v3 lib.rs:1665 `out.created_rows.push((res.to_string(), id))`. create 연산만 담고 `id` 열 없는 resource는 `None`(1175-1189) |
| 2 | `Src::Lit(id, cast)`로 바꾸면 `ensure_refs_readable`, `create_row` 경로를 탄다 | 맞음 | lib.rs:1084 `let Src::Lit(id, _) = src`가 Lit만 읽는다. 1662-1663에서 두 함수가 순서대로 불린다 |
| 3 | `write_err`가 22003을 `WIRE.BAD_VALUE`로 분류 | 맞음 | lib.rs:782. 실행: bigint 오버플로 `ERROR: 22003: bigint out of range`, numeric(12,2) 오버플로 `22003: numeric field overflow` |
| 4 | 기존 `ranged` 사후 검사를 `arith`로 넓힌다 | 보정 | `ranged`(lib.rs:568-574)는 `update()`의 전이 `to`만 본다. effect `update`(lib.rs:863 이하)에는 같은 검사가 없다(B-4) |
| 5 | 지문은 `write_binding` 접미부 전에 계산 | 맞음 | v5 lib.rs:224-227 `fingerprint_of(&module)` 뒤에 `write_binding(facts)` 접미부가 붙는다. 다만 `Ref<X>`는 이미 `Id<"X">`로 계약에 있어(ts_type) `Created<"Order">` 타입에 새 메타데이터가 필요 없을 수 있다 |
| 6 | `bundle_needs_serializable`이 create 키에 의존 | 맞음 | lib.rs:1351. 실행: compose 연산이 든 `atomic`에서 `false`(0절 D-2) |
| 7 | v4 `write_needs_serializable`은 전이 grant만 본다 | 맞음 | v4 write.rs:162-169. 실행: `access {"create.Todo"}`, `exposeCreate.isolation=serializable`에서 `false` |
| 8 | `run_write_in`(v4 write.rs:513)에 create 분기를 더한다 | 맞음 | 거부 지점은 write.rs:564-565 `rej("WIRE.UNSUPPORTED", ...create·mongo·redis 미지원)`. `invoke_write_action`(host 경로, :305-320)에는 create 분기가 이미 있다 |
| 9 | `job_create_request`로 한 연산짜리 atomic 요청을 만든다 | 보정 | write.rs:705-711은 키 집합(`values`, `key`)만 검사하고 `key` 형식은 안 본다. `check_ctx_key`(:130)는 `ctx_request`(:736)에서만 불린다(C-1) |
| 10 | 확장 안 `ctx.data.*`의 key는 형식만 보고 무시한다 | 맞음(apply), 보정(create) | apply는 :736. create는 위 9 때문에 새 분기에서 직접 불러야 한다 |
| 11 | worker.mjs:42, worker.py:44가 `created[0].id`를 돌려준다 | 맞음 | `call("create", res, {values, key})`, Python은 `key=None`이면 `"key": null`을 보내는데 `job_create_request`가 받는다 |
| 12 | 저장 요청은 기호를 그대로 저장하고 재전송은 같은 요청으로 판정한다 | 맞음 | v6 lib.rs:350-356, 382-384. 비교는 `stored == incoming.to_string()`(encryption.rs:121-122)이고 재생은 저장 결과 그대로(357-361). `write_req` 복사본만 변형되고 저장되는 것은 원본 `req`다(364-373, 382) |
| 13 | 재시도 가능한 직렬화 실패·교착만 서버가 최대 3회 재실행 | 보정 | SQL 전용 경로(`handle_apply`, v3 `bundle`)만 맞다. 쓰기 확장 경로(v6 write_extension.rs)는 worker 효과 때문에 재실행하지 않는다(v6 lib.rs:285-286 주석, 확장은 claim 재시도만). P0-C로 확장 안 create가 SERIALIZABLE이 되면 영향이 커진다(C-2) |
| 14 | 연산은 배열 순서로 실행, 대상은 `ORDER BY id FOR UPDATE` | 맞음 | lib.rs:1631-1667(순차), judge_query lib.rs:422(`ORDER BY t.id FOR UPDATE OF t`), compose_exec lib.rs:1851 |
| 15 | 산술은 같은 `UPDATE` 문에서 `col = t.col + $p` | 맞음 | lib.rs:552. 실행(2세션): 세션 A가 stock 10→5를 커밋하는 동안 `SELECT ... (t.stock >= 8) FOR UPDATE`를 건 세션 B는 대기 후 `stock=5, from_ok=f`를 받았다. 판정 `from`이 잠근 뒤 새 버전으로 다시 계산된다 |
| 16 | 어느 연산이든 실패하면 롤백, 멱등 결과 없음 | 맞음 | SQL 경로: idem 삽입은 마지막(v6 lib.rs:386-391). 확장 경로: claim을 먼저 넣지만 같은 트랜잭션이라 롤백 시 사라진다(write_extension.rs:82-123) |
| 17 | compose 전이 단계는 patch/expect 불가(sema.rs:1363, 런타임 :1887) | 맞음 | sema.rs:1363, v3 lib.rs:1884-1888 |
| 18 | `compose_in`(:1809)을 `bundle_run` 분기로 부를 수 있다 | 맞음 | `compose_plan(facts, req, wire)`(lib.rs:1696)는 `req` 전체를 `compose/targets/steps`로 읽으므로 연산 객체를 그대로 넘기면 된다 |
| 19 | 구서버는 새 wire 키를 `UNKNOWN_KEY`로 거부 | 맞음(+1 형태) | `refs`: lib.rs:1636 `keys(m, ["create","values"])`. `input`: lib.rs:1456 `keys(o, [...])`. compose 연산: 같은 1636. `target.created`: lib.rs:323 `keys(tg, ["ids","where"])`. 결정표 V-4 증거에는 마지막 형태가 빠졌으나 동작은 같다 |
| 20 | `many`/`update` effect는 대상 행마다 정확히 하나를 바꾼다 | 맞음 | lib.rs:872-874. 실행(SQL): 주문 둘이 같은 지갑을 가리키는 `UPDATE ... FROM`은 한 번만 반영한다(`100.00 → 80.00`). 이 가드가 없으면 조용한 갱신 유실이다(B-6) |
| 21 | 효과 값의 맨 이름과 `this.`는 전이 대상 행 | 맞음 | sema.rs:1556 `value_scope`가 `sc.clone()`이고 `sc.this`는 전이 resource다 |
| 22 | `DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION` 재사용 | 맞음 | sema.rs:1147(`from`), 1191(`repeat unchanged`), 1250(`expect`). `allow`에는 안 걸린다(B-2). 기존 `counter` 경로는 `continue`로 `to` 값 검사를 건너뛴다(sema.rs:1162-1168) |
| 23 | check가 커밋 시점에 평가되어 같은 묶음 부모·줄을 지킬 수 있다 | 맞음, 코드 정정 | 실행(SQL): 부모+줄이면 위반 0행, 부모만이면 1행. 실패 코드는 `WIRE.CHECK_FAILED`(lib.rs:1319)이고 `INVARIANT_VIOLATED`는 정원·배타 제약(23P01, lib.rs:1297)이다(T-1) |
| 24 | 효과 `update`의 맨 `f`는 효과 대상 행 | 구현 시 함정 | 현재 sema에서 맨 이름은 전이 행으로 해석된다. 위치 규칙을 새로 만들면 `counter`의 `same` 판정(sema.rs:526-530)이 `this.f`도 통과시킨다(B-3) |

## 2. P0-A refs

반례 탐색 결과를 항목별로 적는다.

| 관점 | 결과 |
| --- | --- |
| 생성 행이 호출자에게 안 보이는 경우 | 안전하다. `ensure_refs_readable`(lib.rs:1075-1115)이 참조 resource의 `rowRead`로 확인하고 못 찾으면 `MISSING_TARGET`이라 전체 롤백. 반례 없음. 부작용: 대리 생성(관리자가 고객 주문을 만들고 같은 묶음에 줄 추가)은 부모가 호출자에게 안 보이면 막힌다. 불편이지 취약점이 아니다. 존재 누설 방지용 모호한 코드를 `created` 참조에까지 쓸 필요는 없으니 메시지에 "같은 묶음에서 만든 행이 읽기 정책에 안 보임"을 더하면 진단이 쉽다(참고) |
| 다른 resource를 가리키는 경우 | 안전하다. 정의 시점 facts로 `Ref<연산 i의 resource>`를 판정하므로 트랜잭션 전에 `TYPE_MISMATCH`. `Id<..>`로 선언된 필드는 결정표 규칙 ③이 `Ref`만 말하는데 `literal`은 `Ref<`·`Id<` 둘 다 받는다(lib.rs:1513). 구현 시 `Id<>` 필드를 허용할지 정해야 한다(참고) |
| 기호 저장과 같은 요청 판정 | 반례 없음. 12번 행. `{"created":0}`은 서버가 고쳐 쓰지 않는다. 같은 key에 `refs` 대신 해석된 id를 `values`로 보내면 `IDEMPOTENCY_MISMATCH`가 되고 이것이 맞다 |
| 잠금 순서 변화 | 변화 없음. 만든 행은 INSERT로 자기 트랜잭션이 잠그고, `target.created` apply는 `judge_query`가 자기 트랜잭션이 만든 행을 `FOR UPDATE`한다. FK는 부모가 먼저 INSERT되므로 만족된다. 반례 없음 |
| 구서버 fail-closed | 맞다(19번 행) |

### A-1 [수정 권장] `target.created`는 `id` 대상 형식을 요구해야 한다

- 근거: `parse_target`(v3 lib.rs:318-330)는 `forms.contains("id")`가 아니면 `WIRE.TARGET_NOT_ALLOWED`로 거절한다. 결정표 규칙 ⑤는 resource 일치만 말하고 대상 형식을 말하지 않는다.
- 시나리오: 전이가 `expose apply close target where bulk maxRows 10`만 열었다. `target.created`가 `ids` 형식과 같은 경로(`TargetSpec::Ids`)로 풀리면 서버가 열지 않은 `id` 대상 형식이 `created` 참조를 통해 열린다.
- 수정안: `target.created`는 `ex["target"]`에 `"id"`가 있을 때만 받고, 없으면 `WIRE.TARGET_NOT_ALLOWED`. test-plan A-F3에 한 줄 추가한다.

### A-2 [참고] 응답의 `created[]` 순서와 `created: i`의 번호가 다르다

- `i`는 전체 연산 번호이고 응답 `created[]`는 create 연산만 담는다(lib.rs:1665). `[apply X, create Order, create Line refs{created:1}]`에서 `created[0]`이 Order다. 결정표도 "순서는 create 연산 순서"라고 적었으니 모순은 아니다. SDK 빌더가 핸들로 가려 주지만 원시 요청 사용자에게는 함정이다. 문서에 한 줄 예시를 넣는다.

### A-3 [참고] 중복 rows 계수

- `target.created` apply는 `target.ids`가 없어 현재 코드 방식(lib.rs:1619-1622)이면 `bulkMaxRows`로 센다. 구현은 1행으로 세야 한다(상한 계산이 과대).

## 3. P0-B 산술

| 관점 | 결과 |
| --- | --- |
| 오버플로 | bigint(Int)·numeric(p,s)는 22003→`BAD_VALUE`로 확인(실행). 전이 `to`는 `ranged` 사후 검사가 `BAD_VALUE`로 롤백. effect update는 사후 검사가 없다(B-4) |
| Decimal 자릿수 | 입력 리터럴은 `decimal_binding`이 소수 자릿수·정수부 자릿수를 검사한다(실행: `"12.505"`는 `BAD_VALUE`, `"12.50"` 통과). 검사를 건너뛰면 PG가 조용히 반올림한다(실행: numeric(12,2)에 0.005를 더하면 0.01). 따라서 `input` 값을 `literal`이 아닌 다른 경로로 넣지 않게 해야 한다(B-5) |
| NULL | 정의에서 배제하는 결정은 AIP가 만든 DDL에서는 맞다(DDL이 `NOT NULL`, sqlgen.rs:766-774). 외부 DB 채택 모드(`externalStorage`)에서 열이 nullable이면 `NULL - n = NULL`로 조용히 NULL이 된다. 확인 필요(채택 시 NOT NULL 검증 여부를 안 읽음) |
| 동시 차감 | 전이 대상 행은 맞다. 15번 행의 2세션 실행 증거. 효과 대상 지갑은 B-4 |
| 필드 읽기 정책 | 항·대상 금지는 맞으나 `allow`에서 input과 섞이면 새 통로가 열린다(B-2) |
| 효과 update 자기 참조 이름 해석 | 위치 규칙은 쓸 만하나 sema의 `counter` 재사용 시 `this.f`가 통과한다(B-3) |

### B-1 [1.0 차단] 부호를 정의가 고정하지 않는다

- 근거: 기존 증감은 `Expr::Int(n)`(0 제외)이 리터럴이라 부호가 연산자로만 정해졌다(sema.rs:521-543). `input`은 호출자 값이다. `decimal_binding`은 음수를 받는다(실행: `Decimal<12,2>` `"-100.00"` → accepted). Decimal에는 `range`가 없고, Int `range`는 선택이다(실행: `Int` range 없음에 `-1000000` accepted). `from stock >= input.qty`는 음수 qty에서 항상 참이다.
- 반례(실행): `UPDATE w SET balance = balance - '-100.00'::numeric` → 100.00이 180.00(원래 80.00)이 됐다. 호출자가 자기 지갑을 임의로 늘린다. `+` 쪽은 반대로 차감이 된다.
- 수정안: (1) `input` 항은 항상 양수만 받는다. Int는 정의 단계에서 `range`를 필수로 하고 하한이 1 이상이 아니면 `DEF.BAD_RANGE`. Decimal은 정의 문법에 하한을 두기 어려우니 런타임에서 `항 > 0`이 아니면 `WIRE.BAD_VALUE`(DB 전, 입력 검증 단계). (2) `this.g` 항은 호출자가 아니라도 다른 쓰기 경로가 음수를 넣을 수 있으므로 같은 `UPDATE`에 `AND (term) > 0` 가드를 걸고 RETURNING 사후 검사로 `BAD_VALUE`를 낸다. (3) test-plan B-F10에 "음수·0 입력"을 더하고 B-S2에 `"-12.50"`을 실패 fixture로 더한다.
- 같은 변경으로 "0 제외" 규칙(리터럴 0 금지)과 일관된다.

### B-2 [1.0 차단] `allow`와 `input` 비교가 가려진 필드를 이분 탐색으로 새게 한다

- 근거: sema.rs:1146-1148은 `from`에만 `reject_hidden_reads`를 건다. `allow`(1149-1150)는 걸지 않는다. syntax_card.rs:192도 "`allow`...는 가능하다"고 적는다. 지금은 `allow`가 서버가 고른 상수·actor와 비교하므로 새는 정보가 고정된 한 비트였다. 결정표 P0-B는 `from`·`allow`가 `input.p`와 비교할 수 있게 연다.
- 시나리오: `Account`에 field read 정책이 걸린 `risk_score`가 있고 `transition probe { allow this.risk_score >= input.n ...}`를 쓰면 호출자가 n을 바꿔 가며 `WIRE.FORBIDDEN`이 나는 경계를 찾는다. expect(sema.rs:1250)와 `from`이 이미 막는 것과 같은 종류의 추론이다.
- 수정안: field read 정책 필드가 `input.*`과 같은 비교식(`cmp`, `in`)에 있으면 `allow`·`from`·effect `when` 어디서나 `DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`. 가장 단순한 구현은 `allow`와 `when`에도 `reject_hidden_reads`를 걸되 해당 식에 `input` root가 있을 때만 적용한다. 참조 이동 경로(`this.owner.secret`)는 `hidden_reads`가 이미 따라간다. test-plan B-F6 옆에 `allow` 변형을 더한다.

### B-3 [수정 권장] 효과 update에서 선행 피연산자 해석

- 근거: `counter`(sema.rs:521-543)는 선행 피연산자가 텍스트로 `f` 또는 `this.f`면 통과시킨다. facts의 `{"arith": {op, term}}`에는 선행 피연산자가 기록되지 않는다(결정표 facts 예). 효과 문맥에서 `balance = this.balance - this.total`을 쓰면 sema는 통과시키고 런타임은 `u.balance - t.total`을 실행하지만 읽는 사람과 sema의 기존 스코프 규칙(sema.rs:1556)에서 `this.balance`는 전이 행(주문)의 `balance`다.
- 수정안: 효과 문맥은 선행 피연산자로 맨 `f`만 받고 `this.f`는 `DEF.ARITH_NOT_ALLOWED`로 거절한다. 전이 행에 같은 이름의 필드가 있으면 맨 `f`와 항의 맨 이름이 헷갈리므로 `DEF.AMBIGUOUS_NAME`을 낸다. 항(`this.g`, 맨 `g`)은 전이 행에서만 풀린다고 오류 메시지에 쓴다.

### B-4 [수정 권장] 효과 대상의 범위·하한 보장이 없다

- 근거: `ranged`(lib.rs:568-574)와 사후 거절(:634-636)은 전이 `to`의 `increment`만 본다. effect `update`(lib.rs:863-874)는 `write_err`만 있고 범위 검사가 없다. Int range는 DDL CHECK가 없다(sqlgen.rs:769-772 주석). 효과 Int 필드에 `points = points - this.cost`를 쓰면 `Int(0..1000)` 범위 밖(음수)이 조용히 저장된다.
- Decimal 지갑: K-2는 "잔액 음수 없음"을 기대하지만 Decimal에는 range가 없다. 안전한 경로는 정의 작성자가 `check balance >= 0`(커밋 시점 `WIRE.CHECK_FAILED`, lib.rs:1302-1321)을 두거나 전이 `from`에서 지갑 잔액을 `exists`로 읽는 것이다. 후자는 judge가 지갑 행에 `FOR SHARE`를 잡으므로 동시 두 주문은 효과 UPDATE에서 서로 기다려 교착(40P01)이 나고 재시도 뒤 `INVALID_STATE`로 끝난다(코드 읽기, 미실행). 둘 다 안 쓰면 두 주문이 모두 커밋되어 음수가 된다.
- 수정안: (1) effect 산술에도 `ranged` 사후 검사를 같은 방식으로 붙인다(대상 필드에 Int range가 있을 때). (2) 효과로 Decimal·Int를 줄이는 정의에서 대상 resource에 하한을 보장하는 `check`가 없으면 개발 조언 `DEF.ADVISORY_...`를 낸다. (3) K-2 fixture는 `check balance >= 0`을 포함하고, 없는 변형이 두 주문 모두 커밋되는 것을 음성 시험으로 남긴다.

### B-5 [수정 권장] 구현 체크리스트 (코드 읽기로 확인한 빠뜨리기 쉬운 지점)

| 지점 | 근거 | 필요한 변경 |
| --- | --- | --- |
| `judge_query`에 입력 파라미터가 없다 | lib.rs:379-390 시그니처, 판정 `from`(398), `update`의 재검사 `from`(531) | `from stock >= input.qty`는 `judge_query`와 `update` 두 곳에 같은 바인드로 들어가야 한다. 한쪽만 바꾸면 UPDATE `WHERE`가 바인드 불일치로 `INTERNAL` |
| `done` 계산 | lib.rs:402-405 `increment`만 `FALSE`로 둔다. 새 `arith`는 `cx.value`로 떨어져 `internal` | `arith`도 `done.push("FALSE")` |
| `ranged` | lib.rs:571 `v.get("increment")` | `arith`도 포함 |
| `apply_op`의 키 집합 | lib.rs:1456 | `input`을 허용 키에 더하고 선언 없는 전이는 `apply_input`처럼 `UNKNOWN_KEY` |
| 입력 값 파싱 | `literal`(lib.rs:1498)은 resource 필드 기준으로 type·range를 본다 | `input` 선언의 타입·range로 같은 `scalar::parse`·`validate_range`를 호출하는 별도 함수. 필드와 이름이 같다고 `literal`을 재사용하면 필드 range가 입력에 적용된다 |
| 확장 `ctx.data.apply` | v4 write.rs:735-740 `for key in ["values","expect"]`만 `request`로 옮긴다 | `input`도 옮기지 않으면 `target` 안에 남아 `keys(tg, ["ids","where"])`(v3 lib.rs:323)에서 `UNKNOWN_KEY` |
| `counter`의 hidden 검사 건너뜀 | sema.rs:1162-1168 `continue` | `arith`는 대상·항 모두 hidden 검사를 직접 한다(결정표 의도) |
| 암호화 Decimal 대상 | `counter`는 텍스트 비교라 `path()`의 `reject_encrypted_path`(sema.rs:435)를 안 거친다 | 대상·항이 `encrypted`이면 거절. SUPPORT.md:56상 1.0이 지원하지 않는 기능이라 참고 수준이나, 통과시키면 런타임에서 `text - numeric`으로 `INTERNAL`이 난다 |
| MySQL 가드 | mysql_write.rs:704-708 `increment` 외 값은 `cx.value`로 떨어진다 | 결정표 C0-9대로 `arith`는 `WIRE.UNSUPPORTED`로 거절(1.0 범위 밖이므로 가드만) |

### B-6 [참고] 벌크 전이 + 같은 효과 대상은 계속 `EFFECT_TARGET_MISSING`이어야 한다

- 실행(SQL): 주문 두 건이 같은 지갑을 가리키는 `UPDATE ... FROM ... RETURNING`은 지갑을 한 번만 갱신해 `100.00 → 80.00`(30.00과 20.00 중 하나만 반영)이 된다. 현재는 lib.rs:872-874가 `got`(1개)와 `effect_change`(2개)를 비교해 전체 롤백한다. 산술 효과가 들어오면 이 가드가 조용한 갱신 유실을 막는 유일한 장치다.
- 조치: 이 가드를 건드리지 않는다. 시험 K-5로 "같은 주인의 주문 둘을 한 번에 결제 → `WIRE.EFFECT_TARGET_MISSING`, 지갑 불변"을 더한다. 문서에는 효과 산술이 호출당 대상 행 하나에 하나씩만 대응함을 적는다.

## 4. P0-C 확장 create

| 관점 | 결과 |
| --- | --- |
| 권한 우회 | 반례 없음. 새 분기가 `bundle_in`을 타면 필드 허용 목록(lib.rs:1644-1659), `ensure_refs_readable`(1662), `create_row`의 `allow`(1172-1189)가 클라이언트 create와 같다. 주체는 `caller`. `internal`은 전이에만 쓰인다. 확장이 허용 목록 밖 필드를 못 넣는다는 결정 기각 ①도 코드와 일치 |
| SERIALIZABLE 판정 누락 | 결정표 지적이 맞다(7번 행, 실행 확인). 고칠 곳은 `write_needs_serializable` 하나다(`transaction_setup`, `prepare_write_*`, `invoke_write`가 모두 이 함수를 쓴다). 쓰기 job 경로는 v3의 `write_needs_serializable(facts, request)`(durable.rs:1318-1321)를 쓰므로 이미 create를 본다. 따라서 D-A23의 "쓰기 확장·job은 결정 밖(미착수)"(docs/DECISIONS.md D-A23)은 job에 대해서는 이미 낡았다(참고) |
| poison 롤백 | 동작한다. `mine`인 ctx 실패는 `poisoned`·`poison`에 첫 원인을 남기고(write.rs:586-590) 확장이 성공을 돌려줘도 `extension_error`로 끝난다(:612-620). 원인 태그는 `ctx_cause(op_label)`가 `ctx.create.<원인>`을 만든다(write.rs:64-93). `forbidden`, `access_not_declared`, `field_not_exposed`는 코드 매핑과 일치한다 |
| ctx key 무시와 중복 | 동작은 결정표대로다. 같은 확장 호출 안에서 같은 key로 두 번 create하면 두 행이다. job 경로는 key를 장부에 남겨 재생하므로(D-P-JOBCREATE) 같은 worker ABI가 job과 확장에서 다르게 동작한다(C-3). 바깥 재전송은 claim 선행 방식이라 확장이 다시 돌지 않는다(write_extension.rs:82-123) |

### C-1 [수정 권장] `job_create_request`는 key 형식을 검사하지 않는다

- 근거: write.rs:705-711은 키 집합과 `values` 존재만 보고 `key`는 읽지 않는다. `check_ctx_key`(:130)는 `ctx_request`(:736)에서만 불린다. 결정표 "check_ctx_key로 형식만 보고"와 test-plan C-F5("잘못된 key → `ctx.create.key_format`")는 새 분기가 직접 부르지 않으면 통과할 수 없다.
- 수정안: `run_write_in`의 create 분기에서 `message["input"]["key"]`가 있고 null이 아니면 `check_ctx_key`를 먼저 부른다. `job_create_request`는 job 경로(`mysql_jobs.rs:1430`, durable)와 공유하므로 건드리지 않는다.

### C-2 [수정 권장] 확장 경로에서는 직렬화 실패·유일 제약이 불투명한 `EXTENSION_ERROR`가 된다

- 근거: `run_write_in`의 `done` 처리(write.rs:599-627)는 `poisoned`가 있으면 모두 `extension_error`(`WIRE.EXTENSION_ERROR`, retry never)로 끝낸다. host 경로 `invoke_write_action`(:392-402)에만 `WIRE.CONFLICT`와 `WIRE.ALREADY_EXISTS` 통과 분기가 있다. 결정표 C0-6은 "서버가 최대 3회 재실행"이라 쓰지만 확장 경로에는 그 재시도가 없다(13번 행). wire-errors.md의 `WIRE.CONFLICT` 문구("서버가 이미 3회 재시도했다")도 이 경로에는 안 맞는다.
- 영향: create가 SERIALIZABLE이 되면(위 SERIALIZABLE 항목) 동시 쓰기 충돌(40001)이 `ctx.create.conflict` 원인의 non-retryable `EXTENSION_ERROR`로 호출자에게 간다. 같은 key 재전송으로 복구되는 충돌인데 클라이언트는 재시도하지 않는다. K-4("하나는 `WIRE.ALREADY_EXISTS` 원인")도 호출자가 보는 코드는 `EXTENSION_ERROR`이고 `ALREADY_EXISTS`는 로그 `cause`(`ctx.create.already_exists`)에만 있다.
- 수정안: `run_write_in`에 host 경로와 같은 통과 분기를 더한다(poison이 `WIRE.CONFLICT`면 `WIRE.CONFLICT`, `WIRE.ALREADY_EXISTS`면 `WIRE.ALREADY_EXISTS`). 그리고 C0-6 문구를 "SQL 전용 경로는 3회 재실행, 확장 경로는 재실행 없이 `WIRE.CONFLICT`를 돌려주며 같은 key로 재전송"으로 정정한다.
- 관련: 확장 트랜잭션은 savepoint가 없어 `create_row`의 DB 오류(유일 제약 등)가 나면 트랜잭션이 aborted 상태가 되어 이어지는 ctx 호출은 25P02로 `INTERNAL`이 된다(코드 읽기). 첫 원인은 `get_or_insert`로 보존되므로 진단은 맞다. "create 실패를 잡고 계속하는" 확장은 PG 경로에서 성립하지 않는다는 점을 문서에 적는다(C-F4와 일치).

### C-3 [참고] key 의미가 job과 확장에서 다르다

- job은 key를 장부에 남겨 재시도를 재생하고(D-P-JOBCREATE), PG 확장은 key를 검사만 하고 버린다. 같은 `ctx.data.create(res, values, key)` ABI의 의미가 다르다. 값싼 보완: 확장 호출 하나 안에서 같은 key의 두 번째 create를 이전 결과로 재생(메모리 map)하면 job과 같아진다. 결정표 "key는 무시" 기록을 유지하되 문서에 "같은 호출 안 같은 key는 중복 행"을 명시하면 충분하다.

### C-4 [참고] 암호화 resource

- `run_write_in`의 knobs에는 `seal`이 없다(write.rs:534). 암호화 필드가 있는 resource에 `create.<R>`를 열면 평문이 들어갈 수 있다. 1.0이 암호화를 지원하지 않으므로(SUPPORT.md:56) 차단은 아니나 `ctx.data.read`가 `declares_encrypted`를 거절하는 것(write.rs:661)처럼 create도 거절하는 한 줄을 권한다.

## 5. P0-D bundle 안 compose

| 관점 | 결과 |
| --- | --- |
| 잠금 순서 혼합 교착 | 새로운 종류는 아니다. 연산 배열 순서가 곧 잠금 순서이고(14번 행), 이미 `[apply 1, apply 9]`와 `[apply 9, apply 1]`는 교착할 수 있다. compose는 대상 N개를 id순으로 한 번에 잠그므로 교착 확률이 올라간다. 40P01은 SQL 전용 경로에서 최대 3회 재실행되고 그 뒤 `WIRE.CONFLICT`(retry later)다. 결정표 서술이 맞다. K-3으로 확인하되 "배열 순서를 호출자가 맞춰야 한다"를 문서에 적는다 |
| 행 수 상한 | 현재 사전 계수(lib.rs:1612-1626)는 `apply`가 아닌 모든 연산을 1행으로 센다. 구현이 이 분기를 그대로 두면 compose가 1행으로 센다. compose별 상한(`ids*steps`, lib.rs:1717-1719)은 `compose_plan` 안에 있고 이 함수는 `compose_in` 안에서 트랜잭션 시작 뒤 불린다(D-3) |
| serializable 판정 | D-2 |
| refs 금지 규칙 | 맞다. compose 연산이 만든 행 수는 `ids.len()*create 단계 수`로 불확정이라 번호가 모호하다. 결정표의 `VALUE_SOURCE_NOT_ALLOWED`는 `target.created`에도 같이 적용한다(A-F3에 이미 있음) |

### D-3 [수정 권장] compose 계획 검증을 트랜잭션 전에 한다

- 근거: C0-2는 "판정은 트랜잭션 전에 facts로 끝낸다"고 하지만 `compose_in`은 `compose_plan`을 호출 시점(앞 연산이 이미 실행된 뒤)에 부른다(lib.rs:1817). `STEP_NOT_ALLOWED`, `NOT_EXPOSED`, `TYPE_MISMATCH`, `BULK_LIMIT`이 앞 연산 실행 후에 나와 롤백된다. 결과는 같지만 비용이 들고 "DB에 닿기 전 거절" 약속과 다르다.
- 수정안: `bundle_ops`(lib.rs:1578) 단계에서 compose 연산마다 `compose_plan`을 순수 함수로 호출해 상한 계산(`rows += ids.len()*steps.len()`)과 검증을 모두 끝내고, 실행 단계에서는 계획을 재사용한다. 사전 계수의 분기도 `compose`를 명시적으로 센다.

### D-4 [수정 권장] 응답에서 연산 경계가 사라진다

- 근거: 결정표는 compose 결과가 `applied[]`·`created[]`에 "연산 순서대로 이어 붙는다"고 한다. compose는 단계 수만큼 `applied`를, create 단계마다 대상 수만큼 `created`를 만든다. `[apply, compose(전이 2단계), apply]`는 `applied`가 4개이고 호출자는 어느 항목이 어느 연산인지 알 수 없다. Python SDK는 `applied`를 "연산마다"로 문서화한다(__init__.py:193-194).
- 수정안: 응답에 연산별 요약을 추가형으로 더한다(예: `ops: [{"kind":"compose","applied":[a,b],"created":[c,d]}]`). 기존 `applied[]`·`created[]`는 그대로 둔다. 안 하면 compose 포함 묶음의 결과 해석을 SDK가 할 수 없다.

### D-5 [참고] 암호화 요청 변환기가 중첩 compose를 모른다

- `encrypt_request_for_write`와 `resolve_row_ids_for_create`(v6 encryption.rs:287-300, 377-406)는 `atomic` 안에서 `create`·`apply`만 내려간다. compose 연산은 건너뛴다. 암호화는 1.0 범위 밖이라 차단이 아니다. 다만 암호화 활성 정의에서 compose 연산·`refs`·`input`을 거절하는 한 줄이 필요하다. `refs`의 `scope: ref` 암호화는 `ref_values`가 `values`만 읽으므로(encryption.rs:437-441) 범위 id가 "0"으로 떨어진다. 같은 이유로 암호화 활성 정의에서는 `refs`를 거절한다.

## 6. test-plan 정정

| ID | 정정 |
| --- | --- |
| T-1 (A-S3) | 줄 없는 묶음의 실패 코드는 `WIRE.CHECK_FAILED`(lib.rs:1319)다. `WIRE.INVARIANT_VIOLATED`는 정원·배타 제약용이다. 결정표 P0-A 불변식 문장의 "확인 필요"는 SQL 수준 실행(부모+줄 0행, 부모만 1행)으로 근거가 섰다. 최종 확정은 PG 통합 시험의 몫이다 |
| T-2 (D-F1) | atomic 안 atomic은 `WIRE.UNKNOWN_KEY`다. 연산 객체가 `create` 분기로 떨어져 `keys(m, ["create","values"])`(lib.rs:1636)에 걸린다. `BAD_VALUE`가 아니다 |
| T-3 (C-F5) | C-1 수정이 없으면 이 시험은 통과할 수 없다(RED로 확인 가능) |
| T-4 (K-4) | 호출자가 받는 코드는 C-2 수정 전에는 `EXTENSION_ERROR`다. 수정 후에는 `WIRE.ALREADY_EXISTS` |
| T-5 | 추가 시험: B-1 음수·0 입력(`BAD_VALUE`), B-2 `allow`+`input`+가려진 필드(`DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`), B-6 벌크 효과 대상 충돌, D-3 compose 계획 오류가 앞 연산 실행 전에 나는지, A-1 `where` 전용 전이에 `target.created` |

## 7. 실행 증거

환경: `/opt/homebrew/opt/postgresql@17` 임시 클러스터(`/tmp/aip-review-pg2`, 포트 55987, 시험 뒤 `pg_ctl stop`으로 내림), 임시 crate `/tmp/aip-review-probe`(제품 코드를 경로 의존으로 읽기만 함), `CARGO_TARGET_DIR=/tmp/aip-target-review`.

```
$ cd /tmp/aip-review-probe && CARGO_TARGET_DIR=/tmp/aip-target-review cargo run --offline
P0-D bundle_needs_serializable(compose op, exposeCompose=serializable) = false
P0-D write_needs_serializable(top-level compose) = true
P0-D bundle_ops(compose in atomic) ok=true
P0-C v4 write_needs_serializable(access create.Todo, exposeCreate=serializable) = false
P0-B literal Decimal<12,2> "-100.00" => accepted
P0-B literal Decimal<12,2> "12.505" => WIRE.BAD_VALUE `Decimal<12,2>` 타입에 맞지 않는 값
P0-B literal Int(1..100) -5 => WIRE.BAD_VALUE 선언한 값 범위 초과
P0-B literal Int(no range) -1000000 => accepted

$ psql -f /tmp/aip-review-e1.sql   (PG 17.11)
E1 update-from 두 행이 한 대상: balance 100.00 -> 80.00 (한 번만 반영)
E2 ERROR: 22003: bigint out of range / ERROR: 22003: numeric field overflow
E2c numeric(12,2)에 '0.005'를 더함 -> 0.01 (조용한 반올림)
E4 balance - '-100.00' -> 180.00 (부호 반전)

$ psql -f /tmp/aip-review-a.sql & psql -f /tmp/aip-review-b.sql   (2세션)
B: ... FOR UPDATE 대기 후  id=1 | stock=5 | from_ok=f      (A가 10->5 커밋. 판정 from이 새 버전으로 재계산됨)

$ psql -f /tmp/aip-review-e5.sql
bad_rows_with_line = 0, bad_rows_without_line = 1           (check는 같은 트랜잭션의 줄을 본다)
```

## 8. 하지 않은 것과 한계

- 하지 않음: 저장소의 PG 통합 시험(`spike-v3-write`, `spike-v6-transport`, `spike-v4-worker`)은 돌리지 않았다. 이유: 작업 위치가 읽기 전용이고 시험 DSN은 작업자 전용 DB를 쓴다. 대신 같은 SQL을 임시 DB에서 직접 실행했다.
- 하지 않음: 구현이 아직 없는 동작(P0-A refs 해석, P0-B arith, P0-C create 분기, P0-D 중첩 compose)의 실제 실행. 코드 읽기와 기존 함수 호출로만 판정했다. 그래서 B-4의 지갑 교착 시나리오는 "코드 읽기, 미실행"이다.
- 확인 필요: 외부 DB 채택 모드(`externalStorage`)에서 NOT NULL을 검증하는지(3절 NULL). `docs/DECISIONS.md` D-A23 본문의 "확장·job 미착수"가 낡았는지 여부는 job만 확인했고 확장은 v4 write.rs:162가 전이만 본다는 점에서 맞다.
- 이 파일은 커밋하지 않았다.
