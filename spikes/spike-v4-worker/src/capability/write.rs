//! Write commands of the managed Mongo and Redis capabilities (WBS 7.2.5).
//!
//! A write extension declares `access mongo.<collection>` / `access redis "<key or prefix*>"`; the server owns the connections and
//! checks every write against that grant, a per-collection field allowlist and bounded value shapes. Each command returns an
//! [`EffectOutcome`]: the value for the extension and, when the call asked for `compensate: true`, the record a rollback needs to undo
//! it (the original saga's `compensation_data`). The extension host keeps those records and runs them in reverse order if the
//! relational commit does not happen.
use super::*;
use mongodb::bson::{doc, Bson, Document};

/// What one external write did.
#[derive(Debug, Clone)]
pub struct EffectOutcome {
    pub value: Value,
    /// Undo record (JSON, replayed by `compensate`). `None` when the call did not ask for compensation or has nothing to undo.
    pub compensation: Option<Value>,
}

const MAX_DOCUMENT_BYTES: usize = 1_048_576;
const MAX_DEPTH: usize = 8;
const EXT_JSON_KEYS: [&str; 5] = ["$date", "$oid", "$numberLong", "$numberInt", "$numberDouble"];
const MAX_REDIS_VALUE_BYTES: usize = 1_048_576;
const MAX_HASH_FIELDS: usize = 64;
const MAX_HASH_BYTES: usize = 2_097_152;
const MAX_REDIS_TTL_SECONDS: u64 = 2_592_000;
const MAX_LIST_LEN: i64 = 10_000;

/// Top-level fields each writable collection accepts (the original models' documents; see `app/models/mongodb`).
pub fn write_fields(collection: &str) -> &'static [&'static str] {
    match collection {
        "youtube_watching_data" => &[
            "user_id",
            "video_id",
            "video_view_log_id",
            "duration",
            "created_at",
            "updated_at",
            "completion_rate",
            "dominant_emotion",
            "emotion_percentages",
            "most_emotion_timeline",
            "emotion_score_timeline",
            "emotion_seconds",
            "finalized_at",
            "client_info",
            "frame_count",
            "emotion_sum",
        ],
        "video_distribution" => &[
            "video_id",
            "category",
            "duration",
            "total_frames",
            "average_completion_rate",
            "emotion_averages",
            "recommendation_scores",
            "dominant_emotion",
            "created_at",
            "updated_at",
        ],
        "video_timeline_emotion_count" => &["video_id", "emotion_labels", "counts", "created_at", "updated_at"],
        _ => &[],
    }
}

