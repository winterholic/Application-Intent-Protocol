//! 관리형 Mongo/Redis capability 경계 및 호스트 인터페이스 (WBS 7.2.3 Slice13-A).
//!
//! 워커는 DB 연결 DSN이나 운영 자격증명을 직접 받지 않으며, 서버가 확정한 principal과 화이트리스트 정책 아래에서만
//! 데이터베이스/컬렉션/키/명령어에 접근한다. 허용되지 않은 컬렉션, 키 패턴, 쓰기 명령은 즉시 `WIRE.CAPABILITY_DENIED`로 거부된다.

use serde_json::{json, Value};
use spike_v2_read::id_wire::{parse_uuid, EntityId};
use spike_v2_read::plan::Reject;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod write;
pub use write::{write_fields, EffectOutcome, SAGA_COLLECTION};

pub type CapabilityFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Identity and exact capability grants derived by the server for this invocation.
#[derive(Clone, Debug, Default)]
pub struct CapabilityPrincipal {
    /// Internal numeric handle of the session actor (`aip_principals.actor_id`).
    pub actor_id: Option<i64>,
    /// Public id of a `User.id: Uuid` actor. Set only by `resolve_actor_uuid` from the server-verified session,
    /// never from the extension input or a caller filter. When present it, not the internal handle, owns private rows.
    pub actor_uuid: Option<EntityId>,
    pub grants: HashSet<String>,
}

/// Turns the session's server-verified public actor id into the Uuid that owns private Mongo rows.
/// `None` means an integer-keyed actor (the public id is absent or is that actor's own decimal id).
/// Any other value that is not the canonical lowercase UUID fails closed instead of falling back to the internal handle.
pub fn resolve_actor_uuid(actor_id: Option<i64>, public_id: Option<&str>) -> Result<Option<EntityId>, Reject> {
    let Some(public_id) = public_id else {
        return Ok(None);
    };
    match parse_uuid(public_id) {
        Ok(uuid) => Ok(Some(uuid)),
        Err(_) if actor_id.is_some_and(|id| id.to_string() == public_id) => Ok(None),
        Err(_) => Err(reject("WIRE.ACCESS_DENIED", "세션 actor 공개 id를 확인할 수 없음")),
    }
}

impl CapabilityPrincipal {
    /// Value the private `user_id` column must equal for this actor: the canonical UUID string, or the integer handle.
    fn private_user_id(&self) -> Option<Value> {
        // 인증된 actor_id가 없는 principal은 guest다. actor_uuid만으로 소유권을 인정하지 않는다.
        self.actor_id?;
        match self.actor_uuid {
            Some(id @ EntityId::Uuid(_)) => Some(json!(id.to_string())),
            Some(EntityId::Number(_)) => None,
            None => self.actor_id.map(|id| json!(id)),
        }
    }

    pub fn permits(&self, kind: &str, resource: &str) -> bool {
        let exact = format!("{kind}.{resource}");
        self.grants.contains(&exact)
            || (kind == "redis"
                && self.grants.iter().any(|grant| {
                    grant
                        .strip_suffix('*')
                        .is_some_and(|prefix| prefix.starts_with("redis.") && prefix.len() > "redis.".len() && exact.starts_with(prefix))
                }))
    }
}

/// Mongo 접근 capability 경계.
pub trait MongoCapability: Send + Sync {
    fn call<'a>(&'a self, principal: &'a CapabilityPrincipal, collection: &'a str, input: &'a Value) -> CapabilityFuture<'a, Result<Value, Reject>>;

    /// One write command of a write extension (`insert_one`, `update_one`, `find_one_and_update`, `delete_one`). A read-only
    /// capability refuses every write.
    fn write<'a>(
        &'a self,
        _principal: &'a CapabilityPrincipal,
        _collection: &'a str,
        _input: &'a Value,
    ) -> CapabilityFuture<'a, Result<EffectOutcome, Reject>> {
        Box::pin(async { Err(reject("WIRE.CAPABILITY_DENIED", "Mongo 쓰기 capability가 설정되지 않음")) })
    }

    /// Undoes one write from the record `write` returned.
    fn compensate<'a>(&'a self, _collection: &'a str, _record: &'a Value) -> CapabilityFuture<'a, Result<(), Reject>> {
        Box::pin(async { Err(reject("WIRE.CAPABILITY_DENIED", "Mongo 쓰기 capability가 설정되지 않음")) })
    }

    /// Saves the saga log document of an external-write transaction. A capability without a journal accepts and drops it.
    fn journal<'a>(&'a self, _record: &'a Value) -> CapabilityFuture<'a, Result<(), Reject>> {
        Box::pin(async { Ok(()) })
    }
}

/// Redis 접근 capability 경계.
pub trait RedisCapability: Send + Sync {
    fn call<'a>(&'a self, principal: &'a CapabilityPrincipal, key: &'a str, input: &'a Value) -> CapabilityFuture<'a, Result<Value, Reject>>;

    /// One write command of a write extension (`set_ex`, `set_nx_ex`, `del`, `getdel`, `incr_expire`, `expire`, `push_trim_expire`).
    fn write<'a>(
        &'a self,
        _principal: &'a CapabilityPrincipal,
        _key: &'a str,
        _input: &'a Value,
    ) -> CapabilityFuture<'a, Result<EffectOutcome, Reject>> {
        Box::pin(async { Err(reject("WIRE.CAPABILITY_DENIED", "Redis 쓰기 capability가 설정되지 않음")) })
    }

    /// Undoes one write from the record `write` returned.
    fn compensate<'a>(&'a self, _key: &'a str, _record: &'a Value) -> CapabilityFuture<'a, Result<(), Reject>> {
        Box::pin(async { Err(reject("WIRE.CAPABILITY_DENIED", "Redis 쓰기 capability가 설정되지 않음")) })
    }
}

/// 화이트리스트로 제한된 관리형 MongoDB capability.
#[derive(Clone)]
pub struct MemoryMongoCapability {
    pub allowed_collections: HashSet<String>,
    pub allowed_commands: HashSet<String>,
    data: Arc<Mutex<HashMap<String, Vec<Value>>>>,
}

