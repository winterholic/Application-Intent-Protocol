# AIP 진단·오류 코드

> 이 문서는 `crates/aip-ir/src/codes.rs`의 레지스트리에서 생성된다. 손으로 고치지 않는다. 다시 만들려면 `aip diagnostics --markdown > spec/diagnostics.md`를 실행한다.

## 코드 체계

| 접두 | 뜻 | 발생 지점 |
|---|---|---|
| `AIP-E***` | 오류. 프로그램이 컴파일되지 않는다 | `aip check`, 백엔드 계획 단계 |
| `AIP-W***` | 경고. 컴파일되지만 의도와 다를 가능성이 크다 | `aip check`, 백엔드 계획 단계 |
| `AIP-I***` | Core IR 구조 위반. 프런트엔드가 만든 IR이 구조 규칙을 어긴다 | IR 검증 |
| `AIP.*` | 런타임·프로토콜 오류. 호출자가 응답으로 받는다 | `aip run` |

번호의 앞자리는 영역을 가리킨다. 1xx는 선언과 이름, 2xx는 타입과 값, 3xx는 권한·개인정보·정합성, 4xx는 품질 경고, 5xx는 extension, 6xx는 백엔드다. 번호는 영역 안에서 비어 있을 수 있고 재사용하지 않는다.

## 안정성 규칙

- 한 번 공개된 코드는 의미를 바꾸지 않는다. 더 좁거나 다른 의미가 필요하면 새 코드를 만든다.
- 더 이상 발생하지 않는 코드는 지우지 않고 `deprecated`로 표시한다.
- 메시지 문장은 계약이 아니다. 기계는 `code`(런타임 오류는 `code`와 `reason`)에 의존한다.
- 런타임 오류의 `retryable`은 같은 호출을 같은 `Idempotency-Key`로 다시 보내도 되는지를 뜻한다.

조회: `aip explain-code <CODE>`.

## 오류 (AIP-E)

### `AIP-E100`

syntax error

- 원인: The source does not match the grammar (`spec/grammar.md`): an unexpected character, a malformed literal, a clause out of order or a missing part. Also used when a query has no `from`/`fetch`.
- 수정: Follow the message and its help text; clause order is fixed (see `spec/grammar.md`).

```aip
query Clubs {
  allow public
  from Club c
  page 20 by keyset
  select { id }
}
```

### `AIP-E101`

duplicate or clashing name

- 원인: Two declarations, enum values, parameters, selections, assignments or webhook handlers use the same name in one scope, a name shadows another name or a built-in type, or an intent a form generates (`PublishArticle`, `GiveTermsConsent`, `StartMemberImpersonation`, ...) has the name of a declared one.
- 수정: Rename or remove one of them; a name means exactly one thing in its scope.

```aip
enum Status { ACTIVE ENDED }
entity Club { name: Text }
```

### `AIP-E102`

unknown type or entity

- 원인: A type or entity name (in a type position, `from`, a reference or the actor declaration) is not declared.
- 수정: Declare the type or entity, or fix the spelling; the help text lists what exists.

```aip
entity Club { name: Text  fee: Money(KRW) }
```

### `AIP-E103`

unknown field or predicate

- 원인: A field or predicate is read or written on an entity that does not have it.
- 수정: Use a field the entity declares; the help text lists the entity's fields.

```aip
command Rename(club: Club, name: Text) {
  allow authenticated
  do { set club.name = name }
}
```

### `AIP-E104`

unknown name

- 원인: A name does not refer to a parameter, binding, function, relation, search, intent, flag, partition, event or webhook source that is in scope or known.
- 수정: Declare the name or correct the spelling; the help text suggests close matches.

```aip
relation isOwner(m: Member, c: Club) = exists membership(m, c)
```

### `AIP-E105`

implicit field redeclared

- 원인: The entity declares a field whose name the runtime already maintains implicitly (for example `version` on a versioned or history entity).
- 수정: Remove the declaration, or rename the field.

```aip
entity Club { name: Text }
```

### `AIP-E106`

invalid inverse relation

- 원인: `via` is used on a field that is not an inverse relation, an inverse relation field has no `via <backref field>`, or the named field of the other entity is not a reference back to this entity.
- 수정: Write `via` only on `Entity[]` fields and name a reference field of the other entity that points back; the help text shows the declaration to add.

```aip
entity Item { parent: Order }
entity Order { items: Item[] via parent }
```

### `AIP-E107`

actor declared more than once

- 원인: An application has exactly one actor; a second `actor` declaration is ambiguous about who `actor` refers to.
- 수정: Delete the extra `actor` declaration; model different roles as an enum field on the actor entity.

```aip
use auth
actor Member via auth.oidc(kakao)
entity Member personal { nickname: Text }
```

### `AIP-E108`

type written with wrong options

- 원인: A type that needs options has none (`Money` without a currency) or a type that takes no options was given some.
- 수정: Write the type in its standard form such as `Money(KRW)`, and drop the options from types that take none.

```aip
entity Club { name: Text  fee: Money(KRW) }
```

### `AIP-E110`

actor not declared

