use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, bundle, compose, Knobs};
use std::time::{SystemTime, UNIX_EPOCH};

const DB: &str = aip_test_support::test_database_url!();
const SOURCE: &str = r#"
actor Member
resource Member { fields { id: Id } }
resource SalesOrder {
  fields { id: Id; owner: Member; done: Bool }
  rows read when owner = actor
  transition finish {
    from done = false
    to done = true
    allow owner = actor
    update Item where id = latest Selection chosen where chosen.order = this and chosen.owner = actor order by chosen.id select chosen.item { active = false }
  }
  expose apply finish { target id; bulk maxRows 1 }
  expose compose { bulk maxRows 1; transitions finish }
}
resource Selection { fields { id: Id; order: SalesOrder; owner: Member; item: Item } }
resource Item { fields { id: Id; active: Bool } rows read when true }
"#;

#[tokio::test]
async fn selected_child_match_is_serializable_atomic_and_missing_selection_refuses() {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let schema = format!("aip_latest_match_{}_{}", std::process::id(), nonce);
    sqlgen::set_schema(&schema);
    let facts = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    assert_eq!(facts["resources"]["SalesOrder"]["transitions"]["finish"]["isolation"], "serializable");
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1),(2); \
         INSERT INTO {schema}.sales_order(id,owner_id,done) VALUES(100,1,false),(101,1,false); \
         INSERT INTO {schema}.item(id,active) VALUES(200,true),(201,true),(202,true); \
         INSERT INTO {schema}.selection(id,order_id,owner_id,item_id) VALUES(1,100,1,200),(2,100,1,201),(3,100,2,202); \
         CREATE FUNCTION {schema}.isolation_check() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF current_setting('transaction_isolation') <> 'serializable' THEN RAISE EXCEPTION 'selected match isolation'; END IF; RETURN NEW; END $$; \
         CREATE TRIGGER isolation_check BEFORE UPDATE ON {schema}.item FOR EACH ROW EXECUTE FUNCTION {schema}.isolation_check()"
    ))
    .await
    .unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-11-06T00:00:00Z".into() };
    for mode in ["apply", "bundle", "compose"] {
        for order in [100, 101] {
            db.batch_execute(&format!("UPDATE {schema}.sales_order SET done=false; ALTER TABLE {schema}.item DISABLE TRIGGER isolation_check; UPDATE {schema}.item SET active=true; ALTER TABLE {schema}.item ENABLE TRIGGER isolation_check"))
                .await.unwrap();
            let request = json!({"apply":"SalesOrder.finish","target":{"ids":[order.to_string()]}});
            let result = match mode {
                "apply" => apply(&mut db, &facts, &request, &caller, &Knobs::default()).await.map(|_| ()),
                "bundle" => bundle(&mut db, &facts, &json!({"atomic":[request]}), &caller, &Knobs::default()).await.map(|_| ()),
                "compose" => compose(
                    &mut db,
                    &facts,
                    &json!({"compose":"SalesOrder","targets":{"ids":[order.to_string()]},"steps":[{"transition":"finish"}]}),
                    &caller,
                    &Knobs::default(),
                )
                .await
                .map(|_| ()),
                _ => unreachable!(),
            };
            if order == 100 {
                result.unwrap();
            } else {
                assert_eq!(result.unwrap_err().code, "WIRE.EFFECT_TARGET_MISSING", "{mode}");
            }
            let parent = db.query_one(&format!("SELECT done FROM {schema}.sales_order WHERE id={order}"), &[]).await.unwrap();
            assert_eq!(parent.get::<_, bool>(0), order == 100, "{mode}: missing selection must roll back the parent");
            let children = db.query(&format!("SELECT id,active FROM {schema}.item ORDER BY id"), &[]).await.unwrap();
            let state: Vec<(i64, bool)> = children.iter().map(|row| (row.get(0), row.get(1))).collect();
            assert_eq!(state, vec![(200, true), (201, order != 100), (202, true)], "{mode}: latest authorized mapping only");
        }
        let wrong_actor = Caller { actor_id: Some(2), now: caller.now.clone() };
        let result = apply(&mut db, &facts, &json!({"apply":"SalesOrder.finish","target":{"ids":["100"]}}), &wrong_actor, &Knobs::default()).await;
        assert!(result.is_err(), "{mode}: another actor cannot use the server selection");
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
