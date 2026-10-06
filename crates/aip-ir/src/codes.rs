//! Single source of truth for every diagnostic and error code the toolchain can
//! produce. The codes are a stable contract that machines depend on: what a
//! code means, how to fix it, and when it occurs are answered here, not in
//! prose that can drift. `spec/diagnostics.md` and `aip explain-code` are
//! generated from [`REGISTRY`].
//!
//! The registry lives in `aip-ir` because it has no dependencies of its own and
//! already owns the protocol taxonomy (`facts`) and the structure codes
//! (`validate`); every other crate that emits a code (syntax, sema, pg, runtime)
//! can depend on it without a cycle. Emitters use the constants below instead of
//! string literals; `tests/codes.rs` scans the sources for leftovers.
//!
//! Stability: a published code never changes meaning. A code that is no longer
//! produced stays in the registry with `deprecated: true`.

use serde::Serialize;
use std::fmt::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// `AIP-E***`: the checker or a backend rejects the program.
    Error,
    /// `AIP-W***`: accepted, but probably not what was meant.
    Warning,
    /// `AIP-I***`: Core IR that violates its structural rules.
    Ir,
    /// `AIP.*`: failure a running intent reports to its caller.
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct CodeInfo {
    pub code: &'static str,
    pub kind: Kind,
    pub severity: Severity,
    /// One line, what the code says.
    pub title: &'static str,
    /// Why it is a problem and when it occurs.
    pub explain: &'static str,
    /// The standard form or the way to repair it.
    pub fix: &'static str,
    /// `.aip` that parses and shows the fixed form; empty when there is none.
    pub example: &'static str,
    /// HTTP status of a runtime error; `None` for compile-time codes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// Whether the caller may send the same call again with the same
    /// idempotency key; `None` for compile-time codes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
    pub deprecated: bool,
}

const fn diag(
    kind: Kind,
    severity: Severity,
    code: &'static str,
    title: &'static str,
    explain: &'static str,
    fix: &'static str,
    example: &'static str,
) -> CodeInfo {
    CodeInfo { code, kind, severity, title, explain, fix, example, http_status: None, retryable: None, deprecated: false }
}

const fn err(code: &'static str, title: &'static str, explain: &'static str, fix: &'static str, example: &'static str) -> CodeInfo {
    diag(Kind::Error, Severity::Error, code, title, explain, fix, example)
}

const fn warn(code: &'static str, title: &'static str, explain: &'static str, fix: &'static str, example: &'static str) -> CodeInfo {
    diag(Kind::Warning, Severity::Warning, code, title, explain, fix, example)
}

const fn ir(code: &'static str, title: &'static str, explain: &'static str, fix: &'static str) -> CodeInfo {
    diag(Kind::Ir, Severity::Error, code, title, explain, fix, "")
}

const fn rt(code: &'static str, http_status: u16, retryable: bool, title: &'static str, explain: &'static str, fix: &'static str) -> CodeInfo {
    CodeInfo {
        code,
        kind: Kind::Runtime,
        severity: Severity::Error,
        title,
        explain,
        fix,
        example: "",
        http_status: Some(http_status),
        retryable: Some(retryable),
        deprecated: false,
    }
}

// Compile-time diagnostics. The constant name is the code without `AIP-`.
pub const E100: &str = "AIP-E100";
pub const E101: &str = "AIP-E101";
pub const E102: &str = "AIP-E102";
pub const E103: &str = "AIP-E103";
pub const E104: &str = "AIP-E104";
pub const E105: &str = "AIP-E105";
pub const E106: &str = "AIP-E106";
pub const E107: &str = "AIP-E107";
pub const E108: &str = "AIP-E108";
pub const E110: &str = "AIP-E110";
pub const E111: &str = "AIP-E111";
pub const E201: &str = "AIP-E201";
pub const E202: &str = "AIP-E202";
pub const E203: &str = "AIP-E203";
pub const E204: &str = "AIP-E204";
pub const E205: &str = "AIP-E205";
pub const E206: &str = "AIP-E206";
pub const E207: &str = "AIP-E207";
pub const E208: &str = "AIP-E208";
pub const E209: &str = "AIP-E209";
pub const E210: &str = "AIP-E210";
pub const E211: &str = "AIP-E211";
pub const E212: &str = "AIP-E212";
pub const E213: &str = "AIP-E213";
pub const E214: &str = "AIP-E214";
pub const E216: &str = "AIP-E216";
pub const E220: &str = "AIP-E220";
pub const E221: &str = "AIP-E221";
pub const E222: &str = "AIP-E222";
pub const E301: &str = "AIP-E301";
pub const E302: &str = "AIP-E302";
pub const E306: &str = "AIP-E306";
pub const E309: &str = "AIP-E309";
pub const E310: &str = "AIP-E310";
pub const E311: &str = "AIP-E311";
pub const E312: &str = "AIP-E312";
pub const E313: &str = "AIP-E313";
pub const E314: &str = "AIP-E314";
pub const E315: &str = "AIP-E315";
pub const E316: &str = "AIP-E316";
pub const E317: &str = "AIP-E317";
pub const E318: &str = "AIP-E318";
pub const E319: &str = "AIP-E319";
pub const E320: &str = "AIP-E320";
pub const E501: &str = "AIP-E501";
pub const E502: &str = "AIP-E502";
pub const E600: &str = "AIP-E600";
pub const E601: &str = "AIP-E601";
pub const E602: &str = "AIP-E602";
pub const W402: &str = "AIP-W402";
pub const W403: &str = "AIP-W403";
pub const W404: &str = "AIP-W404";
pub const W501: &str = "AIP-W501";
pub const W502: &str = "AIP-W502";
pub const W602: &str = "AIP-W602";
pub const W603: &str = "AIP-W603";

// Core IR structure codes. Documented per code in `validate`.
pub const I101: &str = "AIP-I101";
pub const I102: &str = "AIP-I102";
pub const I103: &str = "AIP-I103";
pub const I104: &str = "AIP-I104";
pub const I105: &str = "AIP-I105";
pub const I106: &str = "AIP-I106";
pub const I107: &str = "AIP-I107";
pub const I108: &str = "AIP-I108";
pub const I109: &str = "AIP-I109";
pub const I110: &str = "AIP-I110";
pub const I111: &str = "AIP-I111";
pub const I112: &str = "AIP-I112";
pub const I113: &str = "AIP-I113";
pub const I114: &str = "AIP-I114";
pub const I115: &str = "AIP-I115";
pub const I116: &str = "AIP-I116";
pub const I117: &str = "AIP-I117";
pub const I118: &str = "AIP-I118";
pub const I119: &str = "AIP-I119";
pub const I120: &str = "AIP-I120";
pub const I121: &str = "AIP-I121";
pub const I122: &str = "AIP-I122";

