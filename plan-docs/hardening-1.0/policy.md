# 1.0 정책 경화: 권한·테넌트·필드 숨김 반례 탐색 (2026-11-21)

기준: `origin/main` 0c5d108d. 범위는 PostgreSQL 17 제품 경로(`aip service check/init/serve`)다. 동시성·트랜잭션 반례와 계약 정합성은 다른 작업자 담당이라 제외했다. MySQL은 1.0 범위 밖이라 보지 않았다.

방법은 두 가지다. 정의 단계는 `aip service check`로 빠르게 훑고, 통과한 반례는 실제 `aip service init/serve`와 PostgreSQL 17 컨테이너로 올려 HTTP 응답에서 가려진 값이 복원되는지 확인했다. 재현 스크립트는 [policy-probes/](policy-probes/)에 있고 한 줄 명령으로 다시 돌릴 수 있다.

```
AIP_BIN=<aip> AIP_TEST_DATABASE_URL='host=127.0.0.1 port=<포트> dbname=aip_test_ci user=runner' \
AIP_PROBE_PG_CONTAINER=<PG17 컨테이너> node plan-docs/hardening-1.0/policy-probes/policy-probes.mjs [시나리오 id 접두]
```

수정 전 바이너리에서 `D*` 시나리오는 `RECOVERED ...`로 값을 복원한다. 수정 뒤 바이너리에서는 같은 정의가 `aip service init`에서 `DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`으로 거부돼 `REJECTED`가 나온다. 대조군 `C*`와 알려진 한계 `K*`는 두 바이너리에서 같은 줄이다.

## 1. 확정 결함 (수정함)

모두 숨겨진 필드(`field ... read when`)의 값이 가려진 채 남지 않고 호출자에게 건너가는 경로였다. 고친 방식은 전부 정의 단계 거부(`DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`)이고 런타임 응답은 바꾸지 않는다. 회귀 시험은 `spikes/spike-v1-fixture/tests/policy_hardening.rs`의 같은 이름 시험이다. 모두 고치기 전에 실패(RED)했고 고친 뒤 통과(GREEN)했다.