- 원인: `actor`, `authenticated`, `visible to actor`, `publishable by` or `impersonate` is used but the application declares no actor.
- 수정: Declare the actor once at the top of the file (`actor <Entity> via ...`).

```aip
use auth
actor Member via auth.oidc(kakao)
entity Member personal { nickname: Text }
```

### `AIP-E111`

evolution declaration contradicts the program

- 원인: `was old` (on a field or an entity) or `removed field x` / `removed entity X` says how a deployed database moves to this program, so it must agree with the program: a field cannot be `was` its own name or the name of a field that is still declared, a name cannot be both renamed away and removed, two fields cannot continue the same old field, a removed field or entity cannot still be declared, and `was` only applies to entity fields.
- 수정: Delete the declaration that no longer applies, or the field or entity it contradicts. Once every database has been migrated, the declaration may stay: it does nothing when the old name is not in the database.

```aip
entity Task {
  title: Text was name
  removed field legacyCode
}
removed entity OldThing
```

### `AIP-E201`

type mismatch

- 원인: An expression, argument, default, assignment or declaration has a type the context does not accept (wrong operand types, wrong collection element, null for a required field, a non-Bool condition, and similar).
- 수정: Make both sides the same type; convert or aggregate the value, or change the declared type.

```aip
command Promote(t: ClubMember) {
  allow authenticated
  do { set t.role = MANAGER }
}
```

### `AIP-E202`

not a value of the enum

- 원인: An identifier used as an enum value is not one of the enum's values (or belongs to several enums and is ambiguous).
- 수정: Use one of the listed values, or compare against a typed value such as `ClubRole.MANAGER` when the name is shared.

```aip
enum ClubRole ordered { GENERAL < MANAGER < ADMIN }
```

### `AIP-E203`

field cannot be assigned

- 원인: The field is implicit, an inverse relation or a counter, which the runtime maintains, so `set`, `insert` or `expose` cannot write it.
- 수정: Remove the assignment; counters change through `touch`.

### `AIP-E204`

wrong use of a field kind or parameter kind

- 원인: `+=`/`-=` need a numeric field and `touch` needs a counter field; the operation is applied to a field of another kind.
- 수정: Use the operation on the kind of field it is made for.

```aip
use redis
entity Post { views: Int counter via redis }
```

### `AIP-E205`

required fields missing on insert

- 원인: `insert` (or `expose create`) does not set every required field that has no default or generator.
- 수정: Assign each missing field named in the message, or give the field a default or make it optional.

```aip
command NewClub(name: Text) {
  allow authenticated
  do { insert Club { name } }
}
```

### `AIP-E206`

invalid `each` loop

- 원인: `each` needs a bounded collection input and an item name.
- 수정: Declare the input as a bounded list and write `each items x partial { ... }`.

### `AIP-E207`

missing unique constraint for toggle or upsert

- 원인: `toggle` and `upsert` find the existing row through a unique constraint; without a matching one the statement is ambiguous.
- 수정: Add `unique (...)` over exactly the key columns.

```aip
entity Like {
  club: Club
  member: Member on erase cascade
  unique (club, member)
}
```

### `AIP-E208`

construct not allowed in this context

- 원인: A valid construct is used where its context does not allow it: an `Upload` parameter outside a command, `this` outside an entity declaration, a `drafts` query that reads no publishable entity or is `allow public`, a `consent` that lists something a caller cannot invoke (an `internal` intent, a form name), an `impersonate` of an entity other than the actor, or `signingSecret` read anywhere but the `returns` of the (not idempotent) command that inserted the endpoint row.
- 수정: Move the construct to a place that allows it (`Upload` parameters go on a command; `this` is only written inside an entity; `drafts` goes on a query of a publishable entity with its own `allow`; `consent` lists declared commands, queries and jobs; `impersonate` names the actor entity; `signingSecret` is read as `returns ep { secret: ep.signingSecret }` after `insert Endpoint { ... } as ep`).

### `AIP-E209`

invalid assignment target

- 원인: An assignment or `into` target is not a field of a row.
- 수정: Assign to `row.field`; derived values cannot be targets.

```aip
command Rename(club: Club, name: Text) {
  allow authenticated
  do { set club.name = name }
}
```

### `AIP-E210`

empty or below the minimum

- 원인: A construct that needs at least one part has none: an `in` list without values, an enum without values, an approval that requires fewer than 1 approval, a `consent` below version 1, or an `impersonate` with a `ttl` under one second.
- 수정: Add at least one value (or raise the number to 1 or more).

```aip
enum Status { ACTIVE ENDED }
entity Club { name: Text }
```

### `AIP-E211`

wrong number of arguments

- 원인: A function, relation or built-in is called with more or fewer arguments than it declares.
- 수정: Pass exactly the declared number of arguments; the message names the expected count.

```aip
relation memberCount(c: Club) = count(c.members)
```

### `AIP-E212`

form not allowed on this construct

- 원인: A clause or shape that the construct does not take is written: a `: body` on `count`, `same()` without a binder, a spread in an event, `insert ... from` bound with `as`, or approvers without an alias.
- 수정: Write the construct in the form the message names (for example `count(items x where ...)`, one event field per name, an alias on approvers).

