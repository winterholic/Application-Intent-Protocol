# AIP 문법 명세 v0.1

> 상태: Draft · 이 문서가 문법의 정본이다. 설계 문서(`docs/design/*`)의 예제는 이 명세를 따라야 하며, 어긋나면 검증 로그(`docs/design/09-verification-log.md`)에 기록하고 한쪽을 고친다.
> 표기: EBNF. `'kw'`는 키워드, `?` 0-1회, `*` 0회 이상, `+` 1회 이상, `|` 선택.

## 0. Canonical 규칙

> 구현: `crates/aip-syntax` (lexer, parser). 이 명세와 파서가 다르면 둘 중 하나가 버그이며, `aip check-docs`가 문서 예제 전체를 파서로 검사한다.

1. 같은 의미의 두 번째 표기를 두지 않는다. 대안 표기가 떠오르면 이 명세에 없으면 오류다.
2. 절(clause)의 순서는 고정이다. 포매터(`aip fmt`) 출력이 정본이며, 포매터 결과와 다른 파일은 W-FMT.
3. 비교는 `=`, 대입은 `set` 문 안의 `=`, `+=`, `-=`뿐이다. `==`는 없다.
4. 집합을 받는 집계는 전부 **집합식(set-expr)** 한 가지 모양을 쓴다: `count(order.items)`, `sum(order.items i: i.price * i.quantity)`, `all(targets t: outranks(actor, t))`.
5. 존재 검사는 키워드 `exists <set-expr>` 하나다. `exists(...)` 함수 표기는 없다.
6. 시간 길이는 숫자 + 단위 한 가지: `30s`, `10m`, `2h`, `7d`, `2w`, `6mo`, `1y`.
7. 오류 코드는 `UPPER_SNAKE_CASE`.
8. **레이아웃 규칙 (EBNF 밖)**
   - 집합식의 별칭은 원천(source)과 같은 줄에 있어야 하고 절 키워드가 아니어야 한다. 그래서 `exists membership(a, c)` 다음 줄의 `require`는 별칭이 되지 않는다.
   - 필드 수식어는 같은 줄에 쓰거나, 다음 줄이면 필드보다 깊게 들여쓴다. 필드와 같은 들여쓰기의 줄은 새 멤버다.
   - `[a, b]`는 항상 목록이다. 반열린 구간만 `[a, b)`로 쓴다(닫힌 구간 리터럴은 없다).

## 1. 어휘

```ebnf
ident        = letter (letter | digit | '_')* ;
Type-ident   = upper (letter | digit)* ;             (* 엔티티, enum, record, intent 이름 *)
CODE         = upper (upper | digit | '_')* ;
int          = digit+ ;
decimal      = digit+ '.' digit+ ;
string       = '"' { char } '"' ;
duration     = int ('s' | 'm' | 'h' | 'd' | 'w' | 'mo' | 'y') ;
size         = int ('KB' | 'MB' | 'GB') ;
time-of-day  = digit digit ':' digit digit ;
regex        = '/' { char } '/' ;
comment      = '//' { char } newline ;
```

예약어는 부록 A.

## 2. 파일 구조

```ebnf
file         = decl* ;
decl         = use | actor | enum | record | entity | relation | fn | event | upcast
             | intent | on-event | schedule | retain | rule | projection | search
             | job | verification | grant-link | approval | expose | outbound-webhooks
             | consent | config | flag | impersonate | migration | removed-entity ;

use          = 'use' ident ('.' ident)* ;
actor        = 'actor' Type-ident 'via' call-target ('scopes' '[' scope-name (',' scope-name)* ']')?
               ('superuser' 'when' expr 'audited')? ;       (* 모든 allow/visible에 일관 적용되는 전역 우회 *)
scope-name   = ident ('.' ident)* ;
enum         = 'enum' Type-ident 'ordered'? '{' enum-body '}' ;
enum-body    = Type-ident+ | Type-ident ('<' Type-ident)+ ;      (* ordered는 '<'로 서열 표기 *)
record       = 'record' Type-ident '{' (field-decl | record-check)* '}' ;
record-check = 'check' ('when' expr ':')? expr 'else' CODE ;
```

## 3. 타입

```ebnf
type         = base-type ('?')? ;
base-type    = scalar | Type-ident | 'ref' Type-ident ('|' Type-ident)+
             | 'Set' '<' type '>' 'max' int
             | 'List' '<' type '>' 'max' int
             | 'Range' '<' type '>'
             | 'Json' '<' Type-ident '>' | 'Json' 'validated' 'by' path
             | 'Localized' '<' type '>'
             | 'Upload' '(' upload-opt (',' upload-opt)* ')'
             | 'Credential' '<' ident ('.' ident)* '>'
             | 'Money' '(' Type-ident ')'
             | 'Snapshot' '<' Type-ident '>'                             (* history 엔티티의 특정 버전 *)
             | Type-ident '[' ']'                                        (* 역방향 1:N, 'via' 필수 *)
             | ident '.' Type-ident ;                              (* extension 타입: s3.Object *)
scalar       = 'Bool' | 'Uuid' | 'Time' | 'Date' | 'Duration' | 'Recurrence'
             | 'Int' refine? | 'Decimal' '(' int ',' int ')' refine?
             | 'Text' refine? | 'RichText' '(' 'policy' ':' ident ')'
             | 'Email' | 'Url' | 'Phone' '(' Type-ident ')' ;
refine       = '(' refine-opt (',' refine-opt)* ')' ;
refine-opt   = range-spec | 'trim' | 'lower' | 'matches' regex ;
range-spec   = int? '..' int? ;
upload-opt   = 'max' size | 'types' '[' ident (',' ident)* ']' ;
```

## 4. 엔티티

```ebnf
entity       = 'entity' Type-ident ('was' Type-ident)? entity-mod* '{' entity-member* '}' ;
entity-mod   = 'personal' ('access' 'audited')? ;

entity-member= field-decl | removed-field | constraint | lifecycle | visibility | predicate | entity-trait ;

field-decl   = ident ':' type field-default? field-mod* ;
field-default= '=' expr ;
field-mod    = 'was' ident                                  (* 이 필드는 예전에 old라는 이름이었다 *)
             | 'personal' | 'encrypted'
             | 'visible' 'to' pred-ctx
             | 'masked' 'unless' expr 'as' call-target
             | 'on' 'delete' ref-policy
             | 'on' 'erase' ref-policy
             | 'via' ident                                  (* T[] via backref *)
             | 'tree' ('max' 'depth' int)?
             | 'sequence' 'per' path 'format' string
             | 'slug' 'from' ident 'unique' ('per' path)?
             | 'position' 'within' path
             | 'counter' 'via' ident counter-opt*
             | 'variants' '{' (ident ':' string ','?)* '}' ;        (* thumb: "200x200" *)
ref-policy   = 'cascade' | 'restrict' | 'set' 'null' | 'anonymize' | 'reassign' 'to' expr ;
counter-opt  = 'dedupe' 'by' ident 'within' duration | 'window' duration | 'sharded' int ;
pred-ctx     = expr ;                                        (* actor, self 사용 가능 *)

constraint   = 'unique' '(' ident (',' ident)* ')' ('where' expr)? ('else' CODE)?
             | ('exactly' | 'at' 'most' | 'at' 'least') 'one' 'where' expr 'per' ident repair?
             | 'no' 'overlap' '(' ident ')' 'per' ident ('else' CODE)?
             | 'capacity' 'count' '(' set-expr ')' '<=' expr 'else' CODE
             | 'invariant' ident ':' expr ;
repair       = 'repair' 'on' 'erase' 'do' block ;            (* 블록 안에서 per 필드 이름은 깨진 그룹의 값 *)

lifecycle    = 'lifecycle' ident '{' transition+ '}' ;
transition   = Type-ident (',' Type-ident)* '->' Type-ident (',' Type-ident)* ;
visibility   = 'visible' 'to' 'actor' ('when' | 'unless') expr ;
predicate    = 'predicate' ident '=' expr ;                  (* x is <ident> 로 사용 *)

entity-trait = 'track' track-item (',' track-item)* ('by' 'actor')?
             | 'history'
             | 'soft' 'delete' ('retain' duration)?
             | 'versioned'
             | 'publishable' ('by' expr)?                     (* 초안과 게시: 아래 절. by 는 게시와 되돌리기를 허용하는 조건 *)
             | 'tenant' 'via' path
             | 'dynamic' 'schema' 'from' ident ;
track-item   = 'created' | 'updated' ;
```

