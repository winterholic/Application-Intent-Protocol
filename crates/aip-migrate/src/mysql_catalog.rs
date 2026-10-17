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

/// Standard INFORMATION_SCHEMA SQL queries according to MySQL 8.4 reference manual.
pub const SQL_TABLES: &str = "\
SELECT TABLE_NAME \
FROM INFORMATION_SCHEMA.TABLES \
WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'BASE TABLE' \
ORDER BY TABLE_NAME ASC";

pub const SQL_COLUMNS: &str = "\
SELECT COLUMN_NAME, ORDINAL_POSITION, DATA_TYPE, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, EXTRA, GENERATION_EXPRESSION, COLLATION_NAME \
FROM INFORMATION_SCHEMA.COLUMNS \
WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
ORDER BY ORDINAL_POSITION ASC";

pub const SQL_STATISTICS: &str = "\
SELECT INDEX_NAME, NON_UNIQUE, SEQ_IN_INDEX, COLUMN_NAME, EXPRESSION \
FROM INFORMATION_SCHEMA.STATISTICS \
WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
ORDER BY INDEX_NAME ASC, SEQ_IN_INDEX ASC";

pub const SQL_FOREIGN_KEYS: &str = "\
SELECT k.CONSTRAINT_SCHEMA, k.TABLE_NAME, k.CONSTRAINT_NAME, k.ORDINAL_POSITION, k.COLUMN_NAME, \
       k.REFERENCED_TABLE_SCHEMA, k.REFERENCED_TABLE_NAME, k.REFERENCED_COLUMN_NAME, \
       r.DELETE_RULE, r.UPDATE_RULE \
FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE k \
JOIN INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS r \
  ON k.CONSTRAINT_SCHEMA = r.CONSTRAINT_SCHEMA \
 AND k.CONSTRAINT_NAME = r.CONSTRAINT_NAME \
 AND k.TABLE_NAME = r.TABLE_NAME \
WHERE k.CONSTRAINT_SCHEMA = ? AND k.TABLE_NAME = ? AND k.REFERENCED_TABLE_NAME IS NOT NULL \
ORDER BY k.CONSTRAINT_NAME ASC, k.ORDINAL_POSITION ASC";

/// Contract for asynchronous MySQL catalog row retrieval.
/// Implementations may be a mock for testing or an actual live driver connection pool.
pub trait MysqlCatalogReader: Send + Sync {
    fn fetch_tables(&self, schema: &str) -> impl std::future::Future<Output = Result<Vec<String>, CatalogError>> + Send;
    fn fetch_columns(&self, schema: &str, table: &str) -> impl std::future::Future<Output = Result<Vec<ColumnRow>, CatalogError>> + Send;
    fn fetch_indexes(&self, schema: &str, table: &str) -> impl std::future::Future<Output = Result<Vec<IndexRow>, CatalogError>> + Send;
    fn fetch_foreign_keys(&self, schema: &str, table: &str) -> impl std::future::Future<Output = Result<Vec<ForeignKeyRow>, CatalogError>> + Send;
}

/// Introspect a single table by fetching catalog rows and normalizing them into manifests.
pub async fn introspect_table(
    reader: &impl MysqlCatalogReader,
    schema: &str,
    table: &str,
) -> Result<(TableManifest, Vec<ForeignKey>), CatalogError> {
    let columns = reader.fetch_columns(schema, table).await?;
    let indexes = reader.fetch_indexes(schema, table).await?;
    let source_columns: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let manifest = normalize_table(table, columns, indexes)?;
    let fk_rows = reader.fetch_foreign_keys(schema, table).await?;
    let foreign_keys = normalize_foreign_keys(schema, table, &source_columns, fk_rows)?;
    Ok((manifest, foreign_keys))
}

/// Introspect an entire database schema by fetching all tables and normalizing their manifests.
pub async fn introspect_schema(
    reader: &impl MysqlCatalogReader,
    schema: &str,
) -> Result<BTreeMap<String, (TableManifest, Vec<ForeignKey>)>, CatalogError> {
    let tables = reader.fetch_tables(schema).await?;
    let mut results = BTreeMap::new();
    for table in tables {
        let (manifest, foreign_keys) = introspect_table(reader, schema, &table).await?;
        results.insert(table, (manifest, foreign_keys));
    }
    Ok(results)
}

/// Decode percent-encoded characters in URL components (e.g. %40 -> @).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..=i + 2]).unwrap_or(""), 16) {
                output.push(byte);
                i += 3;
                continue;
            }
        }
        output.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

/// Configuration and sanitization for MySQL driver connections.
#[derive(Clone, PartialEq, Eq)]
pub struct MysqlDriverConfig {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: Option<String>,
    pub connect_timeout_ms: u64,
    pub statement_timeout_ms: u64,
    pub pool_min: usize,
    pub pool_max: usize,
    pub tls_required: bool,
}

impl fmt::Debug for MysqlDriverConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MysqlDriverConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("statement_timeout_ms", &self.statement_timeout_ms)
            .field("pool_min", &self.pool_min)
            .field("pool_max", &self.pool_max)
            .field("tls_required", &self.tls_required)
            .finish()
    }
}