impl MemoryMongoCapability {
    pub fn new(allowed_collections: &[&str], allowed_commands: &[&str]) -> Self {
        Self {
            allowed_collections: allowed_collections.iter().map(|s| (*s).to_string()).collect(),
            allowed_commands: allowed_commands.iter().map(|s| (*s).to_string()).collect(),
            data: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 테스트 또는 시드 데이터를 컬렉션에 주입한다.
    pub fn seed(&self, collection: &str, docs: Vec<Value>) {
        let mut map = self.data.lock().unwrap();
        map.insert(collection.to_string(), docs);
    }
}

impl MongoCapability for MemoryMongoCapability {
    fn call<'a>(&'a self, principal: &'a CapabilityPrincipal, collection: &'a str, input: &'a Value) -> CapabilityFuture<'a, Result<Value, Reject>> {
        Box::pin(async move {
            if !principal.permits("mongo", collection) {
                return Err(reject("WIRE.ACCESS_NOT_DECLARED", "Mongo capability가 확장에 선언되지 않음"));
            }
            // 1. 컬렉션 화이트리스트 검증
            if !self.allowed_collections.contains(collection) {
                return Err(Reject { code: "WIRE.CAPABILITY_DENIED", msg: format!("비인가 Mongo 컬렉션 접근 거부: `{collection}`") });
            }

            // 2. 명령어 검증 (읽기 전용 제한)
            let command = input["command"].as_str().unwrap_or("find");
            if !self.allowed_commands.contains(command) {
                return Err(Reject { code: "WIRE.CAPABILITY_DENIED", msg: format!("허용되지 않은 Mongo 명령: `{command}`") });
            }

            // 3. 인메모리/저장소 조회 수행
            let map = self.data.lock().unwrap();
            let docs = map.get(collection).cloned().unwrap_or_default();

            let filter = input.get("filter");
            if filter.is_some_and(|f| !f.is_object()) {
                return Err(reject("WIRE.BAD_VALUE", "Mongo filter는 객체여야 함"));
            }
            let private_actor = if collection == "youtube_watching_data" {
                let Some(actor) = principal.private_user_id() else {
                    return Err(reject("WIRE.ACCESS_DENIED", "인증 principal이 필요함"));
                };
                if filter.and_then(|f| f.get("user_id")).is_some_and(|v| v != &actor) {
                    return Err(reject("WIRE.ACCESS_DENIED", "요청 actor가 인증 principal과 다름"));
                }
                Some(actor)
            } else {
                None
            };
            let filtered: Vec<Value> = if let Some(f) = filter.and_then(Value::as_object) {
                docs.into_iter()
                    .filter(|doc| {
                        private_actor.as_ref().is_none_or(|actor| doc.get("user_id") == Some(actor))
                            && f.iter().all(|(k, v)| {
                                if let Some(in_arr) = v.as_object().and_then(|o| o.get("$in")).and_then(Value::as_array) {
                                    doc.get(k).is_some_and(|val| in_arr.contains(val))
                                } else {
                                    doc.get(k) == Some(v)
                                }
                            })
                    })
                    .collect()
            } else {
                docs
            };

            match command {
                "count_documents" => Ok(json!({ "count": filtered.len() })),
                "find_one" => Ok(json!({ "doc": filtered.into_iter().next() })),
                "find" => {
                    let mut res = filtered;
                    if let Some(sort_field) = input.get("sort_by").and_then(Value::as_str) {
                        let desc = input.get("desc").and_then(Value::as_bool).unwrap_or(false);
                        res.sort_by(|a, b| {
                            let cmp = a.get(sort_field).and_then(Value::as_str).cmp(&b.get(sort_field).and_then(Value::as_str));
                            if desc {
                                cmp.reverse()
                            } else {
                                cmp
                            }
                        });
                    }
                    if let Some(skip) = input.get("skip").and_then(Value::as_u64) {
                        if (skip as usize) < res.len() {
                            res = res.split_off(skip as usize);
                        } else {
                            res.clear();
                        }
                    }
                    if let Some(limit) = input.get("limit").and_then(Value::as_u64) {
                        res.truncate(limit as usize);
                    }
                    Ok(json!({ "docs": res }))
                }
                _ => Err(Reject { code: "WIRE.CAPABILITY_DENIED", msg: format!("미구현 Mongo 명령: `{command}`") }),
            }
        })
    }
}

/// 화이트리스트로 제한된 관리형 Redis capability.
#[derive(Clone)]
pub struct MemoryRedisCapability {
    pub allowed_prefixes: Vec<String>,
    pub allowed_commands: HashSet<String>,
    strings: Arc<Mutex<HashMap<String, String>>>,
    hashes: Arc<Mutex<HashMap<String, HashMap<String, String>>>>,
}

impl MemoryRedisCapability {
    pub fn new(allowed_prefixes: &[&str], allowed_commands: &[&str]) -> Self {
        Self {
            allowed_prefixes: allowed_prefixes.iter().map(|s| (*s).to_string()).collect(),
            allowed_commands: allowed_commands.iter().map(|s| (*s).to_string()).collect(),
            strings: Arc::new(Mutex::new(HashMap::new())),
            hashes: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 테스트용 문자열 키 주입
    pub fn set_str(&self, key: &str, val: &str) {
        let mut s = self.strings.lock().unwrap();
        s.insert(key.to_string(), val.to_string());
    }

    /// 테스트용 시드 주입 (set_str 별칭)
    pub fn seed(&self, key: &str, val: &str) {
        self.set_str(key, val);
    }

    /// 테스트용 해시 키 주입
    pub fn set_hash(&self, key: &str, fields: HashMap<String, String>) {
        let mut h = self.hashes.lock().unwrap();
        h.insert(key.to_string(), fields);
    }
}

impl RedisCapability for MemoryRedisCapability {
    fn call<'a>(&'a self, principal: &'a CapabilityPrincipal, key: &'a str, input: &'a Value) -> CapabilityFuture<'a, Result<Value, Reject>> {
        Box::pin(async move {
            if !principal.permits("redis", key) {
                return Err(reject("WIRE.ACCESS_NOT_DECLARED", "Redis key capability가 확장에 선언되지 않음"));
            }
            // 1. 키 패턴/네임스페이스 검증
            if !self.allowed_prefixes.iter().any(|prefix| key.starts_with(prefix)) {
                return Err(Reject { code: "WIRE.CAPABILITY_DENIED", msg: format!("비인가 Redis 키 접근 거부: `{key}`") });
            }

            // 2. 명령어 검증 (읽기 전용 제한)
            let command = input["command"].as_str().unwrap_or("get");
            if !self.allowed_commands.contains(command) {
                return Err(Reject { code: "WIRE.CAPABILITY_DENIED", msg: format!("허용되지 않은 Redis 명령: `{command}`") });
            }

            match command {
                "get" => {
                    let s = self.strings.lock().unwrap();
                    let val = s.get(key).cloned();
                    Ok(json!({ "value": val }))
                }
                "hgetall" => {
                    let h = self.hashes.lock().unwrap();
                    let val = h.get(key).cloned().unwrap_or_default();
                    Ok(json!({ "value": val }))
                }
                "exists" => {
                    let s = self.strings.lock().unwrap();
                    let h = self.hashes.lock().unwrap();
                    let exists = s.contains_key(key) || h.contains_key(key);
                    Ok(json!({ "exists": exists }))
                }
                "mget" => {
                    let Some(keys) = input.get("keys").and_then(Value::as_array) else {
                        return Err(reject("WIRE.BAD_VALUE", "Redis mget keys는 배열이어야 함"));
                    };
                    if keys.is_empty() || keys.len() > 64 {
                        return Err(reject("WIRE.BAD_VALUE", "Redis mget 키 개수 범위 오류"));
                    }
                    let strings = self.strings.lock().unwrap();
                    let mut values = Vec::with_capacity(keys.len());
                    for candidate in keys {
                        let Some(candidate) = candidate.as_str() else {
                            return Err(reject("WIRE.BAD_VALUE", "Redis key는 문자열이어야 함"));
                        };
                        if !principal.permits("redis", candidate) || !self.allowed_prefixes.iter().any(|p| candidate.starts_with(p)) {
                            return Err(reject("WIRE.CAPABILITY_DENIED", "Redis mget에 허용되지 않은 키가 포함됨"));
                        }
                        values.push(strings.get(candidate).cloned().map(Value::String).unwrap_or(Value::Null));
                    }
                    Ok(json!({ "values": values }))
                }
                _ => Err(Reject { code: "WIRE.CAPABILITY_DENIED", msg: format!("미구현 Redis 명령: `{command}`") }),
            }
        })
    }
}

fn reject(code: &'static str, msg: &str) -> Reject {
    Reject { code, msg: msg.to_string() }
}

/// Real MongoDB read capability. The client's own per-server pool is bounded by `max_pool_size`.
#[derive(Clone)]
pub struct MongoDriverCapability {
    client: mongodb::Client,
    database: String,
    allowed_collections: HashSet<String>,
    allowed_fields: HashSet<String>,
    write_collections: HashSet<String>,
    max_docs: i64,
    max_bytes: usize,
    timeout: Duration,
}

impl MongoDriverCapability {
    pub async fn connect(uri: &str, database: &str, max_pool_size: u32, timeout: Duration) -> Result<Self, Reject> {
        if database.is_empty()
            || database.len() > 63
            || max_pool_size == 0
            || max_pool_size > 64
            || timeout.is_zero()
            || timeout > Duration::from_secs(30)
        {
            return Err(reject("WIRE.BAD_CONFIG", "Mongo capability 설정 범위 오류"));
        }
        let mut options =
            mongodb::options::ClientOptions::parse(uri).await.map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo 설정을 읽지 못함"))?;
        options.max_pool_size = Some(max_pool_size);
        options.connect_timeout = Some(timeout);
        options.server_selection_timeout = Some(timeout);
        let client = mongodb::Client::with_options(options).map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo client 초기화 실패"))?;
        tokio::time::timeout(timeout, client.database(database).run_command(mongodb::bson::doc! {"ping": 1}))
            .await
            .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo readiness timeout"))?
            .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo readiness 실패"))?;
        let allowed_collections =
            ["video_distribution", "youtube_watching_data", "video_timeline_emotion_count"].into_iter().map(str::to_string).collect();
        let allowed_fields =
            ["_id", "video_id", "user_id", "dominant_emotion", "created_at", "emotion_score_timeline", "emotion_percentages", "total_frames"]
                .into_iter()
                .map(str::to_string)
                .collect();
        Ok(Self {
            client,
            database: database.into(),
            allowed_collections,
            allowed_fields,
            write_collections: HashSet::new(),
            max_docs: 100,
            max_bytes: 1_048_576,
            timeout,
        })
    }

    fn validate_filter(&self, collection: &str, filter: &Value, principal: &CapabilityPrincipal) -> Result<mongodb::bson::Document, Reject> {
        let Some(object) = filter.as_object() else {
            return Err(reject("WIRE.BAD_VALUE", "Mongo filter는 객체여야 함"));
        };
        let mut filter_value = filter.clone();
        if collection == "youtube_watching_data" {
            let Some(actor) = principal.private_user_id() else {
                return Err(reject("WIRE.ACCESS_DENIED", "인증 principal이 필요함"));
            };
            if object.get("user_id").is_some_and(|v| v != &actor) {
                return Err(reject("WIRE.ACCESS_DENIED", "다른 user_id filter는 허용하지 않음"));
            }
            filter_value["user_id"] = actor;
        }
        fn validate(value: &Value, fields: &HashSet<String>, inside_field: bool) -> Result<(), Reject> {
            let Some(map) = value.as_object() else {
                return Err(reject("WIRE.BAD_VALUE", "Mongo filter 값 형식 오류"));
            };
            for (key, value) in map {
                if key.starts_with('$') {
                    if !inside_field || key != "$in" {
                        return Err(reject("WIRE.BAD_VALUE", "지원하지 않는 Mongo filter operator"));
                    }
                    let Some(items) = value.as_array() else {
                        return Err(reject("WIRE.BAD_VALUE", "$in은 배열이어야 함"));
                    };
                    if items.len() > 100 {
                        return Err(reject("WIRE.BAD_VALUE", "$in 개수 상한 초과"));
                    }
                } else {
                    if !fields.contains(key) {
                        return Err(reject("WIRE.BAD_VALUE", "Mongo filter field가 허용되지 않음"));
                    }
                    if value.is_object() {
                        validate(value, fields, true)?;
                    }
                }
            }
            Ok(())
        }
        validate(&filter_value, &self.allowed_fields, false)?;
        mongodb::bson::serialize_to_document(&filter_value).map_err(|_| reject("WIRE.BAD_VALUE", "Mongo filter 변환 실패"))
    }
}

/// Virtual collection served by [`MongoDriverCapability::video_timeline`]. It is a derived view over every viewer's
/// `youtube_watching_data.emotion_score_timeline`, so a grant to it never opens a raw session document.
pub const TIMELINE_COLLECTION: &str = "video_emotion_timeline";

pub const TIMELINE_EMOTIONS: [&str; 5] = ["neutral", "happy", "surprise", "sad", "angry"];
const TIMELINE_BUCKETS: usize = 100;
/// The original loads every session of the video; this port fails loudly instead of growing without bound.
const TIMELINE_MAX_DOCS: usize = 50_000;
const TIMELINE_MAX_ENTRIES: usize = 5_000_000;

/// One timeline key (centisecond offset) with the sum of every session's score per emotion and the number of sessions.
pub struct TimelineKey {
    pub key: String,
    pub sessions: u64,
    pub sums: [f64; 5],
}

/// Folds sessions into per-key sums the way `_get_compressed_timeline_data` does: keys in order of first appearance (sessions in
/// natural order, a session's keys in stored order) and each key's scores added one session at a time with plain float addition.
/// Mongo's own sum operator compensates its rounding error, which flips a bucket that sits exactly on a rounding boundary, so the
/// sums are not delegated to the server.
#[derive(Default)]
pub struct TimelineAccumulator {
    keys: Vec<TimelineKey>,
    index: HashMap<String, usize>,
    entries: usize,
}

impl TimelineAccumulator {
    pub fn add_session(&mut self, timeline: &mongodb::bson::Document) -> Result<(), Reject> {
        for (key, scores) in timeline {
            let mongodb::bson::Bson::Array(scores) = scores else { continue };
            if scores.len() < 5 {
                continue;
            }
            let mut values = [0.0_f64; 5];
            for (value, score) in values.iter_mut().zip(scores) {
                *value = match score {
                    mongodb::bson::Bson::Double(v) => *v,
                    mongodb::bson::Bson::Int32(v) => f64::from(*v),
                    mongodb::bson::Bson::Int64(v) => *v as f64,
                    _ => return Err(reject("WIRE.BAD_VALUE", "timeline 점수가 숫자가 아님")),
                };
            }
            self.entries += 1;
            if self.entries > TIMELINE_MAX_ENTRIES {
                return Err(reject("WIRE.BAD_VALUE", "timeline 항목 상한 초과"));
            }
            let position = match self.index.get(key) {
                Some(position) => *position,
                None => {
                    self.keys.push(TimelineKey { key: key.clone(), sessions: 0, sums: [0.0; 5] });
                    self.index.insert(key.clone(), self.keys.len() - 1);
                    self.keys.len() - 1
                }
            };
            let slot = &mut self.keys[position];
            for (sum, value) in slot.sums.iter_mut().zip(values) {
                *sum += value;
            }
            slot.sessions += 1;
        }
        Ok(())
    }

    pub fn into_keys(self) -> Vec<TimelineKey> {
        self.keys
    }
}

/// The original `WatchService._get_compressed_timeline_data` arithmetic: each key contributes its per-session average to the
/// bucket its centisecond offset falls in (`duration` seconds split into 100 buckets), and a bucket reports the mean of its keys'
/// averages rounded to one decimal. `None` is the original's "default timeline" (no usable key at all). Keys that are not a finite
/// non-negative number are skipped (the original skips unparsable keys and would crash or wrap on the other odd ones).
pub fn compress_timeline(keys: &[TimelineKey], duration: Option<i64>) -> Option<Value> {
    if keys.is_empty() {
        return None;
    }
    let total_cs = match duration {
        Some(d) if d != 0 => d as f64 * 100.0,
        _ => 100.0,
    };
    let bucket_size = total_cs / TIMELINE_BUCKETS as f64;
    let mut sums = vec![[0.0_f64; 5]; TIMELINE_BUCKETS];
    let mut counts = vec![0_u32; TIMELINE_BUCKETS];
    for key in keys {
        let Some(cs) = key.key.trim().parse::<f64>().ok().filter(|cs| cs.is_finite() && *cs >= 0.0) else { continue };
        let index = ((cs / bucket_size) as usize).min(TIMELINE_BUCKETS - 1);
        for (sum, total) in sums[index].iter_mut().zip(key.sums) {
            *sum += total / key.sessions as f64;
        }
        counts[index] += 1;
    }
    let mut lists = serde_json::Map::new();
    for (i, emotion) in TIMELINE_EMOTIONS.iter().enumerate() {
        let points: Vec<Value> = (0..TIMELINE_BUCKETS)
            .map(|x| {
                let y = if counts[x] > 0 { round_one_decimal(sums[x][i] / f64::from(counts[x])) } else { 0.0 };
                json!({"x": x + 1, "y": y})
            })
            .collect();
        lists.insert((*emotion).to_string(), Value::Array(points));
    }
    Some(Value::Object(lists))
}

/// Python `round(x, 1)`: both round the exact binary value half to even, which `{:.1}` formatting does too.
fn round_one_decimal(value: f64) -> f64 {
    format!("{value:.1}").parse().unwrap_or(0.0)
}

impl MongoDriverCapability {
    /// Server-side aggregate of all viewers' timelines of one video (original `GET watch/video` timeline). The caller names the
    /// video and its duration and receives only the 100-bucket averages; no session document, user id or per-session value leaves.
    async fn video_timeline(&self, input: &Value) -> Result<Value, Reject> {
        let allowed = ["command", "video_id", "duration"];
        if input.as_object().is_none_or(|m| m.keys().any(|k| !allowed.contains(&k.as_str()))) || input["command"] != "timeline" {
            return Err(reject("WIRE.BAD_VALUE", "timeline 입력이 허용되지 않음"));
        }
        let Some(video_id) = input.get("video_id").and_then(Value::as_str).filter(|v| !v.is_empty() && v.len() <= 64) else {
            return Err(reject("WIRE.BAD_VALUE", "timeline video_id는 문자열이어야 함"));
        };
        let duration = match input.get("duration") {
            None | Some(Value::Null) => None,
            Some(v) if v.as_i64().is_some_and(|n| (0..=1_000_000).contains(&n)) => v.as_i64(),
            _ => return Err(reject("WIRE.BAD_VALUE", "timeline duration 범위 오류")),
        };
        let coll = self.client.database(&self.database).collection::<mongodb::bson::Document>("youtube_watching_data");
        let keys = tokio::time::timeout(self.timeout, async {
            use futures_util::StreamExt;
            let mut cursor = coll
                .find(mongodb::bson::doc! {"video_id": video_id})
                .projection(mongodb::bson::doc! {"emotion_score_timeline": 1, "_id": 0})
                .await
                .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo timeline 읽기 실패"))?;
            let mut accumulator = TimelineAccumulator::default();
            let mut sessions = 0usize;
            while let Some(doc) = cursor.next().await.transpose().map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo cursor 실패"))? {
                sessions += 1;
                if sessions > TIMELINE_MAX_DOCS {
                    return Err(reject("WIRE.BAD_VALUE", "timeline 세션 상한 초과"));
                }
                if let Ok(timeline) = doc.get_document("emotion_score_timeline") {
                    accumulator.add_session(timeline)?;
                }
            }
            Ok(accumulator.into_keys())
        })
        .await
        .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo timeline timeout"))??;
        Ok(json!({"timeline": compress_timeline(&keys, duration)}))
    }
}

/// Virtual collection served by [`MongoDriverCapability::distribution_health`] (WBS 7.2.9, the content health of the administrator
/// dashboard). It reduces every `video_distribution` document that has frames to the averages and the dominant emotion counts the
/// dashboard shows, so a grant to it never opens a raw document. The command takes no argument; who may ask is decided by the extension
/// that holds the grant (an administrator-only aggregate it must be able to call first).
pub const HEALTH_COLLECTION: &str = "video_distribution_stats";

/// Python `round(value, digits)`: the exact binary value rounded half to even, which is what formatting the exact expansion gives.
fn py_round(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}").parse().unwrap_or(value)
}

fn py_round4(value: f64) -> f64 {
    py_round(value, 4)
}

impl MongoDriverCapability {
    /// The original `AdminService._get_content_health` Mongo part: the same `$facet` pipeline run by the server (so Mongo's own
    /// averaging is used), rounded like the original. Dominant emotions with equal counts are ordered by name (the original leaves the
    /// order of equal counts to Mongo).
    async fn distribution_health(&self, input: &Value) -> Result<Value, Reject> {
        if input != &json!({"command": "health"}) {
            return Err(reject("WIRE.BAD_VALUE", "health 입력이 허용되지 않음"));
        }
        use mongodb::bson::{doc, Bson};
        let coll = self.client.database(&self.database).collection::<mongodb::bson::Document>("video_distribution");
        let pipeline = vec![
            doc! {"$match": {"total_frames": {"$gt": 0}}},
            doc! {"$facet": {
                "averages": [{"$group": {
                    "_id": Bson::Null,
                    "avg_completion_rate": {"$avg": "$average_completion_rate"},
                    "avg_neutral": {"$avg": "$emotion_averages.neutral"},
                    "avg_happy": {"$avg": "$emotion_averages.happy"},
                    "avg_surprise": {"$avg": "$emotion_averages.surprise"},
                    "avg_sad": {"$avg": "$emotion_averages.sad"},
                    "avg_angry": {"$avg": "$emotion_averages.angry"},
                }}],
                "dominant_counts": [
                    {"$group": {"_id": "$dominant_emotion", "count": {"$sum": 1}}},
                    {"$sort": {"count": -1, "_id": 1}},
                ],
            }},
        ];
        let facet = tokio::time::timeout(self.timeout, async {
            use futures_util::StreamExt;
            let mut cursor = coll.aggregate(pipeline).await.map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo health 읽기 실패"))?;
            cursor.next().await.transpose().map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo cursor 실패"))
        })
        .await
        .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo health timeout"))??
        .unwrap_or_default();
        let averages = facet.get_array("averages").ok().and_then(|rows| rows.first()).and_then(Bson::as_document).cloned().unwrap_or_default();
        let average = |name: &str| py_round4(averages.get(name).and_then(bson_number).unwrap_or(0.0));
        let mut dominant = Vec::new();
        for row in facet.get_array("dominant_counts").map(|rows| rows.as_slice()).unwrap_or_default() {
            let Some(row) = row.as_document() else { continue };
            let count = match row.get("count") {
                Some(Bson::Int32(n)) => i64::from(*n),
                Some(Bson::Int64(n)) => *n,
                _ => continue,
            };
            // The original skips a document whose dominant emotion is missing or empty.
            if let Some(emotion) = row.get_str("_id").ok().filter(|emotion| !emotion.is_empty()) {
                dominant.push(json!({"emotion": emotion, "video_count": count}));
            }
        }
        Ok(json!({
            "avg_completion_rate": average("avg_completion_rate"),
            "emotion_distribution": {"neutral": average("avg_neutral"), "happy": average("avg_happy"), "surprise": average("avg_surprise"),
                "sad": average("avg_sad"), "angry": average("avg_angry")},
            "dominant": dominant,
        }))
    }
}

/// Virtual collection served by [`MongoDriverCapability::server_status`] (WBS 7.2.9, the system status of the administrator dashboard).
/// It answers an allowlist of server figures and nothing else: `host` the CPU, memory and disk usage of the machine this server runs
/// on (Linux; `null` elsewhere), `ping` whether the MongoDB deployment answers. Who may ask is decided by the extension that holds the
/// grant (an administrator-only aggregate it must be able to call first).
pub const SERVER_COLLECTION: &str = "server_status";

/// The busy and the total CPU time of the first line of `/proc/stat`, the way psutil counts them on Linux: the total leaves out the
/// guest times (already inside user and nice), the busy time leaves out idle and iowait.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn cpu_busy_total(stat: &str) -> Option<(f64, f64)> {
    let line = stat.lines().find(|line| line.starts_with("cpu "))?;
    let mut ticks = [0.0_f64; 10];
    for (slot, field) in ticks.iter_mut().zip(line.split_whitespace().skip(1)) {
        *slot = field.parse().ok()?;
    }
    let total = ticks.iter().sum::<f64>() - ticks[8] - ticks[9];
    Some((total - ticks[3] - ticks[4], total))
}

/// psutil `cpu_percent` between two samples: busy share of the elapsed CPU time, 0 to 100, one decimal.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn cpu_percent(before: (f64, f64), after: (f64, f64)) -> f64 {
    let (busy, total) = (after.0 - before.0, after.1 - before.1);
    if total <= 0.0 {
        return 0.0;
    }
    py_round((busy / total * 100.0).clamp(0.0, 100.0), 1)
}