### 테넌트 (`tenant via`)

`tenant via a.b`가 붙은 엔티티를 테넌트 범위 엔티티라 한다. 경로는 필수 참조 필드의 사슬이고, 끝 엔티티(루트, 예: `Workspace`)가 그 행의 테넌트다. 루트는 자기 자신이 테넌트다. 경로가 선택 참조를 지나거나 존재하지 않는 필드를 가리키거나 앱 안에서 서로 다른 루트로 끝나면 AIP-E313이다.

```aip
entity Workspace { name: Text }
entity Project {
  workspace: Workspace
  tenant via workspace
}
entity Task {
  project: Project
  tenant via project.workspace
}
```

규칙은 다섯 가지다.

1. **참조는 테넌트를 넘지 않는다.** 테넌트 범위 행이 테넌트를 가진 다른 행(테넌트 범위 행이나 루트)을 참조하면 두 행의 테넌트가 같아야 한다. 데이터베이스가 INSERT와 UPDATE에서 검사하고, 어기면 `AIP.TENANT.MISMATCH`다. 경로 중간 행을 다른 테넌트로 옮기는 경우도 같다. 옮겨진 행에서 경로가 시작하는 하위 행까지 따라 내려가며 검사하므로, 하위 행이 다른 테넌트에 남은 행을 참조하게 되면 옮기기가 거부된다.
2. **intent 하나는 테넌트 하나만 다룬다.** 테넌트를 가진 엔티티 타입의 파라미터(집합 파라미터 포함)가 한 호출에서 같은 테넌트의 행이어야 하고, 아니면 Load 뒤에 `AIP.TENANT.MISMATCH`다. 필수 파라미터 중 첫 번째가 그 intent의 앵커 테넌트다. 경로의 첫 참조 필드에 값을 쓰는 insert와 update는 새 값이 앵커 테넌트의 행을 가리켜야 한다.
3. **읽기는 앵커 테넌트로 걸러진다.** 테넌트 범위 엔티티를 `from`으로 읽는 query와 집합 식(`count(Task t where ...)`, update/delete 대상 포함)에는 앵커의 테넌트 조건이 하강 때 들어가고, 이를 끄는 문법은 없다. 엔티티 파라미터에서 참조와 역참조(`project.tasks`)로만 도달한 집합은 필터가 없다. 파라미터는 2번으로 같은 테넌트가 보장되고, 테넌트를 가진 엔티티 사이의 참조는 1번으로 테넌트를 넘지 못하기 때문이다. 앵커 필수 파라미터가 없는 intent가 그 밖의 테넌트 범위 집합을 읽으면 AIP-E314다.
4. **한 트랜잭션의 쓰기는 한 테넌트다.** 테넌트 범위 엔티티와 루트 테이블의 INSERT, UPDATE, DELETE마다 데이터베이스 트리거가 그 행의 테넌트를 계산해 트랜잭션 로컬 설정 `aip.tenant`에 고정한다. 이미 고정된 테넌트와 다르면 `AIP.TENANT.MISMATCH`다. UPDATE는 행이 떠나는 테넌트도 같이 검사한다. 앵커가 있는 intent는 Load 뒤에 `aip.tenant`를 앵커 테넌트로 미리 고정하고, 앵커가 없으면 첫 쓰기가 고정한다. 호출자가 없는 문맥(webhook, 이벤트 핸들러, schedule 항목, rule 행, job 항목)도 같은 트리거 한 곳에서 묶이고, 한 트랜잭션에 여러 항목을 담는 문맥은 항목마다 고정을 초기화해서 시작한다.
5. **테넌트 간 작업은 `cross tenant`로 선언한 것뿐이다.** intent는 클라이언트가 호출할 수 없는 `internal cross tenant`만 가능하다. `internal` 없이 쓰면 AIP-E315다. caller가 없는 `on Event`, `schedule`, `rule`, `consume`과 webhook 핸들러는 `cross tenant`를 앞에 붙일 수 있다(유지보수성 작업용). 이 선언은 2, 3, 4번을 적용받지 않는다. 다만 서로 다른 테넌트의 행을 잇는 참조(1번)는 `cross tenant`에서도 거부된다.

actor 없는 실행 문맥의 기본값은 다음과 같다. `rule`은 규칙이 걸린 행, `schedule ... for X x`는 항목 행(항목을 고르는 `for` 자체는 모든 테넌트를 훑는다), `on Event`는 이벤트 필드 중 테넌트를 가진 엔티티를 참조하는 첫 필드가 앵커다. 앵커가 없는 문맥이 테넌트 범위 집합을 읽으면 AIP-E314다. `webhook`, `consume`은 페이로드를 읽기 전에는 테넌트를 알 수 없어서 읽기 필터가 없지만, 쓰기는 4번으로 한 테넌트에 묶인다. `retain`은 프레임워크가 정의한 행 단위 만료여서 한 문장이 여러 테넌트의 행을 지우며, 고정 검사를 받지 않는다. L3 폼은 approval은 대상 행, job과 grant link와 verification과 subscribe는 파라미터가 앵커이고 2, 3, 4번이 같게 적용된다.

누가 그 테넌트에 속하는지(멤버십)는 이 규칙이 다루지 않는다. 앱이 `allow`와 `visible to`로 정한다. 보이지 않는 행은 NOT_FOUND이고, 두 테넌트를 섞은 호출만 `AIP.TENANT.MISMATCH`(HTTP 404)다.

### 초안과 게시 (`publishable`)

`publishable`이 붙은 엔티티는 행마다 두 버전을 가진다. 작업본은 엔티티 자신의 테이블이다. command, rule, job은 지금처럼 이것을 읽고 고친다. 게시본은 `<table>_published` 테이블이고 클라이언트가 읽는 쪽이다.

