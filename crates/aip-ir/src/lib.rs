//! Canonical Semantic IR (Core IR): the meaning of an AIP application,
//! independent of the language it was written in and of the backend that
//! runs it. See `docs/design/03-ir.md` and `docs/design/10-philosophy-alignment.md`.
//!
//! Rules every producer (the `.aip` frontend today, TS/Python frontends or
//! tools later) must follow:
//! - No source positions. Positions live in a [`SourceMap`] next to the
//!   program, so two frontends that mean the same thing produce byte-equal IR.
//! - Every name is resolved: an expression says whether it reads a parameter,
//!   a local binding, a field (and of which entity), an enum value or a
//!   predicate. Consumers never re-run name lookup.
//! - Sugar is gone. There is one node per meaning (`!=` is `Binary{Ne}`,
//!   shorthand field assignments are spelled out, `expose` is expanded).
//! - Nothing here names SQL, PostgreSQL, HTTP or Rust runtime objects.
//!   Backends (`aip-pg` → `aip-plan`) consume this; this never points back.
//! - Collections are `BTreeMap` (sorted) or `Vec` in declaration order, so
//!   serialization is deterministic and [`digest`] is stable.
//!
//! Every consumer of a program runs [`validate::validate`] (is the IR well-formed?)
//! and [`analyze::analyze`] (does it follow the safety rules?). Rules that need
//! names or types resolved first stay in the frontend; once the IR exists they
//! belong to `analyze`, so a TS or Python frontend is held to the same rules.

pub mod analyze;
pub mod builtin;
pub mod codes;
pub mod diff;
pub mod facts;
pub mod form_intents;
pub mod forms;
pub mod tenant;
pub mod validate;

pub use forms::*;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Format tag written as the first key of every serialized program.
pub const CORE_IR_VERSION: &str = "aip-core/0.11";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub aip_core: String,
    /// Extensions the application depends on (`use s3`, `use payments.stripe`).
    pub uses: Vec<String>,
    pub actor: Option<Actor>,
    pub enums: BTreeMap<String, EnumDef>,
    pub records: BTreeMap<String, RecordDef>,
    pub entities: BTreeMap<String, Entity>,
    pub relations: BTreeMap<String, Relation>,
    pub fns: BTreeMap<String, FnDef>,
    pub events: BTreeMap<String, EventDef>,
    pub configs: BTreeMap<String, ConfigDef>,
    pub flags: BTreeMap<String, FlagDef>,
    pub intents: BTreeMap<String, Intent>,
    /// Reactions to events, time and data changes, in declaration order.
    pub reactions: Vec<Reaction>,
    /// L3 domain forms not yet lowered to Core (see `forms.rs`).
    pub forms: Vec<Form>,
    /// `removed entity X`: entities that existed in a deployed version and are dropped on purpose.
    /// The declaration is evolution intent: it changes nothing about what the program means, only
    /// what a deployment may do when it finds the old entity in the database (see [`diff`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_entities: Vec<String>,
}

// ---------- types ----------

