# 제품 정의 진단 코드

> 이 문서는 `spikes/spike-v1-fixture/src/codes.rs`의 레지스트리에서 생성된다. 손으로 고치지 않는다. 다시 만들려면 `AIP_UPDATE_GOLDEN=1 cargo test --manifest-path spikes/spike-v1-fixture/Cargo.toml --test diagnostics`를 실행한다.

`aip service check`(와 정의를 읽는 모든 제품 명령)가 `DEF.INVALID_DEFINITION`과 함께 돌려주는 진단의 코드다. 각 진단은 `code`, `message`, `line`, `column`에 더해 `scope`(위치한 선언 경로), `help`(가까운 철자, 블록이 받는 키, 고치는 방법), `explain`(`aip explain-code <CODE>`)을 가진다. 메시지 문장은 계약이 아니고 코드가 계약이다. 한 번 공개된 코드는 의미를 바꾸지 않는다.

모든 AIP 오류 코드는 `영역.이름` 형식이다. 이 문서의 영역은 `DEF`(제품 정의 검사기)이고, 제품 HTTP API의 `WIRE` 코드는 [wire-errors.md](wire-errors.md), PoC 언어와 런타임의 `POC` 코드는 [diagnostics.md](diagnostics.md)에 있다. 세 영역은 `aip explain-code`와 `aip diagnostics`로 함께 조회한다. 2026-10-13 이전에는 접두 없는 이름(`DEF.UNKNOWN_FIELD`)이었고 제품 wire 코드와 이름이 겹쳤다.

### `DEF.INVALID_DEFINITION`

the definition has errors

- 원인: `aip service check` and every command that loads a definition report this code with a `diagnostics` list; each entry has its own code, line, column and help. Nothing is generated or deployed from a definition with errors.
- 수정: Fix the diagnostics from the first one. Later entries are often consequences of an earlier one (see the `cascade` note in their help); check again after each repair.

### `DEF.SOURCE_TOO_LARGE`

the definition file is larger than the product limit

- 원인: A definition above the byte ceiling is rejected before it is read, so that checking cannot be used to exhaust memory.
- 수정: Split the application into resources of a smaller definition, or remove generated or commented-out text. The limits are listed in spikes/spike-v1-fixture/PRODUCTION-LIMITS.md.

### `DEF.TOKEN_LIMIT`

the definition has more tokens than the product limit

- 원인: A definition with more tokens than the ceiling is rejected at the position of the first token over the limit.
- 수정: Remove repetition: share a condition with a `predicate`, a visibility rule with `access`. The limits are listed in spikes/spike-v1-fixture/PRODUCTION-LIMITS.md.

### `DEF.POLICY_EXPANSION_LIMIT`

a policy path is longer than the expansion budget

- 원인: A row or field policy that follows references through too many fields is rejected, because the server expands it into one query.
- 수정: Shorten the path: put the condition on the nearer resource, or give the intermediate resource its own `predicate`.

### `DEF.LEX_UNEXPECTED_CHAR`

a character that no token starts with

- 원인: The text contains a character the definition language does not use outside of a string.
- 수정: Remove the character, or put it inside a string literal (`"..."`). Comparison is `=` (not `==`); not-equal is `!=`.

### `DEF.LEX_BAD_STRING`

a string literal is not closed or has an unsupported escape

- 원인: Strings are enclosed in double quotes on one line. Only the documented escapes are accepted.
- 수정: Close the string with `"` on the same line and use only the supported escapes.

### `DEF.LEX_NUMBER_RANGE`

an integer is outside the supported range

- 원인: Integers in a definition are 64-bit signed.
- 수정: Use a smaller number, or a `Decimal(p, s)` field when a value does not fit an Int.

### `DEF.LEX_BAD_DURATION`

unknown unit in a duration

- 원인: A duration is an integer followed by one unit.
- 수정: Use `ms`, `s` or `m` after the integer, for example `250ms`, `2s`, `5m`.

```aip
actor Member
resource Member { fields { id: Id; name: Text } }
resource Note {
  fields { id: Id; body: Text; owner: Member }
  expose read { select id, body
    budget { rows 10; depth 1; deadline 2s; cost 100 } }
}
```