// Runtime and protocol error codes.
pub const AUTH_FORBIDDEN: &str = "AIP.AUTH.FORBIDDEN";
pub const AUTH_UNAUTHENTICATED: &str = "AIP.AUTH.UNAUTHENTICATED";
pub const CONCURRENCY_CONFLICT: &str = "AIP.CONCURRENCY.CONFLICT";
pub const CONFLICT_CAPACITY: &str = "AIP.CONFLICT.CAPACITY";
pub const CONFLICT_IN_USE: &str = "AIP.CONFLICT.IN_USE";
pub const CONFLICT_OVERLAP: &str = "AIP.CONFLICT.OVERLAP";
pub const CONFLICT_STALE_VERSION: &str = "AIP.CONFLICT.STALE_VERSION";
pub const CONFLICT_UNIQUE: &str = "AIP.CONFLICT.UNIQUE";
pub const CONSENT_REQUIRED: &str = "AIP.CONSENT.REQUIRED";
pub const ENCRYPTION_DECRYPT_FAILED: &str = "AIP.ENCRYPTION.DECRYPT_FAILED";
pub const ENCRYPTION_KEYS_MISSING: &str = "AIP.ENCRYPTION.KEYS_MISSING";
pub const IDEMPOTENCY_KEY_REUSED: &str = "AIP.IDEMPOTENCY.KEY_REUSED";
pub const INPUT_IDEMPOTENCY_KEY_REQUIRED: &str = "AIP.INPUT.IDEMPOTENCY_KEY_REQUIRED";
pub const INPUT_IDEMPOTENCY_KEY_UNSUPPORTED: &str = "AIP.INPUT.IDEMPOTENCY_KEY_UNSUPPORTED";
pub const INPUT_INVALID: &str = "AIP.INPUT.INVALID";
pub const INPUT_REFERENCE_NOT_FOUND: &str = "AIP.INPUT.REFERENCE_NOT_FOUND";
pub const INTERNAL: &str = "AIP.INTERNAL";
pub const INVARIANT_VIOLATED: &str = "AIP.INVARIANT.VIOLATED";
pub const MIGRATION_CHANGED: &str = "AIP.MIGRATION.CHANGED";
pub const MIGRATION_FAILED: &str = "AIP.MIGRATION.FAILED";
pub const SCHEMA_DATA_CONFLICT: &str = "AIP.SCHEMA.DATA_CONFLICT";
pub const SCHEMA_ENCRYPTION_CHANGE: &str = "AIP.SCHEMA.ENCRYPTION_CHANGE";
pub const SCHEMA_FAILED: &str = "AIP.SCHEMA.FAILED";
pub const SCHEMA_TYPE_CHANGE: &str = "AIP.SCHEMA.TYPE_CHANGE";
pub const SCHEMA_UNDECLARED: &str = "AIP.SCHEMA.UNDECLARED";
pub const SCHEMA_UNRECORDED: &str = "AIP.SCHEMA.UNRECORDED";
pub const SCHEMA_UNSUPPORTED: &str = "AIP.SCHEMA.UNSUPPORTED";
pub const NOT_FOUND: &str = "AIP.NOT_FOUND";
pub const PRECONDITION_FAILED: &str = "AIP.PRECONDITION.FAILED";
pub const RATE_LIMITED: &str = "AIP.RATE.LIMITED";
pub const REQUEST_MALFORMED: &str = "AIP.REQUEST.MALFORMED";
pub const REQUEST_UNKNOWN_INTENT: &str = "AIP.REQUEST.UNKNOWN_INTENT";
pub const SUBSCRIPTION_LIMIT: &str = "AIP.SUBSCRIPTION.LIMIT";
pub const SUBSCRIPTION_TOO_LARGE: &str = "AIP.SUBSCRIPTION.TOO_LARGE";
pub const TENANT_MISMATCH: &str = "AIP.TENANT.MISMATCH";
pub const UNAVAILABLE: &str = "AIP.UNAVAILABLE";

/// HTTP status of a runtime code; codes outside the registry map to 409, which
/// is what the runtime has always answered for an unknown failure.
pub fn http_status(code: &str) -> u16 {
    lookup(code).and_then(|c| c.http_status).unwrap_or(409)
}

/// Whether the caller may retry a runtime code; unknown codes are not retryable.
pub fn retryable(code: &str) -> bool {
    lookup(code).and_then(|c| c.retryable).unwrap_or(false)
}

pub fn lookup(code: &str) -> Option<&'static CodeInfo> {
    REGISTRY.iter().find(|c| c.code == code)
}

pub fn all() -> &'static [CodeInfo] {
    REGISTRY
}

/// Plain-text rendering used by `aip explain-code`.
pub fn render_text(c: &CodeInfo) -> String {
    let mut out = String::new();
    let sev = match c.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    let _ = writeln!(out, "{}  {}  {}{}", c.code, sev, c.title, if c.deprecated { "  [deprecated]" } else { "" });
    let _ = writeln!(out, "\nwhy:\n  {}", c.explain);
    let _ = writeln!(out, "\nfix:\n  {}", c.fix);
    if !c.example.is_empty() {
        let _ = writeln!(out, "\nexample:");
        for l in c.example.lines() {
            let _ = writeln!(out, "  {l}");
        }
    }
    if let (Some(s), Some(r)) = (c.http_status, c.retryable) {
        let _ = writeln!(out, "\nhttp status: {s}\nretryable with the same idempotency key: {}", if r { "yes" } else { "no" });
    }
    out
}