### `AIP-E213`

multi-valued where one value is needed

- 원인: A set-valued expression (a relation or collection) is used where a single value is required.
- 수정: Aggregate it (`count`, `sum`, `same`, `the`) or bind one element.

```aip
relation memberCount(c: Club) = count(c.members)
```

### `AIP-E214`

invalid webhook option

- 원인: A webhook option is unknown, is not written as a named string literal, or the required `secret` of `http.webhook` is missing.
- 수정: Write options as `name: "value"` with one of secret, header, event, id, payload; `http.webhook` needs `secret: "<ENV VAR NAME>"` (the secret itself never goes in source).

### `AIP-E216`

event emitted with different shapes

- 원인: The same event is emitted with a different field set or types in two places, so consumers cannot rely on its shape.
- 수정: Emit the event with the same fields everywhere, or give the second shape its own event name.

### `AIP-E220`

selection on a non-relation or relation without selection

- 원인: A sub-selection is written on a scalar field, or a relation field is selected without saying which of its fields to return.
- 수정: Select scalars directly; write `relation { fields }` for relations.

### `AIP-E221`

incomplete sort cases

- 원인: `sort by <param> of { ... }` does not cover every value of the enum parameter.
- 수정: Add a case for each enum value listed in the message.

### `AIP-E222`

more than one generator on a field

- 원인: A field has several value generators (sequence, slug, position, counter); the IR keeps one meaning per field.
- 수정: Keep one generator and model the other in a separate field.

```aip
entity Ticket {
  club: Club
  code: Text sequence per club format "T-{n}"
}
```

### `AIP-E301`

missing `allow`

- 원인: Every intent and every exposed operation must say who may call it; there is no default. A `publishable` entity says who may publish it with `publishable by <condition>`, or leaves it to the superuser, and the program must declare one of them.
- 수정: Add `allow public`, `allow authenticated` or an `allow <condition>` clause.

```aip
command Rename(club: Club, name: Text) {
  allow authenticated
  do { set club.name = name }
}
```

### `AIP-E302`

snapshot entity has no dynamic schema

- 원인: `Json validated by <field>` names a `Snapshot<Entity>` field, but that entity declares no `dynamic schema from ...`, so there is nothing to validate against.
- 수정: Add `dynamic schema from <List<Record> field>` to the entity.

### `AIP-E306`

`self` outside actor visibility

- 원인: `self` is only valid in field visibility and masking rules of the actor entity.
- 수정: Use a named parameter or `actor` in other places.

### `AIP-E309`

personal data reference without erase policy

- 원인: A field that references personal data does not say what happens when that data is erased, or an anonymized field is not optional.
- 수정: Add `on erase cascade`, `on erase anonymize` (optional field) or `on erase restrict`.

```aip
entity Comment {
  author: Member? on erase anonymize
  body: Text
}
```

### `AIP-E310`

erase on a non-personal entity

- 원인: `erase` applies to entities declared `personal`.
- 수정: Declare the entity `personal`, or delete the row instead of erasing it.

```aip
entity Member personal { nickname: Text }
```

### `AIP-E311`

ambiguous notification recipient

- 원인: `notify` cannot tell which reference of the row is the recipient because the entity has several references to the actor.
- 수정: Notify a set of actor rows, or an entity with exactly one actor field.

### `AIP-E312`

validated by does not name a snapshot field

- 원인: `Json validated by <field>` must name a `Snapshot<Entity>` field. A plain entity reference (or any other field) can change later, so old answers would lose the schema they were written against.
- 수정: Declare the field as `Snapshot<Entity>` and point `validated by` at it.

### `AIP-E313`

unusable tenant path

- 원인: `tenant via <path>` must be a chain of required references that starts at the entity and ends at the entity that is the tenant (for example `Workspace`). The path names a field that does not exist, is not a reference, is optional, ends at another tenant-scoped entity, or two entities end at different tenants.
- 수정: Write the path as required reference fields, and end every tenant path at the same entity.

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

### `AIP-E314`

tenant-scoped rows read without a tenant

- 원인: The intent, or the form (approval, job, grant link, verification, subscription), reads rows of a tenant-scoped entity (`from`, a set expression, an update or delete target, an inverse relation of something that is not tenant-bound) but has no required parameter that fixes the tenant. A read that names no tenant would span all of them, and the only way to opt out is `internal cross tenant` on an intent. Event handlers, schedule items and rules are anchored by the row they are about; so are the endpoints of an `outbound webhooks` form, by the event, which therefore has to reference a tenant-scoped row. Webhooks, consumers and retention have no anchor and are not checked here: their writes are held to one tenant by the database instead.
- 수정: Add a required parameter of the tenant entity or of a tenant-scoped entity (`workspace: Workspace`), or make the intent `internal cross tenant`.

```aip
entity Workspace { name: Text }
entity Project {
  workspace: Workspace
  title: Text
  tenant via workspace
}
query Projects(workspace: Workspace) {
  allow authenticated
  from Project p
  page 20 by keyset
  select { id title }
}
```

