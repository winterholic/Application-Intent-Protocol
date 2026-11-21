# 실무 시나리오 표현력 감사 (기준 4e132564)

## 등급

| 등급 | 뜻 |
| --- | --- |
| A | 선언만으로 제품 경로에서 된다. 실행 근거(PG 시험 또는 serve 실행)가 있다 |
| B | 선언으로 되지만 작성 방식 제약·우회가 필요하거나, 근거가 check뿐이다 |
| C | 쓰기 확장(`extension write`)이 필요하다. 한 트랜잭션·한 멱등 키로 원자성은 유지된다 |
| D | 확장으로도 원자성이 깨지거나(호출 두 번), 제품 코드는 있으나 1.0 보장 밖이다 |
| E | 제품 경로에서 안 된다(PoC에만 있거나 없음) |

문서만 근거인 항목은 A로 두지 않았다. "실행"은 이번 세션에서 실제로 돌린 것이고, "기존 시험"은 저장소 PG 시험을 이번에 다시 돌려 통과를 본 것이다.

## 시나리오 20개

9·10번은 같은 요구(완료 시 다음 회차 생성)의 두 변형이라 한 시나리오로 센다. 표는 21행이다.

원인: **한계** = AIP 표현력 한계, **작성** = 소비자 작성 방식으로 풀림, **문서** = 문서·SDK·진단 문제.

