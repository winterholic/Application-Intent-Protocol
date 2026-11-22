# Track A 소유자 결정 항목 (공개 count, 잔여 추론 K1~K4, 테넌트 조언)

작성: 2026-11-22, 브랜치 `stab/a-20261122`. 여기 있는 항목은 응답 값, 응답 코드, 기존 정의가 받아들여지는 범위를 바꾸므로 Track A가 확정하지 않았다. 결정 대기로 독립 작업을 멈추지 않았고, 호환 유지형 경고와 판정 순서 수정은 이미 들어갔다(`track-a.md`). 근거 문서는 `plan-docs/hardening-1.0/policy.md`, `plan-docs/expressiveness-1.0/decisions.md`(공개 count O1~O5).

| ID | 주제 | 지금 상태 | 추천 |
|---|---|---|---|
| D-A1 | 공개 count의 기본 의미 | 불변(원본 `rows read` 비상속). 경고 2종 | 3안(추가형 구분)을 1.0.x에, 기본 전환은 1.1 약속 여부를 소유자가 |
| D-A2 | `where`에 `input`이 있는 count(F1, A-01) 거부 여부 | 경고 `DEF.ADVISORY_PUBLIC_COUNT_INPUT`만 구현 | 경고 유지. (a)·(b) 거부는 D-A1 결정 뒤 |
| D-A3 | K1: `unique`에 숨은 필드 | 그대로. 현재 값을 시험으로 고정 | (b) 문서화 + 경고 후보 |
| D-A4 | K2: 숨은 범위 필드의 리터럴 증감 | 그대로. 시험으로 고정 | (b) 문서화 |
| D-A5 | K3: 숨은 자식 때문에 부모 삭제가 `REFERENCE_CONFLICT` | 그대로. 시험으로 고정 | (a) 문서화 |
| D-A6 | K4 잔여: allow가 `default database` 필드를 읽는 외부 create | 판정 순서로 좁힘(남은 경우만 노출) | (a) 정의 거부, 단 기존 정의 영향 조사 후 |
| D-A7 | 테넌트 조언의 구조적 한계(관계 경로 테넌트, `exists`, 근거 없는 정의) | 표로 기록, 개선은 확실한 것만 | 현행 유지 |

## D-A1 공개 count의 기본 의미

**문제.** `release count`는 원본의 `rows read`를 적용하지 않는다. 같은 정의의 `sum`·`min`·`max`는 적용한다(`spikes/spike-v2-read/src/plan.rs:119 measure`). 그래서 호출자는 자기가 읽을 수 없는 행의 수를 count로 알 수 있다. 여기에 호출자가 고르는 `input`이 `where`에 있으면(F1) 숨은 행의 필드 값 분포까지 이분 탐색으로 복원한다.

**재현 근거(PG17, 시험은 `crates/aip-cli/tests/service_flow.rs`).** `service_policy_public_count_counterexample_matrix_http_postgres`는 읽을 수 있는 행 2개(amount 10, 20), 같은 org의 남의 행 1개(500, kind `secretkind`), 다른 org의 행 1개(900)로 다음을 고정한다.

| 반례 | 정의 | 호출자(읽을 수 있는 행 2개)가 얻는 값 |
|---|---|---|
| 읽을 수 없는 행 추론 | `where` 없는 count | 4(읽을 수 없는 2개와 다른 테넌트 1개 포함). 숨은 행 하나를 더하면 5가 된다 |
| 같은 정의의 count와 sum | `release sum(amount)` | 30(읽을 수 있는 행만). 숨은 행을 더해도 안 변한다 |
| `input` + `where` 값 탐색 | `where amount >= input.n`, count | n을 바꿔 가며 이분 탐색하면 다른 테넌트의 행 값 900까지 복원. 같은 `where`의 sum은 읽을 수 있는 행의 최댓값 20만 드러낸다 |
| 값 존재 확인 | `where kind = input.k`, count | `secretkind`=1, `otherkind`=1(다른 테넌트), 없는 값=0 |
| sourceAccess 있음 | guard가 거짓 | `WIRE.ACCESS_DENIED`. guard가 `input`을 받으면(`ofOrg(actor, input.o)`) 자기 org는 숨은 행까지 3, 남의 org는 거부 |
| sourceAccess 없음 | 선언 생략 | 정의 오류 `DEF.MISSING_ITEM` |
| `callerFilter read` | `where owner = actor`가 원본 정책을 함의 | 호출자 filter로도 읽을 수 있는 행 안에서만 센다(`secretkind`=0). 함의하지 않으면 정의 오류 `DEF.CALLER_FILTER_POLICY_REQUIRED` |
| 다른 호출자 | 같은 데이터 | count는 호출자와 무관하게 4, sum은 호출자의 행만 |

