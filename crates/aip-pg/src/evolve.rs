//! Schema evolution: the steps that bring a database deployed with one program to the next one.
//!
//! The meaning of the change comes from `aip_ir::diff` (what was added, renamed, removed or retyped, and what the
//! program declared about it). This module turns that into PostgreSQL: renames, `ADD COLUMN`, `SET NOT NULL`,
//! new constraints and indexes, and the declared drops. Anything that could destroy or reinterpret stored data
//! without the program saying so is not a step but a [`Rejection`].
//!
//! Tables, columns, constraints and indexes are compared by what the generator emits for the old and the new program
//! (the old one is regenerated from the recorded IR). The invariant is that old database + plan = a database a fresh
//! `migrate` of the new program would create: so a constraint whose generated name moved with its ordinal, or a rule
//! that changed its text, is dropped and created again instead of being tracked through a second description of it.
//! Functions and triggers are not compared at all: every one the generator owns is replaced by its current text, and
//! those the program no longer has are dropped, so they cannot be out of date.

use crate::ddl;
use crate::names::q;
use crate::schema::{Col, Schema, published_table};
use aip_ir::diff::{self, Change, TypeRelation};
use aip_ir::{self as ir, Type, codes};
const ENCRYPTION_FIX: &str = "add the field in its new form under a new name, copy the values with a `migration` (it writes through the runtime, which encrypts), and remove the old field with `removed field` in a later deployment.";

use aip_plan::{DataCheck, EvolvePlan, EvolveStep, Rejection, StepClass};
use std::collections::{BTreeMap, BTreeSet};

/// Version of what the generator emits for a program. Recorded with every deployment; a database deployed by another
/// version gets its functions and triggers replaced by this version's even when the program did not change.
pub const DDL_VERSION: u32 = 1;

pub fn plan(old: &ir::Program, new: &ir::Program, new_ddl: &[String]) -> EvolvePlan {
    let mut p = Planner::new(old, new);
    p.run(new_ddl);
    p.out
}

// ---------- reading the generator's own statements ----------

#[derive(Debug, Clone)]
struct TableDef {
    name: String,
    columns: Vec<(String, String)>,
    like: bool,
    sql: String,
}

#[derive(Debug, Clone)]
enum Obj {
    Index {
        name: String,
        table: String,
        unique: bool,
        sql: String,
    },
    Constraint {
        name: String,
        table: String,
        inline: bool,
        sql: String,
    },
    /// A column of its own statement (the generated `tsvector` of a search).
    GenCol {
        name: String,
        table: String,
        sql: String,
    },
}

impl Obj {
    fn table(&self) -> &str {
        match self {
            Obj::Index { table, .. } | Obj::Constraint { table, .. } | Obj::GenCol { table, .. } => table,
        }
    }

    fn sql(&self) -> &str {
        match self {
            Obj::Index { sql, .. } | Obj::Constraint { sql, .. } | Obj::GenCol { sql, .. } => sql,
        }
    }

    fn key(&self) -> String {
        match self {
            Obj::Index { name, .. } => format!("index:{name}"),
            Obj::Constraint { name, table, .. } => format!("constraint:{table}:{name}"),
            Obj::GenCol { name, table, .. } => format!("column:{table}:{name}"),
        }
    }
}

#[derive(Debug, Clone)]
struct Routine {
    sql: String,
    triggers: Vec<(String, String)>,
    functions: Vec<String>,
}

#[derive(Debug, Default)]
struct Catalog {
    tables: Vec<TableDef>,
    objects: Vec<Obj>,
    routines: Vec<Routine>,
    /// `IF NOT EXISTS` statements that can run again at any time.
    idempotent: Vec<String>,
    other: Vec<String>,
}

fn quoted_after<'a>(s: &'a str, marker: &str) -> Option<(String, &'a str)> {
    let i = s.find(marker)? + marker.len();
    let rest = &s[i..];
    let a = rest.find('"')? + 1;
    let b = rest[a..].find('"')? + a;
    Some((rest[a..b].to_string(), &rest[b + 1..]))
}

