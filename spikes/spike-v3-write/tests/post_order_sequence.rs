//! Validate the existing UUID/numeric-companion design before FlaskBB activation.
//! SQL below seeds a disposable fixture; it is not a production import executor.
use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, Knobs};
use std::time::{SystemTime, UNIX_EPOCH};

const DB: &str = aip_test_support::test_database_url!();
const SOURCE: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Post {
  fields { id: Uuid; owner: Member; published: Bool; source_order: PostOrder? }
  rows read when owner = actor
  unique source_order
  check sourceOrderBelongsToPost when source_order = null or source_order.post = this
  transition publish {
    allow owner = actor and not exists Post where published = true and source_order = null
    from published = false
    to published = true
    create PostOrder { post = this }
    update Post where id = this { source_order = latest PostOrder mapped where mapped.post = this order by mapped.id select mapped.id }
  }
  expose apply publish { target id; bulk maxRows 1 }
}
resource PostOrder {
  fields { id: Id; post: Post }
  rows read when post.owner = actor
  unique post
}
"#;

fn request(id: &str) -> serde_json::Value {
    json!({"apply":"Post.publish","target":{"ids":[id]}})
}

#[tokio::test]
async fn imported_order_native_high_water_rollback_and_two_connection_publish() {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let schema = format!("aip_post_order_{}_{}", std::process::id(), nonce);
    sqlgen::set_schema(&schema);
    let facts = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    assert_eq!(facts["resources"]["Post"]["transitions"]["publish"]["isolation"], "serializable");
    let mut db = connect_with_url(DB).await.unwrap();
    for ddl in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&ddl).await.unwrap_or_else(|error| panic!("DDL `{ddl}`: {error}"));
    }
    let older = "ffffffff-ffff-4fff-8fff-ffffffffffff";
    let newer = "00000000-0000-4000-8000-000000000001";
    let failed = "11111111-1111-4111-8111-111111111111";
    let first = "22222222-2222-4222-8222-222222222222";
    let second = "33333333-3333-4333-8333-333333333333";
    let unknown = "44444444-4444-4444-8444-444444444444";
    let blocked = "55555555-5555-4555-8555-555555555555";
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1),(2); \
         INSERT INTO {schema}.post(id,owner_id,published) VALUES \
         ('{older}',1,true),('{newer}',1,true),('{failed}',1,false), \
         ('{first}',1,false),('{second}',1,false),('{blocked}',1,false); \
         INSERT INTO {schema}.post_order(id,post_id) VALUES (41,'{older}'),(42,'{newer}'); \
         UPDATE {schema}.post SET source_order_id=41 WHERE id='{older}'; \
         UPDATE {schema}.post SET source_order_id=42 WHERE id='{newer}'; \
         SELECT setval(pg_get_serial_sequence('{schema}.post_order','id'),90,true); \
         CREATE FUNCTION {schema}.check_publish() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF current_setting('transaction_isolation') <> 'serializable' THEN \
         RAISE EXCEPTION 'publish requires serializable'; END IF; \
         IF NEW.post_id='{failed}'::uuid THEN RAISE EXCEPTION 'injected effect failure'; END IF; \
         RETURN NEW; END $$; \
         CREATE TRIGGER check_publish BEFORE INSERT ON {schema}.post_order \
         FOR EACH ROW EXECUTE FUNCTION {schema}.check_publish()"
    ))
    .await
    .unwrap();
    let winner = db.query_one(&format!("SELECT post_id::text FROM {schema}.post_order ORDER BY id DESC LIMIT 1"), &[]).await.unwrap();
    assert_eq!(winner.get::<_, String>(0), newer, "source integer order must win over reversed public UUID order");
    let caller = Caller { actor_id: Some(1), now: "2026-11-01T00:00:00Z".into() };

    let denied = Caller { actor_id: Some(2), now: caller.now.clone() };
    let rejection = apply(&mut db, &facts, &request(first), &denied, &Knobs::default()).await.unwrap_err();
    assert_eq!(rejection.code, "WIRE.MISSING_TARGET", "an invisible target must not disclose its existence");
    let denied_sequence = db.query_one(&format!("SELECT last_value FROM {schema}.post_order_id_seq"), &[]).await.unwrap().get::<_, i64>(0);
    assert_eq!(denied_sequence, 90, "a denied actor must not allocate a companion ID");
    let rejection = apply(&mut db, &facts, &request(failed), &caller, &Knobs::default()).await.unwrap_err();
    assert_eq!(rejection.code, "WIRE.INTERNAL");
    assert!(rejection.msg.contains("P0001"), "the injected trigger must reject the effect: {rejection:?}");
    let rolled_back = db.query_one(&format!("SELECT published,source_order_id FROM {schema}.post WHERE id='{failed}'"), &[]).await.unwrap();
    assert!(!rolled_back.get::<_, bool>(0));
    assert!(rolled_back.get::<_, Option<i64>>(1).is_none());
    assert_eq!(db.query_one(&format!("SELECT count(*) FROM {schema}.post_order"), &[]).await.unwrap().get::<_, i64>(0), 2);
    let consumed = db.query_one(&format!("SELECT last_value FROM {schema}.post_order_id_seq"), &[]).await.unwrap().get::<_, i64>(0);
    assert!(consumed > 90, "a rolled-back allocation may leave a gap; never rewind it");

    let mut other = connect_with_url(DB).await.unwrap();
    let first_request = request(first);
    let second_request = request(second);
    let knobs = Knobs { pause_before_commit_ms: 50, ..Default::default() };
    let (a, b) = tokio::join!(apply(&mut db, &facts, &first_request, &caller, &knobs), apply(&mut other, &facts, &second_request, &caller, &knobs));
    assert_eq!(a.unwrap().changed[0].to_string(), first);
    assert_eq!(b.unwrap().changed[0].to_string(), second);
    let native = db
        .query(&format!("SELECT p.id::text,p.source_order_id,o.post_id::text FROM {schema}.post p JOIN {schema}.post_order o ON o.id=p.source_order_id WHERE p.id IN ('{first}','{second}') ORDER BY o.id"), &[])
        .await
        .unwrap();
    assert_eq!(native.len(), 2);
    let ids: Vec<i64> = native.iter().map(|row| row.get(1)).collect();
    assert!(ids[0] > consumed && ids[1] > ids[0]);
    for row in native {
        assert_eq!(row.get::<_, String>(0), row.get::<_, String>(2), "both directions must point to the same public UUID");
    }
    let rejection = apply(&mut db, &facts, &request(first), &caller, &Knobs::default()).await.unwrap_err();
    assert_eq!(rejection.code, "WIRE.INVALID_STATE", "a published post must not receive another companion");
    let before = db.query_one(&format!("SELECT last_value FROM {schema}.post_order_id_seq"), &[]).await.unwrap().get::<_, i64>(0);
    db.batch_execute(&format!("INSERT INTO {schema}.post(id,owner_id,published) VALUES ('{unknown}',1,true)")).await.unwrap();
    let rejection = apply(&mut db, &facts, &request(blocked), &caller, &Knobs::default()).await.unwrap_err();
    assert_eq!(rejection.code, "WIRE.FORBIDDEN", "an unmapped published row must block allocation");
    let after = db.query_one(&format!("SELECT last_value FROM {schema}.post_order_id_seq"), &[]).await.unwrap().get::<_, i64>(0);
    assert_eq!(before, after, "authorization/coverage rejection must not allocate");
    let preserved = db.query(&format!("SELECT id,post_id::text FROM {schema}.post_order WHERE id<=90 ORDER BY id"), &[]).await.unwrap();
    assert_eq!(
        preserved.iter().map(|row| (row.get::<_, i64>(0), row.get::<_, String>(1))).collect::<Vec<_>>(),
        [(41, older.into()), (42, newer.into())]
    );
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