| # | 시나리오(프레임워크 관용구) | 등급 | 근거 경로 | 증거 | 원인 |
| --- | --- | --- | --- | --- | --- |
| 1 | 게시글 CRUD·부분수정·`@Version` 낙관적 잠금(Spring JPA) | A | parser.rs:554 `patch`/`expect`, sema.rs:1238, v3 lib.rs:1414 `apply_input`, :515 `update`(expect를 `IS NOT DISTINCT FROM`로 결합) | 실행: fx `s01` check ok, 기존 시험 `spike-v3-write --test general_update` 1 passed | - |
| 2 | 주문+주문줄 원자 생성(NestJS cascade save) | D | v3 lib.rs:1603 `bundle_run`: 연산 키 `create`,`values`만, 단계 간 값 전달 없음 | 실행: `{"order":{"ref":0}}` → `WIRE.BAD_VALUE`, 별칭 키 `as` → `WIRE.UNKNOWN_KEY` | 한계(E1) |
| 3 | 입력 수량만큼 재고 차감 `decrement!(:stock, qty)`(Rails) | C | sema.rs:520 `counter`(리터럴만), parser.rs:554 transition 키에 `input` 없음 | 실행: fx `s03` `DEF.ARITH_NOT_ALLOWED`, `s03b` `DEF.PARSE_UNKNOWN_KEY`(`input`) | 한계(E2) |
| 4 | 지갑 잔액 Decimal 증감(FastAPI) | C | sema.rs:522 counter는 non-null Int만 | 실행: fx `s05` `DEF.TYPE_MISMATCH`(`Decimal<12,2>`) | 한계(E2 확장) |
| 5 | 다른 행 조건으로 결제 가능 판정 후 상태 변경(Spring Service) | A | `allow ... and canPay(actor, this)`의 `exists` predicate, v3 `judge` | 실행: fx `s05b` check ok, 기존 시험 `counters`·`general_update` 통과(조건부 판정 경로) | - |
| 6 | 결제 시 다른 행(지갑) 차감 `balance = balance - this.total` | C | parser.rs:573 효과 `update` 값은 산술 미파싱 | 실행: fx `s05b` 변형 `DEF.PARSE_EXPECTED`(`-` 발견) | 한계(E2) |
| 7 | 주문 확정: 재고 차감·합계·상태 전이(Saleor checkout) | C | LIMITS E1~E3 `shop.place` 확장 | 문서(LIMITS "실행" 표기). 이번 세션은 재실행 안 함 | 한계(E2,E3) |
| 8 | 다단계 승인·반려·본인 승인 금지·일괄 승인(Spring 결재) | A | enum `from`/`to`, predicate `allow`, `expose apply ... where; bulk` | 실행: fx `s06` check ok, 기존 시험 v3 전이 시험군(`general_update`,`counters`) | - |
| 9 | 완료 시 다음 회차 생성, 값은 대상 행에서 | B | sema 효과 `create R { f = this.f }`, v3 lib.rs:795 `effects` | check: fx `s13` ok | 작성 |
| 10 | 완료 시 다음 회차 생성, 값은 계산(마감일+7일) | E | v4 write.rs:565 PG 쓰기 확장 `create` 거부 | 실행: serve `/apply` → `WIRE.EXTENSION_ERROR`, 로그 `cause:"ctx.create.unsupported"`, 대상 행 `done=f` 유지(롤백) | 한계(E7) |
| 11 | 목록: 필터·정렬·cursor·to-one·to-many N개(GraphQL 대체) | A | v2 plan.rs:285 `plan_read_in`, sqlgen | 실행: fx `s07c` check ok, 기존 시험 `spike-v2-read --test keyset_cursor` 7 passed, `--test traverse_many` 4 passed | - |
| 12 | nullable 마감일 정렬 + cursor | B | sema.rs:2012 `DEF.CURSOR_NULLABLE_SORT` | 실행: fx `s09` 오류 확인, offset 우회는 LIMITS check | 한계(E5) |
| 13 | 관계 필드로 필터·정렬(`owner.org.eq`) | E | parser `filter`는 이름 하나만 | 실행: fx `s07b` `DEF.PARSE_EXPECTED`(`.` 발견) | 한계 |
| 14 | 게시물별 댓글 수·Int 합계(Rails counter_cache) | B | sema.rs:1673 `release count/sum/min/max`, 공개 count 계약 | 실행: fx `s07c` ok, `s15` `DEF.ADVISORY_PUBLIC_COUNT` 경고, 기존 시험 `spike-v2-read --test aggregate_row_policy` 1 passed | 작성 |
| 15 | Decimal 합계·평균·`Σ 수량×단가` | E | sema.rs:1673(Int `sum`만), lexer `*` 없음 | 실행: fx `s07a` `DEF.TYPE_MISMATCH`, `s18` `DEF.LEX_UNEXPECTED_CHAR`(`*`) | 한계(E3) |
| 16 | 역할·소유자·필드 가림 권한(Spring Security, Pundit) | A | `rows read`, `field read`, predicate, v2 정책 SQL | 기존 시험: `spike-v2-read --test security_authorization` 2 passed, `spike-v3-write --test create_ref_visibility` 1 passed, fx `s06` check | - |
| 17 | 테넌트 격리(`org = actor.org`) | B | 자동 격리 없음(SUPPORT 13 제외), 정책마다 반복 | check: fx `s01`,`s06` 모든 정책에 수동 반복 | 작성 |
| 18 | 다른 사용자를 지정해 공유 생성 | B | v3 lib.rs:1075 `ensure_refs_readable` | 기존 시험: `create_ref_visibility` 1 passed | 작성(E4) |
| 19 | 비동기 job·외부효과(메일)·재시도 복구(Celery, Sidekiq) | D | `operation job`, durable PG job store만(SUPPORT 17), 외부효과 멱등 미보장(SUPPORT 12) | check: fx `s14` ok(`workers.required: true`) | 한계 |
| 20 | 파일 업로드·실시간 구독 | D | objects API(MinIO만 확인, SUPPORT "PostgreSQL 17 한정"), realtime·data subscription은 지원 표 밖 | 근거 없음(이번 실행 안 함, 문서상 보장 밖) | 한계 |
| 21 | 서버 확장 호출을 SDK 타입으로(TS·Python) | B | v5 sdk generic.ts `WriteExtensionDescriptors`, python.rs:167(apply 입력 중심) | 실행: serve `/apply {"extension":...}` 경로 동작. ABI 확정 전(SUPPORT 16) | 문서·SDK |