/// psutil `virtual_memory`: the used share `(total - available) / total` and the total in MiB, one decimal each.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn memory_figures(meminfo: &str) -> Option<(f64, f64)> {
    let kilobytes = |name: &str| -> Option<f64> {
        let line = meminfo.lines().find(|line| line.starts_with(name) && line[name.len()..].starts_with(':'))?;
        line[name.len() + 1..].split_whitespace().next()?.parse::<f64>().ok()
    };
    let (total, available) = (kilobytes("MemTotal")? * 1024.0, kilobytes("MemAvailable")? * 1024.0);
    if total <= 0.0 {
        return None;
    }
    Some((py_round((total - available) / total * 100.0, 1), py_round(total / (1024.0 * 1024.0), 1)))
}

/// psutil `disk_usage(...).percent`: used over what a user can still reach plus what is used, one decimal.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn disk_percent(blocks: f64, free: f64, available: f64, fragment: f64) -> f64 {
    let used = (blocks - free) * fragment;
    let reachable = used + available * fragment;
    if reachable <= 0.0 {
        return 0.0;
    }
    py_round(used / reachable * 100.0, 1)
}

impl MongoDriverCapability {
    async fn server_status(&self, input: &Value) -> Result<Value, Reject> {
        match input.get("command").and_then(Value::as_str) {
            Some("ping") if input.as_object().is_some_and(|m| m.len() == 1) => {
                let db = self.client.database(&self.database);
                tokio::time::timeout(self.timeout, db.run_command(mongodb::bson::doc! {"ping": 1}))
                    .await
                    .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo ping timeout"))?
                    .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo ping 실패"))?;
                Ok(json!({"ok": true}))
            }
            Some("host") if input.as_object().is_some_and(|m| m.len() == 1) => Ok(host_figures().await),
            _ => Err(reject("WIRE.BAD_VALUE", "알 수 없는 server_status command")),
        }
    }
}

#[cfg(target_os = "linux")]
async fn host_figures() -> Value {
    let before = tokio::fs::read_to_string("/proc/stat").await.ok().as_deref().and_then(cpu_busy_total);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let after = tokio::fs::read_to_string("/proc/stat").await.ok().as_deref().and_then(cpu_busy_total);
    let cpu = before.zip(after).map(|(before, after)| cpu_percent(before, after));
    let memory = tokio::fs::read_to_string("/proc/meminfo").await.ok().as_deref().and_then(memory_figures);
    // SAFETY: statvfs fills the zeroed struct for a NUL-terminated path; the result is only read when the call succeeds.
    let disk = unsafe {
        let mut stats: libc::statvfs = std::mem::zeroed();
        (libc::statvfs(c"/".as_ptr(), &mut stats) == 0)
            .then(|| disk_percent(stats.f_blocks as f64, stats.f_bfree as f64, stats.f_bavail as f64, stats.f_frsize as f64))
    };
    json!({"cpu_usage": cpu, "memory_usage": memory.map(|m| m.0), "memory_total_mb": memory.map(|m| m.1), "disk_usage": disk})
}

/// Only Linux has the counters this reads; elsewhere the figures are `null` rather than another system's numbers.
#[cfg(not(target_os = "linux"))]
async fn host_figures() -> Value {
    json!({"cpu_usage": null, "memory_usage": null, "memory_total_mb": null, "disk_usage": null})
}

/// Virtual collection served by [`MongoDriverCapability::video_session_stats`] (WBS 7.2.9, the dummy data of the administrator). It adds up
/// the sessions of every viewer of one video the way `WatchingDataService._update_video_distribution` reads them, so a job can recompute the
/// video's distribution without the sessions themselves (private to their viewers) leaving: only the sums come back.
pub const SESSION_STATS_COLLECTION: &str = "video_session_stats";
/// The original reads the 1000 newest sessions of the video.
const SESSION_STATS_LIMIT: i64 = 1000;

impl MongoDriverCapability {
    async fn video_session_stats(&self, input: &Value) -> Result<Value, Reject> {
        let allowed = ["command", "video_id"];
        if input.as_object().is_none_or(|m| m.keys().any(|k| !allowed.contains(&k.as_str()))) || input["command"] != "distribution_sums" {
            return Err(reject("WIRE.BAD_VALUE", "distribution_sums 입력이 허용되지 않음"));
        }
        let Some(video_id) = input.get("video_id").and_then(Value::as_str).filter(|v| !v.is_empty() && v.len() <= 64) else {
            return Err(reject("WIRE.BAD_VALUE", "distribution_sums video_id는 문자열이어야 함"));
        };
        let coll = self.client.database(&self.database).collection::<mongodb::bson::Document>("youtube_watching_data");
        tokio::time::timeout(self.timeout, async {
            use futures_util::StreamExt;
            let mut cursor = coll
                .find(mongodb::bson::doc! {"video_id": video_id})
                .sort(mongodb::bson::doc! {"created_at": -1})
                .limit(SESSION_STATS_LIMIT)
                .projection(mongodb::bson::doc! {"emotion_percentages": 1, "completion_rate": 1, "emotion_score_timeline": 1, "_id": 0})
                .await
                .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo session stats 읽기 실패"))?;
            let (mut count, mut sums, mut completion, mut frames) = (0u64, [0.0_f64; 5], 0.0_f64, 0u64);
            let bad = || reject("WIRE.BAD_VALUE", "세션의 감정 비율 또는 완주율이 숫자가 아님");
            while let Some(doc) = cursor.next().await.transpose().map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo cursor 실패"))? {
                count += 1;
                // The original adds float(x) in this order; a missing value is the dataclass default 0.0 and a non-number fails the whole update.
                let percentages = match doc.get("emotion_percentages") {
                    None | Some(mongodb::bson::Bson::Null) => None,
                    Some(mongodb::bson::Bson::Document(percentages)) => Some(percentages),
                    Some(_) => return Err(bad()),
                };
                for (sum, emotion) in sums.iter_mut().zip(STATS_EMOTIONS) {
                    if let Some(value) = percentages.and_then(|p| p.get(emotion)) {
                        *sum += bson_number(value).ok_or_else(bad)?;
                    }
                }
                completion += match doc.get("completion_rate") {
                    None => 0.0,
                    Some(value) => bson_number(value).ok_or_else(bad)?,
                };
                if let Ok(timeline) = doc.get_document("emotion_score_timeline") {
                    frames += timeline.len() as u64;
                }
            }
            Ok(json!({"count": count, "sums": {"neutral": sums[0], "happy": sums[1], "surprise": sums[2], "sad": sums[3], "angry": sums[4]},
                "completion_sum": completion, "frames": frames}))
        })
        .await
        .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo session stats timeout"))?
    }
}

