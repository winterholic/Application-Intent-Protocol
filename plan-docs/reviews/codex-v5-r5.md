# 프론트 SDK 캐시·r4b 반영 정확성 검토 (r5)

기준일: 2026-10-04. 범위: V5-3 캐시, r4b F01~F09 및 지정 기준 문서. 원본 코드·결과 문서는 변경하지 않는다.

판정: **P1 2건, P2 3건**. 다음 단계 전 핵심 수정은 늦은 응답의 호출자 전달 처리(R5-01)와 관계 내부 동적 select 추론(R5-04)이다. r4b 원래 재현은 F01-05·F08-09 해결, F06 문서 정정, F07 의도적 한계 기록으로 확인했다. 새 정상 요청 거부 회귀는 시험한 범위에서 없음.

검증 기준: 창시자 지시 DD-11·DD-13, RK-09, D의 지원자 승인과 캐시. 문서의 확정 계약과 미구현 한계를 구별한다. 발견은 확인 즉시 아래에 추가한다.

실행 방식: 기존 테스트도 생성 파일을 쓰므로 임시 복사본에서 실행한다. V2 기본 schema와 모든 테스트 schema를 r5 전용 이름으로 치환한다. DB는 이 검토가 만든 schema만 사용하고 제거한다. git·.env·비밀 저장소는 읽거나 실행하지 않는다.

## R5-01 · P1 · Q1/Q2: 무효화된 늦은 응답은 저장만 막고 호출자에게 그대로 반환한다

문제: 쓰기 뒤 새 조회가 끝났어도 이전 조회 Promise는 옛 행을 성공 값으로 반환한다. 계정 전환 뒤에도 이전 actor의 행을 반환한다. 저장된 캐시는 안전하지만 화면이 반환 순서대로 갱신하면 최신 값 또는 새 계정 화면을 옛 값으로 덮을 수 있다. 다음 전송·화면 연동 단계 전에 반환 계약을 정하고 처리해야 한다.

근거: `spikes/spike-v5-sdk/sdk/cache.ts:25-34`는 epoch·actor 불일치를 검사해 저장을 막지만 `:34`는 항상 `res.rows`를 반환한다. `sdk/cache.test.ts:52-66`은 `stored=false`와 size만 검사한다. `V5-sdk-results.md:121`의 “그 응답은 캐시에 저장하지 않음”이라는 좁은 주장은 맞다. 화면 동기화까지 보장하는 근거는 아니다. 기준은 창시자 DD-11(`founder-integrated-directive-2026-10-03.md:660-666`)과 RK-09(`E-technical-risks.md:20`)다.

실행: 원본 cache.ts를 복사한 임시 `cache-cases.ts`를 `node cache-cases.ts`로 실행. 이전 읽기는 gate에 대기 → `onWrite(ok, [ClubMember])` → 새 읽기 완료 → gate 해제. 계정 사례는 manager → member 전환으로 같은 순서를 실행했다.

```text
late-after-write fresh={internalNote:null,stored:true}; late={internalNote:"old",stored:false}; hit={internalNote:null,cached:true}
late-after-actor fresh={internalNote:null,stored:true}; late={internalNote:"manager secret",stored:false}; size=1
```

캐시 hit가 낡아지는 사례는 이 순서에서 없음. 문제는 호출자에게 도달하는 늦은 결과다. 재조회·취소 오류·별도 stale 결과 중 하나로 구분하고, actor 전환 이전 응답은 현 actor 결과로 소비할 수 없게 해야 한다. 실제 UI 덮어쓰기는 확인 못함: 이 spike에 화면 소비자가 없다.

## R5-02 · P2 · Q2/Q4: 미확정 결과의 한 번 비우기는 아직 진행 중인 커밋을 포괄하지 못한다

문제: `unknown` 뒤 즉시 재조회하면 서버 커밋 전 값이 다시 저장될 수 있다. 이후 커밋 통보가 없으면 그 값이 계속 hit된다. 다음 응답 유실 복구 단계에서 미확정 작업의 수명과 캐시 재개 조건을 정해야 한다.

근거: `sdk/cache.ts:38-42`는 entries를 지우고 globalEpoch를 한 번 증가시키며 미확정 상태를 유지하지 않는다. 다음 `read`는 즉시 저장 가능하다(`:27-33`). `V5-sdk-results.md:120`의 “전부 비움”은 실행과 맞지만, 이를 미확정 커밋에 대한 안전한 재조회 보장으로 넓히면 근거보다 강하다. 문서는 전송·유실 복구가 미구현이라고 명시한다(`:125`, `:129`), 따라서 기존에 약속한 전송 기능의 회귀로 판정하지 않는다.

실행: `node cache-cases.ts`. DB 대역이 old일 때 `onWrite({status:"unknown"})` → old 재조회·저장 → 대역 커밋(committed) → 동일 조회.

