//! Product cell-sealing boundary for the native PostgreSQL writer.
//!
//! Callers send validated plaintext through the normal `values` contract. After row/field
//! authorization and ID/scope determination, encrypted cells are sealed inside the SAME
//! resource transaction through [`WriteCellSealer`]; key material never enters this crate.
//! Without a sealer (`Knobs::default`) the previous prototype behavior is kept: `v2:`
//! frames pass through and product listeners stay fenced by
//! `DEF.ENCRYPTION_RUNTIME_UNAVAILABLE` until reads, migration/adopt and durable rekey
//! are connected on the same boundary.
use aip_execution::write_sealer::{SealCell, WriteCellSealer};
use serde_json::{Map, Value};
use spike_v2_read::id_wire::{id_array_cast, id_kind, EntityId};
use spike_v2_read::plan::Reject;
use spike_v2_read::sqlgen::{column, table, Ctx};
use std::collections::HashMap;
use tokio_postgres::Transaction;

use super::{id_array, typed_query, write_err, ApplyInput, Src};

/// Initial per-statement bound on sealed cells. Multi-row updates expand into one CASE arm
/// per row, so this keeps the statement and the reservation model small. The per-DEK
/// nonce budget is enforced independently by the key boundary.
pub const SEALED_CELLS_MAX: usize = 500;

fn rej(code: &'static str, msg: impl Into<String>) -> Reject {
    Reject { code, msg: msg.into() }
}

fn internal(e: impl ToString) -> Reject {
    Reject { code: "WIRE.INTERNAL", msg: e.to_string() }
}

fn not_queryable(msg: impl Into<String>) -> Reject {
    rej("WIRE.ENCRYPTED_FIELD_NOT_QUERYABLE", msg)
}

struct EncMeta<'a> {
    kind: &'a str,
    field: Option<&'a str>,
    target: Option<&'a str>,
    lookup_eq: bool,
}

fn enc_meta<'a>(facts: &'a Value, res: &str, field: &str) -> Result<Option<EncMeta<'a>>, Reject> {
    let Some(enc) = facts["resources"][res]["fields"][field].get("encrypted") else { return Ok(None) };
    let scope = enc.get("scope").ok_or_else(|| internal("encrypted scope 누락"))?;
    let kind = scope.get("kind").and_then(Value::as_str).ok_or_else(|| internal("encrypted scope kind 누락"))?;
    Ok(Some(EncMeta {
        kind,
        field: scope.get("field").and_then(Value::as_str),
        target: scope.get("target").and_then(Value::as_str),
        lookup_eq: enc.get("lookup").and_then(Value::as_str) == Some("eq"),
    }))
}

/// Definition-level `DEF.ENCRYPTED_*` rules already forbid these shapes; a sealed writer
/// re-checks them because facts alone must never smuggle plaintext past the boundary.
pub(crate) fn reject_sealed_assignments(facts: &Value, res: &str, tr: &str, input: Option<&ApplyInput>) -> Result<(), Reject> {
    let t = &facts["resources"][res]["transitions"][tr];
    if let Some(to) = t["to"].as_object() {
        for field in to.keys() {
            if enc_meta(facts, res, field)?.is_some() {
                return Err(not_queryable(format!("정의 상수를 `{res}.{field}`에 대입할 수 없음. 호출자의 patch 값을 쓴다")));
            }
        }
    }
    let Some(input) = input else { return Ok(()) };
    for (field, _) in &input.expected {
        if enc_meta(facts, res, field)?.is_some() {
            return Err(not_queryable(format!("expect는 암호화 필드 `{res}.{field}`에 쓸 수 없음")));
        }
    }
    // Scope references are immutable while their cells exist. Sema forbids patching them;
    // a sealed writer refuses hand-built facts that try anyway.
    for (field, _) in &input.values {
        if is_scope_field(facts, res, field)? {
            return Err(rej("WIRE.BAD_VALUE", format!("암호화 scope 참조 `{res}.{field}`는 전이로 바꿀 수 없음")));
        }
    }
    Ok(())
}