/// `true` for a value an extension may send: JSON scalars, arrays and objects whose keys carry no operator (`$...`) and no path dot,
/// plus the one-key extended JSON scalars a date or id needs.
fn check_value(value: &Value, depth: usize) -> Result<(), Reject> {
    if depth > MAX_DEPTH {
        return Err(reject("WIRE.BAD_VALUE", "Mongo 값 중첩이 너무 깊음"));
    }
    match value {
        Value::Array(items) => items.iter().try_for_each(|item| check_value(item, depth + 1)),
        Value::Object(map) => {
            if map.len() == 1 && map.keys().all(|key| EXT_JSON_KEYS.contains(&key.as_str())) {
                return Ok(());
            }
            for (key, item) in map {
                if key.is_empty() || key.starts_with('$') || key.contains('.') || key.contains('\0') {
                    return Err(reject("WIRE.BAD_VALUE", "Mongo 값의 키가 허용되지 않음"));
                }
                check_value(item, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// JSON (with the extended JSON scalars the read side returns) to BSON. Integers become `Int32` when they fit, as the original's
/// Python driver stores them; a one-key `$oid`, `$date`, `$numberLong`, `$numberInt` or `$numberDouble` object is its scalar.
fn json_to_bson(value: &Value) -> Result<Bson, Reject> {
    let bad = || reject("WIRE.BAD_VALUE", "Mongo 값을 변환하지 못함");
    Ok(match value {
        Value::Null => Bson::Null,
        Value::Bool(flag) => Bson::Boolean(*flag),
        Value::Number(number) => {
            if let Some(integer) = number.as_i64() {
                i32::try_from(integer).map_or(Bson::Int64(integer), Bson::Int32)
            } else if let Some(float) = number.as_f64() {
                Bson::Double(float)
            } else {
                return Err(bad());
            }
        }
        Value::String(text) => Bson::String(text.clone()),
        Value::Array(items) => Bson::Array(items.iter().map(json_to_bson).collect::<Result<_, _>>()?),
        Value::Object(map) => {
            if let (1, Some((key, inner))) = (map.len(), map.iter().next()) {
                match (key.as_str(), inner) {
                    ("$oid", Value::String(text)) => return mongodb::bson::oid::ObjectId::parse_str(text).map(Bson::ObjectId).map_err(|_| bad()),
                    ("$date", Value::String(text)) => {
                        return mongodb::bson::DateTime::parse_rfc3339_str(text).map(Bson::DateTime).map_err(|_| bad());
                    }
                    ("$date", Value::Object(millis)) if millis.len() == 1 => {
                        let text = millis.get("$numberLong").and_then(Value::as_str).ok_or_else(bad)?;
                        return text.parse::<i64>().map(|ms| Bson::DateTime(mongodb::bson::DateTime::from_millis(ms))).map_err(|_| bad());
                    }
                    ("$numberLong", Value::String(text)) => return text.parse::<i64>().map(Bson::Int64).map_err(|_| bad()),
                    ("$numberInt", Value::String(text)) => return text.parse::<i32>().map(Bson::Int32).map_err(|_| bad()),
                    ("$numberDouble", Value::String(text)) => return text.parse::<f64>().map(Bson::Double).map_err(|_| bad()),
                    _ => {}
                }
            }
            let mut document = Document::new();
            for (key, item) in map {
                document.insert(key.clone(), json_to_bson(item)?);
            }
            Bson::Document(document)
        }
    })
}

fn to_bson(value: &Value) -> Result<Bson, Reject> {
    check_value(value, 0)?;
    if serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > MAX_DOCUMENT_BYTES) {
        return Err(reject("WIRE.BAD_VALUE", "Mongo 값이 크기 상한을 넘음"));
    }
    json_to_bson(value)
}

/// Converts a value whose parts were already checked one by one (filters and updates carry `$in`/`$set`, which `check_value` refuses
/// as keys of a document value).
fn to_checked_document(value: &Value, what: &str) -> Result<Document, Reject> {
    if serde_json::to_vec(value).map_or(true, |bytes| bytes.len() > MAX_DOCUMENT_BYTES) {
        return Err(reject("WIRE.BAD_VALUE", "Mongo 값이 크기 상한을 넘음"));
    }
    match json_to_bson(value)? {
        Bson::Document(document) => Ok(document),
        _ => Err(reject("WIRE.BAD_VALUE", &format!("Mongo {what}는 객체여야 함"))),
    }
}

fn to_document(value: &Value, what: &str) -> Result<Document, Reject> {
    match to_bson(value)? {
        Bson::Document(document) => Ok(document),
        _ => Err(reject("WIRE.BAD_VALUE", &format!("Mongo {what}는 객체여야 함"))),
    }
}

fn path_allowed(path: &str, fields: &[&str]) -> bool {
    let mut segments = path.split('.');
    let Some(first) = segments.next() else { return false };
    let rest: Vec<&str> = segments.collect();
    fields.contains(&first)
        && rest.len() <= 3
        && rest.iter().all(|segment| !segment.is_empty() && segment.len() <= 64 && segment.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
}

fn input_keys(input: &Value, allowed: &[&str]) -> Result<(), Reject> {
    if input.as_object().is_none_or(|map| map.keys().any(|key| !allowed.contains(&key.as_str()))) {
        return Err(reject("WIRE.BAD_VALUE", "Mongo 쓰기 입력 필드가 허용되지 않음"));
    }
    Ok(())
}

fn wants_compensation(input: &Value) -> Result<bool, Reject> {
    match input.get("compensate") {
        None => Ok(false),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(_) => Err(reject("WIRE.BAD_VALUE", "compensate는 boolean이어야 함")),
    }
}

fn write_failure(error: &mongodb::error::Error) -> Reject {
    use mongodb::error::{ErrorKind, WriteFailure};
    if let ErrorKind::Write(WriteFailure::WriteError(write)) = &*error.kind {
        if write.code == 11000 {
            return reject("WIRE.CONFLICT", "Mongo 고유 키 충돌");
        }
    }
    reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo 쓰기 실패")
}

fn document_value(document: Document) -> Result<Value, Reject> {
    mongodb::bson::deserialize_from_document::<Value>(document).map_err(|_| reject("WIRE.BAD_VALUE", "Mongo 문서 변환 실패"))
}

impl MongoDriverCapability {
    /// Opens `collections` for write commands. A collection outside the original models, or none at all, is refused.
    pub fn enable_writes(mut self, collections: &[&str]) -> Result<Self, Reject> {
        if collections.is_empty() || collections.iter().any(|collection| write_fields(collection).is_empty()) {
            return Err(reject("WIRE.BAD_CONFIG", "Mongo 쓰기 collection 설정 범위 오류"));
        }
        self.write_collections = collections.iter().map(|c| (*c).to_string()).collect();
        Ok(self)
    }

    /// Creates the indexes the original repositories create (`ensure_indexes`/constructors): the unique keys the upserts rely on.
    pub async fn ensure_write_indexes(&self) -> Result<(), Reject> {
        use mongodb::{options::IndexOptions, IndexModel};
        let database = self.client.database(&self.database);
        let unique = |field: &str| IndexModel::builder().keys(doc! {field: 1}).options(IndexOptions::builder().unique(true).build()).build();
        let plain = |keys: Document| IndexModel::builder().keys(keys).build();
        let mut plans: Vec<(&str, Vec<IndexModel>)> = Vec::new();
        for collection in &self.write_collections {
            match collection.as_str() {
                "youtube_watching_data" => plans.push((
                    "youtube_watching_data",
                    vec![
                        plain(doc! {"user_id": 1, "created_at": -1}),
                        plain(doc! {"user_id": 1, "finalized_at": 1, "video_view_log_id": 1}),
                        plain(doc! {"video_id": 1, "created_at": -1}),
                        unique("video_view_log_id"),
                    ],
                )),
                "video_distribution" => plans.push(("video_distribution", vec![unique("video_id")])),
                "video_timeline_emotion_count" => plans.push(("video_timeline_emotion_count", vec![unique("video_id")])),
                _ => {}
            }
        }
        plans.push((SAGA_COLLECTION, vec![unique("transaction_id"), plain(doc! {"created_at": -1}), plain(doc! {"status": 1})]));
        for (name, models) in plans {
            tokio::time::timeout(self.timeout, database.collection::<Document>(name).create_indexes(models))
                .await
                .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo index timeout"))?
                .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo index 생성 실패"))?;
        }
        Ok(())
    }

    fn write_filter(&self, collection: &str, filter: &Value, principal: &CapabilityPrincipal) -> Result<Document, Reject> {
        let fields = write_fields(collection);
        let Some(map) = filter.as_object() else {
            return Err(reject("WIRE.BAD_VALUE", "Mongo filter는 객체여야 함"));
        };
        if map.is_empty() {
            return Err(reject("WIRE.BAD_VALUE", "Mongo 쓰기 filter는 비어 있을 수 없음"));
        }
        let mut checked = serde_json::Map::new();
        for (key, value) in map {
            if key != "_id" && !fields.contains(&key.as_str()) {
                return Err(reject("WIRE.BAD_VALUE", "Mongo filter field가 허용되지 않음"));
            }
            let condition = match value.as_object() {
                Some(operators) if operators.len() == 1 && operators.contains_key("$in") => {
                    let Some(items) = operators["$in"].as_array().filter(|items| items.len() <= 100) else {
                        return Err(reject("WIRE.BAD_VALUE", "$in은 100개 이하 배열이어야 함"));
                    };
                    items.iter().try_for_each(|item| check_value(item, 1))?;
                    value.clone()
                }
                _ => {
                    check_value(value, 1)?;
                    value.clone()
                }
            };
            checked.insert(key.clone(), condition);
        }
        if collection == "youtube_watching_data" {
            let Some(actor) = principal.private_user_id() else {
                return Err(reject("WIRE.ACCESS_DENIED", "인증 principal이 필요함"));
            };
            if checked.get("user_id").is_some_and(|value| value != &actor) {
                return Err(reject("WIRE.ACCESS_DENIED", "요청 actor가 인증 principal과 다름"));
            }
            checked.insert("user_id".into(), actor);
        }
        to_checked_document(&Value::Object(checked), "filter")
    }

    fn check_write_document(&self, collection: &str, document: &Value, principal: &CapabilityPrincipal) -> Result<Document, Reject> {
        let fields = write_fields(collection);
        let Some(map) = document.as_object() else {
            return Err(reject("WIRE.BAD_VALUE", "Mongo document는 객체여야 함"));
        };
        if map.keys().any(|key| !fields.contains(&key.as_str())) {
            return Err(reject("WIRE.BAD_VALUE", "Mongo document field가 허용되지 않음"));
        }
        let mut map = map.clone();
        if collection == "youtube_watching_data" {
            let Some(actor) = principal.private_user_id() else {
                return Err(reject("WIRE.ACCESS_DENIED", "인증 principal이 필요함"));
            };
            if map.get("user_id").is_some_and(|value| value != &actor) {
                return Err(reject("WIRE.ACCESS_DENIED", "요청 actor가 인증 principal과 다름"));
            }
            map.insert("user_id".into(), actor);
        }
        to_document(&Value::Object(map), "document")
    }

    fn check_update(&self, collection: &str, update: &Value, principal: &CapabilityPrincipal) -> Result<Document, Reject> {
        let fields = write_fields(collection);
        let Some(operators) = update.as_object().filter(|map| !map.is_empty()) else {
            return Err(reject("WIRE.BAD_VALUE", "Mongo update는 비어 있지 않은 객체여야 함"));
        };
        let mut checked = serde_json::Map::new();
        for (operator, body) in operators {
            if !["$set", "$setOnInsert", "$inc"].contains(&operator.as_str()) {
                return Err(reject("WIRE.BAD_VALUE", "지원하지 않는 Mongo update operator"));
            }
            let Some(body) = body.as_object().filter(|map| !map.is_empty() && map.len() <= 64) else {
                return Err(reject("WIRE.BAD_VALUE", "Mongo update 본문은 비어 있지 않은 객체여야 함"));
            };
            let mut body = body.clone();
            for (path, value) in &body {
                if !path_allowed(path, fields) {
                    return Err(reject("WIRE.BAD_VALUE", "Mongo update 경로가 허용되지 않음"));
                }
                if operator == "$inc" && !value.is_number() {
                    return Err(reject("WIRE.BAD_VALUE", "$inc 값은 숫자여야 함"));
                }
                check_value(value, 1)?;
                if path == "user_id" && collection == "youtube_watching_data" {
                    let Some(actor) = principal.private_user_id() else {
                        return Err(reject("WIRE.ACCESS_DENIED", "인증 principal이 필요함"));
                    };
                    if value != &actor || operator == "$inc" {
                        return Err(reject("WIRE.ACCESS_DENIED", "요청 actor가 인증 principal과 다름"));
                    }
                }
            }
            if operator == "$setOnInsert" && collection == "youtube_watching_data" {
                if let Some(actor) = principal.private_user_id() {
                    body.entry("user_id").or_insert(actor);
                }
            }
            checked.insert(operator.clone(), Value::Object(body));
        }
        to_checked_document(&Value::Object(checked), "update")
    }

    pub(crate) async fn write_effect(&self, principal: &CapabilityPrincipal, collection: &str, input: &Value) -> Result<EffectOutcome, Reject> {
        if !principal.permits("mongo", collection) {
            return Err(reject("WIRE.ACCESS_NOT_DECLARED", "Mongo capability가 확장에 선언되지 않음"));
        }
        if !self.write_collections.contains(collection) {
            return Err(reject("WIRE.CAPABILITY_DENIED", "Mongo collection에 쓰기가 허용되지 않음"));
        }
        let Some(command) = input.get("command").and_then(Value::as_str) else {
            return Err(reject("WIRE.BAD_VALUE", "Mongo command 누락"));
        };
        let coll = self.client.database(&self.database).collection::<Document>(collection);
        let compensate = wants_compensation(input)?;
        tokio::time::timeout(self.timeout, async {
            match command {
                "insert_one" => {
                    input_keys(input, &["command", "document", "compensate"])?;
                    let document = self.check_write_document(collection, input.get("document").unwrap_or(&Value::Null), principal)?;
                    let result = coll.insert_one(document).await.map_err(|e| write_failure(&e))?;
                    let id = document_value(doc! {"_id": result.inserted_id})?["_id"].clone();
                    let compensation = compensate.then(|| json!({"op": "delete", "filter": {"_id": id.clone()}}));
                    Ok(EffectOutcome { value: json!({"inserted_id": id}), compensation })
                }
                "update_one" | "find_one_and_update" => {
                    input_keys(input, &["command", "filter", "update", "upsert", "compensate"])?;
                    let filter = self.write_filter(collection, input.get("filter").unwrap_or(&Value::Null), principal)?;
                    let update = self.check_update(collection, input.get("update").unwrap_or(&Value::Null), principal)?;
                    let upsert = match input.get("upsert") {
                        None => false,
                        Some(Value::Bool(flag)) => *flag,
                        Some(_) => return Err(reject("WIRE.BAD_VALUE", "upsert는 boolean이어야 함")),
                    };
                    // The pre-image is read only when the caller asked for a way back; a frame upsert on a large session does not.
                    let previous = if compensate {
                        coll.find_one(filter.clone()).await.map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo pre-image 읽기 실패"))?
                    } else {
                        None
                    };
                    let (value, written_id) = if command == "update_one" {
                        let result = coll.update_one(filter.clone(), update).upsert(upsert).await.map_err(|e| write_failure(&e))?;
                        let upserted = match result.upserted_id {
                            Some(id) => Some(document_value(doc! {"_id": id})?["_id"].clone()),
                            None => None,
                        };
                        (json!({"matched": result.matched_count, "modified": result.modified_count, "upserted_id": upserted}), upserted)
                    } else {
                        let after = coll
                            .find_one_and_update(filter.clone(), update)
                            .upsert(upsert)
                            .return_document(mongodb::options::ReturnDocument::After)
                            .await
                            .map_err(|e| write_failure(&e))?;
                        let after = after.map(document_value).transpose()?;
                        if after.as_ref().is_some_and(|v| serde_json::to_vec(v).map_or(true, |b| b.len() > MAX_DOCUMENT_BYTES)) {
                            return Err(reject("WIRE.BAD_VALUE", "Mongo result byte limit exceeded"));
                        }
                        let id = after.as_ref().map(|v| v["_id"].clone());
                        (json!({"doc": after}), previous.is_none().then_some(id).flatten())
                    };
                    let compensation = if compensate {
                        match previous {
                            Some(previous) => {
                                let id = document_value(doc! {"_id": previous.get("_id").cloned().unwrap_or(Bson::Null)})?["_id"].clone();
                                Some(json!({"op": "replace", "filter": {"_id": id}, "document": document_value(previous)?}))
                            }
                            None => written_id.map(|id| json!({"op": "delete", "filter": {"_id": id}})),
                        }
                    } else {
                        None
                    };
                    Ok(EffectOutcome { value, compensation })
                }
                "delete_one" => {
                    input_keys(input, &["command", "filter", "compensate"])?;
                    let filter = self.write_filter(collection, input.get("filter").unwrap_or(&Value::Null), principal)?;
                    let previous = if compensate {
                        coll.find_one(filter.clone()).await.map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo pre-image 읽기 실패"))?
                    } else {
                        None
                    };
                    let result = coll.delete_one(filter).await.map_err(|e| write_failure(&e))?;
                    let compensation = (result.deleted_count > 0).then(|| previous.map(document_value)).flatten().transpose()?;
                    Ok(EffectOutcome {
                        value: json!({"deleted": result.deleted_count}),
                        compensation: compensation.map(|document| json!({"op": "insert", "document": document})),
                    })
                }
                _ => Err(reject("WIRE.CAPABILITY_DENIED", "Mongo write command가 허용되지 않음")),
            }
        })
        .await
        .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo write timeout"))?
    }

    /// Replays one undo record produced by [`Self::write_effect`]. The record is the server's own, never an extension's input.
    pub(crate) async fn compensate_effect(&self, collection: &str, record: &Value) -> Result<(), Reject> {
        if !self.write_collections.contains(collection) {
            return Err(reject("WIRE.CAPABILITY_DENIED", "Mongo collection에 쓰기가 허용되지 않음"));
        }
        let coll = self.client.database(&self.database).collection::<Document>(collection);
        let id_filter = |value: &Value| -> Result<Document, Reject> {
            match json_to_bson(&value["filter"]["_id"]) {
                Ok(id) => Ok(doc! {"_id": id}),
                Err(_) => Err(reject("WIRE.BAD_VALUE", "보상 기록의 _id 형식 오류")),
            }
        };
        let work = async {
            match record["op"].as_str() {
                Some("delete") => {
                    coll.delete_one(id_filter(record)?).await.map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo 보상 실패"))?;
                }
                Some("replace") => {
                    let Bson::Document(document) = json_to_bson(&record["document"])? else {
                        return Err(reject("WIRE.BAD_VALUE", "보상 문서 형식 오류"));
                    };
                    coll.replace_one(id_filter(record)?, document)
                        .upsert(true)
                        .await
                        .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo 보상 실패"))?;
                }
                Some("insert") => {
                    let Bson::Document(document) = json_to_bson(&record["document"])? else {
                        return Err(reject("WIRE.BAD_VALUE", "보상 문서 형식 오류"));
                    };
                    // A retried compensation finds the document already back: that is the intended state, not a failure.
                    match coll.insert_one(document).await {
                        Ok(_) => {}
                        Err(error) if matches!(write_failure(&error).code, "WIRE.CONFLICT") => {}
                        Err(_) => return Err(reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo 보상 실패")),
                    }
                }
                _ => return Err(reject("WIRE.BAD_VALUE", "알 수 없는 보상 기록")),
            }
            Ok(())
        };
        tokio::time::timeout(self.timeout, work).await.map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo 보상 timeout"))?
    }

    /// Upserts the saga log document of one external-write transaction, shaped like the original `saga_transaction_log`.
    pub(crate) async fn journal_record(&self, record: &Value) -> Result<(), Reject> {
        let Some(id) = record["transaction_id"].as_str() else {
            return Err(reject("WIRE.BAD_VALUE", "journal transaction_id 누락"));
        };
        let Bson::Document(document) = json_to_bson(record)? else {
            return Err(reject("WIRE.BAD_VALUE", "journal 형식 오류"));
        };
        let coll = self.client.database(&self.database).collection::<Document>(SAGA_COLLECTION);
        tokio::time::timeout(self.timeout, coll.replace_one(doc! {"transaction_id": id}, document).upsert(true))
            .await
            .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo journal timeout"))?
            .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo journal 쓰기 실패"))?;
        Ok(())
    }
}

/// The original saga log collection. Only the server writes it; no extension grant reaches it.
pub const SAGA_COLLECTION: &str = "saga_transaction_log";

fn bounded_ttl(input: &Value) -> Result<u64, Reject> {
    match input.get("ttl_seconds").and_then(Value::as_u64) {
        Some(ttl) if (1..=MAX_REDIS_TTL_SECONDS).contains(&ttl) => Ok(ttl),
        _ => Err(reject("WIRE.BAD_VALUE", "ttl_seconds는 1초 이상 30일 이하여야 함")),
    }
}

fn redis_value(input: &Value) -> Result<&str, Reject> {
    match input.get("value").and_then(Value::as_str) {
        Some(text) if text.len() <= MAX_REDIS_VALUE_BYTES => Ok(text),
        _ => Err(reject("WIRE.BAD_VALUE", "Redis value는 64KB 이하 문자열이어야 함")),
    }
}

impl RedisDriverCapability {
    pub(crate) async fn write_effect(&self, principal: &CapabilityPrincipal, key: &str, input: &Value) -> Result<EffectOutcome, Reject> {
        let Some(command) = input.get("command").and_then(Value::as_str) else {
            return Err(reject("WIRE.BAD_VALUE", "Redis command 누락"));
        };
        if key.is_empty() || key.len() > 512 {
            return Err(reject("WIRE.BAD_VALUE", "Redis key length 범위 오류"));
        }
        if !principal.permits("redis", key) {
            return Err(reject("WIRE.ACCESS_NOT_DECLARED", "Redis key capability가 확장에 선언되지 않음"));
        }
        if !self.allowed(key) {
            return Err(reject("WIRE.CAPABILITY_DENIED", "Redis key namespace가 허용되지 않음"));
        }
        let allowed: &[&str] = match command {
            "set_ex" | "set_nx_ex" => &["command", "value", "ttl_seconds", "compensate"],
            "del" | "getdel" => &["command", "compensate"],
            "incr_expire" | "expire" => &["command", "ttl_seconds"],
            "push_trim_expire" => &["command", "value", "max_len", "ttl_seconds"],
            "replace_hash_ex" => &["command", "fields", "ttl_seconds"],
            _ => return Err(reject("WIRE.CAPABILITY_DENIED", "Redis write command가 허용되지 않음")),
        };
        input_keys(input, allowed)?;
        let compensate = wants_compensation(input)?;
        let _permit = tokio::time::timeout(self.timeout, self.in_flight.clone().acquire_owned())
            .await
            .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis capacity timeout"))?
            .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis capability가 종료됨"))?;
        let mut con = self.manager.clone();
        let unavailable = |_: redis::RedisError| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis write failed");
        let work = async {
            // Pre-image of a string key (value and remaining time) so that a rollback can put it back.
            let preimage = async |con: &mut redis::aio::ConnectionManager| -> Result<Value, Reject> {
                let (value, pttl): (Option<String>, i64) = redis::cmd("EVAL_RO")
                    .arg("return {redis.call('GET',KEYS[1]), redis.call('PTTL',KEYS[1])}")
                    .arg(1)
                    .arg(key)
                    .query_async(con)
                    .await
                    .map_err(unavailable)?;
                Ok(json!({"op": "restore", "value": value, "pttl": pttl}))
            };
            match command {
                "set_ex" => {
                    let (value, ttl) = (redis_value(input)?, bounded_ttl(input)?);
                    let previous = if compensate { Some(preimage(&mut con).await?) } else { None };
                    redis::cmd("SET").arg(key).arg(value).arg("EX").arg(ttl).exec_async(&mut con).await.map_err(unavailable)?;
                    Ok(EffectOutcome { value: json!({"set": true}), compensation: previous })
                }
                "set_nx_ex" => {
                    let (value, ttl) = (redis_value(input)?, bounded_ttl(input)?);
                    let set: Option<String> =
                        redis::cmd("SET").arg(key).arg(value).arg("NX").arg("EX").arg(ttl).query_async(&mut con).await.map_err(unavailable)?;
                    let created = set.is_some();
                    // The key did not exist, so the way back is to delete it (only when this call created it).
                    Ok(EffectOutcome {
                        value: json!({"set": created}),
                        compensation: (compensate && created).then(|| json!({"op": "restore", "value": null, "pttl": -2})),
                    })
                }
                "del" => {
                    let previous = if compensate { Some(preimage(&mut con).await?) } else { None };
                    let removed: u64 = redis::cmd("DEL").arg(key).query_async(&mut con).await.map_err(unavailable)?;
                    Ok(EffectOutcome { value: json!({"deleted": removed}), compensation: previous.filter(|_| removed > 0) })
                }
                "getdel" => {
                    let previous = if compensate { Some(preimage(&mut con).await?) } else { None };
                    let value: Option<String> = redis::cmd("GETDEL").arg(key).query_async(&mut con).await.map_err(unavailable)?;
                    let compensation = previous.filter(|_| value.is_some());
                    Ok(EffectOutcome { value: json!({"value": value}), compensation })
                }
                "incr_expire" => {
                    let ttl = bounded_ttl(input)?;
                    let value: i64 = redis::cmd("EVAL")
                        .arg("local n=redis.call('INCR',KEYS[1]); redis.call('EXPIRE',KEYS[1],ARGV[1]); return n")
                        .arg(1)
                        .arg(key)
                        .arg(ttl)
                        .query_async(&mut con)
                        .await
                        .map_err(unavailable)?;
                    Ok(EffectOutcome { value: json!({"value": value}), compensation: None })
                }
                "expire" => {
                    let ttl = bounded_ttl(input)?;
                    let applied: bool = redis::cmd("EXPIRE").arg(key).arg(ttl).query_async(&mut con).await.map_err(unavailable)?;
                    Ok(EffectOutcome { value: json!({"applied": applied}), compensation: None })
                }
                "replace_hash_ex" => {
                    let ttl = bounded_ttl(input)?;
                    let Some(fields) = input.get("fields").and_then(Value::as_object).filter(|f| !f.is_empty() && f.len() <= MAX_HASH_FIELDS) else {
                        return Err(reject("WIRE.BAD_VALUE", "fields는 1개 이상 64개 이하의 객체여야 함"));
                    };
                    let mut total = 0usize;
                    let mut arguments: Vec<&str> = Vec::with_capacity(fields.len() * 2);
                    for (name, value) in fields {
                        let Some(text) = value.as_str() else {
                            return Err(reject("WIRE.BAD_VALUE", "hash field 값은 문자열이어야 함"));
                        };
                        total += name.len() + text.len();
                        if name.is_empty() || name.len() > 128 || total > MAX_HASH_BYTES {
                            return Err(reject("WIRE.BAD_VALUE", "hash field 이름 또는 전체 크기가 상한을 넘음"));
                        }
                        arguments.push(name);
                        arguments.push(text);
                    }
                    // One script: the old hash is replaced as a whole, so a reader never sees a half-built category cache.
                    redis::cmd("EVAL")
                        .arg("redis.call('DEL',KEYS[1]); for i=2,#ARGV,2 do redis.call('HSET',KEYS[1],ARGV[i],ARGV[i+1]) end; redis.call('EXPIRE',KEYS[1],ARGV[1]); return 1")
                        .arg(1)
                        .arg(key)
                        .arg(ttl)
                        .arg(&arguments)
                        .exec_async(&mut con)
                        .await
                        .map_err(unavailable)?;
                    Ok(EffectOutcome { value: json!({"replaced": fields.len()}), compensation: None })
                }
                _ => {
                    let value = redis_value(input)?;
                    let max_len = match input.get("max_len").and_then(Value::as_i64) {
                        Some(n) if (1..=MAX_LIST_LEN).contains(&n) => n,
                        _ => return Err(reject("WIRE.BAD_VALUE", "max_len은 1 이상 10000 이하여야 함")),
                    };
                    let ttl = bounded_ttl(input)?;
                    redis::cmd("EVAL")
                        .arg("redis.call('LPUSH',KEYS[1],ARGV[1]); redis.call('LTRIM',KEYS[1],0,ARGV[2]-1); redis.call('EXPIRE',KEYS[1],ARGV[3]); return 1")
                        .arg(1)
                        .arg(key)
                        .arg(value)
                        .arg(max_len)
                        .arg(ttl)
                        .exec_async(&mut con)
                        .await
                        .map_err(unavailable)?;
                    Ok(EffectOutcome { value: json!({"pushed": true}), compensation: None })
                }
            }
        };
        tokio::time::timeout(self.timeout, work).await.map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis write timeout"))?
    }

    pub(crate) async fn compensate_effect(&self, key: &str, record: &Value) -> Result<(), Reject> {
        if !self.allowed(key) || record["op"] != "restore" {
            return Err(reject("WIRE.CAPABILITY_DENIED", "Redis 보상 기록이 허용되지 않음"));
        }
        let mut con = self.manager.clone();
        let work = async {
            let unavailable = |_: redis::RedisError| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis 보상 실패");
            match (record["value"].as_str(), record["pttl"].as_i64()) {
                (Some(value), Some(pttl)) if pttl > 0 => {
                    redis::cmd("SET").arg(key).arg(value).arg("PX").arg(pttl).exec_async(&mut con).await.map_err(unavailable)
                }
                (Some(value), Some(_)) => redis::cmd("SET").arg(key).arg(value).exec_async(&mut con).await.map_err(unavailable),
                _ => redis::cmd("DEL").arg(key).exec_async(&mut con).await.map_err(unavailable),
            }
        };
        tokio::time::timeout(self.timeout, work).await.map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis 보상 timeout"))?
    }
}
