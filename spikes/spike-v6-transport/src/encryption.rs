use aip_crypto::cell::{self, Context as CellContext, DataKey};
use aip_crypto::fingerprint::{BlindIndexContext, Context as FpContext, Keys as FpKeys};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};

#[derive(Debug, Clone)]
pub struct Reject {
    pub code: &'static str,
    pub msg: String,
}

fn rej(code: &'static str, msg: impl Into<String>) -> Reject {
    Reject { code, msg: msg.into() }
}

pub struct RuntimeCrypto {
    pub fp_keys: FpKeys,
    pub data_keys: BTreeMap<String, DataKey>,
}

static CRYPTO_INSTANCE: OnceLock<RwLock<Option<Arc<RuntimeCrypto>>>> = OnceLock::new();

fn crypto_cell() -> &'static RwLock<Option<Arc<RuntimeCrypto>>> {
    CRYPTO_INSTANCE.get_or_init(|| RwLock::new(None))
}

pub fn set_runtime_crypto(crypto: Option<Arc<RuntimeCrypto>>) {
    let cell = crypto_cell();
    let mut guard = cell.write().expect("lock crypto");
    *guard = crypto;
}

pub fn get_runtime_crypto() -> Option<Arc<RuntimeCrypto>> {
    crypto_cell().read().expect("read crypto").clone()
}

/// Creates a default test crypto instance with standard test keys.
pub fn default_test_crypto() -> Arc<RuntimeCrypto> {
    let fp_keys = FpKeys::new("fp1", vec![("fp1", [0x11; 32]), ("fp2", [0x22; 32])]).expect("valid fp keys");

    let mut data_keys = BTreeMap::new();
    let dek1 = DataKey::new("dek1", 1, [0x33; 32]).expect("valid dek");
    let dek2 = DataKey::new("dek2", 2, [0x44; 32]).expect("valid dek");
    data_keys.insert("dek1:1".to_string(), dek1);
    data_keys.insert("dek2:2".to_string(), dek2);

    Arc::new(RuntimeCrypto { fp_keys, data_keys })
}

