use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, bundle, compose, Knobs};
use std::time::{SystemTime, UNIX_EPOCH};

const DB: &str = aip_test_support::test_database_url!();
const SOURCE: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource SalesOrder {
  fields { id: Id; owner: Member; refreshed: Bool; username: Text; title: Text; created: Time }
  transition refresh {
    allow owner = actor
    from refreshed = false
    to refreshed = true
    update Summary where order = this.id {
      item = latest Item c where c.order = this.id and c.active = true order by c.rank select c.id
      author = latest Item c where c.order = this.id and c.active = true order by c.rank select c.author otherwise this.owner
      username = latest Item c where c.order = this.id and c.active = true order by c.rank select c.username otherwise this.username
      title = latest Item c where c.order = this.id and c.active = true order by c.rank select c.title otherwise this.title
      created = latest Item c where c.order = this.id and c.active = true order by c.rank select c.created otherwise this.created
    }
  }
  expose apply refresh { target id; bulk maxRows 1 }
  expose compose { bulk maxRows 1; transitions refresh }
}
resource Summary { fields { id: Id; order: SalesOrder; item: Item?; author: Member?; username: Text?; title: Text?; created: Time? } }
resource Item { fields { id: Id; order: SalesOrder; active: Bool; rank: Int; author: Member?; username: Text?; title: Text?; created: Time? } }
"#;

#[tokio::test]
async fn five_projections_preserve_one_nullable_candidate_and_fallback_only_for_absence() {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let schema = format!("aip_latest_nullable_{}_{}", std::process::id(), nonce);
    sqlgen::set_schema(&schema);
    let facts = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("DDL `{statement}`: {error}"));
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1),(2); \
         INSERT INTO {schema}.sales_order(id,owner_id,refreshed,username,title,created) VALUES \
         (100,1,false,'fallback-user','fallback-title','2025-03-01T00:00:00Z'), \
         (101,1,false,'fallback-user','fallback-title','2025-03-01T00:00:00Z'); \
         INSERT INTO {schema}.summary(id,order_id) VALUES (300,100),(301,101); \
         INSERT INTO {schema}.item(id,order_id,active,rank,author_id,username,title,created) VALUES \
         (200,100,true,1,2,'older-user','older-title','2025-01-01T00:00:00Z'), \
         (201,100,true,2,NULL,NULL,'selected-title',NULL), \
         (202,100,false,3,2,'excluded-user','excluded-title','2025-02-01T00:00:00Z'); \
         CREATE FUNCTION {schema}.require_selection_isolation() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF current_setting('transaction_isolation') <> 'serializable' THEN \
         RAISE EXCEPTION 'latest projection must run serializable'; END IF; RETURN NEW; END $$; \
         CREATE TRIGGER require_selection_isolation BEFORE UPDATE ON {schema}.summary \
         FOR EACH ROW EXECUTE FUNCTION {schema}.require_selection_isolation()"
    ))
    .await
    .unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-10-31T00:00:00Z".into() };

    for fallback in [false, true] {
        let source = if fallback {
            SOURCE.to_string()
        } else {
            SOURCE
                .replace(" otherwise this.owner", "")
                .replace(" otherwise this.username", "")
                .replace(" otherwise this.title", "")
                .replace(" otherwise this.created", "")
        };
        let facts = load_str(&source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
        for mode in ["apply", "bundle", "compose"] {
            for id in [100, 101] {
                db.batch_execute(&format!(
                    "UPDATE {schema}.sales_order SET refreshed=false WHERE id={id}; \
                 DELETE FROM {schema}.summary WHERE order_id={id}; \
                 INSERT INTO {schema}.summary(id,order_id) VALUES ({}, {id})",
                    id + 200
                ))
                .await
                .unwrap();
                let single = json!({"apply":"SalesOrder.refresh","target":{"ids":[id.to_string()]}});
                match mode {
                    "apply" => {
                        let result = apply(&mut db, &facts, &single, &caller, &Knobs::default()).await.unwrap();
                        assert_eq!(result.changed, [id]);
                    }
                    "bundle" => {
                        let result = bundle(&mut db, &facts, &json!({"atomic":[single]}), &caller, &Knobs::default()).await.unwrap();
                        assert_eq!(result.applied[0].changed, [id]);
                    }
                    "compose" => {
                        let request = json!({"compose":"SalesOrder","targets":{"ids":[id.to_string()]},"steps":[{"transition":"refresh"}]});
                        let result = compose(&mut db, &facts, &request, &caller, &Knobs::default()).await.unwrap();
                        assert_eq!(result.applied[0].changed, [id]);
                    }
                    _ => unreachable!(),
                }
                let row = db
                    .query_one(&format!("SELECT item_id,author_id,username,title,created::text FROM {schema}.summary WHERE order_id={id}"), &[])
                    .await
                    .unwrap();
                let actual = (
                    row.get::<_, Option<i64>>(0),
                    row.get::<_, Option<i64>>(1),
                    row.get::<_, Option<String>>(2),
                    row.get::<_, Option<String>>(3),
                    row.get::<_, Option<String>>(4),
                );
                let expected = if id == 100 {
                    (Some(201), None, None, Some("selected-title".into()), None)
                } else if fallback {
                    (None, Some(1), Some("fallback-user".into()), Some("fallback-title".into()), Some("2025-03-01 00:00:00+00".into()))
                } else {
                    (None, None, None, None, None)
                };
                assert_eq!(actual, expected, "{mode}, order {id}, fallback={fallback}");
            }
        }
    }
    let unrelated = db.query_one(&format!("SELECT username FROM {schema}.item WHERE id=200"), &[]).await.unwrap();
    assert_eq!(unrelated.get::<_, String>(0), "older-user");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
