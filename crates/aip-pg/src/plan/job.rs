//! `job X(params) { ... }` lowers to a start command `X`, a status query
//! `XStatus(job)`, and a `Job` the worker runs outside the request:
//! the item set is snapshotted at start, walked in batches (each batch commits
//! its progress), an optional file is produced into the object store, and the
//! requester is notified at the end.

use super::{Planner, sql};
use crate::names::{lit, q};
use crate::schema::Col;
use crate::sqlexpr::{ACTOR, Compiler, SetElem, Val, marker, typed_marker};
use crate::ty::Ty;
use aip_ir as ir;
use aip_ir::codes;
use aip_ir::form_intents;
use aip_plan::*;
use serde_json::json;

const BATCH: u32 = 100;

impl Planner<'_> {
    pub fn job(&mut self, j: &ir::Job) -> (CommandPlan, QueryPlan, Job) {
        let i = form_intents::job(self.core, j);
        let start = self.job_start(j, &i.start);
        let status = self.job_status(j, &i.status);
        (start, status, self.job_plan(j))
    }

    fn job_start(&mut self, j: &ir::Job, intent: &form_intents::FormIntent) -> CommandPlan {
        self.begin();
        let mut c = self.compiler();
        c.push();
        let params = self.params(&mut c, &j.params);
        // a job that reads out or removes a person's data runs later as that person, with no session behind it
        let mut steps: Vec<Step> = match &j.body {
            Some(b) if form_intents::touches_personal_data(b) => self.not_impersonating().into_iter().collect(),
            _ => Vec::new(),
        };
        steps.extend(self.loads(&mut c, &j.params, &Default::default(), true));
        // starting only queues the job; the worker pins each item to the parameters' tenant
        steps.extend(self.tenant_steps(&mut c, &j.params, false, false));
        let actor = typed_marker(ACTOR, &Ty::Uuid);
        steps.push(Step::Check { kind: CheckKind::Allow, sql: sql(format!("SELECT {actor} IS NOT NULL")), code: None });
        steps.push(self.allow_step(&mut c, &j.allow));
        steps.extend(self.consent_steps(&j.name));
        let stored = if j.params.is_empty() {
            "'{}'::jsonb".to_string()
        } else {
            let pairs: Vec<String> = j.params.iter().map(|p| format!("{}, ({}::text)", lit(&p.name), marker(&p.name))).collect();
            format!("jsonb_build_object({})", pairs.join(", "))
        };
        let name = lit(&j.name);
        // an identical job still queued or running is reused instead of started twice
        steps.push(Step::Exec {
            sql: sql(format!(
                "INSERT INTO \"_aip_job\" (\"name\", \"params\", \"requested_by\") VALUES ({name}, {stored}, {actor}) \
                 ON CONFLICT (\"name\", \"requested_by\", \"params\") WHERE \"status\" IN ('QUEUED', 'RUNNING') DO NOTHING"
            )),
            bind: None,
            label: format!("queue {}", j.name),
        });
        let returns = Some(sql(format!(
            "SELECT jsonb_build_object('job', \"id\", 'status', \"status\") FROM \"_aip_job\" WHERE \"name\" = {name} AND \"requested_by\" = {actor} AND \"params\" = {stored} ORDER BY \"created_at\" DESC LIMIT 1"
        )));
        c.pop();
        self.absorb(&mut c);
        let mut plan = self.finish(intent, params, steps, returns);
        plan.output = json!({"kind": "object", "fields": {"job": {"kind": "uuid"}, "status": {"kind": "enum", "name": "JobStatus"}}});
        plan
    }

    fn job_status(&mut self, j: &ir::Job, intent: &form_intents::FormIntent) -> QueryPlan {
        self.begin();
        let mut c = self.compiler();
        let params = self.params(&mut c, &intent.params);
        let su = c.superuser_sql().map(|s| format!(" OR {s}")).unwrap_or_default();
        self.absorb(&mut c);
        let main = format!(
            "SELECT jsonb_build_object('status', j.\"status\", 'total', j.\"total\", 'done', j.\"done\", \
             'file', CASE WHEN j.\"file\" IS NULL THEN NULL ELSE '/aip/objects/' || j.\"file\" END, 'error', j.\"error\", \
             'createdAt', j.\"created_at\", 'startedAt', j.\"started_at\", 'finishedAt', j.\"finished_at\") \
             FROM \"_aip_job\" j WHERE j.\"id\" = {} AND j.\"name\" = {} AND (j.\"requested_by\" = {}{su})",
            typed_marker("job", &Ty::Uuid),
            lit(&j.name),
            typed_marker(ACTOR, &Ty::Uuid),
        );
        let spec = |t: &Ty| serde_json::to_value(crate::ty::ty_spec(t)).unwrap_or_default();
        let nullable = |t: &Ty| {
            let mut v = spec(t);
            if let Some(m) = v.as_object_mut() {
                m.insert("nullable".into(), json!(true));
            }
            v
        };
        QueryPlan {
            name: intent.name.clone(),
            params,
            internal: false,
            prelude: Vec::new(),
            variants: vec![QueryVariant { when: None, main: sql(main), key_count: 0 }],
            sort_param: None,
            single: true,
            page: None,
            cache_seconds: None,
            touches: Vec::new(),
            rate_limits: Vec::new(),
            errors: super::error_specs(&intent.facts),
            output: json!({"kind": "object", "fields": {
                "status": {"kind": "enum", "name": "JobStatus"},
                "total": nullable(&Ty::Int),
                "done": spec(&Ty::Int),
                "file": {"kind": "url", "nullable": true},
                "error": nullable(&Ty::Text),
                "createdAt": spec(&Ty::Time),
                "startedAt": nullable(&Ty::Time),
                "finishedAt": nullable(&Ty::Time),
            }}),
            decrypt: Vec::new(),
        }
    }

    fn job_plan(&mut self, j: &ir::Job) -> Job {
        self.begin();
        let mut c = self.compiler();
        c.push();
        self.params(&mut c, &j.params);
        // the progress set is filtered to the tenant of the parameters, and every item writes in it
        self.tenant_steps(&mut c, &j.params, false, false);
        let mut source = None;
        let mut item = None;
        let mut item_guard = None;
        let mut entity = None;
        if let Some(se) = &j.progress {
            // the file goes to the requester, so it only contains rows they can see
            c.apply_visibility = true;
            let parts = c.begin_set(se);
            c.end_set();
            c.apply_visibility = false;
            if let SetElem::Entity(e) = &parts.elem {
                source = Some(sql(format!(
                    "SELECT coalesce(jsonb_agg({a}.\"id\" ORDER BY {a}.\"id\"), '[]'::jsonb) FROM {}{}",
                    parts.from,
                    Compiler::where_sql(&parts.conds),
                    a = parts.alias
                )));
                let name = se.alias.clone().unwrap_or_else(|| "__item".into());
                let mut guard = parts.conds.clone();
                guard.push(format!("{}.\"id\" = {}", parts.alias, typed_marker(&name, &Ty::Uuid)));
                item_guard =
                    Some(sql(format!("SELECT {a}.\"id\" FROM {}{} FOR UPDATE OF {a}", parts.from, Compiler::where_sql(&guard), a = parts.alias)));
                c.bind(&name, Val::Id { entity: e.clone(), sql: typed_marker(&name, &Ty::Uuid) });
                if se.alias.is_none() {
                    c.push_row(Val::Id { entity: e.clone(), sql: typed_marker(&name, &Ty::Uuid) });
                }
                item = Some(name);
                entity = Some(e.clone());
            } else {
                self.fail("a job can only walk a set of rows");
            }
        }
        let mut steps = Vec::new();
        if let Some(b) = &j.body {
            // a batch commits many items at once; each starts from the tenant it is allowed to write
            steps.extend(self.context_pin(&c, false));
            self.stmts(&mut c, b, &mut steps);
        }
        // a job keeps its parameters as they came (`_aip_job.params`), so one that feeds an encrypted field would leave it readable there
        if let Some(p) = j.params.iter().find(|p| encrypts_from(&steps, &p.name)) {
            self.fail_code(
                codes::E320,
                format!("job {} writes parameter '{}' to an encrypted field, but a job stores its parameters in the clear", j.name, p.name),
            );
        }
        let export = match (&j.produce, &entity) {
            (Some(p), Some(e)) => {
                if p.format != "csv" {
                    self.fail(format!("unsupported export format '{}' (supported: csv)", p.format));
                }
                let (header, rows, decrypt) = self.export_rows(e);
                Some(JobExport {
                    format: p.format.clone(),
                    bucket: p.bucket.clone(),
                    header,
                    rows: sql(rows),
                    expires_seconds: p.expires_seconds.map(|s| s.max(0) as u64),
                    decrypt,
                })
            }
            (Some(_), None) => {
                self.fail("'produce' needs 'progress over' a set of rows");
                None
            }
            _ => None,
        };
        let mut finish = Vec::new();
        if let Some((who, via)) = &j.notify {
            let n = ir::Notify {
                to: ir::SetExpr { source: ir::SetSource::Expr { expr: who.clone() }, alias: None, filter: None },
                via: via.clone(),
                fields: Vec::new(),
                category: None,
                digest_seconds: None,
            };
            if let Step::Notify { recipients, template, category, digest_seconds, .. } = self.notify(&mut c, &n) {
                finish.push(Step::Notify {
                    recipients,
                    template,
                    payload: sql(format!(
                        "SELECT jsonb_build_object('job', ({}::text), 'name', {}, 'file', CASE WHEN ({f}::text) IS NULL THEN NULL ELSE '/aip/objects/' || ({f}::text) END)",
                        marker("__job"),
                        lit(&j.name),
                        f = marker("__file")
                    )),
                    category,
                    digest_seconds,
                });
            }
        }
        c.pop();
        self.absorb(&mut c);
        Job {
            name: j.name.clone(),
            params: j.params.iter().map(|p| p.name.clone()).collect(),
            source,
            item,
            item_guard,
            steps,
            export,
            finish,
            batch: BATCH,
        }
    }

    /// Columns of an exported row: id, then every stored field in declaration
    /// order except those guarded by field-level visibility.
    fn export_rows(&self, entity: &str) -> (Vec<String>, String, Vec<DecryptColumn>) {
        let t = self.s.table(entity);
        let info = self.core.entities.get(entity);
        let mut header = vec!["id".to_string()];
        let mut cols = vec!["t.\"id\"::text".to_string()];
        // encrypted cells leave the database as ciphertext; the runtime opens them with the id in the first cell
        let mut decrypt = Vec::new();
        for f in info.map(|i| i.fields.iter().collect::<Vec<_>>()).unwrap_or_default() {
            if f.name == "id" || f.visible_to.is_some() {
                continue;
            }
            match t.col(&f.name) {
                Some(Col::Scalar { col, ty: Ty::Time }) => {
                    header.push(f.name.clone());
                    cols.push(format!("to_char(t.{} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')", q(col)));
                }
                Some(Col::Scalar { col, ty: Ty::Object }) => {
                    header.push(f.name.clone());
                    cols.push(format!("('/aip/objects/' || t.{})", q(col)));
                }
                Some(Col::Scalar { col, .. }) | Some(Col::Ref { col, .. }) | Some(Col::Counter { col }) => {
                    if f.encrypted {
                        decrypt.push(DecryptColumn { column: cols.len(), field: format!("{entity}.{}", f.name) });
                    }
                    header.push(f.name.clone());
                    cols.push(format!("t.{}::text", q(col)));
                }
                _ => {}
            }
        }
        let rows = format!(
            "SELECT coalesce(jsonb_agg(jsonb_build_array({}) ORDER BY x.ord), '[]'::jsonb) FROM jsonb_array_elements_text(({}::text)::jsonb) WITH ORDINALITY x(id, ord) JOIN {} t ON t.\"id\" = x.id::uuid",
            cols.join(", "),
            marker("__ids"),
            q(&t.table)
        );
        (header, rows, decrypt)
    }
}

/// Whether any step (nested ones included) encrypts the value of `name`.
fn encrypts_from(steps: &[Step], name: &str) -> bool {
    steps.iter().any(|s| match s {
        Step::Encrypt { source, .. } => source == name,
        Step::When { steps, .. } | Step::EachPartial { steps, .. } | Step::ForEach { steps, .. } => encrypts_from(steps, name),
        _ => false,
    })
}