/// Recursively replaces encrypted field values in request payload with idem_v2 HMAC fingerprints.
pub fn mask_request_for_idempotency(facts: &Value, req: &Value, crypto: &RuntimeCrypto, schema: &str, principal: &str, request_key: &str) -> Value {
    let mut masked = req.clone();
    let Some(obj) = masked.as_object_mut() else { return masked };
    let compose_res = obj.get("compose").and_then(Value::as_str).map(str::to_string);
    let apply_name = obj.get("apply").and_then(Value::as_str).map(str::to_string);
    let create_target = obj.get("create").and_then(Value::as_str).map(str::to_string);
    let is_atomic = obj.get("atomic").is_some();

    if let Some(res) = compose_res {
        if let Some(steps) = obj.get_mut("steps").and_then(Value::as_array_mut) {
            for step in steps {
                if let Some(st) = step.as_object_mut() {
                    let st_create = st.get("create").and_then(Value::as_str).map(str::to_string);
                    let st_transition = st.get("transition").and_then(Value::as_str).map(str::to_string);
                    if let Some(target) = st_create {
                        if let Some(vals) = st.get_mut("values").and_then(Value::as_object_mut) {
                            mask_object_fields(facts, &target, vals, crypto, schema, principal, request_key);
                        }
                    } else if st_transition.is_some() {
                        if let Some(vals) = st.get_mut("values").and_then(Value::as_object_mut) {
                            mask_object_fields(facts, &res, vals, crypto, schema, principal, request_key);
                        }
                        if let Some(exp) = st.get_mut("expect").and_then(Value::as_object_mut) {
                            mask_object_fields(facts, &res, exp, crypto, schema, principal, request_key);
                        }
                        if let Some(tg) = st.get_mut("target").and_then(Value::as_object_mut) {
                            if let Some(wh) = tg.get_mut("where").and_then(Value::as_object_mut) {
                                mask_object_fields(facts, &res, wh, crypto, schema, principal, request_key);
                            }
                        }
                    }
                }
            }
        }
    } else if is_atomic {
        if let Some(atomic) = obj.get_mut("atomic").and_then(Value::as_array_mut) {
            for op in atomic {
                if let Some(op_obj) = op.as_object_mut() {
                    let op_create = op_obj.get("create").and_then(Value::as_str).map(str::to_string);
                    let op_apply = op_obj.get("apply").and_then(Value::as_str).map(str::to_string);
                    if let Some(target) = op_create {
                        if let Some(vals) = op_obj.get_mut("values").and_then(Value::as_object_mut) {
                            mask_object_fields(facts, &target, vals, crypto, schema, principal, request_key);
                        }
                    } else if let Some(apply_name) = op_apply {
                        if let Some((res, _)) = apply_name.split_once('.') {
                            let res_owned = res.to_string();
                            if let Some(vals) = op_obj.get_mut("values").and_then(Value::as_object_mut) {
                                mask_object_fields(facts, &res_owned, vals, crypto, schema, principal, request_key);
                            }
                            if let Some(exp) = op_obj.get_mut("expect").and_then(Value::as_object_mut) {
                                mask_object_fields(facts, &res_owned, exp, crypto, schema, principal, request_key);
                            }
                            if let Some(tg) = op_obj.get_mut("target").and_then(Value::as_object_mut) {
                                if let Some(wh) = tg.get_mut("where").and_then(Value::as_object_mut) {
                                    mask_object_fields(facts, &res_owned, wh, crypto, schema, principal, request_key);
                                }
                            }
                        }
                    }
                }
            }
        }
    } else if let Some(apply_name) = apply_name {
        if let Some((res, _)) = apply_name.split_once('.') {
            let res_owned = res.to_string();
            if let Some(vals) = obj.get_mut("values").and_then(Value::as_object_mut) {
                mask_object_fields(facts, &res_owned, vals, crypto, schema, principal, request_key);
            }
            if let Some(exp) = obj.get_mut("expect").and_then(Value::as_object_mut) {
                mask_object_fields(facts, &res_owned, exp, crypto, schema, principal, request_key);
            }
            if let Some(tg) = obj.get_mut("target").and_then(Value::as_object_mut) {
                if let Some(wh) = tg.get_mut("where").and_then(Value::as_object_mut) {
                    mask_object_fields(facts, &res_owned, wh, crypto, schema, principal, request_key);
                }
            }
        }
    } else if let Some(target) = create_target {
        if let Some(vals) = obj.get_mut("values").and_then(Value::as_object_mut) {
            mask_object_fields(facts, &target, vals, crypto, schema, principal, request_key);
        }
    }
    masked
}

fn mask_object_fields(
    facts: &Value,
    res: &str,
    vals: &mut Map<String, Value>,
    crypto: &RuntimeCrypto,
    schema: &str,
    principal: &str,
    request_key: &str,
) {
    let Some(resource_fields) = facts["resources"][res]["fields"].as_object() else { return };
    for (f, val) in vals.iter_mut() {
        if resource_fields.get(f).and_then(|fd| fd.get("encrypted")).is_some() && !val.is_null() {
            let bytes = match &*val {
                Value::String(s) => s.as_bytes().to_vec(),
                other => other.to_string().into_bytes(),
            };
            let ctx = FpContext { schema, principal, request_key, resource: res, field: f };
            if let Ok(marker) = crypto.fp_keys.idem(&ctx, &bytes) {
                *val = json!({ "$aip_enc_fp": marker });
            }
        }
    }
}