### `DEF.LEX_DURATION_RANGE`

a duration does not fit in milliseconds

- 원인: Durations are stored as 64-bit milliseconds.
- 수정: Use a shorter duration; a request deadline above a few minutes is not useful.

### `DEF.PARSE_EXPECTED`

the parser expected another token here

- 원인: The text before this position is valid, but the token found cannot continue the declaration. The message names what was expected and what was found.
- 수정: Write the token the message names at the reported position. A syntax error stops the check, so errors further down appear only after this one is fixed.

```aip
actor Member
resource Member { fields { id: Id; name: Text } }
resource Note {
  fields { id: Id; body: Text(1..500); owner: Member }
}
```

### `DEF.PARSE_UNKNOWN_DECL`

a top-level word that starts no declaration

- 원인: A definition is a list of declarations. The first word of each must be one of the declaration keywords.
- 수정: Start the declaration with `enum`, `actor`, `predicate`, `access`, `limit`, `resource` or `operation`; check the spelling of the word reported. Every declaration is described in spec/definition-syntax.md.

### `DEF.PARSE_UNKNOWN_KEY`

a key that the block does not accept

- 원인: Unknown keys are never ignored silently: a misspelled key would otherwise leave a policy or a limit out of the definition without a sign.
- 수정: Use one of the keys listed in the help. Every block and key is described in spec/definition-syntax.md.

```aip
actor Member
resource Member { fields { id: Id; name: Text } }
resource Note {
  fields { id: Id; body: Text; owner: Member }
  expose read { select id, body; sort id
    budget { rows 10; depth 1; deadline 2s; cost 100 } }
}
```

### `DEF.PARSE_NESTING`

an expression is nested or large beyond the limit

- 원인: Expression depth and node count are bounded so that a definition cannot make the checker or the planner do unbounded work.
- 수정: Name the sub-condition with a `predicate` and call it, instead of nesting it.

### `DEF.WRONG_BINDING`

the definition is not bound the official way in the host file

- 원인: In the embedded forms the block must be written with the official import and exactly one `aip` tagged template (TypeScript) or `aip("""...""")` call (Python).
- 수정: Import the official module and write one block: `export const app = aip` followed by the template. The fixtures spikes/spike-v1-fixture/fixture/recruitment.e.ts and recruitment.e.py show the shape.

### `DEF.NON_LITERAL`

the definition must be a literal in the host file

- 원인: A definition is checked statically, so it cannot be built, concatenated, called or generated at run time in the host language.
- 수정: Write the definition as one literal: no concatenation, function call, variable or conditional around the block.

### `DEF.DYNAMIC_INTERPOLATION`

interpolation inside the definition

- 원인: `${...}` in a TypeScript template and f-strings in Python make the definition depend on run-time values.
- 수정: Remove the interpolation and write the value into the definition text.

### `DEF.ESCAPE_UNSUPPORTED`

an escape sequence that the definition string does not support

- 원인: Host string escapes that would change what the checker reads are rejected.
- 수정: Write the character directly, or use a plain ASCII text.

### `DEF.NO_BLOCK`

the host file has no definition block

- 원인: The host file was loaded as a definition but contains no `aip` block (embedded form) or `define(...)` call (host-literal form).
- 수정: Add one block, or load the correct file. Check the file name: `.e.ts`/`.e.py` are embedded forms, `.h.ts`/`.h.py` host-literal forms.

### `DEF.MULTIPLE_BLOCKS`

more than one definition block in a file

- 원인: One file holds one definition, so there is exactly one contract per file.
- 수정: Keep one block per file; move the other resources into the same block.

### `DEF.HOST_LEX`

the host file cannot be scanned

- 원인: The host-language text has an unclosed string or comment before the definition could be located.
- 수정: Close the string or comment reported.

### `DEF.H_LITERAL_NESTING`

a host literal is nested beyond the limit

- 원인: The host-literal form is a data structure; its depth is bounded.
- 수정: Flatten the literal; move deep conditions into a `predicate`.