/// Virtual collection served by [`MongoDriverCapability::user_watch_stats`] (WBS 7.2.8). It reduces the caller's own
/// `youtube_watching_data` sessions, whose per-frame timelines are too large to hand to an extension, to the figures the my page
/// screens need. A grant to it never opens a raw session document, and every command reads only the verified caller's sessions.
pub const USER_STATS_COLLECTION: &str = "user_watch_stats";

const STATS_EMOTIONS: [&str; 5] = ["neutral", "happy", "surprise", "sad", "angry"];
const STATS_MAX_DOCS: usize = 20_000;
const STATS_MAX_ROWS: usize = 20_000;
const COMPRESS_BUCKETS: usize = 40;
const PEAK_THRESHOLD: f64 = 80.0;
const WINDOW_FRAMES: usize = 60;

/// Python `float(x)` of a stored number; anything else is not a score.
fn bson_number(value: &mongodb::bson::Bson) -> Option<f64> {
    match value {
        mongodb::bson::Bson::Double(v) => Some(*v),
        mongodb::bson::Bson::Int32(v) => Some(f64::from(*v)),
        mongodb::bson::Bson::Int64(v) => Some(*v as f64),
        _ => None,
    }
}

/// The original `_extract_emotion_scores`: the object form (missing emotions are 0) or the old array form, in the order
/// neutral, happy, surprise, sad, angry.
fn extract_scores(scores: &mongodb::bson::Bson) -> Result<[f64; 5], Reject> {
    let bad = || reject("WIRE.BAD_VALUE", "감정 점수가 숫자가 아님");
    let mut result = [0.0_f64; 5];
    match scores {
        mongodb::bson::Bson::Document(map) => {
            for (slot, emotion) in result.iter_mut().zip(STATS_EMOTIONS) {
                if let Some(value) = map.get(emotion) {
                    *slot = bson_number(value).ok_or_else(bad)?;
                }
            }
        }
        mongodb::bson::Bson::Array(list) => {
            for (slot, value) in result.iter_mut().zip(list) {
                *slot = bson_number(value).ok_or_else(bad)?;
            }
        }
        _ => {}
    }
    Ok(result)
}

/// `datetime.isoformat()` of a naive UTC datetime stored with millisecond precision.
fn iso_of(millis: i64) -> String {
    let days = millis.div_euclid(86_400_000);
    let rest = millis.rem_euclid(86_400_000);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year_of_era = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + i64::from(month <= 2);
    let (hour, minute, second, ms) = (rest / 3_600_000, rest / 60_000 % 60, rest / 1000 % 60, rest % 1000);
    let base = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}");
    if ms == 0 {
        base
    } else {
        format!("{base}.{ms:03}000")
    }
}

fn millis_of(doc: &mongodb::bson::Document, field: &str) -> Option<i64> {
    doc.get_datetime(field).ok().map(|date| date.timestamp_millis())
}

/// Python `int(text)` for the keys of a timeline (whitespace and a sign are allowed, nothing else).
fn python_int(text: &str) -> Option<i64> {
    text.trim().parse::<i64>().ok()
}

/// Peak moments of one session: the strongest frame of each 60-frame window, kept when it reaches the threshold (the original
/// `get_moments` loop). A session whose keys are not all integers yields none, as the original skips it.
fn session_moments(timeline: &mongodb::bson::Document) -> Result<Vec<(i64, usize, f64)>, Reject> {
    let mut frames: Vec<(i64, &mongodb::bson::Bson)> = Vec::with_capacity(timeline.len());
    for (key, scores) in timeline {
        match python_int(key) {
            Some(offset) => frames.push((offset, scores)),
            None => return Ok(Vec::new()),
        }
    }
    frames.sort_by_key(|(offset, _)| *offset);
    let mut found = Vec::new();
    let mut best: Option<(f64, i64, usize)> = None;
    let mut window_start = 0usize;
    for (index, (offset, scores)) in frames.iter().enumerate() {
        let values = extract_scores(scores)?;
        let mut peak = 0usize;
        for (slot, value) in values.iter().enumerate() {
            if *value > values[peak] {
                peak = slot;
            }
        }
        let score = values[peak];
        if index - window_start >= WINDOW_FRAMES {
            if let Some((best_score, best_offset, best_emotion)) = best.take().filter(|(s, _, _)| *s >= PEAK_THRESHOLD) {
                found.push((best_offset, best_emotion, best_score));
            }
            window_start = index;
            best = None;
        }
        if score >= PEAK_THRESHOLD && best.is_none_or(|(held, _, _)| score > held) {
            best = Some((score, *offset, peak));
        }
    }
    if let Some((best_score, best_offset, best_emotion)) = best.filter(|(s, _, _)| *s >= PEAK_THRESHOLD) {
        found.push((best_offset, best_emotion, best_score));
    }
    Ok(found)
}

/// Python `float(x)` of a stored value for the summary rules: numbers, booleans and numeric text. `None` is the original's
/// `TypeError`/`ValueError`, which each rule catches or lets fail as its own code does.
fn py_float(value: Option<&mongodb::bson::Bson>) -> Option<f64> {
    use mongodb::bson::Bson;
    match value? {
        Bson::Double(v) => Some(*v),
        Bson::Int32(v) => Some(f64::from(*v)),
        Bson::Int64(v) => Some(*v as f64),
        Bson::Boolean(v) => Some(f64::from(u8::from(*v))),
        Bson::String(text) => text.trim().parse::<f64>().ok().filter(|v| v.is_finite()),
        _ => None,
    }
}

/// Python `int(x)` of a stored value (truncation of numbers); anything else counts as 0 like the original's `except`.
fn py_int_or_zero(value: Option<&mongodb::bson::Bson>) -> i64 {
    py_float(value).map_or(0, |v| v as i64)
}

/// The original `_estimate_watch_seconds_from_summary` for a session document without its frame timelines.
fn estimated_watch_seconds(doc: &mongodb::bson::Document, with_timeline_len: bool) -> f64 {
    if let (Some(duration), Some(completion)) = (py_float(doc.get("duration")), py_float(doc.get("completion_rate"))) {
        if duration > 0.0 && completion > 0.0 {
            return (duration * completion.min(1.0)).max(0.0);
        }
    }
    if with_timeline_len {
        let length = py_int_or_zero(doc.get("timeline_len"));
        if length > 0 {
            return length as f64 / 2.0;
        }
    }
    let frames = py_int_or_zero(doc.get("frame_count"));
    if frames > 0 {
        return frames as f64 / 2.0;
    }
    0.0
}

/// `datetime.fromisoformat` of the UTC text the service writes for a `Time` field (`Z`, `+00:00` or no zone), as milliseconds since the epoch.
fn parse_iso_millis(text: &str) -> Option<i64> {
    let text = text.strip_suffix("+00:00").or_else(|| text.strip_suffix('Z')).unwrap_or(text);
    let (date, time) = text.split_once('T').or_else(|| text.split_once(' '))?;
    let mut ymd = date.split('-');
    let (year, month, day): (i64, i64, i64) = (ymd.next()?.parse().ok()?, ymd.next()?.parse().ok()?, ymd.next()?.parse().ok()?);
    if ymd.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, fraction) = time.split_once('.').map_or((time, ""), |(clock, fraction)| (clock, fraction));
    let mut hms = clock.split(':');
    let (hour, minute, second): (i64, i64, i64) = (hms.next()?.parse().ok()?, hms.next()?.parse().ok()?, hms.next().unwrap_or("0").parse().ok()?);
    if hms.next().is_some() || hour > 23 || minute > 59 || second > 59 || !fraction.bytes().all(|b| b.is_ascii_digit()) || fraction.len() > 9 {
        return None;
    }
    let millis: i64 = format!("{fraction:0<3}")[..3].parse().ok()?;
    // Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm, the inverse of `iso_of`).
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000 + millis)
}