/// Verifies that incoming plaintext request matches a stored idempotency request that contains $aip_enc_fp markers.
pub fn verify_idempotency_request(
    facts: &Value,
    stored_req: &Value,
    incoming_req: &Value,
    crypto: &RuntimeCrypto,
    schema: &str,
    principal: &str,
    request_key: &str,
) -> bool {
    verify_json_match(facts, stored_req, incoming_req, crypto, schema, principal, request_key, None)
}

#[allow(clippy::too_many_arguments, clippy::only_used_in_recursion)]
fn verify_json_match(
    _facts: &Value,
    stored: &Value,
    incoming: &Value,
    crypto: &RuntimeCrypto,
    schema: &str,
    principal: &str,
    request_key: &str,
    current_res_and_field: Option<(&str, &str)>,
) -> bool {
    if let Value::Object(s_map) = stored {
        if let Some(fp_val) = s_map.get("$aip_enc_fp").and_then(Value::as_str) {
            let Some((res, field)) = current_res_and_field else { return false };
            let bytes = match incoming {
                Value::String(s) => s.as_bytes().to_vec(),
                other => other.to_string().into_bytes(),
            };
            let ctx = FpContext { schema, principal, request_key, resource: res, field };
            return crypto.fp_keys.verify_idem(&ctx, &bytes, fp_val).unwrap_or(false);
        }
    }
    match (stored, incoming) {
        (Value::Object(s_map), Value::Object(i_map)) => {
            if s_map.len() != i_map.len() {
                return false;
            }
            for (k, s_val) in s_map {
                let Some(i_val) = i_map.get(k) else { return false };
                let next_context = match current_res_and_field {
                    Some((res, _)) => Some((res, k.as_str())),
                    None => {
                        if let Some(apply_name) = s_map.get("apply").and_then(Value::as_str) {
                            apply_name.split_once('.').map(|(r, _)| (r, k.as_str()))
                        } else {
                            s_map.get("create").and_then(Value::as_str).map(|create_name| (create_name, k.as_str()))
                        }
                    }
                };
                if !verify_json_match(_facts, s_val, i_val, crypto, schema, principal, request_key, next_context) {
                    return false;
                }
            }
            true
        }
        (Value::Array(s_arr), Value::Array(i_arr)) => {
            if s_arr.len() != i_arr.len() {
                return false;
            }
            for (s_item, i_item) in s_arr.iter().zip(i_arr) {
                if !verify_json_match(_facts, s_item, i_item, crypto, schema, principal, request_key, current_res_and_field) {
                    return false;
                }
            }
            true
        }
        (s, i) => s == i,
    }
}

/// Decrypts encrypted fields in /read rows. Removes __aip_id and __aip_scope projection columns.
pub fn decrypt_rows(facts: &Value, rows: &mut [Value], output_type: &Value, crypto: &RuntimeCrypto, schema: &str) -> Result<(), Reject> {
    for row in rows.iter_mut() {
        if let Some(obj) = row.as_object_mut() {
            decrypt_object_fields(facts, obj, output_type.get("rows"), crypto, schema)?;
            // Clean up hidden projection columns
            obj.remove("__aip_id");
            obj.remove("__aip_scope");
        }
    }
    Ok(())
}