```text
unknown-later-commit beforeCommit={rows:["old"],stored:true}; afterCommit={rows:["old"],cached:true}; database="committed"
```

확인 못함: 실제 COMMIT_UNKNOWN/응답 유실이 이 캐시에 통보되는 시점과 서버 상태 조회 연결은 미구현이다. V3에는 COMMIT을 보낸 뒤 기한 초과하면 COMMIT_UNKNOWN을 반환하는 분기 자체가 있다(`spikes/spike-v3-write/src/lib.rs:78-84`, `:90-101`). 그 실제 COMMIT 대기와 SDK를 연결한 시험은 이번에 실행하지 않았다. 이 실행은 아직 커밋 여부가 정해지지 않은 작업이 남는 경우의 경계 모사다. 상태 조회·멱등 재시도 등으로 종결을 확인한 뒤 재조회할지, 그동안 캐시를 우회할지 정한다. 이미 커밋/롤백이 종결된 후 단지 응답을 못 받은 경우에는 비운 뒤 재조회가 최신 값으로 저장되는 정상 대조를 추가 실행했다(Q2 표).

## R5-03 · P2 · Q2: 반환 행의 가변 참조가 캐시에 그대로 남는다

문제: 화면이 받은 행 객체를 수정하면 다음 조회가 서버 값 대신 화면 편집 값을 cache hit로 반환한다. 캐시 API의 소유권·불변성 규칙이 없다. 화면 편집과 결합하기 전에 반환 값의 소유권을 명시할 것을 권한다.

근거: `sdk/cache.ts:26`은 entry의 rows를 직접 반환하고 `:33-34`는 fetch의 rows를 entry와 호출자가 공유한다. 반환 타입도 `unknown[]`이며 readonly가 아니다. 서버와 같은 결과를 보관한다는 의미에서 생기는 차이다. 불변 객체만 쓰도록 정한 문서 규칙은 지정 문서·SDK에서 찾지 못했다.

실행: `node cache-cases.ts`. 서버 `{id:1,title:"server"}`를 읽고 `result.rows[0].title="screen edit"` 한 뒤 동일 조회.

```text
mutable-rows {"rows":[{"id":1,"title":"screen edit"}],"cached":true}
```

복제·freeze·불변 소비 계약 중 비용과 사용성에 맞는 방식을 정한다. 별도의 낙관 갱신 API를 구현했다는 뜻은 아니다.

## 기존 테스트 실행 중 환경 제한

임시 복사본에서 지정 명령 `PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture`를 실행했다(빌드 산출물은 원본 spike의 `target/` 사용). V2는 5 Rust 테스트, V5는 4 Rust 테스트가 통과했다. V4 전체 실행은 v4_1의 MacNetDeny worker가 Node·Python 모두 `WORKER_FAILED`여서 종료 코드 101이었다. 그 전에 정상 ctx·기한·F08·F09 단언은 실패 목록에 없었고, F05 잠금 재현은 Node 102ms·Python 103ms에 `DEADLINE_EXCEEDED`였다. 격리 성공은 확인 못함: 이 실행 환경의 중첩 sandbox worker 기동 실패. 이후 임시 복사본에서 격리 블록만 제외해 검토 범위의 테스트를 실행한다. 이 실패를 F05 수정 실패나 전체 V4 통과로 기록하지 않는다.

## R5-04 · P1 · Q3/Q4: F01 수정은 루트 select에만 적용되어 관계 안 동적 select의 필드를 과대 보장한다

문제: `club.select: [cond ? "name" : "logo"]`는 name·logo 중 하나만 선택하지만, SDK는 둘 다 있는 관계 객체로 추론한다. 관계 select를 길이 미정 배열로 만들어도 같은 문제가 남는다. r4b F01의 원래 루트 재현은 해결됐으나 같은 정상 사용을 관계 안에 적용하면 불건전하다. 계약 생성·SDK 확대 전에 수정해야 한다.

근거: `sdk/aip.ts:37-38`은 관계의 `select` 전체를 원소 union X로 평탄화하고 모든 Q를 필수 키로 만든다. 루트에 추가된 tuple/배열 분기(`:43-48`)는 관계 내부에는 적용되지 않는다. `V5-sdk-results.md:43`의 관계 포함 “정확히 일치”와 `:96`의 동적 select 반영 설명은 이 범위까지 일반화하면 강하다.

실행: 임시 SDK의 `r5-types.ts`를 로컬 `spikes/spike-0-ts/node_modules/.bin/tsc --noEmit --strict --target esnext --module nodenext --moduleResolution nodenext --allowImportingTsExtensions --skipLibCheck sdk/r5-types.ts`로 검사(종료 0). 타입 단언·any 없이 두 경우 모두 다음 대입이 동시에 허용됐다.

```ts
const a = await aip.read({read:"Recruitment", select:[{club:{select:[cond ? "name" : "logo"]}}]});
if (a[0].club) { const name: string = a[0].club.name; const logo: string | null = a[0].club.logo; }
```