1. **query는 게시본만 읽는다.** 게시된 적 없는 행은 목록에도 없고, 엔티티 파라미터로 받으면 NOT_FOUND다. 편집 command는 작업본만 바꾸므로 게시본은 그대로다.
2. **`drafts query`만 작업본을 읽는다.** 수식어는 `internal`, `cross tenant` 다음에 쓴다(`internal drafts query ...`). 읽는 엔티티가 publishable이 아니거나 `allow public`이면 AIP-E208이다. 초안은 편집자의 것이라 호출 조건을 직접 써야 한다.
3. **생성 intent는 둘이다.** `PublishE(e)`는 작업본 행을 게시본에 덮어쓰고(없으면 만들고) 게시 시각을 기록한다. 반환값은 `publishedAt`이다. `DiscardEDraft(e)`는 게시본을 작업본에 덮어쓴다. 게시된 적 없는 행이면 `AIP.PRECONDITION.FAILED`(`NOT_PUBLISHED`)다. 되돌릴 버전이 없는데 초안을 그대로 두면 실패가 숨기 때문이다.
4. **누가 게시하는가는 `publishable by <조건>`이다.** 조건은 엔티티 안의 `visible to`처럼 행의 필드와 `actor`를 쓴다. 두 intent가 같은 조건을 쓰고 superuser는 조건 없이 통과한다. `by`가 없으면 superuser만 통과하고, superuser도 선언되지 않았으면 아무도 게시할 수 없으므로 AIP-E301이다.
5. **삭제는 게시본도 지운다.** 작업본 행이 지워지거나 soft delete 되면 게시본이 같은 트랜잭션에서 사라진다. 게시본은 제약 없는 스냅숏이고 제약은 작업본이 강제한다. 되돌리기는 게시본의 값을 작업본에 다시 쓰므로 작업본의 제약(unique 등)이 그 값을 다시 판정한다.
6. **한계.** `touch` 카운터는 작업본에서 오르고 게시본에는 다음 게시 때 반영된다. 게시본이 가진 `version`은 작업본의 것과 다를 수 있어서, 낙관적 갱신에 쓸 버전은 `drafts query`에서 읽어야 한다.

```aip
entity Article {
  site: Site
  title: Text
  publishable by editorOf(actor, site)
}
query Articles(site: Site) {
  allow public
  from Article a
  where a.site = site
  page 20 by keyset
  select { id title }
}
drafts query ArticleDrafts(site: Site) {
  allow editorOf(actor, site)
  from Article a
  where a.site = site
  page 20 by keyset
  select { id title }
}
```

### 스키마 진화 (`was`, `removed`)

이미 배포된 데이터베이스를 새 프로그램으로 옮기는 일은 `aip migrate`와 `aip run`이 한다. 배포에 성공할 때마다 그 프로그램의 Core IR(canonical JSON)과 digest,
생성기 버전을 `_aip_deployment`에 기록하고, 다음 배포는 기록된 IR과 새 IR의 차이(`aip-ir`의 `diff`)를 SQL 단계로 바꿔 한 트랜잭션에서 적용한다.
빈 데이터베이스는 전체를 새로 만들고, 기록이 없는 데이터베이스는 스키마가 프로그램과 맞으면 기록만 남긴다(맞지 않으면 AIP.SCHEMA.UNRECORDED).
단계는 항상 이 순서다: 확장과 내부 테이블, 이름 변경, 낡은 제약과 인덱스 삭제, 새 테이블, 컬럼 추가와 변경, 선언한 삭제, 새 제약과 인덱스, 생성기가 소유한 트리거와 함수 교체.
적용 결과는 같은 프로그램을 빈 데이터베이스에 처음 배포했을 때와 같은 스키마여야 한다(e2e가 카탈로그 덤프로 비교한다).

```ebnf
removed-field  = 'removed' 'field' ident ;       (* entity 본문 안. 이 필드의 컬럼과 값을 일부러 버린다 *)
removed-entity = 'removed' 'entity' Type-ident ; (* 최상위. 이 엔티티의 테이블과 행을 일부러 버린다 *)
```

프로그램에서 사라진 필드와 엔티티는 지워진 것인지 이름이 바뀐 것인지 알 수 없고, 컬럼 삭제는 데이터를 버리지만 이름 변경은 보존한다. 그래서 의도를 문법으로 적는다. 의도마다 표현은 하나다.

```aip
entity Ticket was Task {
  headline: Text(1..100) was title
  removed field legacyCode
}
removed entity OldThing
```

- 이름 변경은 `was`다: 필드는 `새이름: 타입 was 옛이름`, 엔티티는 `entity 새이름 was 옛이름`. 컬럼과 테이블은 `RENAME`으로 옮겨져 값이 그대로 남고, 이름에서 나온 인덱스, 제약, 트리거는 새 이름으로 다시 만들어진다.
- 삭제는 `removed`다: `removed field x`는 엔티티 안, `removed entity X`는 최상위. 값이 사라지는 단계는 계획에서 `declared`로 표시된다.
- 선언은 배포가 끝난 뒤에도 소스에 남겨도 된다. 데이터베이스에 옛 이름이 없으면 아무것도 하지 않는다.
- 프로그램과 모순되면 AIP-E111이다: 자기 자신의 이름이나 아직 선언된 이름이 `was`, 아직 선언된 필드나 엔티티가 `removed`, 같은 옛 필드를 잇는 필드가 둘, 이름 변경과 삭제를 동시에 선언, record 필드의 `was`.
- 데이터 변환이 필요한 변경은 선언 하나로 되지 않는다. 새 필드를 더하고 `migration`이 값을 옮긴 다음 다음 배포에서 옛 필드를 `removed field`로 지운다. `migration`은 스키마를 맞춘 뒤 같은 배포에서 실행되므로, 그 결과에 기대는 더 엄격한 규칙(필수 전환, unique)은 그 다음 배포에 둔다.

변경 종류별 처리(분류는 `aip migrate --plan`이 단계마다 출력한다):

| 변경 | 처리 | 분류 |
|---|---|---|
| 엔티티, 선택 필드, 기본값이 있는 필수 필드, 참조 필드 추가 | 자동 | `safe` |
| 기본값 없는 필수 필드 추가 | 테이블이 비어 있을 때만 자동, 아니면 AIP.SCHEMA.DATA_CONFLICT | `checked` |
| 일반 인덱스 추가 | 자동. 트랜잭션 안이라 `CONCURRENTLY`를 쓰지 않는다(쓰는 동안 쓰기가 기다린다) | `safe` |
| unique, check, enum 값 목록 변경 | 기존 행을 먼저 검사한다. 어긴 행이 있으면 거부하고 개수와 id 예시를 낸다 | `checked` |
| enum 값 추가(끝) / 중간 | 자동 / 자동하고 `ordered` enum은 비교의 의미가 바뀐다고 알린다 | `safe` |
| enum 값 삭제, 이름 변경 | 그 값을 쓰는 행이 없을 때만 자동(이름 변경은 삭제와 추가다) | `checked` |
| enum 값 순서 변경(`ordered`) | AIP.SCHEMA.UNSUPPORTED | 거부 |
| 길이와 범위 완화, 필수에서 선택으로, 기본값 변경 | 자동 | `safe`, `relaxing` |
| 길이와 범위 축소 | 기존 값이 맞을 때만 자동(`Text`, `Int`), 그 밖의 축소는 AIP.SCHEMA.TYPE_CHANGE | `checked` |
| 선택에서 필수로 | NULL 행이 없을 때만 자동 | `checked` |
| 다른 기본 타입, 다른 enum, pattern 변경 | AIP.SCHEMA.TYPE_CHANGE | 거부 |
| 선언 없는 필드와 엔티티 삭제 | AIP.SCHEMA.UNDECLARED (`was`나 `removed`를 쓰라고 안내) | 거부 |
| `was`, `removed`를 쓴 이름 변경과 삭제 | 자동 | `safe`, `declared` |
| 필드 종류 변경, `history`, `publishable`, `tenant via`, `dynamic schema from` 추가와 삭제 | AIP.SCHEMA.UNSUPPORTED | 거부 |
| 제약, 인덱스 삭제 | 자동하고 알림 | `relaxing` |
| 트리거와 함수 | 생성기가 소유한 것은 매번 현재 텍스트로 교체하고, 프로그램에 없는 것은 지운다 (이름에 `__`가 든 트리거) | `refresh` |

