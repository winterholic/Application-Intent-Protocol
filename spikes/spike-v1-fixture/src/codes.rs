//! Registry of the definition diagnostics of the product path (`aip service check`).
//!
//! The compiler reports `code`, a message and a position. What a code means, how to repair it and a
//! definition that shows the repaired form live here so that a caller (a person or an AI) can act on
//! the output without reading the compiler. `aip explain-code <CODE>` prints an entry, and
//! `Diag::json` points to it. A published code keeps its meaning; `tests/diagnostics.rs` checks that
//! every code the sources can emit is registered and that every example is valid.

/// One registered diagnostic code.
#[derive(Debug, Clone, Copy)]
pub struct Info {
    pub code: &'static str,
    /// One line, what the code says.
    pub title: &'static str,
    /// Why it is rejected and when it occurs.
    pub explain: &'static str,
    /// How to repair it.
    pub fix: &'static str,
    /// A definition (`.aip` text) that is accepted and shows the repaired form; empty when the fix
    /// is not a change of the definition text.
    pub example: &'static str,
}

const fn i(code: &'static str, title: &'static str, explain: &'static str, fix: &'static str, example: &'static str) -> Info {
    Info { code, title, explain, fix, example }
}

/// A minimal valid base used by examples that only need to show one construct.
macro_rules! base {
    () => {
        "actor Member\nresource Member { fields { id: Id; name: Text } }\n"
    };
}

pub fn lookup(code: &str) -> Option<&'static Info> {
    REGISTRY.iter().find(|c| c.code == code)
}

pub fn all() -> &'static [Info] {
    REGISTRY
}

/// Plain text for `aip explain-code`.
pub fn render_text(c: &Info) -> String {
    let mut out = format!("{}  definition diagnostic  {}\n\nwhy:\n  {}\n\nfix:\n  {}\n", c.code, c.title, c.explain, c.fix);
    if !c.example.is_empty() {
        out.push_str("\nexample (an accepted definition in the .aip form):\n");
        for l in c.example.lines() {
            out.push_str(&format!("  {l}\n"));
        }
    }
    out
}

/// `spec/definition-diagnostics.md`, generated; `tests/diagnostics.rs` keeps it equal to the registry.
pub fn render_markdown() -> String {
    let mut out = String::from(
        "# 제품 정의 진단 코드\n\n\
         > 이 문서는 `spikes/spike-v1-fixture/src/codes.rs`의 레지스트리에서 생성된다. 손으로 고치지 않는다. 다시 만들려면 `AIP_UPDATE_GOLDEN=1 cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --test diagnostics`를 실행한다.\n\n\
         `aip service check`(와 정의를 읽는 모든 제품 명령)가 `DEF.INVALID_DEFINITION`과 함께 돌려주는 진단의 코드다. 각 진단은 `code`, `message`, `line`, `column`에 더해 `scope`(위치한 선언 경로), `help`(가까운 철자, 블록이 받는 키, 고치는 방법), `explain`(`aip explain-code <CODE>`)을 가진다. 메시지 문장은 계약이 아니고 코드가 계약이다. 한 번 공개된 코드는 의미를 바꾸지 않는다.\n\n\
         모든 AIP 오류 코드는 `영역.이름` 형식이다. 이 문서의 영역은 `DEF`(제품 정의 검사기)이고, 제품 HTTP API의 `WIRE` 코드는 [wire-errors.md](wire-errors.md), PoC 언어와 런타임의 `POC` 코드는 [diagnostics.md](diagnostics.md)에 있다. 세 영역은 `aip explain-code`와 `aip diagnostics`로 함께 조회한다. 2026-10-13 이전에는 접두 없는 이름(`DEF.UNKNOWN_FIELD`)이었고 제품 wire 코드와 이름이 겹쳤다.\n",
    );
    for c in REGISTRY {
        out.push_str(&format!("\n### `{}`\n\n{}\n\n- 원인: {}\n- 수정: {}\n", c.code, c.title, c.explain, c.fix));
        if !c.example.is_empty() {
            out.push_str("\n```aip\n");
            out.push_str(c.example);
            out.push_str("```\n");
        }
    }
    out
}