| 시나리오 | 반례 이름 | 입력 | 기대 | 실제(수정 전, PG17 serve) | 판정 | 근거(시험 · 프로브 · 커밋) |
|---|---|---|---|---|---|---|
| D1 | `from` 술어 호출로 가려진 필드 읽기 | `transition t { from big(this) ... }`, `predicate big(d: Doc) = d.risk > 3`. 호출자는 `risk`를 못 읽음 | `from`은 가려진 필드를 읽을 수 없다(문서 규칙) | `from big(this)`가 정의 통과. risk 7인 행은 성공, risk 2인 행은 `WIRE.INVALID_STATE` | 확정 결함(수정) | `from_and_to_values_cannot_read_a_hidden_field_through_a_predicate` · `policy-probes.mjs D1` · 커밋 "from and to values cannot read a hidden field through a predicate call" |
| D2 | `to` 값에 술어 호출로 비트 복사 | `to flag = big(this)` | `to` 값 식은 가려진 필드를 읽을 수 없다 | 정의 통과. 적용 뒤 읽을 수 있는 `flag`가 doc1=true, doc2=false로 가려진 `risk > 3` 비트를 담음 | 확정 결함(수정) | 위와 같은 시험 · `D2` · 커밋 "from and to values cannot read a hidden field through a predicate call" |
| D3 | `actor.*` 가려진 필드와 `input` 이분 탐색 | `allow actor.score >= input.n`, `Member.score`는 호출자 행에서도 가려짐(5) | `input`과 가려진 필드를 한 비교식에서 못 읽는다(B-2) | n=5까지 성공, 6부터 `WIRE.FORBIDDEN`. 가려진 score 5 복원. `from actor.score`도 통과 | 확정 결함(수정) | `the_actor_root_is_checked_like_this_for_hidden_fields` · `D3` · 커밋 "hidden-field checks also follow actor-rooted paths" |
| D4 | 효과 값·대상 조건으로 복사·일치 | 효과 `create Log { val = this.risk }`, `update ... { val = this.risk }`, `update Log where val = this.risk`, `delete Log where val = this.risk` | 가려진 필드를 복사·비교할 수 없다 | `Log.val`에 risk 37이 그대로 복사돼 u2가 읽음. 대상 조건은 일치 여부가 전이 성공·실패로 나옴 | 확정 결함(수정) | `effects_cannot_copy_or_match_a_hidden_field` · `D10` · 커밋 "effect values and match conditions cannot read a hidden field" |
| D5 | compose `create ... from` 출처 복사 | `create OrderItem from this.owner, this.sku, this.qty`, `Cart.sku`는 `ordered = true`일 때만 읽힘 | 가려진 필드를 읽을 수 있는 행으로 복사할 수 없다 | `OrderItem.sku`에 `SECRET-SKU-9`가 복사돼 호출자가 읽음 | 확정 결함(수정) | `compose_create_sources_cannot_copy_a_hidden_field` · `D9` · 커밋 "compose create sources cannot copy a hidden field" |
| D6 | `sameScope`에 가려진 필드 | `expose apply t { ...; sameScope risk }`, `[내 행(7), 남의 행]` | 가려진 값의 일치 여부가 응답으로 나가지 않는다 | `[1,4]`(둘 다 7)는 성공, `[1,2]`(7 대 2)는 `WIRE.NOT_SAME_SCOPE`. 값을 한 값씩 확인 | 확정 결함(수정) | `same_scope_cannot_be_a_hidden_field` · `D5` · 커밋 "sameScope cannot be a field with a read policy" |
| D7 | `expose create allow`에 새 행 값과 가려진 필드(술어·exists) | `allow riskAbove(cap)`, `predicate riskAbove(n: Int) = exists Doc where org = 1 and risk >= n` | 호출자가 정하는 값과 가려진 필드를 견줄 수 없다 | cap=37 생성 성공, cap=38 `WIRE.FORBIDDEN`. 남의 risk 37 복원 | 확정 결함(수정) | `create_allow_cannot_compare_caller_values_with_a_hidden_field` · `D7` · 커밋 "expose create allow cannot compare caller values with a hidden field" |
| D8 | `expose create allow`에 참조 이동 뒤 가려진 필드 | `allow owner.score >= cap` | 위와 같다 | cap=37 성공, 38 `FORBIDDEN`. 가려진 `Member.score` 37 복원 | 확정 결함(수정) | 위와 같은 시험 · `D8` · 커밋 "expose create allow cannot compare caller values with a hidden field" |
| D9 | `check`에 가려진 필드와 호출자가 쓰는 필드 | `check c when risk <= cap`, `transition t { patch cap }` | 호출자가 쓰는 값과 가려진 필드를 한 check가 함께 읽을 수 없다 | cap=6까지 `WIRE.CHECK_FAILED`, 7부터 성공. 남의 risk 7 복원 | 확정 결함(수정) | `a_check_cannot_compare_a_hidden_field_with_a_caller_written_field` · `D4` · 커밋 "a check cannot compare a hidden field with a caller-written field" |
| D10 | 집계 `where`에 `input`과 가려진 필드 | 단독 집계 `where secret >= input.n`(count, `callerFilter none`) | 입력과 가려진 필드를 견줄 수 없다 | u2(행도 못 읽음)가 n=0→2, 6→1, 37→1, 38→0으로 남의 secret 37, 5 복원 | 확정 결함(수정) | `aggregate_where_cannot_compare_input_with_a_hidden_field` · `D6` · 커밋 "aggregate where cannot compare input with a hidden field" |
| D11 | 집계 `sourceAccess` guard에 `input`과 가려진 필드 | `sourceAccess guardA(actor, input.n)`, `access guardA(a: Member, n: Int) = a.score >= n` | guard 본문도 같은 금지를 따른다 | n=5까지 통과, 6부터 `WIRE.ACCESS_DENIED`. 자기 행의 가려진 score 5 복원 | 확정 결함(수정) | `aggregate_access_guard_cannot_compare_input_with_a_hidden_field` · `D11` · 커밋 "operation allow and aggregate access guards cannot compare input with a hidden field" |
| D12 | operation `allow`에 `input`과 가려진 필드 | `operation read probe { allow riskAbove(input.n) }` | 위와 같다 | n=37까지 성공, 38은 `WIRE.ACCESS_DENIED`. 읽을 수 없는 행의 risk 37 복원 | 확정 결함(수정) | `operation_allow_cannot_compare_input_with_a_hidden_field` · `D12` · 커밋 "operation allow and aggregate access guards cannot compare input with a hidden field" |