같은 파일에서 루트 동적 배열의 title 필수 대입은 `@ts-expect-error`로 실제 오류를 확인했다. 관계 내부도 선택 항목별 tuple 교집합/union 또는 길이 미정 배열의 optional 키로 재귀 처리해야 한다. 실제 DB 관계 분기 응답 대조는 뒤의 추가 실행 절에 기록한다.

## 기각한 후보: 관계 내부 빈 select

SDK는 `select:[{club:{select:[]}}]`를 허용한다. 실제 V2 대조 결과 서버도 `rows:[{club:{}},{club:{}}]`를 반환했다. 루트의 빈 select 금지와 관계 내부 빈 select 허용은 현재 계약의 차이이며, SDK/서버 불일치 발견은 **없음**. `src/plan.rs:198-200`에는 객체 형식·허용 키·배열 검사가 있고 빈 배열 금지는 없다. 정적 읽기에서 빈 배열 거부라고 잘못 판단했던 후보를 DB 실행으로 기각했다.

## R5-05 · P2 · Q3/Q4: 숫자 Id의 표현 범위를 정하지 않아 큰 Id 왕복이 깨진다

문제: PostgreSQL bigint Id는 정상 반환되지만 JS의 안전 정수 범위를 넘으면 값이 달라진다. F04가 응답 number를 요청에 그대로 쓸 수 있게 고쳤어도 같은 행을 다시 찾지 못한다. 큰 Id를 지원할지, 안전 정수 범위로 제한할지 전송 codec 전에 정해야 한다.

근거: `spikes/spike-v5-sdk/src/lib.rs:51`의 응답 `Id = number`, `:82`의 입력 Id number/숫자 문자열; `spikes/spike-v5-sdk/sdk/aip.ts:66`은 transport 결과를 단언하며 codec이 없다. V2는 bigint로 저장하고 응답 JSON의 Id를 숫자로 생성한다(`spikes/spike-v2-read/src/sqlgen.rs:314-318`, `spikes/spike-v2-read/src/plan.rs:317-319`). `src/plan.rs:87-90`은 정수 number·18자리 이하 숫자 문자열을 받는다. 작은 Id F04 재현의 해결과 큰 Id 양방향 보장은 별개다. 이는 F04 수정이 처음 만든 정밀도 손실이 아니라, 그 수정 후에도 남은 양방향 표현 공백이다.

실행: V1이 허용한 `id.eq` 계약과 임시 PG 행 id `9007199254740993`. V2 문자열 필터 → Rust 원본 JSON `[{"id":9007199254740993}]`. 이를 Node `JSON.parse`로 받아 id를 그대로 필터에 넣고, 실제 V2 계획·DB 실행에 다시 전달했다.

```text
F04-large: rows=[{"id":9007199254740993}]
F04-large-node-roundtrip request.filter[0].value=9007199254740992; rows=[]
```

`100`과 `"100"`은 둘 다 `[{"id":100}]`였다. 지원 Id 범위·문자열 응답/입력 codec을 한 계약으로 정하고, number를 쓰면 저장·요청·응답의 안전 정수 경계를 검증할 것을 권한다. SDK가 모든 Int/bigint를 정확히 표현한다는 확대 주장은 하지 않는다.

### R5-04 실제 관계 응답 대조

임시 `r5_cases` schema에서 같은 관계의 각 분기를 V2로 실행했다.

```text
select club.name -> [{"club":{"name":"A club"}},{"club":{"name":"large id club"}}]
select club.logo -> [{"club":{"logo":"logo.png"}},{"club":{"logo":null}}]
```

name 분기에 logo가, logo 분기에 name이 없다. TS가 둘을 동시에 필수로 보장한 것과 실제 응답이 불일치한다.

## r4b F01~F09 반영 판정

“해결”은 원래 재현 입력의 문제를 뜻한다. 주변 사용까지 일반화하지 않는다. 다음 tsc·PG·Node/Python 실행은 모두 이번 검토의 임시 복사본에서 수행했다.