### `DEF.H_SHAPE`

a host-literal value has the wrong shape

- 원인: In the host-literal form each key expects an object, string, integer, boolean or list. The message names the key and the shape expected.
- 수정: Replace the value with the shape the message names. The fixtures spikes/spike-v1-fixture/fixture/recruitment.h.ts and recruitment.h.py show every key.

### `DEF.WRONG_LANGUAGE`

a PoC program was given to the product checker

- 원인: The product commands (`aip service check/gen/serve`) read definitions of the product language (`resource`, `expose`, `transition`) in `.aip` files. A file with the extension `.aipoc` is a PoC program (`entity`, `command`, `query`), which has its own checker; reading it here would only report typos in a file that has none.
- 수정: Check a PoC program with `aip check`; if the file is a product definition, rename it to `.aip`.

### `DEF.UNKNOWN_KEY`

a key that the object does not accept (host-literal form)

- 원인: As in `DEF.PARSE_UNKNOWN_KEY`: unknown keys are never ignored.
- 수정: Use one of the keys listed in the help.

### `DEF.MISSING_ITEM`

a required part is missing

- 원인: Some declarations have no safe default, so the definition must say the value: there is no unlimited bulk size, no implicit aggregate source access, no implicit actor. The message names the missing part.
- 수정: Add the part the message names.

```aip
actor Member
resource Member { fields { id: Id; name: Text } }
resource Note {
  fields { id: Id; body: Text; owner: Member }
  rows read when owner = actor
  transition archive {
    from body != ""
    to body = ""
    allow owner = actor
  }
  expose apply archive { target id; bulk maxRows 1 }
}
```

### `DEF.DUPLICATE`

a name or key is declared twice

- 원인: Names are unique in their block: declarations, fields, transitions, aggregates, invariants, parameters. A second declaration would make the first one unreachable.
- 수정: Rename or remove one of the two; the first declaration is the one the message does not point to.

### `DEF.AMBIGUOUS`

a name collides with a managed capability

- 원인: Resource names that equal a managed area of the server (for example messages or objects) would make a route mean two things.
- 수정: Rename the resource.

### `DEF.AMBIGUOUS_NAME`

a name reads as both a field and a variable

- 원인: Inside a predicate a parameter and a field of `this` may share a spelling; the checker will not choose for you.
- 수정: Rename the parameter, or qualify the field with `this.`.

### `DEF.UNKNOWN_TYPE`

a type that is not declared

- 원인: A field or parameter uses a type that is neither built in nor an `enum` or `resource` of this definition. Errors that depend on this field (its enum values, its policies) usually follow and disappear when it is fixed.
- 수정: Spell the type as declared: `Id`, `Text`, `Int`, `Bool`, `Time`, `Date`, `Url`, `Email`, `Decimal(p,s)`, an `enum` or `resource` name, `X.Id`, with `?` for nullable. Declare the enum or resource if it is missing.

```aip
actor Member
resource Member { fields { id: Id; name: Text } }
enum Status { OPEN, CLOSED }
resource Ticket {
  fields { id: Id; status: Status; reporter: Member? }
}
```

### `DEF.UNKNOWN_SYMBOL`

a predicate, resource, enum value or declaration that is not declared

- 원인: The name is used but nothing with that name exists at this place. The message says what kind of thing was expected.
- 수정: Declare it, or use the spelling of the existing one (the help offers close names when there are any).

```aip
actor Member
resource Member { fields { id: Id; name: Text } }
enum Status { OPEN, CLOSED }
resource Ticket {
  fields { id: Id; status: Status }
  rows read when status = OPEN
}
```

### `DEF.UNRESOLVED_NAME`

a name that is neither a field, a variable nor an enum value

- 원인: In an expression a bare name must be a field of the row, a parameter, `actor`, `this`, `now`, `null`, or a value of the enum that the other side of the comparison has. If the type of the other side is itself unknown, this error follows from it.
- 수정: Fix an earlier DEF.UNKNOWN_TYPE or DEF.DUPLICATE first. Otherwise spell the field or enum value as declared.

