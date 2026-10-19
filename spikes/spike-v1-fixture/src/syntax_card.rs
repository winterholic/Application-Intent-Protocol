//! The authoring card of the product definition language: every block, the keys it accepts, what each key is for,
//! the defaults the language never assumes, and two worked definitions. It exists so that a person or an AI can write
//! one resource by reading about 13 KB (4 KB of it the two worked definitions) instead of the 24 KB syntax proposal and the V1 to V15 result documents.
//!
//! The keys come from `codes::BLOCK_KEYS`, which `tests/diagnostics.rs` keeps equal to the parser; the notes below must
//! cover exactly those keys (checked both ways). The sentences about defaults are each pinned by a test that loads a
//! definition and shows the behavior. The card is generated into `spec/definition-syntax.md`.

use crate::codes::BLOCK_KEYS;

/// `(block, key, shape, meaning)`.
pub const NOTES: &[(&str, &str, &str, &str)] = &[
    ("선언", "enum", "enum Name { A, B }", "닫힌 값 집합. 값은 따옴표 없이 쓰고 DB에는 text CHECK로 저장된다"),
    ("선언", "actor", "actor Resource", "호출자 행을 가리키는 resource. 조건식의 actor는 그 행이다. 정확히 하나"),
    ("선언", "predicate", "predicate name(p: Type, ...) = condition", "이름 붙인 재사용 조건. name(args)로 부르고 서로 순환 호출할 수 없다"),
    (
        "선언",
        "access",
        "access name = totalOfVisible(R)  |  access name(a: Actor, c: X.Id) = condition",
        "집계·확장 결과를 누구에게 줄지 정하는 guard",
    ),
    ("선언", "limit", "limit name on R = atMost N where condition", "조건에 맞는 행 수의 상한. resource가 invariant로 per 필드마다 건다"),
    ("선언", "resource", "resource Name { ... }", "테이블 하나와 그 테이블을 여는 방식"),
    (
        "선언",
        "operation",
        "operation read|job name { input; output; allow; effect; deadline; implementation }",
        "resource 없는 연산. 확장 구현을 부른다",
    ),
    (
        "resource",
        "fields",
        "fields { id: Id; name: Text(1..100); owner: Member? }",
        "열. id: Uuid는 UUID 기본키, id: Member는 Member.id를 공유하는 1:1 기본키/외래 키다. 공유 기본키는 참조 대상의 UUID/숫자 identity와 같은 wire 형식을 쓰며 null일 수 없다. Json은 JSON 값을 읽기 위한 타입이다. 외부 schema의 기본값 필드는 타입 뒤 default database로 선언한다(읽기 nullable은 유지, create 입력은 생략 가능). ?는 null 허용. owner: Member는 외래 키가 있는 참조(조인·traverse 가능). fields 안에서 `X.Id`는 쓸 수 없고 오류다(DEF.ID_FIELD_NEEDS_REFERENCE). 참조 `X`를 쓴다. resource마다 한 번",
    ),
    ("resource", "rows", "rows read when condition", "호출자가 읽을 수 있는 행. 쓰지 않으면 읽을 수 있는 행이 없다"),
    (
        "resource",
        "field",
        "field name read when condition",
        "필드별 읽기 조건. 조건이 거짓인 행에서 select하면 null이고 출력 타입은 nullable이다(열이 필수여도). 이 필드는 filter·sort·집계에 쓸 수 없다",
    ),
    (
        "resource",
        "expose",
        "expose read {..} | expose apply T {..} | expose create {..} | expose compose {..} | expose aggregate name",
        "호출자에게 여는 것. 쓰지 않은 것은 열리지 않는다",
    ),
    ("resource", "aggregate", "aggregate name: Int { source R; sourceAccess a; groupKey ref; release count }", "행별 또는 단독 집계"),
    (
        "resource",
        "transition",
        "transition name { from cond; to f = v; allow cond }  |  transition name { from cond; delete; allow cond }",
        "상태 전이. from·allow와 to, patch 또는 delete가 필요하다. 숫자는 f = f + 1처럼만 바꾼다",
    ),
    ("resource", "unique", "unique a, b", "유일 제약"),
    (
        "transition",
        "patch",
        "patch title, content",
        "호출자의 values에서 수정할 수 있는 필드. 타입·범위는 fields와 같으며 누락은 유지, null은 nullable만. 기본키·to 대입과 중복 불가",
    ),
    (
        "transition",
        "expect",
        "expect version",
        "호출자의 expect에서 모두 제공해야 하는 이전 값. 인가 뒤 잠근 행과 NULL-safe 비교하며 불일치는 WIRE.CONFLICT. version 증감 to와 조합하면 낙관적 잠금",
    ),
    ("resource", "check", "check name when condition", "쓰기 뒤 행이 만족해야 하는 조건"),
    ("resource", "invariant", "invariant limitName per refField [deferred]", "limit을 참조 필드별로 적용한다(유일 인덱스로 집행)"),
    (
        "resource",
        "extension",
        "extension read|write|job name { input; output; access; effect; deadline; implementation }",
        "표준 밖 Node·Python 확장 호출",
    ),
    ("resource", "docs", "docs { summary \"...\"; visibility internal }", "사람이 읽는 설명. 실행·권한에 쓰이지 않는다"),
    ("expose read", "select", "select a, b", "읽을 수 있는 필드·집계 이름"),
    (
        "expose read",
        "refIds",
        "refIds a, b",
        "select한 참조 필드 가운데 원시 외래 키 id를 대상 행의 가시성과 무관하게 공개할 것. 쓰지 않은 참조 select는 대상 resource의 `rows read`를 적용해 호출자가 대상 행을 볼 수 없으면 null이 된다(출력 타입도 nullable)",
    ),
    (
        "expose read",
        "filter",
        "filter f.op, g.op",
        "필터 허용 목록. contains는 리터럴 부분일치이며 MySQL은 column collation, PostgreSQL은 대소문자 구분 비교를 쓴다. like는 SQL LIKE 와일드카드(%·_)를 쓰며 MySQL은 column collation, PostgreSQL은 DB locale에 따른 ILIKE(대소문자 무시, 악센트·문자 폭은 구분)로 비교한다. 연산 eq in isNull gte lte gt lt prefix contains like 중 필드 타입이 지원하는 것",
    ),
    ("expose read", "sort", "sort a, b", "정렬 허용 목록"),
    (
        "expose read",
        "traverse",
        "traverse rel { select a }  |  traverse Child via field { select a; limit N }",
        "참조를 따라 포함한다. 대상 resource의 expose select 안에서만",
    ),
    ("expose read", "budget", "budget { rows N; depth N; deadline 2s; cost N }", "이 읽기를 루트로 부를 수 있게 하는 한도. 네 값이 모두 필요하다"),
    ("budget", "rows", "rows N", "한 번에 돌려줄 최대 행 수"),
    ("budget", "depth", "depth N", "관계 깊이. 루트가 1이고 traverse마다 1 늘어난다"),
    ("budget", "deadline", "deadline 2s", "시간 한도. ms, s, m 단위"),
    ("budget", "cost", "cost N", "추정 비용 한도"),
    ("budget", "offset", "offset N", "offset 페이징을 허용하고 그 상한을 둔다(선택)"),
    ("budget", "cursor", "cursor", "cursor 페이징을 허용한다(선택). id와 sort 필드가 select에 있어야 한다"),
    ("traverse", "select", "select a, b", "따라간 행에서 읽을 필드"),
    ("traverse", "sort", "sort a", "1:N traverse의 정렬 필드 하나"),
    ("traverse", "limit", "limit N", "1:N traverse가 돌려줄 최대 행(1~1000)"),
    ("aggregate", "input", "input { x: Type }", "호출자가 넘기는 값"),
    ("aggregate", "source", "source R", "집계할 행의 resource. 쓰지 않으면 자기 resource"),
    (
        "aggregate",
        "sourceAccess",
        "sourceAccess name  |  name(args)",
        "원본 행을 읽는 근거가 되는 access. 반드시 쓴다(원본 행 정책을 물려받지 않는다)",
    ),
    ("aggregate", "groupKey", "groupKey ref", "원본에서 이 resource를 가리키는 참조 필드. 있으면 행별 집계"),
    ("aggregate", "where", "where condition", "집계에 넣을 원본 행 조건"),
    (
        "aggregate",
        "callerFilter",
        "callerFilter none | read",
        "호출자의 aggregate filter 허용 여부. none은 거부하고 read는 요청 필터를 허용한다. 단독 집계는 source의 rows read 정책을 물려받지 않으므로, rows read 정책이 있는 source에서 callerFilter read를 쓰면 aggregate where에 같은 행 정책을 직접 적는다. 생략하면 숨긴 행의 존재를 필터로 셀 수 있어 정의 검사에서 경고한다",
    ),
    ("aggregate", "rowOutput", "rowOutput none", "집계가 행을 내보내는지"),
    ("aggregate", "release", "release count  |  sum(f)  |  min(f)  |  max(f)", "내보내는 값"),
    ("extension", "input", "input { x: Type }", "확장이 받는 값"),
    ("extension", "output", "output { y: Type }", "확장이 돌려주는 값"),
    ("extension", "access", "access R.transition  |  R.aggregate", "확장이 쓸 수 있는 전이·집계"),
    ("extension", "effect", "effect none | db | managed", "read는 none, write는 db"),
    ("extension", "deadline", "deadline 2s", "실행 시간 한도"),
    ("extension", "implementation", "implementation \"module.function\"", "확장 모듈과 함수 이름"),
    ("transition", "from", "from condition", "전이를 시작할 수 있는 행의 조건"),
    ("transition", "to", "to f = v; g = g + 1", "바뀌는 값"),
    ("transition", "delete", "delete", "대상 행 물리 삭제. to·patch·expect·repeat unchanged와 함께 쓸 수 없다"),
    ("transition", "allow", "allow condition", "누가 부를 수 있는지"),
    ("transition", "repeat", "repeat unchanged | reject", "이미 to 상태인 행에 다시 부를 때의 처리"),
    ("transition effect", "create", "create R { f = v }", "같은 트랜잭션에서 행을 만든다"),
    (
        "transition effect",
        "update",
        "update R where f = v { g = w }  |  update many R maxRows N where f = v { g = w }",
        "같은 트랜잭션에서 다른 행을 바꾼다. many는 maxRows(1~1000)와 부모 참조 조건이 필요하다",
    ),
    (
        "transition effect",
        "delete",
        "delete R where f = v  |  delete many R maxRows N where f = v",
        "같은 트랜잭션에서 다른 행을 물리 삭제한다. many는 maxRows(1~1000)와 부모 참조 조건이 필요하다",
    ),
    ("transition effect", "notify", "notify actorRef \"topic\"", "알림. 대상은 actor 타입 참조여야 한다"),
    ("expose apply", "target", "target id, where", "전이를 적용할 행을 id로 또는 허용한 필터로 고른다"),
    ("expose apply", "bulk", "bulk maxRows N", "한 번에 바꿀 최대 행 수. 무제한 기본값이 없다"),
    ("expose apply", "sameScope", "sameScope field", "대상 행이 모두 같은 값(예: 같은 동아리)이어야 한다"),
    ("expose create", "allow", "allow condition", "새 행이 만족해야 하는 조건"),
    ("expose create", "fields", "fields a, b", "호출자가 값을 줄 수 있는 필드"),
    ("expose compose", "bulk", "bulk maxRows N", "최대 대상 수"),
    ("expose compose", "sameScope", "sameScope field", "대상이 같은 범위"),
    ("expose compose", "transitions", "transitions a, b", "허용하는 전이 단계"),
    ("expose compose", "selfRow", "selfRow actorRef by scopeField", "대상과 같은 범위의 요청자 자신의 행"),
    ("expose compose", "create", "create R from source, ...", "값을 서버가 읽은 행에서 가져와 만드는 행"),
    ("docs", "summary", "summary \"...\"", "한 줄 설명"),
    ("docs", "visibility", "visibility internal", "문서 노출 범위 표시"),
];

