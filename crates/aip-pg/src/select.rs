//! Selections -> a single `jsonb` expression per row. Nested relations become
//! correlated subqueries, so a whole response tree is one statement.

use crate::names::{lit, q};
use crate::schema::Col;
use crate::sqlexpr::{Compiler, Val};
use crate::ty::Ty;
use aip_ir::{Callee, Expr, Node, Selection, Type};

/// Builds `jsonb_build_object(...)` for `row` (an entity row or id) and `sel`.
pub fn object(c: &mut Compiler<'_>, row: &Val, sel: &Selection) -> String {
    let entity = match row {
        Val::Row { entity, .. } | Val::Id { entity, .. } => entity.clone(),
        Val::Json { .. } => return json_object(c, row, sel),
        _ => return "NULL".into(),
    };
    let mut pairs: Vec<String> = Vec::new();
    c.push();
    c.push_row(row.clone());
    for it in &sel.items {
        let key = lit(&it.name);
        // where this item lands in the result: an encrypted value below it is found again by this path
        c.path_stack.push(it.name.clone());
        let value = match &it.value {
            Some(e) => {
                let v = c.expr(e);
                value_json(c, v, it.sub.as_ref())
            }
            None => field_json(c, &entity, row, &it.name, it.sub.as_ref()),
        };
        c.path_stack.pop();
        pairs.push(format!("{key}, {value}"));
    }
    c.pop_row();
    c.pop();
    build(pairs)
}

fn json_object(c: &mut Compiler<'_>, row: &Val, sel: &Selection) -> String {
    let mut pairs = Vec::new();
    c.push();
    c.push_row(row.clone());
    for it in &sel.items {
        let e = match &it.value {
            Some(e) => e.clone(),
            None => Expr { ty: Type::Unknown, node: Node::Local { name: it.name.clone() } },
        };
        let v = c.expr(&e);
        c.path_stack.push(it.name.clone());
        let j = value_json(c, v, it.sub.as_ref());
        c.path_stack.pop();
        pairs.push(format!("{}, {j}", lit(&it.name)));
    }
    c.pop_row();
    c.pop();
    build(pairs)
}

/// `jsonb_build_object` takes at most 100 arguments; longer objects are concatenated.
fn build(pairs: Vec<String>) -> String {
    if pairs.is_empty() {
        return "'{}'::jsonb".into();
    }
    let chunks: Vec<String> = pairs.chunks(40).map(|ch| format!("jsonb_build_object({})", ch.join(", "))).collect();
    chunks.join(" || ")
}

fn field_json(c: &mut Compiler<'_>, entity: &str, row: &Val, field: &str, sub: Option<&Selection>) -> String {
    let Some(col) = c.s.table(entity).col(field).cloned() else { return "NULL".into() };
    let base = match row {
        Val::Row { alias, .. } => Some(alias.clone()),
        _ => None,
    };
    let access = |c: &mut Compiler<'_>, colname: &str| -> String {
        match (&base, row) {
            (Some(a), _) => format!("{a}.{}", q(colname)),
            (None, Val::Id { sql, .. }) => {
                let t = q(&c.s.table(entity).table);
                let a = c.fresh_alias();
                format!("(SELECT {a}.{} FROM {t} {a} WHERE {a}.\"id\" = {sql})", q(colname))
            }
            _ => "NULL".into(),
        }
    };
    let raw = match col {
        Col::Scalar { col, .. } if c.is_encrypted(entity, field) => {
            // the database only has ciphertext: hand it over with the row id it was encrypted for, and the runtime opens it
            c.enc_paths.push(aip_plan::DecryptPath { path: c.path_stack.clone(), field: format!("{entity}.{field}") });
            let v = access(c, &col);
            let id = access(c, "id");
            format!("(CASE WHEN {v} IS NULL THEN NULL ELSE jsonb_build_object('c', {v}, 'i', {id}) END)")
        }
        Col::Scalar { col, ty } => {
            let v = access(c, &col);
            scalar_json(&v, &ty)
        }
        Col::Counter { col } => access(c, &col),
        Col::Ref { col, target } => {
            let id = access(c, &col);
            match sub {
                Some(sel) => nested(c, &target, &id, sel),
                None => format!("to_jsonb({id})"),
            }
        }
        Col::Union { type_col, id_col, .. } => {
            let t = access(c, &type_col);
            let i = access(c, &id_col);
            format!("jsonb_build_object('type', {t}, 'id', {i})")
        }
        Col::Snapshot { id_col, ver_col, .. } => {
            let i = access(c, &id_col);
            let v = access(c, &ver_col);
            format!("jsonb_build_object('id', {i}, 'version', {v})")
        }
        Col::Inverse { target, via_col } => {
            let parent = c.scalar(row);
            match sub {
                Some(sel) => array(c, &target, &format!("{{a}}.{} = {parent}", q(&via_col)), sel),
                None => "NULL".into(),
            }
        }
    };
    guard_field(c, entity, row, field, raw)
}

