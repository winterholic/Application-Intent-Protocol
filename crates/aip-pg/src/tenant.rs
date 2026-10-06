//! Tenants in SQL: how to read the tenant of a row, the triggers that keep
//! references between tenant entities inside one tenant, and the triggers that
//! pin a transaction's writes to one tenant. The meaning (what a tenant is, which
//! sets are bound) is `aip_ir::tenant`; this only spells it in SQL.
//!
//! The pin lives in two transaction-local settings. `aip.tenant` is the tenant the
//! transaction writes: the first write sets it, a write of another tenant fails.
//! `aip.tenant_cross` = `on` suspends the pin (declared `cross tenant`). Plans set
//! both through [`pin_step`], so every execution context (call, handler, webhook,
//! schedule item, rule row, job item) starts from a state it chose.

use crate::names::q;
use crate::schema::{Col, Schema};
use aip_ir::{self as ir, codes, tenant};
use aip_plan::ErrorSpec;
use std::collections::BTreeMap;

/// The tenant a plan filters to, and the entity parameters already known to share it.
#[derive(Debug, Clone)]
pub struct TenantScope {
    pub sql: String,
    pub checked: Vec<ir::Param>,
}

fn col_of(s: &Schema, entity: &str, field: &str) -> Option<String> {
    match s.table(entity).col(field) {
        Some(Col::Ref { col, .. }) => Some(col.clone()),
        _ => None,
    }
}

/// SQL for the tenant of the row `alias` stands for: the root's id. A root is its own tenant.
/// `None` for an entity that has no tenant.
pub fn row_sql(core: &ir::Program, s: &Schema, entity: &str, alias: &str) -> Option<String> {
    if tenant::scoped(core, entity) {
        let hops = tenant::path(core, entity).ok()?;
        let first = hops.first()?;
        let mut cur = format!("{alias}.{}", q(&col_of(s, &first.entity, &first.field)?));
        for (i, hop) in hops.iter().enumerate().skip(1) {
            let x = format!("tn{i}");
            let col = col_of(s, &hop.entity, &hop.field)?;
            cur = format!("(SELECT {x}.{} FROM {} {x} WHERE {x}.\"id\" = {cur})", q(&col), q(&s.table(&hop.entity).table));
        }
        Some(cur)
    } else if tenant::is_root(core, entity) {
        Some(format!("{alias}.\"id\""))
    } else {
        None
    }
}

/// SQL for the tenant of the row whose id is the expression `id`.
pub fn id_sql(core: &ir::Program, s: &Schema, entity: &str, id: &str) -> Option<String> {
    if tenant::scoped(core, entity) {
        let row = row_sql(core, s, entity, "tn0")?;
        Some(format!("(SELECT {row} FROM {} tn0 WHERE tn0.\"id\" = {id})", q(&s.table(entity).table)))
    } else if tenant::is_root(core, entity) {
        Some(id.to_string())
    } else {
        None
    }
}

/// A `SELECT` of the tenants of the rows `ids` (an id expression, or with `many` a subquery of ids) names.
pub fn rows_select(core: &ir::Program, s: &Schema, entity: &str, ids: &str, many: bool) -> Option<String> {
    let row = row_sql(core, s, entity, "tn0")?;
    let cond = if many { format!("tn0.\"id\" IN ({ids})") } else { format!("tn0.\"id\" = {ids}") };
    Some(format!("SELECT {row} AS t FROM {} tn0 WHERE {cond}", q(&s.table(entity).table)))
}

/// Name of the function every pin trigger calls, and the error name its failure reports.
const PIN_FN: &str = "_aip_tenant_pin";