#[allow(clippy::only_used_in_recursion)]
fn decrypt_object_fields(
    _facts: &Value,
    obj: &mut Map<String, Value>,
    meta_rows: Option<&Value>,
    crypto: &RuntimeCrypto,
    schema: &str,
) -> Result<(), Reject> {
    let row_id = obj.get("__aip_id").or_else(|| obj.get("id")).and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    });

    let scope_id_candidate = obj.get("__aip_scope").and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    });

    let ref_values: BTreeMap<String, String> = obj.iter().filter_map(|(k, v)| val_to_str(v).map(|s| (k.clone(), s))).collect();

    for (k, val) in obj.iter_mut() {
        if k.starts_with("__aip_") {
            continue;
        }
        if let Value::Object(nested_obj) = val {
            let nested_meta = meta_rows.and_then(|m| m.get(k)).and_then(|t| t.get("object"));
            decrypt_object_fields(_facts, nested_obj, nested_meta, crypto, schema)?;
            nested_obj.remove("__aip_id");
            nested_obj.remove("__aip_scope");
            continue;
        }
        if let Value::Array(nested_arr) = val {
            let nested_meta = meta_rows.and_then(|m| m.get(k)).and_then(|t| t.get("array"));
            for item in nested_arr {
                if let Value::Object(item_obj) = item {
                    decrypt_object_fields(_facts, item_obj, nested_meta, crypto, schema)?;
                    item_obj.remove("__aip_id");
                    item_obj.remove("__aip_scope");
                }
            }
            continue;
        }

        let Some(meta) = meta_rows.and_then(|m| m.get(k)) else { continue };
        let Some(enc) = meta.get("encrypted") else { continue };
        let Some(res) = meta.get("resource").and_then(Value::as_str) else { continue };

        if val.is_null() {
            continue;
        }

        let Value::String(frame) = val else {
            return Err(rej("WIRE.ENCRYPTION_CORRUPTED", format!("암호화 셀 포맷 오류: {res}.{k}")));
        };

        let header = cell::parse_header(frame).map_err(|_| rej("WIRE.ENCRYPTION_CORRUPTED", format!("암호화 셀 포맷 오류: {res}.{k}")))?;

        let key_ref = format!("{}:{}", header.dek_id, header.version);
        let dek = crypto.data_keys.get(&key_ref).ok_or_else(|| rej("WIRE.ENCRYPTION_UNAVAILABLE", format!("복호화 키 부재: {key_ref}")))?;

        let scope_kind = enc["scope"]["kind"].as_str().unwrap_or("service");
        let (scope_kind_str, scope_id_str): (String, String) = match scope_kind {
            "service" => ("service".to_string(), "global".to_string()),
            "ref" => {
                let scope_id = scope_id_candidate
                    .clone()
                    .or_else(|| enc["scope"]["field"].as_str().and_then(|rf| ref_values.get(rf)).cloned())
                    .unwrap_or_else(|| "0".to_string());
                (format!("ref:{}", enc["scope"]["target"].as_str().unwrap_or("")), scope_id)
            }
            "owner" => ("owner:actor".to_string(), "0".to_string()),
            _ => ("service".to_string(), "global".to_string()),
        };

        let row_id_str = row_id.clone().unwrap_or_else(|| "0".to_string());
        let ctx = CellContext { schema, resource: res, field: k, row_id: &row_id_str, scope_kind: &scope_kind_str, scope_id: &scope_id_str };

        let plaintext = dek.open(&ctx, frame).map_err(|_| rej("WIRE.ENCRYPTION_CORRUPTED", format!("복호화 인증 실패: {res}.{k}")))?;
        let text = String::from_utf8(plaintext.to_vec()).map_err(|_| rej("WIRE.ENCRYPTION_CORRUPTED", format!("평문 UTF-8 오류: {res}.{k}")))?;
        *val = json!(text);
    }
    Ok(())
}

/// Encrypts encrypted field values in a write request payload using the active DEK.
/// Also produces the blind index column value if lookup: eq is configured.
fn val_to_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