`aip migrate --plan <file>`은 데이터베이스에 붙어 적용할 SQL과 분류, 검사 결과를 출력만 하고 아무것도 바꾸지 않는다(검사는 읽기 전용 질의다. 이번 변경이 만드는 컬럼을 가리키는 검사는 적용할 때 실행된다고 표시한다). 막히면 종료코드 1이다. `aip migrate <file>`은 계획을 한 트랜잭션으로 적용하고, 하나라도 거부 대상이거나 검사에 걸리면 아무것도 바꾸지 않는다. `aip run`은 기동할 때 같은 일을 하고 거부 대상이 있으면 기동하지 않는다. 동시에 기동한 프로세스는 `migration`과 같은 advisory lock 안에서 순서대로 처리되어 변경은 한 번만 적용된다. `--reset`은 그대로 전부 지운다.

`aip diff <file> --against <old-core-ir.json | deployed>`는 같은 두 IR의 공개 계약 차이를 클라이언트 쪽에서 분류한다(`deployed`는 `_aip_deployment`의 최신 IR). 깨짐(종료코드 1): intent 삭제와 이름 변경, 종류 변경, 필수 입력 추가, 입력 삭제, 선택에서 필수로, 입력 타입 축소와 변경, 출력 필드 삭제, nullable화, 출력 타입 변경, 멱등성 제거와 키 필수화, 입력으로 쓰이는 enum 값 삭제. 경고: 오류 코드 추가, `allow`와 `requires` 변경, 출력으로 쓰이는 enum 값 추가. 호환: 그 밖의 추가와 완화. `--json`은 항목별 `{level, rule, subject, message}` 배열이다.

## 5. 관계와 함수

```ebnf
relation     = 'relation' ident '(' params ')' (':' Type-ident)? '=' (expr | set-expr) ;
fn           = 'fn' ident '(' params ')' ':' type ('=' expr | 'wasm' string) ;
params       = (param (',' param)*)? ;
param        = ident ':' type ('=' expr)? ;
```

`fn ... wasm "<module>"`은 샌드박스 순수 함수(escape hatch)다.

## 6. Intent

```ebnf
intent       = intent-mod? (query | command | subscribe | webhook | consume) ;
intent-mod   = 'internal' ('cross' 'tenant')?
             | 'cross' 'tenant' ;                           (* internal 없이는 파싱만 되고 AIP-E315로 거부된다 *)

query        = 'drafts'? 'query' Type-ident ('(' params ')')? query-mod* '{' query-body '}' ;
query-mod    = 'cached' duration ('per' cache-key)?
             | 'limit' rate (',' rate)* ;
cache-key    = 'actor' | 'actor-class' ;
query-body   = let* allow fetch* from? where? group? sort? page? plan? consistency? select touch* ;
let          = 'let' ident '=' expr ('else' CODE)? ;
fetch        = 'fetch' call 'as' ident ;
from         = 'from' (Type-ident ident | ident | call ident) ;     (* 엔티티+별칭 | 엔티티 파라미터 | 검색 호출 *)
where        = 'where' expr ;
group        = 'group' 'by' path (',' path)* ;
sort         = 'sort' 'by' (sort-key (',' sort-key)* | ident 'of' '{' (Type-ident ':' sort-key (',' sort-key)*)+ '}') ;
sort-key     = expr ('asc' | 'desc') ;
page         = 'page' int 'by' ('keyset' | 'offset' 'max' 'page' int) ;
plan         = 'plan' ('batch' | 'join') ;
consistency  = 'consistency' ('strong' | 'eventual') ;
select       = 'select' selection ;
selection    = '{' sel-item* '}' ;
sel-item     = ident selection?                              (* 필드 또는 관계 *)
             | ident ':' expr selection? ;                   (* 파생 필드 *)
touch        = 'touch' path ;

command      = 'command' Type-ident ('(' params ')')? command-mod* '{' command-body '}' ;
command-mod  = 'idempotent' ('by' expr)?
             | 'audited'
             | 'limit' rate (',' rate)* ;
rate         = int 'per' duration 'per' ('actor' | 'client' | path) ;
command-body = let* allow require* do? emit* returns? ;
allow        = 'allow' expr ('else' CODE)? ;
require      = 'require' ('when' expr ':')? expr 'else' CODE ;
do           = 'do' block ;
emit         = 'emit' Type-ident '{' field-assigns? '}' ('to' ident 'topic' string ('key' expr)?)? ;
returns      = 'returns' expr selection? ;

subscribe    = 'subscribe' Type-ident '(' params ')' '{' allow from where? select '}' ;
(* subscribe X(params) { allow ...; from ...; where ...; select ... }: 클라이언트가 열어 두는 query다. 열 수 있는 것은 선언된 구독뿐이고(임의 쿼리는 없다)
   결과 집합이 바뀌면 갱신을 받는다. 결과는 `page`와 `sort` 없이 id 순의 목록 전체이고(`from 파라미터`면 한 행짜리 목록), 한 구독은 `max_rows`(기본 1000)행까지
   싣는다(넘으면 AIP.SUBSCRIPTION.TOO_LARGE로 그 구독이 끝난다). 계획은 같은 본문의 list query와 같다: allow, `visible to`, 필드 가시성, 테넌트 필터가 그대로 적용되고,
   테넌트 범위 엔티티를 읽으려면 테넌트를 정하는 파라미터가 있어야 한다(AIP-E314). 외부 호출 `from X.match(...)`(검색)은 아직 계획이 없어 AIP-E601이다.
   WebSocket 프로토콜은 아래 "구독 프로토콜" 절. *)
webhook      = 'webhook' Type-ident 'via' call-target '{' (cross-tenant? 'on' (Type-ident | string) '(' ident (':' type)? ')' 'do' block)+ '}' ;
cross-tenant = 'cross' 'tenant' ;                           (* 앞의 4절 5번. 핸들러 단위로 붙인다 *)
(* via: http.webhook(secret: "ENV", header?, event?, id?, payload?) | payments.stripe.webhook(secret?: "ENV").
   secret는 환경 변수 이름이다. 비밀 값은 소스에 쓰지 않는다. 옵션의 event/id/payload는 본문 JSON의 점 경로.
   수신: 서명 검증 → 이벤트 id로 중복 제거 → outbox 저장 → 즉시 2xx. 핸들러는 디스패처가 재시도와 함께 실행한다. *)
consume      = cross-tenant? 'consume' ident 'topic' string 'as' Type-ident '{'
                 'key' ':' ident ('dedupe' 'by' ident)? 'do' block '}' ;
```

`allow`는 모든 intent에 필수다(E-ALLOW-MISSING). webhook과 consume은 `allow`가 없는 대신 via 호출(서명 검증)과 dedupe가 신원/멱등을 대신하며, 이는 extension 하강 결과로 IR에 삽입된다.

