//! Cell-opening pass for authorized reads.
//!
//! [`execute_opened`] runs the normal authorization SQL first and then opens stored frames
//! through [`ReadCellOpener`](aip_execution::read_opener::ReadCellOpener). Row/field policies
//! stay in SQL: a redacted cell reads as NULL and is never opened. Planner-emitted hidden
//! keys (`__aip_id`, `__aip_scope`) are stripped from the response. Without an opener the
//! previous prototype output (frames plus hidden keys) is unchanged.
use aip_execution::read_opener::{OpenCell, ReadCellOpener};
use serde_json::{Map, Value};
use tokio_postgres::Client;

use super::{execute, plan::Plan, plan::Reject};

/// Initial per-response bound on opened cells. Reads already bound rows and bytes; this
/// keeps key unwraps proportional to the response the budget allowed.
pub const OPENED_CELLS_MAX: usize = 2000;

/// Runs the authorization SQL, opens encrypted cells, and strips planner hidden keys.
pub async fn execute_opened(client: &mut Client, plan: &Plan, opener: &dyn ReadCellOpener) -> Result<Vec<Value>, Reject> {
    let mut rows = execute(client, plan).await?;
    if !has_encrypted_meta(&plan.output_type) {
        return Ok(rows);
    }
    let empty = Map::new();
    let rows_meta = plan.output_type.get("rows").and_then(Value::as_object).unwrap_or(&empty);
    let mut jobs: Vec<(Vec<Seg>, OpenCell)> = vec![];
    for (index, row) in rows.iter().enumerate() {
        collect_object(row, rows_meta, vec![Seg::Root(index)], &mut jobs)?;
    }
    if jobs.len() > OPENED_CELLS_MAX {
        return Err(Reject { code: "WIRE.OUTPUT_TOO_LARGE", msg: format!("복호화 셀 {}개 > 상한 {OPENED_CELLS_MAX}", jobs.len()) });
    }
    if !jobs.is_empty() {
        let mut paths = Vec::with_capacity(jobs.len());
        let mut cells = Vec::with_capacity(jobs.len());
        for (path, cell) in jobs {
            paths.push(path);
            cells.push(cell);
        }
        let plaintexts = opener.open_cells(client, cells).await.map_err(|e| Reject { code: e.code, msg: e.msg })?;
        if plaintexts.len() != paths.len() {
            return Err(internal("opener가 셀 수와 다른 평문을 반환"));
        }
        for (path, text) in paths.into_iter().zip(plaintexts) {
            *locate(&mut rows, &path)? = Value::String(text);
        }
    }
    for row in &mut rows {
        strip_hidden(row, rows_meta);
    }
    Ok(rows)
}

fn internal(msg: impl Into<String>) -> Reject {
    Reject { code: "WIRE.INTERNAL", msg: msg.into() }
}

fn corrupted(msg: impl Into<String>) -> Reject {
    Reject { code: "WIRE.ENCRYPTION_CORRUPTED", msg: msg.into() }
}

fn has_encrypted_meta(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, child)| key == "encrypted" || has_encrypted_meta(child)),
        Value::Array(items) => items.iter().any(has_encrypted_meta),
        _ => false,
    }
}

#[derive(Clone)]
enum Seg {
    Root(usize),
    Field(String),
    Index(usize),
}

fn locate<'a>(rows: &'a mut [Value], path: &[Seg]) -> Result<&'a mut Value, Reject> {
    fn step<'a>(value: &'a mut Value, segs: &[Seg]) -> Result<&'a mut Value, Reject> {
        let Some((first, rest)) = segs.split_first() else { return Ok(value) };
        let next = match first {
            Seg::Field(key) => value.as_object_mut().and_then(|obj| obj.get_mut(key)),
            Seg::Index(index) => value.as_array_mut().and_then(|items| items.get_mut(*index)),
            Seg::Root(_) => None,
        };
        match next {
            Some(child) => step(child, rest),
            None => Err(internal("응답 구조가 수집 시점과 다름")),
        }
    }
    match path.split_first() {
        Some((Seg::Root(index), rest)) => match rows.get_mut(*index) {
            Some(row) => step(row, rest),
            None => Err(internal("응답 구조가 수집 시점과 다름")),
        },
        _ => Err(internal("빈 셀 경로")),
    }
}

