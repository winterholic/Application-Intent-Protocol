use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, plan::Caller, sqlgen};
use spike_v3_write::{apply, bundle, Knobs};

const SOURCE: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Log { fields { id: Id; title: Text } }
resource Note {
 fields { id: Id; owner: Member; title: Text(1..8); content: Text?; email: Email?; date: Date?; amount: Decimal(8,2)?; version: Int(0..100) }
 rows read when true
 transition edit { from true; allow owner = actor; patch title, content, email, date, amount; expect version; to version = version + 1; create Log { title = title }; notify owner "note.edited" }
 transition overwrite { from true; allow owner = actor; patch title, content }
 transition compare { from true; allow owner = actor; patch title; expect content }
 expose apply edit { target id; bulk maxRows 1 }
 expose apply overwrite { target id; bulk maxRows 1 }
 expose apply compare { target id; bulk maxRows 1 }
 check notForbidden when title != "blocked"
}
"#;

fn req(values: Value, version: i64) -> Value {
    json!({"apply":"Note.edit","target":{"ids":["10"]},"values":values,"expect":{"version":version}})
}

#[tokio::test]
async fn postgres_general_partial_null_authorization_validation_conflict_and_atomic_effects() {
    sqlgen::set_schema(&format!("aip_update_crud_{}", std::process::id()));
    let schema = sqlgen::schema();
    let facts = load_str(SOURCE, Form::A).expect("definition").execution;
    let mut db = connect().await;
    for ddl in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&ddl).await.unwrap();
    }
    db.batch_execute(&spike_v3_write::audit_ddl(&schema)).await.unwrap();
    spike_v3_write::set_audit_table(Some(format!("{schema}.aip_audit"))).unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1),(2); INSERT INTO {schema}.note(id,owner_id,title,content,version) VALUES(10,1,'before','body',0)"
    ))
    .await
    .unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-10-14T00:00:00Z".into() };
    let k = Knobs { audit_key: Some("crud-test".into()), ..Knobs::default() };
    let result = apply(
        &mut db,
        &facts,
        &req(json!({"title":"새 제목","content":"새 본문","email":"a@example.test","date":"2024-02-29","amount":"123.45"}), 0),
        &caller,
        &k,
    )
    .await
    .unwrap();
    assert_eq!(result.changed, [10]);
    apply(&mut db, &facts, &req(json!({"title":"partial"}), 1), &caller, &k).await.unwrap();
    let row = db.query_one(&format!("SELECT title,content,version,amount::text FROM {schema}.note"), &[]).await.unwrap();
    assert_eq!(
        (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, i64>(2), row.get::<_, String>(3)),
        ("partial".into(), "새 본문".into(), 2, "123.45".into())
    );
    apply(&mut db, &facts, &req(json!({"content":null}), 2), &caller, &k).await.unwrap();
    assert!(db.query_one(&format!("SELECT content FROM {schema}.note"), &[]).await.unwrap().get::<_, Option<String>>(0).is_none());
    let other = Caller { actor_id: Some(2), now: caller.now.clone() };
    assert_eq!(apply(&mut db, &facts, &req(json!({"title":"denied"}), 3), &other, &k).await.unwrap_err().code, "WIRE.FORBIDDEN");
    for values in [
        json!({}),
        json!({"title":null}),
        json!({"title":3}),
        json!({"title":"123456789"}),
        json!({"title":""}),
        json!({"email":"bad"}),
        json!({"date":"2023-02-29"}),
        json!({"amount":"1.234"}),
        json!({"id":"99"}),
        json!({"owner":"2"}),
    ] {
        let error = apply(&mut db, &facts, &req(values.clone(), 3), &caller, &k).await.unwrap_err();
        assert!(matches!(error.code, "WIRE.BAD_VALUE" | "WIRE.FIELD_NOT_EXPOSED"), "{values}: {error:?}");
    }
    assert_eq!(apply(&mut db, &facts, &req(json!({"title":"blocked"}), 3), &caller, &k).await.unwrap_err().code, "WIRE.CHECK_FAILED");
    let count = db
        .query_one(
            &format!("SELECT (SELECT count(*) FROM {schema}.log),(SELECT count(*) FROM {schema}.aip_outbox),(SELECT version FROM {schema}.note)"),
            &[],
        )
        .await
        .unwrap();
    assert_eq!((count.get::<_, i64>(0), count.get::<_, i64>(1), count.get::<_, i64>(2)), (3, 3, 3));
    let mut concurrent = connect().await;
    let slow = Knobs { pause_after_lock_ms: 200, ..Knobs::default() };
    let first = req(json!({"title":"winner"}), 3);
    let second = req(json!({"title":"loser"}), 3);
    let (a, b) = tokio::join!(apply(&mut db, &facts, &first, &caller, &slow), apply(&mut concurrent, &facts, &second, &caller, &k));
    assert!(a.is_ok() != b.is_ok());
    assert_eq!(a.err().or_else(|| b.err()).unwrap().code, "WIRE.CONFLICT");
    let compare = json!({"apply":"Note.compare","target":{"ids":["10"]},"values":{"title":"cmp"},"expect":{"content":null}});
    apply(&mut db, &facts, &compare, &caller, &k).await.unwrap();
    let overwrite = json!({"apply":"Note.overwrite","target":{"ids":["10"]},"values":{"content":"kept"}});
    apply(&mut db, &facts, &overwrite, &caller, &k).await.unwrap();
    assert_eq!(apply(&mut db, &facts, &compare, &caller, &k).await.unwrap_err().code, "WIRE.CONFLICT");
    let atomic = json!({"atomic":[req(json!({"title":"bundled"}),4),req(json!({"title":"stale"}),4)]});
    assert_eq!(bundle(&mut db, &facts, &atomic, &caller, &k).await.unwrap_err().code, "WIRE.CONFLICT");
    let row = db.query_one(&format!("SELECT title,version FROM {schema}.note"), &[]).await.unwrap();
    assert_eq!((row.get::<_, String>(0), row.get::<_, i64>(1)), ("cmp".into(), 4));
    let audit = db.query_one(&format!("SELECT before,after,request_key FROM {schema}.aip_audit ORDER BY id LIMIT 1"), &[]).await.unwrap();
    let before: Value = audit.get(0);
    let after: Value = audit.get(1);
    assert_eq!(before["title"], "before");
    assert_eq!(after["title"], "새 제목");
    assert_eq!(after["content"], "새 본문");
    assert_eq!(audit.get::<_, Option<String>>(2), Some("crud-test".into()));
    spike_v3_write::set_audit_table(None).unwrap();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
