//! Delegated decision CD-5 (security audit O-26) over real HTTP: simultaneous `/apply` requests that each pass
//! `allow not exists ...` on a snapshot. The server runs the transition in SERIALIZABLE and retries a serialization
//! failure itself (D-A23: up to 3 times after the first run), so exactly one request per room succeeds and a loser whose
//! only rival is the winner gets the business answer (`WIRE.FORBIDDEN`), not a `WIRE.CONFLICT`.
//!
//! When many rooms are confirmed at once, PostgreSQL takes relation-level predicate locks for the absence scan of a small
//! table, so requests for different rooms also abort each other and the bounded retries can run out. The contract then
//! answers `WIRE.CONFLICT` (`retry: later`, nothing committed) and the caller sends the same key again; the test asserts that
//! path still ends with one winner per room, no duplicate and one stored result per success.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::server::listen_with_apply_contract;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::Barrier,
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
/// Racing pairs run one after another. A test-only trigger holds each confirming transaction open, so both requests of a
/// pair read the room as free before either commits and every pair is a real serialization conflict.
const PAIRS: i64 = 6;

/// Sends the request and, like a real client, sends it again with the same key when the server answers that it did not
/// get to run it (a deadline or an unavailable database under a loaded machine). `CONFLICT` is resent only when
/// `resend_conflict` is set, and the number of `CONFLICT` answers is returned so the caller can tell the two contracts apart.
async fn post_apply(address: SocketAddr, token: &str, key: &str, id: i64, start: Option<Arc<Barrier>>, resend_conflict: bool) -> (Value, usize) {
    let mut conflicts = 0;
    let mut start = start;
    for _ in 0..6 {
        let result = post_apply_once(address, token, key, id, start.take()).await;
        if result["code"] == "WIRE.CONFLICT" {
            assert_eq!(result["retry"], "later", "a serialization CONFLICT is retryable later with the same key: {result}");
            conflicts += 1;
            if resend_conflict {
                continue;
            }
            return (result, conflicts);
        }
        if !matches!(result["code"].as_str(), Some("WIRE.DEADLINE_EXCEEDED" | "WIRE.DB_UNAVAILABLE")) {
            return (result, conflicts);
        }
    }
    (post_apply_once(address, token, key, id, None).await, conflicts)
}

