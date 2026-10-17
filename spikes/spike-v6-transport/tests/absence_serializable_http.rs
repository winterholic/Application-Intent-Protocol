//! Delegated decision CD-5 (security audit O-26) over real HTTP: pairs of simultaneous `/apply` requests that each pass
//! `allow not exists ...` on a snapshot. The server runs the transition in SERIALIZABLE and retries a serialization
//! failure itself, so exactly one request of each pair succeeds and the loser sees the business answer (`WIRE.FORBIDDEN`),
//! not a `WIRE.CONFLICT` the caller would have to retry. The winner's key replays after the fact.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::server::listen_with_apply_contract;
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const SPEC: &str = r#"
enum BookingStatus { REQUESTED, CONFIRMED }
resource Member { fields { id: Id } }
actor Member
resource Room { fields { id: Id; name: Text } rows read when true }
predicate roomFree(r: Room) = not exists Booking where room = r and status = CONFIRMED
resource Booking {
  fields { id: Id; room: Room; guest: Member; status: BookingStatus }
  rows read when true
  transition confirm { allow roomFree(this.room); from status = REQUESTED; to status = CONFIRMED }
  expose apply confirm { target id; bulk maxRows 1 }
}
"#;

const ROOMS: i64 = 6;

/// Sends the request and, like a real client, sends it again with the same key when the server answers that it did not
/// get to run it (a deadline or an unavailable database under a loaded machine). `CONFLICT` is not in that set: the server
/// must have settled serialization failures itself.
async fn post_apply(address: SocketAddr, token: &str, key: &str, id: i64) -> Value {
    for _ in 0..3 {
        let result = post_apply_once(address, token, key, id).await;
        if !matches!(result["code"].as_str(), Some("WIRE.DEADLINE_EXCEEDED" | "WIRE.DB_UNAVAILABLE")) {
            return result;
        }
    }
    post_apply_once(address, token, key, id).await
}

async fn post_apply_once(address: SocketAddr, token: &str, key: &str, id: i64) -> Value {
    let body = json!({"key": key, "request": {"apply": "Booking.confirm", "target": {"ids": [id.to_string()]}}}).to_string();
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_apply(&load_str(SPEC, Form::A).unwrap().execution, IdWire::DecimalString);
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket
        .write_all(
            format!(
                "POST /apply HTTP/1.1\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fingerprint}\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = String::new();
    timeout(Duration::from_secs(10), socket.read_to_string(&mut response)).await.unwrap().unwrap();
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn simultaneous_requests_for_one_room_over_http_confirm_exactly_one() {
    let facts = load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let schema = format!("aip_absence_http_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let db = spike_v2_read::connect().await;
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES (1)")).await.unwrap();
    for room in 1..=ROOMS {
        db.batch_execute(&format!(
            "INSERT INTO {schema}.room(id,name) VALUES ({room},'r{room}'); \
             INSERT INTO {schema}.booking(id,room_id,guest_id,status) VALUES ({room}1,{room},1,'REQUESTED'),({room}2,{room},1,'REQUESTED')"
        ))
        .await
        .unwrap();
    }
    let server =
        listen_with_apply_contract(facts, IdWire::DecimalString, "127.0.0.1:0".parse().unwrap(), aip_test_support::test_database_url!().into())
            .await
            .unwrap();
    let token = server.keys.issue(1, 120);
    let address = server.address;
    let mut calls = vec![];
    for room in 1..=ROOMS {
        for slot in [1, 2] {
            let token = token.clone();
            calls.push(tokio::spawn(async move { (room, slot, post_apply(address, &token, &format!("k-{room}-{slot}"), room * 10 + slot).await) }));
        }
    }
    let mut by_room: std::collections::BTreeMap<i64, Vec<(i64, Value)>> = Default::default();
    for call in calls {
        let (room, slot, result) = call.await.unwrap();
        by_room.entry(room).or_default().push((slot, result));
    }
    let mut winners = vec![];
    for (room, results) in &by_room {
        let ok: Vec<_> = results.iter().filter(|(_, r)| r["ok"] == true).collect();
        let losers: Vec<_> = results.iter().filter(|(_, r)| r["ok"] != true).collect();
        assert_eq!((ok.len(), losers.len()), (1, 1), "room {room}: {results:?}");
        assert_eq!(losers[0].1["code"], "WIRE.FORBIDDEN", "the loser gets the business answer, not a retryable CONFLICT: {results:?}");
        winners.push((*room, ok[0].0));
    }
    let confirmed: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.booking WHERE status='CONFIRMED'"), &[]).await.unwrap().get(0);
    assert_eq!(confirmed, ROOMS, "one confirmed booking per room");
    // the winner's key replays the stored result and does not run again
    let (room, slot) = winners[0];
    let replay = post_apply(address, &token, &format!("k-{room}-{slot}"), room * 10 + slot).await;
    assert_eq!((replay["ok"].clone(), replay["replayed"].clone()), (json!(true), json!(true)), "{replay}");
    let idem: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.aip_idem"), &[]).await.unwrap().get(0);
    assert_eq!(idem, ROOMS, "only the successful requests store a result");
    server.shutdown().await.unwrap();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
