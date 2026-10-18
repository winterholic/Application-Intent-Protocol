//! `User.id: Uuid` actor가 native Mongo capability에서 자기 원본 UUID 문서를 읽는지 실제 mongod로 확인한다.
//! 공유 fixture(15441/15442)는 쓰지 않고, 이 테스트 전용 loopback 고포트 mongod를 `AIP_TEST_MONGO_UUID_URL`로 받는다.
use serde_json::json;
use spike_v2_read::id_wire::{parse_uuid, EntityId};
use spike_v4_worker::capability::resolve_actor_uuid;
use spike_v4_worker::{CapabilityPrincipal, MongoCapability, MongoDriverCapability};
use std::{collections::HashSet, time::Duration};

const ALICE: &str = "0b1f4c7e-52a1-4d3e-9c1a-7f2b8d6e4a10";
const BOB: &str = "9d3c2b1a-8e7f-4a6b-b5c4-1e0f9a8b7c6d";
const ALICE_HANDLE: i64 = 7;

fn mongo_url() -> String {
    let value = std::env::var("AIP_TEST_MONGO_UUID_URL").expect("AIP_TEST_MONGO_UUID_URL is required; start a run-dedicated loopback mongod");
    let parsed = url::Url::parse(&value).expect("mongo URL");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"), "test mongod must bind loopback only");
    let port = parsed.port().expect("explicit port");
    assert!(port > 20_000 && port != 15441 && port != 15442, "must not use shared fixture ports");
    value
}

fn principal(actor_id: Option<i64>, actor_uuid: Option<EntityId>) -> CapabilityPrincipal {
    CapabilityPrincipal { actor_id, actor_uuid, grants: ["mongo.youtube_watching_data".to_string()].into_iter().collect::<HashSet<_>>() }
}

fn alice() -> CapabilityPrincipal {
    principal(Some(ALICE_HANDLE), Some(parse_uuid(ALICE).unwrap()))
}

async fn videos(driver: &MongoDriverCapability, who: &CapabilityPrincipal, filter: serde_json::Value) -> Vec<i64> {
    let result = driver
        .call(who, "youtube_watching_data", &json!({"command":"find","filter":filter,"projection":{"user_id":1,"video_id":1}}))
        .await
        .expect("find");
    let mut ids: Vec<i64> = result["docs"].as_array().expect("docs").iter().map(|d| d["video_id"].as_i64().unwrap()).collect();
    ids.sort_unstable();
    ids
}

async fn denied(driver: &MongoDriverCapability, who: &CapabilityPrincipal, input: serde_json::Value) -> &'static str {
    driver.call(who, "youtube_watching_data", &input).await.expect_err("must be rejected").code
}

#[tokio::test]
async fn uuid_actor_reads_only_its_own_original_uuid_documents() {
    let uri = mongo_url();
    let database = format!("aip_principal_uuid_{}", std::process::id());
    let direct = mongodb::Client::with_uri_str(&uri).await.expect("run-dedicated mongod");
    let collection = direct.database(&database).collection::<mongodb::bson::Document>("youtube_watching_data");
    collection.drop().await.expect("reset");
    collection
        .insert_many([
            mongodb::bson::doc! {"user_id":ALICE, "video_id":1_i64},
            mongodb::bson::doc! {"user_id":BOB, "video_id":2_i64},
            // 같은 숫자 handle/문자열 handle 문서는 UUID actor의 문서가 아니다.
            mongodb::bson::doc! {"user_id":ALICE_HANDLE, "video_id":3_i64},
            mongodb::bson::doc! {"user_id":ALICE_HANDLE.to_string(), "video_id":4_i64},
            mongodb::bson::doc! {"user_id":ALICE.to_uppercase(), "video_id":5_i64},
        ])
        .await
        .expect("seed");
    let driver = MongoDriverCapability::connect(&uri, &database, 2, Duration::from_secs(2)).await.expect("driver readiness");

    // 본인 문서만 보이고, 숫자·문자열 handle 및 대문자 표기 문서는 구분된다.
    assert_eq!(videos(&driver, &alice(), json!({})).await, vec![1]);
    assert_eq!(videos(&driver, &alice(), json!({"video_id":{"$in":[1,2,3,4,5]}})).await, vec![1]);
    assert_eq!(videos(&driver, &alice(), json!({"user_id":ALICE})).await, vec![1]);
    let count = driver.call(&alice(), "youtube_watching_data", &json!({"command":"count_documents","filter":{}})).await.unwrap();
    assert_eq!(count["count"], 1);

    // 외국 UUID, 내부 handle, 연산자 우회는 여전히 거부된다.
    for filter in [
        json!({"user_id":BOB}),
        json!({"user_id":ALICE_HANDLE}),
        json!({"user_id":ALICE_HANDLE.to_string()}),
        json!({"user_id":ALICE.to_uppercase()}),
        json!({"user_id":{"$ne":ALICE}}),
        json!({"user_id":{"$in":[ALICE, BOB]}}),
    ] {
        assert_eq!(denied(&driver, &alice(), json!({"command":"find","filter":filter})).await, "WIRE.ACCESS_DENIED");
    }
    assert_eq!(denied(&driver, &alice(), json!({"command":"find","filter":{"$or":[{"user_id":BOB}]}})).await, "WIRE.BAD_VALUE");

    // 다른 UUID actor는 자기 문서만, guest와 UUID가 아닌 actor_uuid는 거부된다.
    let bob = principal(Some(8), Some(parse_uuid(BOB).unwrap()));
    assert_eq!(videos(&driver, &bob, json!({})).await, vec![2]);
    assert_eq!(denied(&driver, &principal(None, None), json!({"command":"find","filter":{}})).await, "WIRE.ACCESS_DENIED");
    assert_eq!(
        denied(&driver, &principal(Some(ALICE_HANDLE), Some(EntityId::Number(ALICE_HANDLE))), json!({"command":"find","filter":{}})).await,
        "WIRE.ACCESS_DENIED"
    );

    // 정수 actor는 기존대로 숫자 문서만 본다. UUID/문자열 문서는 보이지 않는다.
    assert_eq!(videos(&driver, &principal(Some(ALICE_HANDLE), None), json!({})).await, vec![3]);
    assert_eq!(
        denied(&driver, &principal(Some(ALICE_HANDLE), None), json!({"command":"find","filter":{"user_id":ALICE}})).await,
        "WIRE.ACCESS_DENIED"
    );

    direct.database(&database).drop().await.expect("cleanup");
}

#[test]
fn session_public_id_resolves_to_a_canonical_uuid_or_fails_closed() {
    assert!(matches!(resolve_actor_uuid(Some(7), Some(ALICE)), Ok(Some(EntityId::Uuid(_)))));
    assert!(matches!(resolve_actor_uuid(Some(7), None), Ok(None)));
    // 정수 actor의 공개 id는 자기 10진 id다. 숫자 handle로 UUID 경로를 만들지 않는다.
    assert!(matches!(resolve_actor_uuid(Some(7), Some("7")), Ok(None)));
    for (actor, public) in [
        (Some(7), "8"),
        (None, "7"),
        (Some(7), ""),
        (Some(7), "not-a-uuid"),
        (Some(7), "0B1F4C7E-52A1-4D3E-9C1A-7F2B8D6E4A10"),
        (Some(7), "0b1f4c7e52a14d3e9c1a7f2b8d6e4a10"),
    ] {
        assert_eq!(resolve_actor_uuid(actor, Some(public)).unwrap_err().code, "WIRE.ACCESS_DENIED", "{public:?}");
    }
}