/// Splits at commas that are not inside parentheses, double quotes or single quotes.
fn split_top(s: &str) -> Vec<String> {
    let (mut depth, mut dq, mut sq) = (0i32, false, false);
    let mut out = vec![String::new()];
    for c in s.chars() {
        match c {
            '"' if !sq => dq = !dq,
            '\'' if !dq => sq = !sq,
            '(' if !dq && !sq => depth += 1,
            ')' if !dq && !sq => depth -= 1,
            ',' if depth == 0 && !dq && !sq => {
                out.push(String::new());
                continue;
            }
            _ => {}
        }
        if let Some(last) = out.last_mut() {
            last.push(c);
        }
    }
    out.into_iter().map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

impl Catalog {
    fn read(stmts: &[String]) -> Catalog {
        let mut c = Catalog::default();
        for s in stmts {
            let t = s.trim();
            let head = t.split_whitespace().take(10).collect::<Vec<_>>().join(" ");
            let name = quoted_after(t, "").map(|x| x.0).unwrap_or_default();
            if head.starts_with("CREATE EXTENSION") || head.starts_with("CREATE TABLE IF NOT EXISTS") {
                c.idempotent.push(t.to_string());
            } else if head.starts_with("CREATE TABLE ") {
                if name.starts_with("_aip_") {
                    c.idempotent.push(t.to_string());
                } else {
                    c.read_table(&name, t);
                }
            } else if head.starts_with("CREATE UNIQUE INDEX") || head.starts_with("CREATE INDEX") {
                let table = quoted_after(t, " ON ").map(|x| x.0).unwrap_or_default();
                if name.starts_with("_aip_") {
                    c.idempotent.push(t.to_string());
                } else {
                    c.objects.push(Obj::Index { name, table, unique: head.starts_with("CREATE UNIQUE"), sql: t.to_string() });
                }
            } else if head.starts_with("ALTER TABLE ") {
                if name.starts_with("_aip_") {
                    c.idempotent.push(t.to_string());
                } else if let Some((cname, _)) = quoted_after(t, " ADD CONSTRAINT ") {
                    c.objects.push(Obj::Constraint { name: cname, table: name, inline: false, sql: t.to_string() });
                } else if let Some((col, _)) = quoted_after(t, " ADD COLUMN ") {
                    c.objects.push(Obj::GenCol { name: col, table: name, sql: t.to_string() });
                } else {
                    c.other.push(t.to_string());
                }
            } else if t.contains("CREATE OR REPLACE FUNCTION") || t.contains("CREATE TRIGGER") || t.contains("CREATE CONSTRAINT TRIGGER") {
                c.routines.push(Routine::read(t));
            } else {
                c.other.push(t.to_string());
            }
        }
        c
    }

    fn read_table(&mut self, name: &str, sql: &str) {
        let open = sql.find('(').unwrap_or(0);
        let close = sql.rfind(')').unwrap_or(sql.len());
        let body = &sql[(open + 1).min(close)..close];
        let like = body.trim_start().starts_with("LIKE ");
        let mut columns = Vec::new();
        if !like {
            for el in split_top(body) {
                if el.starts_with("CONSTRAINT ") {
                    let cname = quoted_after(&el, "CONSTRAINT ").map(|x| x.0).unwrap_or_default();
                    self.objects.push(Obj::Constraint {
                        name: cname,
                        table: name.to_string(),
                        inline: true,
                        sql: format!("ALTER TABLE {} ADD {el}", q(name)),
                    });
                } else if el.starts_with('"') {
                    let col = quoted_after(&el, "").map(|x| x.0).unwrap_or_default();
                    columns.push((col, el));
                }
            }
        }
        self.tables.push(TableDef { name: name.to_string(), columns, like, sql: sql.to_string() });
    }
}

impl Routine {
    fn read(sql: &str) -> Routine {
        let mut triggers = Vec::new();
        let mut functions = Vec::new();
        let mut rest = sql;
        while let Some((f, after)) = quoted_after(rest, "CREATE OR REPLACE FUNCTION ") {
            functions.push(f);
            rest = after;
        }
        for marker in ["CREATE TRIGGER ", "CREATE CONSTRAINT TRIGGER "] {
            let mut rest = sql;
            while let Some((tg, after)) = quoted_after(rest, marker) {
                let table = quoted_after(after, " ON ").map(|x| x.0).unwrap_or_default();
                triggers.push((tg, table));
                rest = after;
            }
        }
        Routine { sql: sql.to_string(), triggers, functions }
    }
}

/// `"col" type [NOT NULL] [DEFAULT x]`.
#[derive(Debug, PartialEq, Eq)]
struct ColDef {
    ty: String,
    not_null: bool,
    default: Option<String>,
    pk: bool,
}

fn parse_def(def: &str) -> ColDef {
    let rest = quoted_after(def, "").map(|x| x.1).unwrap_or("").trim();
    let end = [" NOT NULL", " DEFAULT", " PRIMARY KEY", " GENERATED"].iter().filter_map(|m| rest.find(m)).min().unwrap_or(rest.len());
    let default = rest.find(" DEFAULT ").map(|i| rest[i + 9..].trim().to_string());
    ColDef { ty: rest[..end].trim().to_string(), not_null: rest.contains("NOT NULL"), default, pk: rest.contains("PRIMARY KEY") }
}

/// Columns of a unique index and its `WHERE`.
fn parse_index(sql: &str) -> (Vec<String>, Option<String>) {
    let on = sql.find(" ON ").unwrap_or(0);
    let open = sql[on..].find('(').map(|i| i + on).unwrap_or(0);
    let mut depth = 0;
    let mut close = sql.len() - 1;
    for (i, c) in sql[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = open + i;
                    break;
                }
            }
            _ => {}
        }
    }
    let cols = split_top(&sql[open + 1..close]).into_iter().map(|c| c.trim_matches('"').to_string()).collect();
    let tail = sql[close + 1..].trim();
    (cols, tail.strip_prefix("WHERE ").map(|w| w.to_string()))
}