### 구독 프로토콜 (WebSocket)

`GET /aip/subscribe`를 WebSocket으로 연다. 메시지는 JSON 텍스트 프레임이다. 한 연결에 구독 여러 개를 열 수 있고, 구독은 클라이언트가 정한 `id`(1~64자)로 구분한다.

```text
client -> server
  {"type":"auth","token":"<bearer>"}                 연결의 첫 메시지. token을 빼면 익명(allow public인 구독만 열린다). --dev-auth에서는 "actor":"<uuid>"도 된다
  {"type":"subscribe","id":"s1","name":"X","input":{...}}
  {"type":"unsubscribe","id":"s1"}
server -> client
  {"type":"ready","protocol":"aip-subscribe/1"}      auth가 받아들여졌다
  {"type":"snapshot","id":"s1","rows":[...]}         구독을 연 직후 결과 전체
  {"type":"changed","id":"s1","rows":[...]}          결과가 바뀐 뒤 결과 전체(diff가 아니다). 직전에 보낸 것과 같으면 보내지 않는다
  {"type":"error","id":"s1","code":"AIP...","reason":...,"message":...,"retryable":...}   id가 있으면 그 구독이 끝났다
```

- 인증은 첫 메시지 `auth`다. 브라우저의 `WebSocket`은 헤더를 붙일 수 없고, 쿼리 파라미터의 토큰은 접속 로그, 프록시 로그, 브라우저 기록에 남는다. 헤더를 붙일 수 있는 클라이언트는 업그레이드 요청의 `Authorization: Bearer`로 대신해도 되고, 그러면 `auth`를 보내지 않는다. 5초 안에 인증하지 않거나 `auth`보다 먼저 `subscribe`를 보내면 `AIP.AUTH.UNAUTHENTICATED` 뒤에 연결이 닫힌다. 토큰이 만료되면 열린 구독 모두에 같은 오류를 보내고 연결을 닫는다(연결은 토큰보다 오래 살 수 없다). 쿠키 같은 암묵적 자격 증명을 쓰지 않으므로 다른 사이트의 페이지가 연결을 가로챌 수 없다.
- 오류에 `id`가 없으면(JSON이 아니거나, 모르는 `type`, 바이너리 프레임, 인증 실패) 연결이 닫힌다. `id`가 있는 오류는 그 구독만 끝낸다. 선언되지 않은 이름(query, command, 없는 이름 모두)은 `AIP.REQUEST.UNKNOWN_INTENT`, 입력 검증은 query와 같은 `AIP.INPUT.INVALID`다.
- 변경 감지는 PostgreSQL `LISTEN/NOTIFY`다. 구독의 계획이 읽는 테이블(allow, 가시성, 테넌트 경로, 관계가 닿는 테이블까지 계획 SQL에서 뽑는다)마다 문장 단위 AFTER 트리거가 `aip_changed`로 테이블 이름을 알리고, NOTIFY는 커밋 때 전달되므로 롤백된 변경은 알리지 않는다. 런타임은 연결 하나로 듣고, 알림이 온 테이블을 읽는 구독만 100ms 모아 한 번 다시 조회한다. 듣는 연결이 끊겼다 돌아오면 모든 구독이 한 번 다시 조회한다.
- 다시 조회할 때마다 구독자로 query 하나를 처음부터 실행한다. actor가 아직 있는지, 대리 세션 토큰이면 세션이 열려 있는지, allow, `visible to`, 필드 가시성, 테넌트 필터가 모두 그 시점 기준이다. 권한이 철회되면 다음 갱신에서 `error`(allow가 닫히면 AIP.AUTH.FORBIDDEN, 행을 못 보게 되면 AIP.NOT_FOUND, 세션이 끝나면 AIP.AUTH.UNAUTHENTICATED와 reason IMPERSONATION_ENDED)를 받고 그 구독이 끝난다. 구독자가 볼 수 없는 행은 스냅숏에도 갱신에도 없고, 그런 행이 바뀌어도 결과가 같으면 메시지가 가지 않는다. 데이터베이스가 잠깐 쓸 수 없는 경우(AIP.UNAVAILABLE, 직렬화 충돌)는 구독을 끝내지 않고 1초 뒤 다시 시도한다.
- 한 갱신은 repeatable read 스냅숏 하나의 결과다. 100ms보다 가까운 변경은 한 갱신으로 합쳐질 수 있고 중간 상태는 오지 않을 수 있다. 마지막 갱신은 항상 최신 커밋을 반영한다.
- 상한(서버 기본값): 연결 1000, 연결당 구독 20, 전체 구독 5000, 동시에 도는 재조회 8, 구독당 결과 1000행, 메시지 64KB. 넘으면 AIP.SUBSCRIPTION.LIMIT(HTTP 업그레이드 단계면 429)이고 다른 구독은 영향이 없다. 수천 구독의 팬아웃 최적화(변경 행으로 거르기, 같은 입력의 구독 공유)는 하지 않는다(OI-10).
- 클라이언트는 재연결하지 않는다. 연결이 끊기면 생성된 TS 클라이언트의 `onError`가 `AIP.UNAVAILABLE`(retryable)로 알리고, 호출자가 다시 `subscribe`하면 새 스냅숏부터 시작하므로 끊긴 동안의 변경을 놓치지 않는다. 자동 재연결과 백오프는 클라이언트 라이브러리의 몫으로 남긴다.
- 계약(`GET /aip/describe`)에 `subscriptions`가 있고(입력, 출력 Shape, 오류, 보장) `transport.subscribe`가 연결 방법을 알린다. 생성 TS 클라이언트는 `client.subscribe(name, input, onRows, onError?)`를 만들고 `Subscription.close()`를 돌려준다. `aip-protocol/0.1`은 올리지 않았다: 계약 문서에 키를 더했을 뿐이고(`jobs`, `outbound_webhooks`를 더했을 때와 같다) 구독이 없는 프로그램의 계약은 바이트 그대로이며 기존 클라이언트는 영향이 없다. 메시지 프로토콜은 `ready`의 `protocol`(`aip-subscribe/1`)로 따로 버전을 가진다.
- 대상 DB가 스키마를 이미 갖고 있으면 알림 트리거는 `--reset` 없이는 생기지 않는다(스키마 변경은 아직 `--reset`뿐이다). 트리거가 없으면 스냅숏은 오지만 갱신이 오지 않는다.

## 7. 문장

```ebnf
block        = '{' stmt* '}' ;
stmt         = let | insert | update | delete | purge | erase | upsert | toggle | set | when
             | each | effect | reserve | at-run | notify | export-pd ;

insert       = 'insert' Type-ident ('from' set-expr)? '{' field-assigns '}' ('as' ident)? ;
upsert       = 'upsert' Type-ident 'by' '(' ident (',' ident)* ')' '{' field-assigns '}' ('as' ident)? ;
update       = 'update' set-expr ('via' path ident)? 'set' assign (',' assign)* ;
delete       = 'delete' set-expr ;
purge        = 'purge' set-expr ;
erase        = 'erase' expr ;
toggle       = 'toggle' Type-ident '{' field-assigns '}' ;
set          = 'set' assign (',' assign)* ;
assign       = target ('=' | '+=' | '-=') expr ;
target       = path | call '.' ident ;                       (* membership(actor, c).role *)
when         = 'when' expr (':' stmt | block) ;
each         = 'each' set-expr 'partial' block ;
effect       = call ('as' ident)? ('into' path)? ('on' 'failure' 'do' block)? ;   (* deferred 효과가 재시도 한도를 넘으면 block을 로컬 트랜잭션으로 실행 *)
reserve      = ('reserve' | 'confirm' | 'release') ident 'of' path ('for' duration)? ('else' CODE)? ;
at-run       = 'at' expr 'run' Type-ident '(' args? ')' ;
notify       = 'notify' expr 'via' call-target ('{' field-assigns '}')?
               ('respecting' 'preferences' '(' 'category' ':' Type-ident ')')?
               ('digest' 'every' duration)? ;
export-pd    = 'export' 'personal' 'data' 'of' expr 'to' ident ('notify' expr)? ;

field-assigns= field-assign (',' field-assign)* ;
field-assign = ident (':' expr)? | '...' expr ;             (* 이름만 쓰면 같은 이름의 값 *)
```