/// `spec/diagnostics.md`, generated. Regenerate with `aip diagnostics --markdown`.
pub fn render_markdown() -> String {
    let mut out = String::new();
    out.push_str(
        "# AIP 진단·오류 코드\n\n\
         > 이 문서는 `crates/aip-ir/src/codes.rs`의 레지스트리에서 생성된다. 손으로 고치지 않는다. 다시 만들려면 `aip diagnostics --markdown > spec/diagnostics.md`를 실행한다.\n\n\
         ## 코드 체계\n\n\
         | 접두 | 뜻 | 발생 지점 |\n|---|---|---|\n\
         | `AIP-E***` | 오류. 프로그램이 컴파일되지 않는다 | `aip check`, 백엔드 계획 단계 |\n\
         | `AIP-W***` | 경고. 컴파일되지만 의도와 다를 가능성이 크다 | `aip check`, 백엔드 계획 단계 |\n\
         | `AIP-I***` | Core IR 구조 위반. 프런트엔드가 만든 IR이 구조 규칙을 어긴다 | IR 검증 |\n\
         | `AIP.*` | 런타임·프로토콜 오류. 호출자가 응답으로 받는다 | `aip run` |\n\n\
         번호의 앞자리는 영역을 가리킨다. 1xx는 선언과 이름, 2xx는 타입과 값, 3xx는 권한·개인정보·정합성, 4xx는 품질 경고, 5xx는 extension, 6xx는 백엔드다. 번호는 영역 안에서 비어 있을 수 있고 재사용하지 않는다.\n\n\
         ## 안정성 규칙\n\n\
         - 한 번 공개된 코드는 의미를 바꾸지 않는다. 더 좁거나 다른 의미가 필요하면 새 코드를 만든다.\n\
         - 더 이상 발생하지 않는 코드는 지우지 않고 `deprecated`로 표시한다.\n\
         - 메시지 문장은 계약이 아니다. 기계는 `code`(런타임 오류는 `code`와 `reason`)에 의존한다.\n\
         - 런타임 오류의 `retryable`은 같은 호출을 같은 `Idempotency-Key`로 다시 보내도 되는지를 뜻한다.\n\n\
         조회: `aip explain-code <CODE>`.\n",
    );
    let sections =
        [(Kind::Error, "오류 (AIP-E)"), (Kind::Warning, "경고 (AIP-W)"), (Kind::Ir, "IR 구조 (AIP-I)"), (Kind::Runtime, "런타임 오류 (AIP.*)")];
    for (kind, heading) in sections {
        let _ = write!(out, "\n## {heading}\n");
        for c in REGISTRY.iter().filter(|c| c.kind == kind) {
            let _ = write!(out, "\n### `{}`{}\n\n{}\n\n", c.code, if c.deprecated { " (deprecated)" } else { "" }, c.title);
            let _ = write!(out, "- 원인: {}\n- 수정: {}\n", c.explain, c.fix);
            if let (Some(s), Some(r)) = (c.http_status, c.retryable) {
                let _ = write!(out, "- HTTP 상태: {s}\n- 재시도: {}\n", if r { "가능" } else { "불가" });
            }
            if !c.example.is_empty() {
                let _ = write!(out, "\n```aip\n{}\n```\n", c.example);
            }
        }
    }
    out
}

