//! Driver-independent normalization of MySQL INFORMATION_SCHEMA table metadata.
//!
//! This module consumes catalog rows supplied by a future adapter. It does not connect to
//! MySQL, claim database observations, or make unsupported metadata disappear.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnRow {
    pub name: String,
    pub ordinal_position: u64,
    pub data_type: String,
    pub column_type: String,
    /// INFORMATION_SCHEMA.COLUMNS.IS_NULLABLE (`YES` or `NO`).
    pub is_nullable: String,
    /// Kept verbatim because catalog defaults are SQL expressions or literals.
    pub column_default: Option<String>,
    pub extra: String,
    pub generation_expression: String,
    pub collation_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexRow {
    pub index_name: String,
    pub non_unique: bool,
    pub seq_in_index: u64,
    pub column_name: Option<String>,
    pub expression: Option<String>,
}

/// Joined KEY_COLUMN_USAGE and REFERENTIAL_CONSTRAINTS data supplied by a future driver.
/// A row represents one ordered column pair from one foreign-key constraint.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForeignKeyRow {
    pub constraint_schema: String,
    pub table_name: String,
    pub constraint_name: String,
    pub ordinal_position: u64,
    pub column_name: String,
    pub referenced_table_schema: Option<String>,
    pub referenced_table_name: Option<String>,
    pub referenced_column_name: Option<String>,
    pub delete_rule: Option<String>,
    pub update_rule: Option<String>,
}

impl IndexRow {
    pub fn column(index_name: &str, non_unique: u8, seq_in_index: u64, column_name: &str) -> Self {
        Self { index_name: index_name.into(), non_unique: non_unique != 0, seq_in_index, column_name: Some(column_name.into()), expression: None }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogicalType {
    Integer { bits: u8, unsigned: bool, display_width: Option<u16>, boolean_alias: bool },
    Enum(Vec<String>),
    Text,
    Unsupported(String),
}

/// INFORMATION_SCHEMA.COLUMNS cannot distinguish explicit DEFAULT NULL from no DEFAULT clause.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CatalogDefault {
    NullOrAbsent,
    Value(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub ordinal_position: u64,
    pub logical_type: LogicalType,
    pub nullable: bool,
    pub default: CatalogDefault,
    pub generated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Index {
    pub name: String,
    pub unique: bool,
    pub primary: bool,
    /// Ordered by STATISTICS.SEQ_IN_INDEX, never alphabetically.
    pub columns: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReferentialAction {
    #[serde(rename = "CASCADE")]
    Cascade,
    #[serde(rename = "SET NULL")]
    SetNull,
    #[serde(rename = "RESTRICT")]
    Restrict,
    #[serde(rename = "NO ACTION")]
    NoAction,
}

impl fmt::Display for ReferentialAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cascade => "CASCADE",
            Self::SetNull => "SET NULL",
            Self::Restrict => "RESTRICT",
            Self::NoAction => "NO ACTION",
        })
    }
}

