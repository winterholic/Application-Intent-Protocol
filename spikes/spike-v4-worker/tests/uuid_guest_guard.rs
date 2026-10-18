//! `actor_id`가 없는 principal(guest)은 `actor_uuid`가 있어도 비공개 문서의 소유자로 인정되지 않는다.
//! 메모리 fake와 실제 mongod(`AIP_TEST_MONGO_UUID_URL`, 전용 loopback 고포트)에 같은 조합표를 적용한다.
use serde_json::{json, Value};
use spike_v2_read::id_wire::{parse_uuid, EntityId};
use spike_v4_worker::{CapabilityPrincipal, MemoryMongoCapability, MongoCapability, MongoDriverCapability};
use std::{collections::HashSet, time::Duration};

const ALICE: &str = "0b1f4c7e-52a1-4d3e-9c1a-7f2b8d6e4a10";
const BOB: &str = "9d3c2b1a-8e7f-4a6b-b5c4-1e0f9a8b7c6d";

fn principal(actor_id: Option<i64>, actor_uuid: Option<EntityId>) -> CapabilityPrincipal {
    let grants = ["mongo.youtube_watching_data", "mongo.video_distribution"].into_iter().map(String::from).collect::<HashSet<_>>();
    CapabilityPrincipal { actor_id, actor_uuid, grants }
}

fn alice() -> Option<EntityId> {
    Some(parse_uuid(ALICE).unwrap())
}

async fn call(cap: &dyn MongoCapability, who: &CapabilityPrincipal, collection: &str, command: &str) -> Result<Value, &'static str> {
    cap.call(who, collection, &json!({"command": command, "filter": {}})).await.map_err(|reject| reject.code)
}

/// 명령별로 본 문서 수와, 문서가 있으면 그 video_id를 돌려준다.
fn seen(command: &str, value: &Value) -> (usize, Option<i64>) {
    match command {
        "find" => (value["docs"].as_array().unwrap().len(), value["docs"][0]["video_id"].as_i64()),
        "find_one" => (usize::from(!value["doc"].is_null()), value["doc"]["video_id"].as_i64()),
        _ => (value["count"].as_u64().unwrap() as usize, None),
    }
}

async fn matrix(cap: &dyn MongoCapability, label: &str) {
    for command in ["find", "find_one", "count_documents"] {
        // guest 계약: 인증된 actor_id가 없으면 actor_uuid가 있어도 비공개 컬렉션은 거부한다.
        for (id, uuid) in [(None, alice()), (None, None)] {
            let who = principal(id, uuid);
            assert_eq!(
                call(cap, &who, "youtube_watching_data", command).await.unwrap_err(),
                "WIRE.ACCESS_DENIED",
                "{label} {command} {id:?} {uuid:?}"
            );
        }
        // UUID actor는 본인 UUID 문서만, 정수 actor는 숫자 문서만 본다.
        let uuid_actor = call(cap, &principal(Some(7), alice()), "youtube_watching_data", command).await.unwrap();
        let integer_actor = call(cap, &principal(Some(7), None), "youtube_watching_data", command).await.unwrap();
        let (uuid_seen, integer_seen) = (seen(command, &uuid_actor), seen(command, &integer_actor));
        assert_eq!(uuid_seen.0, 1, "{label} {command} uuid actor");
        assert_eq!(integer_seen.0, 1, "{label} {command} integer actor");
        if command != "count_documents" {
            assert_eq!((uuid_seen.1, integer_seen.1), (Some(1), Some(3)), "{label} {command}");
        }
        // 공개 컬렉션은 guest도 기존대로 읽는다 (actor_uuid 유무와 무관).
        for who in [principal(None, None), principal(None, alice()), principal(Some(7), alice())] {
            let public = call(cap, &who, "video_distribution", command).await.unwrap();
            let expected = if command == "find_one" { 1 } else { 2 };
            assert_eq!(seen(command, &public).0, expected, "{label} public {command} {:?}", who.actor_id);
        }
    }
}

fn private_docs() -> Vec<Value> {
    vec![
        json!({"user_id": ALICE, "video_id": 1}),
        json!({"user_id": BOB, "video_id": 2}),
        json!({"user_id": 7, "video_id": 3}),
        json!({"user_id": "7", "video_id": 4}),
    ]
}

fn public_docs() -> Vec<Value> {
    vec![json!({"video_id": 10}), json!({"video_id": 11})]
}

#[tokio::test]
async fn memory_fake_does_not_grant_ownership_to_a_guest_with_a_uuid() {
    let mongo = MemoryMongoCapability::new(&["video_distribution", "youtube_watching_data"], &["find", "find_one", "count_documents"]);
    mongo.seed("youtube_watching_data", private_docs());
    mongo.seed("video_distribution", public_docs());
    matrix(&mongo, "memory").await;
}

#[tokio::test]
async fn real_driver_does_not_grant_ownership_to_a_guest_with_a_uuid() {
    let uri = std::env::var("AIP_TEST_MONGO_UUID_URL").expect("AIP_TEST_MONGO_UUID_URL is required; start a run-dedicated loopback mongod");
    let parsed = url::Url::parse(&uri).expect("mongo URL");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"), "test mongod must bind loopback only");
    let port = parsed.port().expect("explicit port");
    assert!(port > 20_000 && port != 15441 && port != 15442, "must not use shared fixture ports");
    let database = format!("aip_uuid_guest_guard_{}", std::process::id());
    let direct = mongodb::Client::with_uri_str(&uri).await.expect("run-dedicated mongod");
    let db = direct.database(&database);
    for (name, docs) in [("youtube_watching_data", private_docs()), ("video_distribution", public_docs())] {
        let collection = db.collection::<mongodb::bson::Document>(name);
        collection.drop().await.expect("reset");
        let docs = docs.into_iter().map(|doc| mongodb::bson::serialize_to_document(&doc).unwrap()).collect::<Vec<_>>();
        collection.insert_many(docs).await.expect("seed");
    }
    let driver = MongoDriverCapability::connect(&uri, &database, 2, Duration::from_secs(2)).await.expect("driver readiness");
    matrix(&driver, "driver").await;
    db.drop().await.expect("cleanup");
}