pub async fn resolve_row_ids_for_create(tx: &tokio_postgres::Transaction<'_>, facts: &Value, req: &mut Value) -> Result<(), Reject> {
    let Some(obj) = req.as_object_mut() else { return Ok(()) };
    let create_target = obj.get("create").and_then(Value::as_str).map(str::to_string);
    let is_atomic = obj.get("atomic").is_some();
    let is_compose = obj.get("steps").is_some();

    if let Some(target) = create_target {
        resolve_single_create_id(tx, facts, &target, obj).await?;
    } else if is_atomic {
        if let Some(atomic) = obj.get_mut("atomic").and_then(Value::as_array_mut) {
            for op in atomic {
                if let Some(op_obj) = op.as_object_mut() {
                    let op_create = op_obj.get("create").and_then(Value::as_str).map(str::to_string);
                    if let Some(target) = op_create {
                        resolve_single_create_id(tx, facts, &target, op_obj).await?;
                    }
                }
            }
        }
    } else if is_compose {
        if let Some(steps) = obj.get_mut("steps").and_then(Value::as_array_mut) {
            for step in steps {
                if let Some(st) = step.as_object_mut() {
                    let st_create = st.get("create").and_then(Value::as_str).map(str::to_string);
                    if let Some(target) = st_create {
                        resolve_single_create_id(tx, facts, &target, st).await?;
                    }
                }
            }
        }
    }
    Ok(())
}

async fn resolve_single_create_id(
    tx: &tokio_postgres::Transaction<'_>,
    facts: &Value,
    target: &str,
    obj: &mut Map<String, Value>,
) -> Result<(), Reject> {
    if let Some(vals) = obj.get("values").and_then(Value::as_object) {
        if vals.get("id").is_some() && !vals["id"].is_null() {
            return Ok(());
        }
    }
    let ty = facts["resources"][target]["fields"]["id"]["ty"].as_str().unwrap_or("Id");
    if ty == "Uuid" {
        let new_uuid = uuid::Uuid::new_v4().to_string();
        if let Some(vals) = obj.get_mut("values").and_then(Value::as_object_mut) {
            vals.insert("id".to_string(), json!(new_uuid));
        }
    } else {
        let tbl = spike_v2_read::sqlgen::table(target);
        let q = format!("SELECT nextval(pg_get_serial_sequence('{tbl}', 'id'))::text");
        let row = tx.query_one(&q, &[]).await.map_err(|e| rej("WIRE.INTERNAL", format!("시퀀스 조회 실패: {e}")))?;
        let next_id_str: String = row.get(0);
        let next_id_num: i64 = next_id_str.parse().unwrap_or(0);
        if let Some(vals) = obj.get_mut("values").and_then(Value::as_object_mut) {
            vals.insert("id".to_string(), json!(next_id_num));
        }
    }
    Ok(())
}