fn col_names(c: Option<&Col>) -> Vec<String> {
    match c {
        Some(Col::Scalar { col, .. }) | Some(Col::Ref { col, .. }) | Some(Col::Counter { col }) => vec![col.clone()],
        Some(Col::Union { type_col, id_col, .. }) => vec![type_col.clone(), id_col.clone()],
        Some(Col::Snapshot { id_col, ver_col, .. }) => vec![id_col.clone(), ver_col.clone()],
        Some(Col::Inverse { .. }) | None => Vec::new(),
    }
}

fn check(what: String, table: &str, cond: &str) -> DataCheck {
    DataCheck {
        what,
        count_sql: format!("SELECT count(*) FROM {} WHERE {cond}", q(table)),
        sample_sql: format!("SELECT \"id\"::text FROM {} WHERE {cond} ORDER BY \"id\" LIMIT 5", q(table)),
    }
}

// ---------- the planner ----------

struct Planner<'a> {
    old: &'a ir::Program,
    new: &'a ir::Program,
    old_s: Schema,
    new_s: Schema,
    out: EvolvePlan,
    /// Old table -> new table, for the tables of renamed entities.
    tables: BTreeMap<String, String>,
    /// (old table, old column) -> new column.
    columns: BTreeMap<(String, String), String>,
    /// Old tables that go away, and (old table, old column) that are dropped, on the program's say-so.
    renamed_tables: BTreeSet<String>,
    dead_tables: BTreeSet<String>,
    dead_columns: BTreeSet<(String, String)>,
    // phases, in the order they run
    renames: Vec<EvolveStep>,
    drops: Vec<EvolveStep>,
    creates: Vec<EvolveStep>,
    alters: Vec<EvolveStep>,
    dropped: Vec<EvolveStep>,
    objects: Vec<EvolveStep>,
}

fn step(sql: String, class: StepClass, why: impl Into<String>) -> EvolveStep {
    EvolveStep { sql, class, why: why.into(), checks: Vec::new() }
}

impl<'a> Planner<'a> {
    fn new(old: &'a ir::Program, new: &'a ir::Program) -> Self {
        Planner {
            old,
            new,
            old_s: Schema::build(old),
            new_s: Schema::build(new),
            out: EvolvePlan::default(),
            tables: BTreeMap::new(),
            columns: BTreeMap::new(),
            renamed_tables: BTreeSet::new(),
            dead_tables: BTreeSet::new(),
            dead_columns: BTreeSet::new(),
            renames: Vec::new(),
            drops: Vec::new(),
            creates: Vec::new(),
            alters: Vec::new(),
            dropped: Vec::new(),
            objects: Vec::new(),
        }
    }

    fn reject(&mut self, code: &str, message: String, fix: &str) {
        self.out.rejections.push(Rejection { code: code.into(), message, fix: fix.into() });
    }

    fn new_table(&self, old_table: &str) -> String {
        self.tables.get(old_table).cloned().unwrap_or_else(|| old_table.to_string())
    }

    fn new_column(&self, old_table: &str, col: &str) -> String {
        self.columns.get(&(old_table.to_string(), col.to_string())).cloned().unwrap_or_else(|| col.to_string())
    }

    fn run(&mut self, new_ddl: &[String]) {
        let changes = diff::diff(self.old, self.new);
        for ch in &changes {
            self.read_change(ch);
        }
        let oc = Catalog::read(&ddl::generate(self.old, &self.old_s).statements);
        let nc = Catalog::read(new_ddl);
        self.tables_and_columns(&oc, &nc);
        self.objects(&oc, &nc);
        self.assemble(&oc, &nc);
    }

    // ---- what the IR diff says ----