| r4b | 원래 입력의 판정 | 이번 실행 입력·결과 / 근거 | 정상 요청의 추가 거부·다른 의미 |
|---|---|---|---|
| F01 | 해결 | `[cond ? "title" : "views"]` 후 title·views 필수 대입은 각각 TS2339. 서버 두 분기는 title만/ views만 반환. `sdk/aip.ts:43-48`; `sdk/type-tests.ts:28-30` | 고정 tuple 정상 타입 검사는 통과. 루트 길이 미정 배열은 optional로 보수화. 관계 안 동적 선택은 R5-04 |
| F02 | 해결 | 원래 `select:[]`, `{club:...,school:...}`는 TS2322, 같은 JSON은 서버 BAD_REQUEST. `sdk/aip.ts:50-53`; `src/plan.rs:124-126`, `:192`, `:244` | 유효한 한 키 관계는 통과. 관계 안 빈 배열도 SDK·서버 모두 허용하므로 불일치 없음 |
| F03 | 해결 | V1 원문 select에서 periodEnd 제거, filter periodEnd.gte/lte 유지. 생성 SDK tsc 통과, V2 입력 `select:["id"], periodEnd.gte="2026-10-09T00:00:00Z"` → id 100(및 추가 경계 행). periodEnd select는 여전히 타입 오류. `src/lib.rs:74-83`, `sdk/aip.ts:16-20` | 필터 허용을 select 허용으로 확대한 사례 없음. filterFields 원소는 V1이 정렬한 filter 집합에서 생성되므로 인접 dedup이 현재 경로에서 유효(`spike-v1-fixture/src/sema.rs:1083`, `:1156`) |
| F04 | 해결(작은 Id 원래 입력) | V1에 id.eq 추가, 100과 "100" 모두 SDK 통과·V2 결과 `[{"id":100}]`. `spike-v2-read/src/plan.rs:86-90`, `spike-v5-sdk/src/lib.rs:82` | 숫자 입력을 추가했고 원래 숫자 문자열을 거부하지 않았다. 큰 Id 정밀도는 R5-05. SDK `${number}`는 "100.0"·"1e2"도 tsc 통과하나 서버는 숫자 문자만 받으므로 "100.0"은 BAD_VALUE. 문자열 문법은 아직 양쪽 완전히 같지 않다 |
| F05 | 해결 | 원문 deadline 100ms, 다른 DB 연결이 Apply에 ACCESS EXCLUSIVE 450ms. 기존 시험 Node 102ms·Python 103ms에 DEADLINE_EXCEEDED; 격리 제외 재실행 102/102ms. 추가 시험 같은 worker·DB에서 다음 정상 actor3/club11 요청 → 둘 다 `{approvedApplicants:2}`. `spike-v4-worker/src/lib.rs:200-204`, `:218-220`, `:261-264` | 기존 정상 ctx 시나리오 통과. DB/worker를 새로 만들지 않아도 기한 취소 뒤 정상 요청 동작. stdin 송신은 아직 같은 timeout으로 둘러싸지 않음(`:183`, `:210`); backpressure의 실제 재현은 확인 못함: 이번 범위는 정상 ctx/DB 대기 경로 |
| F06 | 문서 정정 반영 | 첫 3회 실패 후 회복 가능 공급자 → attempts=3, dead=true, delivered=false, calls=3, effects=0. send 뒤 4번 rollback → attempts=0, dead=false, calls=4. `src/outbox.rs:57`, `:72-87`; `V4-official-extension-results.md:100`, `:106` | 유한 재시도 동작은 그대로이고 문서가 정확히 좁아졌다. 코드 파일 상단 `src/outbox.rs:2`에는 옛 “최소 한 번 전달” 표현이 남음. 이는 런타임 신규 실패가 아니라 문서/주석 정합성 정리 대상 |
| F07 | 의도적 미수정·한계 기록 | 현재 SQL lexical 시험: ClubMember 정확 식별; literal→Member 오탐; other schema→Member 오탐; 인용 schema/table→빈 deps. `spike-v2-read/src/plan.rs:33-46`, `V5-sdk-results.md:100` | 기존 V5 8 resource 대조에서 결과를 바꾸는 resource 누락 없음. 정상 SQL의 정밀도·건전성 전체를 증명했다는 뜻은 아니다 |
| F08 | 해결 | nullable note 키를 생략하는 원래 stats는 Node/Python OUTPUT_INVALID. 추가 원문 계약 + `note:null` 정상 출력은 둘 다 `OK {approvedApplicants:1,note:null}`. `spike-v4-worker/src/lib.rs:143-150` | nullable을 optional로 쓰던 입력·출력은 이제 거부. 문서의 정확한 키 계약(`V4-official-extension-results.md:77`)과 맞는 거부다. 명시적 null 정상 출력의 신규 거부 없음 |
| F09 | 해결 | 원문 output ClubRole + ADMIN 정상 값은 둘 다 OK; INVALID는 OUTPUT_INVALID. 정상 Time("2026-10-04T00:00:00Z"), Url("https://example.com/a"), Club 참조("10") 출력도 둘 다 OK. V1 원문의 Club 타입은 facts에서 Ref<Club>로 내려감. `spike-v4-worker/src/lib.rs:115-129` | 정상 enum·Time·Url·Ref의 신규 거부 없음(시험 값 범위). Time은 길이·10번 위치 T, Url은 문자열 타입만 검사하므로 날짜·URL 유효성까지 보장한 검증으로 읽지 않는다 |

새롭게 정상 고정 tuple·단일 관계·filter 전용 필드·작은 Id·명시적 nullable 출력·enum을 거부하는 회귀는 **없음**(위 실행 범위). r4b F10은 사용자 지정 F01~F09 범위 밖이므로 재검토하지 않는다.

## Q1: 쓰기 태그·동시 쓰기·후속 효과의 확인 범위

