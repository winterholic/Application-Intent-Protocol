//! Physical mapping of entities to PostgreSQL tables and columns.

use crate::names::snake;
use crate::ty::Ty;
use aip_ir as ir;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub enum Col {
    Scalar { col: String, ty: Ty },
    Ref { col: String, target: String },
    Union { type_col: String, id_col: String, targets: Vec<String> },
    Inverse { target: String, via_col: String },
    Counter { col: String },
    Snapshot { id_col: String, ver_col: String, target: String },
}

#[derive(Debug, Clone)]
pub struct Table {
    pub entity: String,
    pub table: String,
    /// Versions of a `history` entity; stays with the working table when [`Schema::published`] renames `table`.
    pub history_table: String,
    pub cols: BTreeMap<String, Col>,
    pub soft_delete: bool,
    pub history: bool,
    /// `publishable`: the table holds the working (draft) version and `published_table` the one clients read.
    pub published: bool,
    pub rank: usize,
}

/// Name of the snapshot table that holds the published version of `table`'s rows.
pub fn published_table(table: &str) -> String {
    format!("{table}_published")
}

impl Table {
    pub fn col(&self, field: &str) -> Option<&Col> {
        self.cols.get(field)
    }
}

/// How a `search` is kept in the database: one generated `tsvector` column on the entity's table, with a GIN index.
#[derive(Debug, Clone)]
pub struct SearchCfg {
    pub name: String,
    pub entity: String,
    /// The generated column holding the weighted document.
    pub column: String,
    /// Text search configuration the document and the query are analyzed with.
    pub config: String,
    /// Query words match the beginning of document words (languages without a stemmer, see `aip_ir::builtin::search_is_approximate`).
    pub prefix: bool,
    /// The document as an expression over the table's own columns (the generated column's definition).
    pub document: String,
}

pub struct Schema {
    pub tables: BTreeMap<String, Table>,
    pub searches: BTreeMap<String, SearchCfg>,
}

impl Schema {
    pub fn table(&self, entity: &str) -> &Table {
        self.tables.get(entity).unwrap_or_else(|| panic!("no table for entity {entity}"))
    }

    /// The same tables as clients see them: a `publishable` entity reads from its published snapshot table,
    /// which has the same columns. Everything that is compiled against this schema (a query's source, the rows
    /// it reaches through references, its parameter loads) sees the published version only.
    pub fn published(&self) -> Schema {
        let mut s = Schema { tables: self.tables.clone(), searches: self.searches.clone() };
        for t in s.tables.values_mut() {
            if t.published {
                t.table = published_table(&t.table);
            }
        }
        s
    }