impl MongoDriverCapability {
    async fn user_watch_stats(&self, principal: &CapabilityPrincipal, input: &Value) -> Result<Value, Reject> {
        let Some(user) = principal.private_user_id() else {
            return Err(reject("WIRE.ACCESS_DENIED", "인증 principal이 필요함"));
        };
        let Some(command) = input.get("command").and_then(Value::as_str) else {
            return Err(reject("WIRE.BAD_VALUE", "stats command 누락"));
        };
        let allowed: &[&str] = match command {
            "sessions" => &["command", "emotion", "skip", "limit"],
            "compress" => &["command", "items"],
            "calendar" => &["command", "start", "end"],
            "moments" => &["command", "limit"],
            "dna" | "highlight" => &["command"],
            "emotion_seconds" => &["command", "after"],
            _ => return Err(reject("WIRE.BAD_VALUE", "알 수 없는 stats command")),
        };
        if input.as_object().is_none_or(|m| m.keys().any(|k| !allowed.contains(&k.as_str()))) {
            return Err(reject("WIRE.BAD_VALUE", "stats 입력 field가 허용되지 않음"));
        }
        let user = user.as_str().map(str::to_owned).ok_or_else(|| reject("WIRE.ACCESS_DENIED", "principal id 형식 오류"))?;
        let coll = self.client.database(&self.database).collection::<mongodb::bson::Document>("youtube_watching_data");
        let unavailable = || reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo stats 읽기 실패");
        let uint = |name: &str, max: u64, default: u64| -> Result<u64, Reject> {
            match input.get(name) {
                None => Ok(default),
                Some(v) if v.as_u64().is_some_and(|n| n <= max) => Ok(v.as_u64().unwrap_or(default)),
                _ => Err(reject("WIRE.BAD_VALUE", "stats 숫자 범위 오류")),
            }
        };
        tokio::time::timeout(self.timeout, async {
            use futures_util::StreamExt;
            match command {
                "sessions" => {
                    let (skip, limit) = (uint("skip", 100_000, 0)?, uint("limit", 51, 11)?);
                    let mut filter = mongodb::bson::doc! {"user_id": &user};
                    match input.get("emotion") {
                        None | Some(Value::Null) => {}
                        Some(Value::String(emotion)) if STATS_EMOTIONS.contains(&emotion.as_str()) => {
                            filter.insert("dominant_emotion", emotion.as_str());
                        }
                        _ => return Err(reject("WIRE.BAD_VALUE", "stats emotion 오류")),
                    }
                    let mut cursor = coll
                        .find(filter)
                        .projection(mongodb::bson::doc! {"video_id": 1, "dominant_emotion": 1, "emotion_percentages": 1, "created_at": 1})
                        .sort(mongodb::bson::doc! {"created_at": -1})
                        .skip(skip)
                        .limit(limit as i64)
                        .await
                        .map_err(|_| unavailable())?;
                    let mut rows = Vec::new();
                    while let Some(doc) = cursor.next().await.transpose().map_err(|_| unavailable())? {
                        let percentages: Value = doc
                            .get_document("emotion_percentages")
                            .ok()
                            .and_then(|map| mongodb::bson::deserialize_from_document(map.clone()).ok())
                            .unwrap_or(Value::Null);
                        rows.push(json!({
                            "session": doc.get_object_id("_id").map(|id| id.to_hex()).unwrap_or_default(),
                            "video_id": doc.get_str("video_id").unwrap_or_default(),
                            "dominant_emotion": doc.get_str("dominant_emotion").ok(),
                            "emotion_percentages": percentages,
                            "created_at": millis_of(&doc, "created_at").map(iso_of),
                        }));
                    }
                    Ok(json!({"sessions": rows}))
                }
                "compress" => {
                    let Some(items) = input.get("items").and_then(Value::as_array).filter(|items| items.len() <= 51) else {
                        return Err(reject("WIRE.BAD_VALUE", "stats items 오류"));
                    };
                    let mut out = Vec::new();
                    for item in items {
                        let (Some(session), duration) = (item.get("session").and_then(Value::as_str), item.get("duration").and_then(Value::as_i64)) else {
                            return Err(reject("WIRE.BAD_VALUE", "stats item 오류"));
                        };
                        let id = mongodb::bson::oid::ObjectId::parse_str(session).map_err(|_| reject("WIRE.BAD_VALUE", "stats session id 오류"))?;
                        let doc = coll
                            .find_one(mongodb::bson::doc! {"_id": id, "user_id": &user})
                            .projection(mongodb::bson::doc! {"emotion_score_timeline": 1})
                            .await
                            .map_err(|_| unavailable())?;
                        let timeline = doc.as_ref().and_then(|doc| doc.get_document("emotion_score_timeline").ok());
                        let usable = duration.filter(|d| *d != 0);
                        let (Some(timeline), Some(duration)) = (timeline.filter(|t| !t.is_empty()), usable) else {
                            out.push(Value::Null);
                            continue;
                        };
                        let bucket_size = duration as f64 * 100.0 / COMPRESS_BUCKETS as f64;
                        let mut sums = vec![[0.0_f64; 5]; COMPRESS_BUCKETS];
                        let mut counts = vec![0_u64; COMPRESS_BUCKETS];
                        for (key, scores) in timeline {
                            let Some(cs) = key.trim().parse::<f64>().ok().filter(|cs| cs.is_finite()) else { continue };
                            let position = (cs / bucket_size).trunc() as i64;
                            // Python indexes a list with a negative number from the end.
                            let position = if position < 0 { position + COMPRESS_BUCKETS as i64 } else { position.min(COMPRESS_BUCKETS as i64 - 1) };
                            let Some(index) = usize::try_from(position).ok().filter(|i| *i < COMPRESS_BUCKETS) else {
                                return Err(reject("WIRE.BAD_VALUE", "timeline 키가 범위를 벗어남"));
                            };
                            let values = extract_scores(scores)?;
                            for (sum, value) in sums[index].iter_mut().zip(values) {
                                *sum += value;
                            }
                            counts[index] += 1;
                        }
                        out.push(json!({"sums": sums, "counts": counts}));
                    }
                    Ok(json!({"timelines": out}))
                }
                "calendar" => {
                    let bound = |name: &str| -> Result<mongodb::bson::DateTime, Reject> {
                        input
                            .get(name)
                            .and_then(Value::as_i64)
                            .map(mongodb::bson::DateTime::from_millis)
                            .ok_or_else(|| reject("WIRE.BAD_VALUE", "stats 기간 오류"))
                    };
                    let filter = mongodb::bson::doc! {"user_id": &user, "created_at": {"$gte": bound("start")?, "$lt": bound("end")?}};
                    let mut cursor = coll
                        .find(filter)
                        .projection(mongodb::bson::doc! {"dominant_emotion": 1, "emotion_percentages": 1, "created_at": 1, "emotion_score_timeline": 1})
                        .await
                        .map_err(|_| unavailable())?;
                    let mut rows = Vec::new();
                    while let Some(doc) = cursor.next().await.transpose().map_err(|_| unavailable())? {
                        if rows.len() >= STATS_MAX_ROWS {
                            return Err(reject("WIRE.BAD_VALUE", "stats 행 상한 초과"));
                        }
                        let dominant = doc.get_str("dominant_emotion").unwrap_or("neutral").to_owned();
                        let intensity =
                            doc.get_document("emotion_percentages").ok().and_then(|map| map.get(&dominant)).and_then(bson_number).unwrap_or(0.0);
                        let length = doc.get_document("emotion_score_timeline").map(|t| t.len()).unwrap_or(0);
                        rows.push(json!({"created_at": millis_of(&doc, "created_at").map(iso_of), "dominant": dominant, "intensity": intensity, "timeline_len": length}));
                    }
                    Ok(json!({"rows": rows}))
                }
                "moments" => {
                    let limit = uint("limit", 200, 200)?;
                    let mut cursor = coll
                        .find(mongodb::bson::doc! {"user_id": &user})
                        .projection(mongodb::bson::doc! {"video_id": 1, "created_at": 1, "emotion_score_timeline": 1})
                        .sort(mongodb::bson::doc! {"created_at": -1})
                        .limit(limit as i64)
                        .await
                        .map_err(|_| unavailable())?;
                    let mut sessions = Vec::new();
                    let mut total = 0usize;
                    while let Some(doc) = cursor.next().await.transpose().map_err(|_| unavailable())? {
                        let moments = match doc.get_document("emotion_score_timeline") {
                            Ok(timeline) => session_moments(timeline)?,
                            Err(_) => Vec::new(),
                        };
                        total += moments.len();
                        if total > STATS_MAX_ROWS {
                            return Err(reject("WIRE.BAD_VALUE", "stats 모먼트 상한 초과"));
                        }
                        let list: Vec<Value> = moments.iter().map(|(offset, emotion, score)| json!({"offset": offset, "emotion": STATS_EMOTIONS[*emotion], "score": score})).collect();
                        sessions.push(json!({"video_id": doc.get_str("video_id").unwrap_or_default(), "created_at": millis_of(&doc, "created_at").map(iso_of), "moments": list}));
                    }
                    Ok(json!({"sessions": sessions}))
                }
                "highlight" => {
                    // The original's `get_highlight` reads the latest 500 sessions with only the video and the shares.
                    let mut cursor = coll
                        .find(mongodb::bson::doc! {"user_id": &user})
                        .projection(mongodb::bson::doc! {"video_id": 1, "emotion_percentages": 1})
                        .sort(mongodb::bson::doc! {"created_at": -1})
                        .limit(500)
                        .await
                        .map_err(|_| unavailable())?;
                    let mut rows = Vec::new();
                    while let Some(doc) = cursor.next().await.transpose().map_err(|_| unavailable())? {
                        let percentages: Value = doc
                            .get_document("emotion_percentages")
                            .ok()
                            .and_then(|map| mongodb::bson::deserialize_from_document(map.clone()).ok())
                            .unwrap_or(Value::Null);
                        rows.push(json!({"video_id": doc.get_str("video_id").unwrap_or_default(), "emotion_percentages": percentages}));
                    }
                    Ok(json!({"sessions": rows}))
                }
                "dna" => {
                    let mut cursor = coll
                        .find(mongodb::bson::doc! {"user_id": &user})
                        .projection(mongodb::bson::doc! {"video_id": 1, "created_at": 1, "completion_rate": 1, "emotion_percentages": 1})
                        .await
                        .map_err(|_| unavailable())?;
                    let (mut count, mut night, mut completion) = (0u64, 0u64, 0.0_f64);
                    let mut sums = [0.0_f64; 5];
                    let mut videos: Vec<String> = Vec::new();
                    let mut seen = HashSet::new();
                    while let Some(doc) = cursor.next().await.transpose().map_err(|_| unavailable())? {
                        count += 1;
                        if count as usize > STATS_MAX_DOCS {
                            return Err(reject("WIRE.BAD_VALUE", "stats 세션 상한 초과"));
                        }
                        if let Ok(percentages) = doc.get_document("emotion_percentages") {
                            for (sum, emotion) in sums.iter_mut().zip(STATS_EMOTIONS) {
                                if let Some(value) = percentages.get(emotion) {
                                    *sum += bson_number(value).ok_or_else(|| reject("WIRE.BAD_VALUE", "감정 비율이 숫자가 아님"))?;
                                }
                            }
                        }
                        completion += doc.get("completion_rate").and_then(bson_number).unwrap_or(0.0);
                        if let Some(millis) = millis_of(&doc, "created_at") {
                            let hour = millis.rem_euclid(86_400_000) / 3_600_000;
                            if hour >= 22 || hour <= 3 {
                                night += 1;
                            }
                        }
                        if let Ok(video) = doc.get_str("video_id") {
                            if seen.insert(video.to_owned()) {
                                videos.push(video.to_owned());
                            }
                        }
                    }
                    Ok(json!({"count": count, "night": night, "completion": completion, "sums": sums, "videos": videos}))
                }
                "emotion_seconds" => {
                    // The original `get_emotion_summary`: without a checkpoint the whole history is summed (its timelines, once), with one
                    // only the sessions finalized after it (their stored `emotion_seconds`), oldest first, so the last one is the new
                    // checkpoint. The sums stay unrounded: the original truncates the total, not each session.
                    let after = match input.get("after") {
                        None | Some(Value::Null) => None,
                        Some(Value::Object(after)) if after.keys().all(|k| k == "at" || k == "session") => {
                            let at = after.get("at").and_then(Value::as_str).and_then(parse_iso_millis);
                            let session = after.get("session").and_then(Value::as_str);
                            match (at, session) {
                                (Some(at), Some(session)) if session.len() <= 64 => Some((at, session.to_owned())),
                                _ => return Err(reject("WIRE.BAD_VALUE", "stats after 오류")),
                            }
                        }
                        _ => return Err(reject("WIRE.BAD_VALUE", "stats after 오류")),
                    };
                    let mut sums = [0.0_f64; 5];
                    let mut count = 0u64;
                    let mut last: Option<(i64, String)> = None;
                    let bad = |message: &'static str| reject("WIRE.BAD_VALUE", message);
                    let mut cursor = match &after {
                        None => {
                            coll.find(mongodb::bson::doc! {"user_id": &user})
                                .projection(mongodb::bson::doc! {"most_emotion_timeline": 1, "emotion_percentages": 1, "duration": 1, "completion_rate": 1, "frame_count": 1})
                                .await
                        }
                        Some((at, session)) => {
                            let at = mongodb::bson::DateTime::from_millis(*at);
                            coll.find(mongodb::bson::doc! {"user_id": &user, "$or": [
                                {"finalized_at": {"$gt": at}},
                                {"finalized_at": at, "video_view_log_id": {"$gt": session}},
                            ]})
                            .projection(mongodb::bson::doc! {"emotion_score_timeline": 0, "most_emotion_timeline": 0, "client_info": 0})
                            .sort(mongodb::bson::doc! {"finalized_at": 1, "video_view_log_id": 1})
                            .await
                        }
                    }
                    .map_err(|_| unavailable())?;
                    while let Some(doc) = cursor.next().await.transpose().map_err(|_| unavailable())? {
                        count += 1;
                        if count as usize > STATS_MAX_DOCS {
                            return Err(bad("stats 세션 상한 초과"));
                        }
                        if after.is_some() {
                            let (Some(at), Ok(session)) = (millis_of(&doc, "finalized_at"), doc.get_str("video_view_log_id")) else {
                                return Err(bad("마감 시각 또는 세션 id가 없음"));
                            };
                            last = Some((at, session.to_owned()));
                            if let Ok(precomputed) = doc.get_document("emotion_seconds") {
                                for (sum, emotion) in sums.iter_mut().zip(STATS_EMOTIONS) {
                                    *sum += py_float(precomputed.get(emotion)).unwrap_or(0.0);
                                }
                                continue;
                            }
                        } else {
                            let per_second = match doc.get("most_emotion_timeline") {
                                None | Some(mongodb::bson::Bson::Null) => None,
                                Some(mongodb::bson::Bson::Document(timeline)) => {
                                    let mut seconds: std::collections::BTreeMap<i64, usize> = Default::default();
                                    for (key, value) in timeline {
                                        let Some(slot) = value.as_str().and_then(|name| STATS_EMOTIONS.iter().position(|e| *e == name)) else { continue };
                                        let Ok(offset) = key.trim().parse::<f64>() else { continue };
                                        if offset.is_nan() {
                                            continue;
                                        }
                                        if offset.is_infinite() {
                                            return Err(bad("timeline 키가 유한하지 않음"));
                                        }
                                        seconds.insert((offset / 100.0).floor() as i64, slot);
                                    }
                                    Some(seconds).filter(|seconds| !seconds.is_empty())
                                }
                                Some(_) => return Err(bad("most_emotion_timeline 형식 오류")),
                            };
                            if let Some(seconds) = per_second {
                                for slot in seconds.values() {
                                    sums[*slot] += 1.0;
                                }
                                continue;
                            }
                        }
                        let percentages = match doc.get("emotion_percentages") {
                            None | Some(mongodb::bson::Bson::Null) => None,
                            Some(mongodb::bson::Bson::Document(percentages)) => Some(percentages),
                            Some(_) => return Err(bad("emotion_percentages 형식 오류")),
                        };
                        let seconds = estimated_watch_seconds(&doc, after.is_some());
                        if let Some(percentages) = percentages {
                            for (sum, emotion) in sums.iter_mut().zip(STATS_EMOTIONS) {
                                if let Some(value) = percentages.get(emotion) {
                                    *sum += py_float(Some(value)).ok_or_else(|| bad("감정 비율이 숫자가 아님"))? * seconds;
                                }
                            }
                        }
                    }
                    let checkpoint = match (&after, last) {
                        (Some(_), Some((at, session))) => json!({"at": iso_of(at), "session": session}),
                        (Some(_), None) => Value::Null,
                        (None, _) => {
                            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
                            json!({"at": iso_of(now), "session": ""})
                        }
                    };
                    // The verified caller's id is answered so a first call can create the member's own summary row.
                    Ok(json!({"user": user, "count": count, "seconds": sums, "checkpoint": checkpoint}))
                }
                _ => Err(reject("WIRE.BAD_VALUE", "알 수 없는 stats command")),
            }
        })
        .await
        .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo stats timeout"))?
    }
}

