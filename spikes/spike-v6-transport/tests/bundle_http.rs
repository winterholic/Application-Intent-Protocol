//! Delegated decision CD-6 (2026-10-13): the caller picks exposed actions (`expose apply` transitions, `expose create`,
//! `expose compose`) and sends them as one atomic request. Over real HTTP, `/apply` takes
//! `{atomic:[{apply..}|{create,values}]}` and `{compose,targets,steps}` next to the single transition. These tests
//! cover what the single-transition route already guarantees, now for bundles: all-or-nothing, an idempotency key that
//! replays one stored result, recovery from a lost response, commit-time `check` re-evaluation, the create Ref
//! visibility rule (D-A17), the size caps, the contract fingerprint, and that no error names a hidden row.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Project {
  fields { id: Id; owner: Member; name: Text(1..50); archived: Bool }
  rows read when owner = actor
  transition archive { allow owner = actor; from archived = false; to archived = true }
  expose apply archive { target id; bulk maxRows 5 }
  expose compose { bulk maxRows 2; transitions archive; create Log from owner, name }
}
resource Log {
  fields { id: Id; owner: Member; name: Text(1..50) }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner, name }
}
resource Task {
  fields { id: Id; project: Project; author: Member; title: Text(1..100); points: Int }
  rows read when author = actor
  check smallTask when points <= 10
  unique author, title
  expose create { allow author = actor; fields project, author, title, points }
}
"#;

async fn post(address: SocketAddr, token: &str, extra: &str, body: &Value) -> Value {
    let body = body.to_string();
    // the contract header is required; a test that sends its own (a mismatching one) replaces the real one
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_apply(&load_str(SPEC, Form::A).unwrap().execution, IdWire::DecimalString);
    let own = if extra.contains("x-aip-contract") { String::new() } else { format!("x-aip-contract: {fingerprint}\r\n") };
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket
        .write_all(
            format!("POST /apply HTTP/1.1\r\nauthorization: Bearer {token}\r\n{own}{extra}content-length: {}\r\n\r\n{body}", body.len()).as_bytes(),
        )
        .await
        .unwrap();
    let mut response = String::new();
    // a dropped response (x-spike-drop-response) closes the socket without a body
    let _ = timeout(Duration::from_secs(10), socket.read_to_string(&mut response)).await.unwrap();
    match response.split_once("\r\n\r\n") {
        Some((_, body)) if !body.is_empty() => serde_json::from_str(body).unwrap(),
        _ => json!({"dropped": true}),
    }
}