**선택지 비교.** "저장소 영향"은 저장소의 `.aip` 67개 가운데 정의 오류인 8개(의도적 반례·조각)를 뺀 59개(같은 정의의 `.e/.h` 변형은 같은 정의라 한 번만 센다)에서 `release count` 28개를 `where`가 원본 `rows read`를 함의하는지로 나눈 값이다(측정: 임시 시험으로 facts를 읽음). 함의함 14개(FaceReview 7, FlaskBB 4, 표현력 앱 community·saas 각 1, 표현력 fixture 1)는 어느 안에서도 값이 안 변한다. 함의하지 않는 14개가 값이 바뀔 수 있는 대상이다: `where` 없음 8개, 상수 `where` 5개, `input` `where` 1개(`recruitment.aip` `approvedCount`). 파일로는 9개이고 그 가운데 독립 소비자 앱 3개(`taskshare` 1, `stockdesk` 1, `stockdesk-1.1.0-src` 1)가 들어 있다.

| | 1안 현행 유지 + 선언적 공개 의도 요구 | 2안 count에 `rows read` 기본 상속 | 3안 공개 집계와 호출자 가시 집계를 표기로 구분(추가형) |
|---|---|---|---|
| 내용 | 응답 값은 그대로. `release count public` 같은 표기를 받고, 표기 없는 공개 count는 지금은 경고, 나중에 오류(`plan-docs/expressiveness-1.0/decisions.md` O2의 값 불변 변형) | `measure`가 count에도 `FILTER (WHERE visible)`를 건다(sum과 같은 경로). 공개 총계는 표현할 수 없게 된다(O3) | 새 표기 `release count visible`(상속)을 더하고 기존 `release count`는 현재 의미(공개)를 유지한다(O4). 기본 전환은 별도 결정(O5) |
| 보안성 | 표기를 쓴 정의는 의도가 문서화된다. 표기를 강제하기 전까지 숨은 행 추론은 그대로 열려 있다 | 가장 높다. 숨은 행 수·값 탐색이 count에서 사라진다 | 새 표기를 쓰는 정의만 높다. 기본이 공개라 모르고 쓰면 그대로 노출된다. 경고가 이를 알린다(구현됨) |
| 호환성(저장소) | 경고 단계는 영향 0. 오류 단계는 14개(9파일, 소비자 앱 3)에 표기를 추가해야 한다 | **14개의 응답 값이 바뀔 수 있다**(9파일, 소비자 앱 3). 계약 지문은 그대로라 구 클라이언트가 조용히 다른 값을 받는다. 지문 변경으로 막을지 별도 결정 | 영향 0. 기존 정의와 응답 값 불변 |
| 작성 부담 | 공개 count마다 표기 한 단어 | 공개 총계가 필요하면 우회가 없다(확장으로만) | 보호하고 싶은 count만 `visible`로 바꾼다 |
| 런타임 복잡도 | 낮다. 파서와 sema에 표기 추가, 런타임 불변(표기를 facts에 넣으면 digest가 바뀌므로 count facts에는 넣지 않는다) | 낮다. `plan.rs:119`의 count 분기 몇 줄. 단 행 정책에 `exists`가 있으면 행마다 부질의가 붙어 느려진다. `callerFilter read`의 정책 함의 검사는 불필요해져 제거할 수 있다 | 중간. `visible` 표기만 facts 키(`releaseVisible`)가 생기고 기존 count의 digest는 안 바뀐다. `measure`에 sum과 같은 분기를 count에 하나 더한다 |
| 코드 변경 범위 | `sema.rs:2197 aggregate`(release 파싱), `syntax_card.rs`, 생성 문서, SDK 계약 설명 | `plan.rs:119 measure`, `plan.rs:1015 plan_aggregate`, `sema.rs:2386`(경고 문구), 문서 `product/SDK.md` "공개 count 계약", `SUPPORT.md` | `sema.rs aggregate`, `plan.rs measure`, `syntax_card.rs`, SDK 생성(릴리스 형태 표기), 문서 |

