//! 관리형 Mongo/Redis capability 경계 및 호스트 인터페이스 (WBS 7.2.3 Slice13-A).
//!
//! 워커는 DB 연결 DSN이나 운영 자격증명을 직접 받지 않으며, 서버가 확정한 principal과 화이트리스트 정책 아래에서만
//! 데이터베이스/컬렉션/키/명령어에 접근한다. 허용되지 않은 컬렉션, 키 패턴, 쓰기 명령은 즉시 `WIRE.CAPABILITY_DENIED`로 거부된다.

use serde_json::{json, Value};
use spike_v2_read::plan::Reject;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

pub type CapabilityFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Mongo 접근 capability 경계.
pub trait MongoCapability: Send + Sync {
    fn call<'a>(&'a self, collection: &'a str, input: &'a Value) -> CapabilityFuture<'a, Result<Value, Reject>>;
}

/// Redis 접근 capability 경계.
pub trait RedisCapability: Send + Sync {
    fn call<'a>(&'a self, key: &'a str, input: &'a Value) -> CapabilityFuture<'a, Result<Value, Reject>>;
}

/// 화이트리스트로 제한된 관리형 MongoDB capability.
#[derive(Clone)]
pub struct ManagedMongoCapability {
    pub allowed_collections: HashSet<String>,
    pub allowed_commands: HashSet<String>,
    data: Arc<Mutex<HashMap<String, Vec<Value>>>>,
}