- **현재 전이 facts의 태그 누락: 없음**(전이 원본 + create/update 효과). 기존 V5-3은 Apply.approve → `[Apply,ClubMember]`, MemberAlarm.read → `[MemberAlarm]`. 추가 V1 원문은 approve에 `create ClubMember`, `update Recruitment ... views=6`, `notify member`를 함께 넣고 의미 검사를 통과시켰다. `write_tags` 결과는 `[Apply,ClubMember,Recruitment]`. `src/lib.rs:93-104`는 효과 목록 전체를 순회하고 sort/dedup한다. 실제 V3 효과 실행기의 create/update/notify 분기는 `spikes/spike-v3-write/src/lib.rs:312-363`과 대응한다. 이번 추가 실행은 facts·태그 생성 검증이며 V3 승인 커밋과 SDK의 연결 시험은 아니다.
- **notify → outbox**: notify가 쓰는 것은 현재 `aip_outbox`다(`spike-v3-write/src/lib.rs:355-360`). 현재 V4 공급자 대역은 메모리 effects만 추가한다(`spike-v4-worker/src/outbox.rs:19-28`); MemberAlarm을 DB에 생성하지 않는다. 따라서 “현 공급자 후속 MemberAlarm 쓰기가 태그에서 빠졌다”는 도달 가능한 현 코드 결함은 **없음**. 확인 못함: 실제 별도 알림 트랜잭션·공식 worker 공급자와 SDK 연결은 미구현. 그런 후속 쓰기는 승인 응답 태그만으로 포괄할 수 없고 **후속 커밋 자체의 태그 통보 또는 안전한 재조회**가 필요하다. 기준은 `D-development-experience.md:55-57`이다.
- **여러 쓰기 동시 완료**: 두 비동기 쓰기 Promise의 완료 순서를 Recruitment→ClubMember, ClubMember→Recruitment로 각각 gate 제어했다. 각 쓰기 직후 `onWrite(ok, tags)`, 중간 read rows `[1]`, 두 번째 이후 read `[2]`, 둘 다 miss·stored=true; 다음 read는 hit. 여러 알림이 epoch를 지우거나 되돌려 무효화를 놓치는 사례는 **없음**(성공 커밋마다 통보된다는 가정). 여러 쓰기 사이 대기 중인 이전 read도 stored=false다. 실제 복수 전송 응답의 순서·유실은 확인 못함: transport 없음.
- **읽기/쓰기 교차**: 응답이 쓰기 전에 저장되면 `onWrite`가 삭제하고, 읽기 시작 뒤 관련 쓰기가 통보되면 해당 응답은 저장되지 않는다. 무관 resource 통보는 hit 유지. 서로 다른 관련 쓰기가 잇달아 와도 늦은 read 저장 차단을 유지했다. 다만 늦은 결과의 호출자 반환은 R5-01이다.
- **쓰기가 반영돼도 이후 같은 값이 hit되는 경계**: 미확정 통보 → 커밋 전 read → 나중 커밋·추가 통보 없음은 R5-02. 직접 create/W0/W1·여러 연산의 최종 changed resource 집합을 `write_tags(Resource.transition)` 하나로 생성하는 경로는 현재 없으므로 연결 정확성은 확인 못함: 단일 전이 이름 API와 가짜 onWrite 호출만 있다. 단일 전이 태그를 다른 쓰기 형태 전체의 완성된 변경 집합으로 재사용하지 않는다.

## Q2: 캐시 키·actor·미확정·과다 무효화