### `AIP-E315`

cross tenant on an intent that is not internal

- 원인: `cross tenant` lifts the single-tenant rules, so on an intent it is only allowed together with `internal`, which clients cannot call. A public intent that works across tenants would let any caller read or write other tenants. Declarations that have no caller (`on Event`, `schedule`, `rule`, `consume` and a webhook handler) may be written `cross tenant` without `internal`.
- 수정: Write `internal cross tenant`, or remove `cross tenant` and give the intent a parameter that fixes its tenant.

```aip
entity Workspace { name: Text }
internal cross tenant query AllWorkspaces() {
  allow public
  from Workspace w
  page 50 by keyset
  select { id name }
}
```

### `AIP-E316`

invalid search declaration

- 원인: A `search` names a language the runtime has no text analysis for, a weight other than A, B, C or D, a field that is not text (Text, RichText, Email, Url, Phone), the same field twice, or no field at all. A search over an entity with `publishable` is refused too, because readers see the published copy and the index is on the working table.
- 수정: Use one of the supported languages, weights A to D, and text fields of the entity, each once.

```aip
entity Post {
  title: Text(1..100)
  body: Text(1..4000)
}
search PostSearch on Post fields [title weight A, body weight B] language english
```

### `AIP-E317`

invalid outbound webhooks declaration

- 원인: An `outbound webhooks` form names a signature scheme other than `hmac_sha256`, a retry count above 16, a window (`over`) or a `disable after` that is not positive, no event, or an endpoint entity without a `url: Url` field to deliver to. Two forms for one entity are refused too, because an endpoint belongs to one subscription.
- 수정: Use `sign hmac_sha256`, `retry` from 0 to 16, positive durations, at least one event, and a `url: Url` field on the endpoint entity; declare one form per endpoint entity.

```aip
entity Endpoint {
  url: Url
}
event Paid { id: Uuid }
outbound webhooks for Endpoint e {
  events [Paid]
  sign hmac_sha256 retry 8 over 24h disable after 3d failing
}
```

### `AIP-E318`

invalid encrypted field

- 원인: An `encrypted` field is stored as ciphertext the database cannot compute on. It must be a plain stored text field (Text, Email, Url, Phone), must not carry a `default` or a generated value (the default would be stored in the clear), and cannot be part of a `unique`, `no overlap` or `at most one` declaration or of a `search`, because the database would compare, index or search the ciphertext, which is random for every value.
- 수정: Keep `encrypted` on text fields only, remove the default, and take the field out of unique, search and the other declarations; look a row up by something that is not encrypted (an id, or a separate field such as a hash you compute and store yourself).

```aip
entity Member {
  email: Email personal encrypted
  nickname: Text(2..20)
  unique (nickname)
}
```

### `AIP-E319`

encrypted field used in an expression

- 원인: The runtime encrypts a value just before it is bound to a statement and decrypts a result just before it is returned, so the database never sees the plaintext of an `encrypted` field. An expression that makes the database read it (a comparison, a filter, `sort by`, an aggregate, a `group by`, a condition, a computed value, an event or notification payload, a copy into another field or row, an `exists` over it) would work on ciphertext and silently give wrong answers.
- 수정: Select the field by name in a `select` or `returns` of the same row, or write it from a parameter with `set row.field = param`. To filter or sort by it, keep a separate, unencrypted field for that purpose.

```aip
entity Member {
  email: Email personal encrypted
  nickname: Text(2..20)
}
query MemberByNickname(nickname: Text) {
  allow authenticated
  from Member m where m.nickname = nickname
  page 20 by keyset
  select { id nickname email }
}
```

### `AIP-E320`

encrypted field cannot be written this way

- 원인: The ciphertext of a row is bound to the row's id, so a value is encrypted for exactly one row. The value written to an `encrypted` field has to be a parameter (or a value the program already received), and the write has to name one row: `insert`, or `set row.field = value`. An `update` or `upsert` over many rows, an `insert ... from` a set, a `toggle`, or a value computed from other data cannot be encrypted per row.
- 수정: Write the field with `set row.field = param` on one row, or insert the row with the parameter as the value; do the work row by row with `each`.

```aip
entity Member {
  email: Email personal encrypted
  nickname: Text(2..20)
}
command ChangeEmail(member: Member, email: Email) {
  allow actor = member
  do { set member.email = email }
}
```

### `AIP-E501`

extension not declared or misused

- 원인: A name, type, effect, broker, webhook source or provider belongs to an extension the application did not declare with `use`.
- 수정: Add `use <extension>` at the top of the file.

```aip
use s3
entity Photo { file: s3.Object }
```

### `AIP-E502`

call is not an extension effect

- 원인: A statement is a call without a namespace; only extension effects (`ns.effect(...)`) can stand alone as statements.
- 수정: Write the effect as `ns.effect(...)`, or use the call inside an expression.

### `AIP-E600`

expression cannot be planned

- 원인: The PostgreSQL backend cannot turn an expression into SQL (an unsupported construct or an unresolved type). Reported by the backend with a path instead of a source position.
- 수정: Rewrite the expression with supported constructs; see the message for the construct.

