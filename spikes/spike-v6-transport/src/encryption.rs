use aip_crypto::cell::{self, Context as CellContext, DataKey};
use aip_crypto::fingerprint::{BlindIndexContext, Keys as FpKeys};
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

/// Shared visitor for create/apply/atomic/compose values, expectations and target filters.
pub fn mask_request_for_idempotency(
    facts: &Value,
    req: &Value,
    crypto: &RuntimeCrypto,
    schema: &str,
    principal: &str,
    request_key: &str,
) -> Result<Value, Reject> {
    aip_execution::encrypted_journal::protect(
        facts,
        req,
        &crypto.fp_keys,
        &aip_execution::encrypted_journal::Context { schema, principal, request_key },
    )
    .map_err(journal_error)
}

pub fn verify_idempotency_request(
    facts: &Value,
    stored: &Value,
    incoming: &Value,
    crypto: &RuntimeCrypto,
    schema: &str,
    principal: &str,
    request_key: &str,
) -> Result<bool, Reject> {
    aip_execution::encrypted_journal::verify(
        facts,
        stored,
        incoming,
        &crypto.fp_keys,
        &aip_execution::encrypted_journal::Context { schema, principal, request_key },
    )
    .map_err(journal_error)
}

fn journal_error(error: aip_execution::encrypted_journal::Error) -> Reject {
    use aip_execution::encrypted_journal::Error;
    match error {
        Error::Unavailable => rej("WIRE.ENCRYPTION_UNAVAILABLE", "멱등 지문 키를 사용할 수 없음"),
        Error::Corrupted => rej("WIRE.ENCRYPTION_CORRUPTED", "저장된 멱등 지문 형식 오류"),
        Error::InvalidInput => rej("WIRE.BAD_REQUEST", "멱등 요청 형식·크기 오류"),
    }
}

fn needs_crypto(facts: &Value) -> bool {
    facts["resources"]
        .as_object()
        .into_iter()
        .flat_map(|r| r.values())
        .any(|r| r["fields"].as_object().is_some_and(|fields| fields.values().any(|f| f.get("encrypted").is_some())))
}

/// Applied by /apply, /status and extension replay; never trust caller-supplied stored markers
/// through raw equality, and distinguish missing fingerprint keys from changed requests.
pub fn stored_request_matches(
    facts: &Value,
    stored: &str,
    incoming: &Value,
    schema: &str,
    principal: &str,
    request_key: &str,
) -> Result<bool, Reject> {
    if let Some(crypto) = get_runtime_crypto() {
        let stored = serde_json::from_str(stored).map_err(|_| rej("WIRE.ENCRYPTION_CORRUPTED", "저장된 멱등 요청 형식 오류"))?;
        verify_idempotency_request(facts, &stored, incoming, &crypto, schema, principal, request_key)
    } else if needs_crypto(facts) {
        Err(rej("WIRE.ENCRYPTION_UNAVAILABLE", "멱등 지문 키를 사용할 수 없음"))
    } else {
        let canonical = incoming.to_string();
        Ok(stored == canonical)
    }
}

pub fn journal_request(facts: &Value, incoming: &Value, schema: &str, principal: &str, request_key: &str) -> Result<String, Reject> {
    if let Some(crypto) = get_runtime_crypto() {
        mask_request_for_idempotency(facts, incoming, &crypto, schema, principal, request_key).map(|v| v.to_string())
    } else if needs_crypto(facts) {
        Err(rej("WIRE.ENCRYPTION_UNAVAILABLE", "멱등 지문 키를 사용할 수 없음"))
    } else {
        Ok(incoming.to_string())
    }
}

/// Durable effects use the same retained-key fingerprint contract as ordinary write replay.
/// Callers keep this snapshot for the entire effect, so a process-local key change cannot make
/// request generation and stored-key verification observe two different rings mid-request.
pub struct EffectJournal {
    crypto: Option<Arc<RuntimeCrypto>>,
}
impl EffectJournal {
    pub fn new(facts: &Value) -> Result<Self, Reject> {
        let crypto = get_runtime_crypto();
        if crypto.is_none() && needs_crypto(facts) {
            return Err(rej("WIRE.ENCRYPTION_UNAVAILABLE", "효과 지문 키를 사용할 수 없음"));
        }
        Ok(Self { crypto })
    }
    pub fn marker(
        &self,
        effect: &aip_execution::encrypted_journal::Effect<'_>,
        context: &aip_execution::encrypted_journal::Context<'_>,
    ) -> Result<String, Reject> {
        if let Some(crypto) = &self.crypto {
            aip_execution::encrypted_journal::effect_marker(effect, &crypto.fp_keys, context).map_err(journal_error)
        } else {
            Ok(spike_v1_fixture::digest(&json!([effect.kind, effect.name, effect.input, effect.implementation])))
        }
    }
    pub fn matches(
        &self,
        effect: &aip_execution::encrypted_journal::Effect<'_>,
        context: &aip_execution::encrypted_journal::Context<'_>,
        stored: &str,
    ) -> Result<bool, Reject> {
        if let Some(crypto) = &self.crypto {
            aip_execution::encrypted_journal::verify_effect(effect, stored, &crypto.fp_keys, context).map_err(journal_error)
        } else {
            Ok(self.marker(effect, context)? == stored)
        }
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