/// Declared value types. Refinements that change what values are legal
/// (ranges, lengths, patterns) are part of the type, so validators in every
/// language derive from the same data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Type {
    Bool,
    Int {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<i64>,
    },
    /// Exact decimal. Never a binary float.
    Decimal {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        precision: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<u32>,
    },
    Text {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<i64>,
        #[serde(default, skip_serializing_if = "is_false")]
        trim: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        lower: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
    },
    RichText {
        /// Sanitizer policy (`RichText(policy: basic)`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        policy: Option<String>,
    },
    Email,
    Url,
    Phone {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
    },
    /// An instant (UTC).
    Time,
    Date,
    Duration,
    /// Byte size.
    Size,
    Uuid,
    /// Exact amount in one currency.
    Money {
        currency: String,
    },
    /// JSON value. `schema` (`Json<X>`) names the record it follows; `validated_by`
    /// (`Json validated by form`) is the path of the pinned dynamic schema it is
    /// checked against on write. The two are different meanings, never both set.
    Json {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        validated_by: Option<String>,
    },
    Enum {
        name: String,
    },
    Record {
        name: String,
    },
    /// Payload of a declared or implied event (the binding of `on Event(e)`).
    Event {
        name: String,
    },
    /// Reference to one row of an entity (the value on the wire is its id).
    Ref {
        entity: String,
    },
    /// Reference to one row of any of several entities.
    RefUnion {
        entities: Vec<String>,
    },
    /// A pinned version of a `history` entity row.
    Snapshot {
        entity: String,
    },
    Set {
        of: Box<Type>,
        max: u64,
    },
    List {
        of: Box<Type>,
        max: u64,
    },
    Range {
        of: Box<Type>,
    },
    Localized {
        of: Box<Type>,
    },
    Upload {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<u64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        types: Vec<String>,
    },
    /// A stored object (upload after staging).
    Object,
    Credential {
        kind: String,
    },
    Recurrence,
    /// Opaque type provided by an extension (`redis.Counter`).
    Ext {
        name: String,
    },
    // Expression-only types (never declared):
    /// A multi-valued expression (inverse relation, set parameter, path through one).
    Many {
        of: Box<Type>,
    },
    /// The literal `null`.
    Null,
    /// A bare identifier argument naming a provider variant (`kakao` in `auth.oidc(kakao)`).
    Symbol,
    /// The frontend could not type this expression (only in programs with errors).
    Unknown,
}

fn is_false(b: &bool) -> bool {
    !*b
}