fn is_scope_field(facts: &Value, res: &str, field: &str) -> Result<bool, Reject> {
    let Some(fields) = facts["resources"][res]["fields"].as_object() else { return Ok(false) };
    for name in fields.keys() {
        if let Some(meta) = enc_meta(facts, res, name)? {
            if matches!(meta.kind, "ref" | "owner") && meta.field == Some(field) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Resolves the writable scope kind. The `bool` tells whether the scope id must be read
/// from the row's scope reference instead of using the service singleton.
fn sealed_scope(meta: &EncMeta, res: &str, field: &str) -> Result<(String, bool), Reject> {
    if meta.lookup_eq {
        return Err(not_queryable(format!("`{res}.{field}`의 lookup: eq 쓰기는 blind index 연결 전까지 미지원")));
    }
    match meta.kind {
        "service" => Ok(("service".into(), false)),
        "ref" => {
            let target = meta.target.ok_or_else(|| internal("ref scope target 누락"))?;
            Ok((format!("ref:{target}"), true))
        }
        "owner" => Err(not_queryable(format!("`{res}.{field}`의 owner scope 쓰기는 첫 수직 구현 뒤에 지원"))),
        _ => Err(internal(format!("알 수 없는 scope kind {}", meta.kind))),
    }
}

pub(crate) struct SealedUpdate {
    pub fields: Vec<String>,
    /// frames[row_index][field_index], aligned with `change`.
    pub frames: Vec<Vec<String>>,
}

pub(crate) async fn sealed_plan_for_update(
    tx: &Transaction<'_>,
    facts: &Value,
    res: &str,
    change: &[EntityId],
    input: Option<&ApplyInput>,
    sealer: &dyn WriteCellSealer,
) -> Result<Option<SealedUpdate>, Reject> {
    let Some(input) = input else { return Ok(None) };
    // (field, plaintext, scope kind, scope field when ref-scoped).
    let mut fields: Vec<(String, Vec<u8>, String, Option<String>)> = vec![];
    let mut scope_fields: Vec<String> = vec![];
    for (field, src) in &input.values {
        let Some(meta) = enc_meta(facts, res, field)? else { continue };
        match src {
            Src::Null => continue,
            Src::Lit(value, _) if value.starts_with("v2:") => continue,
            Src::Lit(value, _) => {
                let (scope_kind, needs_scope) = sealed_scope(&meta, res, field)?;
                let scope_field = if needs_scope {
                    let name = meta.field.ok_or_else(|| internal("ref scope field 누락"))?.to_string();
                    if !scope_fields.iter().any(|f| f == &name) {
                        scope_fields.push(name.clone());
                    }
                    Some(name)
                } else {
                    None
                };
                fields.push((field.clone(), value.as_bytes().to_vec(), scope_kind, scope_field));
            }
            Src::Item { .. } => return Err(internal("apply input must be a literal")),
        }
    }
    if fields.is_empty() {
        return Ok(None);
    }
    if change.len() * fields.len() > SEALED_CELLS_MAX {
        return Err(rej("WIRE.BULK_LIMIT", format!("봉인 셀 {}개 > 상한 {SEALED_CELLS_MAX}", change.len() * fields.len())));
    }
    let scopes = read_scope_ids(tx, facts, res, change, &scope_fields).await?;
    let mut cells = Vec::with_capacity(change.len() * fields.len());
    for id in change {
        let row_id = id.to_string();
        for (field, plaintext, scope_kind, scope_field) in &fields {
            let scope_id = match scope_field {
                None => "global".to_string(),
                Some(name) => scopes.get(&(row_id.clone(), name.clone())).cloned().ok_or_else(|| internal("scope 조회 누락"))?,
            };
            cells.push(SealCell {
                scope_kind: scope_kind.clone(),
                scope_id,
                resource: res.to_string(),
                field: field.clone(),
                row_id: row_id.clone(),
                plaintext: plaintext.clone(),
            });
        }
    }
    let frames = sealer.seal_cells(tx, cells).await.map_err(|e| Reject { code: e.code, msg: e.msg })?;
    if frames.len() != change.len() * fields.len() {
        return Err(internal("sealer가 셀 수와 다른 프레임을 반환"));
    }
    let mut rows = Vec::with_capacity(change.len());
    for chunk in frames.chunks_exact(fields.len()) {
        rows.push(chunk.to_vec());
    }
    Ok(Some(SealedUpdate { fields: fields.into_iter().map(|(field, _, _, _)| field).collect(), frames: rows }))
}

/// Reads the scope references of already-locked target rows, keyed by
/// (row id, scope field). A missing row or a null scope fails closed instead of
/// falling back to a default scope.
async fn read_scope_ids(
    tx: &Transaction<'_>,
    facts: &Value,
    res: &str,
    change: &[EntityId],
    scope_fields: &[String],
) -> Result<HashMap<(String, String), String>, Reject> {
    let mut out = HashMap::new();
    if scope_fields.is_empty() {
        return Ok(out);
    }
    let mut cols = vec!["t.id".to_string()];
    for field in scope_fields {
        let col = column(facts, res, field).ok_or_else(|| internal(format!("scope 열 `{res}.{field}` 누락")))?;
        cols.push(format!("t.{col}"));
    }
    let sql = format!("SELECT {} FROM {} t WHERE t.id = ANY($1::text::{})", cols.join(", "), table(res), id_array_cast(facts, res));
    let params = [Some(id_array(change))];
    let rows = typed_query(tx, sql.as_str(), &params).await.map_err(|e| write_err("scope 조회", e))?;
    for row in rows {
        let id: EntityId = row.get(0);
        for (i, field) in scope_fields.iter().enumerate() {
            let value: Option<EntityId> = row.get(i + 1);
            let Some(scope_id) = value.map(|id| id.to_string()) else {
                return Err(rej("WIRE.BAD_VALUE", format!("`{res}.{field}` scope 참조가 비어 있어 봉인할 수 없음")));
            };
            out.insert((id.to_string(), field.clone()), scope_id);
        }
    }
    for id in change {
        for field in scope_fields {
            if !out.contains_key(&(id.to_string(), field.clone())) {
                return Err(internal("판정 행의 scope를 조회하지 못함"));
            }
        }
    }
    Ok(out)
}

/// Builds the SET expression for one sealed field: a direct binding for single-row
/// updates, one CASE arm per row otherwise. Frames carry per-row AAD and random nonces,
/// so a shared binding across rows would corrupt every row but one.
pub(crate) fn sealed_set_expr(
    cx: &mut Ctx<'_>,
    facts: &Value,
    res: &str,
    change: &[EntityId],
    plan: &Option<SealedUpdate>,
    field: &str,
) -> Option<String> {
    let plan = plan.as_ref()?;
    let fi = plan.fields.iter().position(|f| f == field)?;
    if change.len() == 1 {
        return Some(cx.params.bind(Some(plan.frames[0][fi].clone()), "text"));
    }
    let mut arms = String::from("CASE t.id");
    for (ri, id) in change.iter().enumerate() {
        let slot = cx.params.bind(Some(id.to_string()), id_kind(facts, res));
        let frame = cx.params.bind(Some(plan.frames[ri][fi].clone()), "text");
        arms.push_str(&format!(" WHEN {slot} THEN {frame}"));
    }
    arms.push_str(" END");
    Some(arms)
}

pub(crate) struct SealedCreate {
    pub frames: HashMap<String, String>,
}

pub(crate) async fn sealed_plan_for_create(
    tx: &Transaction<'_>,
    facts: &Value,
    target: &str,
    values: &[(String, Src)],
    sealer: &dyn WriteCellSealer,
) -> Result<Option<SealedCreate>, Reject> {
    // (field, plaintext, scope kind).
    let mut fields: Vec<(String, Vec<u8>, String)> = vec![];
    for (field, src) in values {
        let Some(meta) = enc_meta(facts, target, field)? else { continue };
        match src {
            Src::Null => continue,
            Src::Lit(value, _) if value.starts_with("v2:") => continue,
            Src::Lit(value, _) => {
                let (scope_kind, _) = sealed_scope(&meta, target, field)?;
                fields.push((field.clone(), value.as_bytes().to_vec(), scope_kind));
            }
            Src::Item { .. } => {
                return Err(not_queryable(format!("다른 행 값을 `{target}.{field}`에 복사할 수 없음. 셀 AAD가 행에 묶임")));
            }
        }
    }
    if fields.is_empty() {
        return Ok(None);
    }
    // AAD binds the row id before the insert, so sealed creates need an explicit id.
    // Letting the database generate one would seal the wrong identity.
    let row_id = values.iter().find_map(|(field, src)| match (field.as_str(), src) {
        ("id", Src::Lit(id, _)) => Some(id.clone()),
        _ => None,
    });
    let Some(row_id) = row_id else {
        return Err(rej("WIRE.BAD_VALUE", format!("암호화 리소스 `{target}` 생성에는 명시적 id가 필요")));
    };
    if fields.len() > SEALED_CELLS_MAX {
        return Err(rej("WIRE.BULK_LIMIT", format!("봉인 셀 {}개 > 상한 {SEALED_CELLS_MAX}", fields.len())));
    }
    let mut cells = Vec::with_capacity(fields.len());
    for (field, plaintext, scope_kind) in &fields {
        let scope_id = if *scope_kind == "service" {
            "global".to_string()
        } else {
            let meta = enc_meta(facts, target, field)?.ok_or_else(|| internal("암호화 메타데이터 누락"))?;
            let scope_field = meta.field.ok_or_else(|| internal("ref scope field 누락"))?;
            scope_literal(values, target, scope_field)?
        };
        cells.push(SealCell {
            scope_kind: scope_kind.clone(),
            scope_id,
            resource: target.to_string(),
            field: field.clone(),
            row_id: row_id.clone(),
            plaintext: plaintext.clone(),
        });
    }
    let frames = sealer.seal_cells(tx, cells).await.map_err(|e| Reject { code: e.code, msg: e.msg })?;
    if frames.len() != fields.len() {
        return Err(internal("sealer가 셀 수와 다른 프레임을 반환"));
    }
    Ok(Some(SealedCreate { frames: fields.into_iter().map(|(field, _, _)| field).zip(frames).collect() }))
}

/// Ref scope for a created row comes from the caller-supplied reference literal. A value
/// copied from another row is rejected: its scope identity is not knowable before SQL.
fn scope_literal(values: &[(String, Src)], target: &str, scope_field: &str) -> Result<String, Reject> {
    for (field, src) in values {
        if field != scope_field {
            continue;
        }
        match src {
            Src::Lit(id, _) => return Ok(id.clone()),
            _ => break,
        }
    }
    Err(rej("WIRE.BAD_VALUE", format!("`{target}.{scope_field}` scope 참조 리터럴이 필요")))
}

/// Transition effects build raw SQL from server-declared values, so a sealed writer
/// refuses effects that touch encrypted cells until effects seal through this boundary.
pub(crate) fn reject_sealed_effect(facts: &Value, target: &str, values: &Map<String, Value>) -> Result<(), Reject> {
    for field in values.keys() {
        if enc_meta(facts, target, field)?.is_some() {
            return Err(not_queryable(format!("전이 효과는 암호화 필드 `{target}.{field}`에 쓸 수 없음")));
        }
    }
    Ok(())
}