### `DEF.UNKNOWN_FIELD`

a field that the resource does not have

- 원인: A select, filter, sort, policy or effect names a field the resource does not declare.
- 수정: Use a declared field (the help offers close names when there are any) or declare the field in `fields`.

```aip
actor Member
resource Member { fields { id: Id; name: Text } }
resource Note {
  fields { id: Id; body: Text; owner: Member }
  expose read { select id, body
    budget { rows 10; depth 1; deadline 2s; cost 100 } }
}
```

### `DEF.UNKNOWN_RESOURCE`

a resource that is not declared

- 원인: A traverse or reference names a resource that this definition does not declare.
- 수정: Declare the resource or spell its name as declared.

### `DEF.UNKNOWN_OPERATOR`

a filter operator that does not exist

- 원인: Filters are `field.operator`; the operators are fixed.
- 수정: Use `eq`, `in`, `isNull`, `gte`, `lte`, `gt`, `lt`, `prefix` or `contains`, whichever the field type supports (DEF.OP_TYPE_MISMATCH tells which).

### `DEF.OP_TYPE_MISMATCH`

the operator is not defined for the type of the field

- 원인: For example `gte` needs an ordered type (Time, Date, Int, Decimal), `prefix` and `contains` need Text, `isNull` needs a nullable field, and Bool or reference fields cannot be sorted.
- 수정: Use an operator that fits the type, or filter by another field.

### `DEF.TYPE_MISMATCH`

the types do not fit

- 원인: Both sides of a comparison, an assignment, a predicate argument or an effect value must have the same type; conditions must be Bool. The message shows both types.
- 수정: Change the value or the field so that the types are the same. A literal needs the form of its type (an enum value without quotes, a Text in quotes).

### `DEF.NULL_COMPARE_NON_NULLABLE`

a field that can never be null is compared with null

- 원인: The comparison is always false (or always true), which is almost never what was meant.
- 수정: Declare the field nullable with `?` if null is a real state, otherwise remove the comparison.

```aip
actor Member
resource Member { fields { id: Id; name: Text } }
resource Club {
  fields { id: Id; name: Text; school: Member? }
  rows read when school = null
}
```

### `DEF.BAD_VALUE`

a literal does not have the form of its type

- 원인: A Url, Email, Time, Date or Decimal literal must be written in its canonical form.
- 수정: Write the literal as the message says, for example an absolute https URL or an RFC 3339 time.

### `DEF.BAD_DECIMAL_TYPE`

Decimal without precision and scale

- 원인: A Decimal column has an exact size; it has no default.
- 수정: Write `Decimal(p,s)`, for example `Decimal(12,2)`.

```aip
actor Member
resource Member { fields { id: Id; name: Text } }
resource Price {
  fields { id: Id; amount: Decimal(12,2) }
}
```

### `DEF.BAD_RANGE`

a range that cannot be satisfied or a value outside it

- 원인: A range such as `Text(1..100)` needs `lo <= hi`, and values assigned by a transition must lie inside the range of the field.
- 수정: Fix the bounds or the value.

### `DEF.ID_FIELD_NEEDS_REFERENCE`

a resource field declared as `X.Id`

- 원인: A field in `resource ... fields { }` is a database column. `club: Club` is a reference: a foreign key the database checks, joinable and traversable. `club: Club.Id` would be a bare number that merely happens to be a club id, with nothing keeping it valid. The language keeps one spelling for "this row belongs to a club", so the second is rejected. The primary key follows the same rule: it is `id: Id` and nothing else, so `id: Note.Id` is rejected too. `X.Id` stays valid where no column is involved: `input`, `access` parameters, operation inputs and outputs.
- 수정: Write `club: Club` (add `?` when it may be null). Adding the reference also adds a foreign key to the table. For the key, write `id: Id`.

```aip
actor Member
resource Member { fields { id: Id; name: Text } }
resource Club {
  fields { id: Id; name: Text }
}
resource Note {
  fields { id: Id; body: Text; club: Club }
}
```

### `DEF.ARITH_NOT_ALLOWED`

arithmetic outside the one place it is allowed

