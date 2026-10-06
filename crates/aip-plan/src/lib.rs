//! Executable plan produced by the compiler and consumed by the runtime.
//!
//! Every SQL statement the runtime will ever run is fixed here at compile
//! time, which is what makes `aip explain` exact. Parameters are named; the
//! runtime binds each one as text and the SQL casts it (`$1::text::uuid`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const IR_VERSION: &str = "0.1";

/// PostgreSQL notification channel the change triggers of a subscription's tables notify on (payload: the table name).
pub const CHANGE_CHANNEL: &str = "aip_changed";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Program {
    pub ir_version: String,
    pub actor: Option<ActorSpec>,
    pub enums: BTreeMap<String, Vec<String>>,
    pub records: BTreeMap<String, Vec<ParamSpec>>,
    pub entities: BTreeMap<String, EntitySpec>,
    /// Ordered DDL: tables, then constraints, indexes, triggers.
    pub ddl: Vec<String>,
    pub intents: BTreeMap<String, Intent>,
    pub handlers: Vec<Handler>,
    pub schedules: Vec<Schedule>,
    #[serde(default)]
    pub jobs: Vec<Job>,
    #[serde(default)]
    pub webhooks: Vec<Webhook>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// `outbound webhooks`: how the runtime delivers what the form's handlers queue.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outbound: Vec<Outbound>,
    pub events: BTreeMap<String, Vec<(String, TypeSpec)>>,
    /// `subscribe`: queries a client may keep open, re-run when a table they read changes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<Subscription>,
    /// `migration`: data migrations, run once each in declaration order after the schema is in place.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub migrations: Vec<Migration>,
    /// `impersonate`: the intents the runtime recognises to issue and honour impersonation tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impersonation: Option<ImpersonationSpec>,
    pub extensions: Vec<String>,
    /// Database constraint / trigger name -> contract error.
    pub constraint_errors: BTreeMap<String, ErrorSpec>,
}

/// A declared subscription. `query` is the one statement set that produces its rows (a list query nobody can call over
/// HTTP); the runtime runs it for the subscriber, again and again, with the same checks every time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub name: String,
    pub query: QueryPlan,
    /// Tables the plan's SQL reads, the ones whose change can change the rows. A commit that touches one of them wakes the subscription.
    pub reads: Vec<String>,
}

/// A data migration. `digest` identifies the body that was run, so a body edited after it ran is noticed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Migration {
    pub name: String,
    pub digest: String,
    pub steps: Vec<Step>,
}

/// How the runtime handles impersonation: `start` returns the session, the HTTP layer signs it into a token
/// that lives until `expires_at`; every call carrying such a token is checked against the session row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpersonationSpec {
    pub start: String,
    pub stop: String,
    pub ttl_seconds: u64,
    /// Boolean SQL over a CTE named `s` (the open session row): does the operator still satisfy `by` (or the
    /// superuser test)? The engine evaluates it with the session lookup on every call, so a session ends when the
    /// operator loses the power it was opened with.
    pub operator_check: Sql,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorSpec {
    pub entity: String,
    pub table: String,
    pub provider: String,
    /// Boolean SQL over `$actor`; when true, every allow/visibility passes (audited).
    pub superuser: Option<Sql>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntitySpec {
    pub table: String,
    pub columns: Vec<ColumnSpec>,
    pub personal: bool,
    /// Where the published copy of a `publishable` entity lives. Only set when a column is encrypted: key rotation
    /// has to re-encrypt the copy too, and the plans of every other entity stay as they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_table: Option<String>,
    /// Table of the versions of a `history` entity (one JSON document per row version). Only set when a column is encrypted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_table: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnSpec {
    pub field: String,
    pub column: String,
    pub ty: TypeSpec,
    pub nullable: bool,
    pub personal: bool,
    /// Stored as AES-256-GCM ciphertext; the runtime encrypts before binding and decrypts before answering.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub encrypted: bool,
}

/// Where an encrypted value sits in the JSON a query or command returns, so the runtime decrypts exactly those
/// places and nothing else. Every other string in a result is data, even one that looks like ciphertext.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecryptPath {
    /// Object keys from the root of the result; `[]` stands for every element of an array.
    pub path: Vec<String>,
    /// `Entity.field`: with the row id, the context the value was encrypted for.
    pub field: String,
}