impl Default for ManagedMongoCapability {
    fn default() -> Self {
        let mut allowed_collections = HashSet::new();
        allowed_collections.insert("video_distribution".to_string());
        allowed_collections.insert("youtube_watching_data".to_string());
        allowed_collections.insert("video_timeline_emotion_count".to_string());

        let mut allowed_commands = HashSet::new();
        allowed_commands.insert("find".to_string());
        allowed_commands.insert("find_one".to_string());
        allowed_commands.insert("count_documents".to_string());

        Self {
            allowed_collections,
            allowed_commands,
            data: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl ManagedMongoCapability {
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

impl MongoCapability for ManagedMongoCapability {
    fn call<'a>(&'a self, collection: &'a str, input: &'a Value) -> CapabilityFuture<'a, Result<Value, Reject>> {
        Box::pin(async move {
            // 1. 컬렉션 화이트리스트 검증
            if !self.allowed_collections.contains(collection) {
                return Err(Reject {
                    code: "WIRE.CAPABILITY_DENIED",
                    msg: format!("비인가 Mongo 컬렉션 접근 거부: `{collection}`"),
                });
            }

            // 2. 명령어 검증 (읽기 전용 제한)
            let command = input["command"].as_str().unwrap_or("find");
            if !self.allowed_commands.contains(command) {
                return Err(Reject {
                    code: "WIRE.CAPABILITY_DENIED",
                    msg: format!("허용되지 않은 Mongo 명령: `{command}`"),
                });
            }

            // 3. 인메모리/저장소 조회 수행
            let map = self.data.lock().unwrap();
            let docs = map.get(collection).cloned().unwrap_or_default();

            let filter = input.get("filter");
            let filtered: Vec<Value> = if let Some(f) = filter.and_then(Value::as_object) {
                docs.into_iter()
                    .filter(|doc| {
                        f.iter().all(|(k, v)| {
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
                            if desc { cmp.reverse() } else { cmp }
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
                _ => Err(Reject {
                    code: "WIRE.CAPABILITY_DENIED",
                    msg: format!("미구현 Mongo 명령: `{command}`"),
                }),
            }
        })
    }
}

/// 화이트리스트로 제한된 관리형 Redis capability.
#[derive(Clone)]
pub struct ManagedRedisCapability {
    pub allowed_prefixes: Vec<String>,
    pub allowed_commands: HashSet<String>,
    strings: Arc<Mutex<HashMap<String, String>>>,
    hashes: Arc<Mutex<HashMap<String, HashMap<String, String>>>>,
}

impl Default for ManagedRedisCapability {
    fn default() -> Self {
        let allowed_prefixes = vec!["facereview:reco:".to_string(), "facereview:session:".to_string()];
        let mut allowed_commands = HashSet::new();
        allowed_commands.insert("get".to_string());
        allowed_commands.insert("hgetall".to_string());
        allowed_commands.insert("exists".to_string());
        allowed_commands.insert("mget".to_string());

        Self {
            allowed_prefixes,
            allowed_commands,
            strings: Arc::new(Mutex::new(HashMap::new())),
            hashes: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl ManagedRedisCapability {
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

impl RedisCapability for ManagedRedisCapability {
    fn call<'a>(&'a self, key: &'a str, input: &'a Value) -> CapabilityFuture<'a, Result<Value, Reject>> {
        Box::pin(async move {
            // 1. 키 패턴/네임스페이스 검증
            if !self.allowed_prefixes.iter().any(|prefix| key.starts_with(prefix)) {
                return Err(Reject {
                    code: "WIRE.CAPABILITY_DENIED",
                    msg: format!("비인가 Redis 키 접근 거부: `{key}`"),
                });
            }

            // 2. 명령어 검증 (읽기 전용 제한)
            let command = input["command"].as_str().unwrap_or("get");
            if !self.allowed_commands.contains(command) {
                return Err(Reject {
                    code: "WIRE.CAPABILITY_DENIED",
                    msg: format!("허용되지 않은 Redis 명령: `{command}`"),
                });
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
                _ => Err(Reject {
                    code: "WIRE.CAPABILITY_DENIED",
                    msg: format!("미구현 Redis 명령: `{command}`"),
                }),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mongo_capability_allows_whitelisted_collection_and_read_command() {
        let mongo = ManagedMongoCapability::default();
        mongo.seed("video_distribution", vec![
            json!({ "video_id": 1, "dominant_emotion": "happy" }),
            json!({ "video_id": 2, "dominant_emotion": "neutral" }),
            json!({ "video_id": 3, "dominant_emotion": "happy" }),
        ]);

        // find with filter
        let res = mongo.call("video_distribution", &json!({
            "command": "find",
            "filter": { "dominant_emotion": "happy" }
        })).await.unwrap();
        assert_eq!(res["docs"].as_array().unwrap().len(), 2);

        // count_documents
        let count_res = mongo.call("video_distribution", &json!({
            "command": "count_documents",
            "filter": { "dominant_emotion": "neutral" }
        })).await.unwrap();
        assert_eq!(count_res["count"], 1);

        // find_one
        let one_res = mongo.call("video_distribution", &json!({
            "command": "find_one",
            "filter": { "video_id": 2 }
        })).await.unwrap();
        assert_eq!(one_res["doc"]["video_id"], 2);
    }

    #[tokio::test]
    async fn mongo_capability_rejects_unauthorized_collection_or_write_command() {
        let mongo = ManagedMongoCapability::default();

        // 1. 비인가 컬렉션 거부
        let err = mongo.call("admin_secrets", &json!({ "command": "find" })).await.unwrap_err();
        assert_eq!(err.code, "WIRE.CAPABILITY_DENIED");
        assert!(err.msg.contains("비인가 Mongo 컬렉션"));

        // 2. 비인가 쓰기 명령 거부
        let err_write = mongo.call("video_distribution", &json!({
            "command": "insert",
            "doc": { "video_id": 99 }
        })).await.unwrap_err();
        assert_eq!(err_write.code, "WIRE.CAPABILITY_DENIED");
        assert!(err_write.msg.contains("허용되지 않은 Mongo 명령"));

        // 3. 삭제 명령 거부
        let err_del = mongo.call("video_distribution", &json!({
            "command": "delete",
            "filter": { "video_id": 1 }
        })).await.unwrap_err();
        assert_eq!(err_del.code, "WIRE.CAPABILITY_DENIED");
    }

    #[tokio::test]
    async fn redis_capability_allows_whitelisted_keys_and_read_commands() {
        let redis = ManagedRedisCapability::default();
        redis.set_str("facereview:reco:pool", "[\"v1\",\"v2\"]");
        let mut cat_fields = HashMap::new();
        cat_fields.insert("Comedy".to_string(), "[\"v1\"]".to_string());
        redis.set_hash("facereview:reco:category", cat_fields);

        // get
        let get_res = redis.call("facereview:reco:pool", &json!({ "command": "get" })).await.unwrap();
        assert_eq!(get_res["value"], "[\"v1\",\"v2\"]");

        // hgetall
        let hash_res = redis.call("facereview:reco:category", &json!({ "command": "hgetall" })).await.unwrap();
        assert_eq!(hash_res["value"]["Comedy"], "[\"v1\"]");

        // exists
        let exists_res = redis.call("facereview:reco:pool", &json!({ "command": "exists" })).await.unwrap();
        assert_eq!(exists_res["exists"], true);
    }

    #[tokio::test]
    async fn redis_capability_rejects_unauthorized_keys_or_write_commands() {
        let redis = ManagedRedisCapability::default();

        // 1. 비인가 키 네임스페이스 거부
        let err_key = redis.call("other_app:secret", &json!({ "command": "get" })).await.unwrap_err();
        assert_eq!(err_key.code, "WIRE.CAPABILITY_DENIED");
        assert!(err_key.msg.contains("비인가 Redis 키 접근 거부"));

        // 2. 비인가 쓰기 명령 거부
        let err_set = redis.call("facereview:reco:pool", &json!({
            "command": "set",
            "value": "injected"
        })).await.unwrap_err();
        assert_eq!(err_set.code, "WIRE.CAPABILITY_DENIED");
        assert!(err_set.msg.contains("허용되지 않은 Redis 명령"));

        // 3. 비인가 삭제 명령 거부
        let err_del = redis.call("facereview:reco:pool", &json!({
            "command": "del"
        })).await.unwrap_err();
        assert_eq!(err_del.code, "WIRE.CAPABILITY_DENIED");
    }
}
