use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, Knobs};

const DB: &str = "host=localhost dbname=postgres";
const SOURCE: &str = r#"
resource Member { fields { id: Id } }
actor Member
limit maxLabel on LabelItem = atMost 2 where label = "O'Brien"
limit maxUnits on UnitsItem = atMost 2 where units = 7
limit maxDay on DayItem = atMost 2 where day = "2026-10-07"
limit maxAmount on AmountItem = atMost 2 where amount = "1.20"
resource LabelItem {
  fields { id: Id; owner: Member; label: Text }
  rows read when true
  transition qualify { allow true; from label = "no"; to label = "O'Brien" }
  expose apply qualify { target id; bulk maxRows 1 }
  invariant maxLabel per owner
}
resource UnitsItem {
  fields { id: Id; owner: Member; units: Int }
  rows read when true
  transition qualify { allow true; from units = 0; to units = 7 }
  expose apply qualify { target id; bulk maxRows 1 }
  invariant maxUnits per owner
}
resource DayItem {
  fields { id: Id; owner: Member; day: Date }
  rows read when true
  transition qualify { allow true; from day = "2026-10-06"; to day = "2026-10-07" }
  expose apply qualify { target id; bulk maxRows 1 }
  invariant maxDay per owner
}
resource AmountItem {
  fields { id: Id; owner: Member; amount: Decimal(5,2) }
  rows read when true
  transition qualify { allow true; from amount = "0.00"; to amount = "1.20" }
  expose apply qualify { target id; bulk maxRows 1 }
  invariant maxAmount per owner
}
"#;

fn facts() -> Value {
    load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution
}
fn caller() -> Caller {
    Caller { actor_id: Some(1), now: "2026-10-07T00:00:00Z".into() }
}
fn request(resource: &str, id: i64) -> Value {
    json!({"apply":format!("{resource}.qualify"),"target":{"ids":[id.to_string()]}})
}

#[tokio::test]
async fn locked_count_predicates_bind_text_integer_date_and_decimal_literals() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let schema = format!("aip_capacity_predicate_{unique}");
    sqlgen::set_schema(&schema);
    let facts = facts();
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1);
         INSERT INTO {schema}.label_item(id,owner_id,label) VALUES(11,1,'O''Brien'),(12,1,'O''Brien'),(13,1,'no');
         INSERT INTO {schema}.units_item(id,owner_id,units) VALUES(21,1,7),(22,1,7),(23,1,0);
         INSERT INTO {schema}.day_item(id,owner_id,day) VALUES(31,1,'2026-10-07'),(32,1,'2026-10-07'),(33,1,'2026-10-06');
         INSERT INTO {schema}.amount_item(id,owner_id,amount) VALUES(41,1,1.20),(42,1,1.20),(43,1,0.00)"
    ))
    .await
    .unwrap();

    for (resource, id) in [("LabelItem", 13), ("UnitsItem", 23), ("DayItem", 33), ("AmountItem", 43)] {
        let result = apply(&mut db, &facts, &request(resource, id), &caller(), &Knobs::default()).await;
        assert_eq!(result.unwrap_err().code, "INVARIANT_VIOLATED", "{resource}");
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