판정 기준과 허용 범위:
- `allow`와 효과 `when`의 **호출자가 고르지 않은 서버 상수와의 한 비트 비교**(`allow actor.score >= 3`, `update ... when big(this)`)는 B-2 결정대로 허용한다. 복사나 `input` 이분 탐색이 아니기 때문이다.
- 새 행 자신의 필드는 호출자가 준 값이라 `expose create allow`에서 숨길 것이 없다. 참조 이동 뒤·`actor`·술어 본문의 가려진 필드만 본다.
- `check`는 가려진 필드가 모두 호출자가 쓰는 필드이면(예: `check risk >= 0`과 `patch risk`) 거부하지 않는다. 호출자는 그 값을 이미 안다.
- 술어 호출 검사는 본문을 매개변수 타입으로 따라가므로 `create allow`에서 `big(this)`처럼 새 행 전체를 넘기고 본문이 가려진 필드를 읽으면 보수적으로 거부한다(새 행 자신의 가려진 필드도 본문 안에서는 구분하지 않는다). 저장소 정의 68개와 이 저장소의 시험 정의에는 해당 사례가 없었다.

## 2. 설계 결정이 필요한 것 (고치지 않음)

응답 값이나 기존 정의 호환에 영향이 있어 소유자 결정이 필요하다. 이번에는 바꾸지 않았다.

| 시나리오 | 반례 이름 | 입력 | 실제 | 판정 | 선택지와 영향 |
|---|---|---|---|---|---|
| F1 | 집계 `where`의 `input`이 가려지지 않은 필드를 숨은 행에 견줌 | `where amount >= input.n`(count, `callerFilter none`, 원본에 `rows read`) | count는 `rows read`를 상속하지 않아 u2가 읽을 수 없는 행을 센다(D10의 n=0이 2를 돌려줌). `callerFilter read`는 where가 정책을 함의하도록 이미 막았지만 `input`이라는 다른 통로에는 같은 검사가 없음 | 기존 알려진 한계(공개 count 계약, decisions.md O1~O5)의 변형 | (a) where에 `input`이 있는 count는 `callerFilter read`와 같은 규칙으로 정책 함의를 요구한다. 이번 시도에서 `spikes/spike-v1-fixture/fixture/recruitment.aip`의 `approvedCount`(`where recruitment.club.id = input.clubId`, `sourceAccess clubManagerOnly(actor, input.clubId)`)처럼 **guard가 같은 `input`을 이미 제한하는 기존 정의가 거부돼** `cardinality_capacity` 시험이 실패했다. 되돌렸다. (b) (a)에서 sourceAccess guard 인자에 같은 `input.*`가 들어가면 면제한다. 구조로만 판정하므로 guard가 실제로 제한하는지는 못 본다. (c) 경고 `DEF.ADVISORY_PUBLIC_COUNT`를 `input`이 where에 있는 count로 넓힌다(거부 없음, 응답 불변). 1.0에는 (c), 정식 결정은 O2~O5와 함께 권고 |
| R1 | 읽기 확장의 `ctx.data.read`에 `access read.<Resource>` 선언이 없음 | `extension read`에는 `read.<Resource>`를 선언하는 문법이 없고(`DEF.UNKNOWN_SYMBOL`), `ctx.data.read`는 선언 없이 `expose read`가 있는 어느 resource든 읽는다. FaceReview의 `Video.homeSearch`, `User.myRecentVideos`가 그 예 | C7: 호출자의 행·필드 정책, 테넌트 범위, 계약, 노출 여부가 공개 `/read`와 같이 적용돼 **우회는 못 찾았다**(§3). 선언이 없어서 생기는 위험은 권한 확대가 아니라 "확장 코드가 어느 resource를 읽는지 계약에 안 적힘"이다 | 기존 알려진 한계(선언 부재). 결함 아님. 공개 계약을 바꾸는 수정이라 고치지 않음 | (a) 읽기 확장에 `access read.<Resource>`를 허용하고, 하나라도 선언한 확장에서만 목록 밖 읽기를 거부한다(opt-in. 기존 확장 호환, 선언한 확장만 보호). (b) 선언을 필수로 한다. 이전에 시도된 브랜치가 퇴보로 판정된 이유(FaceReview `homeSearch`·`myRecentVideos`가 `WIRE.ACCESS_NOT_DECLARED`로 깨짐)와 같아 호환이 깨지고 sema·SDK·문서 변경이 따른다. (c) 정의 검사 경고만 더한다(`ctx.data.read`가 정적으로 보이지 않아 어렵다). 1.0에는 (a)가 호환상 가장 작다. 결정은 소유자 |
| K1 | `unique`에 가려진 필드 | `unique code`, `code`는 가려진 필드. u2가 `create`로 code 값을 시도 | code=4242는 `WIRE.ALREADY_EXISTS`, code=1은 성공. 가려진 값의 존재를 한 값씩 확인 | 기존 알려진 한계(definition-syntax의 `unique`·`limit`·`invariant` 존재 노출과 같은 계열이나 가려진 필드 값을 직접 추측하는 점이 더 강함) | (a) `unique`에 field read 정책 필드가 있으면 정의 오류로 거부(기존 정의 호환 영향 있음, 저장소 정의 68개 중 해당 없음). (b) 현행 유지하고 문서에 가려진 필드 값이 `unique`로 확인될 수 있다고 쓴다. (c) 경고를 더한다 |
| K2 | 가려진 범위 필드의 리터럴 증가 | `to risk = risk + 1`, `risk: Int(0..100)`(가려짐). u2가 반복 호출 | 37번 성공 뒤 `WIRE.BAD_VALUE`. 63(=100-37)을 복원. 값은 파괴됨 | 기존 알려진 한계. `input` 증감은 이미 거부했고 리터럴만 남음 | (a) 가려진 필드의 `f = f ± 리터럴`도 range가 있으면 거부(숨은 카운터 패턴 호환 영향). (b) range가 충분히 크면(호출 횟수 비용) 허용하고 문서화. 지금은 (b) |
| K3 | 숨은 자식 행이 참조하는 부모 삭제 | `delete` 전이, 숨은 `Child`가 `Parent`를 참조 | 자식 있는 부모는 `WIRE.REFERENCE_CONFLICT`, 없는 부모는 성공. 숨은 행의 존재가 드러남 | 기존 알려진 한계(`unique`·`invariant`의 존재 노출과 같은 계열) | (a) 현행 유지, 문서에 FK 거부도 숨은 행의 존재를 알린다고 쓴다. (b) 거부 사유를 합쳐 `WIRE.INVALID_STATE` 같은 일반 코드로 돌린다(응답 코드 변경) |