    pub fn build(core: &ir::Program) -> Schema {
        let mut tables = BTreeMap::new();
        // rank = position in a deterministic order: referenced tables before referrers where possible
        let order = rank_entities(core);
        for (rank, name) in order.iter().enumerate() {
            let info = &core.entities[name];
            let mut cols = BTreeMap::new();
            for f in &info.fields {
                let ty = Ty::from_ir(&f.ty);
                let col = match (&f.kind, &ty) {
                    (ir::FieldKind::Ref { target, .. }, _) => Col::Ref { col: format!("{}_id", snake(&f.name)), target: target.clone() },
                    (ir::FieldKind::RefUnion { targets, .. }, _) => Col::Union {
                        type_col: format!("{}_type", snake(&f.name)),
                        id_col: format!("{}_id", snake(&f.name)),
                        targets: targets.clone(),
                    },
                    (ir::FieldKind::Inverse { target, via }, _) => Col::Inverse { target: target.clone(), via_col: format!("{}_id", snake(via)) },
                    (ir::FieldKind::Counter { .. }, _) => Col::Counter { col: snake(&f.name) },
                    (_, Ty::Snapshot(t)) => {
                        Col::Snapshot { id_col: format!("{}_id", snake(&f.name)), ver_col: format!("{}_version", snake(&f.name)), target: t.clone() }
                    }
                    (_, Ty::Entity(t)) => Col::Ref { col: format!("{}_id", snake(&f.name)), target: t.clone() },
                    _ => Col::Scalar { col: snake(&f.name), ty },
                };
                cols.insert(f.name.clone(), col);
            }
            tables.insert(
                name.clone(),
                Table {
                    entity: name.clone(),
                    table: snake(name),
                    history_table: format!("{}_history", snake(name)),
                    cols,
                    soft_delete: info.traits.soft_delete.is_some(),
                    history: info.traits.history,
                    published: info.traits.publishable,
                    rank,
                },
            );
        }
        let searches = core
            .forms
            .iter()
            .filter_map(|f| match f {
                ir::Form::Search(x) => Some(x),
                _ => None,
            })
            .filter_map(|x| {
                let t = tables.get(&x.entity)?;
                // `korean` has no stemmer in the engine: the plain configuration plus prefix matching stands in for it
                let config = match x.language.as_deref() {
                    None | Some("korean") => "simple".to_string(),
                    Some(l) => l.to_string(),
                };
                let parts: Vec<String> = x
                    .fields
                    .iter()
                    .filter_map(|(field, weight)| {
                        let Col::Scalar { col, .. } = t.col(field)? else { return None };
                        let w = weight.as_deref().unwrap_or("D");
                        Some(format!("setweight(to_tsvector('{config}'::regconfig, coalesce(\"{col}\"::text, '')), '{w}')"))
                    })
                    .collect();
                let cfg = SearchCfg {
                    name: x.name.clone(),
                    entity: x.entity.clone(),
                    column: format!("__search_{}", snake(&x.name)),
                    prefix: ir::builtin::search_is_approximate(x.language.as_deref()),
                    config,
                    document: parts.join(" || "),
                };
                Some((x.name.clone(), cfg))
            })
            .collect();
        Schema { tables, searches }
    }
}

/// Deterministic global order used for DDL and for lock acquisition.
fn rank_entities(core: &ir::Program) -> Vec<String> {
    let names: Vec<String> = core.entities.keys().cloned().collect();
    let mut placed: Vec<String> = Vec::new();
    let mut remaining = names.clone();
    while !remaining.is_empty() {
        let before = remaining.len();
        remaining.retain(|n| {
            let deps: Vec<String> = core.entities[n]
                .fields
                .iter()
                .filter_map(|f| match &f.kind {
                    ir::FieldKind::Ref { target, .. } if target != n => Some(target.clone()),
                    _ => None,
                })
                .collect();
            if deps.iter().all(|d| placed.contains(d)) {
                placed.push(n.clone());
                false
            } else {
                true
            }
        });
        if remaining.len() == before {
            // reference cycle: break it alphabetically
            let first = remaining.remove(0);
            placed.push(first);
        }
    }
    placed
}

/// Column type for a semantic type.
pub fn sql_type(ty: &Ty) -> &'static str {
    match ty {
        Ty::Bool => "boolean",
        Ty::Int | Ty::Size => "bigint",
        Ty::Decimal | Ty::Money(_) => "numeric",
        Ty::Text | Ty::RichText | Ty::Email | Ty::Url | Ty::Phone | Ty::Enum(_) | Ty::Object | Ty::Recurrence => "text",
        Ty::Time => "timestamptz",
        Ty::Date => "date",
        Ty::Duration => "interval",
        Ty::Uuid | Ty::Entity(_) => "uuid",
        Ty::Range(inner) => match inner.as_ref() {
            Ty::Date => "daterange",
            Ty::Int => "int8range",
            Ty::Decimal | Ty::Money(_) => "numrange",
            _ => "tstzrange",
        },
        _ => "jsonb",
    }
}

/// Cast suffix for a parameter bound as text.
pub fn cast(ty: &Ty) -> &'static str {
    sql_type(ty)
}