- 원인: Only the `to` block of a transition may change a number, written `field = field + 1` or `field = field - 1`. Conditions never compute.
- 수정: Move the arithmetic into a transition, or compare with a literal.

### `DEF.ARITY_MISMATCH`

a predicate is called with the wrong number of arguments

- 원인: A predicate call passes exactly the parameters it declares.
- 수정: Pass one argument for each declared parameter, in order.

### `DEF.THIS_IN_EXISTS`

`this` used where there is no row

- 원인: `this` is the row a policy is evaluated for; inside `exists ... where` the quantified row is named instead.
- 수정: Name the row explicitly through a predicate parameter.

### `DEF.ACCESS_MISMATCH`

an access used in a place it was not declared for

- 원인: An `access` that names a group key may only guard aggregates that group by that key.
- 수정: Declare an access for this aggregate, or give the aggregate the matching `groupKey`.

### `DEF.BAD_CAPABILITY`

a capability name that is not valid

- 원인: Managed capability names are ASCII identifiers.
- 수정: Use letters, digits and `_`.

### `DEF.POLICY_CYCLE`

predicates call each other in a cycle

- 원인: A policy is expanded into one query; a cycle has no end.
- 수정: Break the cycle: move the shared part into a third predicate that calls neither.

### `DEF.POLICY_FIELD_NOT_FILTERABLE`

a field with a read policy cannot be filtered or sorted

- 원인: If a hidden value could be filtered or sorted on, the caller could infer it from which rows come back.
- 수정: Remove the field from `filter` and `sort`, or remove its `field ... read when` policy.

### `DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION`

a field with a read policy in a transition's `from`, the value of `to`, or under `repeat unchanged`

- 원인: A caller who cannot read the field still sees the outcome of a transition: `WIRE.INVALID_STATE` when `from` is false, `unchanged` when the row already holds the target value, and a copied value when `to` reads the field into a visible one. Each of those would reveal the hidden value, so the definition is rejected. `allow` is the author's authorization rule and a write to a hidden field without `repeat unchanged` reveals nothing, so those stay valid.
- 수정: Express the condition on a field without a read policy, or remove the field's `field ... read when` policy. For a hidden target field, drop `repeat unchanged`.

### `DEF.POLICY_FIELD_NOT_AGGREGATABLE`

a field with a read policy cannot be aggregated

- 원인: An aggregate such as `sum(field)` would reveal the hidden values.
- 수정: Aggregate another field, or remove the field policy.

### `DEF.AGG_NOT_SELECTABLE`

an aggregate that is not a per-row aggregate is selected

- 원인: Only aggregates with a `groupKey` give each row a value.
- 수정: Remove it from `select`, or give it a `groupKey` that points to this resource.

### `DEF.TRAVERSE_NOT_EXPOSED`

traversing to a resource or field that is not exposed

- 원인: A traverse can read only what the child's own `expose read` selects.
- 수정: Add the field to the child's `expose read select`, or select another field.

### `DEF.TRAVERSE_VIA_NOT_REF`

the `via` field does not reference this resource

- 원인: A one-to-many traverse follows a reference field of the child that points back.
- 수정: Name a field of the child whose type is this resource.

### `DEF.TRAVERSE_MANY_AGGREGATE`

an aggregate inside a one-to-many traverse

- 원인: Aggregates in the children of a 1:N traverse are not supported yet.
- 수정: Select a plain field, or read the aggregate on the child resource itself.

### `DEF.BAD_LIMIT`

a traverse limit outside 1..1000

- 원인: Every traverse is bounded.
- 수정: Use a number from 1 to 1000.

### `DEF.BAD_BUDGET`

a budget or bulk size outside its range

- 원인: Budgets and bulk sizes have no unlimited default: they are positive and bounded.
- 수정: Use a positive number within the product limit.

### `DEF.BUDGET_DEPTH_TOO_SMALL`

a traverse needs depth 2 or more

- 원인: The root is depth 1 and each traverse adds one.
- 수정: Set `depth 2` (or more) in `budget`.

### `DEF.CURSOR_SORT_NOT_SELECTED`