## 3. 정상으로 확인한 것

| 시나리오 | 확인 | 결과 | 근거 |
|---|---|---|---|
| C1 | 다른 테넌트의 id와 없는 id는 create 참조, apply 대상, filter, patch 참조, 섞인 대상 목록에서 같은 응답 | 6쌍 모두 byte 단위로 같은 `WIRE.MISSING_TARGET`·빈 목록(`requestId`만 다름) | `policy-probes.mjs C1` |
| C2 | bundle의 `refs`·`target.created`로 호출자가 읽을 수 없게 만든 행 가리키기 | 둘 다 `WIRE.MISSING_TARGET` | `C2` |
| C3 | 확장 `ctx.data`: 가려진 필드 읽기(null), 가려진 필드 filter, 다른 테넌트 행, access에 없는 읽기, internal 전이, 남의 행 전이 | null, `EXTENSION_ERROR`(거부), 빈 행, 거부, 거부, 거부. 자기 행 전이만 성공 | `C3` |
| C4 | internal 전이를 apply·bundle·compose에서 부르기, 잘못된 토큰 | 전부 `WIRE.NOT_EXPOSED`, 토큰은 `WIRE.UNAUTHENTICATED` | `C4` |
| C5 | `after` 커서에 가려진 필드 키, 가려진 필드 sort·filter, 참조 filter | `WIRE.BAD_CURSOR`, `WIRE.SORT_NOT_ALLOWED`, `WIRE.FILTER_NOT_ALLOWED`, 보이지 않는 대상의 참조 filter는 빈 목록 | `C5` |
| C6 | 같은 멱등 key를 다른 주체가 같은 본문으로 보내기 | 저장된 결과를 재생받지 않는다. key 충돌도 드러나지 않는다 | `C6` |
| C7 | 읽기 확장의 `ctx.data.read`: 가려진 필드, 가려진 필드 filter, 다른 테넌트 행(org 조건·id 지정), `expose read` 없는 resource, 관계 대상 전용(budget 없음) resource, select·sort·filter 연산·limit가 계약 밖인 질의, 알 수 없는 키 | 가려진 필드는 `null`. 나머지는 빈 목록 또는 `WIRE.EXTENSION_ERROR`(거부). 같은 질의의 확장 결과와 공개 `/read` 결과가 같고, 확장 입력의 `Doc.Id`는 형식만 검사해서 다른 테넌트 id와 없는 id의 응답이 같다 | `policy-probes.mjs C7` |
| C8 | 읽기 확장의 `ctx.data.aggregate`·`apply`·`create`·`jobs` | 선언한 집계만, 확장 입력에 묶인 값으로만 된다. 입력을 바꾸면 거부, 선언 안 한 집계 거부, `apply`·`create`·`jobs`는 읽기 확장에서 거부 | `C8` |
| S1 | 가려진 필드의 filter·sort, sum·min·max 집계, traverse sort, `callerFilter read` 정책 함의, 가변 증감 대상 | 정의 단계에서 이미 거부(`DEF.POLICY_FIELD_NOT_FILTERABLE`, `DEF.POLICY_FIELD_NOT_AGGREGATABLE`, `DEF.CALLER_FILTER_POLICY_REQUIRED`, `DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`) | `aip service check`로 변형 정의를 직접 시도 |
| S2 | 오류 본문 | 유일 제약·참조 무결성·check 거부 메시지에 행 id나 값이 없다(`생성: 유일 제약 위반`, `삭제: 참조 무결성 제약 위반. 전체 취소`) | `D*`·`K*` 시나리오 출력 |

