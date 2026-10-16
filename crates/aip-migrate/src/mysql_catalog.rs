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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub ordinal_position: u64,
    pub logical_type: LogicalType,
    pub nullable: bool,
    pub default: Option<String>,
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
    for row in columns {
        if row.name.is_empty() || row.ordinal_position == 0 || seen_columns.insert(row.name.clone(), ()).is_some() {
            return Err(CatalogError(format!("invalid or duplicate column metadata: {}", row.name)));
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
        normalized.push(Column {
            name: row.name,
            ordinal_position: row.ordinal_position,
            logical_type,
            nullable,
            default: row.column_default,
            generated,
        });
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
        let primary = index_name == "PRIMARY";
        let unique = primary || members.iter().all(|r| !r.non_unique);
        let mut index_columns = Vec::with_capacity(members.len());
        for member in members {
            if let Some(expression) = member.expression.filter(|s| !s.is_empty()) {
                diagnostics
                    .push(Diagnostic { field: index_name.clone(), reason: format!("functional index expression is not represented: {expression}") });
            } else if let Some(column) = member.column_name.filter(|s| !s.is_empty()) {
                index_columns.push(column);
            } else {
                return Err(CatalogError(format!("index {index_name} member has neither column nor expression")));
            }
        }
        indexes.push(Index { name: index_name, unique, primary, columns: index_columns });
    }
    Ok(TableManifest { name: name.into(), columns: normalized, indexes, diagnostics })
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