// ---------- declarations ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Actor {
    pub entity: String,
    /// Authentication provider (`auth.oidc`, `auth.token`).
    pub provider: Call,
    pub scopes: Vec<String>,
    /// When true for the actor, every allow/visibility passes. Always audited.
    pub superuser: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumDef {
    /// Declaration order is meaningful when `ordered` (comparison by rank).
    pub ordered: bool,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordDef {
    pub fields: Vec<Field>,
    pub checks: Vec<Require>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entity {
    /// `entity New was Old`: the deployed entity this one continues under a new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was: Option<String>,
    /// `removed field x`: fields of a deployed version that are dropped on purpose.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_fields: Vec<String>,
    pub personal: bool,
    pub access_audited: bool,
    /// Declared and implicit fields (`id`, `createdAt`, ...), declared first.
    pub fields: Vec<Field>,
    pub constraints: Vec<Constraint>,
    pub lifecycles: Vec<Lifecycle>,
    pub visibility: Option<Visibility>,
    pub predicates: BTreeMap<String, Expr>,
    pub traits: Traits,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    /// `was old`: the deployed field this one continues under a new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was: Option<String>,
    pub ty: Type,
    /// May be absent. v0: an absent value and `null` mean the same thing
    /// (open issue OI-T1 in `docs/DECISIONS.md`).
    pub optional: bool,
    pub default: Option<Expr>,
    pub kind: FieldKind,
    #[serde(default, skip_serializing_if = "is_false")]
    pub personal: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub encrypted: bool,
    /// Field-level read policy; readers that fail it see the field as absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_to: Option<Expr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masked: Option<Masked>,
    /// Value produced by the runtime instead of the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<Generated>,
    /// Derived image variants (`thumb: "200x200"`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum FieldKind {
    Stored,
    Ref {
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_delete: Option<RefPolicy>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_erase: Option<RefPolicy>,
    },
    RefUnion {
        targets: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_delete: Option<RefPolicy>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_erase: Option<RefPolicy>,
    },
    /// One-to-many inverse of `target.via`.
    Inverse {
        target: String,
        via: String,
    },
    /// Counter whose value is kept by an extension (`redis`) named in `uses`.
    Counter {
        store: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dedupe: Option<CounterDedupe>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window_seconds: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sharded: Option<i64>,
    },
    /// `id`, `createdAt`, `updatedAt`, `createdBy`, `updatedBy`, `deletedAt`, `version`.
    Implicit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CounterDedupe {
    pub by: String,
    pub within_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum RefPolicy {
    Cascade,
    Restrict,
    SetNull,
    Anonymize,
    /// Repoint references to the row this expression selects.
    Reassign {
        to: Expr,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Masked {
    pub unless: Expr,
    pub with: Call,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "gen", rename_all = "snake_case")]
pub enum Generated {
    /// Gap-free per-scope number rendered with `format`.
    Sequence { per: Vec<String>, format: String },
    Slug {
        from: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        per: Vec<String>,
    },
    /// Dense order within the group.
    Position { within: Vec<String> },
    /// Parent reference forming a tree.
    Tree { max_depth: Option<i64> },
}

// constraints are cold data held in a Vec; see `Form`
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "c", rename_all = "snake_case")]
pub enum Constraint {
    Unique {
        /// Position among the entity's declared members; backends use it to
        /// name the objects they create for this constraint.
        ordinal: u32,
        fields: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filter: Option<Expr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    /// `exactly one` / `at most one` / `at least one` rows matching `filter` per `per`.
    Cardinality {
        /// Position among the entity's declared members; backends use it to
        /// name the objects they create for this constraint.
        ordinal: u32,
        kind: Cardinality,
        filter: Expr,
        per: String,
        /// Statements run when a change would break `at least one`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repair: Option<Vec<Stmt>>,
    },
    NoOverlap {
        /// Position among the entity's declared members; backends use it to
        /// name the objects they create for this constraint.
        ordinal: u32,
        range: String,
        per: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    /// The number of rows in `count` may not exceed `limit` (a value from data).
    Capacity {
        /// Position among the entity's declared members; backends use it to
        /// name the objects they create for this constraint.
        ordinal: u32,
        count: SetExpr,
        limit: Expr,
        code: String,
    },
    Invariant {
        /// Position among the entity's declared members; backends use it to
        /// name the objects they create for this constraint.
        ordinal: u32,
        name: String,
        cond: Expr,
    },
}

impl Constraint {
    pub fn ordinal(&self) -> u32 {
        match self {
            Constraint::Unique { ordinal, .. }
            | Constraint::Cardinality { ordinal, .. }
            | Constraint::NoOverlap { ordinal, .. }
            | Constraint::Capacity { ordinal, .. }
            | Constraint::Invariant { ordinal, .. } => *ordinal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cardinality {
    ExactlyOne,
    AtMostOne,
    AtLeastOne,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lifecycle {
    pub field: String,
    /// Every allowed change `from -> to`; anything else fails with TRANSITION.
    pub transitions: Vec<Transition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    pub from: Vec<String>,
    pub to: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Visibility {
    /// `visible to actor unless cond` (true) vs `when cond` (false).
    pub unless: bool,
    pub cond: Expr,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Traits {
    #[serde(default, skip_serializing_if = "is_false")]
    pub track_created: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub track_updated: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub track_by_actor: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub history: bool,
    /// Soft delete; `Some(retain)` with the retention in seconds, `Some(None)` without.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_delete: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub versioned: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub publishable: bool,
    /// `publishable by <cond>`: who may publish or discard a draft, over the row and the actor. Without it only the superuser may.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_by: Option<Expr>,
    /// Tenant scope path (`club.org`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<Vec<String>>,
    /// Field whose referenced row holds the dynamic schema for this row's JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic_schema: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub optional: bool,
    pub default: Option<Expr>,
}

/// Named authorization relation (`relation managerOf(m: Member, c: Club)`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    pub params: Vec<Param>,
    /// Entity the relation yields, when it is not boolean.
    pub result: Option<String>,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FnDef {
    pub params: Vec<Param>,
    pub ret: Type,
    pub body: FnBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum FnBody {
    Expr {
        expr: Expr,
    },
    /// Opaque pure function: the frontend cannot see inside (Pure Custom).
    Wasm {
        module: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventDef {
    pub version: Option<i64>,
    pub fields: Vec<(String, Type)>,
    /// `false` when the event is implied by an `emit` without a declaration.
    pub declared: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigDef {
    pub ty: Type,
    pub optional: bool,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlagDef {
    pub default_on: bool,
    /// Percentage of actors the flag is on for.
    pub rollout: Option<i64>,
}

// ---------- intents ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Intent {
    Query(Query),
    Command(Command),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Let {
    pub name: String,
    pub value: Expr,
    /// Fails the intent with this code when the value is null.
    pub code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    pub cond: Expr,
    pub code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Require {
    pub when: Option<Expr>,
    pub cond: Expr,
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimit {
    pub n: i64,
    pub per_seconds: i64,
    pub key: RateKey,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum RateKey {
    Actor,
    Client,
    Path { path: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Query {
    pub internal: bool,
    /// `drafts query`: reads the working version of publishable entities; any other query reads only what is published.
    #[serde(default, skip_serializing_if = "is_false")]
    pub drafts: bool,
    /// `internal cross tenant`: exempt from the single-tenant rules (see [`tenant`]).
    #[serde(default, skip_serializing_if = "is_false")]
    pub cross_tenant: bool,
    pub params: Vec<Param>,
    pub cache: Option<Cache>,
    pub rate_limits: Vec<RateLimit>,
    pub lets: Vec<Let>,
    pub allow: Option<Policy>,
    /// Extension reads bound before the main source (`fetch weather.now() as w`).
    pub fetches: Vec<(Call, String)>,
    pub source: Option<QuerySource>,
    pub filter: Option<Expr>,
    pub group_by: Vec<Expr>,
    pub sort: Option<Sort>,
    pub page: Option<Page>,
    /// Plan hint name and consistency level as written; meaning is backend-checked.
    pub plan: Option<String>,
    pub consistency: Option<String>,
    pub select: Selection,
    /// Counters bumped by reading (`touch r.views`).
    pub touches: Vec<Expr>,
    pub output: Shape,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cache {
    pub seconds: i64,
    /// Partition key (`per actor`).
    pub per: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum QuerySource {
    Entity {
        entity: String,
        alias: String,
    },
    /// Rows of an entity parameter (`from r`).
    Param {
        param: String,
        entity: String,
    },
    /// Rows produced by an extension call.
    Call {
        call: Call,
        alias: String,
    },
    /// Rows of the entity a `search` declaration covers that match `query`, best match first (`from Search.match(q) r`).
    /// The match score of a row is [`Node::SearchRank`] under `alias`.
    Search {
        search: String,
        query: Box<Expr>,
        alias: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Sort {
    Keys {
        keys: Vec<SortKey>,
    },
    /// The caller picks one of the named orders through `param`.
    ByParam {
        param: String,
        cases: Vec<(String, Vec<SortKey>)>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SortKey {
    pub expr: Expr,
    pub desc: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub size: i64,
    /// `None`: keyset (cursor) paging. `Some(n)`: offset paging up to page n.
    pub offset_max_page: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Selection {
    pub items: Vec<SelItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelItem {
    pub name: String,
    /// `None`: the field of the same name.
    pub value: Option<Expr>,
    pub sub: Option<Selection>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub internal: bool,
    /// `internal cross tenant`: exempt from the single-tenant rules (see [`tenant`]).
    #[serde(default, skip_serializing_if = "is_false")]
    pub cross_tenant: bool,
    pub params: Vec<Param>,
    pub idempotency: Idempotency,
    pub audited: bool,
    pub rate_limits: Vec<RateLimit>,
    pub lets: Vec<Let>,
    pub allow: Option<Policy>,
    pub requires: Vec<Require>,
    pub body: Vec<Stmt>,
    pub emits: Vec<Emit>,
    pub returns: Option<Returns>,
    pub output: Shape,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Idempotency {
    None,
    /// The caller supplies a key per logical request.
    CallerKey,
    /// The key is derived from the input.
    Derived {
        key: Expr,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Returns {
    pub value: Expr,
    pub select: Option<Selection>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Emit {
    pub event: String,
    pub fields: Vec<(String, Expr)>,
    /// External broker delivery (`to kafka "topic" key k`).
    pub to: Option<EmitTarget>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmitTarget {
    pub broker: String,
    pub topic: String,
    pub key: Option<Expr>,
}

/// What an intent returns, for clients in any language.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Shape {
    /// Nothing (a command without `returns`).
    None,
    Value {
        ty: Type,
        nullable: bool,
    },
    Object {
        fields: Vec<(String, Shape)>,
        nullable: bool,
    },
    List {
        of: Box<Shape>,
        nullable: bool,
    },
}

// ---------- statements ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "s", rename_all = "snake_case")]
pub enum Stmt {
    Let(Let),
    /// One row, or one row per element of `from`.
    Insert {
        entity: String,
        from: Option<SetExpr>,
        values: Vec<(String, Expr)>,
        bind: Option<String>,
    },
    Upsert {
        entity: String,
        keys: Vec<String>,
        values: Vec<(String, Expr)>,
        bind: Option<String>,
    },
    /// Set-based update of every row in `target`.
    Update {
        target: SetExpr,
        /// `update ... via path alias`: rows reached through `path`, bound as `alias`.
        via: Option<UpdateVia>,
        assigns: Vec<Assign>,
    },
    /// Soft delete when the entity has the trait, hard delete otherwise.
    Delete {
        target: SetExpr,
    },
    /// Hard delete, even for soft-delete entities.
    Purge {
        target: SetExpr,
    },
    /// Personal-data erasure following the reference policies.
    Erase {
        target: Expr,
    },
    /// Insert if absent, delete if present.
    Toggle {
        entity: String,
        values: Vec<(String, Expr)>,
    },
    /// Assignments to fields of already-bound rows.
    Set {
        assigns: Vec<Assign>,
    },
    When {
        cond: Expr,
        body: Vec<Stmt>,
    },
    /// Per-item body over a bounded set; with `partial`, items fail independently.
    Each {
        source: SetExpr,
        body: Vec<Stmt>,
    },
    /// Extension effect. Its consistency class comes from the extension's signature.
    Effect {
        call: Call,
        bind: Option<String>,
        into: Option<Expr>,
        on_failure: Option<Vec<Stmt>>,
    },
    /// Reservation on a reservable resource (`reserve hold seat of e for 10m`).
    Reserve {
        op: String,
        what: String,
        of: Expr,
        seconds: Option<i64>,
        code: Option<String>,
    },
    /// Durable timer: run `intent` at `at`.
    AtRun {
        at: Expr,
        intent: String,
        args: Vec<Arg>,
    },
    Notify(Notify),
    ExportPersonalData {
        of: Expr,
        to: String,
        notify: Option<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateVia {
    pub path: Expr,
    pub alias: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assign {
    /// A field path rooted at a bound row (`m.role`).
    pub target: Expr,
    pub op: AssignOp,
    pub value: Expr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignOp {
    Set,
    Add,
    Sub,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notify {
    pub to: SetExpr,
    pub via: Call,
    pub fields: Vec<(String, Expr)>,
    pub category: Option<String>,
    pub digest_seconds: Option<i64>,
}

// ---------- expressions ----------

/// A typed expression. `ty` is the frontend's inferred type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expr {
    pub ty: Type,
    #[serde(flatten)]
    pub node: Node,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "e", rename_all = "snake_case")]
pub enum Node {
    Lit {
        lit: Literal,
    },
    /// An intent / form / relation / fn parameter.
    Param {
        name: String,
    },
    /// A `let`, a statement binding, a set alias or an event/item binding.
    Local {
        name: String,
    },
    /// A field of the implicit entity row (entity-level predicates, constraints, visibility).
    RowField {
        entity: String,
        field: String,
    },
    /// A bare field inside a record check (`record R { ... check ... }`).
    RecordField {
        record: String,
        field: String,
    },
    /// Identifier argument that names a provider variant, not a value in scope.
    Symbol {
        name: String,
    },
    EnumValue {
        #[serde(rename = "enum")]
        enum_name: String,
        value: String,
    },
    /// Configuration value or feature flag.
    Config {
        name: String,
    },
    Actor,
    /// The match score of the row bound to `alias` by a `QuerySource::Search`: a decimal, higher is a better match.
    /// It is not a field of the entity, so it exists only in the query that searches.
    SearchRank {
        alias: String,
    },
    /// The secret an endpoint row of an `outbound webhooks` form is signed with (`ep.signingSecret`). The server derives
    /// it from the row and its own secret and stores nothing, so it can be shown when the row is created and never again:
    /// only the `returns` of the command that inserted the row may read it.
    SigningSecret {
        base: Box<Expr>,
    },
    /// The row an entity-level declaration is about (`self`).
    SelfRow,
    /// The current item in a contextual form (`this`).
    This,
    Now,
    Today,
    /// Policy constants.
    Public,
    Authenticated,
    Field {
        base: Box<Expr>,
        /// Entity `base` refers to, when it is a row.
        entity: Option<String>,
        field: String,
    },
    Unary {
        op: UnOp,
        arg: Box<Expr>,
    },
    Binary {
        op: BinOp,
        l: Box<Expr>,
        r: Box<Expr>,
    },
    InList {
        value: Box<Expr>,
        list: Vec<Expr>,
    },
    InRange {
        value: Box<Expr>,
        lo: Box<Expr>,
        hi: Box<Expr>,
    },
    /// Membership in a range value (`now in r.period`), as opposed to the literal `[lo, hi)` of `InRange`.
    InRangeValue {
        value: Box<Expr>,
        range: Box<Expr>,
    },
    /// Membership in a multi-valued expression or a set parameter.
    InSet {
        value: Box<Expr>,
        set: Box<Expr>,
    },
    /// Type test: the row or actor `value` is a row of `entity` (`actor is Member`).
    TypeTest {
        value: Box<Expr>,
        entity: String,
    },
    /// Named entity predicate applied to a row (`m is admin`).
    Pred {
        entity: String,
        name: String,
        row: Box<Expr>,
    },
    HasScope {
        subject: Box<Expr>,
        scope: String,
    },
    Exists {
        set: Box<SetExpr>,
    },
    /// The single row of `set`; null when there is none.
    The {
        set: Box<SetExpr>,
    },
    Latest {
        set: Box<SetExpr>,
        by: Box<Expr>,
    },
    First {
        set: Box<SetExpr>,
        order: Vec<SortKey>,
    },
    Agg {
        func: AggFn,
        set: Option<Box<SetExpr>>,
        value: Option<Box<Expr>>,
    },
    Quant {
        all: bool,
        set: Box<SetExpr>,
        body: Box<Expr>,
    },
    RunningSum {
        value: Box<Expr>,
        over: String,
        order: Box<Expr>,
    },
    If {
        cond: Box<Expr>,
        then: Box<Expr>,
        otherwise: Box<Expr>,
    },
    List {
        items: Vec<Expr>,
    },
    Call(Call),
    /// `items x: body` argument: a set and an expression over each item.
    Binder {
        set: Box<SetExpr>,
        body: Box<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "lit", content = "v", rename_all = "snake_case")]
pub enum Literal {
    Int(i64),
    /// Exact decimal text, never parsed through a float.
    Decimal(String),
    Text(String),
    Bool(bool),
    Null,
    /// A duration as written. Months and years are calendar units, so the unit is kept.
    Duration(Dur),
    SizeBytes(u64),
    TimeOfDay(u8, u8),
}

/// A duration with the unit it was written in. `3 days` and `72 hours` are
/// equal in length but a backend that has calendar arithmetic (`1 month` is not
/// 30 days there) needs the unit, so it is not normalized away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dur {
    pub n: i64,
    pub unit: DurUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurUnit {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

impl Dur {
    /// Length in seconds; a month counts as 30 days and a year as 365 days.
    pub fn seconds(self) -> i64 {
        self.n.max(0)
            * match self.unit {
                DurUnit::Second => 1,
                DurUnit::Minute => 60,
                DurUnit::Hour => 3600,
                DurUnit::Day => 86_400,
                DurUnit::Week => 604_800,
                DurUnit::Month => 2_592_000,
                DurUnit::Year => 31_536_000,
            }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggFn {
    Count,
    Sum,
    Min,
    Max,
    Avg,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub callee: Callee,
    pub args: Vec<Arg>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Callee {
    /// Pure function declared in the program.
    Fn {
        name: String,
    },
    Relation {
        name: String,
    },
    /// Built-in pure function (`same`, `len`, `lower`, `days`...).
    Builtin {
        name: String,
    },
    /// Extension operation (`s3.put`, `mail.send`); effects carry their class.
    Ext {
        name: String,
    },
    /// Another intent (timers, `at ... run`).
    Intent {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Arg {
    pub name: Option<String>,
    pub value: Expr,
}

/// A set of rows: `source [alias] [where filter]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetExpr {
    pub source: SetSource,
    pub alias: Option<String>,
    pub filter: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum SetSource {
    /// All rows of an entity.
    Entity { entity: String },
    /// A multi-valued expression (inverse relation, set parameter, path).
    Expr { expr: Expr },
}

// ---------- reactions ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "on", rename_all = "snake_case")]
pub enum Reaction {
    /// `on Event(e) [when cond] do { ... }`. Runs after commit, at least once.
    Event {
        /// `cross tenant`: may write rows of several tenants (see [`tenant`]).
        #[serde(default, skip_serializing_if = "is_false")]
        cross_tenant: bool,
        event: String,
        binding: String,
        when: Option<Expr>,
        body: Vec<Stmt>,
    },
    Schedule(Schedule),
    /// Retention: delete or anonymize rows `keep` after `after`.
    Retain {
        entity: String,
        keep: Dur,
        after: Vec<String>,
        anonymize: bool,
        notify: Option<RetainNotify>,
    },
    /// Runs the body when `when` becomes true for a row, evaluated before every commit.
    Rule {
        /// `cross tenant`: may write rows of several tenants (see [`tenant`]).
        #[serde(default, skip_serializing_if = "is_false")]
        cross_tenant: bool,
        name: String,
        entity: String,
        alias: String,
        when: Expr,
        body: Vec<Stmt>,
    },
    Webhook(Webhook),
    /// Broker consumer, deduplicated per message key.
    Consume {
        /// `cross tenant`: may write rows of several tenants (see [`tenant`]).
        #[serde(default, skip_serializing_if = "is_false")]
        cross_tenant: bool,
        name: String,
        broker: String,
        topic: String,
        key: String,
        dedupe: Option<String>,
        body: Vec<Stmt>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Schedule {
    /// `cross tenant`: may write rows of several tenants (see [`tenant`]).
    #[serde(default, skip_serializing_if = "is_false")]
    pub cross_tenant: bool,
    pub name: String,
    pub every: Every,
    pub at: Option<(u8, u8)>,
    pub tz: Option<String>,
    pub catch_up_once: Option<bool>,
    pub for_each: Option<SetExpr>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub enum Every {
    Day,
    Week { day: String },
    MonthDay { day: i64 },
    Interval { seconds: i64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetainNotify {
    pub path: Vec<String>,
    pub before: Dur,
    pub via: Call,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Webhook {
    pub name: String,
    /// Verification source (`payments.stripe.webhook`, `http.webhook(...)`).
    pub via: Call,
    pub handlers: Vec<WebhookOn>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebhookOn {
    /// `cross tenant`: may write rows of several tenants (see [`tenant`]).
    #[serde(default, skip_serializing_if = "is_false")]
    pub cross_tenant: bool,
    pub event: String,
    pub binding: String,
    pub ty: Option<Type>,
    pub body: Vec<Stmt>,
}

// ---------- serialization ----------

/// Source positions for diagnostics, keyed by IR path (`intents.ApplyClub`,
/// `entities.Club.fields.name`). Not part of the canonical program.
pub type SourceMap = BTreeMap<String, Pos>;

/// Position of an IR path: the path itself, else the closest enclosing path the source map knows; `(0, 0)` when none is.
pub fn locate(map: &SourceMap, path: &str) -> (u32, u32) {
    let mut p = path;
    loop {
        if let Some(pos) = map.get(p) {
            return (pos.line, pos.col);
        }
        match p.rfind(['.', '[']) {
            Some(i) => p = &p[..i],
            None => return (0, 0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

/// Canonical JSON text: deterministic for equal programs.
pub fn canonical_json(p: &Program) -> String {
    serde_json::to_string(p).unwrap_or_default()
}

/// `sha256:<hex>` of the canonical JSON. Equal meaning ⇒ equal digest, for
/// programs from any frontend.
pub fn digest(p: &Program) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(canonical_json(p).as_bytes());
    let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}