    fn read_change(&mut self, ch: &Change) {
        match ch {
            Change::EntityRenamed { from, to } => {
                // the ciphertext of a field is bound to its entity's name
                if self.old.entities.get(from).is_some_and(|e| e.fields.iter().any(|f| f.encrypted)) {
                    self.reject(
                        codes::SCHEMA_ENCRYPTION_CHANGE,
                        format!("entity {from} has encrypted fields and continues as {to}; their ciphertext is bound to the name {from}"),
                        ENCRYPTION_FIX,
                    );
                }
                let (o, n) = (self.old_s.table(from).clone(), self.new_s.table(to).clone());
                self.tables.insert(o.table.clone(), n.table.clone());
                if o.history && n.history {
                    self.tables.insert(o.history_table.clone(), n.history_table.clone());
                }
                if o.published && n.published {
                    self.tables.insert(published_table(&o.table), published_table(&n.table));
                }
                for (a, b) in self.tables.clone() {
                    if a != b && !self.renamed_tables.contains(&a) {
                        self.renamed_tables.insert(a.clone());
                        // the primary key keeps the old table's name until it is renamed too
                        self.renames.push(step(
                            format!("ALTER TABLE {} RENAME TO {};\nALTER TABLE {} RENAME CONSTRAINT {} TO {}", q(&a), q(&b), q(&b), q(&format!("{a}_pkey")), q(&format!("{b}_pkey"))),
                            StepClass::Safe,
                            format!("entity {from} continues as {to} (was)"),
                        ));
                    }
                }
            }
            Change::EntityRemoved { entity, declared } => {
                let t = self.old_s.table(entity).clone();
                if *declared {
                    // the snapshot and history tables go before the working table they hang from
                    let mut names = Vec::new();
                    if t.published {
                        names.push(published_table(&t.table));
                    }
                    if t.history {
                        names.push(t.history_table.clone());
                    }
                    names.push(t.table.clone());
                    for name in names {
                        self.dead_tables.insert(name.clone());
                        self.dropped.push(step(format!("DROP TABLE {}", q(&name)), StepClass::Declared, format!("removed entity {entity}")));
                    }
                } else {
                    let added: Vec<String> = self.new.entities.keys().filter(|n| !self.old.entities.contains_key(*n)).cloned().collect();
                    let hint = if added.is_empty() { String::new() } else { format!(" (new in the program: {})", added.join(", ")) };
                    self.reject(
                        codes::SCHEMA_UNDECLARED,
                        format!("entity {entity} is in the database but not in the program{hint}"),
                        &format!("renamed: write `entity <NewName> was {entity}`. Removed (its rows are deleted): write `removed entity {entity}` at top level."),
                    );
                }
            }
            Change::FieldRenamed { entity, from, to } => {
                let Some(oe) = diff::old_entity_name(self.old, self.new, entity) else { return };
                if self.old.entities[oe].fields.iter().any(|f| f.name == *from && f.encrypted) {
                    self.reject(
                        codes::SCHEMA_ENCRYPTION_CHANGE,
                        format!("{entity}.{to} continues the encrypted field {from}; its ciphertext is bound to the name {oe}.{from}"),
                        ENCRYPTION_FIX,
                    );
                }
                let (ot, nt) = (self.old_s.table(oe).clone(), self.new_s.table(entity).clone());
                let (oc, nc) = (col_names(ot.col(from)), col_names(nt.col(to)));
                for (a, b) in oc.iter().zip(&nc) {
                    if a != b {
                        self.columns.insert((ot.table.clone(), a.clone()), b.clone());
                        let why = format!("{entity}.{from} continues as {to} (was)");
                        let mut sql = format!("ALTER TABLE {} RENAME COLUMN {} TO {}", q(&nt.table), q(a), q(b));
                        if nt.published {
                            sql.push_str(&format!(";\nALTER TABLE {} RENAME COLUMN {} TO {}", q(&published_table(&nt.table)), q(a), q(b)));
                        }
                        self.renames.push(step(sql, StepClass::Safe, why));
                    }
                }
            }
            Change::FieldRemoved { entity, field, declared } => {
                let Some(oe) = diff::old_entity_name(self.old, self.new, entity) else { return };
                let of = self.old.entities[oe].fields.iter().find(|f| f.name == *field);
                // a virtual field (the other side of a reference) has no column, so nothing is lost
                if matches!(of.map(|f| &f.kind), Some(ir::FieldKind::Inverse { .. })) {
                    return;
                }
                if *declared {
                    let ot = self.old_s.table(oe).table.clone();
                    for c in col_names(self.old_s.table(oe).col(field)) {
                        self.dead_columns.insert((ot.clone(), c));
                    }
                } else {
                    let added: Vec<String> = self.new.entities[entity]
                        .fields
                        .iter()
                        .filter(|f| diff::old_field_name(&self.old.entities[oe], f).is_none())
                        .map(|f| f.name.clone())
                        .collect();
                    let hint = if added.is_empty() { String::new() } else { format!(" (new in {entity}: {})", added.join(", ")) };
                    self.reject(
                        codes::SCHEMA_UNDECLARED,
                        format!("field {entity}.{field} is in the database but not in the program{hint}"),
                        &format!(
                            "renamed: write `was {field}` after the new field. Removed (its values are deleted): write `removed field {field}` inside entity {entity}."
                        ),
                    );
                }
            }
            Change::FieldEncryption { entity, field, now } => self.reject(
                codes::SCHEMA_ENCRYPTION_CHANGE,
                if *now {
                    format!("{entity}.{field} became encrypted, but its stored values are plaintext")
                } else {
                    format!("{entity}.{field} stopped being encrypted, but its stored values are ciphertext")
                },
                ENCRYPTION_FIX,
            ),
            Change::FieldKind { entity, field } => self.reject(
                codes::SCHEMA_UNSUPPORTED,
                format!("{entity}.{field} changed what kind of field it is (stored value, reference, counter, ...)"),
                "add the field in its new form under a new name, copy the data with a `migration`, and remove the old field with `removed field` in a later deployment.",
            ),
            Change::TraitChanged { entity, name, now } => self.reject(
                codes::SCHEMA_UNSUPPORTED,
                format!("{entity} {} the `{}` trait, which owns tables and triggers of its own", if *now { "gained" } else { "lost" }, name.replace('_', " ")),
                "create the entity anew under another name and move the rows with a `migration`, or change the database by hand and start again.",
            ),
            Change::EnumReordered { name } => self.reject(
                codes::SCHEMA_UNSUPPORTED,
                format!("the values of ordered enum {name} changed order: comparisons such as `>=` would read differently for rows already stored"),
                "keep the existing values in their order and add new values; or make the enum unordered.",
            ),
            Change::EnumValueAdded { name, value, inside: true, ordered: true } => self.out.notes.push(format!(
                "ordered enum {name} gained {value} in the middle of its order: comparisons that include its position now include it; stored rows keep their relative order"
            )),
            Change::ConstraintRemoved { entity, kind } => self.out.notes.push(format!("{entity}: {kind} is no longer enforced")),
            Change::FieldType { entity, field, from, to, relation } => self.field_type(entity, field, from, to, *relation),
            _ => {}
        }
    }