/// A normalized foreign key, retaining its database constraint identity and column pair order.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForeignKey {
    pub source_schema: String,
    pub source_table: String,
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_schema: String,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
    pub on_delete: ReferentialAction,
    pub on_update: ReferentialAction,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub field: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableManifest {
    pub name: String,
    pub columns: Vec<Column>,
    pub indexes: Vec<Index>,
    /// Details relevant to safety but not represented by the neutral manifest shape.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogError(String);

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CatalogError {}

/// Normalize already-read catalog rows. Synthetic test rows are not database evidence.
pub fn normalize_table(name: &str, mut columns: Vec<ColumnRow>, rows: Vec<IndexRow>) -> Result<TableManifest, CatalogError> {
    columns.sort_by_key(|c| c.ordinal_position);
    let mut diagnostics = Vec::new();
    let mut seen_columns = BTreeMap::new();
    let mut normalized = Vec::with_capacity(columns.len());
    for (index, row) in columns.into_iter().enumerate() {
        if row.name.is_empty() || row.ordinal_position == 0 || seen_columns.insert(row.name.clone(), ()).is_some() {
            return Err(CatalogError(format!("invalid or duplicate column metadata: {}", row.name)));
        }
        if row.ordinal_position != index as u64 + 1 {
            return Err(CatalogError(format!("non-contiguous or duplicate column ordinal position: {}", row.ordinal_position)));
        }
        let nullable = match row.is_nullable.as_str() {
            "YES" => true,
            "NO" => false,
            _ => return Err(CatalogError(format!("invalid nullable metadata for {}", row.name))),
        };
        let (logical_type, type_diagnostics) = logical_type(&row.data_type, &row.column_type);
        for reason in type_diagnostics {
            diagnostics.push(Diagnostic { field: row.name.clone(), reason });
        }
        if matches!(row.data_type.to_ascii_lowercase().as_str(), "char" | "varchar") {
            diagnostics.push(Diagnostic {
                field: row.name.clone(),
                reason: format!("{} length is not represented by the text manifest", row.data_type.to_ascii_lowercase()),
            });
        }
        let generated = !row.generation_expression.is_empty()
            || row.extra.to_ascii_lowercase().contains("generated")
            || row.extra.to_ascii_lowercase().contains("auto_increment");
        if !row.generation_expression.is_empty() {
            diagnostics.push(Diagnostic { field: row.name.clone(), reason: "generation expression is not represented".into() });
        }
        let lower_extra = row.extra.to_ascii_lowercase();
        if lower_extra.contains("on update") {
            diagnostics.push(Diagnostic { field: row.name.clone(), reason: "on update behavior is not represented".into() });
        }
        if lower_extra.contains("auto_increment") {
            diagnostics.push(Diagnostic { field: row.name.clone(), reason: "auto_increment strategy is not represented".into() });
        }
        if row.collation_name.is_some() {
            diagnostics.push(Diagnostic { field: row.name.clone(), reason: "column collation is not represented".into() });
        }
        let default = match row.column_default {
            Some(value) => CatalogDefault::Value(value),
            None => {
                diagnostics.push(Diagnostic {
                    field: row.name.clone(),
                    reason: "INFORMATION_SCHEMA COLUMN_DEFAULT NULL cannot distinguish explicit DEFAULT NULL from no DEFAULT clause".into(),
                });
                CatalogDefault::NullOrAbsent
            }
        };
        normalized.push(Column { name: row.name, ordinal_position: row.ordinal_position, logical_type, nullable, default, generated });
    }

    let mut grouped: BTreeMap<String, Vec<IndexRow>> = BTreeMap::new();
    for row in rows {
        if row.index_name.is_empty() || row.seq_in_index == 0 {
            return Err(CatalogError("invalid index metadata".into()));
        }
        grouped.entry(row.index_name.clone()).or_default().push(row);
    }
    let mut indexes = Vec::with_capacity(grouped.len());
    for (index_name, mut members) in grouped {
        members.sort_by_key(|r| r.seq_in_index);
        if members.iter().enumerate().any(|(i, r)| r.seq_in_index != i as u64 + 1) {
            return Err(CatalogError(format!("non-contiguous sequence for index {index_name}")));
        }
        let non_unique = members[0].non_unique;
        if members.iter().any(|r| r.non_unique != non_unique) {
            return Err(CatalogError(format!("inconsistent non_unique metadata for index {index_name}")));
        }
        let primary = index_name == "PRIMARY";
        let unique = primary || members.iter().all(|r| !r.non_unique);
        let mut index_columns = Vec::with_capacity(members.len());
        for member in members {
            if let Some(expression) = member.expression.filter(|s| !s.is_empty()) {
                diagnostics
                    .push(Diagnostic { field: index_name.clone(), reason: format!("functional index expression is not represented: {expression}") });
            } else if let Some(column) = member.column_name.filter(|s| !s.is_empty()) {
                if !seen_columns.contains_key(&column) {
                    return Err(CatalogError(format!("index {index_name} references missing column {column}")));
                }
                index_columns.push(column);
            } else {
                return Err(CatalogError(format!("index {index_name} member has neither column nor expression")));
            }
        }
        indexes.push(Index { name: index_name, unique, primary, columns: index_columns });
    }
    Ok(TableManifest { name: name.into(), columns: normalized, indexes, diagnostics })
}

/// Normalize joined MySQL catalog rows for a single source table.
/// This is an input/manifest boundary for a future driver; it executes no query.
pub fn normalize_foreign_keys(
    constraint_schema: &str,
    table_name: &str,
    source_columns: &[String],
    rows: Vec<ForeignKeyRow>,
) -> Result<Vec<ForeignKey>, CatalogError> {
    let available_columns = source_columns.iter().map(String::as_str).collect::<std::collections::BTreeSet<_>>();
    let mut grouped: BTreeMap<(String, String, String), Vec<ForeignKeyRow>> = BTreeMap::new();
    for row in rows {
        if row.constraint_schema != constraint_schema || row.table_name != table_name {
            return Err(CatalogError("foreign-key row does not belong to requested source table".into()));
        }
        if row.constraint_name.is_empty() || row.ordinal_position == 0 || row.column_name.is_empty() {
            return Err(CatalogError("invalid foreign-key constraint identity, sequence, or source column".into()));
        }
        grouped.entry((row.constraint_schema.clone(), row.table_name.clone(), row.constraint_name.clone())).or_default().push(row);
    }

    let mut keys = Vec::with_capacity(grouped.len());
    for ((_, _, name), mut members) in grouped {
        members.sort_by_key(|row| row.ordinal_position);
        if members.iter().enumerate().any(|(i, row)| row.ordinal_position != i as u64 + 1) {
            return Err(CatalogError(format!("non-contiguous sequence for foreign key {name}")));
        }
        let first = &members[0];
        let referenced_schema = required_fk_value(first.referenced_table_schema.as_deref(), &name, "referenced schema")?.to_owned();
        let referenced_table = required_fk_value(first.referenced_table_name.as_deref(), &name, "referenced table")?.to_owned();
        let on_delete = parse_action(first.delete_rule.as_deref(), &name, "delete")?;
        let on_update = parse_action(first.update_rule.as_deref(), &name, "update")?;
        let mut columns = Vec::with_capacity(members.len());
        let mut referenced_columns = Vec::with_capacity(members.len());
        let mut seen_source = std::collections::BTreeSet::new();
        let mut seen_referenced = std::collections::BTreeSet::new();
        for row in members {
            if row.referenced_table_schema.as_deref() != Some(referenced_schema.as_str())
                || row.referenced_table_name.as_deref() != Some(referenced_table.as_str())
                || parse_action(row.delete_rule.as_deref(), &name, "delete")? != on_delete
                || parse_action(row.update_rule.as_deref(), &name, "update")? != on_update
            {
                return Err(CatalogError(format!("inconsistent target or action metadata for foreign key {name}")));
            }
            let referenced_column = required_fk_value(row.referenced_column_name.as_deref(), &name, "referenced column")?;
            if !available_columns.contains(row.column_name.as_str()) {
                return Err(CatalogError(format!("foreign key {name} references missing source column {}", row.column_name)));
            }
            if !seen_source.insert(row.column_name.clone()) || !seen_referenced.insert(referenced_column.to_owned()) {
                return Err(CatalogError(format!("duplicate column pair in foreign key {name}")));
            }
            columns.push(row.column_name);
            referenced_columns.push(referenced_column.to_owned());
        }
        keys.push(ForeignKey {
            source_schema: constraint_schema.into(),
            source_table: table_name.into(),
            name,
            columns,
            referenced_schema,
            referenced_table,
            referenced_columns,
            on_delete,
            on_update,
        });
    }
    Ok(keys)
}

fn required_fk_value<'a>(value: Option<&'a str>, name: &str, field: &str) -> Result<&'a str, CatalogError> {
    value.filter(|value| !value.is_empty()).ok_or_else(|| CatalogError(format!("missing {field} for foreign key {name}")))
}