### `AIP-E601`

statement cannot be planned

- 원인: The PostgreSQL backend cannot plan a statement or intent. Reported with a path instead of a source position.
- 수정: Simplify the statement; see the message for the unsupported part.

### `AIP-E602`

checked but not executable yet

- 원인: The frontend accepts the construct (a consumer, a projection, a search, a subscription or another form) but this runtime has no executable plan for it, so it is refused instead of silently ignored.
- 수정: Remove the construct, or wait for a runtime that supports it.

## 경고 (AIP-W)

### `AIP-W402`

redundant comparison or call

- 원인: Both sides of a comparison are the same expression, or a function is called with the same expression twice, which is almost always a typo.
- 수정: Compare two different values.

```aip
relation sameClub(a: ClubMember, b: ClubMember) = a.club = b.club
```

### `AIP-W403`

unbounded list query

- 원인: A query returns every row; the result grows with the data and cannot be paged by clients safely.
- 수정: Add `page N by keyset` (or `by offset`) to the query.

```aip
query Clubs {
  allow public
  from Club c
  page 20 by keyset
  select { id }
}
```

### `AIP-W404`

schedule with time of day but no timezone

- 원인: A schedule that fires at a time of day without a timezone depends on the server's zone.
- 수정: Add `tz "Asia/Seoul"` (or another IANA zone) to the schedule.

### `AIP-W501`

non-idempotent command creates rows

- 원인: A command inserts rows and binds them without an `idempotent` declaration, so a client retry creates duplicates.
- 수정: Add `idempotent` so callers send an `Idempotency-Key`, or rely on a unique constraint.

### `AIP-W502`

extension is not first-party

- 원인: The extension named in `use` is not on the toolchain's first-party list, so the compiler has no knowledge of its effects.
- 수정: Use a first-party extension, or accept that its effects are unchecked.

### `AIP-W602` (deprecated)

declared but not enforced by this runtime

- 원인: The runtime accepts the declaration but does not implement it, so the stated guarantee does not hold. No declaration is in this state any more: `encrypted` used to be the one, and now the runtime encrypts it.
- 수정: Treat the guarantee as absent until the runtime supports it.

### `AIP-W603`

search language is approximated

- 원인: The `search` is declared with a language whose words the engine cannot reduce to stems (`korean`). It matches by word prefix: a query word finds the words that begin with it, so a particle or ending after the stem is tolerated, but a changed stem, a compound word or a match in the middle of a word is not found. This is not morphological analysis (open issue OI-08).
- 수정: Accept prefix matching, or run a search engine with a Korean analyzer and keep this declaration for development.

## IR 구조 (AIP-I)

### `AIP-I101`

version tag differs

- 원인: The program's `aip_core` tag is not the version this toolchain reads.
- 수정: Regenerate the IR with a frontend that targets `CORE_IR_VERSION`.

### `AIP-I102`

unknown enum in a type

- 원인: A type names an enum that is not in the program.
- 수정: Add the enum or fix the type name.

### `AIP-I103`

unknown record in a type

- 원인: A type names a record that is not in the program.
- 수정: Add the record or fix the type name.

### `AIP-I104`

unknown entity in a type

- 원인: A `Ref`, `RefUnion` or `Snapshot` type names an entity that is not in the program.
- 수정: Add the entity or fix the type name.

### `AIP-I105`

zero bound

- 원인: A `Set` or `List` bound is zero.
- 수정: Use a bound of at least one.

### `AIP-I106`

reference to unknown entity

- 원인: A reference field targets an entity that does not exist.
- 수정: Add the entity or fix the target.

### `AIP-I107`

invalid inverse field

- 원인: An inverse field targets a missing entity or a missing `via` field.
- 수정: Point it at an existing entity and reference field.

### `AIP-I108`

unknown field expression

- 원인: A `Field` expression names a missing entity or field.
- 수정: Name an entity and field that exist.

### `AIP-I109`

unknown row field

- 원인: A `RowField` names a missing entity or field.
- 수정: Name an entity and field that exist.

### `AIP-I110`

unknown enum value

- 원인: An `EnumValue` names a missing enum or value.
- 수정: Name an enum and value that exist.

### `AIP-I111`

unknown callee

- 원인: A call names a missing function, relation or intent.
- 수정: Declare the callee or fix the name.

### `AIP-I112`

unknown write target

- 원인: An insert, upsert or toggle names a missing entity or field.
- 수정: Name an entity and fields that exist.

### `AIP-I113`

invalid lifecycle

- 원인: A lifecycle names a missing field, a non-enum field or a missing enum value.
- 수정: Point it at an enum field and its values.

### `AIP-I114`

unknown event

- 원인: A reaction, emit, projection, upcast or event type names a missing event.
- 수정: Declare the event or fix the name.

### `AIP-I115`

unknown predicate

- 원인: A predicate application names a missing entity or predicate.
- 수정: Declare the predicate on the entity or fix the name.

### `AIP-I116`

unknown entity reference

