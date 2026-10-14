use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    execute,
    id_wire::IdWire,
    plan::{plan_read_with_wire, Caller},
    sqlgen,
};
use spike_v3_write::{apply_in_with_wire, bundle_in, Knobs};
#[tokio::test]
async fn uuid_default_ref_cursor_and_transition_use_native_pg_uuid() {
    sqlgen::set_schema("aip_uuid_engine_test");
    let facts = load_str(
        r#"actor Member
 resource Member { fields { id: Id } rows read when true }
 resource Note { fields { id: Uuid; title: Text; archived: Bool }
 rows read when true
 transition Archive { from archived = false to archived = true allow true repeat unchanged }
 expose read { select id, title, archived; filter id.eq, id.gt; sort id; budget { rows 10; depth 1; cost 1000; deadline 2s; cursor } }
 expose apply Archive { target id bulk maxRows 10 }
 expose create { fields title, archived allow true }
 }"#,
        Form::A,
    )
    .unwrap()
    .execution;
    let mut db = spike_v2_read::connect_owned_with_url("postgres://localhost/postgres").await.unwrap();
    db.batch_execute("DROP SCHEMA IF EXISTS aip_uuid_engine_test CASCADE").await.unwrap();
    for ddl in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&ddl).await.unwrap();
    }
    let caller = Caller { actor_id: None, now: "2026-10-14T00:00:00Z".into() };
    let tx = db.transaction().await.unwrap();
    let created = bundle_in(
        &tx,
        &facts,
        &json!({"atomic":[{"create":"Note","values":{"title":"hello","archived":false}}]}),
        &caller,
        &Knobs::default(),
        IdWire::DecimalString,
    )
    .await
    .unwrap();
    let id = created.created_rows[0].1.unwrap().to_string();
    assert_eq!(id.len(), 36);
    let changed =
        apply_in_with_wire(&tx, &facts, &json!({"apply":"Note.Archive","target":{"ids":[id]}}), &caller, &Knobs::default(), IdWire::DecimalString)
            .await
            .unwrap();
    assert_eq!(changed.changed.len(), 1);
    tx.commit().await.unwrap();
    let req = json!({"read":"Note","select":["id","title","archived"],"filter":[{"field":"id","op":"eq","value":id}],"limit":10});
    let p = plan_read_with_wire(&facts, &req, &caller, IdWire::DecimalString).unwrap();
    let rows = execute(&mut db, &p).await.unwrap();
    assert_eq!(rows[0]["id"], id);
    assert_eq!(rows[0]["archived"], true);
    db.batch_execute("DROP SCHEMA aip_uuid_engine_test CASCADE").await.unwrap();
}