impl MysqlDriverConfig {
    pub fn parse_url(url: &str) -> Result<Self, CatalogError> {
        let (scheme, rest) = url.split_once("://").ok_or_else(|| CatalogError("invalid database URL scheme".into()))?;
        let s = scheme.to_ascii_lowercase();
        if s != "mysql" && s != "mariadb" {
            return Err(CatalogError(format!("expected mysql or mariadb scheme, found {scheme}")));
        }

        let (authority, path_query) = rest.split_once('/').unwrap_or((rest, ""));
        let (user_info, host_port) = authority.rsplit_once('@').map_or(("", authority), |(u, h)| (u, h));
        let (user, password) = if user_info.is_empty() {
            ("root".to_string(), None)
        } else if let Some((u, p)) = user_info.split_once(':') {
            (percent_decode(u), Some(percent_decode(p)))
        } else {
            (percent_decode(user_info), None)
        };

        let (host, port) = if host_port.starts_with('[') {
            if let Some(bracket_end) = host_port.find(']') {
                let h = &host_port[0..=bracket_end];
                let rest = &host_port[bracket_end + 1..];
                if rest.is_empty() {
                    (h.to_string(), 3306)
                } else if let Some(p_str) = rest.strip_prefix(':') {
                    let parsed_port = p_str.parse::<u16>().map_err(|_| CatalogError("invalid port in database URL".into()))?;
                    (h.to_string(), parsed_port)
                } else {
                    return Err(CatalogError("invalid IPv6 host in database URL".into()));
                }
            } else {
                return Err(CatalogError("unclosed IPv6 bracket in database URL".into()));
            }
        } else if let Some((h, p)) = host_port.split_once(':') {
            let parsed_port = p.parse::<u16>().map_err(|_| CatalogError("invalid port in database URL".into()))?;
            (h.to_string(), parsed_port)
        } else {
            (if host_port.is_empty() { "127.0.0.1".to_string() } else { host_port.to_string() }, 3306)
        };

        let (database_raw, query) = match path_query.split_once('?') {
            Some((db, q)) => (db, Some(q)),
            None => (path_query.split('#').next().unwrap_or(""), None),
        };
        let database = database_raw.trim_matches('/').to_string();
        if database.is_empty() {
            return Err(CatalogError("database name is required in database URL".into()));
        }

        let mut tls_required = false;
        let mut connect_timeout_ms = 5_000;
        let mut statement_timeout_ms = 10_000;
        let pool_min = 1;
        let pool_max = 10;

        if let Some(q) = query {
            for pair in q.split('&') {
                if let Some((k, v)) = pair.split_once('=') {
                    let key = k.trim().to_ascii_lowercase();
                    let val = v.trim().to_ascii_lowercase();
                    if key == "sslmode" || key == "ssl-mode" {
                        if matches!(val.as_str(), "required" | "verify-ca" | "verify-full" | "require") {
                            tls_required = true;
                        }
                    } else if key == "connect_timeout" || key == "connect-timeout" {
                        if let Ok(secs) = val.parse::<u64>() {
                            connect_timeout_ms = secs * 1000;
                        }
                    } else if key == "statement_timeout" || key == "statement-timeout" {
                        if let Ok(secs) = val.parse::<u64>() {
                            statement_timeout_ms = secs * 1000;
                        }
                    }
                }
            }
        }

        Ok(Self {
            host,
            port,
            database,
            user,
            password,
            connect_timeout_ms,
            statement_timeout_ms,
            pool_min,
            pool_max,
            tls_required,
        })
    }

    /// Redacted representation safe for error messages and logging.
    pub fn display_sanitized(&self) -> String {
        let auth = if self.password.is_some() {
            format!("{}:***@", self.user)
        } else {
            format!("{}@", self.user)
        };
        format!("mysql://{}{}:{}/{}", auth, self.host, self.port, self.database)
    }
}

/// Normalized database errors mapping MySQL server errors into standard AIP codes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedDriverError {
    pub code: &'static str,
    pub message: String,
}

/// Sanitize raw error messages by redacting passwords in URLs and credentials.
pub fn sanitize_error_message(msg: &str) -> String {
    let mut sanitized = if msg.contains("Access denied for user") {
        "access denied for user (credentials rejected)".to_string()
    } else {
        msg.to_string()
    };

    if let Some(scheme_idx) = sanitized.find("://") {
        let after_scheme = &sanitized[scheme_idx + 3..];
        if let Some(at_idx) = after_scheme.find('@') {
            let user_pass = &after_scheme[..at_idx];
            if let Some(colon_idx) = user_pass.find(':') {
                let prefix = &sanitized[..scheme_idx + 3 + colon_idx + 1];
                let suffix = &after_scheme[at_idx..];
                sanitized = format!("{prefix}***{suffix}");
            }
        }
    }
    sanitized
}

pub fn normalize_mysql_error_code(error_code: u16, raw_message: &str) -> NormalizedDriverError {
    let sanitized = sanitize_error_message(raw_message);

    let code = match error_code {
        1045 => "DB_AUTH_FAILED",
        1049 => "DB_NOT_FOUND",
        1146 => "TABLE_NOT_FOUND",
        1062 => "UNIQUE_VIOLATION",
        1213 => "DEADLOCK_DETECTED",
        1205 => "LOCK_TIMEOUT",
        1451 | 1452 => "FOREIGN_KEY_VIOLATION",
        2002 | 2003 => "DB_CONNECT_FAILED",
        _ => "DB_ERROR",
    };
    NormalizedDriverError { code, message: sanitized }
}