impl MongoCapability for MongoDriverCapability {
    fn call<'a>(&'a self, principal: &'a CapabilityPrincipal, collection: &'a str, input: &'a Value) -> CapabilityFuture<'a, Result<Value, Reject>> {
        Box::pin(async move {
            if !principal.permits("mongo", collection) {
                return Err(reject("WIRE.ACCESS_NOT_DECLARED", "Mongo capability가 확장에 선언되지 않음"));
            }
            if collection == TIMELINE_COLLECTION {
                return self.video_timeline(input).await;
            }
            if collection == USER_STATS_COLLECTION {
                return self.user_watch_stats(principal, input).await;
            }
            if collection == HEALTH_COLLECTION {
                return self.distribution_health(input).await;
            }
            if collection == SERVER_COLLECTION {
                return self.server_status(input).await;
            }
            if collection == SESSION_STATS_COLLECTION {
                return self.video_session_stats(input).await;
            }
            if !self.allowed_collections.contains(collection) {
                return Err(reject("WIRE.CAPABILITY_DENIED", "Mongo collection이 허용되지 않음"));
            }
            let Some(command) = input.get("command").and_then(Value::as_str) else {
                return Err(reject("WIRE.BAD_VALUE", "Mongo command 누락"));
            };
            if !["find", "find_one", "count_documents"].contains(&command) {
                return Err(reject("WIRE.CAPABILITY_DENIED", "Mongo read command가 허용되지 않음"));
            }
            let allowed_input = ["command", "filter", "projection", "limit", "skip", "sort", "sort_by", "desc"];
            if input.as_object().is_none_or(|m| m.keys().any(|k| !allowed_input.contains(&k.as_str()))) {
                return Err(reject("WIRE.BAD_VALUE", "Mongo command input field가 허용되지 않음"));
            }
            if command == "count_documents" && ["projection", "limit", "skip", "sort", "sort_by", "desc"].iter().any(|k| input.get(*k).is_some()) {
                return Err(reject("WIRE.BAD_VALUE", "count_documents는 filter만 받음"));
            }
            if command == "find_one" && ["skip", "sort", "sort_by", "desc"].iter().any(|k| input.get(*k).is_some()) {
                return Err(reject("WIRE.BAD_VALUE", "find_one은 skip/sort를 지원하지 않음"));
            }
            let filter = input.get("filter").unwrap_or(&Value::Null);
            let filter = self.validate_filter(collection, filter, principal)?;
            let limit = match input.get("limit") {
                None => {
                    if command == "find_one" {
                        1
                    } else {
                        100
                    }
                }
                Some(v) if v.as_i64().is_some_and(|n| n > 0 && n <= self.max_docs) => v.as_i64().unwrap(),
                _ => return Err(reject("WIRE.BAD_VALUE", "Mongo limit은 양수 bounded integer여야 함")),
            };
            let projection = input
                .get("projection")
                .map(|v| {
                    let Some(map) = v.as_object() else {
                        return Err(reject("WIRE.BAD_VALUE", "Mongo projection은 객체여야 함"));
                    };
                    let modes: HashSet<bool> = map
                        .iter()
                        .filter(|(key, _)| key.as_str() != "_id")
                        .filter_map(|(_, v)| {
                            if let Some(b) = v.as_bool() {
                                Some(b)
                            } else if v.as_i64() == Some(1) {
                                Some(true)
                            } else if v.as_i64() == Some(0) {
                                Some(false)
                            } else {
                                None
                            }
                        })
                        .collect();
                    if map.len() > self.allowed_fields.len()
                        || map.keys().any(|k| !self.allowed_fields.contains(k))
                        || modes.len() != 1
                        || map.values().any(|v| !v.is_boolean() && ![Some(0), Some(1)].contains(&v.as_i64()))
                    {
                        return Err(reject("WIRE.BAD_VALUE", "Mongo projection이 허용되지 않음"));
                    }
                    mongodb::bson::serialize_to_document(v).map_err(|_| reject("WIRE.BAD_VALUE", "Mongo projection 변환 실패"))
                })
                .transpose()?;
            if input.get("sort").is_some() && (input.get("sort_by").is_some() || input.get("desc").is_some()) {
                return Err(reject("WIRE.BAD_VALUE", "sort와 sort_by 방식은 함께 사용할 수 없음"));
            }
            let skip = match input.get("skip") {
                None => 0,
                Some(v) if v.as_u64().is_some_and(|n| n <= 100_000) => v.as_u64().unwrap(),
                _ => return Err(reject("WIRE.BAD_VALUE", "Mongo skip 범위 오류")),
            };
            let sort = input
                .get("sort")
                .map(|v| {
                    let Some(map) = v.as_object() else {
                        return Err(reject("WIRE.BAD_VALUE", "Mongo sort는 객체여야 함"));
                    };
                    if map.is_empty()
                        || map.len() > 4
                        || map.keys().any(|k| !self.allowed_fields.contains(k))
                        || map.values().any(|v| !matches!(v.as_i64(), Some(-1 | 1)))
                    {
                        return Err(reject("WIRE.BAD_VALUE", "Mongo sort field/direction이 허용되지 않음"));
                    }
                    mongodb::bson::serialize_to_document(v).map_err(|_| reject("WIRE.BAD_VALUE", "Mongo sort 변환 실패"))
                })
                .transpose()?;
            let coll = self.client.database(&self.database).collection::<mongodb::bson::Document>(collection);
            let result = tokio::time::timeout(self.timeout, async {
                match command {
                    "count_documents" => {
                        Ok(json!({"count": coll.count_documents(filter).await.map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo read 실패"))?}))
                    }
                    "find_one" => {
                        let mut action = coll.find_one(filter);
                        if let Some(projection) = projection {
                            action = action.projection(projection);
                        }
                        let doc = action.await.map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo read 실패"))?;
                        let value = doc.map(|d| mongodb::bson::deserialize_from_document::<Value>(d).unwrap_or(Value::Null));
                        if value.as_ref().is_some_and(|v| serde_json::to_vec(v).map_or(true, |b| b.len() > self.max_bytes)) {
                            return Err(reject("WIRE.BAD_VALUE", "Mongo result byte limit exceeded"));
                        }
                        Ok(json!({"doc": value}))
                    }
                    _ => {
                        let mut action = coll.find(filter).limit(limit).skip(skip);
                        if let Some(projection) = projection {
                            action = action.projection(projection);
                        }
                        if let Some(sort) = sort {
                            action = action.sort(sort);
                        }
                        if let (Some(field), Some(desc)) = (input.get("sort_by"), input.get("desc")) {
                            let Some(field) = field.as_str() else {
                                return Err(reject("WIRE.BAD_VALUE", "sort_by는 문자열이어야 함"));
                            };
                            let Some(desc) = desc.as_bool() else {
                                return Err(reject("WIRE.BAD_VALUE", "desc는 boolean이어야 함"));
                            };
                            if !self.allowed_fields.contains(field) {
                                return Err(reject("WIRE.BAD_VALUE", "sort_by field가 허용되지 않음"));
                            }
                            action = action.sort(mongodb::bson::doc! { field: if desc { -1 } else { 1 } });
                        } else if input.get("sort_by").is_some() || input.get("desc").is_some() {
                            return Err(reject("WIRE.BAD_VALUE", "sort_by와 desc는 함께 지정해야 함"));
                        }
                        let mut cursor = action.await.map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo read 실패"))?;
                        let mut docs = Vec::new();
                        let mut bytes = 0usize;
                        use futures_util::StreamExt;
                        while let Some(doc) = cursor.next().await.transpose().map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo cursor 실패"))?
                        {
                            let value = mongodb::bson::deserialize_from_document::<Value>(doc)
                                .map_err(|_| reject("WIRE.BAD_VALUE", "Mongo document serialization 실패"))?;
                            bytes = bytes
                                .saturating_add(serde_json::to_vec(&value).map_err(|_| reject("WIRE.BAD_VALUE", "Mongo serialization 실패"))?.len());
                            if bytes > self.max_bytes {
                                return Err(reject("WIRE.BAD_VALUE", "Mongo result byte limit exceeded"));
                            }
                            docs.push(value);
                        }
                        Ok(json!({"docs": docs}))
                    }
                }
            })
            .await
            .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Mongo query timeout"))?;
            result
        })
    }

    fn write<'a>(
        &'a self,
        principal: &'a CapabilityPrincipal,
        collection: &'a str,
        input: &'a Value,
    ) -> CapabilityFuture<'a, Result<EffectOutcome, Reject>> {
        Box::pin(self.write_effect(principal, collection, input))
    }

    fn compensate<'a>(&'a self, collection: &'a str, record: &'a Value) -> CapabilityFuture<'a, Result<(), Reject>> {
        Box::pin(self.compensate_effect(collection, record))
    }

    fn journal<'a>(&'a self, record: &'a Value) -> CapabilityFuture<'a, Result<(), Reject>> {
        Box::pin(self.journal_record(record))
    }
}

/// Real Redis read-only capability with a reconnecting bounded single connection manager.
#[derive(Clone)]
pub struct RedisDriverCapability {
    manager: redis::aio::ConnectionManager,
    prefixes: Vec<String>,
    timeout: Duration,
    in_flight: Arc<tokio::sync::Semaphore>,
}

impl RedisDriverCapability {
    pub async fn connect(uri: &str, prefixes: Vec<String>, timeout: Duration) -> Result<Self, Reject> {
        Self::connect_with_limit(uri, prefixes, timeout, 32).await
    }
    pub async fn connect_with_limit(uri: &str, prefixes: Vec<String>, timeout: Duration, max_in_flight: u32) -> Result<Self, Reject> {
        if prefixes.is_empty()
            || prefixes.iter().any(String::is_empty)
            || timeout.is_zero()
            || timeout > Duration::from_secs(30)
            || !(1..=256).contains(&max_in_flight)
        {
            return Err(reject("WIRE.BAD_CONFIG", "Redis capability 설정 범위 오류"));
        }
        let client = redis::Client::open(uri).map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis 설정을 읽지 못함"))?;
        let manager = tokio::time::timeout(timeout, client.get_connection_manager())
            .await
            .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis readiness timeout"))?
            .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis readiness 실패"))?;
        let mut probe = manager.clone();
        let _: String = tokio::time::timeout(timeout, redis::cmd("PING").query_async(&mut probe))
            .await
            .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis PING timeout"))?
            .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis PING 실패"))?;
        Ok(Self { manager, prefixes, timeout, in_flight: Arc::new(tokio::sync::Semaphore::new(max_in_flight as usize)) })
    }
    fn allowed(&self, key: &str) -> bool {
        self.prefixes.iter().any(|prefix| key.starts_with(prefix))
    }
}

