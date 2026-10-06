//! Human-readable execution plans (`aip explain <file> <Intent>`).

use aip_plan::*;
use std::fmt::Write;

pub fn explain(i: &Intent) -> String {
    let mut out = String::new();
    match i {
        Intent::Command(c) => {
            let _ =
                writeln!(out, "command {}{}{}", c.name, if c.idempotent { "  [idempotent]" } else { "" }, if c.audited { "  [audited]" } else { "" });
            let _ = writeln!(out, "  input: {}", params(&c.params));
            let _ = writeln!(out, "  transaction:");
            steps(&mut out, &c.steps, 2);
            if let Some(r) = &c.returns {
                let _ = writeln!(out, "  returns:\n      {}", r.text);
            }
            decrypts(&mut out, &c.decrypt);
            let _ = writeln!(out, "  writes: {}", c.writes.join(", "));
            if !c.emits.is_empty() {
                let _ = writeln!(out, "  emits (outbox, at-least-once): {}", c.emits.join(", "));
            }
            if !c.effects.is_empty() {
                let _ = writeln!(out, "  effects: {}", c.effects.join(", "));
            }
            let _ = writeln!(out, "  errors: {}", errors(&c.errors));
        }
        Intent::Query(q) => {
            let _ = writeln!(out, "query {}", q.name);
            let _ = writeln!(out, "  input: {}", params(&q.params));
            if !q.prelude.is_empty() {
                let _ = writeln!(out, "  before:");
                steps(&mut out, &q.prelude, 2);
            }
            for v in &q.variants {
                let _ = writeln!(out, "  statement{}: (1 round trip)", v.when.as_ref().map(|w| format!(" when sort = {w}")).unwrap_or_default());
                let _ = writeln!(out, "      {}", v.main.text);
            }
            decrypts(&mut out, &q.decrypt);
            let _ = writeln!(out, "  errors: {}", errors(&q.errors));
        }
    }
    out
}

/// The worker side of a `job`: what runs after the start command returns.
pub fn explain_job(j: &Job) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "job worker {}  (batches of {}, each batch commits its progress; items run at least once)", j.name, j.batch);
    if let Some(src) = &j.source {
        let _ = writeln!(out, "  items (snapshot at start):\n      {}", src.text);
    }
    if let Some(g) = &j.item_guard {
        let _ = writeln!(out, "  per item, re-check and lock (skip if gone):\n      {}", g.text);
    }
    if !j.steps.is_empty() {
        let _ = writeln!(out, "  per item{}:", j.item.as_ref().map(|i| format!(" ({i})")).unwrap_or_default());
        steps(&mut out, &j.steps, 2);
    }
    if let Some(x) = &j.export {
        let _ = writeln!(
            out,
            "  produce {} into bucket \"{}\"{}",
            x.format,
            x.bucket,
            x.expires_seconds.map(|s| format!(", deleted after {s}s")).unwrap_or_default()
        );
        let _ = writeln!(out, "    columns: {}", x.header.join(", "));
        for d in &x.decrypt {
            let _ = writeln!(out, "    column {} ({}) is decrypted before it is written to the file", d.column + 1, d.field);
        }
        let _ = writeln!(out, "      {}", x.rows.text);
    }
    if !j.finish.is_empty() {
        let _ = writeln!(out, "  when done:");
        steps(&mut out, &j.finish, 2);
    }
    out
}

/// Where the runtime opens ciphertext in the answer; the statement itself returns it as `{"c": ciphertext, "i": row id}`.
fn decrypts(out: &mut String, paths: &[DecryptPath]) {
    for p in paths {
        let _ = writeln!(
            out,
            "  decrypts {} at {} (after the statement, before the answer)",
            p.field,
            if p.path.is_empty() { "$".into() } else { p.path.join(".").replace(".[]", "[]") }
        );
    }
}

fn params(ps: &[ParamSpec]) -> String {
    ps.iter().map(|p| format!("{}{}", p.name, if p.optional { "?" } else { "" })).collect::<Vec<_>>().join(", ")
}