/// Field-level `visible to` and `masked unless`.
fn guard_field(c: &mut Compiler<'_>, entity: &str, row: &Val, field: &str, raw: String) -> String {
    let core = c.core;
    let Some(f) = core.entities.get(entity).and_then(|e| e.fields.iter().find(|f| f.name == field)) else { return raw };
    let mut out = raw;
    let is_actor = c.actor_entity() == Some(entity);
    if let Some(cond) = &f.visible_to {
        c.set_self_row(if is_actor { Some(row.clone()) } else { None });
        let p = c.pred(cond);
        c.set_self_row(None);
        out = format!("(CASE WHEN {p} THEN {out} END)");
    }
    if let Some(m) = &f.masked {
        c.set_self_row(if is_actor { Some(row.clone()) } else { None });
        let p = c.pred(&m.unless);
        c.set_self_row(None);
        let (Callee::Fn { name } | Callee::Relation { name } | Callee::Builtin { name } | Callee::Ext { name } | Callee::Intent { name }) =
            &m.with.callee;
        // the mask works on the plaintext, which only the runtime has: it is told which mask to apply after decrypting
        let encrypted = f.encrypted;
        let masked = match name.as_str() {
            _ if encrypted => format!("({out} || jsonb_build_object('m', {}))", lit(name)),
            "mask.email" => format!("to_jsonb(regexp_replace({out} #>> '{{}}', '^(.).*(@.*)$', '\\1***\\2'))"),
            "mask.phone" => format!("to_jsonb(regexp_replace({out} #>> '{{}}', '^(\\d{{3}}).*(\\d{{4}})$', '\\1-****-\\2'))"),
            _ => "NULL".into(),
        };
        out = format!("(CASE WHEN {p} THEN {out} ELSE {masked} END)");
    }
    out
}

pub fn scalar_json(v: &str, ty: &Ty) -> String {
    match ty {
        Ty::Object => format!("(CASE WHEN {v} IS NULL THEN NULL ELSE to_jsonb('/aip/objects/' || {v}) END)"),
        Ty::Range(_) => format!("(CASE WHEN {v} IS NULL THEN NULL ELSE jsonb_build_object('start', lower({v}), 'end', upper({v})) END)"),
        Ty::Json | Ty::Coll(_) | Ty::Record(_) | Ty::Localized(_) => v.to_string(),
        // a string keeps every digit; as a JSON number it would be read back through a double
        Ty::Money(_) | Ty::Decimal => format!("to_jsonb(({v})::text)"),
        _ => format!("to_jsonb({v})"),
    }
}

fn value_json(c: &mut Compiler<'_>, v: Val, sub: Option<&Selection>) -> String {
    match v {
        Val::Id { entity, sql } => match sub {
            Some(sel) => nested(c, &entity, &sql, sel),
            None => format!("to_jsonb({sql})"),
        },
        Val::Row { entity, alias } => match sub {
            Some(sel) => object(c, &Val::Row { entity, alias }, sel),
            None => format!("to_jsonb({alias}.\"id\")"),
        },
        Val::Scalar { sql, ty } => scalar_json(&sql, &ty),
        Val::Json { sql, record } => match sub {
            Some(sel) => json_object(c, &Val::Json { sql, record }, sel),
            None => sql,
        },
        Val::JsonArray { sql, .. } => sql,
        Val::IdSet { entity, sql } => match sub {
            Some(sel) => array(c, &entity, &format!("{{a}}.\"id\" IN ({sql})"), sel),
            None => format!("(SELECT coalesce(jsonb_agg(x), '[]'::jsonb) FROM ({sql}) s(x))"),
        },
        Val::Inverse { entity, via_col, parent } => match sub {
            Some(sel) => array(c, &entity, &format!("{{a}}.{} = {parent}", q(&via_col)), sel),
            None => "NULL".into(),
        },
        Val::ScalarSet { sql, .. } => format!("(SELECT coalesce(jsonb_agg(x), '[]'::jsonb) FROM ({sql}) s(x))"),
        Val::Snap { id, version, .. } => format!("jsonb_build_object('id', {id}, 'version', {version})"),
    }
}

/// One related row as an object (NULL when missing or not visible).
pub fn nested(c: &mut Compiler<'_>, entity: &str, id: &str, sel: &Selection) -> String {
    let t = q(&c.s.table(entity).table);
    let a = c.fresh_alias();
    let row = Val::Row { entity: entity.to_string(), alias: a.clone() };
    let mut conds = vec![format!("{a}.\"id\" = {id}")];
    extra_conds(c, entity, &row, &a, &mut conds);
    let obj = object(c, &row, sel);
    format!("(SELECT {obj} FROM {t} {a} WHERE {})", conds.join(" AND "))
}

/// Related rows as an array; `cond` uses `{a}` for the row alias.
pub fn array(c: &mut Compiler<'_>, entity: &str, cond: &str, sel: &Selection) -> String {
    let t = q(&c.s.table(entity).table);
    let a = c.fresh_alias();
    let row = Val::Row { entity: entity.to_string(), alias: a.clone() };
    let mut conds = vec![cond.replace("{a}", &a)];
    extra_conds(c, entity, &row, &a, &mut conds);
    c.path_stack.push("[]".into());
    let obj = object(c, &row, sel);
    c.path_stack.pop();
    let order = if c.s.table(entity).col("createdAt").is_some() { format!("{a}.\"created_at\", {a}.\"id\"") } else { format!("{a}.\"id\"") };
    format!("(SELECT coalesce(jsonb_agg({obj} ORDER BY {order}), '[]'::jsonb) FROM {t} {a} WHERE {})", conds.join(" AND "))
}

fn extra_conds(c: &mut Compiler<'_>, entity: &str, row: &Val, alias: &str, conds: &mut Vec<String>) {
    if c.s.table(entity).soft_delete {
        conds.push(format!("{alias}.\"deleted_at\" IS NULL"));
    }
    if let Some(v) = c.visibility_sql(entity, row) {
        conds.push(v);
    }
}
