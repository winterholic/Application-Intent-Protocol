//! Full AEAD verification of adopted encrypted columns.
//!
//! Every non-null cell must open under the deployment KEK before adopt mutates anything.
//! Plaintexts are dropped page by page; only counts leave this module. Lookup blind-index
//! columns are not verified: the write boundary refuses lookup writes, so adopted bidx
//! values are never trusted for queries.
use super::{configure_schema, err, quote_ident, validate_structure};
use aip_execution::read_opener::{OpenCell, ReadCellOpener};
use serde_json::Value;
use spike_v2_read::sqlgen;
use tokio_postgres::{Transaction, types::ToSql};

/// Rows scanned per verification round trip. Keyset pagination keeps memory flat while
/// covering the whole column.
const PAGE_ROWS: i64 = 1000;

#[derive(Debug)]
pub enum ScopeSpec {
    Service,
    Ref { target: String, column: String },
}

#[derive(Debug)]
pub struct CellColumn {
    pub resource: String,
    pub field: String,
    pub table: String,
    pub column: String,
    pub id_kind: &'static str,
    pub scope: ScopeSpec,
}

/// Enumerates encrypted columns in deterministic order. Owner scope fails closed here,
/// matching the write and read boundaries that defer it past the first vertical.
pub fn encrypted_columns(schema: &str, facts: &Value) -> Result<Vec<CellColumn>, Value> {
    validate_structure(facts)?;
    let mut out = vec![];
    let resources = facts["resources"].as_object().ok_or_else(|| err("BAD_FACTS", "resources 객체 누락"))?;
    for (res, rf) in resources {
        let table = format!("{schema}.{}", sqlgen::snake(res));
        let id_kind = if rf["idKind"] == "uuid" { "uuid" } else { "bigint" };
        let fields = rf["fields"].as_object().ok_or_else(|| err("BAD_FACTS", "fields 객체 누락"))?;
        for (field, desc) in fields {
            let Some(enc) = desc.get("encrypted") else { continue };
            let scope = enc.get("scope").and_then(Value::as_object).ok_or_else(|| err("BAD_FACTS", "encrypted scope 객체 누락"))?;
            let kind = scope.get("kind").and_then(Value::as_str).ok_or_else(|| err("BAD_FACTS", "encrypted scope kind 누락"))?;
            let scope = match kind {
                "service" => ScopeSpec::Service,
                "ref" => {
                    let target = scope.get("target").and_then(Value::as_str).ok_or_else(|| err("BAD_FACTS", "encrypted ref scope target 누락"))?;
                    let scope_field = scope.get("field").and_then(Value::as_str).ok_or_else(|| err("BAD_FACTS", "encrypted ref scope field 누락"))?;
                    let column = sqlgen::column(facts, res, scope_field).ok_or_else(|| err("BAD_FACTS", "encrypted ref scope 열 확인 실패"))?;
                    ScopeSpec::Ref { target: target.to_string(), column }
                }
                "owner" => {
                    return Err(err("WIRE.ENCRYPTED_FIELD_NOT_QUERYABLE", "owner scope 열은 첫 수직 구현 뒤에 이관 지원"));
                }
                _ => return Err(err("BAD_FACTS", "지원하지 않는 encrypted scope 종류")),
            };
            let column = sqlgen::column(facts, res, field).ok_or_else(|| err("BAD_FACTS", "암호화 열 확인 실패"))?;
            out.push(CellColumn { resource: res.clone(), field: field.clone(), table: table.clone(), column, id_kind, scope });
        }
    }
    out.sort_by(|a, b| (&a.table, &a.column).cmp(&(&b.table, &b.column)));
    Ok(out)
}

/// Opens every non-null cell of every encrypted column. Returns (columns, cells).
/// Runs inside the caller's transaction so adopted rows are verified under lock.
pub async fn verify_encrypted_cells(tx: &Transaction<'_>, schema: &str, facts: &Value, opener: &dyn ReadCellOpener) -> Result<(usize, usize), Value> {
    configure_schema(schema)?;
    let columns = encrypted_columns(schema, facts)?;
    let mut cells = 0usize;
    for column in &columns {
        cells += verify_column(tx, column, opener).await?;
    }
    Ok((columns.len(), cells))
}

async fn verify_column(tx: &Transaction<'_>, column: &CellColumn, opener: &dyn ReadCellOpener) -> Result<usize, Value> {
    let (schema, table) = column.table.split_once('.').ok_or_else(|| err("BAD_FACTS", "테이블 이름 오류"))?;
    let table = format!("{}.{}", quote_ident(schema), quote_ident(table));
    let cell = quote_ident(&column.column);
    let scope_select = match &column.scope {
        ScopeSpec::Service => String::new(),
        ScopeSpec::Ref { column, .. } => format!("{}::text, ", quote_ident(column)),
    };
    let mut last: Option<String> = None;
    let mut count = 0usize;
    loop {
        let (sql, params): (String, Vec<&(dyn ToSql + Sync)>) = match &last {
            None => (format!("SELECT id::text, {scope_select}{cell} FROM {table} ORDER BY id LIMIT {PAGE_ROWS}"), vec![]),
            Some(id) => (
                format!("SELECT id::text, {scope_select}{cell} FROM {table} WHERE id > $1::text::{} ORDER BY id LIMIT {PAGE_ROWS}", column.id_kind),
                vec![id],
            ),
        };
        let rows = tx.query(&sql, &params).await.map_err(|_| err("DB_CATALOG", "암호화 열 조회 실패"))?;
        if rows.is_empty() {
            break;
        }
        let width = rows.len();
        let mut batch = Vec::with_capacity(width);
        for row in &rows {
            let id: String = row.try_get(0).map_err(|_| err("WIRE.ENCRYPTION_CORRUPTED", "암호화 행의 id 확인 실패"))?;
            if id.is_empty() {
                return Err(err("WIRE.ENCRYPTION_CORRUPTED", "암호화 행의 id 확인 실패"));
            }
            last = Some(id.clone());
            let (scope_kind, scope_id, frame): (String, String, Option<String>) = match &column.scope {
                ScopeSpec::Service => {
                    let frame: Option<String> = row.try_get(1).map_err(|_| err("WIRE.ENCRYPTION_CORRUPTED", "셀 조회 실패"))?;
                    ("service".to_string(), "global".to_string(), frame)
                }
                ScopeSpec::Ref { target, .. } => {
                    let scope: Option<String> = row.try_get(1).map_err(|_| err("WIRE.ENCRYPTION_CORRUPTED", "scope 조회 실패"))?;
                    let frame: Option<String> = row.try_get(2).map_err(|_| err("WIRE.ENCRYPTION_CORRUPTED", "셀 조회 실패"))?;
                    match (scope.filter(|s| !s.is_empty()), frame) {
                        (_, None) => continue,
                        (Some(scope_id), Some(frame)) => (format!("ref:{target}"), scope_id, Some(frame)),
                        (None, Some(_)) => return Err(err("WIRE.ENCRYPTION_CORRUPTED", "암호화 행의 scope 참조 없음")),
                    }
                }
            };
            let Some(frame) = frame else { continue };
            batch.push(OpenCell { scope_kind, scope_id, resource: column.resource.clone(), field: column.field.clone(), row_id: id, frame });
        }
        if !batch.is_empty() {
            let opened = batch.len();
            opener.open_cells_in_tx(tx, batch).await.map_err(|e| err(e.code, &e.msg))?;
            count += opened;
        }
        if width < PAGE_ROWS as usize {
            break;
        }
    }
    Ok(count)
}