| 항목 | 판정과 실제 확인 | 근거 |
|---|---|---|
| 키 | actor 문자열과 `JSON.stringify(query)`다. 동일 actor·동일 JSON은 hit. `{read,select}`와 `{select,read}`는 의미가 같아도 서로 miss여서 fetch 2회·entry 2개가 된다. 관측한 차이는 중복 캐시/재조회이며 서로 다른 정상 결과를 잘못 합치는 key collision은 없음(plain JSON 정상 요청 시험 범위) | `sdk/cache.ts:24-26`; 임시 `cache-cases.ts`의 key-overinvalidate |
| actor 전환·로그아웃 | actor가 바뀌면 entries 전부 삭제, globalEpoch 증가. actor1→actor2 DB 응답을 각각 저장했으며 두 번째는 miss. null 전환 시 size=0. 같은 actor를 다시 설정하면 기존 hit 유지. 여러 actor의 캐시를 동시에 유지하는 구조는 아니다 | `sdk/cache.ts:14-21`, `cache.test.ts:37-50` |
| actor 전환 중 in-flight | 저장은 차단하므로 이전 actor entry가 되살아나지 않는다. 이전 actor rows를 호출자에게 반환하는 것은 R5-01 | `sdk/cache.ts:27-34` |
| 결과 미확정 | 기존 entry를 전부 비우고 이미 진행 중인 읽기의 저장도 막는다. 이미 서버 커밋이 종결된 후 unknown을 받은 정상 대조는 다음 read에서 committed를 반환·저장했다. 아직 커밋이 진행 중이면 R5-02 | `sdk/cache.ts:38-42`; `cache-more.ts` unknown-already-settled |
| resource 과다 무효화 | deps에 Member가 있으면 actor와 무관한 다른 회원 행의 학교 변경도 이 actor의 모든 관련 쿼리를 삭제한다. 실제 PG에서 member2.school을 1→NULL로 바꿔도 actor1의 모집 결과는 동일했고, deps에는 Member가 있다. 정밀 행 영향 분석은 없다 | `V5-sdk-results.md:86`, `sdk/cache.ts:44-45`; 추가 PG 실행 |
| 서로 다른 resource | 모집 목록 deps에 MemberAlarm이 없어서 알림 읽음은 이 목록을 유지한다. 같은 알림 쓰기는 알림 목록만 miss. 단순히 모든 쓰기마다 전체 clear하는 구현은 아니다 | `cache.test.ts:22-35`; 기존 Rust facts `listDeps=[Club,ClubMember,Member,Recruitment]`, `alarmDeps=[MemberAlarm]` |
| 선언 효과의 상위집합 | write_tags는 실제 바뀐 행 수와 관계없이 원본+모든 create/update 대상이다. 0행·repeat unchanged에서 이를 ok 태그로 통보하면 불필요한 재조회가 된다. 현재 태그 함수에는 실행 결과 인자가 없다 | `src/lib.rs:93-104`; 실제 no-op writer와 SDK 연결의 양은 확인 못함: 연결 미구현 |
| 무효화 비용·보관 | onWrite는 모든 entry의 deps를 순회한다. 메모리에 unique query entry를 유지하며 별도 용량 한도·eviction·TTL은 없다. 지금 측정한 것은 정확성과 fetch 횟수이며 생산 규모의 성능은 확인 못함: 부하 측정 미실행 | `sdk/cache.ts:7`, `:24-33`, `:44-45` |
| tenant/session/계약 버전 | 별도 key 구성요소가 없다. 현재 실행은 actor와 단일 계약·schema다. 같은 actor 식별자를 유지하는 tenant/session/계약 변경은 actor 전환으로 자동 검출된다고 주장할 수 없다 | RK-09 `E-technical-risks.md:20`; 실제 다중 tenant 동작은 확인 못함: spike에 tenant 입력 없음 |

과다 무효화는 **resource가 겹치는 현재 actor의 모든 저장 entry**까지다. actor 전환·unknown은 resource와 무관하게 전체 삭제한다. 중간 read가 실제로 새 값이어도 시작 후 관련 쓰기 통보가 한 번 있으면 저장을 포기한다. 보수적 안전 방향이지만 재조회 비용은 커질 수 있다. 몇 % 낭비인지는 확인 못함: 요청 분포·entry 수·행 영향 분포를 측정하지 않았다.

## Q4: 문서 주장과 실행 근거의 경계

1. `V5-sdk-results.md:43`의 관계 포함 결과 타입 “정확히 일치”는 고정 select 표본에서는 맞지만 관계 안 동적 select까지는 아니다(R5-04). 같은 문서 `:96`의 union/배열 보수화도 루트에만 적용된다.
2. `V5-sdk-results.md:106-121`의 캐시 실험은 **실제 facts에서 계획·태그를 생성하고 Node의 숫자 호출 횟수 대역으로 무효화를 시험**한 것이다(`tests/v5_3_cache.rs:30-41`, `sdk/cache.test.ts:11-19`). “승인 뒤 목록 무효화”를 실제 V3 승인 → DB 커밋 → 응답 → SDK → 최신 DB 읽기의 끝까지 시험한 결과로 읽으면 강하다. 본 검토도 태그/캐시 분리 및 실제 V2 응답 대조까지 했고 그 전체 연결은 확인 못함: transport와 쓰기 연결 없음.
3. `V5-sdk-results.md:118-121`의 actor 분리·전체 clear·늦은 응답 저장 차단은 명시된 범위에서 맞다. **현재 계정 화면으로 옛 응답이 도달하지 않는 보장** 또는 **진행 중인 미확정 커밋이 종결돼도 최신 캐시 유지**로 넓히지 않는다(R5-01·02).
4. `V5-sdk-results.md:88`의 “전체 무효화 미구현”은 V5-2 시점 기록이고, §6에는 unknown clear 구현이 있다. 현 상태는 “unknown 통보를 받으면 clear는 구현, 응답 유실을 unknown으로 판별·복구하는 전송 연결은 미구현”으로 구분해 읽어야 한다. 이는 새 런타임 결함이 아니라 문서 상태 표현의 정리 대상이다.
5. `V4-official-extension-results.md:114`의 F05 DB 대기 해결, `:116`의 키 존재, `:117`의 enum 출력 정상은 이번 실행과 맞다. **모든 IO를 포함한 강제 종료 기한**이나 **Time/Url의 도메인 값 유효성**까지 보장한 시험은 아니다. 확인 못함: stdin backpressure·실제 프로세스 취소 및 잘못된 날짜/URL의 전체 경계 검증은 이번에 실행하지 않았다.
6. outbox의 좁아진 보장(`V4-official-extension-results.md:100-106`)은 원래 재현과 맞다. 코드 상단 옛 보장(`spike-v4-worker/src/outbox.rs:2`)은 정리 대상이다. 공급자가 실제 DB 알림을 생성하고 그 변경이 SDK에 도달하는 보장은 없다.
7. 시간 의존 만료는 문서가 정확히 **미구현**으로 적었다(`V5-sdk-results.md:125`). 추가 실제 V2 실행에서 같은 actor·query·deps로 now=2026-10-04에는 모집 2행, now=2026-10-11에는 0행이다. 이를 캐시에 연결하면 쓰기 통보가 없어 옛 2행을 hit하고 fetch를 호출하지 않았다. **새 발견으로 부풀리지 않고 명시된 한계를 실행 확인**한다. deps만으로 쓰기 없는 시간 변화까지 동기화할 수 없다.