    fn field_type(&mut self, entity: &str, field: &str, from: &Type, to: &Type, relation: TypeRelation) {
        let again =
            "add a field of the new type, copy the values with a `migration`, and remove the old field with `removed field` in a later deployment.";
        match relation {
            TypeRelation::Widens => {}
            TypeRelation::Incompatible => self.reject(
                codes::SCHEMA_TYPE_CHANGE,
                format!("{entity}.{field} changed type in a way that can lose or reinterpret stored values ({})", describe(from, to)),
                again,
            ),
            // the stored value is ciphertext whatever the limits say; the runtime checks the limits of what is written from now on
            TypeRelation::Narrows if self.new.entities.get(entity).is_some_and(|e| e.fields.iter().any(|f| f.name == field && f.encrypted)) => {
                self.out.notes.push(format!("{entity}.{field} is encrypted and accepts fewer values than before; stored values are not checked against the new limits"));
            }
            TypeRelation::Narrows => {
                // the database keeps the value as text or bigint whatever the limits are; the rows only have to fit the new limits
                let Some(oe) = diff::old_entity_name(self.old, self.new, entity) else { return };
                let Some(Col::Scalar { col, .. }) = self.old_s.table(oe).col(field).cloned() else {
                    return self.reject(
                        codes::SCHEMA_TYPE_CHANGE,
                        format!("{entity}.{field} became narrower and the existing values cannot be checked ({})", describe(from, to)),
                        again,
                    );
                };
                let table = self.new_s.table(entity).table.clone();
                let c = q(&self.new_column(&self.old_s.table(oe).table, &col));
                let mut conds = Vec::new();
                match (from, to) {
                    (
                        Type::Text { min: a0, max: a1, trim, lower, pattern, .. },
                        Type::Text { min: b0, max: b1, trim: t2, lower: l2, pattern: p2 },
                    ) if (!t2 || trim == t2) && (!l2 || lower == l2) && (p2.is_none() || pattern == p2) => {
                        if b0.is_some() && b0 != a0 {
                            conds.push(format!("char_length({c}) < {}", b0.unwrap_or(0)));
                        }
                        if b1.is_some() && b1 != a1 {
                            conds.push(format!("char_length({c}) > {}", b1.unwrap_or(0)));
                        }
                    }
                    (Type::Int { min: a0, max: a1 }, Type::Int { min: b0, max: b1 }) => {
                        if b0.is_some() && b0 != a0 {
                            conds.push(format!("{c} < {}", b0.unwrap_or(0)));
                        }
                        if b1.is_some() && b1 != a1 {
                            conds.push(format!("{c} > {}", b1.unwrap_or(0)));
                        }
                    }
                    _ => {}
                }
                if conds.is_empty() {
                    return self.reject(
                        codes::SCHEMA_TYPE_CHANGE,
                        format!("{entity}.{field} became narrower and the existing values cannot be checked ({})", describe(from, to)),
                        again,
                    );
                }
                let mut s =
                    step(String::new(), StepClass::Checked, format!("{entity}.{field} accepts fewer values than before; the rows must already fit"));
                s.checks.push(check(format!("{entity}.{field} holds values the new type refuses"), &table, &conds.join(" OR ")));
                self.alters.push(s);
            }
        }
    }

    // ---- tables and columns ----