/// Keys each block accepts, by the block name the parser uses in `DEF.PARSE_UNKNOWN_KEY`.
/// `tests/diagnostics.rs` checks every key against the parser source.
pub const BLOCK_KEYS: &[(&str, &[&str])] = &[
    ("선언", &["enum", "actor", "predicate", "access", "limit", "resource", "operation"]),
    ("resource", &["fields", "rows", "field", "expose", "aggregate", "transition", "unique", "check", "invariant", "extension", "docs"]),
    ("expose read", &["select", "refIds", "filter", "sort", "traverse", "budget"]),
    ("budget", &["rows", "depth", "deadline", "cost", "offset", "cursor"]),
    ("traverse", &["select", "sort", "limit"]),
    ("aggregate", &["input", "source", "sourceAccess", "groupKey", "where", "callerFilter", "rowOutput", "release"]),
    ("extension", &["input", "output", "access", "effect", "deadline", "implementation"]),
    ("transition", &["from", "to", "delete", "patch", "expect", "allow", "repeat"]),
    ("transition effect", &["create", "update", "delete", "notify"]),
    ("expose apply", &["target", "bulk", "sameScope"]),
    ("expose create", &["allow", "fields"]),
    ("expose compose", &["bulk", "sameScope", "transitions", "selfRow", "create"]),
    ("docs", &["summary", "visibility"]),
];

/// The allowed keys of a block, when the parser names the block in its message.
pub fn keys_of(block: &str) -> Option<&'static [&'static str]> {
    BLOCK_KEYS.iter().find(|(b, _)| *b == block).map(|(_, k)| *k)
}