impl RedisCapability for RedisDriverCapability {
    fn call<'a>(&'a self, principal: &'a CapabilityPrincipal, key: &'a str, input: &'a Value) -> CapabilityFuture<'a, Result<Value, Reject>> {
        Box::pin(async move {
            let command = input.get("command").and_then(Value::as_str).ok_or_else(|| reject("WIRE.BAD_VALUE", "Redis command 누락"))?;
            if input.as_object().is_none_or(|m| m.keys().any(|k| !["command", "keys", "start", "stop"].contains(&k.as_str())))
                || (command != "mget" && input.get("keys").is_some())
                || (command != "lrange" && (input.get("start").is_some() || input.get("stop").is_some()))
            {
                return Err(reject("WIRE.BAD_VALUE", "Redis command input field가 허용되지 않음"));
            }
            let keys: Vec<&str> = if command == "mget" {
                let Some(values) = input.get("keys").and_then(Value::as_array) else {
                    return Err(reject("WIRE.BAD_VALUE", "Redis mget keys 형식 오류"));
                };
                if values.is_empty() || values.len() > 64 {
                    return Err(reject("WIRE.BAD_VALUE", "Redis mget key count 범위 오류"));
                }
                values.iter().map(|v| v.as_str().ok_or_else(|| reject("WIRE.BAD_VALUE", "Redis key는 문자열이어야 함"))).collect::<Result<_, _>>()?
            } else {
                vec![key]
            };
            for k in &keys {
                if k.is_empty() || k.len() > 512 {
                    return Err(reject("WIRE.BAD_VALUE", "Redis key length 범위 오류"));
                }
                if !principal.permits("redis", k) {
                    return Err(reject("WIRE.ACCESS_NOT_DECLARED", "Redis key capability가 확장에 선언되지 않음"));
                }
                if !self.allowed(k) {
                    return Err(reject("WIRE.CAPABILITY_DENIED", "Redis key namespace가 허용되지 않음"));
                }
            }
            if !["get", "hgetall", "exists", "mget", "lrange"].contains(&command) {
                return Err(reject("WIRE.CAPABILITY_DENIED", "Redis read command가 허용되지 않음"));
            }
            let _permit = tokio::time::timeout(self.timeout, self.in_flight.clone().acquire_owned())
                .await
                .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis capacity timeout"))?
                .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis capability가 종료됨"))?;
            let mut con = self.manager.clone();
            let future = async {
                match command {
                    "get" => {
                        let script = "local n=redis.call('STRLEN',KEYS[1]); if n>1048576 then return redis.error_reply('AIP_RESULT_LIMIT') end; return redis.call('GET',KEYS[1])";
                        let value: Option<String> =
                            redis::cmd("EVAL_RO").arg(script).arg(1).arg(key).query_async(&mut con).await.map_err(redis_failure)?;
                        Ok(json!({"value":value}))
                    }
                    "lrange" => {
                        // Window of at most 1000 elements starting within the first 10000, as the original's metrics list (`lrange 0 -1`).
                        let start = input.get("start").and_then(Value::as_i64).unwrap_or(0);
                        let stop = input.get("stop").and_then(Value::as_i64).unwrap_or(-1);
                        if !(0..=10_000).contains(&start) || !(-1..=10_000).contains(&stop) || (stop >= 0 && stop < start) {
                            return Err(reject("WIRE.BAD_VALUE", "Redis lrange 범위 오류"));
                        }
                        let last = if stop < 0 { start + 999 } else { stop.min(start + 999) };
                        let script = "local r=redis.call('LRANGE',KEYS[1],ARGV[1],ARGV[2]); local n=0; for _,v in ipairs(r) do n=n+#v; if n>1048576 then return redis.error_reply('AIP_RESULT_LIMIT') end end; return r";
                        let values: Vec<String> = redis::cmd("EVAL_RO")
                            .arg(script)
                            .arg(1)
                            .arg(key)
                            .arg(start)
                            .arg(last)
                            .query_async(&mut con)
                            .await
                            .map_err(redis_failure)?;
                        Ok(json!({"values": values}))
                    }
                    "exists" => {
                        let value: bool = redis::cmd("EXISTS")
                            .arg(key)
                            .query_async(&mut con)
                            .await
                            .map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis EXISTS 실패"))?;
                        Ok(json!({"exists":value}))
                    }
                    "hgetall" => {
                        let script = "local c=redis.call('HLEN',KEYS[1]); if c>10000 then return redis.error_reply('AIP_RESULT_LIMIT') end; local ks=redis.call('HKEYS',KEYS[1]); local n=0; local out={}; for _,k in ipairs(ks) do n=n+#k; if n>1048576 then return redis.error_reply('AIP_RESULT_LIMIT') end; local v=redis.call('HGET',KEYS[1],k); n=n+#v; if n>1048576 then return redis.error_reply('AIP_RESULT_LIMIT') end; table.insert(out,k); table.insert(out,v); end; return out";
                        let flat: Vec<String> =
                            redis::cmd("EVAL_RO").arg(script).arg(1).arg(key).query_async(&mut con).await.map_err(redis_failure)?;
                        if !flat.len().is_multiple_of(2) {
                            return Err(reject("WIRE.ADAPTER_UNAVAILABLE", "Redis HGETALL 응답 형식 오류"));
                        }
                        let value: HashMap<String, String> = flat.as_chunks::<2>().0.iter().map(|pair| (pair[0].clone(), pair[1].clone())).collect();
                        Ok(json!({"value":value}))
                    }
                    _ => {
                        let script = "local out={}; local n=0; for i,k in ipairs(KEYS) do local z=redis.call('STRLEN',k); n=n+z; if n>1048576 then return redis.error_reply('AIP_RESULT_LIMIT') end; out[i]=redis.call('GET',k); end; return out";
                        let values: Vec<Option<String>> =
                            redis::cmd("EVAL_RO").arg(script).arg(keys.len()).arg(&keys).query_async(&mut con).await.map_err(redis_failure)?;
                        Ok(json!({"values":values}))
                    }
                }
            };
            tokio::time::timeout(self.timeout, future).await.map_err(|_| reject("WIRE.ADAPTER_UNAVAILABLE", "Redis command timeout"))?
        })
    }

    fn write<'a>(
        &'a self,
        principal: &'a CapabilityPrincipal,
        key: &'a str,
        input: &'a Value,
    ) -> CapabilityFuture<'a, Result<EffectOutcome, Reject>> {
        Box::pin(self.write_effect(principal, key, input))
    }

    fn compensate<'a>(&'a self, key: &'a str, record: &'a Value) -> CapabilityFuture<'a, Result<(), Reject>> {
        Box::pin(self.compensate_effect(key, record))
    }
}