pub static REGISTRY: &[CodeInfo] = &[
    // ---- AIP-E1xx: declarations and names ----
    err(
        E100,
        "syntax error",
        "The source does not match the grammar (`spec/grammar.md`): an unexpected character, a malformed literal, a clause out of order or a missing part. Also used when a query has no `from`/`fetch`.",
        "Follow the message and its help text; clause order is fixed (see `spec/grammar.md`).",
        "query Clubs {\n  allow public\n  from Club c\n  page 20 by keyset\n  select { id }\n}",
    ),
    err(
        E101,
        "duplicate or clashing name",
        "Two declarations, enum values, parameters, selections, assignments or webhook handlers use the same name in one scope, a name shadows another name or a built-in type, or an intent a form generates (`PublishArticle`, `GiveTermsConsent`, `StartMemberImpersonation`, ...) has the name of a declared one.",
        "Rename or remove one of them; a name means exactly one thing in its scope.",
        "enum Status { ACTIVE ENDED }\nentity Club { name: Text }",
    ),
    err(
        E102,
        "unknown type or entity",
        "A type or entity name (in a type position, `from`, a reference or the actor declaration) is not declared.",
        "Declare the type or entity, or fix the spelling; the help text lists what exists.",
        "entity Club { name: Text  fee: Money(KRW) }",
    ),
    err(
        E103,
        "unknown field or predicate",
        "A field or predicate is read or written on an entity that does not have it.",
        "Use a field the entity declares; the help text lists the entity's fields.",
        "command Rename(club: Club, name: Text) {\n  allow authenticated\n  do { set club.name = name }\n}",
    ),
    err(
        E104,
        "unknown name",
        "A name does not refer to a parameter, binding, function, relation, search, intent, flag, partition, event or webhook source that is in scope or known.",
        "Declare the name or correct the spelling; the help text suggests close matches.",
        "relation isOwner(m: Member, c: Club) = exists membership(m, c)",
    ),
    err(
        E105,
        "implicit field redeclared",
        "The entity declares a field whose name the runtime already maintains implicitly (for example `version` on a versioned or history entity).",
        "Remove the declaration, or rename the field.",
        "entity Club { name: Text }",
    ),
    err(
        E106,
        "invalid inverse relation",
        "`via` is used on a field that is not an inverse relation, an inverse relation field has no `via <backref field>`, or the named field of the other entity is not a reference back to this entity.",
        "Write `via` only on `Entity[]` fields and name a reference field of the other entity that points back; the help text shows the declaration to add.",
        "entity Item { parent: Order }\nentity Order { items: Item[] via parent }",
    ),
    err(
        E107,
        "actor declared more than once",
        "An application has exactly one actor; a second `actor` declaration is ambiguous about who `actor` refers to.",
        "Delete the extra `actor` declaration; model different roles as an enum field on the actor entity.",
        "use auth\nactor Member via auth.oidc(kakao)\nentity Member personal { nickname: Text }",
    ),
    err(
        E108,
        "type written with wrong options",
        "A type that needs options has none (`Money` without a currency) or a type that takes no options was given some.",
        "Write the type in its standard form such as `Money(KRW)`, and drop the options from types that take none.",
        "entity Club { name: Text  fee: Money(KRW) }",
    ),
    err(
        E110,
        "actor not declared",
        "`actor`, `authenticated`, `visible to actor`, `publishable by` or `impersonate` is used but the application declares no actor.",
        "Declare the actor once at the top of the file (`actor <Entity> via ...`).",
        "use auth\nactor Member via auth.oidc(kakao)\nentity Member personal { nickname: Text }",
    ),
    err(
        E111,
        "evolution declaration contradicts the program",
        "`was old` (on a field or an entity) or `removed field x` / `removed entity X` says how a deployed database moves to this program, so it must agree with the program: a field cannot be `was` its own name or the name of a field that is still declared, a name cannot be both renamed away and removed, two fields cannot continue the same old field, a removed field or entity cannot still be declared, and `was` only applies to entity fields.",
        "Delete the declaration that no longer applies, or the field or entity it contradicts. Once every database has been migrated, the declaration may stay: it does nothing when the old name is not in the database.",
        "entity Task {\n  title: Text was name\n  removed field legacyCode\n}\nremoved entity OldThing",
    ),
    // ---- AIP-E2xx: types and values ----
    err(
        E201,
        "type mismatch",
        "An expression, argument, default, assignment or declaration has a type the context does not accept (wrong operand types, wrong collection element, null for a required field, a non-Bool condition, and similar).",
        "Make both sides the same type; convert or aggregate the value, or change the declared type.",
        "command Promote(t: ClubMember) {\n  allow authenticated\n  do { set t.role = MANAGER }\n}",
    ),
    err(
        E202,
        "not a value of the enum",
        "An identifier used as an enum value is not one of the enum's values (or belongs to several enums and is ambiguous).",
        "Use one of the listed values, or compare against a typed value such as `ClubRole.MANAGER` when the name is shared.",
        "enum ClubRole ordered { GENERAL < MANAGER < ADMIN }",
    ),
    err(
        E203,
        "field cannot be assigned",
        "The field is implicit, an inverse relation or a counter, which the runtime maintains, so `set`, `insert` or `expose` cannot write it.",
        "Remove the assignment; counters change through `touch`.",
        "",
    ),
    err(
        E204,
        "wrong use of a field kind or parameter kind",
        "`+=`/`-=` need a numeric field and `touch` needs a counter field; the operation is applied to a field of another kind.",
        "Use the operation on the kind of field it is made for.",
        "use redis\nentity Post { views: Int counter via redis }",
    ),
    err(
        E205,
        "required fields missing on insert",
        "`insert` (or `expose create`) does not set every required field that has no default or generator.",
        "Assign each missing field named in the message, or give the field a default or make it optional.",
        "command NewClub(name: Text) {\n  allow authenticated\n  do { insert Club { name } }\n}",
    ),
    err(
        E206,
        "invalid `each` loop",
        "`each` needs a bounded collection input and an item name.",
        "Declare the input as a bounded list and write `each items x partial { ... }`.",
        "",
    ),
    err(
        E207,
        "missing unique constraint for toggle or upsert",
        "`toggle` and `upsert` find the existing row through a unique constraint; without a matching one the statement is ambiguous.",
        "Add `unique (...)` over exactly the key columns.",
        "entity Like {\n  club: Club\n  member: Member on erase cascade\n  unique (club, member)\n}",
    ),
    err(
        E208,
        "construct not allowed in this context",
        "A valid construct is used where its context does not allow it: an `Upload` parameter outside a command, `this` outside an entity declaration, a `drafts` query that reads no publishable entity or is `allow public`, a `consent` that lists something a caller cannot invoke (an `internal` intent, a form name), an `impersonate` of an entity other than the actor, or `signingSecret` read anywhere but the `returns` of the (not idempotent) command that inserted the endpoint row.",
        "Move the construct to a place that allows it (`Upload` parameters go on a command; `this` is only written inside an entity; `drafts` goes on a query of a publishable entity with its own `allow`; `consent` lists declared commands, queries and jobs; `impersonate` names the actor entity; `signingSecret` is read as `returns ep { secret: ep.signingSecret }` after `insert Endpoint { ... } as ep`).",
        "",
    ),
    err(
        E209,
        "invalid assignment target",
        "An assignment or `into` target is not a field of a row.",
        "Assign to `row.field`; derived values cannot be targets.",
        "command Rename(club: Club, name: Text) {\n  allow authenticated\n  do { set club.name = name }\n}",
    ),
    err(
        E210,
        "empty or below the minimum",
        "A construct that needs at least one part has none: an `in` list without values, an enum without values, an approval that requires fewer than 1 approval, a `consent` below version 1, or an `impersonate` with a `ttl` under one second.",
        "Add at least one value (or raise the number to 1 or more).",
        "enum Status { ACTIVE ENDED }\nentity Club { name: Text }",
    ),
    err(
        E211,
        "wrong number of arguments",
        "A function, relation or built-in is called with more or fewer arguments than it declares.",
        "Pass exactly the declared number of arguments; the message names the expected count.",
        "relation memberCount(c: Club) = count(c.members)",
    ),
    err(
        E212,
        "form not allowed on this construct",
        "A clause or shape that the construct does not take is written: a `: body` on `count`, `same()` without a binder, a spread in an event, `insert ... from` bound with `as`, or approvers without an alias.",
        "Write the construct in the form the message names (for example `count(items x where ...)`, one event field per name, an alias on approvers).",
        "",
    ),
    err(
        E213,
        "multi-valued where one value is needed",
        "A set-valued expression (a relation or collection) is used where a single value is required.",
        "Aggregate it (`count`, `sum`, `same`, `the`) or bind one element.",
        "relation memberCount(c: Club) = count(c.members)",
    ),
    err(
        E214,
        "invalid webhook option",
        "A webhook option is unknown, is not written as a named string literal, or the required `secret` of `http.webhook` is missing.",
        "Write options as `name: \"value\"` with one of secret, header, event, id, payload; `http.webhook` needs `secret: \"<ENV VAR NAME>\"` (the secret itself never goes in source).",
        "",
    ),
    err(
        E216,
        "event emitted with different shapes",
        "The same event is emitted with a different field set or types in two places, so consumers cannot rely on its shape.",
        "Emit the event with the same fields everywhere, or give the second shape its own event name.",
        "",
    ),
    err(
        E220,
        "selection on a non-relation or relation without selection",
        "A sub-selection is written on a scalar field, or a relation field is selected without saying which of its fields to return.",
        "Select scalars directly; write `relation { fields }` for relations.",
        "",
    ),
    err(
        E221,
        "incomplete sort cases",
        "`sort by <param> of { ... }` does not cover every value of the enum parameter.",
        "Add a case for each enum value listed in the message.",
        "",
    ),
    err(
        E222,
        "more than one generator on a field",
        "A field has several value generators (sequence, slug, position, counter); the IR keeps one meaning per field.",
        "Keep one generator and model the other in a separate field.",
        "entity Ticket {\n  club: Club\n  code: Text sequence per club format \"T-{n}\"\n}",
    ),
    // ---- AIP-E3xx: authorization, personal data, consistency ----
    err(
        E301,
        "missing `allow`",
        "Every intent and every exposed operation must say who may call it; there is no default. A `publishable` entity says who may publish it with `publishable by <condition>`, or leaves it to the superuser, and the program must declare one of them.",
        "Add `allow public`, `allow authenticated` or an `allow <condition>` clause.",
        "command Rename(club: Club, name: Text) {\n  allow authenticated\n  do { set club.name = name }\n}",
    ),
    err(
        E302,
        "snapshot entity has no dynamic schema",
        "`Json validated by <field>` names a `Snapshot<Entity>` field, but that entity declares no `dynamic schema from ...`, so there is nothing to validate against.",
        "Add `dynamic schema from <List<Record> field>` to the entity.",
        "",
    ),
    err(
        E306,
        "`self` outside actor visibility",
        "`self` is only valid in field visibility and masking rules of the actor entity.",
        "Use a named parameter or `actor` in other places.",
        "",
    ),
    err(
        E309,
        "personal data reference without erase policy",
        "A field that references personal data does not say what happens when that data is erased, or an anonymized field is not optional.",
        "Add `on erase cascade`, `on erase anonymize` (optional field) or `on erase restrict`.",
        "entity Comment {\n  author: Member? on erase anonymize\n  body: Text\n}",
    ),
    err(
        E310,
        "erase on a non-personal entity",
        "`erase` applies to entities declared `personal`.",
        "Declare the entity `personal`, or delete the row instead of erasing it.",
        "entity Member personal { nickname: Text }",
    ),
    err(
        E311,
        "ambiguous notification recipient",
        "`notify` cannot tell which reference of the row is the recipient because the entity has several references to the actor.",
        "Notify a set of actor rows, or an entity with exactly one actor field.",
        "",
    ),
    err(
        E312,
        "validated by does not name a snapshot field",
        "`Json validated by <field>` must name a `Snapshot<Entity>` field. A plain entity reference (or any other field) can change later, so old answers would lose the schema they were written against.",
        "Declare the field as `Snapshot<Entity>` and point `validated by` at it.",
        "",
    ),
    err(
        E313,
        "unusable tenant path",
        "`tenant via <path>` must be a chain of required references that starts at the entity and ends at the entity that is the tenant (for example `Workspace`). The path names a field that does not exist, is not a reference, is optional, ends at another tenant-scoped entity, or two entities end at different tenants.",
        "Write the path as required reference fields, and end every tenant path at the same entity.",
        "entity Workspace { name: Text }\nentity Project {\n  workspace: Workspace\n  tenant via workspace\n}\nentity Task {\n  project: Project\n  tenant via project.workspace\n}",
    ),
    err(
        E314,
        "tenant-scoped rows read without a tenant",
        "The intent, or the form (approval, job, grant link, verification, subscription), reads rows of a tenant-scoped entity (`from`, a set expression, an update or delete target, an inverse relation of something that is not tenant-bound) but has no required parameter that fixes the tenant. A read that names no tenant would span all of them, and the only way to opt out is `internal cross tenant` on an intent. Event handlers, schedule items and rules are anchored by the row they are about; so are the endpoints of an `outbound webhooks` form, by the event, which therefore has to reference a tenant-scoped row. Webhooks, consumers and retention have no anchor and are not checked here: their writes are held to one tenant by the database instead.",
        "Add a required parameter of the tenant entity or of a tenant-scoped entity (`workspace: Workspace`), or make the intent `internal cross tenant`.",
        "entity Workspace { name: Text }\nentity Project {\n  workspace: Workspace\n  title: Text\n  tenant via workspace\n}\nquery Projects(workspace: Workspace) {\n  allow authenticated\n  from Project p\n  page 20 by keyset\n  select { id title }\n}",
    ),
    err(
        E315,
        "cross tenant on an intent that is not internal",
        "`cross tenant` lifts the single-tenant rules, so on an intent it is only allowed together with `internal`, which clients cannot call. A public intent that works across tenants would let any caller read or write other tenants. Declarations that have no caller (`on Event`, `schedule`, `rule`, `consume` and a webhook handler) may be written `cross tenant` without `internal`.",
        "Write `internal cross tenant`, or remove `cross tenant` and give the intent a parameter that fixes its tenant.",
        "entity Workspace { name: Text }\ninternal cross tenant query AllWorkspaces() {\n  allow public\n  from Workspace w\n  page 50 by keyset\n  select { id name }\n}",
    ),
    err(
        E316,
        "invalid search declaration",
        "A `search` names a language the runtime has no text analysis for, a weight other than A, B, C or D, a field that is not text (Text, RichText, Email, Url, Phone), the same field twice, or no field at all. A search over an entity with `publishable` is refused too, because readers see the published copy and the index is on the working table.",
        "Use one of the supported languages, weights A to D, and text fields of the entity, each once.",
        "entity Post {\n  title: Text(1..100)\n  body: Text(1..4000)\n}\nsearch PostSearch on Post fields [title weight A, body weight B] language english",
    ),
    err(
        E317,
        "invalid outbound webhooks declaration",
        "An `outbound webhooks` form names a signature scheme other than `hmac_sha256`, a retry count above 16, a window (`over`) or a `disable after` that is not positive, no event, or an endpoint entity without a `url: Url` field to deliver to. Two forms for one entity are refused too, because an endpoint belongs to one subscription.",
        "Use `sign hmac_sha256`, `retry` from 0 to 16, positive durations, at least one event, and a `url: Url` field on the endpoint entity; declare one form per endpoint entity.",
        "entity Endpoint {\n  url: Url\n}\nevent Paid { id: Uuid }\noutbound webhooks for Endpoint e {\n  events [Paid]\n  sign hmac_sha256 retry 8 over 24h disable after 3d failing\n}",
    ),
    err(
        E318,
        "invalid encrypted field",
        "An `encrypted` field is stored as ciphertext the database cannot compute on. It must be a plain stored text field (Text, Email, Url, Phone), must not carry a `default` or a generated value (the default would be stored in the clear), and cannot be part of a `unique`, `no overlap` or `at most one` declaration or of a `search`, because the database would compare, index or search the ciphertext, which is random for every value.",
        "Keep `encrypted` on text fields only, remove the default, and take the field out of unique, search and the other declarations; look a row up by something that is not encrypted (an id, or a separate field such as a hash you compute and store yourself).",
        "entity Member {\n  email: Email personal encrypted\n  nickname: Text(2..20)\n  unique (nickname)\n}",
    ),
    err(
        E319,
        "encrypted field used in an expression",
        "The runtime encrypts a value just before it is bound to a statement and decrypts a result just before it is returned, so the database never sees the plaintext of an `encrypted` field. An expression that makes the database read it (a comparison, a filter, `sort by`, an aggregate, a `group by`, a condition, a computed value, an event or notification payload, a copy into another field or row, an `exists` over it) would work on ciphertext and silently give wrong answers.",
        "Select the field by name in a `select` or `returns` of the same row, or write it from a parameter with `set row.field = param`. To filter or sort by it, keep a separate, unencrypted field for that purpose.",
        "entity Member {\n  email: Email personal encrypted\n  nickname: Text(2..20)\n}\nquery MemberByNickname(nickname: Text) {\n  allow authenticated\n  from Member m where m.nickname = nickname\n  page 20 by keyset\n  select { id nickname email }\n}",
    ),
    err(
        E320,
        "encrypted field cannot be written this way",
        "The ciphertext of a row is bound to the row's id, so a value is encrypted for exactly one row. The value written to an `encrypted` field has to be a parameter (or a value the program already received), and the write has to name one row: `insert`, or `set row.field = value`. An `update` or `upsert` over many rows, an `insert ... from` a set, a `toggle`, or a value computed from other data cannot be encrypted per row.",
        "Write the field with `set row.field = param` on one row, or insert the row with the parameter as the value; do the work row by row with `each`.",
        "entity Member {\n  email: Email personal encrypted\n  nickname: Text(2..20)\n}\ncommand ChangeEmail(member: Member, email: Email) {\n  allow actor = member\n  do { set member.email = email }\n}",
    ),
    // ---- AIP-E5xx: extensions ----
    err(
        E501,
        "extension not declared or misused",
        "A name, type, effect, broker, webhook source or provider belongs to an extension the application did not declare with `use`.",
        "Add `use <extension>` at the top of the file.",
        "use s3\nentity Photo { file: s3.Object }",
    ),
    err(
        E502,
        "call is not an extension effect",
        "A statement is a call without a namespace; only extension effects (`ns.effect(...)`) can stand alone as statements.",
        "Write the effect as `ns.effect(...)`, or use the call inside an expression.",
        "",
    ),
    // ---- AIP-E6xx: backend ----
    err(
        E600,
        "expression cannot be planned",
        "The PostgreSQL backend cannot turn an expression into SQL (an unsupported construct or an unresolved type). Reported by the backend with a path instead of a source position.",
        "Rewrite the expression with supported constructs; see the message for the construct.",
        "",
    ),
    err(
        E601,
        "statement cannot be planned",
        "The PostgreSQL backend cannot plan a statement or intent. Reported with a path instead of a source position.",
        "Simplify the statement; see the message for the unsupported part.",
        "",
    ),
    err(
        E602,
        "checked but not executable yet",
        "The frontend accepts the construct (a consumer, a projection, a search, a subscription or another form) but this runtime has no executable plan for it, so it is refused instead of silently ignored.",
        "Remove the construct, or wait for a runtime that supports it.",
        "",
    ),
    // ---- AIP-W: warnings ----
    warn(
        W402,
        "redundant comparison or call",
        "Both sides of a comparison are the same expression, or a function is called with the same expression twice, which is almost always a typo.",
        "Compare two different values.",
        "relation sameClub(a: ClubMember, b: ClubMember) = a.club = b.club",
    ),
    warn(
        W403,
        "unbounded list query",
        "A query returns every row; the result grows with the data and cannot be paged by clients safely.",
        "Add `page N by keyset` (or `by offset`) to the query.",
        "query Clubs {\n  allow public\n  from Club c\n  page 20 by keyset\n  select { id }\n}",
    ),
    warn(
        W404,
        "schedule with time of day but no timezone",
        "A schedule that fires at a time of day without a timezone depends on the server's zone.",
        "Add `tz \"Asia/Seoul\"` (or another IANA zone) to the schedule.",
        "",
    ),
    warn(
        W501,
        "non-idempotent command creates rows",
        "A command inserts rows and binds them without an `idempotent` declaration, so a client retry creates duplicates.",
        "Add `idempotent` so callers send an `Idempotency-Key`, or rely on a unique constraint.",
        "",
    ),
    warn(
        W502,
        "extension is not first-party",
        "The extension named in `use` is not on the toolchain's first-party list, so the compiler has no knowledge of its effects.",
        "Use a first-party extension, or accept that its effects are unchecked.",
        "",
    ),
    // `encrypted`, its only use so far, is implemented now (E318 to E320 and the encryption runtime codes); the code stays for the next unenforced declaration
    CodeInfo {
        deprecated: true,
        ..warn(
            W602,
            "declared but not enforced by this runtime",
            "The runtime accepts the declaration but does not implement it, so the stated guarantee does not hold. No declaration is in this state any more: `encrypted` used to be the one, and now the runtime encrypts it.",
            "Treat the guarantee as absent until the runtime supports it.",
            "",
        )
    },
    warn(
        W603,
        "search language is approximated",
        "The `search` is declared with a language whose words the engine cannot reduce to stems (`korean`). It matches by word prefix: a query word finds the words that begin with it, so a particle or ending after the stem is tolerated, but a changed stem, a compound word or a match in the middle of a word is not found. This is not morphological analysis (open issue OI-08).",
        "Accept prefix matching, or run a search engine with a Korean analyzer and keep this declaration for development.",
        "",
    ),
    // ---- AIP-I: Core IR structure ----
    ir(
        I101,
        "version tag differs",
        "The program's `aip_core` tag is not the version this toolchain reads.",
        "Regenerate the IR with a frontend that targets `CORE_IR_VERSION`.",
    ),
    ir(I102, "unknown enum in a type", "A type names an enum that is not in the program.", "Add the enum or fix the type name."),
    ir(I103, "unknown record in a type", "A type names a record that is not in the program.", "Add the record or fix the type name."),
    ir(
        I104,
        "unknown entity in a type",
        "A `Ref`, `RefUnion` or `Snapshot` type names an entity that is not in the program.",
        "Add the entity or fix the type name.",
    ),
    ir(I105, "zero bound", "A `Set` or `List` bound is zero.", "Use a bound of at least one."),
    ir(I106, "reference to unknown entity", "A reference field targets an entity that does not exist.", "Add the entity or fix the target."),
    ir(
        I107,
        "invalid inverse field",
        "An inverse field targets a missing entity or a missing `via` field.",
        "Point it at an existing entity and reference field.",
    ),
    ir(I108, "unknown field expression", "A `Field` expression names a missing entity or field.", "Name an entity and field that exist."),
    ir(I109, "unknown row field", "A `RowField` names a missing entity or field.", "Name an entity and field that exist."),
    ir(I110, "unknown enum value", "An `EnumValue` names a missing enum or value.", "Name an enum and value that exist."),
    ir(I111, "unknown callee", "A call names a missing function, relation or intent.", "Declare the callee or fix the name."),
    ir(I112, "unknown write target", "An insert, upsert or toggle names a missing entity or field.", "Name an entity and fields that exist."),
    ir(
        I113,
        "invalid lifecycle",
        "A lifecycle names a missing field, a non-enum field or a missing enum value.",
        "Point it at an enum field and its values.",
    ),
    ir(I114, "unknown event", "A reaction, emit, projection, upcast or event type names a missing event.", "Declare the event or fix the name."),
    ir(
        I115,
        "unknown predicate",
        "A predicate application names a missing entity or predicate.",
        "Declare the predicate on the entity or fix the name.",
    ),
    ir(
        I116,
        "unknown entity reference",
        "An entity reference (actor, source, rule, retain, type test and similar) names a missing entity.",
        "Add the entity or fix the name.",
    ),
    ir(I117, "constraint on unknown field", "A constraint names a field the entity does not have.", "Name a field the entity declares."),
    ir(I118, "unknown record field", "A `RecordField` names a missing record or field.", "Name a record and field that exist."),
    ir(
        I119,
        "counter store not declared",
        "A counter `store` is not an extension listed in `uses`.",
        "Add the extension to `uses` or change the store.",
    ),
    ir(I120, "duplicate constraint ordinal", "Two constraints of one entity share an ordinal.", "Give each constraint its own ordinal."),
    ir(I121, "validated-by field missing", "`Json validated by` names a field the entity does not have.", "Name a field of the entity."),
    ir(
        I122,
        "search missing or misused",
        "A query's `Search` source names a search declaration that is not in the program, or a `SearchRank` node names an alias that no `Search` source of its query binds.",
        "Declare the search, or use the rank only in the query that searches under the alias.",
    ),
    // ---- AIP.*: runtime and protocol ----
    rt(
        AUTH_FORBIDDEN,
        403,
        false,
        "caller is not allowed",
        "The caller is authenticated but the intent's `allow` condition (or an approval rule) is false. `reason` carries the declared code, if any.",
        "Call with an actor that satisfies the condition; retrying with the same actor gives the same answer.",
    ),
    rt(
        AUTH_UNAUTHENTICATED,
        401,
        false,
        "no valid caller",
        "The intent needs an actor and the request has no valid token, actor header or webhook signature.",
        "Send a valid actor token (`aip token`) or the correct webhook signature.",
    ),
    rt(
        CONCURRENCY_CONFLICT,
        409,
        true,
        "concurrent update, retry",
        "The database aborted the transaction because of a serialization failure or deadlock. The engine already retried a few times.",
        "Send the same call again with the same idempotency key.",
    ),
    rt(
        CONFLICT_CAPACITY,
        409,
        false,
        "capacity exceeded",
        "A `capacity` constraint of the entity would be exceeded by the write. `reason` is the declared code.",
        "Free capacity or choose another slot; the same call fails again until the state changes.",
    ),
    rt(
        CONFLICT_IN_USE,
        409,
        false,
        "row is still referenced",
        "Deleting a row failed because other rows still reference it and no `on delete` rule removes them.",
        "Remove or re-point the referencing rows first, or declare an `on delete` rule.",
    ),
    rt(
        CONFLICT_OVERLAP,
        409,
        false,
        "ranges overlap",
        "A `no overlap` constraint would be violated: the new range intersects an existing one. `reason` is the declared code.",
        "Choose a range that does not overlap.",
    ),
    rt(
        CONFLICT_STALE_VERSION,
        409,
        false,
        "row changed since it was read",
        "The command carries a `version` check and the row's version moved on. `reason` names the check.",
        "Reload the row, show the user the new state, then send a new call with a new idempotency key.",
    ),
    rt(
        CONFLICT_UNIQUE,
        409,
        false,
        "unique value already taken",
        "A `unique` constraint or generated unique value (slug, sequence) is already in use. `reason` is the declared code.",
        "Choose another value; repeating the same call fails again.",
    ),
    rt(
        CONSENT_REQUIRED,
        403,
        false,
        "required consent is missing",
        "The intent is listed in a `consent` declaration and the caller has no active record of consent to that consent's current version (never given, given for an older version, or withdrawn). `reason` is the consent's name. 403, not 409, because the caller is known and refused until the user acts; the state of the call itself is fine.",
        "Show the consent text and call `Give<Name>Consent`, then send the original call again; `<Name>ConsentStatus` tells which case it is.",
    ),
    rt(
        ENCRYPTION_DECRYPT_FAILED,
        500,
        false,
        "encrypted value cannot be decrypted",
        "A value stored in an `encrypted` field failed authentication: it was written under a key that is not in `AIP_ENCRYPTION_KEYS`, it was changed in the database, or it was copied from another row or column (the ciphertext is bound to its entity, field and row id). Nothing is returned for that call; the message names the field and the key id, never the value.",
        "Add the key the value was written with to `AIP_ENCRYPTION_KEYS` (after the current one). If the value was moved or edited in the database, restore it from a backup; a value cannot be recovered without its key.",
    ),
    rt(
        ENCRYPTION_KEYS_MISSING,
        500,
        false,
        "encryption keys are missing or unusable",
        "The program has `encrypted` fields, but `AIP_ENCRYPTION_KEYS` is not set or is not a list of `<id>:<base64 of 32 bytes>` entries separated by commas. The server does not start (and `aip migrate` is not affected): running with plaintext would break the guarantee silently. `AIP_ENCRYPTION_KEYS` is separate from `AIP_SECRET`.",
        "Generate a key with `openssl rand -base64 32` and set `AIP_ENCRYPTION_KEYS=k1:<key>`. The first entry encrypts new values, every entry can decrypt; to rotate, put the new key first and run `aip rekey`.",
    ),
    rt(
        IDEMPOTENCY_KEY_REUSED,
        409,
        false,
        "idempotency key reused with different input",
        "The key was already used with a different input. The same input with the same key returns the stored result instead of this error.",
        "Use a new key for a new input, or resend the original input to read the stored result.",
    ),
    rt(
        INPUT_IDEMPOTENCY_KEY_REQUIRED,
        400,
        false,
        "idempotency key required",
        "The command is idempotent and the request has no `Idempotency-Key` header.",
        "Send an `Idempotency-Key` header and use it again on every retry.",
    ),
    rt(
        INPUT_IDEMPOTENCY_KEY_UNSUPPORTED,
        400,
        false,
        "idempotency key not supported",
        "An `Idempotency-Key` was sent to an intent that is not declared idempotent.",
        "Omit the header for this intent.",
    ),
    rt(
        INPUT_INVALID,
        400,
        false,
        "input is invalid",
        "The input does not match the intent's parameter types, bounds, upload rules, cursor or page limits, or a database value was rejected. `path` points at the field.",
        "Correct the field named by `path` and send a new call.",
    ),
    rt(
        INPUT_REFERENCE_NOT_FOUND,
        422,
        false,
        "referenced row does not exist",
        "A write points at a row that does not exist (foreign key violation).",
        "Create the referenced row first or use an id that exists.",
    ),
    rt(
        INTERNAL,
        500,
        false,
        "internal error",
        "An unexpected failure in the runtime (an unmapped database error, an object store failure, rules that never settle). Details are logged on the server, not returned.",
        "Report it with the intent name and time; retrying does not help until the server is fixed.",
    ),
    rt(
        INVARIANT_VIOLATED,
        409,
        false,
        "invariant violated",
        "A cardinality constraint, invariant or check of the entity would be false after the write. `reason` names it.",
        "Change the write so the invariant holds; repeating the same call fails again.",
    ),
    rt(
        MIGRATION_CHANGED,
        500,
        false,
        "an applied migration was edited",
        "A `migration` already ran in this database (it is recorded in `_aip_migration` with the digest of the body that ran), but the body in the program now is different. Reported at startup, by `aip migrate` and `aip run`, and the server does not start: what ran is what the data went through, so an applied migration cannot be changed.",
        "Restore the body that ran, or leave the migration as it was and add a new migration that does the further change.",
    ),
    rt(
        MIGRATION_FAILED,
        500,
        false,
        "a migration failed",
        "A `migration` statement failed. Its transaction is rolled back and nothing is recorded, so the next start runs it again from the beginning; later migrations did not run and the server does not start. Reported at startup, by `aip migrate` and `aip run`.",
        "Fix the migration's body (the message carries the database's reason) or the data it trips over, and start again.",
    ),
    rt(
        SCHEMA_DATA_CONFLICT,
        500,
        false,
        "existing rows break a new rule",
        "A schema change (a new unique or check rule, a field made required, an enum value taken away, a narrower type) cannot be applied because rows already in the database do not satisfy it. The message gives how many rows and the ids of some of them. Nothing was changed. Reported by `aip migrate`, `aip migrate --plan` and at the start of `aip run`.",
        "Fix or remove those rows (for example with a `migration` in the deployment before this one), then deploy the stricter rule. A `migration` runs after the schema change of its own deployment, so a rule that depends on its result goes into the next deployment.",
    ),
    rt(
        SCHEMA_ENCRYPTION_CHANGE,
        500,
        false,
        "encryption of a field changed",
        "A field became `encrypted`, stopped being `encrypted`, or an encrypted field or its entity was renamed. Stored values would stay in the old form (plaintext, or ciphertext bound to the old name), so reads would fail or expose ciphertext. Nothing was changed.",
        "Add the field in its new form under a new name, copy the values with a `migration`, and remove the old field with `removed field` in a later deployment.",
    ),
    rt(
        SCHEMA_FAILED,
        500,
        false,
        "a schema statement failed",
        "A statement of the schema change plan failed in PostgreSQL. The whole change ran in one transaction and was rolled back; the database is as it was. Reported by `aip migrate` and at the start of `aip run`.",
        "Read the database's message in the report; it names the statement. Change the program so the plan does not contain it, or fix what the database objects to, and start again.",
    ),
    rt(
        SCHEMA_TYPE_CHANGE,
        500,
        false,
        "field type change is not applied automatically",
        "A field changed type in a way that can lose or reinterpret stored values (another base type, another enum, a narrower length or range than the rows hold, a changed pattern). Only widenings are applied by themselves: a longer limit, a wider numeric range, a text type that accepts more. Nothing was changed.",
        "Add a new field of the new type, copy the values with a `migration`, and drop the old field with `removed field` in a later deployment.",
    ),
    rt(
        SCHEMA_UNDECLARED,
        500,
        false,
        "removal or rename must be declared",
        "A field or entity of the deployed version is not in the program, and nothing says whether it was removed or renamed. Dropping a column destroys its data and a rename keeps it, so the program has to state which one was meant. Nothing was changed.",
        "Renamed: write `was <oldName>` after the new field (`entity New was Old` for an entity). Removed: write `removed field <name>` inside the entity (`removed entity <Name>` at top level).",
    ),
    rt(
        SCHEMA_UNRECORDED,
        500,
        false,
        "database has no deployment record",
        "The database already has tables, but no deployment is recorded in `_aip_deployment` and the tables do not match the program, so there is nothing to compare the program with. This happens with a database that was created before deployments were recorded.",
        "Use `aip migrate --reset` on a development database. For data that must stay, bring the schema in line by hand once; the next start with a matching schema records the deployment and later changes are planned from it.",
    ),
    rt(
        SCHEMA_UNSUPPORTED,
        500,
        false,
        "schema change is not supported",
        "The change cannot be done by the automatic planner: an entity trait that owns tables (`history`, `publishable`, `tenant via`, `dynamic schema from`) was added or removed, or a field changed its kind (stored, reference, inverse, counter). Nothing was changed.",
        "Create the new shape under new names and move the data with a `migration`, or change the database by hand and start again.",
    ),
    rt(
        NOT_FOUND,
        404,
        false,
        "row not found",
        "A row loaded by id does not exist or is not visible to the caller, or the webhook does not exist. `reason` is `<ENTITY>_NOT_FOUND`; `path` is the parameter.",
        "Use an id of an existing, visible row.",
    ),
    rt(
        PRECONDITION_FAILED,
        409,
        false,
        "precondition failed",
        "A `require ... else CODE`, a lifecycle transition rule, a lookup that must exist or an erase restriction failed. `reason` is the declared code.",
        "Make the state satisfy the precondition and send a new call.",
    ),
    rt(
        RATE_LIMITED,
        429,
        true,
        "rate limit reached",
        "The intent's rate limit (`n` calls per `per_seconds`) is used up for this caller.",
        "Wait and send the same call again with the same idempotency key.",
    ),
    rt(
        REQUEST_MALFORMED,
        400,
        false,
        "request is malformed",
        "The HTTP body is not valid JSON or multipart, is too large, or a webhook payload lacks its event id or type.",
        "Send a well-formed request; the message names the part.",
    ),
    rt(
        REQUEST_UNKNOWN_INTENT,
        404,
        false,
        "unknown intent",
        "No public intent has the requested name; internal intents are not callable.",
        "Use a name from the contract (`aip contract`).",
    ),
    rt(
        SUBSCRIPTION_LIMIT,
        429,
        true,
        "too many subscriptions",
        "Sent on a subscription WebSocket: the connection already holds its maximum number of open subscriptions, or the server holds its maximum over all connections, or there are too many connections. Only the subscription that was refused is affected; the others keep running.",
        "Close a subscription (`unsubscribe`) or a connection, then subscribe again after a short delay.",
    ),
    rt(
        SUBSCRIPTION_TOO_LARGE,
        413,
        false,
        "subscription result too large",
        "Sent on a subscription WebSocket: the rows of the subscription no longer fit the maximum a subscription may carry (a whole result is sent on every change). The subscription is closed.",
        "Narrow the subscription with a `where` condition on a parameter, or read the rows with a paged query instead.",
    ),
    rt(
        TENANT_MISMATCH,
        404,
        false,
        "rows belong to different tenants",
        "Rows the call touches belong to different tenants: parameters of one call, a reference between tenant-scoped rows, a value written into a tenant that is not the call's, or, for work without a caller (a webhook, an event handler, a schedule item, a rule row, a job item), a second tenant written in the same transaction. The database refuses the second tenant unless the declaration is `cross tenant`. It answers 404 like NOT_FOUND so a caller cannot tell another tenant's row from a missing one by the status. Rows an actor may not see are NOT_FOUND before this is reached.",
        "Pass rows of one tenant, or reference only rows of the row's own tenant.",
    ),
    rt(
        UNAVAILABLE,
        503,
        true,
        "temporarily unavailable",
        "The runtime cannot use an external dependency or resource right now (for example the database pool or connection). It may work again shortly, so the call is not at fault.",
        "Send the same call again with the same idempotency key after a short delay.",
    ),
];