- 원인: An entity reference (actor, source, rule, retain, type test and similar) names a missing entity.
- 수정: Add the entity or fix the name.

### `AIP-I117`

constraint on unknown field

- 원인: A constraint names a field the entity does not have.
- 수정: Name a field the entity declares.

### `AIP-I118`

unknown record field

- 원인: A `RecordField` names a missing record or field.
- 수정: Name a record and field that exist.

### `AIP-I119`

counter store not declared

- 원인: A counter `store` is not an extension listed in `uses`.
- 수정: Add the extension to `uses` or change the store.

### `AIP-I120`

duplicate constraint ordinal

- 원인: Two constraints of one entity share an ordinal.
- 수정: Give each constraint its own ordinal.

### `AIP-I121`

validated-by field missing

- 원인: `Json validated by` names a field the entity does not have.
- 수정: Name a field of the entity.

### `AIP-I122`

search missing or misused

- 원인: A query's `Search` source names a search declaration that is not in the program, or a `SearchRank` node names an alias that no `Search` source of its query binds.
- 수정: Declare the search, or use the rank only in the query that searches under the alias.

## 런타임 오류 (AIP.*)

### `AIP.AUTH.FORBIDDEN`

caller is not allowed

- 원인: The caller is authenticated but the intent's `allow` condition (or an approval rule) is false. `reason` carries the declared code, if any.
- 수정: Call with an actor that satisfies the condition; retrying with the same actor gives the same answer.
- HTTP 상태: 403
- 재시도: 불가

### `AIP.AUTH.UNAUTHENTICATED`

no valid caller

- 원인: The intent needs an actor and the request has no valid token, actor header or webhook signature.
- 수정: Send a valid actor token (`aip token`) or the correct webhook signature.
- HTTP 상태: 401
- 재시도: 불가

### `AIP.CONCURRENCY.CONFLICT`

concurrent update, retry

- 원인: The database aborted the transaction because of a serialization failure or deadlock. The engine already retried a few times.
- 수정: Send the same call again with the same idempotency key.
- HTTP 상태: 409
- 재시도: 가능

### `AIP.CONFLICT.CAPACITY`

capacity exceeded

- 원인: A `capacity` constraint of the entity would be exceeded by the write. `reason` is the declared code.
- 수정: Free capacity or choose another slot; the same call fails again until the state changes.
- HTTP 상태: 409
- 재시도: 불가

### `AIP.CONFLICT.IN_USE`

row is still referenced

- 원인: Deleting a row failed because other rows still reference it and no `on delete` rule removes them.
- 수정: Remove or re-point the referencing rows first, or declare an `on delete` rule.
- HTTP 상태: 409
- 재시도: 불가

### `AIP.CONFLICT.OVERLAP`

ranges overlap

- 원인: A `no overlap` constraint would be violated: the new range intersects an existing one. `reason` is the declared code.
- 수정: Choose a range that does not overlap.
- HTTP 상태: 409
- 재시도: 불가

### `AIP.CONFLICT.STALE_VERSION`

row changed since it was read

- 원인: The command carries a `version` check and the row's version moved on. `reason` names the check.
- 수정: Reload the row, show the user the new state, then send a new call with a new idempotency key.
- HTTP 상태: 409
- 재시도: 불가

### `AIP.CONFLICT.UNIQUE`

unique value already taken

- 원인: A `unique` constraint or generated unique value (slug, sequence) is already in use. `reason` is the declared code.
- 수정: Choose another value; repeating the same call fails again.
- HTTP 상태: 409
- 재시도: 불가

### `AIP.CONSENT.REQUIRED`

required consent is missing

- 원인: The intent is listed in a `consent` declaration and the caller has no active record of consent to that consent's current version (never given, given for an older version, or withdrawn). `reason` is the consent's name. 403, not 409, because the caller is known and refused until the user acts; the state of the call itself is fine.
- 수정: Show the consent text and call `Give<Name>Consent`, then send the original call again; `<Name>ConsentStatus` tells which case it is.
- HTTP 상태: 403
- 재시도: 불가

### `AIP.ENCRYPTION.DECRYPT_FAILED`

encrypted value cannot be decrypted

- 원인: A value stored in an `encrypted` field failed authentication: it was written under a key that is not in `AIP_ENCRYPTION_KEYS`, it was changed in the database, or it was copied from another row or column (the ciphertext is bound to its entity, field and row id). Nothing is returned for that call; the message names the field and the key id, never the value.
- 수정: Add the key the value was written with to `AIP_ENCRYPTION_KEYS` (after the current one). If the value was moved or edited in the database, restore it from a backup; a value cannot be recovered without its key.
- HTTP 상태: 500
- 재시도: 불가

### `AIP.ENCRYPTION.KEYS_MISSING`

encryption keys are missing or unusable

- 원인: The program has `encrypted` fields, but `AIP_ENCRYPTION_KEYS` is not set or is not a list of `<id>:<base64 of 32 bytes>` entries separated by commas. The server does not start (and `aip migrate` is not affected): running with plaintext would break the guarantee silently. `AIP_ENCRYPTION_KEYS` is separate from `AIP_SECRET`.
- 수정: Generate a key with `openssl rand -base64 32` and set `AIP_ENCRYPTION_KEYS=k1:<key>`. The first entry encrypts new values, every entry can decrypt; to rotate, put the new key first and run `aip rekey`.
- HTTP 상태: 500
- 재시도: 불가