cursor paging needs the sort fields to be selected

- 원인: A cursor is built from the sort values of the last row.
- 수정: Add the sort fields to `select`, and make sure they have no field policy.

### `DEF.CURSOR_NULLABLE_SORT`

cursor paging cannot sort on a nullable field

- 원인: Null ordering would make the page boundary ambiguous.
- 수정: Sort on a non-null field.

### `DEF.CURSOR_ID_NOT_SELECTED`

cursor paging needs `id` selected

- 원인: `id` breaks ties between rows with equal sort values.
- 수정: Add `id` to `select`.

### `DEF.GROUP_KEY_MISMATCH`

the groupKey does not reference the owning resource

- 원인: A per-row aggregate groups the source rows by the reference that points to the row it belongs to.
- 수정: Name a field of the source whose type is the owning resource.

### `DEF.EXT_INPUT_UNBOUND`

an access input is not received by the extension

- 원인: An extension may use an access only if every input that access needs is an input of the extension with the same type.
- 수정: Add the input to the extension `input` block, with the same name and type.

### `DEF.EFFECT_MISMATCH`

the declared effect does not fit the extension kind

- 원인: A read extension has `effect none`; a write extension has `effect db`.
- 수정: Change `effect` to the value the message names.

### `DEF.UNSUPPORTED_EFFECT`

an effect that the executor cannot make atomic

- 원인: A transition cannot change the primary key of the locked row, and a many-row update needs a parent condition so that its scope is the row's own children.
- 수정: Follow the message: set only other fields, or add `parent = this.id` to the update.

### `DEF.UNSUPPORTED_INVARIANT`

an invariant condition that cannot be enforced in the database

- 원인: An invariant is a unique index; it can use the fields of the row and literals, not time, the actor or sub-queries.
- 수정: Make the condition use only fields of the row and literals.

### `DEF.UNSUPPORTED`

a form that the product does not support yet

- 원인: The construct is recognised but not executable. The message says which part and what is supported instead.
- 수정: Use the supported form named in the message.

### `DEF.BAD_OPTION`

a check option has the wrong type

- 원인: The options of the development checks are a small object.
- 수정: Pass `{ "devChecks": true }` or `{ "devChecks": false }`.

### `DEF.UNKNOWN_OPTION`

a check option that does not exist

- 원인: Unknown options are rejected, not ignored.
- 수정: Use only `devChecks`.

### `DEF.ADVISORY_UNREACHABLE_READ`

an expose read that no caller can reach (advisory)

- 원인: Not an error: the definition is accepted. An `expose read` without `budget` is only a target of another resource's `traverse`; when no resource traverses it, every read of it fails at run time with WIRE.NOT_ROOT_QUERYABLE. The usual cause is a forgotten `budget`.
- 수정: Add `budget { rows N; depth 1; deadline 2s; cost N }` to the `expose read` to make it readable on its own, or traverse it from the resource that should embed it.

```aip
actor Member
resource Member { fields { id: Id; name: Text } }
resource Note {
  fields { id: Id; body: Text; owner: Member }
  expose read { select id, body
    budget { rows 10; depth 1; deadline 2s; cost 100 } }
}
```

### `DEF.ADVISORY_ABSENCE_POLICY`

a write whose condition needs another row to be absent (advisory)

- 원인: Not an error: the definition is accepted. A transition or create whose `allow`/`from` says `not exists ...` is decided on a snapshot, so two requests that change different rows could both pass it. The server therefore runs it in a SERIALIZABLE transaction and retries a collision, which costs more than the plain path.
- 수정: If the goal is uniqueness or a capacity, declare `unique a, b` or `limit name on X = atMost N where ...` with `invariant name per field`; the database enforces those directly and cheaper. Keep the `not exists` rule when it expresses something those cannot.

### `DEF.ADVISORY_UNUSED_PREDICATE`

a predicate that nothing uses (advisory)

- 원인: Not an error: the definition is accepted. An unused predicate is usually a rule that was meant to be applied and is not.
- 수정: Use the predicate in a `rows read`, `field ... read`, `allow` or `access`, or remove it.