fn errors(es: &[ErrorSpec]) -> String {
    es.iter()
        .map(|e| match &e.reason {
            Some(r) => format!("{}({r})", e.code),
            None => e.code.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn steps(out: &mut String, ss: &[Step], depth: usize) {
    let pad = "  ".repeat(depth);
    for (i, s) in ss.iter().enumerate() {
        let n = i + 1;
        match s {
            Step::Load { param, entity, many, lock, sql } => {
                let _ = writeln!(
                    out,
                    "{pad}{n}. load {param}: {entity}{} {}",
                    if *many { "[]" } else { "" },
                    if *lock { "(lock for update)" } else { "(share lock)" }
                );
                let _ = writeln!(out, "{pad}     {}", sql.text);
            }
            Step::Lock { sql } => {
                let _ = writeln!(out, "{pad}{n}. lock\n{pad}     {}", sql.text);
            }
            Step::Let { name, sql, code } => {
                let _ =
                    writeln!(out, "{pad}{n}. let {name}{}\n{pad}     {}", code.as_ref().map(|c| format!(" else {c}")).unwrap_or_default(), sql.text);
            }
            Step::Check { kind, sql, code } => {
                let _ = writeln!(
                    out,
                    "{pad}{n}. check {kind:?}{}\n{pad}     {}",
                    code.as_ref().map(|c| format!(" else {c}")).unwrap_or_default(),
                    sql.text
                );
            }
            Step::NewId { name } => {
                let _ = writeln!(out, "{pad}{n}. pick the id of the row to insert -> {name}");
            }
            Step::Encrypt { field, source, member, bind, row } => {
                let of = match member {
                    Some(m) => format!("{source}.{m}"),
                    None => source.clone(),
                };
                let for_row = match row {
                    EncryptRow::New { id } => format!("the new row ({id})"),
                    EncryptRow::Existing { id } => format!("the row {}", id.text),
                };
                let _ = writeln!(out, "{pad}{n}. encrypt {of} for {field} of {for_row} -> {bind} (AES-256-GCM in the runtime)");
            }
            Step::Exec { sql, bind, label } => {
                let _ = writeln!(out, "{pad}{n}. {label}{}\n{pad}     {}", bind.as_ref().map(|b| format!(" -> {b}")).unwrap_or_default(), sql.text);
            }
            Step::When { cond, steps: inner } => {
                let _ = writeln!(out, "{pad}{n}. when {}", cond.text);
                steps(out, inner, depth + 2);
            }
            Step::EachPartial { source, item, steps: inner } => {
                let _ = writeln!(out, "{pad}{n}. each {source} as {item} (per-item savepoint)");
                steps(out, inner, depth + 2);
            }
            Step::ForEach { source, item, steps: inner } => {
                let _ = writeln!(out, "{pad}{n}. for each {item} in {}", source.text);
                steps(out, inner, depth + 2);
            }
            Step::Upload { param, bucket, .. } => {
                let _ = writeln!(out, "{pad}{n}. stage upload {param} -> bucket {bucket} (promoted after commit, GC on rollback)");
            }
            Step::Deferred { effect, args, .. } => {
                let _ = writeln!(out, "{pad}{n}. after commit: {effect} (outbox, retried)\n{pad}     {}", args.text);
            }
            Step::Emit { event, payload, broker } => {
                let _ = writeln!(
                    out,
                    "{pad}{n}. emit {event}{} (outbox)\n{pad}     {}",
                    broker.as_ref().map(|(b, t)| format!(" to {b}:{t}")).unwrap_or_default(),
                    payload.text
                );
            }
            Step::Notify { template, recipients, .. } => {
                let _ = writeln!(out, "{pad}{n}. notify via {template} (after commit)\n{pad}     {}", recipients.text);
            }
            Step::Fail { code, commit } => {
                let _ = writeln!(out, "{pad}{n}. fail {code}{}", if *commit { " (after committing)" } else { "" });
            }
            Step::ValidateDynamic { path, .. } => {
                let _ = writeln!(out, "{pad}{n}. validate {path} against its pinned dynamic schema");
            }
        }
    }
}