- `each ... partial`만 반복 모양을 가지며, 입력 집합 상한이 있고 본문은 항목별 savepoint로 실행된다. 결과는 항목별 성공/실패 목록이다.
- `page` 절이 있는 query에는 transport 수준 cursor 입력이 자동으로 붙는다. 파라미터로 선언하지 않는다.
- `consume`의 본문에서는 수신 메시지가 `msg`로 바인딩된다.
- `update X set f = e`에서 좌변은 대상 행의 필드 이름이고, 우변의 맨 이름은 파라미터/let/별칭으로 해석한다(대상 필드를 우변에서 쓰려면 별칭 경로).
- `update X via path alias set ...`에서 우변의 집계(`sum(alias.f)`)는 대상 행별로 모인 조인 행을 범위로 한다. 이 경우에만 집계 인자에 별칭 없는 경로를 허용한다.

## 8. 식

```ebnf
expr         = or ;
or           = and ('or' and)* ;
and          = not ('and' not)* ;
not          = 'not' not | cmp ;
cmp          = add (cmp-op add | 'in' (list | range-lit | add) | 'is' ident | 'has' 'scope' scope-name)? ;
cmp-op       = '=' | '!=' | '<' | '<=' | '>' | '>=' ;
add          = mul (('+' | '-') mul)* ;
mul          = unary (('*' | '/') unary)* ;
unary        = '-' unary | postfix ;
postfix      = primary ('.' ident)* ;
primary      = literal | path | call | aggregate | quantified | 'exists' set-expr
             | 'the' set-expr | 'latest' set-expr 'by' expr
             | 'first' set-expr 'order' 'by' sort-key (',' sort-key)*
             | 'if' expr 'then' expr 'else' expr
             | '(' expr ')' | list | range-lit
             | 'actor' | 'self' | 'this' | 'now' | 'today'
             | 'public' | 'authenticated' ;                 (* allow 전용 술어: true / actor != null *)
aggregate    = ('count' '(' set-expr ')')
             | ('sum' | 'min' | 'max' | 'avg') '(' set-expr (':' expr)? ')'
             | 'count' '(' ')'                                (* group by 문맥 *)
             | 'running_sum' '(' expr ')' 'over' ident 'order' 'by' expr ;
quantified   = ('all' | 'any') '(' set-expr ':' expr ')' ;
set-expr     = source ident? ('where' expr)? ;
source       = Type-ident | path | call ;
call         = ident ('.' ident)* '(' args? ')' ;
call-target  = ident ('.' ident)* ('(' args? ')')? ;       (* via 뒤: 인자 없으면 괄호 생략 *)
args         = arg (',' arg)* ;
arg          = (ident ':')? expr
             | set-expr ':' expr ;                           (* binder 인자: same(items x: x.club) *)
path         = ident ('.' ident)* ;
list         = '[' (expr (',' expr)*)? ']' ;
range-lit    = '[' expr ',' expr ')' ;                       (* 반열린 구간만 *)
literal      = int | decimal | string | duration | time-of-day | 'true' | 'false' | 'null' ;
```

## 9. 이벤트, 시간, 규칙

```ebnf
event        = 'event' Type-ident ('v' int)? '{' (ident ':' type)* '}' ;
upcast       = 'upcast' Type-ident 'v' int '->' 'v' int 'with' field-assigns ;
on-event     = cross-tenant? 'on' Type-ident ident ('when' expr)? (block-do | notify) ;
block-do     = 'do' block ;

schedule     = cross-tenant? 'schedule' Type-ident when-spec ('catch' 'up' ('once' | 'skip'))? '{'
                 ('for' set-expr)? (stmt | notify)* '}' ;
when-spec    = 'every' ('day' | 'week' 'on' weekday | 'month' 'on' 'day' int | duration)
               ('at' time-of-day)? ('tz' string)? ;
weekday      = 'mon' | 'tue' | 'wed' | 'thu' | 'fri' | 'sat' | 'sun' ;

retain       = 'retain' Type-ident 'for' duration 'after' path
               'then' ('purge' | 'anonymize')
               ('notify' path duration 'before' 'via' call-target)? ;

rule         = cross-tenant? 'rule' Type-ident 'on' Type-ident ident 'when' expr 'do' block ;
(* 같은 트랜잭션 안, 커밋 직전에 평가한다. 조건이 거짓에서 참이 되는 순간에만 본문을 실행하고(에지),
   거짓이 되면 다시 무장한다. 조건은 행 자신, 행의 직접 컬렉션(e.items), 행을 참조하는 집합
   (Order o where o.product = p)만 읽을 수 있다. 연쇄는 16라운드 안에 끝나야 한다. *)
projection   = 'projection' Type-ident 'from' 'events' '[' Type-ident (',' Type-ident)* ']' 'key' ident '{'
                 ('on' Type-ident ident ':' stmt)+ '}' ;
search       = 'search' Type-ident 'on' Type-ident 'fields' '[' search-field (',' search-field)* ']'
               ('language' ident)? ;
search-field = ident ('weight' ('A' | 'B' | 'C' | 'D'))? ;
(* search X on E fields [f weight A, g weight B] language L: E의 텍스트 필드(Text, RichText, Email, Url, Phone)를 가중치 A~D로 색인한다
   (가중치를 생략하면 D, 같은 필드는 한 번, 비워 둘 수 없다). `from X.match(q) r`는 q를 웹 검색창 문법으로 읽어(단어는 AND, `or`, "구절", `-제외`)
   일치하는 행을 일치도 순으로, 동률이면 id 순으로 돌려준다. `r.rank`는 엔티티 필드가 아니라 그 검색 결과의 일치도(Decimal, 소수 6자리, 클수록 좋음)이고
   `from X.match(...)` 쿼리의 select, where, sort 안에서만 쓴다(엔티티에 `rank` 필드가 있어도 `r.rank`는 일치도다). 정렬을 쓰지 않으면 `rank desc, id`.
   페이지는 일반 쿼리와 같다(`page N by keyset`은 일치도와 id를 커서에 담아 페이지 사이에 행이 추가돼도 중복과 누락이 없고, offset은 그대로 밀린다).
   `visible to`, 소프트 삭제, 테넌트 필터는 그대로 적용되므로 보이지 않는 행과 다른 테넌트의 행은 결과에도, `has_more`에도 섞이지 않는다.
   테넌트 범위 엔티티의 검색 쿼리는 일반 쿼리처럼 테넌트를 정하는 파라미터가 필요하다(AIP-E314).
   검색어에서 단어가 하나도 안 나오는 질의(구두점만, 불용어만)는 오류가 아니라 빈 결과다. 길이 0은 파라미터 타입(`Text(1..N)`)이 거부한다.
   language: english 외 PostgreSQL이 어간 처리를 가진 언어(german, french, ...)는 어간 검색, 생략하면 어간 없이 단어 그대로.
   `korean`은 형태소 분석이 없어서(OI-08) 단어의 앞부분 일치로 근사한다: 질의 단어가 문서 단어의 접두면 일치하므로 조사와 어미가 붙은 단어는 찾지만,
   어간이 바뀐 형태, 합성어의 중간, 단어 중간 일치는 못 찾는다. 컴파일러가 AIP-W603을 경고하고 계약의 query guarantees.search에도 드러낸다.
   `publishable` 엔티티의 검색은 거부한다(AIP-E316). 색인은 엔티티 테이블의 생성 컬럼(tsvector)과 GIN 인덱스다. *)
job          = 'job' Type-ident '(' params ')' '{' allow ('progress' 'over' set-expr)?
                 ('produce' ident 'to' ident 'bucket' string ('expires' duration)?)?
                 do? ('notify' expr 'via' call-target)? '}' ;
(* job X: 시작 명령 X(params)와 상태 조회 XStatus(job)를 만든다. progress 집합은 시작 시점에 스냅숏되고
   100건 단위 배치로 커밋된다. do 블록은 항목마다 실행되며(최소 1회), 스냅숏 이후 조건에서 벗어난 항목은 건너뛴다.
   produce는 요청자에게 보이는 행만 csv로 만든다(UTF-8 BOM, 시각은 ISO 8601 UTC). *)
```

