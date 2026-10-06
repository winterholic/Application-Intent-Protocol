//! Entities -> DDL. Invariants are enforced by the database wherever possible:
//! UNIQUE / partial UNIQUE indexes, EXCLUDE constraints, CHECK, and triggers
//! for conditions that reach through references or count rows.

use crate::names::{lit, q, snake};
use crate::schema::{Col, Schema, sql_type};
use crate::sqlexpr::{Compiler, Val};
use crate::ty::Ty;
use aip_ir::codes;
use aip_ir::{self as ir, BinOp, Constraint, Expr, Generated, Literal, Node, Program, RefPolicy, SetExpr};
use aip_plan::ErrorSpec;
use std::collections::BTreeMap;

pub struct Ddl {
    pub statements: Vec<String>,
    /// constraint / trigger error name -> contract error
    pub codes: BTreeMap<String, ErrorSpec>,
}

pub const INTERNAL_TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS "_aip_outbox" (
  "id" bigserial PRIMARY KEY,
  "kind" text NOT NULL,
  "name" text NOT NULL,
  "intent" text,
  "key" text,
  "payload" jsonb NOT NULL,
  "available_at" timestamptz NOT NULL DEFAULT now(),
  "attempts" int NOT NULL DEFAULT 0,
  "last_error" text,
  "done_at" timestamptz,
  "created_at" timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS "_aip_outbox_pending" ON "_aip_outbox" ("available_at") WHERE "done_at" IS NULL;
CREATE TABLE IF NOT EXISTS "_aip_idempotency" (
  "intent" text NOT NULL,
  "actor" text NOT NULL,
  "key" text NOT NULL,
  "request_hash" text NOT NULL,
  "response" jsonb,
  "created_at" timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY ("intent", "actor", "key")
);
CREATE TABLE IF NOT EXISTS "_aip_audit" (
  "id" bigserial PRIMARY KEY,
  "intent" text NOT NULL,
  "actor" uuid,
  "superuser" boolean NOT NULL DEFAULT false,
  "input" jsonb NOT NULL,
  "at" timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS "_aip_counter_seen" (
  "key" text PRIMARY KEY,
  "expires_at" timestamptz NOT NULL
);
CREATE TABLE IF NOT EXISTS "_aip_rate" (
  "key" text NOT NULL,
  "window_start" timestamptz NOT NULL,
  "count" int NOT NULL,
  PRIMARY KEY ("key", "window_start")
);
CREATE TABLE IF NOT EXISTS "_aip_schedule_run" (
  "name" text PRIMARY KEY,
  "last_run" timestamptz NOT NULL
);
CREATE TABLE IF NOT EXISTS "_aip_processed" (
  "handler" text NOT NULL,
  "outbox_id" bigint NOT NULL,
  PRIMARY KEY ("handler", "outbox_id")
);
CREATE TABLE IF NOT EXISTS "_aip_notification" (
  "id" bigserial PRIMARY KEY,
  "recipient" uuid NOT NULL,
  "template" text NOT NULL,
  "data" jsonb NOT NULL,
  "category" text,
  "read_at" timestamptz,
  "created_at" timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS "_aip_notification_recipient" ON "_aip_notification" ("recipient", "created_at" DESC);
CREATE TABLE IF NOT EXISTS "_aip_mail" (
  "id" bigserial PRIMARY KEY,
  "template" text NOT NULL,
  "to" text,
  "data" jsonb NOT NULL,
  "created_at" timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS "_aip_grant" (
  "id" uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  "link" text NOT NULL,
  "token_hash" text NOT NULL UNIQUE,
  "scope" jsonb NOT NULL,
  "to_member" uuid,
  "issued_by" uuid,
  "expires_at" timestamptz NOT NULL,
  "uses_left" int NOT NULL,
  "created_at" timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS "_aip_verification" (
  "id" uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  "name" text NOT NULL,
  "subject" uuid NOT NULL,
  "target" text NOT NULL,
  "code_hash" text NOT NULL,
  "expires_at" timestamptz NOT NULL,
  "attempts_left" int NOT NULL,
  "created_at" timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS "_aip_approval" (
  "id" uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  "name" text NOT NULL,
  "subject" uuid NOT NULL,
  "requested_by" uuid,
  "status" text NOT NULL DEFAULT 'PENDING' CHECK ("status" IN ('PENDING', 'APPROVED', 'REJECTED', 'CANCELLED', 'EXPIRED')),
  "required" int NOT NULL,
  "expires_at" timestamptz,
  "created_at" timestamptz NOT NULL DEFAULT now(),
  "decided_at" timestamptz
);
CREATE UNIQUE INDEX IF NOT EXISTS "_aip_approval_pending" ON "_aip_approval" ("name", "subject") WHERE "status" = 'PENDING';
CREATE TABLE IF NOT EXISTS "_aip_approval_vote" (
  "approval" uuid NOT NULL REFERENCES "_aip_approval" ("id") ON DELETE CASCADE,
  "voter" uuid NOT NULL,
  "decision" text NOT NULL CHECK ("decision" IN ('APPROVE', 'REJECT')),
  "comment" text,
  "at" timestamptz NOT NULL DEFAULT now(),
  CONSTRAINT "_aip_approval_vote_once" PRIMARY KEY ("approval", "voter")
);
CREATE TABLE IF NOT EXISTS "_aip_job" (
  "id" uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  "name" text NOT NULL,
  "params" jsonb NOT NULL,
  "requested_by" uuid NOT NULL,
  "status" text NOT NULL DEFAULT 'QUEUED' CHECK ("status" IN ('QUEUED', 'RUNNING', 'DONE', 'FAILED')),
  "items" jsonb,
  "total" int,
  "done" int NOT NULL DEFAULT 0,
  "file" text,
  "error" text,
  "attempts" int NOT NULL DEFAULT 0,
  "available_at" timestamptz NOT NULL DEFAULT now(),
  "heartbeat" timestamptz,
  "created_at" timestamptz NOT NULL DEFAULT now(),
  "started_at" timestamptz,
  "finished_at" timestamptz
);
CREATE UNIQUE INDEX IF NOT EXISTS "_aip_job_active" ON "_aip_job" ("name", "requested_by", "params") WHERE "status" IN ('QUEUED', 'RUNNING');
CREATE INDEX IF NOT EXISTS "_aip_job_queue" ON "_aip_job" ("available_at") WHERE "status" IN ('QUEUED', 'RUNNING');
CREATE TABLE IF NOT EXISTS "_aip_webhook_seen" (
  "webhook" text NOT NULL,
  "event_id" text NOT NULL,
  "received_at" timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY ("webhook", "event_id")
);
CREATE TABLE IF NOT EXISTS "_aip_rule_pending" (
  "rule" text NOT NULL,
  "id" uuid NOT NULL,
  PRIMARY KEY ("rule", "id")
);
CREATE TABLE IF NOT EXISTS "_aip_rule_state" (
  "rule" text NOT NULL,
  "id" uuid NOT NULL,
  PRIMARY KEY ("rule", "id")
);
CREATE TABLE IF NOT EXISTS "_aip_sequence" (
  "scope" text PRIMARY KEY,
  "n" bigint NOT NULL
);
CREATE TABLE IF NOT EXISTS "_aip_object" (
  "key" text PRIMARY KEY,
  "bucket" text NOT NULL,
  "content_type" text,
  "size" bigint NOT NULL,
  "state" text NOT NULL DEFAULT 'staged',
  "created_at" timestamptz NOT NULL DEFAULT now()
);
"#;

/// Only programs with a `consent` form get this table, so the others keep their schema.
const CONSENT_TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS "_aip_consent" (
  "id" uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  "name" text NOT NULL,
  "actor" uuid NOT NULL,
  "version" int NOT NULL,
  "given_at" timestamptz NOT NULL DEFAULT now(),
  "withdrawn_at" timestamptz
);
CREATE UNIQUE INDEX IF NOT EXISTS "_aip_consent_active" ON "_aip_consent" ("name", "actor", "version") WHERE "withdrawn_at" IS NULL;
CREATE INDEX IF NOT EXISTS "_aip_consent_actor" ON "_aip_consent" ("actor", "name", "given_at" DESC)
"#;

/// Only programs with an `impersonate` form get this table and the audit columns that name the operator.
const IMPERSONATION_TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS "_aip_impersonation" (
  "id" uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  "operator" uuid NOT NULL,
  "target" uuid NOT NULL,
  "reason" text NOT NULL,
  "started_at" timestamptz NOT NULL DEFAULT now(),
  "expires_at" timestamptz NOT NULL,
  "ended_at" timestamptz
);
CREATE INDEX IF NOT EXISTS "_aip_impersonation_operator" ON "_aip_impersonation" ("operator") WHERE "ended_at" IS NULL;
ALTER TABLE "_aip_audit" ADD COLUMN IF NOT EXISTS "impersonated_by" uuid;
ALTER TABLE "_aip_audit" ADD COLUMN IF NOT EXISTS "impersonation" uuid
"#;

/// Only programs with an `outbound webhooks` form get these tables: the queue of deliveries and the health of each endpoint.
const OUTBOUND_TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS "_aip_outbound_delivery" (
  "id" uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  "form" text NOT NULL,
  "endpoint" uuid NOT NULL,
  "event" text NOT NULL,
  "event_id" text NOT NULL,
  "body" jsonb NOT NULL,
  "status" text NOT NULL DEFAULT 'PENDING' CHECK ("status" IN ('PENDING', 'DELIVERED', 'FAILED', 'CANCELLED')),
  "attempts" int NOT NULL DEFAULT 0,
  "next_attempt_at" timestamptz NOT NULL DEFAULT now(),
  "deadline" timestamptz NOT NULL,
  "last_status" int,
  "last_error" text,
  "created_at" timestamptz NOT NULL DEFAULT now(),
  "delivered_at" timestamptz,
  UNIQUE ("form", "endpoint", "event_id")
);
CREATE INDEX IF NOT EXISTS "_aip_outbound_delivery_due" ON "_aip_outbound_delivery" ("next_attempt_at") WHERE "status" = 'PENDING';
CREATE TABLE IF NOT EXISTS "_aip_outbound_endpoint" (
  "form" text NOT NULL,
  "endpoint" uuid NOT NULL,
  "failing_since" timestamptz,
  "disabled_at" timestamptz,
  "disabled_reason" text,
  PRIMARY KEY ("form", "endpoint")
)
"#;

pub fn generate(core: &Program, s: &Schema) -> Ddl {
    let mut out = Ddl { statements: Vec::new(), codes: BTreeMap::new() };
    // the explicit checks in approval plans fire first; these only catch concurrent requests
    out.codes
        .insert("_aip_approval_pending".into(), ErrorSpec { code: codes::CONFLICT_UNIQUE.into(), reason: Some("APPROVAL_ALREADY_PENDING".into()) });
    out.codes.insert("_aip_approval_vote_once".into(), ErrorSpec { code: codes::CONFLICT_UNIQUE.into(), reason: Some("ALREADY_VOTED".into()) });
    out.statements.push("CREATE EXTENSION IF NOT EXISTS btree_gist".into());
    out.statements.push("CREATE EXTENSION IF NOT EXISTS pgcrypto".into());
    let mut later: Vec<String> = Vec::new();
    let absolute = crate::writes::absolute_fields(core);
    let deltas = crate::writes::delta_fields(core);
    let mut ordered: Vec<&crate::schema::Table> = s.tables.values().collect();
    ordered.sort_by_key(|t| t.rank);
    for t in &ordered {
        let info = &core.entities[&t.entity];
        let mut cols: Vec<String> = Vec::new();
        let mut checks: Vec<String> = Vec::new();
        for f in &info.fields {
            let Some(col) = t.col(&f.name) else { continue };
            let not_null = if f.optional { "" } else { " NOT NULL" };
            match col {
                Col::Scalar { col, ty } => {
                    let default = match f.name.as_str() {
                        "id" => " PRIMARY KEY DEFAULT gen_random_uuid()".to_string(),
                        "createdAt" | "updatedAt" => " DEFAULT now()".to_string(),
                        "version" => " DEFAULT 1".to_string(),
                        _ => match f.default.as_ref() {
                            Some(d) => default_sql(d).map(|v| format!(" DEFAULT {v}")).unwrap_or_default(),
                            None => String::new(),
                        },
                    };
                    let nn = if f.name == "id" { "" } else { not_null };
                    cols.push(format!("{} {}{nn}{default}", q(col), sql_type(ty)));
                    if let Ty::Enum(en) = ty {
                        let vals: Vec<String> = core.enums[en].values.iter().map(|v| lit(v)).collect();
                        checks.push(format!("CONSTRAINT {} CHECK ({} IN ({}))", q(&format!("{}__{col}_enum", t.table)), q(col), vals.join(", ")));
                    }
                }
                Col::Counter { col } => cols.push(format!("{} bigint NOT NULL DEFAULT 0", q(col))),
                Col::Ref { col, target } => {
                    let on_delete = match &f.kind {
                        ir::FieldKind::Ref { on_delete, .. } => on_delete.as_ref(),
                        _ => None,
                    };
                    let policy = on_delete
                        .map(|p| match p {
                            RefPolicy::Cascade => " ON DELETE CASCADE",
                            RefPolicy::Restrict => " ON DELETE RESTRICT",
                            RefPolicy::SetNull => " ON DELETE SET NULL",
                            _ => "",
                        })
                        .unwrap_or("");
                    // implicit createdBy/updatedBy must not block deleting the actor
                    let policy = if matches!(f.name.as_str(), "createdBy" | "updatedBy") { " ON DELETE SET NULL" } else { policy };
                    cols.push(format!("{} uuid{not_null}", q(col)));
                    let tt = s.table(target).table.clone();
                    later.push(format!(
                        "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {}(\"id\"){policy} DEFERRABLE INITIALLY IMMEDIATE",
                        q(&t.table),
                        q(&format!("{}__{col}_fk", t.table)),
                        q(col),
                        q(&tt)
                    ));
                    later.push(format!("CREATE INDEX IF NOT EXISTS {} ON {} ({})", q(&format!("{}__{col}_idx", t.table)), q(&t.table), q(col)));
                }
                Col::Union { type_col, id_col, targets } => {
                    cols.push(format!("{} text{not_null}", q(type_col)));
                    cols.push(format!("{} uuid{not_null}", q(id_col)));
                    let vals: Vec<String> = targets.iter().map(|x| lit(x)).collect();
                    checks.push(format!(
                        "CONSTRAINT {} CHECK ({} IN ({}))",
                        q(&format!("{}__{type_col}_enum", t.table)),
                        q(type_col),
                        vals.join(", ")
                    ));
                    later.push(format!(
                        "CREATE INDEX IF NOT EXISTS {} ON {} ({}, {})",
                        q(&format!("{}__{id_col}_idx", t.table)),
                        q(&t.table),
                        q(type_col),
                        q(id_col)
                    ));
                }
                Col::Snapshot { id_col, ver_col, target } => {
                    cols.push(format!("{} uuid{not_null}", q(id_col)));
                    cols.push(format!("{} bigint{not_null}", q(ver_col)));
                    let tt = s.table(target).table.clone();
                    later.push(format!(
                        "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}, {}) REFERENCES {}(\"id\", \"version\")",
                        q(&t.table),
                        q(&format!("{}__{id_col}_snap_fk", t.table)),
                        q(id_col),
                        q(ver_col),
                        q(&format!("{tt}_history"))
                    ));
                }
                Col::Inverse { .. } => {}
            }
        }
        let body = cols.into_iter().chain(checks).collect::<Vec<_>>().join(",\n  ");
        out.statements.push(format!("CREATE TABLE {} (\n  {body}\n)", q(&t.table)));
        if t.published {
            published_tables(t, &mut out.statements, &mut later);
        }
        if info.traits.track_updated {
            later.push(trigger(&t.table, "touch", "BEFORE UPDATE", "NEW.\"updated_at\" := now(); RETURN NEW;"));
        }
        if t.history || info.traits.versioned {
            // counters and the touch timestamp change without a user edit; they must not make an edit stale
            let mut ignored: Vec<String> = t
                .cols
                .iter()
                .filter_map(|(field, c)| match c {
                    Col::Counter { col } => Some(lit(col)),
                    // only ever moved by += / -= anywhere in the program
                    Col::Scalar { col, .. }
                        if !absolute.contains(&(t.entity.clone(), field.clone())) && deltas.contains(&(t.entity.clone(), field.clone())) =>
                    {
                        Some(lit(col))
                    }
                    _ => None,
                })
                .collect();
            ignored.extend(["'updated_at'".to_string(), "'version'".to_string()]);
            // derived from the row, never a user edit
            ignored.extend(s.searches.values().filter(|x| x.entity == t.entity).map(|x| lit(&x.column)));
            let strip = |row: &str| format!("(to_jsonb({row}) - ARRAY[{}]::text[])", ignored.join(", "));
            later.push(trigger(
                &t.table,
                "version",
                "BEFORE UPDATE",
                &format!("IF {} IS DISTINCT FROM {} THEN NEW.\"version\" := OLD.\"version\" + 1; END IF; RETURN NEW;", strip("NEW"), strip("OLD")),
            ));
        }
        if t.history {
            // the search document is derived from the row; a version snapshot does not carry it
            let cols: Vec<String> = s.searches.values().filter(|x| x.entity == t.entity).map(|x| lit(&x.column)).collect();
            let derived = if cols.is_empty() { String::new() } else { format!(" - ARRAY[{}]::text[]", cols.join(", ")) };
            out.statements.push(format!(
                "CREATE TABLE {} (\n  \"id\" uuid NOT NULL,\n  \"version\" bigint NOT NULL,\n  \"data\" jsonb NOT NULL,\n  \"at\" timestamptz NOT NULL DEFAULT now(),\n  PRIMARY KEY (\"id\", \"version\")\n)",
                q(&format!("{}_history", t.table))
            ));
            later.push(trigger(
                &t.table,
                "history",
                "AFTER INSERT OR UPDATE",
                &format!(
                    "INSERT INTO {} (\"id\", \"version\", \"data\") VALUES (NEW.\"id\", NEW.\"version\", to_jsonb(NEW){derived}) ON CONFLICT DO NOTHING; RETURN NULL;",
                    q(&format!("{}_history", t.table))
                ),
            ));
        }
        if core.forms.iter().any(|f| matches!(f, ir::Form::OutboundWebhooks(o) if o.entity == t.entity))
            && let Some(Col::Scalar { col, .. }) = t.col("url")
        {
            // a first, cheap refusal at registration; where the URL really points is checked when it is called
            let name = format!("{}__url_scheme", t.table);
            later.push(format!(
                "ALTER TABLE {} ADD CONSTRAINT {} CHECK ({} IS NULL OR {} ~* '^https?://[^/?#@[:space:]]+([/?#][^[:space:]]*)?$')",
                q(&t.table),
                q(&name),
                q(col),
                q(col)
            ));
            out.codes.insert(name, ErrorSpec { code: codes::INPUT_INVALID.into(), reason: Some("WEBHOOK_URL_INVALID".into()) });
        }
        for sc in s.searches.values().filter(|x| x.entity == t.entity) {
            later.push(format!("ALTER TABLE {} ADD COLUMN {} tsvector GENERATED ALWAYS AS ({}) STORED", q(&t.table), q(&sc.column), sc.document));
            later.push(format!("CREATE INDEX {} ON {} USING GIN ({})", q(&format!("{}__{}_idx", t.table, sc.column)), q(&t.table), q(&sc.column)));
        }
        generated_values(s, t, &info.fields, &mut later, &mut out.codes);
        constraints(core, s, t, &t.entity, &info.constraints, &mut later, &mut out.codes);
    }
    crate::tenant::ddl(core, s, &mut later, &mut out.codes);
    out.statements.extend(later);
    out.statements.extend(INTERNAL_TABLES.split(";\n").map(|x| x.trim().to_string()).filter(|x| !x.is_empty()));
    let optional = |text: &str| -> Vec<String> { text.split(";\n").map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect() };
    if core.forms.iter().any(|f| matches!(f, ir::Form::Consent(_))) {
        out.statements.extend(optional(CONSENT_TABLES));
    }
    if ir::form_intents::impersonation(core).is_some() {
        out.statements.extend(optional(IMPERSONATION_TABLES));
    }
    if core.forms.iter().any(|f| matches!(f, ir::Form::OutboundWebhooks(_))) {
        out.statements.extend(optional(OUTBOUND_TABLES));
    }
    out
}

/// The published version of a `publishable` entity: the same columns, no constraints (it is a snapshot, the working table
/// enforces them), one row per published entity. A trigger drops it when the working row goes, so deleting unpublishes.
fn published_tables(t: &crate::schema::Table, statements: &mut Vec<String>, later: &mut Vec<String>) {
    let pt = crate::schema::published_table(&t.table);
    statements.push(format!(
        "CREATE TABLE {} (LIKE {}, \"published_at\" timestamptz NOT NULL DEFAULT now(), PRIMARY KEY (\"id\"))",
        q(&pt),
        q(&t.table)
    ));
    for col in t.cols.values() {
        if let Col::Ref { col, .. } = col {
            later.push(format!("CREATE INDEX IF NOT EXISTS {} ON {} ({})", q(&format!("{pt}__{col}_idx")), q(&pt), q(col)));
        }
    }
    let gone = format!("DELETE FROM {} WHERE \"id\" = OLD.\"id\";", q(&pt));
    let body = if t.soft_delete {
        format!("IF TG_OP = 'UPDATE' THEN IF NEW.\"deleted_at\" IS NOT NULL THEN {gone} END IF; ELSE {gone} END IF; RETURN NULL;")
    } else {
        format!("{gone} RETURN NULL;")
    };
    let when = if t.soft_delete { "AFTER DELETE OR UPDATE OF \"deleted_at\"" } else { "AFTER DELETE" };
    later.push(trigger(&t.table, "unpublish", when, &body));
}

fn default_sql(e: &Expr) -> Option<String> {
    match &e.node {
        Node::Lit { lit: Literal::Int(n) } => Some(n.to_string()),
        Node::Lit { lit: Literal::Decimal(d) } => Some(d.clone()),
        Node::Lit { lit: Literal::Text(s) } => Some(lit(s)),
        Node::Lit { lit: Literal::Bool(b) } => Some(if *b { "TRUE".into() } else { "FALSE".into() }),
        Node::EnumValue { value, .. } => Some(lit(value)),
        Node::Now => Some("now()".into()),
        _ => None,
    }
}

pub fn trigger(table: &str, name: &str, when: &str, body: &str) -> String {
    let f = format!("{table}__{name}_fn");
    format!(
        "CREATE OR REPLACE FUNCTION {}() RETURNS trigger LANGUAGE plpgsql AS $aip$ BEGIN {body} END $aip$;\nCREATE TRIGGER {} {when} ON {} FOR EACH ROW EXECUTE FUNCTION {}()",
        q(&f),
        q(&format!("{table}__{name}")),
        q(table),
        q(&f)
    )
}

fn raise(code: &str) -> String {
    // SQLSTATE P0001 with the constraint name as message; the runtime maps it
    format!("RAISE EXCEPTION USING ERRCODE = 'P0001', MESSAGE = 'aip:{code}';")
}

fn constraints(
    core: &Program,
    s: &Schema,
    t: &crate::schema::Table,
    entity: &str,
    list: &[Constraint],
    later: &mut Vec<String>,
    codes: &mut BTreeMap<String, ErrorSpec>,
) {
    let table = &t.table;
    let colname = |field: &str| -> String {
        match t.col(field) {
            Some(Col::Scalar { col, .. }) | Some(Col::Counter { col }) | Some(Col::Ref { col, .. }) => col.clone(),
            Some(Col::Union { id_col, .. }) => id_col.clone(),
            _ => snake(field),
        }
    };
    let row_pred = |cond: &Expr, alias: &str| -> String {
        let mut c = Compiler::new(core, s, "");
        c.push_row(Val::Row { entity: t.entity.clone(), alias: alias.to_string() });

        c.pred(cond)
    };
    for con in list {
        let i = con.ordinal();
        match con {
            Constraint::Unique { fields: cols, filter, code, .. } => {
                let name = format!("{table}__u{i}");
                let mut parts: Vec<String> = cols.iter().map(|c| q(&colname(c))).collect();
                if let Some(Col::Union { type_col, .. }) = cols.iter().find_map(|c| t.col(c).filter(|x| matches!(x, Col::Union { .. }))) {
                    parts.insert(0, q(type_col));
                }
                let mut wher = filter.as_ref().map(|f| row_pred(f, table)).map(|w| strip_alias(&w, table)).unwrap_or_default();
                if t.soft_delete {
                    wher = if wher.is_empty() { "\"deleted_at\" IS NULL".into() } else { format!("({wher}) AND \"deleted_at\" IS NULL") };
                }
                let w = if wher.is_empty() { String::new() } else { format!(" WHERE {wher}") };
                later.push(format!("CREATE UNIQUE INDEX {} ON {} ({}){w}", q(&name), q(table), parts.join(", ")));
                codes.insert(name, ErrorSpec { code: codes::CONFLICT_UNIQUE.into(), reason: code.clone() });
            }
            Constraint::Cardinality { kind, filter, per, .. } => {
                let per_col = colname(per);
                let cond = strip_alias(&row_pred(filter, table), table);
                let reason = format!(
                    "{}_{}",
                    snake(entity).to_uppercase(),
                    match kind {
                        ir::Cardinality::ExactlyOne => "EXACTLY_ONE",
                        ir::Cardinality::AtMostOne => "AT_MOST_ONE",
                        ir::Cardinality::AtLeastOne => "AT_LEAST_ONE",
                    }
                );
                if matches!(kind, ir::Cardinality::ExactlyOne | ir::Cardinality::AtMostOne) {
                    let name = format!("{table}__one{i}");
                    later.push(format!("CREATE UNIQUE INDEX {} ON {} ({}) WHERE {cond}", q(&name), q(table), q(&per_col)));
                    codes.insert(name, ErrorSpec { code: codes::INVARIANT_VIOLATED.into(), reason: Some(reason.clone()) });
                }
                if matches!(kind, ir::Cardinality::ExactlyOne | ir::Cardinality::AtLeastOne) {
                    // checked at commit, only for groups this transaction touched
                    let name = format!("{table}__least{i}");
                    let nested = row_pred(filter, "x");
                    let body = format!(
                        "IF TG_OP <> 'INSERT' AND OLD.{pc} IS NOT NULL AND EXISTS (SELECT 1 FROM {tq} p WHERE p.\"id\" = OLD.{pc}) AND NOT EXISTS (SELECT 1 FROM {t} x WHERE x.{pc} = OLD.{pc} AND {nested}) THEN {r} END IF; \
                         IF TG_OP <> 'DELETE' AND NOT EXISTS (SELECT 1 FROM {t} x WHERE x.{pc} = NEW.{pc} AND {nested}) THEN {r} END IF; RETURN NULL;",
                        pc = q(&per_col),
                        t = q(table),
                        tq = q(&parent_table(s, t, per)),
                        r = raise(&name)
                    );
                    let f = format!("{table}__least{i}_fn");
                    later.push(format!(
                        "CREATE OR REPLACE FUNCTION {}() RETURNS trigger LANGUAGE plpgsql AS $aip$ BEGIN {body} END $aip$;\nCREATE CONSTRAINT TRIGGER {} AFTER INSERT OR UPDATE OR DELETE ON {} DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION {}()",
                        q(&f),
                        q(&name),
                        q(table),
                        q(&f)
                    ));
                    codes.insert(name, ErrorSpec { code: codes::INVARIANT_VIOLATED.into(), reason: Some(reason) });
                }
            }
            Constraint::NoOverlap { range, per, code, .. } => {
                let name = format!("{table}__nooverlap{i}");
                let sd = if t.soft_delete { " WHERE (\"deleted_at\" IS NULL)" } else { "" };
                later.push(format!(
                    "ALTER TABLE {} ADD CONSTRAINT {} EXCLUDE USING gist ({} WITH =, {} WITH &&){sd}",
                    q(table),
                    q(&name),
                    q(&colname(per)),
                    q(&colname(range))
                ));
                codes.insert(name, ErrorSpec { code: codes::CONFLICT_OVERLAP.into(), reason: code.clone() });
            }
            Constraint::Invariant { name: iname, cond, .. } => {
                let name = format!("{table}__{}", snake(iname));
                let local = is_row_local(cond, t);
                if local {
                    let sql = strip_alias(&row_pred(cond, table), table);
                    later.push(format!("ALTER TABLE {} ADD CONSTRAINT {} CHECK ({sql})", q(table), q(&name)));
                } else {
                    let sql = row_pred(cond, "NEW");
                    later.push(trigger(
                        table,
                        &format!("inv_{}", snake(iname)),
                        "BEFORE INSERT OR UPDATE",
                        &format!("IF NOT {sql} THEN {} END IF; RETURN NEW;", raise(&name)),
                    ));
                }
                codes.insert(name, ErrorSpec { code: codes::INVARIANT_VIOLATED.into(), reason: Some(iname.to_uppercase()) });
            }
            Constraint::Capacity { count, limit, code, .. } => {
                let name = format!("{table}__cap{i}");
                // serialize inserts per group by locking the parent row the filter pins
                let parent_lock = capacity_parent(count, t)
                    .map(|(col, ptable)| format!("PERFORM 1 FROM {} WHERE \"id\" = NEW.{} FOR UPDATE;", q(&ptable), q(&col)))
                    .unwrap_or_default();
                let mut c = Compiler::new(core, s, "");
                c.push_row(Val::Row { entity: t.entity.clone(), alias: "NEW".into() });
                let parts = c.begin_set(count);
                c.end_set();
                let lim = c.sql(limit);
                let mut conds = parts.conds.clone();
                conds.push(format!("{}.\"id\" <> NEW.\"id\"", parts.alias));
                // does the new row itself count? (same filter with the alias bound to NEW)
                let mut c2 = Compiler::new(core, s, "");
                c2.push_row(Val::Row { entity: t.entity.clone(), alias: "NEW".into() });
                c2.push();
                if let Some(al) = &count.alias {
                    c2.bind(al, Val::Row { entity: t.entity.clone(), alias: "NEW".into() });
                }
                let counts_self = count.filter.as_ref().map(|f| c2.pred(f)).unwrap_or_else(|| "TRUE".into());
                let body = format!(
                    "{parent_lock} IF {counts_self} AND (SELECT count(*) FROM {}{}) + 1 > ({lim}) THEN {} END IF; RETURN NEW;",
                    parts.from,
                    Compiler::where_sql(&conds),
                    raise(&name)
                );
                later.push(trigger(table, &format!("cap{i}"), "BEFORE INSERT OR UPDATE", &body));
                codes.insert(name, ErrorSpec { code: codes::CONFLICT_CAPACITY.into(), reason: Some(code.clone()) });
            }
        }
    }
}

fn parent_table(s: &Schema, t: &crate::schema::Table, field: &str) -> String {
    match t.col(field) {
        Some(Col::Ref { target, .. }) => s.table(target).table.clone(),
        _ => t.table.clone(),
    }
}

/// `capacity count(X x where x.parent = parent and ...)`: the reference both sides share.
fn capacity_parent(count: &SetExpr, t: &crate::schema::Table) -> Option<(String, String)> {
    let f = count.filter.as_ref()?;
    fn walk(e: &Expr, t: &crate::schema::Table) -> Option<String> {
        match &e.node {
            Node::Binary { op: BinOp::And, l, r } => walk(l, t).or_else(|| walk(r, t)),
            Node::Binary { op: BinOp::Eq, l, r } => {
                for side in [l, r] {
                    if let Node::RowField { field, .. } = &side.node
                        && matches!(t.col(field), Some(Col::Ref { .. }))
                    {
                        return Some(field.clone());
                    }
                }
                None
            }
            _ => None,
        }
    }
    let field = walk(f, t)?;
    match t.col(&field) {
        Some(Col::Ref { col, target }) => Some((col.clone(), crate::names::snake(target))),
        _ => None,
    }
}

/// True if the condition only reads this row's own columns.
fn is_row_local(e: &Expr, t: &crate::schema::Table) -> bool {
    match &e.node {
        Node::RowField { field, .. } => !matches!(t.col(field), Some(Col::Inverse { .. })),
        Node::Field { .. } | Node::Call(_) | Node::Exists { .. } | Node::The { .. } | Node::Agg { .. } | Node::Quant { .. } => false,
        Node::Binary { l, r, .. } => is_row_local(l, t) && is_row_local(r, t),
        Node::Unary { arg, .. } => is_row_local(arg, t),
        Node::InList { value, list } => is_row_local(value, t) && list.iter().all(|i| is_row_local(i, t)),
        Node::If { cond, then, otherwise } => is_row_local(cond, t) && is_row_local(then, t) && is_row_local(otherwise, t),
        Node::Actor | Node::SelfRow => false,
        _ => true,
    }
}

/// Indexes and CHECKs cannot use a table alias; the compiler prefixes columns
/// with the table name used as alias, which we remove here.
fn strip_alias(sql: &str, table: &str) -> String {
    let needle = format!("{table}.");
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    let bytes = sql.as_bytes();
    while i < sql.len() {
        let prev_ident = i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b'"');
        if !prev_ident && sql[i..].starts_with(&needle) {
            i += needle.len();
            continue;
        }
        let ch = sql[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// `position within`, `sequence per ... format`, `slug from ... unique`: values the
/// runtime generates on insert, inside the same transaction.
fn generated_values(s: &Schema, t: &crate::schema::Table, fields: &[ir::Field], later: &mut Vec<String>, codes: &mut BTreeMap<String, ErrorSpec>) {
    let scope_sql = |path: &[String]| -> String {
        // first segment is a field of this row; a second one reads through the reference
        let Some(first) = path.first() else { return "''".to_string() };
        match (t.col(first), path.get(1)) {
            (Some(Col::Ref { col, target }), Some(next)) => {
                let tt = s.table(target).clone();
                let ncol = match tt.col(next) {
                    Some(Col::Scalar { col, .. }) | Some(Col::Ref { col, .. }) => col.clone(),
                    _ => snake(next),
                };
                format!("(SELECT p.{} FROM {} p WHERE p.\"id\" = NEW.{})::text", q(&ncol), q(&tt.table), q(col))
            }
            (Some(Col::Ref { col, .. }), None) | (Some(Col::Scalar { col, .. }), None) => format!("NEW.{}::text", q(col)),
            _ => "''".to_string(),
        }
    };
    for f in fields {
        let Some(generated) = &f.generated else { continue };
        let Some(Col::Scalar { col, .. }) = t.col(&f.name).cloned() else { continue };
        {
            match generated {
                Generated::Position { .. } => {
                    // append order; reordering rewrites the key between neighbours
                    later.push(trigger(
                        &t.table,
                        &format!("pos_{col}"),
                        "BEFORE INSERT",
                        &format!("IF NEW.{c} IS NULL THEN NEW.{c} := to_char(clock_timestamp(), 'YYYYMMDDHH24MISSUS') || substr(md5(random()::text), 1, 4); END IF; RETURN NEW;", c = q(&col)),
                    ));
                }
                Generated::Sequence { per, format } => {
                    let scope = scope_sql(per);
                    let rendered = render_format(format, &scope_sql);
                    let key = format!("{} || ':' || coalesce({scope}, '')", lit(&(t.table.clone() + "." + &col)));
                    later.push(trigger(
                        &t.table,
                        &format!("seq_{col}"),
                        "BEFORE INSERT",
                        &format!(
                            "DECLARE n bigint; BEGIN IF NEW.{c} IS NULL THEN INSERT INTO \"_aip_sequence\" (\"scope\", \"n\") VALUES ({key}, 1) ON CONFLICT (\"scope\") DO UPDATE SET \"n\" = \"_aip_sequence\".\"n\" + 1 RETURNING \"n\" INTO n; NEW.{c} := {rendered}; END IF; RETURN NEW; END",
                            c = q(&col),
                        )
                        .replacen("BEGIN ", "", 0),
                    ));
                    // trigger() wraps the body in BEGIN..END; a DECLARE block needs its own wrapper
                    if let Some(last) = later.last_mut() {
                        *last = last.replace("$aip$ BEGIN DECLARE", "$aip$ DECLARE").replace("END END $aip$", "END $aip$");
                    }
                    let name = format!("{}__{}_seq_key", t.table, col);
                    later.push(format!("CREATE UNIQUE INDEX {} ON {} ({})", q(&name), q(&t.table), q(&col)));
                    codes.insert(name, ErrorSpec { code: codes::CONFLICT_UNIQUE.into(), reason: Some(format!("{}_TAKEN", col.to_uppercase())) });
                }
                Generated::Slug { from, per } => {
                    let from_col = match t.col(from) {
                        Some(Col::Scalar { col, .. }) => col.clone(),
                        _ => snake(from),
                    };
                    let (scope_cond, scope_cols) = match per.first() {
                        Some(p) => match t.col(p) {
                            Some(Col::Ref { col, .. }) | Some(Col::Scalar { col, .. }) => {
                                (format!(" AND x.{c} IS NOT DISTINCT FROM NEW.{c}", c = q(col)), format!(", {}", q(col)))
                            }
                            _ => (String::new(), String::new()),
                        },
                        None => (String::new(), String::new()),
                    };
                    later.push(format!(
                        "CREATE OR REPLACE FUNCTION {f}() RETURNS trigger LANGUAGE plpgsql AS $aip$ DECLARE base text; cand text; i int := 1; BEGIN \
                         IF NEW.{c} IS NOT NULL THEN RETURN NEW; END IF; \
                         base := trim(both '-' from regexp_replace(lower(NEW.{fc}), '[^a-z0-9가-힣]+', '-', 'g')); IF base = '' THEN base := 'item'; END IF; cand := base; \
                         WHILE EXISTS (SELECT 1 FROM {t} x WHERE x.{c} = cand{scope_cond}) LOOP i := i + 1; cand := base || '-' || i; END LOOP; \
                         NEW.{c} := cand; RETURN NEW; END $aip$;\nCREATE TRIGGER {tg} BEFORE INSERT ON {t} FOR EACH ROW EXECUTE FUNCTION {f}()",
                        f = q(&format!("{}__slug_{col}_fn", t.table)),
                        tg = q(&format!("{}__slug_{col}", t.table)),
                        t = q(&t.table),
                        c = q(&col),
                        fc = q(&from_col),
                    ));
                    let name = format!("{}__{}_slug_key", t.table, col);
                    later.push(format!("CREATE UNIQUE INDEX {} ON {} ({}{scope_cols})", q(&name), q(&t.table), q(&col)));
                    codes.insert(name, ErrorSpec { code: codes::CONFLICT_UNIQUE.into(), reason: Some(format!("{}_TAKEN", col.to_uppercase())) });
                }
                _ => {}
            }
        }
    }
}

/// `"{board.code}-{n:06}"` -> SQL string expression; `n` is the plpgsql counter.
fn render_format(fmt: &str, scope_sql: &dyn Fn(&[String]) -> String) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut rest = fmt;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            parts.push(lit(&rest[..open]));
        }
        let Some(close) = rest[open..].find('}') else { break };
        let inner = &rest[open + 1..open + close];
        if let Some(width) = inner.strip_prefix("n:0").and_then(|w| w.parse::<usize>().ok()) {
            parts.push(format!("lpad(n::text, {width}, '0')"));
        } else if inner == "n" {
            parts.push("n::text".into());
        } else {
            let path: Vec<String> = inner.split('.').map(String::from).collect();
            parts.push(format!("coalesce({}, '')", scope_sql(&path)));
        }
        rest = &rest[open + close + 1..];
    }
    if !rest.is_empty() {
        parts.push(lit(rest));
    }
    if parts.is_empty() { "''".into() } else { parts.join(" || ") }
}