합계(21행): A 5(1,5,8,11,16), B 6(9,12,14,17,18,21), C 4(3,4,6,7), D 3(2,19,20), E 3(10,13,15). 7번(C)은 LIMITS 문서의 실행 기록만 근거라 등급 상한을 C로 둔 것이고 이번 세션에 재실행하지 않았다. 8번 A는 이 정의 자체는 check만 했고, 같은 구문(enum 전이·predicate allow·bulk where)의 PG 실행은 기존 v3 시험군 근거다.

## 이번 세션 실행 증거

```
aip service check /tmp/aip-expr-fx/s11_pg_ext_create.aip
{"advisories":[],...,"ok":true,"workers":{"extensions":[{"implementation":"todo.completeAndRepeat","kind":"write",...}],"required":true}}

POST /apply {"key":"r1","request":{"extension":"Todo.completeAndRepeat","input":{"id":"10"}}}
{"code":"WIRE.EXTENSION_ERROR","msg":"공식 WRITE 확장 호출 거부","ok":false,...}
serve.log: {"cause":"ctx.create.unsupported","code":"WIRE.EXTENSION_ERROR",...}   psql: 10|f

POST /apply {"key":"r2","request":{"atomic":[{"create":"Order",...},{"create":"OrderLine","values":{"order":{"ref":0},"qty":1}}]}}
{"code":"WIRE.BAD_VALUE","msg":"선택한 wire의 Id 형식·범위에 맞지 않음",...}
POST /apply (같은 key r2c 두 번) → 두 번째 {"created":[{"id":"2","resource":"Order"}],"ok":true,"replayed":true,...}
```

## E1~E7 재검증 (4e132564 코드)

| # | 여전히 유효 | 현재 코드 근거 | 이번 확인 |
| --- | --- | --- | --- |
| E1 부모·자식 원자 생성 불가 | 유효 | v3 lib.rs:1603 `bundle_run`, :1578 `bundle_ops` | 실행(R2) |
| E2 가변량 증감 불가 | 유효. 효과 `update` 값은 산술 자체가 파싱되지 않는다(문서에 없는 추가 한계) | sema.rs:520, parser.rs:573 | check(s03, s03b, s05, s05b) |
| E3 계산 집계 불가 | 유효. Decimal `sum`도 거부 | sema.rs:1673 | check(s07a, s18) |
| E4 참조 생성은 대상 행 가시성 필요 | 유효 | v3 lib.rs:1075 | 기존 PG 시험 `create_ref_visibility` 통과 |
| E5 cursor + nullable 정렬 불가 | 유효 | sema.rs:2012 | check(s09) |
| E6 `from` 필수 | 유효(진단이 `from true`를 안내) | sema transition 검사 | check(s10) |
| E7 PG 쓰기 확장 create 불가 | 유효. 다만 **check는 통과시키고 런타임에서만 거부**한다 | sema.rs:2217이 `create.<R>` access 허용, v4 write.rs:565가 거부 | check(s11) + 실행(R1) |

## 범위 밖 발견(고치지 않음)

- E7 check·runtime 불일치: `extension write ... access create.Todo`가 `aip service check`를 통과하고 serve에서만 `ctx.create.unsupported`가 된다. P0-C가 기능으로 메우므로 별도 진단 수정은 하지 않는다. P0-C가 1.0에서 빠지면 sema에서 `DEF.UNSUPPORTED`로 막아야 한다.
- LIMITS E2 본문은 "확장의 어떤 실패도 같은 `WIRE.EXTENSION_ERROR`"라고 쓰지만 33abdaee 이후 `ctx.reject(code, message)`가 `WIRE.EXTENSION_REJECTED`와 업무 코드를 돌려준다(`spike-v4-worker/tests/write_ctx_key_and_rejection.rs` X2). 문서가 코드보다 오래됐다.
- 로컬 셸에 `PGHOST`·`PGPORT`가 설정돼 있어 `aip-test-support` build.rs가 strict-db 빌드를 막는다. PG 시험 전에 `unset PGHOST PGPORT`가 필요하다.