## 10. 도메인 형식 (L3)

```ebnf
verification = 'verification' Type-ident '{'
                 'subject' ':' Type-ident
                 'target' ':' type ('where' expr)?
                 'code' ':' ('digits' | 'alnum') int 'ttl' duration 'attempts' int ('resend' 'after' duration)?
                 'deliver' 'via' call-target
                 'on' 'verified' '(' ident ',' ident ')' 'do' block '}' ;
grant-link   = 'grant' 'link' Type-ident '{'
                 ('grants' ':' expr 'as' Type-ident)?          (* 관계 부여. 없으면 행위만 허용 *)
                 'scope' ':' params
                 'issued' 'by' expr
                 ('to' expr)?                                  (* 특정 대상 전용 *)
                 'expires' duration ('uses' int)?
                 ('redeem' 'with' '(' params ')')?             (* 사용자가 링크를 쓸 때 주는 입력 (OI-16) *)
                 require*
                 ('on' 'redeem' 'do' block)? '}' ;
(* grant link 본문에서 'holder'는 링크를 사용하는 actor에 바인딩된다. grants와 on redeem 중 최소 하나는 필수 *)
approval     = 'approval' Type-ident 'for' Type-ident ident '{'
                 'approvers' ':' set-expr                      (* actor 행이거나 actor 참조 필드가 정확히 하나인 행 *)
                 ('requested' 'by' expr)?                      (* 없으면 행을 볼 수 있는 인증 사용자 누구나 *)
                 'require' int 'approvals' (',' 'no' 'self' 'approval')?
                 'on' 'approved' 'do' block
                 'on' 'rejected' 'do' block
                 ('expires' duration)? '}' ;
(* approval X for E r: RequestX(e), ApproveX(e, comment?), RejectX(e, reason?), CancelX(e), XStatus(e)를 만든다.
   대상당 대기 중 요청은 하나. N번째 승인이 on approved를, 반려 한 번이 on rejected를 실행한다.
   만료된 요청에 대한 투표는 APPROVAL_EXPIRED로 실패하고 만료 처리는 커밋된다. *)
expose       = 'expose' Type-ident '{' expose-op+ '}' ;
expose-op    = ('read' ':' ('visible' | expr))
             | ('create' | 'update') ':' expr 'fields' '[' ident (',' ident)* ']'
             | 'delete' ':' expr ;
outbound-webhooks = 'outbound' 'webhooks' 'for' Type-ident ident '{'
                 'events' '[' Type-ident (',' Type-ident)* ']' ('where' expr)?
                 'sign' ident 'retry' int 'over' duration ('disable' 'after' duration 'failing')? '}' ;
(* outbound webhooks for E e { events [A, B] where cond sign hmac_sha256 retry N over D disable after D2 failing }:
   E의 행이 사용자가 등록한 엔드포인트다. E에는 `url: Url` 필드가 있어야 하고(AIP-E317), 목록의 이벤트가 디스패치될 때 cond(`e`는 엔드포인트 행, `event`는 이벤트)가
   참인 엔드포인트마다 전달 행을 만든다. E가 테넌트 범위면 이벤트가 가리키는 테넌트의 엔드포인트만 받는다(cond가 테넌트를 빠뜨려도 다른 테넌트로 가지 않고,
   이벤트가 테넌트 범위 행을 가리키지 않으면 AIP-E314). 전달은 outbox 뒤에서 비동기로 일어나며 커밋과 무관하다.
   요청: POST, 본문 {"id": "evt_<n>", "type": 이벤트, "created_at", "data": 이벤트 필드(행은 id)}. 헤더 `AIP-Signature: t=<unix 초>,v1=<hex>`는 inbound
   `payments.stripe.webhook`과 같은 방식이다(HMAC-SHA256, 서명 대상은 "<t>.<본문>", 키는 엔드포인트 비밀). t는 시도마다 새로 찍으므로 수신자가 5분보다 오래된 서명을
   거부하면 재전송 공격이 막힌다. `AIP-Event-Id`는 같은 이벤트의 모든 시도와 모든 엔드포인트에서 같다(수신자 dedupe 용). 그 밖에 `AIP-Event`, `AIP-Delivery-Id`,
   `AIP-Delivery-Attempt`.
   보장: 최소 한 번(2xx를 못 받으면 반복한다, 중복 가능), 순서 없음(엔드포인트별로도 없다. 재시도된 이벤트는 뒤 이벤트보다 늦게 도착한다).
   재시도: 첫 시도 뒤 N번, 대기는 매번 두 배, 마지막 재시도가 이벤트로부터 정확히 D 뒤다. 4xx, 5xx, 시간 초과, 연결 오류 모두 재시도하고 서버가 부를 수 없는 URL은 재시도하지 않는다.
   비활성: 마지막 성공 이후 `disable after` 동안 실패만 했으면 그 엔드포인트는 비활성이 되어(이유와 시각이 `_aip_outbound_endpoint`에 남는다) 이후 이벤트를 받지 않고,
   대기 중이던 전달은 취소된다. 성공하면 실패 기간이 지워진다. 다시 켜는 방법은 아직 없다(새로 등록).
   비밀: `whsec_` + HMAC-SHA256(서버 비밀 AIP_SECRET, "aip-outbound-webhook:<엔티티>:<엔드포인트 id>")의 hex. 어디에도 저장하지 않고 엔드포인트를 만든 command의
   `returns endpoint { secret: endpoint.signingSecret }`에서 한 번 보여 준다. `signingSecret`은 그 command가 `insert E {...} as endpoint`로 만든 행에서만,
   `returns`의 select에서만 읽을 수 있고 command는 `idempotent`일 수 없다(저장된 응답에 비밀이 남는다. AIP-E208). AIP_SECRET을 바꾸면 모든 비밀이 바뀐다.
   URL: http와 https만, 자격 증명 없음. 서버는 전달 직전에 호스트를 해석해 나온 주소가 모두 공개 주소여야 하고(루프백, 사설, 링크로컬, 공유 주소 대역, 예약, 멀티캐스트,
   IPv4 매핑 IPv6 거부) 확인한 그 주소로 연결한다(DNS 리바인딩 방지). 리다이렉트는 따르지 않고 프록시를 쓰지 않는다. 사설 주소는 `aip run --allow-private-webhook-targets`
   (개발과 테스트 전용)로만 허용한다. 엔드포인트 테이블에는 등록 때 scheme을 거르는 CHECK도 걸린다(AIP.INPUT.INVALID, WEBHOOK_URL_INVALID). *)
consent      = 'consent' Type-ident 'version' int 'required' 'for' '[' Type-ident (',' Type-ident)* ']' ;
(* consent X version N required for [A, B]: 나열된 command, query, job 시작 intent는 호출자가 X의 현재 버전(N)에 동의한 기록이
   있어야 실행된다. 없으면 AIP.CONSENT.REQUIRED(HTTP 403, reason은 X). 인증 없는 호출은 AIP.AUTH.UNAUTHENTICATED다.
   GiveXConsent()는 현재 버전을 기록하고(이미 있으면 그대로), WithdrawXConsent()는 호출자의 활성 기록을 모두 철회하며,
   XConsentStatus()는 {name, version, status(NONE|GIVEN|OUTDATED|WITHDRAWN), givenVersion, givenAt, withdrawnAt}를 돌려준다.
   기록은 내부 테이블 _aip_consent에 쌓이고(누가, 어느 버전, 언제, 철회 시각) 지워지지 않는다. 프로그램의 버전을 올리면 이전 기록은
   그대로 남고 더는 인정되지 않는다. 동의는 본인의 행위라서 대리 중(impersonate)에는 Give와 Withdraw가 거부된다.
   대상은 호출할 수 있는 command, query, job 이름이어야 한다(internal이나 폼이 만든 이름이면 AIP-E208). 버전은 1 이상(AIP-E210). *)
config       = 'config' ident ':' type ('=' expr)? ;
flag         = 'flag' ident 'default' ('on' | 'off') ('rollout' int '%' 'by' 'actor')? ;
impersonate  = 'impersonate' Type-ident 'by' expr 'audited' 'reason' 'required' 'ttl' duration ;
(* impersonate E by cond ... ttl d: cond(actor만 읽는다)를 만족하는 호출자가 E의 한 행으로 d 동안 대리 실행한다. E는 actor 엔티티여야 하고
   (AIP-E208) 선언은 하나뿐이다(AIP-E101). 만든 intent: StartEImpersonation(target, reason)은 세션을 열어 {session, token, target, expiresAt}를
   돌려준다. token은 HTTP 계층이 서명한다(`aip1.<b64 target.exp.session>.<서명>`, actor는 target). StopEImpersonation()은 세션 안에서는 그 세션을,
   운영자 본인 토큰으로는 열려 있는 자기 세션 전부를 끝내고 {ended}를 돌려준다. 두 intent 모두 audited다.
   세션이 열려 있는 동안(끝나지 않았고 ttl 안이고 target이 그 actor일 때만) 호출은 target으로 실행되고, 모든 command는 _aip_audit에
   actor=target, impersonated_by=운영자, impersonation=세션 으로 남는다(이유는 _aip_impersonation.reason).
   거부 규칙: 세션 안에서 시작(AUTH.FORBIDDEN/IMPERSONATION_NESTED), 자기 자신(IMPERSONATION_SELF), cond나 superuser를 만족하는 target
   (IMPERSONATION_TARGET_PRIVILEGED, 세션이 운영자보다 큰 힘을 빌려주지 못하게). 세션 안에서는 superuser 우회가 꺼지고,
   erase와 export personal data를 쓰는 command, 동의 Give/Withdraw, approval 투표가 IMPERSONATION_FORBIDDEN으로 거부된다. *)
migration    = 'migration' ident block ;
(* migration X { ... }: 데이터 마이그레이션이다. 스키마가 맞춰진 뒤(`aip migrate`와 `aip run`이 같은 경로를 지난다) 아직 실행되지 않은 migration을 선언 순서대로
   각각 한 트랜잭션에서 실행하고 `_aip_migration`(이름, 본문 digest, 시각)에 같은 트랜잭션으로 기록한다. 이미 기록된 이름은 다시 실행하지 않고,
   기록된 digest와 지금 본문의 digest가 다르면 아무것도 실행하기 전에 AIP.MIGRATION.CHANGED로 기동이 실패한다(적용된 마이그레이션은 바꿀 수 없다.
   바꾸려면 새 migration을 더한다). 문장이 실패하면 그 migration은 롤백되고 기록되지 않으며 AIP.MIGRATION.FAILED로 기동이 중단된다(뒤 migration은 실행하지 않는다.
   고친 뒤 다시 시작하면 그 migration부터 실행한다). 여러 프로세스가 함께 기동해도 advisory lock으로 한 번만 실행된다(기다린 쪽은 기록을 보고 건너뛴다).
   본문은 schedule 본문과 같은 문장들이고 actor가 없다. 테넌트는 `cross tenant`로 취급한다: 배포 한 번이 모든 워크스페이스의 행을 고치는 일이 마이그레이션이므로
   트랜잭션의 테넌트 고정을 풀고(AIP-E314 검사도 하지 않는다), 한 트랜잭션이 여러 테넌트의 행을 쓸 수 있다. 테넌트를 가진 엔티티 사이의 참조 트리거는 그대로다.
   `--reset`은 `_aip_migration`도 지우므로 처음부터 다시 실행된다. 선언에서 지운 migration의 기록은 남아 있어도 오류가 아니다.
   digest는 의미 IR로 내린 본문의 해시라서 공백과 주석을 고쳐도 바뀌지 않는다. *)
```