### `AIP.IDEMPOTENCY.KEY_REUSED`

idempotency key reused with different input

- 원인: The key was already used with a different input. The same input with the same key returns the stored result instead of this error.
- 수정: Use a new key for a new input, or resend the original input to read the stored result.
- HTTP 상태: 409
- 재시도: 불가

### `AIP.INPUT.IDEMPOTENCY_KEY_REQUIRED`

idempotency key required

- 원인: The command is idempotent and the request has no `Idempotency-Key` header.
- 수정: Send an `Idempotency-Key` header and use it again on every retry.
- HTTP 상태: 400
- 재시도: 불가

### `AIP.INPUT.IDEMPOTENCY_KEY_UNSUPPORTED`

idempotency key not supported

- 원인: An `Idempotency-Key` was sent to an intent that is not declared idempotent.
- 수정: Omit the header for this intent.
- HTTP 상태: 400
- 재시도: 불가

### `AIP.INPUT.INVALID`

input is invalid

- 원인: The input does not match the intent's parameter types, bounds, upload rules, cursor or page limits, or a database value was rejected. `path` points at the field.
- 수정: Correct the field named by `path` and send a new call.
- HTTP 상태: 400
- 재시도: 불가

### `AIP.INPUT.REFERENCE_NOT_FOUND`

referenced row does not exist

- 원인: A write points at a row that does not exist (foreign key violation).
- 수정: Create the referenced row first or use an id that exists.
- HTTP 상태: 422
- 재시도: 불가

### `AIP.INTERNAL`

internal error

- 원인: An unexpected failure in the runtime (an unmapped database error, an object store failure, rules that never settle). Details are logged on the server, not returned.
- 수정: Report it with the intent name and time; retrying does not help until the server is fixed.
- HTTP 상태: 500
- 재시도: 불가

### `AIP.INVARIANT.VIOLATED`

invariant violated

- 원인: A cardinality constraint, invariant or check of the entity would be false after the write. `reason` names it.
- 수정: Change the write so the invariant holds; repeating the same call fails again.
- HTTP 상태: 409
- 재시도: 불가

### `AIP.MIGRATION.CHANGED`

an applied migration was edited

- 원인: A `migration` already ran in this database (it is recorded in `_aip_migration` with the digest of the body that ran), but the body in the program now is different. Reported at startup, by `aip migrate` and `aip run`, and the server does not start: what ran is what the data went through, so an applied migration cannot be changed.
- 수정: Restore the body that ran, or leave the migration as it was and add a new migration that does the further change.
- HTTP 상태: 500
- 재시도: 불가

### `AIP.MIGRATION.FAILED`

a migration failed

- 원인: A `migration` statement failed. Its transaction is rolled back and nothing is recorded, so the next start runs it again from the beginning; later migrations did not run and the server does not start. Reported at startup, by `aip migrate` and `aip run`.
- 수정: Fix the migration's body (the message carries the database's reason) or the data it trips over, and start again.
- HTTP 상태: 500
- 재시도: 불가

### `AIP.SCHEMA.DATA_CONFLICT`

existing rows break a new rule

- 원인: A schema change (a new unique or check rule, a field made required, an enum value taken away, a narrower type) cannot be applied because rows already in the database do not satisfy it. The message gives how many rows and the ids of some of them. Nothing was changed. Reported by `aip migrate`, `aip migrate --plan` and at the start of `aip run`.
- 수정: Fix or remove those rows (for example with a `migration` in the deployment before this one), then deploy the stricter rule. A `migration` runs after the schema change of its own deployment, so a rule that depends on its result goes into the next deployment.
- HTTP 상태: 500
- 재시도: 불가

### `AIP.SCHEMA.ENCRYPTION_CHANGE`

encryption of a field changed

- 원인: A field became `encrypted`, stopped being `encrypted`, or an encrypted field or its entity was renamed. Stored values would stay in the old form (plaintext, or ciphertext bound to the old name), so reads would fail or expose ciphertext. Nothing was changed.
- 수정: Add the field in its new form under a new name, copy the values with a `migration`, and remove the old field with `removed field` in a later deployment.
- HTTP 상태: 500
- 재시도: 불가

### `AIP.SCHEMA.FAILED`

a schema statement failed

- 원인: A statement of the schema change plan failed in PostgreSQL. The whole change ran in one transaction and was rolled back; the database is as it was. Reported by `aip migrate` and at the start of `aip run`.
- 수정: Read the database's message in the report; it names the statement. Change the program so the plan does not contain it, or fix what the database objects to, and start again.
- HTTP 상태: 500
- 재시도: 불가

### `AIP.SCHEMA.TYPE_CHANGE`

field type change is not applied automatically