**추천.** 3안을 1.0.x의 가산 변경으로 도입하고(응답 값 불변, 기존 정의 불변), 지금 구현한 경고 `DEF.ADVISORY_PUBLIC_COUNT`·`DEF.ADVISORY_PUBLIC_COUNT_INPUT`을 그대로 둔다. 1안의 오류 단계와 2안의 기본 전환은 1.0에 넣지 않는다. 이유: 2안은 저장소의 14개와 독립 소비자 앱 3개의 응답 값을 조용히 바꾸고, 1안의 오류 단계는 같은 14개 정의를 새로 거부한다. 3안은 둘 다 0이다. 소유자 결정은 두 가지다. ① 1.0에 `visible` 표기를 넣을지, ② 1.1에서 기본을 상속으로 바꾸겠다고 약속할지(O5 2단계, 지문을 바꿔 구 클라이언트를 끊을지 포함).

**기존 소비자 영향.** 추천안(3안 + 현행 경고): 없음. 경고가 늘 뿐 `ok`와 종료 코드는 그대로다. 단 경고를 실패로 처리하는 CI가 있으면 `where` 없는 count 8개와 `input` where count의 새 경고가 보인다(저장소 안에서 새로 경고가 나는 정의는 0개: 아래 D-A2).

## D-A2 `where`에 `input`이 있는 count (F1, A-01)

**문제.** `callerFilter read`는 `where`가 원본 정책을 함의해야 하지만(`DEF.CALLER_FILTER_POLICY_REQUIRED`), `input`이라는 다른 통로에는 같은 검사가 없었다. 위 매트릭스의 `probeCount`·`kindCount`가 그 예다.

**구현한 것.** `where`가 `input`을 쓰는 count가 원본 정책을 함의하지 않고, 원본 정책이 `true`가 아니고, sourceAccess guard가 `where`가 쓰는 `input`을 모두 받지 않으면 경고 `DEF.ADVISORY_PUBLIC_COUNT_INPUT`을 낸다(`sema.rs`, 커밋 `b292a880`). 정의는 통과하고 응답 값과 facts는 불변이다. 저장소 정의 71개에서 새로 나온 경고는 0개다(`recruitment.aip`의 `approvedCount`는 guard가 같은 `input.clubId`를 받아 면제).

**선택지.** (a) `where`에 `input`이 있는 count는 `callerFilter read`와 같은 규칙으로 정책 함의를 요구한다. 이전 시도에서 `approvedCount`가 거부돼 `cardinality_capacity` 시험이 실패했다. (b) (a)에서 guard가 같은 `input`을 받으면 면제한다. 구조로만 판정하므로 guard가 실제로 제한하는지는 모른다(예: `access g(a, x) = true`). (c) 경고만(구현됨).

**추천.** (c) 유지. (a)·(b)는 새 거부라 외부 정의를 깨고, 어떤 `where`가 안전한지의 기준이 D-A1(count 기본 의미)과 같이 정해져야 한다. 구현한 면제 조건(guard가 같은 input을 받음)은 (b)와 같은 구조 판정이라 경고 단계에서만 쓴다.

**코드 변경 범위.** (a)·(b)는 `sema.rs aggregate`의 같은 위치에서 `d(...)` 호출 한 곳과 시험 변경. **기존 소비자 영향.** (a): `approvedCount`류 정의가 거부된다. (b): 구조상 맞는 정의는 통과.

## D-A3 K1: `unique`에 가려진 필드