### 다음 단계 전에 기술 계약으로 정할 것

| 순서 | 결정·수정할 내용 | 이유 |
|---|---|---|
| 1 · P1 | 늦은 read의 취소/재조회/stale 반환 계약과 actor 전환 시 호출자 결과 폐기; 관계 내부 select 타입 추론 수정 | R5-01·04. 다음 화면·전송 연결에 그대로 붙이면 옛 값 소비와 없는 필드 보장이 남는다 |
| 2 | 쓰기 변경 집합은 실제 커밋 결과의 안전 상위집합. 단일 전이 효과, W0/W1 조합·직접 create·후속 별도 커밋을 구분해 모두 통보 | 현재 write_tags는 단일 전이 facts 함수다. 원래 쓰기 응답의 태그로 미래 후속 커밋까지 알 수 없다 |
| 3 | 어떤 transport 상태를 unknown으로 분류할지, 미확정 작업 종결 전 cache 저장/재사용을 어떻게 제한할지, 재시도 멱등 키·상태 조회의 범위 | 한 번 clear와 진행 중인 커밋 안전성은 다르다(R5-02). commit 확인 전 성공/실패로 단정하지 않는다 |
| 4 | 커밋/무효화 통보와 read snapshot의 순서, 타 클라이언트·타 actor의 관련 쓰기 통보·재연결 시 놓친 변경의 처리 | 현 epoch는 이 캐시가 받은 통보만 관측한다. 실제 전송에서 누락이 없어야 현재 알고리즘의 가정이 성립한다 |
| 5 | actor/tenant/session/권한·계약 세대를 캐시 scope로 식별하고 전환 시 in-flight 결과까지 버리는 규칙 | RK-09 요구. 현재 setActor만으로 미래 모든 scope 전환을 식별하지 못한다 |
| 6 | query snapshot·키 정규화 범위, rows/deps의 소유권, 보관 한도 | JSON 객체 키 순서로 재사용이 줄고 가변 rows는 캐시를 바꾼다(R5-03). 정규화는 의미가 같은 요청만 합치도록 제한한다 |
| 7 | Id/Int wire 표현과 지원 범위, 시간 의존 조회의 next-expiry/TTL/재조회 조건 | R5-05 및 명시된 now 한계. 서버의 now와 만료 기준 시간을 일치시킨다 |

### 창시자 결정이 필요한 것

- **기본 동기화의 제품 보장**: DD-11은 재조회 기반 기본을 “검토”한다고 했다(`founder-integrated-directive-2026-10-03.md:664`). 자동 재조회 트리거, 다른 사용자의 쓰기를 반영하는 최대 지연, 오프라인·미확정 상태의 화면 표시를 어디까지 기본 SDK 책임으로 둘지 결정해야 한다. 실시간 통보를 필수 기본값으로 새로 확정하지 않는다.
- **안전한 과다 무효화와 비용의 우선순위**: resource 단위 안전 상위집합을 초기 기본으로 채택할지, SDK 저장 한도·TTL과 요청 비용 목표를 어디에 둘지. 현재 데이터는 정확성 표본이며 정밀 행 태그의 비용 우위를 입증하지 않는다.
- **Id 공개 표현**: 큰 bigint를 허용할 경우 string Id/codec을 공개 API로 채택할지, number를 유지하고 안전 정수 범위를 프레임워크 불변식으로 제한할지. 이는 단순 캐시 구현 선택보다 넓은 공개 계약이다.
- **시간 기준과 freshness 정책**: 마감 시각 같은 경계에서 얼마까지 낡은 결과를 허용할지와 만료·재조회 기본을 결정한다. `now` 의존을 무조건 모든 캐시 비활성으로 처리하는 정책은 아직 창시자 지시가 아니다.