/// A plan step that starts an execution context: clears the pin, then fixes it to `tenant`
/// (a SQL expression, or `None` to let the first write decide) and sets or clears the cross switch.
pub fn pin_step(tenant: Option<&str>, cross: bool) -> aip_plan::Step {
    let pin = match tenant {
        Some(t) if !cross => format!("coalesce(({t})::text, '')"),
        _ => "''".to_string(),
    };
    let text = format!("SELECT set_config('aip.tenant', {pin}, true), set_config('aip.tenant_cross', '{}', true)", if cross { "on" } else { "" });
    let label = if cross {
        "tenant: cross"
    } else if tenant.is_some() {
        "tenant: pin to the context's tenant"
    } else {
        "tenant: reset"
    };
    aip_plan::Step::Exec { sql: crate::sqlexpr::finalize(&text), bind: None, label: label.into() }
}

fn raise(name: &str) -> String {
    format!("RAISE EXCEPTION USING ERRCODE = 'P0001', MESSAGE = 'aip:{name}';")
}

/// Per tenant-scoped entity: a function that checks one row, and a trigger that runs it.
///
/// The check has three parts. (1) Every reference of the row to a row that has a tenant points
/// into the row's own tenant. (2) On update, every tenant-scoped row that references this one
/// is still in this one's tenant. (3) On update, the rows whose tenant is derived through this
/// one (their path starts here) are checked the same way, because moving a row moves them.
/// Part 3 is what makes moving a `Board` to another `Workspace` fail when a `Task` of
/// the board points at a task that stays behind.
pub fn ddl(core: &ir::Program, s: &Schema, later: &mut Vec<String>, codes_out: &mut BTreeMap<String, ErrorSpec>) {
    let scoped: Vec<(&String, Vec<tenant::Hop>)> =
        core.entities.keys().filter(|e| tenant::scoped(core, e)).filter_map(|e| tenant::path(core, e).ok().map(|h| (e, h))).collect();
    for (entity, hops) in &scoped {
        let t = s.table(entity);
        let table = &t.table;
        let name = format!("{table}__tenant");
        let fail = raise(&name);
        let Some(own) = row_sql(core, s, entity, "rec") else { continue };
        let mut body = format!(
            "DECLARE rec {tq}%ROWTYPE; t uuid; r record; BEGIN \
             IF depth > 16 THEN {fail} END IF; \
             SELECT * INTO rec FROM {tq} WHERE \"id\" = rid; IF NOT FOUND THEN RETURN; END IF; \
             t := {own}; ",
            tq = q(table)
        );
        let mut ref_cols = Vec::new();
        for (field, col) in &t.cols {
            let Col::Ref { col, target } = col else { continue };
            if tenant::has_tenant(core, target) {
                ref_cols.push(q(col));
            }
            // the first hop of the path defines the tenant, so it agrees with it by construction
            if *field == hops[0].field {
                continue;
            }
            if let Some(other) = id_sql(core, s, target, &format!("rec.{}", q(col))) {
                body.push_str(&format!("IF rec.{c} IS NOT NULL AND {other} IS DISTINCT FROM t THEN {fail} END IF; ", c = q(col)));
            }
        }
        for (other, other_hops) in &scoped {
            let ot = s.table(other);
            for (field, c) in &ot.cols {
                let Col::Ref { col, target } = c else { continue };
                if target != *entity {
                    continue;
                }
                let Some(their) = row_sql(core, s, other, "x") else { continue };
                body.push_str(&format!(
                    "IF deep AND EXISTS (SELECT 1 FROM {ot} x WHERE x.{c} = rid AND {their} IS DISTINCT FROM t) THEN {fail} END IF; ",
                    ot = q(&ot.table),
                    c = q(col)
                ));
                if *field == other_hops[0].field {
                    body.push_str(&format!(
                        "IF deep THEN FOR r IN SELECT \"id\" FROM {ot} WHERE {c} = rid LOOP PERFORM {f}(r.\"id\", true, depth + 1); END LOOP; END IF; ",
                        ot = q(&ot.table),
                        c = q(col),
                        f = q(&format!("{}__tenant_check", ot.table))
                    ));
                }
            }
        }
        body.push_str("END");
        later.push(format!(
            "CREATE OR REPLACE FUNCTION {}(rid uuid, deep boolean, depth int) RETURNS void LANGUAGE plpgsql AS $aip$ {body} $aip$",
            q(&format!("{table}__tenant_check"))
        ));
        let f = format!("{table}__tenant_fn");
        later.push(format!(
            "CREATE OR REPLACE FUNCTION {}() RETURNS trigger LANGUAGE plpgsql AS $aip$ BEGIN PERFORM {}(NEW.\"id\", TG_OP = 'UPDATE', 0); RETURN NULL; END $aip$;\n\
             CREATE TRIGGER {} AFTER INSERT OR UPDATE OF {} ON {} FOR EACH ROW EXECUTE FUNCTION {}()",
            q(&f),
            q(&format!("{table}__tenant_check")),
            q(&name),
            ref_cols.join(", "),
            q(table),
            q(&f)
        ));
        codes_out.insert(name, ErrorSpec { code: codes::TENANT_MISMATCH.into(), reason: None });
    }
    pin_ddl(core, s, later, codes_out);
}

