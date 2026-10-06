//! PostgreSQL backend: compiles a Core IR program (`aip_ir::Program`) into DDL
//! and execution plans (`aip_plan::Program`). It reads nothing but the IR, so a
//! program from any frontend runs the same way.

pub mod ddl;
pub mod evolve;
pub mod names;
pub mod plan;
pub mod schema;
pub mod select;
pub mod shape;
pub mod sqlexpr;
pub mod tenant;
pub mod ty;
pub mod writes;

use aip_ir as ir;
use aip_ir::codes;
use aip_plan::*;
use std::collections::BTreeMap;
use ty::ty_spec;

pub struct Compiled {
    pub program: Program,
    pub diagnostics: Vec<Diagnostic>,
}

fn locate(map: &ir::SourceMap, path: &str) -> (u32, u32) {
    ir::locate(map, path)
}

fn diag(map: &ir::SourceMap, severity: Severity, code: &str, message: String, path: &str) -> Diagnostic {
    let (line, col) = locate(map, path);
    Diagnostic { severity, code: code.into(), message, path: path.into(), line, col }
}

pub fn compile(core: &ir::Program, map: &ir::SourceMap) -> Compiled {
    let s = schema::Schema::build(core);
    let published = s.published();
    let ddl = ddl::generate(core, &s);
    let mut planner = plan::Planner::new(core, &s).with_published(&published);
    let mut intents = BTreeMap::new();
    let mut handlers = Vec::new();
    let mut schedules = Vec::new();
    let mut jobs = Vec::new();
    let mut webhooks = Vec::new();
    let mut rules = Vec::new();
    let mut outbound = Vec::new();
    let mut rule_ddl: Vec<String> = Vec::new();
    let mut subscriptions = Vec::new();
    let mut migrations = Vec::new();
    // errors keyed by the declaration they came from, so they come out in source order
    let mut by_decl: Vec<((u32, u32), Vec<Diagnostic>)> = Vec::new();
    let mut unsupported: Vec<Diagnostic> = Vec::new();
    let mut take = |planner: &mut plan::Planner<'_>, path: &str, map: &ir::SourceMap| {
        let mut errs = std::mem::take(&mut planner.errors);
        for d in &mut errs {
            let (line, col) = locate(map, &d.path);
            d.line = line;
            d.col = col;
        }
        if !errs.is_empty() {
            by_decl.push((locate(map, path), errs));
        }
    };

    for (name, intent) in &core.intents {
        let path = format!("intents.{name}");
        planner.at = path.clone();
        match intent {
            ir::Intent::Command(c) => {
                let p = planner.command(name, c);
                intents.insert(p.name.clone(), Intent::Command(p));
            }
            ir::Intent::Query(qd) => {
                let p = planner.query(name, qd);
                intents.insert(p.name.clone(), Intent::Query(p));
            }
        }
        take(&mut planner, &path, map);
    }
    for (i, r) in core.reactions.iter().enumerate() {
        let path = format!("reactions[{i}]");
        planner.at = path.clone();
        match r {
            ir::Reaction::Event { cross_tenant, event, binding, when, body } => {
                let h = planner.handler(event, binding, when.as_ref(), body, handlers.len(), *cross_tenant);
                handlers.push(h);
            }
            ir::Reaction::Schedule(sd) => schedules.push(planner.schedule(sd)),
            ir::Reaction::Retain { entity, keep, after, anonymize, notify } => {
                schedules.push(planner.retain(entity, keep, after, *anonymize, notify.as_ref()));
            }
            ir::Reaction::Rule { cross_tenant, name, entity, alias, when, body } => {
                let (rule, ddl) = planner.rule(name, entity, alias, when, body, *cross_tenant);
                rule_ddl.extend(ddl);
                rules.push(rule);
            }
            ir::Reaction::Webhook(w) => webhooks.push(planner.webhook(w)),
            ir::Reaction::Consume { name, .. } => {
                // parsed and checked, but no executable plan yet: say so instead of ignoring it
                unsupported.push(diag(
                    map,
                    Severity::Error,
                    codes::E602,
                    format!("'{name}' is checked but cannot be executed yet by this runtime"),
                    &format!("{path}.name"),
                ));
            }
        }
        take(&mut planner, &path, map);
    }
    // the commands that move a publishable entity's rows between its working and published versions
    for (name, e) in &core.entities {
        if e.traits.publishable {
            planner.at = format!("entities.{name}.traits.publishable");
            for p in planner.publishable(name) {
                intents.insert(p.name.clone(), Intent::Command(p));
            }
            take(&mut planner, &format!("entities.{name}.traits.publishable"), map);
        }
    }
    for (i, f) in core.forms.iter().enumerate() {
        let path = format!("forms[{i}]");
        planner.at = path.clone();
        let mut unplanned = |name: &str| {
            unsupported.push(diag(
                map,
                Severity::Error,
                codes::E602,
                format!("'{name}' is checked but cannot be executed yet by this runtime"),
                &format!("{path}.name"),
            ));
        };
        match f {
            ir::Form::GrantLink(g) => {
                for p in planner.grant_link(g) {
                    intents.insert(p.name.clone(), Intent::Command(p));
                }
            }
            ir::Form::Verification(v) => {
                for p in planner.verification(v) {
                    intents.insert(p.name.clone(), Intent::Command(p));
                }
            }
            ir::Form::Approval(ap) => {
                let (commands, status) = planner.approval(ap);
                for p in commands {
                    intents.insert(p.name.clone(), Intent::Command(p));
                }
                intents.insert(status.name.clone(), Intent::Query(status));
            }
            ir::Form::Job(j) => {
                let (start, status, job) = planner.job(j);
                intents.insert(start.name.clone(), Intent::Command(start));
                intents.insert(status.name.clone(), Intent::Query(status));
                jobs.push(job);
            }
            ir::Form::Subscribe(x) => subscriptions.push(planner.subscription(x)),
            ir::Form::Projection(x) => unplanned(&x.name),
            // the index lives in the DDL and `from X.match(q)` queries read it; there is nothing to plan for the declaration itself
            ir::Form::Search(x) if ir::builtin::search_is_approximate(x.language.as_deref()) => {
                unsupported.push(diag(
                    map,
                    Severity::Warning,
                    codes::W603,
                    format!(
                        "search {} is declared 'language korean': it matches by word prefix, not by morphological analysis, so word endings are tolerated but changed stems and compounds are not found",
                        x.name
                    ),
                    &format!("{path}.name"),
                ));
            }
            ir::Form::Search(_) => {}
            ir::Form::OutboundWebhooks(x) => {
                let (hs, plan) = planner.outbound(x);
                handlers.extend(hs);
                outbound.extend(plan);
            }
            ir::Form::Consent(x) => {
                let (commands, status) = planner.consent(x);
                for p in commands {
                    intents.insert(p.name.clone(), Intent::Command(p));
                }
                intents.insert(status.name.clone(), Intent::Query(status));
            }
            ir::Form::Impersonate(x) => {
                for p in planner.impersonation(x) {
                    intents.insert(p.name.clone(), Intent::Command(p));
                }
            }
            ir::Form::Migration(x) => migrations.push(planner.migration(x)),
            ir::Form::Upcast(_) => {}
        }
        take(&mut planner, &path, map);
    }

    let impersonation_spec = ir::form_intents::impersonation(core).map(|i| {
        planner.at = "forms.impersonate".into();
        ImpersonationSpec {
            start: format!("Start{}Impersonation", i.entity),
            stop: format!("Stop{}Impersonation", i.entity),
            ttl_seconds: i.ttl_seconds.max(1) as u64,
            operator_check: planner.operator_check(i),
        }
    });
    let actor = core.actor.as_ref().map(|ad| {
        let mut c = sqlexpr::Compiler::new(core, &s, "actor");
        let su = c.superuser_sql().map(|t| sqlexpr::finalize(&format!("SELECT {t}")));
        let provider = match &ad.provider.callee {
            ir::Callee::Ext { name }
            | ir::Callee::Fn { name }
            | ir::Callee::Relation { name }
            | ir::Callee::Builtin { name }
            | ir::Callee::Intent { name } => name.clone(),
        };
        ActorSpec { entity: ad.entity.clone(), table: s.table(&ad.entity).table.clone(), provider, superuser: su }
    });
    let entities = s
        .tables
        .values()
        .map(|t| {
            let info = &core.entities[&t.entity];
            let columns = info
                .fields
                .iter()
                .filter_map(|f| {
                    let col = match t.col(&f.name)? {
                        schema::Col::Scalar { col, .. } | schema::Col::Counter { col } | schema::Col::Ref { col, .. } => col.clone(),
                        schema::Col::Union { id_col, .. } => id_col.clone(),
                        schema::Col::Snapshot { id_col, .. } => id_col.clone(),
                        schema::Col::Inverse { .. } => return None,
                    };
                    Some(ColumnSpec {
                        field: f.name.clone(),
                        column: col,
                        ty: ty_spec(&ty::Ty::from_ir(&f.ty)),
                        nullable: f.optional,
                        personal: f.personal,
                        encrypted: f.encrypted,
                    })
                })
                .collect();
            let columns: Vec<ColumnSpec> = columns;
            // key rotation re-encrypts the copies of an encrypted column too; only then are they named, so other plans stay as they were
            let copies = columns.iter().any(|c| c.encrypted);
            let published_table = (copies && t.published).then(|| schema::published_table(&t.table));
            let history_table = (copies && t.history).then(|| t.history_table.clone());
            (t.entity.clone(), EntitySpec { table: t.table.clone(), columns, personal: info.personal, published_table, history_table })
        })
        .collect();
    let mut enums: BTreeMap<String, Vec<String>> = core.enums.iter().map(|(n, d)| (n.clone(), d.values.clone())).collect();
    for (name, values) in ir::builtin::form_enums(core) {
        enums.entry(name).or_insert(values);
    }
    let records = core
        .records
        .iter()
        .map(|(n, r)| {
            let fields = r
                .fields
                .iter()
                .map(|f| ParamSpec { name: f.name.clone(), ty: ty::type_spec(&f.ty), optional: f.optional || f.default.is_some(), default: None })
                .collect();
            (n.clone(), fields)
        })
        .collect();
    let events =
        core.events.iter().map(|(n, e)| (n.clone(), e.fields.iter().map(|(f, t)| (f.clone(), ty_spec(&ty::Ty::from_ir(t)))).collect())).collect();
    let watched: std::collections::BTreeSet<String> = subscriptions.iter().flat_map(|s: &Subscription| s.reads.iter().cloned()).collect();
    let program = Program {
        ir_version: IR_VERSION.into(),
        actor,
        enums,
        records,
        entities,
        ddl: ddl.statements.iter().cloned().chain(rule_ddl).chain(plan::subscribe::notify_ddl(&watched)).collect(),
        intents,
        handlers,
        schedules,
        jobs,
        webhooks,
        rules,
        outbound,
        events,
        subscriptions,
        migrations,
        impersonation: impersonation_spec,
        extensions: core.uses.clone(),
        constraint_errors: ddl.codes.clone(),
    };
    // planner errors in source order, then what the runtime does not support
    by_decl.sort_by_key(|(pos, _)| *pos);
    let mut diagnostics: Vec<Diagnostic> = by_decl.into_iter().flat_map(|(_, d)| d).collect();
    unsupported.sort_by_key(|d| (d.line, d.col));
    diagnostics.extend(unsupported);
    Compiled { program, diagnostics }
}