**문제.** `unique code`에서 `code`가 `field read`로 가려진 필드면, 권한 있는 호출자가 create로 값을 한 값씩 시도해 `ALREADY_EXISTS`로 가려진 값의 존재를 확인한다.

**재현 근거.** `product/tests/policy-residual-paths.test.mjs`의 `K1-K3 residuals`(PG17): code=4242는 `WIRE.ALREADY_EXISTS`, code=1은 성공. 권한 없는 호출자는 이 경로로 못 가고 `WIRE.FORBIDDEN`을 받는다(같은 파일의 첫 두 시험과 `C9`). 다른 테넌트의 행과 충돌해도 같은 코드다(`spec/definition-syntax.md` 20번째 줄이 이미 계약으로 적는다).

**선택지.** (a) `unique`에 `field read` 정책이 있는 필드가 있으면 정의 오류로 거부. 저장소의 정의 68개 가운데 해당 없음으로 기록돼 있으나(`policy.md`) 외부 정의는 깨질 수 있다. (b) 현행 유지와 문서화. (c) 경고 추가(조언만).

**추천.** (c)를 다음 단계로, 확정은 소유자. 권한 없는 호출자 노출은 이미 닫혔으므로 남은 것은 권한 있는 호출자가 보는 존재 여부다. `ALREADY_EXISTS`는 권한 있는 호출자의 진단이라 응답 코드를 합치는 안은 권하지 않는다.

**코드 변경 범위.** (a) `sema.rs:1913` unique 검사에 한 항, (c) `policy_advice.rs`에 한 항. **기존 소비자 영향.** (a) 외부 정의 거부 가능, (c) 경고만.

## D-A4 K2: 숨은 범위 필드의 리터럴 증감

**문제.** 가려진 `Int(0..100)`에 `risk = risk + 1`을 반복하면 `BAD_VALUE`가 나는 횟수로 값이 드러난다(63이면 37번). 같은 시험에서 확인했다.

**선택지.** (a) 가려진 필드의 `f = f ± 리터럴`을 range가 있으면 거부. (b) 호출 횟수 비용이 충분히 크면 허용하고 문서화. **추천.** (b) 유지. 값이 파괴되고(증가가 남는다) 호출 횟수가 범위 크기에 비례해 비용이 든다. (a)는 숨은 카운터 패턴 정의를 깬다. **코드 변경 범위.** (a) `sema.rs`의 전이 `to` 검사. **기존 소비자 영향.** (a) 숨은 카운터가 있는 정의 거부.

## D-A5 K3: 숨은 자식 때문에 부모 삭제가 실패

**문제.** 호출자가 못 읽는 자식 행이 부모를 참조하면 부모 삭제는 `WIRE.REFERENCE_CONFLICT`, 자식이 없으면 성공해 숨은 행의 존재가 드러난다.

**재현 근거.** 위 시험: 숨은 자식이 있는 부모 1은 `WIRE.REFERENCE_CONFLICT`, 없는 부모 2는 성공. 권한 없는 호출자의 같은 시도는 `WIRE.FORBIDDEN`이다(`a transition the caller may not make...` 시험).

**선택지.** (a) 현행 유지, 문서에 FK 거부도 숨은 행의 존재를 알린다고 쓴다. (b) 거부 사유를 일반 코드로 합친다(응답 코드 변경, 권한 있는 호출자의 진단이 사라짐). **추천.** (a). 진단 훼손 없이 노출을 닫는 정의 단계 방법이 없다(자식의 가시성은 호출자별이다). **코드 변경 범위.** (b) `spikes/spike-v3-write/src/lib.rs:862`. **기존 소비자 영향.** (b) `REFERENCE_CONFLICT`에 의존하는 클라이언트(SDK 오류 처리 포함)가 깨진다.

## D-A6 K4 잔여: allow가 `default database` 필드를 읽는 외부 create