/// `start` makes racing requests write their bodies only after every rival has connected.
async fn post_apply_once(address: SocketAddr, token: &str, key: &str, id: i64, start: Option<Arc<Barrier>>) -> Value {
    let body = json!({"key": key, "request": {"apply": "Booking.confirm", "target": {"ids": [id.to_string()]}}}).to_string();
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_apply(&load_str(SPEC, Form::A).unwrap().execution, IdWire::DecimalString);
    let mut socket = TcpStream::connect(address).await.unwrap();
    if let Some(start) = start {
        start.wait().await;
    }
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

/// Races both requests of each room and returns the final answers grouped by room, plus the `CONFLICT` answers seen.
async fn race(
    address: SocketAddr,
    token: &str,
    rooms: &[Vec<i64>],
    resend_conflict: bool,
) -> (std::collections::BTreeMap<i64, Vec<(i64, Value)>>, usize) {
    let mut by_room: std::collections::BTreeMap<i64, Vec<(i64, Value)>> = Default::default();
    let mut conflicts = 0;
    for wave in rooms {
        let start = Arc::new(Barrier::new(wave.len() * 2));
        let mut calls = vec![];
        for &room in wave {
            for slot in [1, 2] {
                let (token, start) = (token.to_owned(), start.clone());
                calls.push(tokio::spawn(async move {
                    (room, slot, post_apply(address, &token, &format!("k-{room}-{slot}"), room * 10 + slot, Some(start), resend_conflict).await)
                }));
            }
        }
        for call in calls {
            let (room, slot, (result, seen)) = call.await.unwrap();
            conflicts += seen;
            by_room.entry(room).or_default().push((slot, result));
        }
    }
    (by_room, conflicts)
}

/// One success and one business refusal per room; returns the winning slot of each room.
fn one_winner_per_room(by_room: &std::collections::BTreeMap<i64, Vec<(i64, Value)>>) -> Vec<(i64, i64)> {
    let mut winners = vec![];
    for (room, results) in by_room {
        let ok: Vec<_> = results.iter().filter(|(_, r)| r["ok"] == true).collect();
        let losers: Vec<_> = results.iter().filter(|(_, r)| r["ok"] != true).collect();
        assert_eq!((ok.len(), losers.len()), (1, 1), "room {room}: {results:?}");
        assert_eq!(losers[0].1["code"], "WIRE.FORBIDDEN", "the loser gets the business answer: {results:?}");
        winners.push((*room, ok[0].0));
    }
    winners
}

async fn count(db: &tokio_postgres::Client, sql: String) -> i64 {
    db.query_one(&sql, &[]).await.unwrap().get(0)
}

fn test_db_url() -> String {
    std::env::var("AIP_TEST_DATABASE_URL").unwrap_or_else(|_| spike_v2_read::DB_URL.to_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn simultaneous_requests_for_one_room_over_http_confirm_exactly_one() {
    let facts = load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let schema = format!("aip_absence_http_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let db_url = test_db_url();
    let db = spike_v2_read::connect_with_url(&db_url).await.expect("테스트 PostgreSQL 연결");
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES (1)")).await.unwrap();
    for room in 1..=ROOMS + PAIRS {
        db.batch_execute(&format!(
            "INSERT INTO {schema}.room(id,name) VALUES ({room},'r{room}'); \
             INSERT INTO {schema}.booking(id,room_id,guest_id,status) VALUES ({room}1,{room},1,'REQUESTED'),({room}2,{room},1,'REQUESTED')"
        ))
        .await
        .unwrap();
    }
    let server = listen_with_apply_contract(facts, IdWire::DecimalString, "127.0.0.1:0".parse().unwrap(), db_url).await.unwrap();
    let token = server.keys.issue(1, 120);
    let address = server.address;

    // Every room at once on a cold pool (the shape that ran out of retries): different rooms also abort each other,
    // so a bounded-retry CONFLICT may reach the caller.
    let crowd: Vec<Vec<i64>> = vec![(1..=ROOMS).collect()];
    let (by_room, conflicts) = race(address, &token, &crowd, true).await;
    eprintln!("crowd: {conflicts} WIRE.CONFLICT answers resent with the same key");
    one_winner_per_room(&by_room);

    // A pair whose only rival is the other request of the same room: the server's retries settle it, never CONFLICT.
    // The trigger keeps the first transaction open long enough that the second one always reads before it commits.
    db.batch_execute(&format!(
        "CREATE FUNCTION {schema}.hold_confirm() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(0.05); RETURN NEW; END $$; \
         CREATE TRIGGER hold_confirm BEFORE UPDATE ON {schema}.booking FOR EACH ROW WHEN (NEW.status = 'CONFIRMED') EXECUTE FUNCTION {schema}.hold_confirm()"
    ))
    .await
    .unwrap();
    let pairs: Vec<Vec<i64>> = (ROOMS + 1..=ROOMS + PAIRS).map(|room| vec![room]).collect();
    let (by_room, conflicts) = race(address, &token, &pairs, false).await;
    assert_eq!(conflicts, 0, "a racing pair must be settled by the server: {by_room:?}");
    let winners = one_winner_per_room(&by_room);

    let confirmed = count(&db, format!("SELECT count(*) FROM {schema}.booking WHERE status='CONFIRMED'")).await;
    assert_eq!(confirmed, ROOMS + PAIRS, "one confirmed booking per room");
    let duplicated = count(
        &db,
        format!("SELECT count(*) FROM (SELECT room_id FROM {schema}.booking WHERE status='CONFIRMED' GROUP BY room_id HAVING count(*) > 1) d"),
    )
    .await;
    assert_eq!(duplicated, 0, "no room has two confirmed bookings");
    // the winner's key replays the stored result and does not run again
    let (room, slot) = winners[0];
    let (replay, _) = post_apply(address, &token, &format!("k-{room}-{slot}"), room * 10 + slot, None, false).await;
    assert_eq!((replay["ok"].clone(), replay["replayed"].clone()), (json!(true), json!(true)), "{replay}");
    let idem = count(&db, format!("SELECT count(*) FROM {schema}.aip_idem")).await;
    assert_eq!(idem, ROOMS + PAIRS, "only the successful requests store a result; a CONFLICT stores nothing");
    server.shutdown().await.unwrap();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