fn task(project: &str, title: &str, points: i64) -> Value {
    json!({"create": "Task", "values": {"project": project, "author": "1", "title": title, "points": points}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn atomic_bundles_and_compose_run_over_http_with_the_apply_guarantees() {
    let facts = load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let schema = format!("aip_bundle_http_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let db = spike_v2_read::connect().await;
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(&db).await;
    // member 1 owns projects 10 and 11; member 2 owns project 20 and a task whose title and project are hidden from member 1
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1),(2); \
         INSERT INTO {schema}.project(id,owner_id,name,archived) VALUES (10,1,'p10',false),(11,1,'p11',false),(20,2,'p20',false); \
         INSERT INTO {schema}.task(id,project_id,author_id,title,points) VALUES (900,20,2,'HIDDEN-TITLE',1)"
    ))
    .await
    .unwrap();
    // the test server honours `x-spike-drop-response` (an ordinary server ignores it)
    let (port, keys) = spike_v6_transport::start_with_apply_contract(facts, IdWire::DecimalString).await;
    let address: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let token = keys.issue(1, 300);
    let count = |table: &'static str| {
        let db = &db;
        let schema = &schema;
        async move { db.query_one(&format!("SELECT count(*) FROM {schema}.{table}"), &[]).await.unwrap().get::<_, i64>(0) }
    };

    // 1. a bundle of a create and a transition commits together and returns the new id
    let request = json!({"atomic": [task("10", "write", 3), {"apply": "Project.archive", "target": {"ids": ["10"]}}]});
    let done = post(address, &token, "", &json!({"key": "b1", "request": request})).await;
    assert_eq!(done["ok"], true, "{done}");
    assert_eq!(done["applied"], json!([{"changed": ["10"], "unchanged": []}]), "{done}");
    assert_eq!(done["created"][0]["resource"], "Task", "{done}");
    let created_id = done["created"][0]["id"].as_str().unwrap().to_string();
    assert!(created_id.chars().all(|c| c.is_ascii_digit()), "ids use the configured wire: {done}");
    assert_eq!(done["tags"], json!(["Project", "Task"]), "{done}");
    assert_eq!((count("task").await, count("aip_idem").await), (2, 1));

    // 2. the same key and request replays the stored result: nothing runs again
    let replay = post(address, &token, "", &json!({"key": "b1", "request": request})).await;
    assert_eq!(
        (replay["ok"].clone(), replay["replayed"].clone(), replay["created"].clone()),
        (json!(true), json!(true), done["created"].clone()),
        "{replay}"
    );
    assert_eq!(count("task").await, 2, "a replay creates no second row");
    let other = post(address, &token, "", &json!({"key": "b1", "request": {"atomic": [task("11", "other", 1)]}})).await;
    assert_eq!(other["code"], "WIRE.IDEMPOTENCY_MISMATCH", "{other}");

    // 3. all or nothing: the transition is not applicable (already archived), so the create before it is rolled back
    let before = count("task").await;
    let rolled = post(
        address,
        &token,
        "",
        &json!({"key": "b2", "request": {"atomic": [task("11", "ghost", 1), {"apply": "Project.archive", "target": {"ids": ["10"]}}]}}),
    )
    .await;
    assert_eq!((rolled["ok"].clone(), rolled["code"].clone()), (json!(false), json!("WIRE.INVALID_STATE")), "{rolled}");
    assert_eq!(count("task").await, before, "the create of a failed bundle leaves no row");
    assert_eq!(count("aip_idem").await, 1, "a failed bundle stores no idempotency result");

    // 4. commit-time `check` is evaluated for the rows the bundle wrote, and the whole bundle rolls back
    let checked = post(address, &token, "", &json!({"key": "b3", "request": {"atomic": [task("11", "fine", 2), task("11", "too big", 99)]}})).await;
    assert_eq!(checked["code"], "WIRE.CHECK_FAILED", "{checked}");
    assert_eq!(count("task").await, before, "the earlier create is rolled back with the failed check");

    // 5. a lost response is recovered with the same key and returns the committed result
    let lost = json!({"key": "b4", "request": {"atomic": [task("11", "lost", 5)]}});
    let dropped = post(address, &token, "x-spike-drop-response: 1\r\n", &lost).await;
    assert_eq!(dropped, json!({"dropped": true}));
    let recovered = post(address, &token, "", &lost).await;
    assert_eq!((recovered["ok"].clone(), recovered["replayed"].clone()), (json!(true), json!(true)), "{recovered}");
    assert_eq!(count("task").await, before + 1, "the dropped response committed exactly once");

    // 6. create Ref visibility (D-A17): a project the caller cannot read is MISSING_TARGET and names nothing about it
    let hidden = post(address, &token, "", &json!({"key": "b5", "request": {"atomic": [task("20", "into-theirs", 1)]}})).await;
    let missing = post(address, &token, "", &json!({"key": "b6", "request": {"atomic": [task("424242", "into-nothing", 1)]}})).await;
    assert_eq!(hidden["code"], "WIRE.MISSING_TARGET", "{hidden}");
    assert_eq!(hidden["msg"], missing["msg"], "a hidden target looks the same as a missing one");
    assert_eq!(missing["code"], "WIRE.MISSING_TARGET", "{missing}");
    assert!(!hidden.to_string().contains("p20"), "{hidden}");

    // 7. a unique collision with a hidden row may say so, but never carries its id or value (O-27)
    let dup = post(address, &token, "", &json!({"key": "b7", "request": {"atomic": [json!({"create": "Task", "values": {"project": "11", "author": "1", "title": "HIDDEN-TITLE", "points": 1}})]}})).await;
    // the hidden task belongs to author 2, so unique (author, title) does not collide for author 1: the create succeeds
    assert_eq!(dup["ok"], true, "{dup}");
    let dup_again = post(address, &token, "", &json!({"key": "b8", "request": {"atomic": [json!({"create": "Task", "values": {"project": "11", "author": "1", "title": "HIDDEN-TITLE", "points": 1}})]}})).await;
    assert_eq!(dup_again["code"], "WIRE.ALREADY_EXISTS", "{dup_again}");
    for token_text in ["900", "p20"] {
        assert!(!dup_again.to_string().contains(token_text), "{dup_again}");
    }

    // 8. size caps: too many operations
    let many: Vec<Value> = (0..21).map(|i| task("11", &format!("n{i}"), 1)).collect();
    let capped = post(address, &token, "", &json!({"key": "b9", "request": {"atomic": many}})).await;
    assert_eq!(capped["code"], "WIRE.BULK_LIMIT", "{capped}");
    let empty = post(address, &token, "", &json!({"key": "b10", "request": {"atomic": []}})).await;
    assert_eq!(empty["code"], "WIRE.BAD_REQUEST", "{empty}");

    // 9. an operation that is not exposed, and a field the contract does not list
    let unexposed = post(address, &token, "", &json!({"key": "b11", "request": {"atomic": [{"create": "Member", "values": {}}]}})).await;
    assert_eq!(unexposed["code"], "WIRE.NOT_EXPOSED", "{unexposed}");
    let extra_field = post(address, &token, "", &json!({"key": "b12", "request": {"atomic": [{"create": "Task", "values": {"project": "11", "author": "1", "title": "x", "points": 1, "id": "5"}}]}})).await;
    assert_eq!(extra_field["code"], "WIRE.FIELD_NOT_EXPOSED", "{extra_field}");

    // 10. compose: archive project 11 and create a Log from values the server reads from that row
    let logs = count("log").await;
    let compose = json!({"compose": "Project", "targets": {"ids": ["11"]}, "steps": [
        {"transition": "archive"},
        {"create": "Log", "values": {"owner": {"item": "owner"}, "name": {"item": "name"}}}
    ]});
    let composed = post(address, &token, "", &json!({"key": "c1", "request": compose})).await;
    assert_eq!(composed["ok"], true, "{composed}");
    assert_eq!(composed["applied"], json!([{"changed": ["11"], "unchanged": []}]), "{composed}");
    assert_eq!(composed["created"][0]["resource"], "Log", "{composed}");
    assert_eq!(count("log").await, logs + 1);
    let composed_replay = post(address, &token, "", &json!({"key": "c1", "request": compose})).await;
    assert_eq!((composed_replay["replayed"].clone(), count("log").await), (json!(true), logs + 1));
    let bad_source = post(address, &token, "", &json!({"key": "c2", "request": {"compose": "Project", "targets": {"ids": ["10"]}, "steps": [{"create": "Log", "values": {"owner": {"item": "id"}, "name": {"item": "name"}}}]}})).await;
    assert_eq!(bad_source["code"], "WIRE.VALUE_SOURCE_NOT_ALLOWED", "{bad_source}");

    // 11. the contract fingerprint is checked before anything runs
    let mismatched = post(
        address,
        &token,
        &format!("x-aip-contract: {}\r\n", "0".repeat(64)),
        &json!({"key": "b13", "request": {"atomic": [task("11", "mismatch", 1)]}}),
    )
    .await;
    assert_eq!(mismatched["code"], "WIRE.CONTRACT_MISMATCH", "{mismatched}");
    // only the requests that committed stored a result: b1, b4, b7 and the compose c1
    assert_eq!(count("aip_idem").await, 4);

    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
