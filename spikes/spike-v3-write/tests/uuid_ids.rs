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
 expose create { fields id, title, archived allow true }
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
    db.batch_execute(&spike_v3_write::audit_ddl("aip_uuid_engine_test")).await.unwrap();
    db.batch_execute(
        "INSERT INTO aip_uuid_engine_test.aip_audit(via,resource,transition,target_id,before,after) VALUES('apply','Old','seed',42,'{}','{}')",
    )
    .await
    .unwrap();
    db.batch_execute(&spike_v3_write::audit_ddl_with_uuid("aip_uuid_engine_test")).await.unwrap();
    spike_v3_write::set_audit_table(Some("aip_uuid_engine_test.aip_audit".into())).unwrap();
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
    let supplied = "aaaaaaaa-0000-4000-8000-000000000001";
    let explicit = bundle_in(
        &tx,
        &facts,
        &json!({"atomic":[{"create":"Note","values":{"id":supplied,"title":"chosen","archived":false}}]}),
        &caller,
        &Knobs::default(),
        IdWire::DecimalString,
    )
    .await
    .unwrap();
    assert_eq!(explicit.created_rows[0].1.unwrap().to_string(), supplied);
    let bad = apply_in_with_wire(
        &tx,
        &facts,
        &json!({"apply":"Note.Archive","target":{"ids":[supplied.to_uppercase()]}}),
        &caller,
        &Knobs::default(),
        IdWire::DecimalString,
    )
    .await
    .unwrap_err();
    assert_eq!(bad.code, "WIRE.BAD_VALUE");

    let changed =
        apply_in_with_wire(&tx, &facts, &json!({"apply":"Note.Archive","target":{"ids":[id]}}), &caller, &Knobs::default(), IdWire::DecimalString)
            .await
            .unwrap();
    assert_eq!(changed.changed.len(), 1);
    tx.commit().await.unwrap();
    spike_v3_write::set_audit_table(None).unwrap();
    let audit: Vec<String> =
        db.query("SELECT target_id FROM aip_uuid_engine_test.aip_audit ORDER BY id", &[]).await.unwrap().iter().map(|r| r.get(0)).collect();
    assert_eq!(audit, vec!["42".to_string(), id.clone()]);

    let req = json!({"read":"Note","select":["id","title","archived"],"filter":[{"field":"id","op":"eq","value":id}],"limit":10});
    let p = plan_read_with_wire(&facts, &req, &caller, IdWire::DecimalString).unwrap();
    let rows = execute(&mut db, &p).await.unwrap();
    assert_eq!(rows[0]["id"], id);
    assert_eq!(rows[0]["archived"], true);
    db.batch_execute("DROP SCHEMA aip_uuid_engine_test CASCADE").await.unwrap();
}