    fn tables_and_columns(&mut self, oc: &Catalog, nc: &Catalog) {
        let new_published: BTreeSet<&str> = nc.tables.iter().filter(|t| t.like).map(|t| t.name.as_str()).collect();
        for nt in &nc.tables {
            let known = oc.tables.iter().any(|ot| self.new_table(&ot.name) == nt.name);
            if !known {
                self.creates.push(step(nt.sql.clone(), StepClass::Safe, format!("new table {}", nt.name)));
            }
        }
        for ot in &oc.tables {
            let nname = self.new_table(&ot.name);
            let Some(nt) = nc.tables.iter().find(|t| t.name == nname) else { continue };
            if ot.like || nt.like {
                continue;
            }
            let mirror = new_published.contains(published_table(&nt.name).as_str());
            for (ncol, ndef) in &nt.columns {
                let ocol =
                    self.columns.iter().find(|((t, _), v)| *t == ot.name && *v == ncol).map(|((_, c), _)| c.clone()).unwrap_or_else(|| ncol.clone());
                match ot.columns.iter().find(|(c, _)| *c == ocol) {
                    None => self.add_column(&nt.name, ncol, ndef, mirror),
                    Some((_, odef)) => self.alter_column(&nt.name, ncol, odef, ndef, mirror),
                }
            }
            for (ocol, _) in &ot.columns {
                let mapped = self.new_column(&ot.name, ocol);
                if !nt.columns.iter().any(|(c, _)| *c == mapped) && self.dead_columns.contains(&(ot.name.clone(), ocol.clone())) {
                    let mut sql = format!("ALTER TABLE {} DROP COLUMN {}", q(&nt.name), q(ocol));
                    if mirror {
                        sql.push_str(&format!(";\nALTER TABLE {} DROP COLUMN {}", q(&published_table(&nt.name)), q(ocol)));
                    }
                    self.dropped.push(step(sql, StepClass::Declared, format!("removed field: {}.{ocol} and its values are dropped", nt.name)));
                }
            }
        }
    }

    fn add_column(&mut self, table: &str, col: &str, def: &str, mirror: bool) {
        let d = parse_def(def);
        let mut sql = format!("ALTER TABLE {} ADD COLUMN {def}", q(table));
        if mirror {
            sql.push_str(&format!(";\nALTER TABLE {} ADD COLUMN {def}", q(&published_table(table))));
            // `LIKE` copies NOT NULL but not defaults, so the snapshot's copy must not keep one
            if d.default.is_some() {
                sql.push_str(&format!(";\nALTER TABLE {} ALTER COLUMN {} DROP DEFAULT", q(&published_table(table)), q(col)));
            }
        }
        let needs_rows_to_fit = d.not_null && d.default.is_none() && !d.pk;
        let mut s = step(
            sql,
            if needs_rows_to_fit { StepClass::Checked } else { StepClass::Safe },
            if d.not_null && d.default.is_some() {
                format!("new required column {table}.{col}; existing rows get the default")
            } else if d.not_null {
                format!("new required column {table}.{col} without a default; only an empty table can take it")
            } else {
                format!("new optional column {table}.{col}")
            },
        );
        if needs_rows_to_fit {
            s.checks.push(DataCheck {
                what: format!("{table}.{col} is required and has no default, but {table} already has rows"),
                count_sql: format!("SELECT count(*) FROM {}", q(table)),
                sample_sql: format!("SELECT \"id\"::text FROM {} ORDER BY \"id\" LIMIT 5", q(table)),
            });
        }
        self.alters.push(s);
    }

    fn alter_column(&mut self, table: &str, col: &str, odef: &str, ndef: &str, mirror: bool) {
        let (o, n) = (parse_def(odef), parse_def(ndef));
        let both = |sql: String, mirror: bool| -> String {
            if mirror {
                let p = sql.replacen(&q(table), &q(&published_table(table)), 1);
                format!("{sql};\n{p}")
            } else {
                sql
            }
        };
        if o.ty != n.ty || o.pk != n.pk {
            self.reject(
                codes::SCHEMA_TYPE_CHANGE,
                format!("column {table}.{col} changes from `{}` to `{}`", odef.trim(), ndef.trim()),
                "add a column of the new type, copy the values with a `migration`, and remove the old field with `removed field` in a later deployment.",
            );
            return;
        }
        if !o.not_null && n.not_null {
            let mut s = step(
                both(format!("ALTER TABLE {} ALTER COLUMN {} SET NOT NULL", q(table), q(col)), mirror),
                StepClass::Checked,
                format!("{table}.{col} becomes required; no row may be empty"),
            );
            s.checks.push(check(format!("{table}.{col} is empty in some rows but the field is now required"), table, &format!("{} IS NULL", q(col))));
            self.alters.push(s);
        } else if o.not_null && !n.not_null {
            self.alters.push(step(
                both(format!("ALTER TABLE {} ALTER COLUMN {} DROP NOT NULL", q(table), q(col)), mirror),
                StepClass::Relaxing,
                format!("{table}.{col} becomes optional"),
            ));
        }
        if o.default != n.default {
            let sql = match &n.default {
                Some(d) => format!("ALTER TABLE {} ALTER COLUMN {} SET DEFAULT {d}", q(table), q(col)),
                None => format!("ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT", q(table), q(col)),
            };
            self.alters.push(step(sql, StepClass::Safe, format!("default of {table}.{col} changes (rows keep their values)")));
        }
    }