**지금.** 외부 저장소 create의 `allow`가 생략한 `default database` 필드를 읽으면 INSERT 뒤에야 전체를 판정한다. 이번에 그 필드를 읽지 않는 최상위 AND 항은 INSERT 전에 판정하도록 고쳤다(`create_row`, 커밋 `74ba5b6b`). `allow member = actor and status = "open"`에서 권한 없는 호출자(`member = actor` 거짓)는 숨은 값이 있든 없든 `FORBIDDEN`이다. 확장 `ctx.data.create`도 같은 함수를 부르므로 같다(PG17에서 처음 실행해 확인).

**남은 노출.** `member = actor`는 채우고 `status = "open"`만 거짓인 호출자(DB 기본값이 `closed`)는 INSERT까지 가서 유일 충돌이 `FORBIDDEN`보다 먼저 나온다. 시험 `K4 residual`이 이 줄을 고정한다. 이 호출자는 allow의 다른 항을 만족한 호출자라 노출 대상이 좁다.

**선택지.** (a) 외부 resource의 `expose create allow`가 create 필드에 없는 `default database` 필드를 읽으면 정의 오류로 거부(남은 노출까지 닫음, 응답 코드 불변). 새 거부라 기존 정의 호환 영향이 있어 FaceReview·uuid 입양 정의 조사가 필요하다. (b) 그 경우 유일 제약 위반을 `FORBIDDEN`으로 합친다(권한 있는 호출자의 `ALREADY_EXISTS` 진단이 사라짐). (c) 현행 유지와 문서화.

**저장소 영향(측정).** `default database` 필드가 있는 정의 5개(`external-effects`, `facereview-comments`, `facereview-mysql`, `mysql-worker-action`, `mysql-worker-e2e`)에서 create `allow`가 create 필드에 없는 기본값 필드를 읽는 resource를 임시 시험으로 찾았다(실행 `cargo test --test zz_tmp_k4`, 결과는 이 문서에만 옮김). 해당은 1개다: `product/tests/fixtures/facereview-comments/comments.aip`의 `Comment`(MySQL fixture, 1.0 범위 밖). 나머지 4개는 0이다. (a)를 택하면 그 fixture 하나가 새로 거부된다. 외부 저장소 PostgreSQL 정의 중에는 해당이 없다.

**추천.** (a). 새 거부가 저장소 안에서는 MySQL fixture 하나뿐이다. 저장소 밖 정의는 확인할 수 없으므로 CHANGELOG에 호환 변경으로 적고 소유자가 확정한다. (c)는 지금 상태(좁혀진 뒤)다.

**코드 변경 범위.** (a) `sema.rs:293-300` 근처(`default database` 처리)와 expose create allow 검사에 한 항, 시험 변경. **기존 소비자 영향.** (a) 해당 정의 거부.

## D-A7 테넌트 조언의 구조적 한계

`track-a.md`의 표가 근거다. 확실히 판단되는 두 가지(부정으로 쓴 테넌트 조건의 오탐, 단독 count의 다른 테넌트 집계 미탐)는 구현했다. 아래는 구현하지 않은 것과 이유다.

- 관계 경로 테넌트(`rows read when owner.org = actor.org`): 행 자신의 `org`가 아니라 소유자의 `org`로 묶는데도 지금은 테넌트 조건으로 읽는다(미탐 가능). 자기 필드가 아닌 경로를 테넌트로 인정하는 것은 `policy_advice.rs`의 의도된 규칙(`this.a.b.field`)이다. 바꾸면 소유자 경유 테넌트를 쓰는 정의에 경고가 늘어 오탐이 늘 수 있다. 추천: 현행 유지.
- `exists` 정책: 판정하지 않는다(멤버십 표는 구조 비교로 따라갈 수 없다).
- 테넌트 근거가 없는 정의(어떤 정책도 같은 이름·대상의 참조를 actor와 묶지 않음): 경고하지 않는다. 근거 없이 추측해 경고하지 않는 것이 이번 지시의 불변 조건이다.
- OR 가지 중 하나가 `actor.admin`처럼 행을 보지 않는 조건이면 지금은 "테넌트를 지키지 않는 가지"로 경고한다. 플랫폼 관리자를 뜻하면 의도이고 테넌트 관리자를 뜻하면 결함이라 정의만으로 구분할 수 없다. 추천: 현행 유지(경고는 확인을 요청하는 것이다).