/// Per entity that has a tenant (scoped entities and the roots): an AFTER trigger on every
/// insert, update and delete that pins the transaction to the tenant of the row, unless the
/// transaction is `cross`. An update checks the tenant a row leaves as well as the one it
/// enters, so a context pinned to one tenant can neither write into another nor pull a row out of it.
fn pin_ddl(core: &ir::Program, s: &Schema, later: &mut Vec<String>, codes_out: &mut BTreeMap<String, ErrorSpec>) {
    let tenants: Vec<&String> = core.entities.keys().filter(|e| tenant::has_tenant(core, e)).collect();
    if tenants.is_empty() {
        return;
    }
    let pin = q(PIN_FN);
    later.push(format!(
        "CREATE OR REPLACE FUNCTION {pin}(t uuid) RETURNS void LANGUAGE plpgsql AS $aip$ \
         DECLARE cur text := nullif(current_setting('aip.tenant', true), ''); \
         BEGIN IF t IS NULL THEN RETURN; END IF; \
         IF cur IS NULL THEN PERFORM set_config('aip.tenant', t::text, true); \
         ELSIF cur <> t::text THEN {} END IF; END $aip$",
        raise(PIN_FN)
    ));
    codes_out.insert(PIN_FN.into(), ErrorSpec { code: codes::TENANT_MISMATCH.into(), reason: None });
    for entity in tenants {
        let table = &s.table(entity).table;
        let (Some(new), Some(old)) = (row_sql(core, s, entity, "NEW"), row_sql(core, s, entity, "OLD")) else { continue };
        // a root cannot change tenant; a scoped row changes it only through the first reference of its path
        let moved = match tenant::path(core, entity).ok().and_then(|h| h.first().and_then(|f| col_of(s, &f.entity, &f.field))) {
            Some(c) => {
                let c = q(&c);
                format!("IF TG_OP = 'UPDATE' AND OLD.{c} IS DISTINCT FROM NEW.{c} THEN PERFORM {pin}(({old})); END IF; ")
            }
            None => String::new(),
        };
        let f = format!("{table}__tenant_pin_fn");
        later.push(format!(
            "CREATE OR REPLACE FUNCTION {}() RETURNS trigger LANGUAGE plpgsql AS $aip$ BEGIN \
             IF coalesce(current_setting('aip.tenant_cross', true), '') = 'on' THEN RETURN NULL; END IF; \
             IF TG_OP = 'DELETE' THEN PERFORM {pin}(({old})); RETURN NULL; END IF; \
             PERFORM {pin}(({new})); {moved}RETURN NULL; END $aip$;\n\
             CREATE TRIGGER {} AFTER INSERT OR UPDATE OR DELETE ON {} FOR EACH ROW EXECUTE FUNCTION {}()",
            q(&f),
            q(&format!("{table}__tenant_pin")),
            q(table),
            q(&f),
        ));
    }
}
