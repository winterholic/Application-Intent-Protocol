use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, sqlgen};

const DB: &str = "host=localhost dbname=postgres";

const SOURCE: &str = r#"
enum State { ACTIVE, INACTIVE }
resource Member { fields { id: Id } }
actor Member
limit uniqueLabel on LabelItem = atMost 1 where label = "O'Brien"
limit uniqueUnits on UnitsItem = atMost 1 where units = 7
limit uniqueDay on DayItem = atMost 1 where day = "2026-10-07"
limit uniqueAmount on AmountItem = atMost 1 where amount = "1.20"
limit uniqueState on StateItem = atMost 1 where state = ACTIVE
resource LabelItem { fields { id: Id; owner: Member; label: Text } invariant uniqueLabel per owner }
resource UnitsItem { fields { id: Id; owner: Member; units: Int } invariant uniqueUnits per owner }
resource DayItem { fields { id: Id; owner: Member; day: Date } invariant uniqueDay per owner }
resource AmountItem { fields { id: Id; owner: Member; amount: Decimal(5,2) } invariant uniqueAmount per owner }
resource StateItem { fields { id: Id; owner: Member; state: State } invariant uniqueState per owner }
"#;

fn facts() -> Value {
    load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution
}

#[tokio::test]
async fn partial_unique_predicates_inline_typed_literals_for_postgres() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let schema = format!("aip_capacity_literals_{unique}");
    sqlgen::set_schema(&schema);
    let mut facts = facts();
    // A source string cannot currently contain a backslash; exercise a valid typed-facts payload too.
    facts["resources"]["LabelItem"]["invariants"]["uniqueLabel"]["enforcement"]["where"]["r"]["lit"] = json!("O'\\path");
    let db = connect_with_url(DB).await.unwrap();
    let ddl = sqlgen::create_ddl_in(&schema, &facts).unwrap();
    assert!(ddl.iter().any(|statement| statement.contains("WHERE (label = E'O''\\\\path'::text)")), "{ddl:?}");
    assert!(ddl.iter().any(|statement| statement.contains("WHERE (units = 7)")), "{ddl:?}");
    assert!(ddl.iter().any(|statement| statement.contains("WHERE (day = E'2026-10-07'::date)")), "{ddl:?}");
    assert!(ddl.iter().any(|statement| statement.contains("WHERE (amount = E'1.20'::numeric(5,2))")), "{ddl:?}");
    assert!(ddl.iter().any(|statement| statement.contains("WHERE (state = 'ACTIVE')")), "{ddl:?}");
    for statement in ddl {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1)")).await.unwrap();

    let label_sql = format!("INSERT INTO {schema}.label_item(owner_id,label) VALUES(1,$1)");
    let label = "O'\\path";
    db.execute(&label_sql, &[&label]).await.unwrap();
    assert_eq!(db.execute(&label_sql, &[&label]).await.unwrap_err().code().unwrap().code(), "23505");
    let units_sql = format!("INSERT INTO {schema}.units_item(owner_id,units) VALUES(1,7)");
    db.execute(&units_sql, &[]).await.unwrap();
    assert_eq!(db.execute(&units_sql, &[]).await.unwrap_err().code().unwrap().code(), "23505");
    let day_sql = format!("INSERT INTO {schema}.day_item(owner_id,day) VALUES(1,'2026-10-07')");
    db.execute(&day_sql, &[]).await.unwrap();
    assert_eq!(db.execute(&day_sql, &[]).await.unwrap_err().code().unwrap().code(), "23505");
    let amount_sql = format!("INSERT INTO {schema}.amount_item(owner_id,amount) VALUES(1,1.20)");
    db.execute(&amount_sql, &[]).await.unwrap();
    assert_eq!(db.execute(&amount_sql, &[]).await.unwrap_err().code().unwrap().code(), "23505");
    let state_sql = format!("INSERT INTO {schema}.state_item(owner_id,state) VALUES(1,'ACTIVE')");
    db.execute(&state_sql, &[]).await.unwrap();
    assert_eq!(db.execute(&state_sql, &[]).await.unwrap_err().code().unwrap().code(), "23505");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