/// Which row an encrypted value is for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "row", rename_all = "snake_case")]
pub enum EncryptRow {
    /// A row the next statement inserts: its id was picked by a `NewId` step and is in the environment under `id`.
    New { id: String },
    /// A row that exists: `id` evaluates to its id.
    Existing { id: Sql },
}

/// A cell of an exported row that holds ciphertext; the first cell of the row is its id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecryptColumn {
    pub column: usize,
    pub field: String,
}

/// A parameterised SQL fragment. `params[i]` is the name bound to `$i+1`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Sql {
    pub text: String,
    pub params: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeSpec {
    Bool,
    Int { min: Option<i64>, max: Option<i64> },
    Decimal,
    Text { min: Option<i64>, max: Option<i64>, trim: bool, lower: bool, pattern: Option<String> },
    RichText,
    Email,
    Url,
    Phone,
    Time,
    Date,
    Duration,
    Uuid,
    Money { currency: String },
    Json,
    Enum { name: String },
    Record { name: String },
    Entity { entity: String },
    Union { entities: Vec<String> },
    Snapshot { entity: String },
    Set { of: Box<TypeSpec>, max: u64 },
    List { of: Box<TypeSpec>, max: u64 },
    Range { of: Box<TypeSpec> },
    Upload { max_bytes: Option<u64>, types: Vec<String> },
    Object,
    Other { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamSpec {
    pub name: String,
    pub ty: TypeSpec,
    pub optional: bool,
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Intent {
    Query(QueryPlan),
    Command(CommandPlan),
}

impl Intent {
    pub fn name(&self) -> &str {
        match self {
            Intent::Query(q) => &q.name,
            Intent::Command(c) => &c.name,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPlan {
    pub name: String,
    pub params: Vec<ParamSpec>,
    pub internal: bool,
    /// Loads, lets and the allow check, run before the main statement.
    pub prelude: Vec<Step>,
    /// Main statements; `when` selects a variant by the sort parameter's value.
    /// Each returns one JSON value: an object (single row) or an array (list).
    pub variants: Vec<QueryVariant>,
    pub sort_param: Option<String>,
    pub single: bool,
    pub page: Option<PageSpec>,
    pub cache_seconds: Option<u64>,
    pub touches: Vec<Touch>,
    pub rate_limits: Vec<RateLimit>,
    pub errors: Vec<ErrorSpec>,
    /// Output shape for the contract (see `aip-pg/src/shape.rs`).
    #[serde(default)]
    pub output: serde_json::Value,
    /// Encrypted values in the result, decrypted before it is returned.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decrypt: Vec<DecryptPath>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryVariant {
    pub when: Option<String>,
    pub main: Sql,
    /// Number of keyset key columns (each row carries them in `__k`).
    pub key_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSpec {
    pub size: u64,
    pub keyset: bool,
    pub max_page: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Touch {
    pub entity: String,
    pub field: String,
    pub id: Sql,
    pub dedupe_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    pub n: u64,
    pub per_seconds: u64,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandPlan {
    pub name: String,
    pub params: Vec<ParamSpec>,
    pub internal: bool,
    pub idempotent: bool,
    pub idempotency_key: Option<Sql>,
    pub audited: bool,
    pub steps: Vec<Step>,
    pub returns: Option<Sql>,
    pub rate_limits: Vec<RateLimit>,
    pub errors: Vec<ErrorSpec>,
    pub effects: Vec<String>,
    pub writes: Vec<String>,
    pub emits: Vec<String>,
    #[serde(default)]
    pub output: serde_json::Value,
    /// Encrypted values in what `returns` selects, decrypted before it is returned (and before it is kept for an idempotent replay: the kept copy stays encrypted).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decrypt: Vec<DecryptPath>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ErrorSpec {
    pub code: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    Allow,
    Require,
    Transition,
    /// Optimistic concurrency: the caller edited a stale version.
    Version,
    /// The rows the call names, or a value it writes, are not all in one tenant.
    Tenant,
    /// The caller has not consented to the current version of a consent the intent requires; `code` is its name.
    Consent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Step {
    /// Loads an entity (or set of entities) parameter. `sql` returns the ids
    /// that exist and are visible; fewer than requested means NOT_FOUND.
    Load {
        param: String,
        entity: String,
        many: bool,
        lock: bool,
        sql: Sql,
    },
    /// Locks rows in global order before any check reads them.
    Lock {
        sql: Sql,
    },
    /// Evaluates one value; NULL with `code` fails the intent.
    Let {
        name: String,
        sql: Sql,
        code: Option<String>,
    },
    /// Boolean check. Allow failures become FORBIDDEN/UNAUTHENTICATED.
    Check {
        kind: CheckKind,
        sql: Sql,
        code: Option<String>,
    },
    /// Picks the id of a row about to be inserted and keeps it as `name`: the insert names it, and the ciphertext of the
    /// row's encrypted fields is bound to it.
    NewId {
        name: String,
    },
    /// Encrypts `source` (or its member `member`, when it is a record) for one row of `field` (`Entity.field`) and binds
    /// the ciphertext as `bind` for the statement that follows.
    Encrypt {
        field: String,
        source: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        member: Option<String>,
        bind: String,
        row: EncryptRow,
    },
    /// Mutation. `bind` captures `RETURNING id`.
    Exec {
        sql: Sql,
        bind: Option<String>,
        label: String,
    },
    When {
        cond: Sql,
        steps: Vec<Step>,
    },
    EachPartial {
        source: String,
        item: String,
        steps: Vec<Step>,
    },
    /// Runs `steps` once per element of the JSON array computed by `source`.
    ForEach {
        source: Sql,
        item: String,
        steps: Vec<Step>,
    },
    /// Staged upload of an Upload parameter; the object key is stored into `column` after commit-safe staging.
    Upload {
        param: String,
        bucket: String,
        bind: Option<String>,
        target: Option<(String, String, String)>,
    },
    /// Extension effect placed after commit via the outbox.
    Deferred {
        effect: String,
        args: Sql,
        key: Option<Sql>,
        on_failure: Vec<Step>,
    },
    Emit {
        event: String,
        payload: Sql,
        broker: Option<(String, String)>,
    },
    Notify {
        recipients: Sql,
        template: String,
        payload: Sql,
        category: Option<String>,
        digest_seconds: Option<u64>,
    },
    /// Validates a JSON value against a dynamic schema (list of questions) selected by `schema`.
    ValidateDynamic {
        value: Sql,
        schema: Sql,
        path: String,
    },
    /// Fails the intent with PRECONDITION(code); with `commit`, the work done so far
    /// (e.g. a spent verification attempt) is committed first.
    Fail {
        code: String,
        commit: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handler {
    pub event: String,
    pub name: String,
    pub when: Option<Sql>,
    pub steps: Vec<Step>,
}

/// `rule X on E e when cond do { ... }`: triggers mark the affected `E` rows
/// pending; before every commit the runtime evaluates them and runs the body
/// for rows whose condition has just become true.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub name: String,
    pub entity: String,
    /// Environment name the row id is bound to.
    pub binding: String,
    /// True while the row exists and the condition holds.
    pub when: Sql,
    pub steps: Vec<Step>,
}

/// An inbound webhook at `POST /aip/webhooks/<name>`. The request is verified,
/// deduplicated by event id and stored in the outbox; handlers run from there
/// with retries, so the provider gets its 2xx without waiting for them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Webhook {
    pub name: String,
    pub source: String,
    /// `hmac-sha256` (hex digest of the body, optional `sha256=` prefix) or `stripe`.
    pub scheme: String,
    pub header: String,
    /// Name of the environment variable holding the secret.
    pub secret_env: String,
    /// Dotted JSON paths into the request body.
    pub event_path: String,
    pub id_path: String,
    pub payload_path: String,
    pub handlers: Vec<WebhookHandler>,
}

/// `outbound webhooks for E`: the events become delivery rows (by the form's handlers, in the transaction of the
/// event's dispatch); the runtime sends them, signed, to the `url` of the endpoint row they were queued for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outbound {
    /// Endpoint entity, which also names the form.
    pub entity: String,
    pub table: String,
    pub url_column: String,
    pub soft_delete: bool,
    pub events: Vec<String>,
    /// Retries after the first attempt, all within `over_seconds` of the event.
    pub retry: u32,
    pub over_seconds: u64,
    /// Failing without a success for this long disables the endpoint.
    pub disable_after_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookHandler {
    pub event: String,
    /// Environment name the payload JSON is bound to.
    pub binding: String,
    pub steps: Vec<Step>,
}

/// A durable background task started by an intent of the same name and run
/// by the job worker in batches, each batch in its own transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub name: String,
    /// Parameter names restored from the stored JSON into the environment.
    pub params: Vec<String>,
    /// `progress over`: a JSON array of the item ids the job walks (snapshot at start).
    pub source: Option<Sql>,
    /// Environment name the current item id is bound to.
    pub item: Option<String>,
    /// Re-checks and locks one item before its steps run; no row means the item
    /// no longer belongs to the set (changed since the snapshot) and is skipped.
    #[serde(default)]
    pub item_guard: Option<Sql>,
    /// Runs once per item with `progress`, otherwise once. At least once per item.
    pub steps: Vec<Step>,
    pub export: Option<JobExport>,
    /// Runs after the last item with `__job` and `__file` set.
    pub finish: Vec<Step>,
    pub batch: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobExport {
    pub format: String,
    pub bucket: String,
    pub header: Vec<String>,
    /// Given `__ids` (JSON array), returns a JSON array of rows (arrays of text).
    pub rows: Sql,
    pub expires_seconds: Option<u64>,
    /// Cells that hold ciphertext, decrypted before they are written to the file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decrypt: Vec<DecryptColumn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub name: String,
    pub cron: String,
    pub tz: Option<String>,
    pub source: Option<Sql>,
    pub item: Option<String>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// A problem a backend found while compiling a Core IR program. `path` is the
/// IR path of the node it concerns; `line`/`col` come from the source map
/// (0 when the producer gave none).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub path: String,
    pub line: u32,
    pub col: u32,
}

impl Diagnostic {
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// Same text the frontend diagnostics use: `error AIP-E600 file:3:5  message`.
    pub fn render(&self, file: &str) -> String {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        format!("{sev} {} {file}:{}:{}  {}", self.code, self.line, self.col, self.message)
    }
}

// ---------- schema evolution ----------

/// How a step of an [`EvolvePlan`] relates to the data already in the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepClass {
    /// Adds something; no stored value is touched or at risk.
    Safe,
    /// Removes a rule or a derived object (a constraint, an index, a `NOT NULL`); no stored value is lost.
    Relaxing,
    /// Adds a rule the stored rows must already satisfy; the checks of the step run first and refuse the change if they find rows.
    Checked,
    /// Replaces a function or trigger the generator owns, so it always matches the program.
    Refresh,
    /// Destroys data, and the program says so (`removed field`, `removed entity`).
    Declared,
}

/// A query that counts the rows a rule would reject, and one that names some of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataCheck {
    pub what: String,
    /// `SELECT count(*)` of the rows that break the rule.
    pub count_sql: String,
    /// Ids (as text) of some of those rows, at most five.
    pub sample_sql: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvolveStep {
    /// Empty when the step only carries checks.
    pub sql: String,
    pub class: StepClass,
    pub why: String,
    pub checks: Vec<DataCheck>,
}

/// A change the planner will not make by itself, with the registry code that explains it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    pub code: String,
    pub message: String,
    pub fix: String,
}

/// The ordered steps that bring a database deployed with one program to another, and what stops them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvolvePlan {
    pub steps: Vec<EvolveStep>,
    pub rejections: Vec<Rejection>,
    /// Effects that are allowed but worth knowing (an ordered enum gained a value in the middle, a constraint was dropped).
    pub notes: Vec<String>,
}