    // ---- constraints, indexes, generated columns ----

    fn objects(&mut self, oc: &Catalog, nc: &Catalog) {
        // old text with the renames applied, so a rule on a renamed column or table is not seen as changed
        let adjust = |o: &Obj| -> String {
            let mut t = o.sql().to_string();
            for ((table, col), new) in &self.columns {
                if table == o.table() {
                    t = t.replace(&q(col), &q(new));
                }
            }
            for (a, b) in &self.tables {
                t = t.replace(&q(a), &q(b));
            }
            t
        };
        let new_keys: BTreeMap<String, &Obj> = nc.objects.iter().map(|o| (o.key(), o)).collect();
        let created_tables: BTreeSet<&str> =
            nc.tables.iter().filter(|nt| !oc.tables.iter().any(|ot| self.new_table(&ot.name) == nt.name)).map(|t| t.name.as_str()).collect();
        // a generated column dropped or redefined takes its indexes with it
        let mut rebuilt: BTreeSet<(String, String)> = BTreeSet::new();
        let mut stale: BTreeSet<String> = BTreeSet::new();
        let mut drops: Vec<EvolveStep> = Vec::new();
        for o in &oc.objects {
            let table = self.new_table(o.table());
            if self.dead_tables.contains(o.table()) {
                continue;
            }
            let key = match o {
                Obj::Index { .. } => o.key(),
                Obj::Constraint { name, .. } => format!("constraint:{table}:{name}"),
                Obj::GenCol { name, .. } => format!("column:{table}:{name}"),
            };
            let same = new_keys.get(&key).is_some_and(|n| n.sql() == adjust(o));
            if same {
                continue;
            }
            stale.insert(key.clone());
            let (sql, why) = match o {
                Obj::Index { name, .. } => (format!("DROP INDEX IF EXISTS {}", q(name)), format!("index {name} is replaced or no longer needed")),
                Obj::Constraint { name, .. } => (
                    format!("ALTER TABLE {} DROP CONSTRAINT IF EXISTS {}", q(&table), q(name)),
                    format!("constraint {name} on {table} is replaced or no longer needed"),
                ),
                Obj::GenCol { name, .. } => {
                    rebuilt.insert((table.clone(), name.clone()));
                    (
                        format!("ALTER TABLE {} DROP COLUMN IF EXISTS {}", q(&table), q(name)),
                        format!("derived column {name} on {table} is replaced or no longer needed"),
                    )
                }
            };
            drops.push(step(sql, StepClass::Relaxing, why));
        }
        // indexes on a column that was rebuilt are gone with it
        for o in &oc.objects {
            if let Obj::Index { name, table, sql, .. } = o {
                let t = self.new_table(table);
                if rebuilt.iter().any(|(rt, c)| *rt == t && sql.contains(&q(c))) && !stale.contains(&o.key()) {
                    stale.insert(o.key());
                    drops.push(step(
                        format!("DROP INDEX IF EXISTS {}", q(name)),
                        StepClass::Relaxing,
                        format!("index {name} depends on a rebuilt column"),
                    ));
                }
            }
        }
        self.drops.extend(drops);
        let old_keys: BTreeMap<String, ()> = oc
            .objects
            .iter()
            .map(|o| {
                let t = self.new_table(o.table());
                (
                    match o {
                        Obj::Index { .. } => o.key(),
                        Obj::Constraint { name, .. } => format!("constraint:{t}:{name}"),
                        Obj::GenCol { name, .. } => format!("column:{t}:{name}"),
                    },
                    (),
                )
            })
            .collect();
        for n in &nc.objects {
            let key = n.key();
            if old_keys.contains_key(&key) && !stale.contains(&key) {
                continue;
            }
            let fresh_table = created_tables.contains(n.table());
            // the inline constraints of a table created in this plan come with its `CREATE TABLE`
            if matches!(n, Obj::Constraint { inline: true, .. }) && fresh_table {
                continue;
            }
            self.objects.push(self.create_object(n, fresh_table));
        }
    }