## 부록 A. 예약어

```
actor after all allow authenticated failure redeem snapshot superuser and anonymize any approval approvals approvers as asc at attempts audited avg
batch before by cached capacity cascade catch code config consent consistency consume count counter
create created cross day decrement default delete deliver depth desc digest digits disable do dynamic
each else encrypted entity enum erase event every exactly exists expires export expose fetch
fields flag fn for format from grant grants group has history idempotent if impersonate in insert
internal into invariant is issued job keyset key language latest least let first lifecycle limit link
masked max migration min month most no not notify now null of offset on once one or order ordered
outbound over overlap page partial per personal plan position predicate public preferences produce
progress projection publishable purge query reason record relation release repair require
required reserve resend respecting restrict retain retry returns rollout rule run running_sum
schema scope scopes search select self sequence set sign skip slug soft sort strong subject
subscribe sum target tenant the then this to today toggle topic touch track tree true false ttl tz
unique unless update updated upcast upsert use uses v validated variants verification verified
versioned via visible webhook webhooks week weight when where with within
```

(예약어 목록은 파서 구현 시 문맥 키워드와 전역 키워드로 나눈다. 필드 이름으로 흔히 쓰이는 `order`, `key`, `target`, `code`, `scope`, `status` 같은 단어는 문맥 키워드로만 둔다.)