/// The notes of a block in the order of `BLOCK_KEYS`.
fn notes_of(block: &str, key: &str) -> Option<&'static (&'static str, &'static str, &'static str, &'static str)> {
    NOTES.iter().find(|(b, k, _, _)| *b == block && *k == key)
}

/// `spec/definition-syntax.md`, generated; `tests/diagnostics.rs` keeps it equal to this rendering.
pub fn render_markdown() -> String {
    let mut out = String::from(
        "# 제품 정의 언어: 작성 카드\n\n\
         > 이 문서는 `spikes/spike-v1-fixture/src/syntax_card.rs`에서 생성된다. 손으로 고치지 않는다. 다시 만들려면 `AIP_UPDATE_GOLDEN=1 cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --test diagnostics`를 실행한다.\n\n\
         제품 경로(`aip service check/gen/serve`)가 읽는 정의 언어의 요약이다. PoC 언어(entity·command·query, 확장자 `.aipoc`, [grammar.md](grammar.md))와는 다른 언어이고 확장자도 다르다(`.aip`는 이 제품 정의 언어 전용이다). 정의는 `.aip` 텍스트, TS·Python 안의 `aip` 블록(`.e.ts`, `.e.py`), TS·Python 데이터 리터럴(`.h.ts`, `.h.py`) 다섯 형식 중 하나로 쓰고 모두 같은 계약이 된다. 키 목록은 파서와 같은 표에서 나오고, 오류 코드와 고치는 방법은 [definition-diagnostics.md](definition-diagnostics.md)에 있다.\n\n\
         ## 언어가 가정하지 않는 것\n\n\
         무제한 기본값과 암묵적 허용이 없다. 빠뜨리면 열리지 않거나 오류가 된다.\n\n\
         - `rows read`를 쓰지 않은 resource는 읽을 수 있는 행이 없다.\n\
         - `expose read`를 쓰지 않은 resource는 읽을 수 없다.\n\
         - `budget`이 없는 `expose read`는 루트로 읽을 수 없다. 다른 resource의 `traverse` 대상으로만 쓰인다.\n\
         - `budget`은 rows·depth·deadline·cost 네 값이 모두 있어야 한다.\n\
         - `expose apply`에는 `bulk maxRows`가 필요하다.\n\
         - `aggregate`에는 `sourceAccess`가 필요하다. 원본 행의 읽기 정책을 물려받지 않는다.\n\
         - 필드 읽기 정책(`field ... read when`)이 있는 필드는 filter·sort·집계에 쓸 수 없다.\n\
         - 필드 읽기 정책이 있는 필드는 전이의 `from`, `to`의 값 식, `repeat unchanged`가 걸린 `to` 대상에도 쓸 수 없다(`DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`). 결과로 가려진 값이 추론되기 때문이다. `allow`와 `repeat unchanged` 없는 `to` 쓰기는 가능하다.\n\
         - `unique`·`limit`·`invariant` 위반(`WIRE.ALREADY_EXISTS`, `WIRE.INVARIANT_VIOLATED`)은 호출자가 볼 수 없는 행과 충돌해도 같은 코드로 실패한다. 존재는 알려도 오류 본문에 그 행의 id나 값은 싣지 않는다.\n\
         - `fields` 안에서 다른 resource를 가리키는 타입은 참조 `X` 하나뿐이다(외래 키가 생긴다). `X.Id`는 `input`·`access` 인자·operation 입출력처럼 DB 열이 아닌 자리에서만 쓰며, `fields` 안에서는 정의 오류(`DEF.ID_FIELD_NEEDS_REFERENCE`)다. 기본 키는 숫자 `id: Id`(기본) 또는 UUID `id: Uuid`로 선언하며 `id: Item.Id`는 같은 오류다.\n\
         - 다른 행이 없음에 의존하는 `allow`·`from`(`not exists`, predicate 안과 중첩 포함)이나 그런 `check`가 걸린 전이·create는 서버가 SERIALIZABLE 트랜잭션으로 실행하고, 직렬화 실패·교착은 같은 요청을 서버가 최대 3회 다시 실행한다. 유일성·정원이 목적이면 `unique`나 `limit`과 `invariant`가 더 싸다.\n\
         - 알 수 없는 키는 무시하지 않고 오류다.\n\
         - 문법 오류는 첫 번째에서 멈춘다. 의미 오류는 모아서 보고하며 원인을 먼저 둔다.\n",
    );
    out.push_str("\n## 블록과 키\n");
    for (block, keys) in BLOCK_KEYS {
        out.push_str(&format!("\n### {block}\n\n| 키 | 형태 | 뜻 |\n|---|---|---|\n"));
        for key in *keys {
            let (shape, meaning) = notes_of(block, key).map_or(("", ""), |(_, _, s, m)| (*s, *m));
            out.push_str(&format!("| `{key}` | `{}` | {meaning} |\n", shape.replace('|', "\\|")));
        }
    }
    out.push_str(
        "\n## 타입 범위\n\n\
         `Int(lo..hi)`의 양 끝은 포함되며 음수 경계도 쓸 수 있다(예: `Int(-1000..1000)`, `Int(-1000..-1)`). \
         범위는 `lo <= hi`여야 한다. 부호는 Int 범위에서만 허용하며 Text 길이·budget·Decimal 자릿수에는 허용하지 않는다. \
         기존 정수 토큰의 크기 한도는 유지한다. operation 입출력 범위는 음수도 JavaScript 안전 정수 영역 안이어야 한다. \
         이 구문은 범위 선언이며 식의 음수 리터럴 문법을 추가하지 않는다.\n",
    );
    out.push_str("\n## 작성 예 1: 읽기, 정책, 집계, 관계\n\n```aip\n");
    out.push_str(include_str!("../fixture/recruitment.aip"));
    out.push_str("```\n\n## 작성 예 2: 쓰기, 연산, 확장\n\n```aip\n");
    out.push_str(include_str!("../../../prototype/example/app.aip"));
    out.push_str("```\n");
    out
}