기존 공개 계약을 어긴 R5-04의 타입 과대 보장 수정, 계정 전환 응답 분리, 기존 문서의 보장 범위 정리는 창시자에게 정책 승인을 요청할 사항으로 돌리지 않는다.

## 실행 원자료와 확인 한계

임시 복사본 위치: `/var/folders/7_/5w7y5vq9329g81pk_fv8_mhh0000gn/T/aip-r5-h3845h_u`.
환경 실측: node `v26.9.0`, 로컬 tsc `7.0.2`, cargo `1.98.1 (797e8a9bc 2026-08-05)`.
각 Rust 실행에서 `CARGO_TARGET_DIR`만 해당 원본 spike의 `target/`로 지정했다. V2 기본 `aip_v2_spike`는 `r5_2_spike`로, 나머지 하드코딩 테스트 schema도 전부 r5 이름으로 치환했다. 각 임시 이름이 기존 DB에 없음을 먼저 확인하고 CREATE했다.

| 실행 | 이번 출력 |
|---|---|
| 임시 spike-v2-read에서 `PATH="$HOME/.cargo/bin:$PATH" cargo test --offline -q -- --nocapture` | `4 passed; 0 failed` + `1 passed; 0 failed` |
| 임시 spike-v5-sdk에서 같은 명령(기존 테스트) | `2 passed; 0 failed` + deps `1 passed` + cache Rust `1 passed` |
| 임시 sdk에서 `node --test cache.test.ts` | `tests 3`, `pass 3`, `fail 0` |
| 임시 spike-v4-worker에서 같은 명령(그대로) | 종료 101. MacNetDeny worker 두 언어 WORKER_FAILED. 다른 실행에서 동일 sandbox 프로필로 기동 대조: `/usr/bin/sandbox-exec -p '(version 1)(allow default)(deny network*)' node -e 'process.stdout.write("worker-started\n")'` → `sandbox-exec: sandbox_apply: Operation not permitted`(종료 71) |
| 임시 spike-v4-worker에서 격리 블록만 제외 후 같은 명령 | v4_1 `1 passed; 0 failed`, outbox `1 passed; 0 failed` |
| 추가 `cargo test --offline -q --test r5_cases -- --nocapture` | `1 passed; 0 failed`; 원래 V2 재현·관계 선택 분기·write_tags(create/update/notify)·시간·큰 Id Node 왕복 |
| 추가 `cargo test --offline -q --test r5_extra -- --nocapture` | `1 passed; 0 failed`; F05 같은 worker 복구·명시적 null·enum 음성 대조·Time/Url/Ref 양성·F06 원래 재현 |
| 추가 `node cache-cases.ts`, `node cache-more.ts` | 종료 0. 늦은 반환·미확정/종결 대조·행 참조·키 순서·actor·두 쓰기 완료 순서·시간 경계 확인 |
| 추가 `tsc ... sdk/r5-original-neg.ts` | 종료 1: title/views TS2339, 빈/두 키 선택 TS2322(의도한 거부) |
| 추가 `tsc ... sdk/r5-types.ts`, `tsc ... sdk/r5variant/r5-id.ts` | 종료 0: 관계 안 동적 select 과대 보장 및 Id 문자열 범위 차이 확인 |

추가 시험의 첫 seed는 같은 club에 PUBLISHED 모집 2개를 넣어 `recruitment_at_most_one_published`에 거부됐다. 테스트 입력을 별도 club으로 나눠 재실행했다. Ref 출력 첫 입력도 원문에 facts 표기 `Ref<Club>`를 잘못 사용해 V1 parser가 거부했다. 원문 표기 `Club`로 바로잡고 V1 의미 검사를 거쳐 재실행했다. 두 실패는 실험 입력 오류이며 AIP 발견으로 집계하지 않았다.

확인 못함: 실제 transport·response-loss·재연결·다중 tenant·후속 알림 DB write·V3 쓰기와 SDK의 연결·제품 부하 성능. 현재 코드의 테스트/분리된 경계 실험 결과로 생산 단계 정확성을 승인하지 않는다. 결과 문서도 SDK API 결정이 아니라고 명시한다(`V5-sdk-results.md:3`).

## 최종 산출물 검증·DB 정리

보고서 구성 검사: R5 발견 P1 2건·P2 3건, F01~F09 판정 9행, Q1~Q4 답변과 확인 한계를 확인했다. 임시 schema `r5_2_spike`, `r5_2_r2b`, `r5_4`, `r5_4_outbox`, `r5_5`, `r5_cases`를 모두 `DROP SCHEMA IF EXISTS ... CASCADE`로 정리한 뒤 `pg_namespace`에서 해당 이름의 개수는 **0**이었다. 원본 cache.ts·cache.test.ts·aip.ts는 임시 복사본과 바이트 대조가 일치했다. git 명령·커밋은 사용자 금지에 따라 미실행.