## 4. 하지 않은 것과 한계

- 동시성·트랜잭션(교착, 직렬화 재시도, 멱등 동시 재전송)은 범위 밖이라 보지 않았다. 계약 정합성(지문, facts, SDK 일치)도 같다.
- UUID id 추측은 보지 않았다(정수 id로만 확인). 시간 부채널(응답 시간 차이)은 측정하지 않았다.
- 확장 `ctx.data.create` 경로는 C3처럼 읽기·전이만 시도했다. 암호화 필드, Mongo·Redis, MySQL 경로는 보지 않았다.
- 읽기 확장(C7·C8)은 PostgreSQL 경로로만 시험했다. FaceReview의 `homeSearch`·`myRecentVideos`는 MySQL 경로라 그대로는 못 돌렸지만, `ctx.data.read` 처리(`spike-v4-worker`의 `serve_call`)가 같은 `plan_read_in`을 방언만 바꿔 부른다. MySQL 방언 쪽 차이는 확인하지 못했다.
- 읽기 확장의 `ctx.data.read`는 공개 `/read`와 달리 암호화 필드의 blind index 준비와 복호화를 거치지 않는다(`spikes/spike-v6-transport/src/lib.rs`의 `/read` 경로와 비교한 코드 읽기). 평문을 내주는 쪽이 아니라 암호화 필드를 못 쓰는 쪽의 차이라 보안 결함으로 보지 않았고, 암호화 설정을 켠 실행은 하지 않았다(미실행).
- 확장 코드는 서버가 신뢰하는 코드이고 `ctx` 호출 횟수에는 상한(`MAX_CTX_CALLS`)이 있다. 한 확장이 여러 번 읽어 호출자의 한 번 읽기 상한(`budget rows`)보다 많은 행을 반환하는 것은 확장 작성자의 선택으로 보았다(시험하지 않음).
- D1~D12는 정의 단계 거부로 막았다. 이미 배포된 정의가 이 패턴을 쓰면 다음 `service check`·`init`·`serve`에서 새로 거부된다. 저장소의 `.aip`·`.e.ts`·`.h.ts` 정의 68개는 수정 전후 `aip service check` 결과가 같다(변경 0건).
- `allow`의 한 비트 상수 비교와 §2의 항목은 의도적으로 남겼다. 한 비트 비교는 B-2 결정, §2는 소유자 결정이 필요하다.