fn redis_failure(error: redis::RedisError) -> Reject {
    if error.to_string().contains("AIP_RESULT_LIMIT") {
        reject("WIRE.BAD_VALUE", "Redis result limit exceeded")
    } else {
        reject("WIRE.ADAPTER_UNAVAILABLE", "Redis read failed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reference values: a python:3.13 container (Linux, psutil 7.2.2) read /proc and asked psutil in the same instants.
    #[test]
    fn host_figures_equal_psutil_on_a_linux_capture() {
        let before = cpu_busy_total("cpu  2265256 0 5558056 597084404 111996 0 580719 0 0 0\ncpu0 1 2 3\n").unwrap();
        let after = cpu_busy_total("cpu  2265356 0 5558056 597085205 111996 0 580720 0 0 0\n").unwrap();
        assert_eq!(cpu_percent(before, after), 11.2);
        assert_eq!(cpu_percent(after, after), 0.0, "no elapsed time");
        // Guest time sits inside user and is left out of the total: the same counters with 50 guest ticks in both samples.
        assert_eq!(cpu_busy_total("cpu  100 0 100 800 0 0 0 0 50 0").unwrap(), (200.0, 1000.0));
        let meminfo = "MemTotal:       16423016 kB\nMemFree:        11522368 kB\nMemAvailable:   13454768 kB\nBuffers:             152 kB\n";
        assert_eq!(memory_figures(meminfo), Some((18.1, 16038.1)));
        assert_eq!(memory_figures("MemTotal: 10 kB\n"), None, "no MemAvailable (kernels before 3.14)");
        assert_eq!(disk_percent(64225280.0, 58782806.0, 58782806.0, 4096.0), 8.5);
        assert_eq!(disk_percent(0.0, 0.0, 0.0, 4096.0), 0.0);
    }

    // On Linux the real counters answer: every figure is a number in range (the values themselves change every moment).
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn host_figures_read_this_linux_machine() {
        let figures = host_figures().await;
        for name in ["cpu_usage", "memory_usage", "disk_usage"] {
            assert!(figures[name].as_f64().is_some_and(|v| (0.0..=100.0).contains(&v)), "{name}: {figures}");
        }
        assert!(figures["memory_total_mb"].as_f64().is_some_and(|v| v > 0.0), "{figures}");
    }

    #[test]
    fn health_averages_round_like_python_round_to_four_places() {
        // Reference values from Python 3.13 round(x, 4); 0.28125 is an exact tie (half to even), 0.12345 is just above its tie.
        for (value, want) in [
            (0.0, 0.0),
            (0.12345, 0.1235),
            (0.123451, 0.1235),
            (0.99995, 1.0),
            (0.00005, 0.0001),
            (1.0, 1.0),
            (0.33333333, 0.3333),
            (0.00015, 0.0001),
            (0.28125, 0.2812),
            (2.00005, 2.0),
        ] {
            assert_eq!(py_round4(value), want, "{value}");
        }
    }

    fn timeline_key(key: &str, sessions: u64, sums: [f64; 5]) -> TimelineKey {
        TimelineKey { key: key.into(), sessions, sums }
    }

    #[test]
    fn the_checkpoint_text_of_a_time_field_is_read_in_every_form_the_service_writes() {
        // 2026-03-01T10:00:00Z = 1772359200 s; a fraction keeps its first three digits.
        for text in ["2026-03-01T10:00:00", "2026-03-01T10:00:00Z", "2026-03-01T10:00:00+00:00", "2026-03-01 10:00:00", "2026-03-01T10:00:00.000000"]
        {
            assert_eq!(parse_iso_millis(text), Some(1_772_359_200_000), "{text}");
        }
        assert_eq!(parse_iso_millis("2026-03-01T10:00:00.4"), Some(1_772_359_200_400));
        assert_eq!(parse_iso_millis("2026-03-01T10:00:00.600000+00:00"), Some(1_772_359_200_600));
        assert_eq!(parse_iso_millis("1970-01-01T00:00:00"), Some(0));
        assert_eq!(parse_iso_millis("2024-02-29T23:59:59.999"), Some(1_709_251_199_999));
        for bad in ["", "2026-03-01", "2026-13-01T00:00:00", "2026-03-01T24:00:00", "2026-03-01T10:00:00+09:00", "x", "2026-03-01T10:00:00.12x"] {
            assert_eq!(parse_iso_millis(bad), None, "{bad}");
        }
        // The round trip with the text the server answers.
        for millis in [0, 1_772_359_200_400, 1_709_251_199_999, 951_782_400_000] {
            assert_eq!(parse_iso_millis(&iso_of(millis)), Some(millis));
        }
    }

    #[test]
    fn watch_seconds_follow_the_originals_estimate_order() {
        let estimate = |doc: mongodb::bson::Document, with_timeline_len: bool| estimated_watch_seconds(&doc, with_timeline_len);
        use mongodb::bson::doc;
        assert_eq!(estimate(doc! {"duration": 100, "completion_rate": 0.5}, false), 50.0);
        assert_eq!(estimate(doc! {"duration": 40.0, "completion_rate": 1.5}, false), 40.0, "completion above 1 counts as 1");
        // Only one of the two usable: fall through to the frame count.
        assert_eq!(estimate(doc! {"duration": 100, "completion_rate": 0.0, "frame_count": 9}, false), 4.5);
        assert_eq!(
            estimate(doc! {"duration": "abc", "completion_rate": 0.5, "frame_count": 21}, false),
            10.5,
            "text that is not a number drops both"
        );
        assert_eq!(estimate(doc! {"timeline_len": 40, "frame_count": 10}, true), 20.0);
        assert_eq!(estimate(doc! {"timeline_len": 40, "frame_count": 10}, false), 5.0, "the first pass never reads timeline_len");
        assert_eq!(estimate(doc! {}, true), 0.0);
        assert_eq!(py_float(Some(&mongodb::bson::Bson::String(" 2.5 ".into()))), Some(2.5));
        assert_eq!(py_float(Some(&mongodb::bson::Bson::String("nan".into()))), None);
        assert_eq!(py_float(Some(&mongodb::bson::Bson::Null)), None);
    }

    #[test]
    fn timeline_buckets_average_each_key_first_then_each_bucket_like_the_original() {
        assert!(compress_timeline(&[], Some(10)).is_none());
        // 10 s video: bucket width is 10 cs. Keys 0 and 5 share bucket 0, key 10 is bucket 1.
        // key "0": two sessions summing neutral 30 -> avg 15; key "5": one session neutral 50; bucket 0 mean = (15 + 50) / 2 = 32.5.
        let keys = [
            timeline_key("0", 2, [30.0, 10.0, 0.0, 0.0, 0.0]),
            timeline_key("5", 1, [50.0, 0.0, 0.0, 0.0, 0.0]),
            timeline_key("10", 1, [7.25, 0.0, 0.0, 0.0, 100.0]),
            timeline_key("not-a-number", 1, [99.0; 5]),
            timeline_key("-5", 1, [99.0; 5]),
        ];
        let timeline = compress_timeline(&keys, Some(10)).unwrap();
        assert_eq!(timeline["neutral"][0], json!({"x": 1, "y": 32.5}));
        assert_eq!(timeline["neutral"].as_array().unwrap().len(), 100);
        assert_eq!(timeline["happy"][0], json!({"x": 1, "y": 2.5}));
        assert_eq!(timeline["neutral"][1], json!({"x": 2, "y": 7.2}), "7.25 rounds half to even like Python");
        assert_eq!(timeline["angry"][1], json!({"x": 2, "y": 100.0}));
        assert_eq!(timeline["sad"][99], json!({"x": 100, "y": 0.0}));
        // A key past the end lands in the last bucket; a missing or zero duration means one second, as `duration or 1`.
        let late = compress_timeline(&[timeline_key("999999", 1, [40.0, 0.0, 0.0, 0.0, 0.0])], None).unwrap();
        assert_eq!(late["neutral"][99], json!({"x": 100, "y": 40.0}));
        let zero = compress_timeline(&[timeline_key("50", 1, [40.0, 0.0, 0.0, 0.0, 0.0])], Some(0)).unwrap();
        assert_eq!(zero["neutral"][50], json!({"x": 51, "y": 40.0}));
    }

    #[test]
    fn one_decimal_rounding_matches_python_round_half_even_on_the_binary_value() {
        // Python: round(0.25, 1) == 0.2, round(0.35, 1) == 0.3 (0.35 is below .35), round(2.675, 2) differs but we only use 1 decimal.
        assert_eq!(round_one_decimal(0.25), 0.2);
        assert_eq!(round_one_decimal(0.35), 0.3);
        assert_eq!(round_one_decimal(0.45), 0.5);
        assert_eq!(round_one_decimal(12.34999), 12.3);
    }

    #[test]
    fn mongo_scope_requires_exact_grant_while_redis_prefix_grants_stay_namespaced() {
        let mongo_wildcard = CapabilityPrincipal { actor_id: Some(1), actor_uuid: None, grants: ["mongo.*".into()].into_iter().collect() };
        assert!(!mongo_wildcard.permits("mongo", "youtube_watching_data"));
        let redis_prefix =
            CapabilityPrincipal { actor_id: Some(1), actor_uuid: None, grants: ["redis.facereview:reco:*".into()].into_iter().collect() };
        assert!(redis_prefix.permits("redis", "facereview:reco:popular"));
        assert!(!redis_prefix.permits("redis", "facereview:session:private"));
    }

    fn mongo_test() -> MemoryMongoCapability {
        MemoryMongoCapability::new(&["video_distribution", "youtube_watching_data"], &["find", "find_one", "count_documents"])
    }
    fn redis_test() -> MemoryRedisCapability {
        MemoryRedisCapability::new(&["facereview:reco:"], &["get", "hgetall", "exists", "mget"])
    }

    fn test_principal() -> CapabilityPrincipal {
        CapabilityPrincipal {
            actor_id: Some(1),
            actor_uuid: None,
            grants: [
                "mongo.youtube_watching_data".to_string(),
                "mongo.video_distribution".to_string(),
                "mongo.admin_secrets".to_string(),
                "redis.facereview:reco:*".to_string(),
                "redis.other_app:secret".to_string(),
            ]
            .into_iter()
            .collect(),
        }
    }

    #[tokio::test]
    async fn mongo_capability_allows_whitelisted_collection_and_read_command() {
        let mongo = mongo_test();
        mongo.seed(
            "video_distribution",
            vec![
                json!({ "video_id": 1, "dominant_emotion": "happy" }),
                json!({ "video_id": 2, "dominant_emotion": "neutral" }),
                json!({ "video_id": 3, "dominant_emotion": "happy" }),
            ],
        );

        // find with filter
        let res = mongo
            .call(
                &test_principal(),
                "video_distribution",
                &json!({
                    "command": "find",
                    "filter": { "dominant_emotion": "happy" }
                }),
            )
            .await
            .unwrap();
        assert_eq!(res["docs"].as_array().unwrap().len(), 2);

        // count_documents
        let count_res = mongo
            .call(
                &test_principal(),
                "video_distribution",
                &json!({
                    "command": "count_documents",
                    "filter": { "dominant_emotion": "neutral" }
                }),
            )
            .await
            .unwrap();
        assert_eq!(count_res["count"], 1);

        // find_one
        let one_res = mongo
            .call(
                &test_principal(),
                "video_distribution",
                &json!({
                    "command": "find_one",
                    "filter": { "video_id": 2 }
                }),
            )
            .await
            .unwrap();
        assert_eq!(one_res["doc"]["video_id"], 2);
    }

    #[tokio::test]
    async fn mongo_capability_rejects_unauthorized_collection_or_write_command() {
        let mongo = mongo_test();

        // 1. 비인가 컬렉션 거부
        let err = mongo.call(&test_principal(), "admin_secrets", &json!({ "command": "find" })).await.unwrap_err();
        assert_eq!(err.code, "WIRE.CAPABILITY_DENIED");
        assert!(err.msg.contains("비인가 Mongo 컬렉션"));

        // 2. 비인가 쓰기 명령 거부
        let err_write = mongo
            .call(
                &test_principal(),
                "video_distribution",
                &json!({
                    "command": "insert",
                    "doc": { "video_id": 99 }
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(err_write.code, "WIRE.CAPABILITY_DENIED");
        assert!(err_write.msg.contains("허용되지 않은 Mongo 명령"));

        // 3. 삭제 명령 거부
        let err_del = mongo
            .call(
                &test_principal(),
                "video_distribution",
                &json!({
                    "command": "delete",
                    "filter": { "video_id": 1 }
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(err_del.code, "WIRE.CAPABILITY_DENIED");
    }

    #[tokio::test]
    async fn redis_capability_allows_whitelisted_keys_and_read_commands() {
        let redis = redis_test();
        redis.set_str("facereview:reco:pool", "[\"v1\",\"v2\"]");
        let mut cat_fields = HashMap::new();
        cat_fields.insert("Comedy".to_string(), "[\"v1\"]".to_string());
        redis.set_hash("facereview:reco:category", cat_fields);

        // get
        let get_res = redis.call(&test_principal(), "facereview:reco:pool", &json!({ "command": "get" })).await.unwrap();
        assert_eq!(get_res["value"], "[\"v1\",\"v2\"]");

        // hgetall
        let hash_res = redis.call(&test_principal(), "facereview:reco:category", &json!({ "command": "hgetall" })).await.unwrap();
        assert_eq!(hash_res["value"]["Comedy"], "[\"v1\"]");

        // exists
        let exists_res = redis.call(&test_principal(), "facereview:reco:pool", &json!({ "command": "exists" })).await.unwrap();
        assert_eq!(exists_res["exists"], true);
    }

    #[tokio::test]
    async fn redis_capability_rejects_unauthorized_keys_or_write_commands() {
        let redis = redis_test();

        // 1. 비인가 키 네임스페이스 거부
        let err_key = redis.call(&test_principal(), "other_app:secret", &json!({ "command": "get" })).await.unwrap_err();
        assert_eq!(err_key.code, "WIRE.CAPABILITY_DENIED");
        assert!(err_key.msg.contains("비인가 Redis 키 접근 거부"));

        // 2. 비인가 쓰기 명령 거부
        let err_set = redis
            .call(
                &test_principal(),
                "facereview:reco:pool",
                &json!({
                    "command": "set",
                    "value": "injected"
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(err_set.code, "WIRE.CAPABILITY_DENIED");
        assert!(err_set.msg.contains("허용되지 않은 Redis 명령"));

        // 3. 비인가 삭제 명령 거부
        let err_del = redis
            .call(
                &test_principal(),
                "facereview:reco:pool",
                &json!({
                    "command": "del"
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(err_del.code, "WIRE.CAPABILITY_DENIED");
    }

    #[tokio::test]
    async fn advertised_mget_reads_all_keys_from_the_backend() {
        let redis = redis_test();
        redis.seed("facereview:reco:first", "one");
        redis.seed("facereview:reco:second", "two");
        let result = redis
            .call(
                &test_principal(),
                "facereview:reco:first",
                &json!({
                    "command":"mget", "keys":["facereview:reco:first", "facereview:reco:second"]
                }),
            )
            .await
            .unwrap();
        assert_eq!(result["values"], json!(["one", "two"]));
    }

    #[tokio::test]
    async fn private_watch_data_forces_the_principal_actor_and_rejects_spoofed_filters() {
        let mongo = mongo_test();
        mongo.seed("youtube_watching_data", vec![json!({"user_id": 7, "video_id": 1}), json!({"user_id": 8, "video_id": 2})]);
        let principal =
            CapabilityPrincipal { actor_id: Some(7), actor_uuid: None, grants: ["mongo.youtube_watching_data".to_string()].into_iter().collect() };
        let ok = mongo.call(&principal, "youtube_watching_data", &json!({"command":"find","filter":{}})).await.unwrap();
        assert_eq!(ok["docs"].as_array().unwrap().len(), 1);
        assert_eq!(ok["docs"][0]["video_id"], 1);
        let forged = mongo.call(&principal, "youtube_watching_data", &json!({"command":"find","filter":{"user_id": 8}})).await.unwrap_err();
        assert_eq!(forged.code, "WIRE.ACCESS_DENIED");
        let guest =
            CapabilityPrincipal { actor_id: None, actor_uuid: None, grants: ["mongo.youtube_watching_data".to_string()].into_iter().collect() };
        assert_eq!(mongo.call(&guest, "youtube_watching_data", &json!({"command":"find","filter":{}})).await.unwrap_err().code, "WIRE.ACCESS_DENIED");
    }

    #[tokio::test]
    async fn uuid_actor_owns_uuid_string_documents_not_the_internal_handle_documents() {
        const ALICE: &str = "0b1f4c7e-52a1-4d3e-9c1a-7f2b8d6e4a10";
        const BOB: &str = "9d3c2b1a-8e7f-4a6b-b5c4-1e0f9a8b7c6d";
        let mongo = mongo_test();
        mongo.seed(
            "youtube_watching_data",
            vec![
                json!({"user_id": ALICE, "video_id": 1}),
                json!({"user_id": BOB, "video_id": 2}),
                json!({"user_id": 7, "video_id": 3}),
                json!({"user_id": "7", "video_id": 4}),
            ],
        );
        let grants: HashSet<String> = ["mongo.youtube_watching_data".to_string()].into_iter().collect();
        let alice = CapabilityPrincipal { actor_id: Some(7), actor_uuid: Some(parse_uuid(ALICE).unwrap()), grants: grants.clone() };
        let ok = mongo.call(&alice, "youtube_watching_data", &json!({"command":"find","filter":{}})).await.unwrap();
        assert_eq!(ok["docs"].as_array().unwrap().len(), 1);
        assert_eq!(ok["docs"][0]["video_id"], 1);
        for filter in [
            json!({"user_id": BOB}),
            json!({"user_id": 7}),
            json!({"user_id": "7"}),
            json!({"user_id": {"$ne": ALICE}}),
            json!({"user_id": {"$in": [ALICE, BOB]}}),
        ] {
            let rejected = mongo.call(&alice, "youtube_watching_data", &json!({"command":"find","filter":filter})).await.unwrap_err();
            assert_eq!(rejected.code, "WIRE.ACCESS_DENIED");
        }
        // UUID가 아닌 actor_uuid는 숫자 handle로 대체되지 않고 거부된다.
        let not_uuid = CapabilityPrincipal { actor_id: Some(7), actor_uuid: Some(EntityId::Number(7)), grants: grants.clone() };
        assert_eq!(
            mongo.call(&not_uuid, "youtube_watching_data", &json!({"command":"find","filter":{}})).await.unwrap_err().code,
            "WIRE.ACCESS_DENIED"
        );
        // 정수 actor는 기존대로 숫자 문서만 본다.
        let integer = CapabilityPrincipal { actor_id: Some(7), actor_uuid: None, grants };
        let docs = mongo.call(&integer, "youtube_watching_data", &json!({"command":"find","filter":{}})).await.unwrap();
        assert_eq!(docs["docs"].as_array().unwrap().len(), 1);
        assert_eq!(docs["docs"][0]["video_id"], 3);
    }
}