fn parse_action(value: Option<&str>, name: &str, operation: &str) -> Result<ReferentialAction, CatalogError> {
    match value.map(str::trim).map(str::to_ascii_uppercase).as_deref() {
        Some("CASCADE") => Ok(ReferentialAction::Cascade),
        Some("SET NULL") => Ok(ReferentialAction::SetNull),
        Some("RESTRICT") => Ok(ReferentialAction::Restrict),
        Some("NO ACTION") => Ok(ReferentialAction::NoAction),
        Some("SET DEFAULT") => Err(CatalogError(format!("unsupported {operation} rule SET DEFAULT for foreign key {name}"))),
        _ => Err(CatalogError(format!("unsupported {operation} rule for foreign key {name}"))),
    }
}

fn logical_type(data_type: &str, column_type: &str) -> (LogicalType, Vec<String>) {
    let lower = column_type.to_ascii_lowercase();
    let unsigned = lower.split_whitespace().any(|part| part == "unsigned");
    let type_head = lower.split_whitespace().next().unwrap_or_default();
    let display_width = type_head.split_once('(').and_then(|(_, width)| width.strip_suffix(')')).and_then(|width| width.parse::<u16>().ok());
    let integer = match data_type.to_ascii_lowercase().as_str() {
        "tinyint" => Some(8),
        "smallint" => Some(16),
        "mediumint" => Some(24),
        "int" | "integer" => Some(32),
        "bigint" => Some(64),
        _ => None,
    };
    if let Some(bits) = integer {
        let boolean_alias = bits == 8 && display_width == Some(1);
        let diagnostics = if boolean_alias { vec!["tinyint(1) is a display-width alias and is not constrained to 0/1".into()] } else { Vec::new() };
        return (LogicalType::Integer { bits, unsigned, display_width, boolean_alias }, diagnostics);
    }
    match data_type.to_ascii_lowercase().as_str() {
        "enum" => match parse_enum(column_type) {
            Some(values) => (LogicalType::Enum(values), Vec::new()),
            None => (LogicalType::Unsupported(column_type.into()), vec!["malformed enum declaration is unsupported".into()]),
        },
        "char" | "varchar" | "tinytext" | "text" | "mediumtext" | "longtext" => (LogicalType::Text, Vec::new()),
        _ => (LogicalType::Unsupported(column_type.into()), vec![format!("unsupported MySQL type: {column_type}")]),
    }
}

fn parse_enum(column_type: &str) -> Option<Vec<String>> {
    let mut input = column_type.strip_prefix("enum(")?.strip_suffix(')')?.chars().peekable();
    let mut values = Vec::new();
    loop {
        if input.next()? != '\'' {
            return None;
        }
        let mut value = String::new();
        loop {
            match input.next()? {
                '\\' => value.push(input.next()?),
                '\'' if input.peek() == Some(&'\'') => {
                    input.next();
                    value.push('\'');
                }
                '\'' => break,
                ch => value.push(ch),
            }
        }
        values.push(value);
        match input.next() {
            None => break,
            Some(',') => {}
            Some(_) => return None,
        }
    }
    (!values.is_empty()).then_some(values)
}