    fn create_object(&self, o: &Obj, fresh: bool) -> EvolveStep {
        match o {
            Obj::GenCol { name, table, sql } => {
                step(sql.clone(), StepClass::Safe, format!("derived column {table}.{name} is added (the table is rewritten once)"))
            }
            Obj::Index { name, table, unique, sql } => {
                if !*unique {
                    return step(
                        sql.clone(),
                        StepClass::Safe,
                        format!("index {name} on {table} (built inside the transaction: writers wait; CREATE INDEX CONCURRENTLY cannot run in one)"),
                    );
                }
                let mut s = step(sql.clone(), if fresh { StepClass::Safe } else { StepClass::Checked }, format!("unique rule {name} on {table}"));
                if !fresh {
                    let (cols, wher) = parse_index(sql);
                    let part = cols.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ");
                    let notnull = cols.iter().map(|c| format!("{} IS NOT NULL", q(c))).collect::<Vec<_>>().join(" AND ");
                    let w = wher.map(|w| format!(" AND ({w})")).unwrap_or_default();
                    let inner =
                        |select: &str| format!("SELECT {select}, count(*) OVER (PARTITION BY {part}) AS n FROM {} WHERE {notnull}{w}", q(table));
                    s.checks.push(DataCheck {
                        what: format!("rows of {table} share values of ({}), which the new unique rule forbids", cols.join(", ")),
                        count_sql: format!("SELECT count(*) FROM ({}) x WHERE n > 1", inner("1")),
                        sample_sql: format!("SELECT \"id\"::text FROM ({}) x WHERE n > 1 ORDER BY \"id\" LIMIT 5", inner("\"id\"")),
                    });
                }
                s
            }
            Obj::Constraint { name, table, sql, .. } => {
                if let Some(i) = sql.find(" CHECK (") {
                    let mut s = step(sql.clone(), if fresh { StepClass::Safe } else { StepClass::Checked }, format!("rule {name} on {table}"));
                    if !fresh {
                        let expr = sql[i + 7..].trim();
                        s.checks.push(check(format!("rows of {table} break the new rule {name}"), table, &format!("NOT {expr}")));
                    }
                    s
                } else if sql.contains(" EXCLUDE ") {
                    step(
                        sql.clone(),
                        if fresh { StepClass::Safe } else { StepClass::Checked },
                        format!("no-overlap rule {name} on {table} (the database refuses it if rows already overlap)"),
                    )
                } else {
                    step(sql.clone(), StepClass::Safe, format!("constraint {name} on {table}"))
                }
            }
        }
    }

    // ---- putting it together ----

    fn assemble(&mut self, oc: &Catalog, nc: &Catalog) {
        let mut steps = Vec::new();
        if !nc.idempotent.is_empty() {
            steps.push(step(
                nc.idempotent.join(";\n"),
                StepClass::Safe,
                format!("extensions and internal tables ({} statements, all IF NOT EXISTS)", nc.idempotent.len()),
            ));
        }
        steps.append(&mut self.renames);
        steps.append(&mut self.drops);
        steps.append(&mut self.creates);
        steps.append(&mut self.alters);
        steps.append(&mut self.dropped);
        steps.append(&mut self.objects);
        let old_other: BTreeSet<&String> = oc.other.iter().collect();
        for o in nc.other.iter().filter(|o| !old_other.contains(o)) {
            steps.push(step(o.clone(), StepClass::Safe, "statement of the generator that is new in this program"));
        }
        let trig_names: Vec<String> = nc.routines.iter().flat_map(|r| r.triggers.iter().map(|t| format!("'{}'", t.0))).collect();
        let fn_names: Vec<String> = nc.routines.iter().flat_map(|r| r.functions.iter().map(|f| format!("'{f}'"))).collect();
        steps.push(step(
            format!(
                "DO $aip$ DECLARE r record; BEGIN FOR r IN SELECT t.tgname, c.relname FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE NOT t.tgisinternal AND n.nspname = 'public' AND t.tgname LIKE '%\\_\\_%' AND t.tgname NOT IN ({}) \
                 LOOP EXECUTE format('DROP TRIGGER IF EXISTS %I ON %I', r.tgname, r.relname); END LOOP; END $aip$;\n\
                 DO $aip$ DECLARE r record; BEGIN FOR r IN SELECT p.oid::regprocedure AS sig FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
                 WHERE n.nspname = 'public' AND p.proname LIKE '%\\_\\_%\\_fn' AND p.proname NOT IN ({}) \
                 LOOP EXECUTE format('DROP FUNCTION IF EXISTS %s', r.sig); END LOOP; END $aip$",
                if trig_names.is_empty() { "''".to_string() } else { trig_names.join(", ") },
                if fn_names.is_empty() { "''".to_string() } else { fn_names.join(", ") },
            ),
            StepClass::Refresh,
            "drop generated triggers and functions the program no longer has",
        ));
        let mut refresh = Vec::new();
        for r in &nc.routines {
            for (tg, table) in &r.triggers {
                refresh.push(format!("DROP TRIGGER IF EXISTS {} ON {}", q(tg), q(table)));
            }
            refresh.push(r.sql.clone());
        }
        if !refresh.is_empty() {
            steps.push(step(
                refresh.join(";\n"),
                StepClass::Refresh,
                format!("{} generated function/trigger statements replaced with their current text", nc.routines.len()),
            ));
        }
        self.out.steps = steps;
    }
}

fn describe(from: &Type, to: &Type) -> String {
    let f = |t: &Type| serde_json::to_string(t).unwrap_or_default();
    format!("{} -> {}", f(from), f(to))
}