- 원인: A field changed type in a way that can lose or reinterpret stored values (another base type, another enum, a narrower length or range than the rows hold, a changed pattern). Only widenings are applied by themselves: a longer limit, a wider numeric range, a text type that accepts more. Nothing was changed.
- 수정: Add a new field of the new type, copy the values with a `migration`, and drop the old field with `removed field` in a later deployment.
- HTTP 상태: 500
- 재시도: 불가

### `AIP.SCHEMA.UNDECLARED`

removal or rename must be declared

- 원인: A field or entity of the deployed version is not in the program, and nothing says whether it was removed or renamed. Dropping a column destroys its data and a rename keeps it, so the program has to state which one was meant. Nothing was changed.
- 수정: Renamed: write `was <oldName>` after the new field (`entity New was Old` for an entity). Removed: write `removed field <name>` inside the entity (`removed entity <Name>` at top level).
- HTTP 상태: 500
- 재시도: 불가

### `AIP.SCHEMA.UNRECORDED`

database has no deployment record

- 원인: The database already has tables, but no deployment is recorded in `_aip_deployment` and the tables do not match the program, so there is nothing to compare the program with. This happens with a database that was created before deployments were recorded.
- 수정: Use `aip migrate --reset` on a development database. For data that must stay, bring the schema in line by hand once; the next start with a matching schema records the deployment and later changes are planned from it.
- HTTP 상태: 500
- 재시도: 불가

### `AIP.SCHEMA.UNSUPPORTED`

schema change is not supported

- 원인: The change cannot be done by the automatic planner: an entity trait that owns tables (`history`, `publishable`, `tenant via`, `dynamic schema from`) was added or removed, or a field changed its kind (stored, reference, inverse, counter). Nothing was changed.
- 수정: Create the new shape under new names and move the data with a `migration`, or change the database by hand and start again.
- HTTP 상태: 500
- 재시도: 불가

### `AIP.NOT_FOUND`

row not found

- 원인: A row loaded by id does not exist or is not visible to the caller, or the webhook does not exist. `reason` is `<ENTITY>_NOT_FOUND`; `path` is the parameter.
- 수정: Use an id of an existing, visible row.
- HTTP 상태: 404
- 재시도: 불가

### `AIP.PRECONDITION.FAILED`

precondition failed

- 원인: A `require ... else CODE`, a lifecycle transition rule, a lookup that must exist or an erase restriction failed. `reason` is the declared code.
- 수정: Make the state satisfy the precondition and send a new call.
- HTTP 상태: 409
- 재시도: 불가

### `AIP.RATE.LIMITED`

rate limit reached

- 원인: The intent's rate limit (`n` calls per `per_seconds`) is used up for this caller.
- 수정: Wait and send the same call again with the same idempotency key.
- HTTP 상태: 429
- 재시도: 가능

### `AIP.REQUEST.MALFORMED`

request is malformed

- 원인: The HTTP body is not valid JSON or multipart, is too large, or a webhook payload lacks its event id or type.
- 수정: Send a well-formed request; the message names the part.
- HTTP 상태: 400
- 재시도: 불가

### `AIP.REQUEST.UNKNOWN_INTENT`

unknown intent

- 원인: No public intent has the requested name; internal intents are not callable.
- 수정: Use a name from the contract (`aip contract`).
- HTTP 상태: 404
- 재시도: 불가

### `AIP.SUBSCRIPTION.LIMIT`

too many subscriptions

- 원인: Sent on a subscription WebSocket: the connection already holds its maximum number of open subscriptions, or the server holds its maximum over all connections, or there are too many connections. Only the subscription that was refused is affected; the others keep running.
- 수정: Close a subscription (`unsubscribe`) or a connection, then subscribe again after a short delay.
- HTTP 상태: 429
- 재시도: 가능

### `AIP.SUBSCRIPTION.TOO_LARGE`

subscription result too large

- 원인: Sent on a subscription WebSocket: the rows of the subscription no longer fit the maximum a subscription may carry (a whole result is sent on every change). The subscription is closed.
- 수정: Narrow the subscription with a `where` condition on a parameter, or read the rows with a paged query instead.
- HTTP 상태: 413
- 재시도: 불가

### `AIP.TENANT.MISMATCH`

rows belong to different tenants

- 원인: Rows the call touches belong to different tenants: parameters of one call, a reference between tenant-scoped rows, a value written into a tenant that is not the call's, or, for work without a caller (a webhook, an event handler, a schedule item, a rule row, a job item), a second tenant written in the same transaction. The database refuses the second tenant unless the declaration is `cross tenant`. It answers 404 like NOT_FOUND so a caller cannot tell another tenant's row from a missing one by the status. Rows an actor may not see are NOT_FOUND before this is reached.
- 수정: Pass rows of one tenant, or reference only rows of the row's own tenant.
- HTTP 상태: 404
- 재시도: 불가

### `AIP.UNAVAILABLE`

temporarily unavailable

- 원인: The runtime cannot use an external dependency or resource right now (for example the database pool or connection). It may work again shortly, so the call is not at fault.
- 수정: Send the same call again with the same idempotency key after a short delay.
- HTTP 상태: 503
- 재시도: 가능