/// Encrypts encrypted field values in a write request payload using the active DEK.
/// Also produces the blind index column value if lookup: eq is configured.
pub fn encrypt_request_for_write(facts: &Value, req: &Value, crypto: &RuntimeCrypto, schema: &str, default_row_id: &str) -> Result<Value, Reject> {
    let mut encrypted_req = req.clone();
    let Some(obj) = encrypted_req.as_object_mut() else { return Ok(encrypted_req) };
    let apply_name = obj.get("apply").and_then(Value::as_str).map(str::to_string);
    let create_target = obj.get("create").and_then(Value::as_str).map(str::to_string);
    let compose_res = obj.get("compose").and_then(Value::as_str).map(str::to_string);
    let is_atomic = obj.get("atomic").is_some();

    if let Some(apply_name) = apply_name {
        if let Some((res, _)) = apply_name.split_once('.') {
            let res_owned = res.to_string();
            let row_id = obj.get("target").and_then(|t| t.get("id")).and_then(val_to_str).unwrap_or_else(|| default_row_id.to_string());
            if let Some(vals) = obj.get_mut("values").and_then(Value::as_object_mut) {
                encrypt_object_values(facts, &res_owned, vals, crypto, schema, &row_id)?;
            }
        }
    } else if let Some(target) = create_target {
        let row_id = obj
            .get("values")
            .and_then(|v| v.get("id"))
            .and_then(val_to_str)
            .or_else(|| obj.get("id").and_then(val_to_str))
            .unwrap_or_else(|| default_row_id.to_string());
        if let Some(vals) = obj.get_mut("values").and_then(Value::as_object_mut) {
            encrypt_object_values(facts, &target, vals, crypto, schema, &row_id)?;
        }
    } else if is_atomic {
        if let Some(atomic) = obj.get_mut("atomic").and_then(Value::as_array_mut) {
            for op in atomic {
                if let Some(op_obj) = op.as_object_mut() {
                    let op_create = op_obj.get("create").and_then(Value::as_str).map(str::to_string);
                    let op_apply = op_obj.get("apply").and_then(Value::as_str).map(str::to_string);
                    if let Some(target) = op_create {
                        let row_id = op_obj
                            .get("values")
                            .and_then(|v| v.get("id"))
                            .and_then(val_to_str)
                            .or_else(|| op_obj.get("id").and_then(val_to_str))
                            .unwrap_or_else(|| default_row_id.to_string());
                        if let Some(vals) = op_obj.get_mut("values").and_then(Value::as_object_mut) {
                            encrypt_object_values(facts, &target, vals, crypto, schema, &row_id)?;
                        }
                    } else if let Some(apply_name) = op_apply {
                        if let Some((res, _)) = apply_name.split_once('.') {
                            let res_owned = res.to_string();
                            let row_id =
                                op_obj.get("target").and_then(|t| t.get("id")).and_then(val_to_str).unwrap_or_else(|| default_row_id.to_string());
                            if let Some(vals) = op_obj.get_mut("values").and_then(Value::as_object_mut) {
                                encrypt_object_values(facts, &res_owned, vals, crypto, schema, &row_id)?;
                            }
                        }
                    }
                }
            }
        }
    } else if let Some(res) = compose_res {
        if let Some(steps) = obj.get_mut("steps").and_then(Value::as_array_mut) {
            for step in steps {
                if let Some(st) = step.as_object_mut() {
                    let st_create = st.get("create").and_then(Value::as_str).map(str::to_string);
                    let st_transition = st.get("transition").and_then(Value::as_str).map(str::to_string);
                    if let Some(target) = st_create {
                        let row_id = st
                            .get("values")
                            .and_then(|v| v.get("id"))
                            .and_then(val_to_str)
                            .or_else(|| st.get("id").and_then(val_to_str))
                            .unwrap_or_else(|| default_row_id.to_string());
                        if let Some(vals) = st.get_mut("values").and_then(Value::as_object_mut) {
                            encrypt_object_values(facts, &target, vals, crypto, schema, &row_id)?;
                        }
                    } else if st_transition.is_some() {
                        let row_id = st.get("target").and_then(|t| t.get("id")).and_then(val_to_str).unwrap_or_else(|| default_row_id.to_string());
                        if let Some(vals) = st.get_mut("values").and_then(Value::as_object_mut) {
                            encrypt_object_values(facts, &res, vals, crypto, schema, &row_id)?;
                        }
                    }
                }
            }
        }
    }
    Ok(encrypted_req)
}