pub static REGISTRY: &[Info] = &[
    // The envelope the service puts around diagnostics.
    i(
        "DEF.INVALID_DEFINITION",
        "the definition has errors",
        "`aip service check` and every command that loads a definition report this code with a `diagnostics` list; each entry has its own code, line, column and help. Nothing is generated or deployed from a definition with errors.",
        "Fix the diagnostics from the first one. Later entries are often consequences of an earlier one (see the `cascade` note in their help); check again after each repair.",
        "",
    ),
    // Source and file level.
    i(
        "DEF.SOURCE_TOO_LARGE",
        "the definition file is larger than the product limit",
        "A definition above the byte ceiling is rejected before it is read, so that checking cannot be used to exhaust memory.",
        "Split the application into resources of a smaller definition, or remove generated or commented-out text. The limits are listed in spikes/spike-v1-fixture/PRODUCTION-LIMITS.md.",
        "",
    ),
    i(
        "DEF.TOKEN_LIMIT",
        "the definition has more tokens than the product limit",
        "A definition with more tokens than the ceiling is rejected at the position of the first token over the limit.",
        "Remove repetition: share a condition with a `predicate`, a visibility rule with `access`. The limits are listed in spikes/spike-v1-fixture/PRODUCTION-LIMITS.md.",
        "",
    ),
    i(
        "DEF.POLICY_EXPANSION_LIMIT",
        "a policy path is longer than the expansion budget",
        "A row or field policy that follows references through too many fields is rejected, because the server expands it into one query.",
        "Shorten the path: put the condition on the nearer resource, or give the intermediate resource its own `predicate`.",
        "",
    ),
    // Lexer.
    i(
        "DEF.LEX_UNEXPECTED_CHAR",
        "a character that no token starts with",
        "The text contains a character the definition language does not use outside of a string.",
        "Remove the character, or put it inside a string literal (`\"...\"`). Comparison is `=` (not `==`); not-equal is `!=`.",
        "",
    ),
    i(
        "DEF.LEX_BAD_STRING",
        "a string literal is not closed or has an unsupported escape",
        "Strings are enclosed in double quotes on one line. Only the documented escapes are accepted.",
        "Close the string with `\"` on the same line and use only the supported escapes.",
        "",
    ),
    i(
        "DEF.LEX_NUMBER_RANGE",
        "an integer is outside the supported range",
        "Integers in a definition are 64-bit signed.",
        "Use a smaller number, or a `Decimal(p, s)` field when a value does not fit an Int.",
        "",
    ),
    i(
        "DEF.LEX_BAD_DURATION",
        "unknown unit in a duration",
        "A duration is an integer followed by one unit.",
        "Use `ms`, `s` or `m` after the integer, for example `250ms`, `2s`, `5m`.",
        concat!(
            base!(),
            "resource Note {\n  fields { id: Id; body: Text; owner: Member }\n  expose read { select id, body\n    budget { rows 10; depth 1; deadline 2s; cost 100 } }\n}\n"
        ),
    ),
    i(
        "DEF.LEX_DURATION_RANGE",
        "a duration does not fit in milliseconds",
        "Durations are stored as 64-bit milliseconds.",
        "Use a shorter duration; a request deadline above a few minutes is not useful.",
        "",
    ),
    // Parser.
    i(
        "DEF.PARSE_EXPECTED",
        "the parser expected another token here",
        "The text before this position is valid, but the token found cannot continue the declaration. The message names what was expected and what was found.",
        "Write the token the message names at the reported position. A syntax error stops the check, so errors further down appear only after this one is fixed.",
        concat!(base!(), "resource Note {\n  fields { id: Id; body: Text(1..500); owner: Member }\n}\n"),
    ),
    i(
        "DEF.PARSE_UNKNOWN_DECL",
        "a top-level word that starts no declaration",
        "A definition is a list of declarations. The first word of each must be one of the declaration keywords.",
        "Start the declaration with `enum`, `actor`, `predicate`, `access`, `limit`, `resource` or `operation`; check the spelling of the word reported. Every declaration is described in spec/definition-syntax.md.",
        "",
    ),
    i(
        "DEF.PARSE_UNKNOWN_KEY",
        "a key that the block does not accept",
        "Unknown keys are never ignored silently: a misspelled key would otherwise leave a policy or a limit out of the definition without a sign.",
        "Use one of the keys listed in the help. Every block and key is described in spec/definition-syntax.md.",
        concat!(
            base!(),
            "resource Note {\n  fields { id: Id; body: Text; owner: Member }\n  expose read { select id, body; sort id\n    budget { rows 10; depth 1; deadline 2s; cost 100 } }\n}\n"
        ),
    ),
    i(
        "DEF.PARSE_NESTING",
        "an expression is nested or large beyond the limit",
        "Expression depth and node count are bounded so that a definition cannot make the checker or the planner do unbounded work.",
        "Name the sub-condition with a `predicate` and call it, instead of nesting it.",
        "",
    ),
    // Host-language forms.
    i(
        "DEF.WRONG_BINDING",
        "the definition is not bound the official way in the host file",
        "In the embedded forms the block must be written with the official import and exactly one `aip` tagged template (TypeScript) or `aip(\"\"\"...\"\"\")` call (Python).",
        "Import the official module and write one block: `export const app = aip` followed by the template. The fixtures spikes/spike-v1-fixture/fixture/recruitment.e.ts and recruitment.e.py show the shape.",
        "",
    ),
    i(
        "DEF.NON_LITERAL",
        "the definition must be a literal in the host file",
        "A definition is checked statically, so it cannot be built, concatenated, called or generated at run time in the host language.",
        "Write the definition as one literal: no concatenation, function call, variable or conditional around the block.",
        "",
    ),
    i(
        "DEF.DYNAMIC_INTERPOLATION",
        "interpolation inside the definition",
        "`${...}` in a TypeScript template and f-strings in Python make the definition depend on run-time values.",
        "Remove the interpolation and write the value into the definition text.",
        "",
    ),
    i(
        "DEF.ESCAPE_UNSUPPORTED",
        "an escape sequence that the definition string does not support",
        "Host string escapes that would change what the checker reads are rejected.",
        "Write the character directly, or use a plain ASCII text.",
        "",
    ),
    i(
        "DEF.NO_BLOCK",
        "the host file has no definition block",
        "The host file was loaded as a definition but contains no `aip` block (embedded form) or `define(...)` call (host-literal form).",
        "Add one block, or load the correct file. Check the file name: `.e.ts`/`.e.py` are embedded forms, `.h.ts`/`.h.py` host-literal forms.",
        "",
    ),
    i(
        "DEF.MULTIPLE_BLOCKS",
        "more than one definition block in a file",
        "One file holds one definition, so there is exactly one contract per file.",
        "Keep one block per file; move the other resources into the same block.",
        "",
    ),
    i(
        "DEF.HOST_LEX",
        "the host file cannot be scanned",
        "The host-language text has an unclosed string or comment before the definition could be located.",
        "Close the string or comment reported.",
        "",
    ),
    i(
        "DEF.H_LITERAL_NESTING",
        "a host literal is nested beyond the limit",
        "The host-literal form is a data structure; its depth is bounded.",
        "Flatten the literal; move deep conditions into a `predicate`.",
        "",
    ),
    i(
        "DEF.H_SHAPE",
        "a host-literal value has the wrong shape",
        "In the host-literal form each key expects an object, string, integer, boolean or list. The message names the key and the shape expected.",
        "Replace the value with the shape the message names. The fixtures spikes/spike-v1-fixture/fixture/recruitment.h.ts and recruitment.h.py show every key.",
        "",
    ),
    i(
        "DEF.WRONG_LANGUAGE",
        "a PoC program was given to the product checker",
        "The product commands (`aip service check/gen/serve`) read definitions of the product language (`resource`, `expose`, `transition`) in `.aip` files. A file with the extension `.aipoc` is a PoC program (`entity`, `command`, `query`), which has its own checker; reading it here would only report typos in a file that has none.",
        "Check a PoC program with `aip check`; if the file is a product definition, rename it to `.aip`.",
        "",
    ),
    i(
        "DEF.UNKNOWN_KEY",
        "a key that the object does not accept (host-literal form)",
        "As in `DEF.PARSE_UNKNOWN_KEY`: unknown keys are never ignored.",
        "Use one of the keys listed in the help.",
        "",
    ),
    i(
        "DEF.MISSING_ITEM",
        "a required part is missing",
        "Some declarations have no safe default, so the definition must say the value: there is no unlimited bulk size, no implicit aggregate source access, no implicit actor. The message names the missing part.",
        "Add the part the message names.",
        concat!(
            base!(),
            "resource Note {\n  fields { id: Id; body: Text; owner: Member }\n  rows read when owner = actor\n  transition archive {\n    from body != \"\"\n    to body = \"\"\n    allow owner = actor\n  }\n  expose apply archive { target id; bulk maxRows 1 }\n}\n"
        ),
    ),
    i(
        "DEF.DUPLICATE",
        "a name or key is declared twice",
        "Names are unique in their block: declarations, fields, transitions, aggregates, invariants, parameters. A second declaration would make the first one unreachable.",
        "Rename or remove one of the two; the first declaration is the one the message does not point to.",
        "",
    ),
    i(
        "DEF.AMBIGUOUS",
        "a name collides with a managed capability",
        "Resource names that equal a managed area of the server (for example messages or objects) would make a route mean two things.",
        "Rename the resource.",
        "",
    ),
    i(
        "DEF.AMBIGUOUS_NAME",
        "a name reads as both a field and a variable",
        "Inside a predicate a parameter and a field of `this` may share a spelling; the checker will not choose for you.",
        "Rename the parameter, or qualify the field with `this.`.",
        "",
    ),
    // Names and types.
    i(
        "DEF.UNKNOWN_TYPE",
        "a type that is not declared",
        "A field or parameter uses a type that is neither built in nor an `enum` or `resource` of this definition. Errors that depend on this field (its enum values, its policies) usually follow and disappear when it is fixed.",
        "Spell the type as declared: `Id`, `Uuid` (primary identity), `Text`, `Int`, `Bool`, `Json`, `Time`, `Date`, `Url`, `Email`, `Decimal(p,s)`, an `enum` or `resource` name, `X.Id`, with `?` for nullable. Declare the enum or resource if it is missing.",
        concat!(base!(), "enum Status { OPEN, CLOSED }\nresource Ticket {\n  fields { id: Id; status: Status; reporter: Member? }\n}\n"),
    ),
    i(
        "DEF.UNKNOWN_SYMBOL",
        "a predicate, resource, enum value or declaration that is not declared",
        "The name is used but nothing with that name exists at this place. The message says what kind of thing was expected.",
        "Declare it, or use the spelling of the existing one (the help offers close names when there are any).",
        concat!(base!(), "enum Status { OPEN, CLOSED }\nresource Ticket {\n  fields { id: Id; status: Status }\n  rows read when status = OPEN\n}\n"),
    ),
    i(
        "DEF.UNRESOLVED_NAME",
        "a name that is neither a field, a variable nor an enum value",
        "In an expression a bare name must be a field of the row, a parameter, `actor`, `this`, `now`, `null`, or a value of the enum that the other side of the comparison has. If the type of the other side is itself unknown, this error follows from it.",
        "Fix an earlier DEF.UNKNOWN_TYPE or DEF.DUPLICATE first. Otherwise spell the field or enum value as declared.",
        "",
    ),
    i(
        "DEF.UNKNOWN_FIELD",
        "a field that the resource does not have",
        "A select, filter, sort, policy or effect names a field the resource does not declare.",
        "Use a declared field (the help offers close names when there are any) or declare the field in `fields`.",
        concat!(
            base!(),
            "resource Note {\n  fields { id: Id; body: Text; owner: Member }\n  expose read { select id, body\n    budget { rows 10; depth 1; deadline 2s; cost 100 } }\n}\n"
        ),
    ),
    i(
        "DEF.UNKNOWN_RESOURCE",
        "a resource that is not declared",
        "A traverse or reference names a resource that this definition does not declare.",
        "Declare the resource or spell its name as declared.",
        "",
    ),
    i(
        "DEF.UNKNOWN_OPERATOR",
        "a filter operator that does not exist",
        "Filters are `field.operator`; the operators are fixed.",
        "Use `eq`, `in`, `isNull`, `gte`, `lte`, `gt`, `lt`, `prefix`, `contains` or `like`, whichever the field type supports (DEF.OP_TYPE_MISMATCH tells which).",
        "",
    ),
    i(
        "DEF.OP_TYPE_MISMATCH",
        "the operator is not defined for the type of the field",
        "For example `gte` needs an ordered type (Time, Date, Int, Decimal), `prefix`, `contains` and `like` need Text, `isNull` needs a nullable field, and Json or reference fields cannot be sorted.",
        "Use an operator that fits the type, or filter by another field.",
        "",
    ),
    i(
        "DEF.TYPE_MISMATCH",
        "the types do not fit",
        "Both sides of a comparison, an assignment, a predicate argument or an effect value must have the same type; conditions must be Bool. The message shows both types.",
        "Change the value or the field so that the types are the same. A literal needs the form of its type (an enum value without quotes, a Text in quotes).",
        "",
    ),
    i(
        "DEF.NULL_COMPARE_NON_NULLABLE",
        "a field that can never be null is compared with null",
        "The comparison is always false (or always true), which is almost never what was meant.",
        "Declare the field nullable with `?` if null is a real state, otherwise remove the comparison.",
        concat!(base!(), "resource Club {\n  fields { id: Id; name: Text; school: Member? }\n  rows read when school = null\n}\n"),
    ),
    i(
        "DEF.BAD_VALUE",
        "a literal does not have the form of its type",
        "A Url, Email, Time, Date or Decimal literal must be written in its canonical form.",
        "Write the literal as the message says, for example an absolute https URL or an RFC 3339 time.",
        "",
    ),
    i(
        "DEF.BAD_DECIMAL_TYPE",
        "Decimal without precision and scale",
        "A Decimal column has an exact size; it has no default.",
        "Write `Decimal(p,s)`, for example `Decimal(12,2)`.",
        concat!(base!(), "resource Price {\n  fields { id: Id; amount: Decimal(12,2) }\n}\n"),
    ),
    i(
        "DEF.BAD_RANGE",
        "a range that cannot be satisfied or a value outside it",
        "A range such as `Text(1..100)` needs `lo <= hi`, and values assigned by a transition must lie inside the range of the field.",
        "Fix the bounds or the value.",
        "",
    ),
    i(
        "DEF.ID_FIELD_NEEDS_REFERENCE",
        "a resource field declared as `X.Id`",
        "A field in `resource ... fields { }` is a database column. `club: Club` is a reference: a foreign key the database checks, joinable and traversable. `club: Club.Id` would be a bare number that merely happens to be a club id, with nothing keeping it valid. The language keeps one spelling for \"this row belongs to a club\", so the second is rejected. The primary key follows the same rule: it is `id: Id` and nothing else, so `id: Note.Id` is rejected too. `X.Id` stays valid where no column is involved: `input`, `access` parameters, operation inputs and outputs.",
        "Write `club: Club` (add `?` when it may be null). Adding the reference also adds a foreign key to the table. For the key, write `id: Id`.",
        concat!(base!(), "resource Club {\n  fields { id: Id; name: Text }\n}\nresource Note {\n  fields { id: Id; body: Text; club: Club }\n}\n"),
    ),
    i(
        "DEF.RESERVED_NAME",
        "a name the generated code cannot carry",
        "JavaScript reads `__proto__` as the prototype accessor, not as an ordinary key, so a resource, field, transition, operation or parameter with that name would silently vanish from generated descriptors and SDK objects, and every call that uses it would be refused.",
        "Rename it, for example `proto` or `protoField`.",
        concat!(base!(), "resource Note {\n  fields { id: Id; proto: Text }\n}\n"),
    ),
    i(
        "DEF.ARITH_NOT_ALLOWED",
        "arithmetic outside the one place it is allowed",
        "Only the `to` block of a transition may change a number, written `field = field + 1` or `field = field - 1`. Conditions never compute.",
        "Move the arithmetic into a transition, or compare with a literal.",
        "",
    ),
    i(
        "DEF.ARITY_MISMATCH",
        "a predicate is called with the wrong number of arguments",
        "A predicate call passes exactly the parameters it declares.",
        "Pass one argument for each declared parameter, in order.",
        "",
    ),
    i(
        "DEF.THIS_IN_EXISTS",
        "`this` used where there is no row",
        "`this` is the row a policy is evaluated for; inside `exists ... where` the quantified row is named instead.",
        "Name the row explicitly through a predicate parameter.",
        "",
    ),
    // Policies and access.
    i(
        "DEF.ACCESS_MISMATCH",
        "an access used in a place it was not declared for",
        "An `access` that names a group key may only guard aggregates that group by that key.",
        "Declare an access for this aggregate, or give the aggregate the matching `groupKey`.",
        "",
    ),
    i(
        "DEF.BAD_CAPABILITY",
        "a capability name that is not valid",
        "Managed capability names are ASCII identifiers.",
        "Use letters, digits and `_`.",
        "",
    ),
    i(
        "DEF.POLICY_CYCLE",
        "predicates call each other in a cycle",
        "A policy is expanded into one query; a cycle has no end.",
        "Break the cycle: move the shared part into a third predicate that calls neither.",
        "",
    ),
    i(
        "DEF.POLICY_FIELD_NOT_FILTERABLE",
        "a field with a read policy cannot be filtered or sorted",
        "If a hidden value could be filtered or sorted on, the caller could infer it from which rows come back.",
        "Remove the field from `filter` and `sort`, or remove its `field ... read when` policy.",
        "",
    ),
    i(
        "DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION",
        "a field with a read policy in a transition's `from`, the value of `to`, or under `repeat unchanged`",
        "A caller who cannot read the field still sees the outcome of a transition: `WIRE.INVALID_STATE` when `from` is false, `unchanged` when the row already holds the target value, and a copied value when `to` reads the field into a visible one. Each of those would reveal the hidden value, so the definition is rejected. `allow` is the author's authorization rule and a write to a hidden field without `repeat unchanged` reveals nothing, so those stay valid.",
        "Express the condition on a field without a read policy, or remove the field's `field ... read when` policy. For a hidden target field, drop `repeat unchanged`.",
        "",
    ),
    i(
        "DEF.POLICY_FIELD_NOT_AGGREGATABLE",
        "a field with a read policy cannot be aggregated",
        "An aggregate such as `sum(field)` would reveal the hidden values.",
        "Aggregate another field, or remove the field policy.",
        "",
    ),
    // Read exposure.
    i(
        "DEF.AGG_NOT_SELECTABLE",
        "an aggregate that is not a per-row aggregate is selected",
        "Only aggregates with a `groupKey` give each row a value.",
        "Remove it from `select`, or give it a `groupKey` that points to this resource.",
        "",
    ),
    i(
        "DEF.TRAVERSE_NOT_EXPOSED",
        "traversing to a resource or field that is not exposed",
        "A traverse can read only what the child's own `expose read` selects.",
        "Add the field to the child's `expose read select`, or select another field.",
        "",
    ),
    i(
        "DEF.TRAVERSE_VIA_NOT_REF",
        "the `via` field does not reference this resource",
        "A one-to-many traverse follows a reference field of the child that points back.",
        "Name a field of the child whose type is this resource.",
        "",
    ),
    i(
        "DEF.TRAVERSE_MANY_AGGREGATE",
        "an aggregate inside a one-to-many traverse",
        "Aggregates in the children of a 1:N traverse are not supported yet.",
        "Select a plain field, or read the aggregate on the child resource itself.",
        "",
    ),
    i("DEF.BAD_LIMIT", "a traverse limit outside 1..1000", "Every traverse is bounded.", "Use a number from 1 to 1000.", ""),
    i(
        "DEF.BAD_BUDGET",
        "a budget or bulk size outside its range",
        "Budgets and bulk sizes have no unlimited default: they are positive and bounded.",
        "Use a positive number within the product limit.",
        "",
    ),
    i(
        "DEF.BUDGET_DEPTH_TOO_SMALL",
        "a traverse needs depth 2 or more",
        "The root is depth 1 and each traverse adds one.",
        "Set `depth 2` (or more) in `budget`.",
        "",
    ),
    i(
        "DEF.CURSOR_SORT_NOT_SELECTED",
        "cursor paging needs the sort fields to be selected",
        "A cursor is built from the sort values of the last row.",
        "Add the sort fields to `select`, and make sure they have no field policy.",
        "",
    ),
    i(
        "DEF.CURSOR_NULLABLE_SORT",
        "cursor paging cannot sort on a nullable field",
        "Null ordering would make the page boundary ambiguous.",
        "Sort on a non-null field.",
        "",
    ),
    i(
        "DEF.CURSOR_ID_NOT_SELECTED",
        "cursor paging needs `id` selected",
        "`id` breaks ties between rows with equal sort values.",
        "Add `id` to `select`.",
        "",
    ),
    // Aggregates, extensions, effects.
    i(
        "DEF.GROUP_KEY_MISMATCH",
        "the groupKey does not reference the owning resource",
        "A per-row aggregate groups the source rows by the reference that points to the row it belongs to.",
        "Name a field of the source whose type is the owning resource.",
        "",
    ),
    i(
        "DEF.EXT_INPUT_UNBOUND",
        "an access input is not received by the extension",
        "An extension may use an access only if every input that access needs is an input of the extension with the same type.",
        "Add the input to the extension `input` block, with the same name and type.",
        "",
    ),
    i(
        "DEF.EFFECT_MISMATCH",
        "the declared effect does not fit the extension kind",
        "A read extension has `effect none`; a write extension has `effect db`.",
        "Change `effect` to the value the message names.",
        "",
    ),
    i(
        "DEF.UNSUPPORTED_EFFECT",
        "an effect that the executor cannot make atomic",
        "A transition cannot change the primary key of the locked row, and a many-row update needs a parent condition so that its scope is the row's own children.",
        "Follow the message: set only other fields, or add `parent = this.id` to the update.",
        "",
    ),
    i(
        "DEF.UNSUPPORTED_INVARIANT",
        "an invariant condition that cannot be enforced in the database",
        "An invariant is a unique index; it can use the fields of the row and literals, not time, the actor or sub-queries.",
        "Make the condition use only fields of the row and literals.",
        "",
    ),
    i(
        "DEF.UNSUPPORTED",
        "a form that the product does not support yet",
        "The construct is recognised but not executable. The message says which part and what is supported instead.",
        "Use the supported form named in the message.",
        "",
    ),
    // Options of the development checks.
    i(
        "DEF.BAD_OPTION",
        "a check option has the wrong type",
        "The options of the development checks are a small object.",
        "Pass `{ \"devChecks\": true }` or `{ \"devChecks\": false }`.",
        "",
    ),
    i("DEF.UNKNOWN_OPTION", "a check option that does not exist", "Unknown options are rejected, not ignored.", "Use only `devChecks`.", ""),
    i(
        "DEF.ADVISORY_UNREACHABLE_READ",
        "an expose read that no caller can reach (advisory)",
        "Not an error: the definition is accepted. An `expose read` without `budget` is only a target of another resource's `traverse`; when no resource traverses it, every read of it fails at run time with WIRE.NOT_ROOT_QUERYABLE. The usual cause is a forgotten `budget`.",
        "Add `budget { rows N; depth 1; deadline 2s; cost N }` to the `expose read` to make it readable on its own, or traverse it from the resource that should embed it.",
        concat!(
            base!(),
            "resource Note {\n  fields { id: Id; body: Text; owner: Member }\n  expose read { select id, body\n    budget { rows 10; depth 1; deadline 2s; cost 100 } }\n}\n"
        ),
    ),
    i(
        "DEF.ADVISORY_ABSENCE_POLICY",
        "a write whose condition needs another row to be absent (advisory)",
        "Not an error: the definition is accepted. A transition or create whose `allow`/`from` says `not exists ...` is decided on a snapshot, so two requests that change different rows could both pass it. The server therefore runs it in a SERIALIZABLE transaction and retries a collision, which costs more than the plain path.",
        "If the goal is uniqueness or a capacity, declare `unique a, b` or `limit name on X = atMost N where ...` with `invariant name per field`; the database enforces those directly and cheaper. Keep the `not exists` rule when it expresses something those cannot.",
        "",
    ),
    i(
        "DEF.ADVISORY_UNUSED_PREDICATE",
        "a predicate that nothing uses (advisory)",
        "Not an error: the definition is accepted. An unused predicate is usually a rule that was meant to be applied and is not.",
        "Use the predicate in a `rows read`, `field ... read`, `allow` or `access`, or remove it.",
        "",
    ),
    i(
        "DEF.ADVISORY_CALLER_FILTER_POLICY",
        "a caller-filtered aggregate omits its source row policy (advisory)",
        "Not an error: the definition is accepted. A standalone aggregate does not inherit the source resource's `rows read` policy. With `callerFilter read` and no aggregate `where`, the count may reveal whether rows hidden from the caller match a supplied filter.",
        "Repeat the intended row policy in the aggregate's `where`.",
        "",
    ),
];
