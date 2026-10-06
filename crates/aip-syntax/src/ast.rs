//! Surface AST. Mirrors `spec/grammar.md` closely; no semantic information.

use crate::diag::Span;
use crate::lexer::DurUnit;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// Dotted name: `s3.put`, `payments.toss.webhook`, `orders.write`.
#[derive(Debug, Clone, Serialize)]
pub struct QualName {
    pub parts: Vec<Ident>,
    pub span: Span,
}

impl QualName {
    pub fn text(&self) -> String {
        self.parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(".")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Duration {
    pub n: i64,
    pub unit: DurUnit,
}

#[derive(Debug, Clone, Serialize)]
pub struct File {
    pub decls: Vec<Decl>,
}

#[derive(Debug, Clone, Serialize)]
pub enum Decl {
    Use(QualName),
    Actor(ActorDecl),
    Enum(EnumDecl),
    Record(RecordDecl),
    Entity(EntityDecl),
    Relation(RelationDecl),
    Fn(FnDecl),
    Event(EventDecl),
    Upcast(UpcastDecl),
    Query(QueryDecl),
    Command(CommandDecl),
    Subscribe(SubscribeDecl),
    Webhook(WebhookDecl),
    Consume(ConsumeDecl),
    OnEvent(OnEventDecl),
    Schedule(ScheduleDecl),
    Retain(RetainDecl),
    Rule(RuleDecl),
    Projection(ProjectionDecl),
    Search(SearchDecl),
    Job(JobDecl),
    Verification(VerificationDecl),
    GrantLink(GrantLinkDecl),
    Approval(ApprovalDecl),
    Expose(ExposeDecl),
    OutboundWebhooks(OutboundDecl),
    Consent(ConsentDecl),
    Config(ConfigDecl),
    Flag(FlagDecl),
    Impersonate(ImpersonateDecl),
    Migration(MigrationDecl),
    Removed(RemovedDecl),
}

impl Decl {
    pub fn name(&self) -> Option<&Ident> {
        Some(match self {
            Decl::Use(_) | Decl::Upcast(_) | Decl::OnEvent(_) | Decl::Removed(_) => return None,
            Decl::Actor(d) => &d.name,
            Decl::Enum(d) => &d.name,
            Decl::Record(d) => &d.name,
            Decl::Entity(d) => &d.name,
            Decl::Relation(d) => &d.name,
            Decl::Fn(d) => &d.name,
            Decl::Event(d) => &d.name,
            Decl::Query(d) => &d.name,
            Decl::Command(d) => &d.name,
            Decl::Subscribe(d) => &d.name,
            Decl::Webhook(d) => &d.name,
            Decl::Consume(d) => &d.name,
            Decl::Schedule(d) => &d.name,
            Decl::Retain(d) => &d.entity,
            Decl::Rule(d) => &d.name,
            Decl::Projection(d) => &d.name,
            Decl::Search(d) => &d.name,
            Decl::Job(d) => &d.name,
            Decl::Verification(d) => &d.name,
            Decl::GrantLink(d) => &d.name,
            Decl::Approval(d) => &d.name,
            Decl::Expose(d) => &d.entity,
            Decl::OutboundWebhooks(d) => &d.entity,
            Decl::Consent(d) => &d.name,
            Decl::Config(d) => &d.name,
            Decl::Flag(d) => &d.name,
            Decl::Impersonate(d) => &d.entity,
            Decl::Migration(d) => &d.name,
        })
    }
}

// ---------- types ----------

#[derive(Debug, Clone, Serialize)]
pub struct TypeExpr {
    pub kind: TypeKind,
    pub optional: bool,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub enum TypeKind {
    /// `Text`, `Club`, `s3.Object`
    Name(QualName),
    /// `Int(0..)`, `Text(1..30, trim)`, `Decimal(10, 2)`, `Phone(KR)`, `Money(KRW)`, `Upload(max 10MB, types [png])`
    Refined { base: Ident, opts: Vec<RefineOpt> },
    /// `Set<T> max N`, `List<T> max N`, `Range<T>`, `Json<S>`, `Localized<T>`, `Credential<a.b>`, `Snapshot<E>`
    Generic { base: Ident, arg: Box<TypeExpr>, max: Option<u64> },
    /// `Json validated by form`
    JsonValidated(QualName),
    /// `OrderItem[]` (inverse relation, needs `via`)
    Many(Ident),
    /// `ref A | B`
    Union(Vec<Ident>),
}

#[derive(Debug, Clone, Serialize)]
pub enum RefineOpt {
    Range(Option<i64>, Option<i64>),
    Word(Ident),
    Int(i64),
    Matches(String),
    Max(u64),
    Types(Vec<Ident>),
    KeyValue(Ident, Ident),
}

// ---------- declarations ----------

#[derive(Debug, Clone, Serialize)]
pub struct ActorDecl {
    pub name: Ident,
    pub via: CallTarget,
    pub scopes: Vec<QualName>,
    pub superuser: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnumDecl {
    pub name: Ident,
    pub ordered: bool,
    pub values: Vec<Ident>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordDecl {
    pub name: Ident,
    pub fields: Vec<FieldDecl>,
    pub checks: Vec<RecordCheck>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordCheck {
    pub when: Option<Expr>,
    pub cond: Expr,
    pub code: Ident,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityDecl {
    pub name: Ident,
    /// `entity New was Old`: the table of `Old` is renamed, not dropped.
    pub was: Option<Ident>,
    pub personal: bool,
    pub access_audited: bool,
    pub members: Vec<EntityMember>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub enum EntityMember {
    Field(FieldDecl),
    Constraint(Constraint),
    Lifecycle(Lifecycle),
    Visibility(Visibility),
    Predicate(PredicateDecl),
    Trait(EntityTrait),
    /// `removed field x`: the column of a field that is no longer declared is dropped on purpose.
    RemovedField(Ident),
}

/// `removed entity X`: the table of an entity that is no longer declared is dropped on purpose.
#[derive(Debug, Clone, Serialize)]
pub struct RemovedDecl {
    pub name: Ident,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldDecl {
    pub name: Ident,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
    pub mods: Vec<FieldMod>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub enum FieldMod {
    /// `was old`: this field is the one that used to be called `old`.
    Was(Ident),
    Personal(Span),
    Encrypted(Span),
    VisibleTo(Expr),
    Masked {
        unless: Expr,
        with: CallTarget,
    },
    OnDelete(RefPolicy),
    OnErase(RefPolicy),
    Via(Ident),
    Tree {
        max_depth: Option<i64>,
        span: Span,
    },
    Sequence {
        per: QualName,
        format: String,
        span: Span,
    },
    Slug {
        from: Ident,
        per: Option<QualName>,
        span: Span,
    },
    Position {
        within: QualName,
        span: Span,
    },
    Counter {
        via: Ident,
        opts: Vec<CounterOpt>,
        span: Span,
    },
    Variants(Vec<(Ident, String)>),
}

#[derive(Debug, Clone, Serialize)]
pub enum RefPolicy {
    Cascade(Span),
    Restrict(Span),
    SetNull(Span),
    Anonymize(Span),
    Reassign(Expr),
}

#[derive(Debug, Clone, Serialize)]
pub enum CounterOpt {
    Dedupe { by: Ident, within: Duration },
    Window(Duration),
    Sharded(i64),
}

#[derive(Debug, Clone, Serialize)]
pub enum Constraint {
    Unique { cols: Vec<Ident>, filter: Option<Expr>, code: Option<Ident>, span: Span },
    Cardinality { kind: CardKind, filter: Expr, per: Ident, repair: Option<Block>, span: Span },
    NoOverlap { range: Ident, per: Ident, code: Option<Ident>, span: Span },
    Capacity { count: SetExpr, limit: Expr, code: Ident, span: Span },
    Invariant { name: Ident, cond: Expr, span: Span },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CardKind {
    ExactlyOne,
    AtMostOne,
    AtLeastOne,
}

#[derive(Debug, Clone, Serialize)]
pub struct Lifecycle {
    pub field: Ident,
    pub transitions: Vec<Transition>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct Transition {
    pub from: Vec<Ident>,
    pub to: Vec<Ident>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct Visibility {
    pub unless: bool,
    pub cond: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct PredicateDecl {
    pub name: Ident,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub enum EntityTrait {
    Track {
        created: bool,
        updated: bool,
        by_actor: bool,
        span: Span,
    },
    History(Span),
    SoftDelete {
        retain: Option<Duration>,
        span: Span,
    },
    Versioned(Span),
    /// `publishable` or `publishable by <cond>`: who may publish and discard drafts of the entity.
    Publishable {
        by: Option<Expr>,
        span: Span,
    },
    Tenant {
        via: QualName,
        span: Span,
    },
    DynamicSchema {
        from: Ident,
        span: Span,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct Param {
    pub name: Ident,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelationDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub result: Option<Ident>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct FnDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub ret: TypeExpr,
    pub body: FnBody,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub enum FnBody {
    Expr(Expr),
    Wasm(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct EventDecl {
    pub name: Ident,
    pub version: Option<i64>,
    pub fields: Vec<(Ident, TypeExpr)>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpcastDecl {
    pub event: Ident,
    pub from: i64,
    pub to: i64,
    pub with: Vec<FieldAssign>,
    pub span: Span,
}

// ---------- intents ----------

#[derive(Debug, Clone, Serialize)]
pub struct Let {
    pub name: Ident,
    pub value: Expr,
    pub code: Option<Ident>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct Allow {
    pub cond: Expr,
    pub code: Option<Ident>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct Require {
    pub when: Option<Expr>,
    pub cond: Expr,
    pub code: Ident,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct Rate {
    pub n: i64,
    pub per: Duration,
    pub key: RateKey,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub enum RateKey {
    Actor,
    Client,
    Path(QualName),
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryDecl {
    pub internal: bool,
    /// `drafts query`: reads the working version of publishable entities instead of the published one.
    pub drafts: bool,
    pub cross_tenant: bool,
    pub name: Ident,
    pub params: Vec<Param>,
    pub cached: Option<(Duration, Option<Ident>)>,
    pub limits: Vec<Rate>,
    pub lets: Vec<Let>,
    pub allow: Option<Allow>,
    pub fetches: Vec<(CallExpr, Ident)>,
    pub from: Option<FromClause>,
    pub filter: Option<Expr>,
    pub group_by: Vec<Expr>,
    pub sort: Option<Sort>,
    pub page: Option<Page>,
    pub plan: Option<Ident>,
    pub consistency: Option<Ident>,
    pub select: Selection,
    pub touches: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub enum FromClause {
    /// `from Recruitment r`
    Entity { entity: Ident, alias: Ident },
    /// `from r` where r is an entity parameter
    Param(Ident),
    /// `from RecruitmentSearch.match(q) r`
    Call { call: CallExpr, alias: Ident },
}

#[derive(Debug, Clone, Serialize)]
pub enum Sort {
    Keys(Vec<SortKey>),
    ByParam { param: Ident, cases: Vec<(Ident, Vec<SortKey>)> },
}

#[derive(Debug, Clone, Serialize)]
pub struct SortKey {
    pub expr: Expr,
    pub desc: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Page {
    pub size: i64,
    pub offset_max_page: Option<i64>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct Selection {
    pub items: Vec<SelItem>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct SelItem {
    pub name: Ident,
    pub value: Option<Expr>,
    pub sub: Option<Selection>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandDecl {
    pub internal: bool,
    pub cross_tenant: bool,
    pub name: Ident,
    pub params: Vec<Param>,
    pub idempotent: Option<Option<Expr>>,
    pub audited: bool,
    pub limits: Vec<Rate>,
    pub lets: Vec<Let>,
    pub allow: Option<Allow>,
    pub requires: Vec<Require>,
    pub body: Option<Block>,
    pub emits: Vec<Emit>,
    pub returns: Option<(Expr, Option<Selection>)>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct Emit {
    pub event: Ident,
    pub fields: Vec<FieldAssign>,
    pub to: Option<EmitTarget>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct EmitTarget {
    pub broker: Ident,
    pub topic: String,
    pub key: Option<Expr>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubscribeDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub allow: Allow,
    pub from: FromClause,
    pub filter: Option<Expr>,
    pub select: Selection,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebhookDecl {
    pub name: Ident,
    pub via: CallTarget,
    pub handlers: Vec<WebhookOn>,
    pub span: Span,
}

/// `on "payment_intent.succeeded"(e: PaymentIntent) do { ... }`; the event may
/// also be an identifier when the provider's names are identifier-shaped.
#[derive(Debug, Clone, Serialize)]
pub struct WebhookOn {
    /// `cross tenant on ...`: this handler may write rows of several tenants.
    pub cross_tenant: bool,
    pub event: String,
    pub event_span: Span,
    pub binding: Ident,
    pub ty: Option<TypeExpr>,
    pub body: Block,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsumeDecl {
    pub cross_tenant: bool,
    pub broker: Ident,
    pub topic: String,
    pub name: Ident,
    pub key: Ident,
    pub dedupe: Option<Ident>,
    pub body: Block,
    pub span: Span,
}

// ---------- statements ----------

#[derive(Debug, Clone, Serialize)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub enum Stmt {
    Let(Let),
    Insert { entity: Ident, from: Option<SetExpr>, fields: Vec<FieldAssign>, bind: Option<Ident>, span: Span },
    Upsert { entity: Ident, keys: Vec<Ident>, fields: Vec<FieldAssign>, bind: Option<Ident>, span: Span },
    Update { target: SetExpr, via: Option<(Expr, Ident)>, assigns: Vec<Assign>, span: Span },
    Delete { target: SetExpr, span: Span },
    Purge { target: SetExpr, span: Span },
    Erase { target: Expr, span: Span },
    Toggle { entity: Ident, fields: Vec<FieldAssign>, span: Span },
    Set { assigns: Vec<Assign>, span: Span },
    When { cond: Expr, body: Block, span: Span },
    Each { source: SetExpr, body: Block, span: Span },
    Effect { call: CallExpr, bind: Option<Ident>, into: Option<Expr>, on_failure: Option<Block>, span: Span },
    Reserve { op: Ident, what: Ident, of: Expr, duration: Option<Duration>, code: Option<Ident>, span: Span },
    AtRun { at: Expr, intent: Ident, args: Vec<Arg>, span: Span },
    Notify(Notify),
    ExportPersonalData { of: Expr, to: Ident, notify: Option<Expr>, span: Span },
}

#[derive(Debug, Clone, Serialize)]
pub struct Notify {
    /// Recipients: a member path (`e.comment.author`) or a set (`ClubMember m where ...`).
    pub to: SetExpr,
    pub via: CallTarget,
    pub fields: Vec<FieldAssign>,
    pub category: Option<Ident>,
    pub digest: Option<Duration>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct Assign {
    pub target: Expr,
    pub op: AssignOp,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AssignOp {
    Set,
    Add,
    Sub,
}

#[derive(Debug, Clone, Serialize)]
pub enum FieldAssign {
    /// `name: expr` or shorthand `name` (value = same-named binding)
    Named { name: Ident, value: Option<Expr> },
    /// `...expr`
    Spread(Expr),
}

// ---------- expressions ----------

#[derive(Debug, Clone, Serialize)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Kw {
    Actor,
    SelfRow,
    This,
    Now,
    Today,
    Public,
    Authenticated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AggFn {
    Count,
    Sum,
    Min,
    Max,
    Avg,
}

#[derive(Debug, Clone, Serialize)]
pub struct CallExpr {
    pub callee: QualName,
    pub args: Vec<Arg>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct Arg {
    pub name: Option<Ident>,
    pub value: Expr,
}

/// `via` target: a call whose parentheses may be omitted.
#[derive(Debug, Clone, Serialize)]
pub struct CallTarget {
    pub name: QualName,
    pub args: Option<Vec<Arg>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetExpr {
    pub source: Expr,
    pub alias: Option<Ident>,
    pub filter: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub enum ExprKind {
    Int(i64),
    Decimal(String),
    Str(String),
    Bool(bool),
    Null,
    Duration(Duration),
    Size(u64),
    TimeOfDay(u8, u8),
    Name(Ident),
    Kw(Kw),
    Field(Box<Expr>, Ident),
    Call(CallExpr),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    InList(Box<Expr>, Vec<Expr>),
    InRange(Box<Expr>, Box<Expr>, Box<Expr>),
    InExpr(Box<Expr>, Box<Expr>),
    Is(Box<Expr>, Ident),
    HasScope(Box<Expr>, QualName),
    Exists(Box<SetExpr>),
    The(Box<SetExpr>),
    Latest(Box<SetExpr>, Box<Expr>),
    First(Box<SetExpr>, Vec<SortKey>),
    Agg {
        func: AggFn,
        set: Option<Box<SetExpr>>,
        body: Option<Box<Expr>>,
    },
    Quant {
        all: bool,
        set: Box<SetExpr>,
        body: Box<Expr>,
    },
    RunningSum {
        value: Box<Expr>,
        over: Ident,
        order: Box<Expr>,
    },
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    List(Vec<Expr>),
    /// Call argument of the form `items x: body` (e.g. `same(items x: x.club)`).
    Binder(Box<SetExpr>, Box<Expr>),
}

// ---------- time, rules, L3 forms ----------

#[derive(Debug, Clone, Serialize)]
pub struct OnEventDecl {
    pub cross_tenant: bool,
    pub event: Ident,
    pub binding: Ident,
    pub when: Option<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScheduleDecl {
    pub cross_tenant: bool,
    pub name: Ident,
    pub every: Every,
    pub at: Option<(u8, u8)>,
    pub tz: Option<String>,
    pub catch_up_once: Option<bool>,
    pub for_each: Option<SetExpr>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub enum Every {
    Day,
    Week(Ident),
    MonthDay(i64),
    Interval(Duration),
}

#[derive(Debug, Clone, Serialize)]
pub struct RetainDecl {
    pub entity: Ident,
    pub keep: Duration,
    pub after: QualName,
    pub anonymize: bool,
    pub notify: Option<(QualName, Duration, CallTarget)>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleDecl {
    pub cross_tenant: bool,
    pub name: Ident,
    pub entity: Ident,
    pub alias: Ident,
    pub when: Expr,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectionDecl {
    pub name: Ident,
    pub events: Vec<Ident>,
    pub key: Ident,
    pub handlers: Vec<(Ident, Ident, Stmt)>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchDecl {
    pub name: Ident,
    pub entity: Ident,
    pub fields: Vec<(Ident, Option<Ident>)>,
    pub language: Option<Ident>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub allow: Allow,
    pub progress: Option<SetExpr>,
    pub produce: Option<(Ident, Ident, String, Option<Duration>)>,
    pub body: Option<Block>,
    pub notify: Option<(Expr, CallTarget)>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerificationDecl {
    pub name: Ident,
    pub subject: Ident,
    pub target: TypeExpr,
    pub target_where: Option<Expr>,
    pub alnum: bool,
    pub length: i64,
    pub ttl: Duration,
    pub attempts: i64,
    pub resend_after: Option<Duration>,
    pub deliver: CallTarget,
    pub on_verified: (Ident, Ident, Block),
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct GrantLinkDecl {
    pub name: Ident,
    pub grants: Option<(Expr, Ident)>,
    pub scope: Vec<Param>,
    /// Inputs the holder supplies when redeeming (`redeem with (name: Text)`).
    pub redeem_params: Vec<Param>,
    pub issued_by: Expr,
    pub to: Option<Expr>,
    pub expires: Duration,
    pub uses: Option<i64>,
    pub requires: Vec<Require>,
    pub on_redeem: Option<Block>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalDecl {
    pub name: Ident,
    pub entity: Ident,
    pub alias: Ident,
    pub approvers: SetExpr,
    /// Who may open a request; without it anyone who can see the row may.
    pub requested_by: Option<Expr>,
    pub required: i64,
    pub no_self: bool,
    pub on_approved: Block,
    pub on_rejected: Block,
    pub expires: Option<Duration>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExposeDecl {
    pub entity: Ident,
    pub read: Option<Option<Expr>>,
    pub create: Option<(Expr, Vec<Ident>)>,
    pub update: Option<(Expr, Vec<Ident>)>,
    pub delete: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutboundDecl {
    pub entity: Ident,
    pub alias: Ident,
    pub events: Vec<Ident>,
    pub filter: Option<Expr>,
    pub sign: Ident,
    pub retry: i64,
    pub over: Duration,
    pub disable_after: Option<Duration>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsentDecl {
    pub name: Ident,
    pub version: i64,
    pub intents: Vec<Ident>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigDecl {
    pub name: Ident,
    pub ty: TypeExpr,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlagDecl {
    pub name: Ident,
    pub default_on: bool,
    pub rollout: Option<i64>,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpersonateDecl {
    pub entity: Ident,
    pub by: Expr,
    pub ttl: Duration,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct MigrationDecl {
    pub name: Ident,
    pub body: Block,
    pub span: Span,
}