/// Collects open jobs for one object level and descends into traverse children.
/// A missing relation (null object) carries no cells.
fn collect_object(value: &Value, meta: &Map<String, Value>, path: Vec<Seg>, jobs: &mut Vec<(Vec<Seg>, OpenCell)>) -> Result<(), Reject> {
    let Some(obj) = value.as_object() else { return Ok(()) };
    for (key, field_meta) in meta {
        let Some(child) = obj.get(key) else { continue };
        if let Some(nested) = field_meta.get("object").and_then(Value::as_object) {
            let mut next = path.clone();
            next.push(Seg::Field(key.clone()));
            collect_object(child, nested, next, jobs)?;
        } else if let Some(items_meta) = field_meta.get("array").and_then(Value::as_object) {
            if let Some(items) = child.as_array() {
                for (index, item) in items.iter().enumerate() {
                    let mut next = path.clone();
                    next.push(Seg::Field(key.clone()));
                    next.push(Seg::Index(index));
                    collect_object(item, items_meta, next, jobs)?;
                }
            }
        }
    }
    // (field, frame, resource) triples at this level; redacted cells read as NULL and stay sealed.
    let mut sealed: Vec<(&str, &str, &str)> = vec![];
    for (field, field_meta) in meta {
        if field_meta.get("encrypted").is_none() {
            continue;
        }
        let resource = field_meta.get("resource").and_then(Value::as_str).unwrap_or_default();
        match obj.get(field) {
            None | Some(Value::Null) => continue,
            Some(Value::String(frame)) => sealed.push((field, frame, resource)),
            Some(_) => return Err(corrupted(format!("`{field}` 셀 형식이 암호문 문자열이 아님"))),
        }
    }
    if sealed.is_empty() {
        return Ok(());
    }
    let row_id = obj.get("__aip_id").and_then(id_string).ok_or_else(|| corrupted("암호화 행의 id 없음"))?;
    let scope = object_scope(obj, meta)?;
    for (field, frame, resource) in sealed {
        let mut slot = path.clone();
        slot.push(Seg::Field(field.to_string()));
        jobs.push((
            slot,
            OpenCell {
                scope_kind: scope.0.clone(),
                scope_id: scope.1.clone(),
                resource: resource.to_string(),
                field: field.to_string(),
                row_id: row_id.clone(),
                frame: frame.to_string(),
            },
        ));
    }
    Ok(())
}

/// Resolves the single scope pair every sealed cell of one object shares. Service cells use
/// the singleton; ref cells use the planner-projected scope value. Mixed scope references
/// and owner scope fail closed: the planner projects at most one scope column per object.
fn object_scope(obj: &Map<String, Value>, meta: &Map<String, Value>) -> Result<(String, String), Reject> {
    let mut kinds: Vec<(&str, Option<&str>, Option<&str>)> = vec![];
    for field_meta in meta.values() {
        let Some(enc) = field_meta.get("encrypted") else { continue };
        let scope = enc.get("scope").ok_or_else(|| internal("encrypted scope 누락"))?;
        kinds.push((
            scope.get("kind").and_then(Value::as_str).ok_or_else(|| internal("encrypted scope kind 누락"))?,
            scope.get("field").and_then(Value::as_str),
            scope.get("target").and_then(Value::as_str),
        ));
    }
    let mut refs: Vec<(&str, &str)> = vec![];
    for (kind, field, target) in &kinds {
        match *kind {
            "service" => {}
            "ref" => refs.push((field.ok_or_else(|| internal("ref scope field 누락"))?, target.ok_or_else(|| internal("ref scope target 누락"))?)),
            "owner" => {
                return Err(Reject {
                    code: "WIRE.ENCRYPTED_FIELD_NOT_QUERYABLE", msg: "owner scope 읽기는 첫 수직 구현 뒤에 지원".into()
                })
            }
            _ => return Err(internal(format!("알 수 없는 scope kind {kind}"))),
        }
    }
    refs.sort();
    refs.dedup();
    if refs.len() > 1 {
        return Err(Reject {
            code: "WIRE.ENCRYPTED_FIELD_NOT_QUERYABLE",
            msg: "한 행의 암호화 필드가 서로 다른 scope 참조를 써서 열 수 없음".into(),
        });
    }
    match refs.first() {
        None => Ok(("service".into(), "global".into())),
        Some((_, target)) => {
            let scope_id = obj.get("__aip_scope").and_then(id_string).ok_or_else(|| corrupted("암호화 행의 scope 참조 없음"))?;
            Ok((format!("ref:{target}"), scope_id))
        }
    }
}

/// Row and scope identities arrive as JSON strings (uuid) or numbers (bigint).
fn id_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Removes planner hidden keys from every walked object. Only keys the output type marks
/// hidden are removed, so a same-named user field (not reserved today) would survive.
fn strip_hidden(value: &mut Value, meta: &Map<String, Value>) {
    let Some(obj) = value.as_object_mut() else { return };
    for (key, field_meta) in meta {
        let Some(child) = obj.get_mut(key) else { continue };
        if let Some(nested) = field_meta.get("object").and_then(Value::as_object) {
            strip_hidden(child, nested);
        } else if let Some(items_meta) = field_meta.get("array").and_then(Value::as_object) {
            if let Some(items) = child.as_array_mut() {
                for item in items {
                    strip_hidden(item, items_meta);
                }
            }
        }
    }
    for hidden in ["__aip_id", "__aip_scope"] {
        if meta.get(hidden).and_then(|m| m.get("hidden")) == Some(&Value::Bool(true)) {
            obj.remove(hidden);
        }
    }
}
