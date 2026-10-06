//! L3 domain forms that are still carried as forms in Core IR.
//!
//! `docs/design/03-ir.md` §10.1 says L3 forms have no node of their own in
//! Core IR: they lower to entities, intents, constraints, timers and effects.
//! Today the PostgreSQL backend lowers them straight to SQL instead, so v0 of
//! Core IR keeps them as typed forms with resolved names. Each one moves out
//! of this module when its L3→L2 lowering exists (open issue OI-IR2).

use crate::*;

// forms are cold data held in a Vec; boxing would not change the serialized shape but would add noise to every consumer
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum Form {
    Subscribe(Subscribe),
    Projection(Projection),
    Search(Search),
    Job(Job),
    Verification(Verification),
    GrantLink(GrantLink),
    Approval(Approval),
    OutboundWebhooks(OutboundWebhooks),
    Consent(Consent),
    Impersonate(Impersonate),
    Migration(Migration),
    Upcast(Upcast),
}

/// Live query pushed to the client when its rows change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subscribe {
    pub name: String,
    pub params: Vec<Param>,
    pub allow: Policy,
    pub source: QuerySource,
    pub filter: Option<Expr>,
    pub select: Selection,
    /// What one update carries: the list of selected rows.
    pub output: Shape,
}

impl Subscribe {
    /// The list query a subscription is: every refresh answers what this query would, for the same caller. Nobody can call it
    /// as an intent (`internal`), so its meaning, errors and plan are exactly those of a query without being one.
    pub fn as_query(&self) -> Query {
        Query {
            internal: true,
            drafts: false,
            cross_tenant: false,
            params: self.params.clone(),
            cache: None,
            rate_limits: Vec::new(),
            lets: Vec::new(),
            allow: Some(self.allow.clone()),
            fetches: Vec::new(),
            source: Some(self.source.clone()),
            filter: self.filter.clone(),
            group_by: Vec::new(),
            sort: None,
            page: None,
            plan: None,
            consistency: None,
            select: self.select.clone(),
            touches: Vec::new(),
            output: self.output.clone(),
        }
    }
}

/// Read model folded from events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Projection {
    pub name: String,
    pub events: Vec<String>,
    pub key: String,
    /// `(event, binding, statement)`.
    pub handlers: Vec<(String, String, Stmt)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Search {
    pub name: String,
    pub entity: String,
    /// `(field, weight)`.
    pub fields: Vec<(String, Option<String>)>,
    pub language: Option<String>,
}

/// Durable background task: a start intent, a status query and a worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub name: String,
    pub params: Vec<Param>,
    pub allow: Policy,
    /// Items the job walks, snapshotted at start.
    pub progress: Option<SetExpr>,
    pub produce: Option<JobProduce>,
    pub body: Option<Vec<Stmt>>,
    pub notify: Option<(Expr, Call)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobProduce {
    pub format: String,
    /// Extension that stores the file (`s3`).
    pub store: String,
    pub bucket: String,
    pub expires_seconds: Option<i64>,
}

/// One-time code sent to a target and checked with limited attempts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verification {
    pub name: String,
    pub subject: String,
    pub target: Type,
    pub target_where: Option<Expr>,
    pub alnum: bool,
    pub length: i64,
    pub ttl_seconds: i64,
    pub attempts: i64,
    pub resend_after_seconds: Option<i64>,
    pub deliver: Call,
    /// `(subject binding, target binding, body)`.
    pub on_verified: (String, String, Vec<Stmt>),
}

/// Shareable link granting a scoped right, redeemed by its holder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrantLink {
    pub name: String,
    /// `grants <membership expr> as <role>`.
    pub grants: Option<(Expr, String)>,
    pub scope: Vec<Param>,
    pub redeem_params: Vec<Param>,
    pub issued_by: Expr,
    pub to: Option<Expr>,
    pub expires_seconds: i64,
    pub uses: Option<i64>,
    pub requires: Vec<Require>,
    pub on_redeem: Option<Vec<Stmt>>,
}

/// N-of-M approval of a row by a set of approvers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Approval {
    pub name: String,
    pub entity: String,
    pub alias: String,
    pub approvers: SetExpr,
    pub requested_by: Option<Expr>,
    pub required: i64,
    pub no_self: bool,
    pub on_approved: Vec<Stmt>,
    pub on_rejected: Vec<Stmt>,
    pub expires_seconds: Option<i64>,
}

/// Signed event delivery to endpoints registered by users.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboundWebhooks {
    pub entity: String,
    pub alias: String,
    pub events: Vec<String>,
    pub filter: Option<Expr>,
    pub sign: String,
    pub retry: i64,
    pub over_seconds: i64,
    pub disable_after_seconds: Option<i64>,
}

/// Versioned consent required before the listed intents: commands, queries and job starts the caller may invoke.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Consent {
    pub name: String,
    pub version: i64,
    pub intents: Vec<String>,
}

/// An operator acts as one user of the app for a while. `audited reason required` is part of the syntax, not of the
/// meaning: every session is audited and every start needs a reason, so the form carries neither.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Impersonate {
    /// The entity the operator acts as; the actor entity.
    pub entity: String,
    /// Who may start: a condition over the actor only.
    pub by: Expr,
    pub ttl_seconds: i64,
}

/// Data migration run once per deployment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Migration {
    pub name: String,
    pub body: Vec<Stmt>,
}

impl Migration {
    /// `sha256:<hex>` of the canonical JSON of the body. A migration that already ran is identified by its name, and this
    /// is what tells whether the body it ran is still the one written now.
    pub fn digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let json = serde_json::to_string(&self.body).unwrap_or_default();
        let h = Sha256::digest(json.as_bytes());
        let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
        format!("sha256:{hex}")
    }
}

/// Upgrades stored events of an older version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Upcast {
    pub event: String,
    pub from: i64,
    pub to: i64,
    pub with: Vec<(String, Expr)>,
}
