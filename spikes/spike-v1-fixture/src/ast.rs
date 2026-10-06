use crate::diag::Span;

#[derive(Debug, Clone, Default)]
pub struct Spec {
    pub enums: Vec<EnumDecl>,
    pub actor: Option<(String, Span)>,
    pub predicates: Vec<Predicate>,
    pub accesses: Vec<Access>,
    pub limits: Vec<Limit>,
    pub resources: Vec<Resource>,
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub variants: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypeRef {
    pub name: String,
    pub id_of: bool,
    pub nullable: bool,
    pub range: Option<(i64, i64)>,
    pub span: Span,
}

pub type Params = Vec<(String, TypeRef)>;

#[derive(Debug, Clone)]
pub struct Predicate {
    pub name: String,
    pub params: Params,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum AccessBody {
    TotalOfVisible(String, Span),
    Guard(Expr),
}

#[derive(Debug, Clone)]
pub struct Access {
    pub name: String,
    pub params: Params,
    pub body: AccessBody,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Limit {
    pub name: String,
    pub on: String,
    pub at_most: i64,
    pub cond: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub ty: TypeRef,
    pub span: Span,
}

#[derive(Debug, Clone, Default)]
pub struct Budget {
    pub rows: Option<i64>,
    pub depth: Option<i64>,
    pub deadline_ms: Option<u64>,
    pub cost: Option<i64>,
    pub span: Span,
}

pub type TraverseSelection = (String, Vec<(String, Span)>, Span);

#[derive(Debug, Clone, Default)]
pub struct ExposeRead {
    pub select: Vec<(String, Span)>,
    pub filter: Vec<(String, String, Span)>,
    pub sort: Vec<(String, Span)>,
    pub traverse: Vec<TraverseSelection>,
    pub budget: Option<Budget>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum AccessRef {
    Name(String),
    Call(String, Vec<Expr>),
}

#[derive(Debug, Clone)]
pub struct Aggregate {
    pub name: String,
    pub ty: TypeRef,
    pub input: Params,
    pub source: Option<String>,
    pub source_access: Option<AccessRef>,
    pub group_key: Option<String>,
    pub where_: Option<Expr>,
    pub caller_filter: Option<String>,
    pub row_output: Option<String>,
    pub release: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Transition {
    pub name: String,
    pub from: Expr,
    pub to: Vec<(String, Expr)>,
    pub allow: Expr,
    pub repeat: Option<(String, Span)>,
    pub effects: Vec<Effect>,
    pub span: Span,
}

/// 서버가 정의한 전이 효과. 값은 대상 행에서 서버가 계산한다(호출자 값 아님).
#[derive(Debug, Clone)]
pub enum Effect {
    Create {
        resource: String,
        values: Vec<(String, Expr)>,
        span: Span,
    },
    /// 대상 행에서 계산한 값으로 다른 행 하나를 찾아 바꾼다(예: 위임 시 actor 자신의 역할). 찾은 행은 대상당 정확히 하나여야 한다.
    Update {
        resource: String,
        matches: Vec<(String, Expr)>,
        values: Vec<(String, Expr)>,
        span: Span,
    },
    Notify {
        to: Expr,
        topic: String,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub struct ExposeCreate {
    pub allow: Option<Expr>,
    pub fields: Vec<(String, Span)>,
    pub span: Span,
}

/// W1 실험: 호출자가 항목별 단계를 조합하되, 허용 단계와 값 출처는 서버가 선언한다.
#[derive(Debug, Clone)]
pub struct ExposeCompose {
    pub bulk: Option<i64>,
    pub same_scope: Option<Expr>,
    pub transitions: Vec<(String, Span)>,
    pub creates: Vec<(String, Vec<Expr>, Span)>,
    /// 요청자 자신의 행: (actor를 가리키는 필드, 대상과 같아야 하는 범위 필드). 위임처럼 대상과 자기 행에 다른 단계를 쓸 때.
    pub self_row: Option<(String, String, Span)>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ExposeApply {
    pub transition: String,
    pub targets: Vec<(String, Span)>,
    pub bulk: Option<i64>,
    pub same_scope: Option<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Extension {
    pub kind: String,
    pub name: String,
    pub input: Params,
    pub output: Params,
    pub access: Vec<(String, String)>,
    pub effect: String,
    pub deadline_ms: Option<u64>,
    pub implementation: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Docs {
    pub summary: Option<String>,
    pub visibility: Option<String>,
    pub span: Span,
}

#[derive(Debug, Clone, Default)]
pub struct Resource {
    pub name: String,
    pub span: Span,
    pub fields: Vec<Field>,
    pub row_read: Option<Expr>,
    pub field_read: Vec<(String, Expr, Span)>,
    pub expose_read: Option<ExposeRead>,
    pub expose_aggregates: Vec<(String, Span)>,
    pub expose_apply: Vec<ExposeApply>,
    pub expose_create: Option<ExposeCreate>,
    pub expose_compose: Option<ExposeCompose>,
    pub uniques: Vec<(Vec<(String, Span)>, Span)>,
    pub checks: Vec<(String, Expr, Span)>,
    pub aggregates: Vec<Aggregate>,
    pub transitions: Vec<Transition>,
    pub invariants: Vec<(String, String, Span)>,
    /// 커밋 시점에 검사하는 불변식 이름. 중간 상태(예: 관리자 교대 중 2명)를 허용한다.
    pub deferred_invariants: Vec<String>,
    pub extensions: Vec<Extension>,
    pub docs: Option<Docs>,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Or(Vec<Expr>),
    And(Vec<Expr>),
    Not(Box<Expr>),
    Cmp(&'static str, Box<Expr>, Box<Expr>),
    In(Box<Expr>, Vec<Expr>),
    Path(Vec<String>, Span),
    Null,
    Now,
    Int(i64),
    Bool(bool),
    Str(String),
    Call(String, Vec<Expr>, Span),
    Exists(String, Box<Expr>, Span),
    /// `+`/`-`. 의미 검사는 전이 `to`의 `필드 = 같은 필드 ± 정수`만 받는다.
    Arith(&'static str, Box<Expr>, Box<Expr>, Span),
}