fn encrypt_object_values(
    facts: &Value,
    res: &str,
    vals: &mut Map<String, Value>,
    crypto: &RuntimeCrypto,
    schema: &str,
    row_id: &str,
) -> Result<(), Reject> {
    let Some(resource_fields) = facts["resources"][res]["fields"].as_object() else { return Ok(()) };
    let ref_values: BTreeMap<String, String> = vals.iter().filter_map(|(k, v)| val_to_str(v).map(|s| (k.clone(), s))).collect();
    let mut additions = vec![];
    for (f, val) in vals.iter_mut() {
        let Some(fd) = resource_fields.get(f) else { continue };
        let Some(enc) = fd.get("encrypted") else { continue };
        if val.is_null() {
            continue;
        }

        let bytes = match &*val {
            Value::String(s) => s.as_bytes().to_vec(),
            other => other.to_string().into_bytes(),
        };

        let scope_kind = enc["scope"]["kind"].as_str().unwrap_or("service");
        let (scope_kind_str, scope_id_str): (String, String) = match scope_kind {
            "service" => ("service".to_string(), "global".to_string()),
            "ref" => {
                let ref_field = enc["scope"]["field"].as_str().unwrap_or("");
                let scope_id = ref_values.get(ref_field).cloned().unwrap_or_else(|| "0".to_string());
                (format!("ref:{}", enc["scope"]["target"].as_str().unwrap_or("")), scope_id)
            }
            "owner" => ("owner:actor".to_string(), "0".to_string()),
            _ => ("service".to_string(), "global".to_string()),
        };

        let ctx = CellContext { schema, resource: res, field: f, row_id, scope_kind: &scope_kind_str, scope_id: &scope_id_str };

        // Use the first active DEK
        let dek = crypto.data_keys.values().next().ok_or_else(|| rej("WIRE.ENCRYPTION_UNAVAILABLE", "암호화 활성 DEK 부재"))?;

        let frame = dek.seal(&ctx, &bytes).map_err(|_| rej("WIRE.ENCRYPTION_UNAVAILABLE", "암호화 봉인 실패"))?;
        *val = json!(frame);

        // Blind index if lookup: eq is enabled
        if enc["lookup"] == "eq" {
            let bidx_ctx = BlindIndexContext { schema, resource: res, field: f, scope_kind: &scope_kind_str, scope_id: &scope_id_str };
            if let Ok(tags) = crypto.fp_keys.blind_tags(&bidx_ctx, &bytes) {
                if let Some(tag) = tags.first() {
                    additions.push((format!("{f}__bidx"), json!(tag)));
                }
            }
        }
    }
    for (k, v) in additions {
        vals.insert(k, v);
    }
    Ok(())
}

/// Prepares query filters by transforming plaintext eq filter values for encrypted fields into blind index HMAC tags.
pub fn prepare_query_blind_index(facts: &Value, query: &mut Value, crypto: &RuntimeCrypto, schema: &str) {
    let Some(q_obj) = query.as_object_mut() else { return };
    let Some(res) = q_obj.get("read").and_then(Value::as_str).map(str::to_string) else { return };
    let Some(filters) = q_obj.get_mut("filter").and_then(Value::as_array_mut) else { return };

    for filter in filters {
        if let Some(fo) = filter.as_object_mut() {
            if let Some(or_items) = fo.get_mut("or").and_then(Value::as_array_mut) {
                for item in or_items {
                    if let Some(io) = item.as_object_mut() {
                        transform_single_filter(facts, &res, io, crypto, schema);
                    }
                }
            } else {
                transform_single_filter(facts, &res, fo, crypto, schema);
            }
        }
    }
}

fn transform_single_filter(facts: &Value, res: &str, fo: &mut Map<String, Value>, crypto: &RuntimeCrypto, schema: &str) {
    let Some(field) = fo.get("field").and_then(Value::as_str) else { return };
    let Some(op) = fo.get("op").and_then(Value::as_str) else { return };
    if op != "eq" {
        return;
    }
    let Some(enc) = facts["resources"][res]["fields"][field].get("encrypted") else { return };
    if enc["lookup"] != "eq" {
        return;
    }

    let Some(raw_val) = fo.get("value") else { return };
    let bytes = match raw_val {
        Value::String(s) => s.as_bytes().to_vec(),
        other => other.to_string().into_bytes(),
    };

    let scope_kind = enc["scope"]["kind"].as_str().unwrap_or("service");
    let (scope_kind_str, scope_id_str): (String, String) = match scope_kind {
        "service" => ("service".to_string(), "global".to_string()),
        _ => ("service".to_string(), "global".to_string()),
    };

    let bidx_ctx = BlindIndexContext { schema, resource: res, field, scope_kind: &scope_kind_str, scope_id: &scope_id_str };

    if let Ok(tags) = crypto.fp_keys.blind_tags(&bidx_ctx, &bytes) {
        if let Some(tag) = tags.first() {
            fo.insert("value".to_string(), json!(tag));
        }
    }
}
