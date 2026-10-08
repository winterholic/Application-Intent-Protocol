use serde_json::json;
use spike_v1_fixture::{digest, load_str, Form};
use spike_v2_read::{
    connect_with_url, execute,
    id_wire::{encode_rows, IdWire},
    plan::{plan_read_with_wire, Caller},
    sqlgen,
};

const DB: &str = "host=localhost dbname=postgres";
const PAIR: &str = r#"
resource Member {
  fields { id: Id; dept: Dept? }
  rows read when true
  expose read { select id, dept; budget { rows 10; depth 1; deadline 2s; cost 100 } }
}
actor Member
resource Dept { fields { id: Id; lead: Member? } }
"#;

fn facts(source: &str) -> serde_json::Value {
    load_str(source, Form::A).unwrap_or_else(|errors| panic!("V1: {errors:?}")).execution
}

#[test]
fn cyclic_refs_generate_named_fk_after_both_tables() {
    let ddl = sqlgen::create_ddl_in("aip_ref_cycle", &facts(PAIR)).expect("V1-accepted cyclic Ref must have executable DDL");
    let member = ddl.iter().position(|s| s.starts_with("CREATE TABLE aip_ref_cycle.member ")).unwrap();
    let dept = ddl.iter().position(|s| s.starts_with("CREATE TABLE aip_ref_cycle.dept ")).unwrap();
    let deferred = ddl.iter().position(|s| s.starts_with("ALTER TABLE aip_ref_cycle.dept ADD CONSTRAINT aip_fk_")).unwrap();
    assert!(deferred > member && deferred > dept, "{ddl:?}");
    assert!(ddl[member].contains("dept_id bigint REFERENCES aip_ref_cycle.dept(id)"), "{ddl:?}");
    assert!(ddl[deferred].contains("FOREIGN KEY (lead_id) REFERENCES aip_ref_cycle.member(id)"), "{ddl:?}");
    assert!(!ddl[deferred].contains("DEFERRABLE"), "{ddl:?}");
}

#[test]
fn three_resource_cycle_with_tail_defers_only_an_uncreated_target() {
    let source = "resource Alpha { fields { id: Id; beta: Beta? } } actor Alpha \
                  resource Beta { fields { id: Id; gamma: Gamma? } } \
                  resource Gamma { fields { id: Id; alpha: Alpha? } } \
                  resource Tail { fields { id: Id; alpha: Alpha? } }";
    let ddl = sqlgen::create_ddl_in("aip_ref_three", &facts(source)).unwrap();
    let deferred: Vec<_> = ddl.iter().filter(|s| s.starts_with("ALTER TABLE aip_ref_three.") && s.contains("aip_fk_")).collect();
    assert_eq!(deferred.len(), 1, "{ddl:?}");
    assert!(deferred[0].contains("FOREIGN KEY (beta_id) REFERENCES aip_ref_three.beta(id)"), "{ddl:?}");
    assert!(ddl.iter().any(|s| s.contains("gamma_id bigint REFERENCES aip_ref_three.gamma(id)")), "{ddl:?}");
    assert!(ddl.iter().any(|s| s.contains("alpha_id bigint REFERENCES aip_ref_three.alpha(id)")), "{ddl:?}");
}

#[test]
fn lexically_earlier_tail_keeps_its_inline_fk() {
    let source = "resource Alpha { fields { id: Id; beta: Beta? } } actor Alpha \
                  resource Beta { fields { id: Id; gamma: Gamma? } } \
                  resource Gamma { fields { id: Id; beta: Beta? } }";
    let ddl = sqlgen::create_ddl_in("aip_ref_tail", &facts(source)).unwrap();
    let tail = ddl.iter().find(|s| s.starts_with("CREATE TABLE aip_ref_tail.alpha ")).unwrap();
    assert!(tail.contains("beta_id bigint REFERENCES aip_ref_tail.beta(id)"), "{ddl:?}");
    let deferred: Vec<_> = ddl.iter().filter(|s| s.starts_with("ALTER TABLE aip_ref_tail.") && s.contains("aip_fk_")).collect();
    assert_eq!(deferred.len(), 1, "{ddl:?}");
    assert!(deferred[0].contains("FOREIGN KEY (gamma_id) REFERENCES aip_ref_tail.gamma(id)"), "{ddl:?}");
}

#[test]
fn existing_acyclic_fixture_ddl_digest_is_unchanged() {
    let source = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
    let ddl = sqlgen::create_ddl_in("aip_ref_baseline", &facts(source)).unwrap();
    let actual = digest(&json!(ddl));
    assert_eq!(actual, "e8297a345d46f6b2399ca194720e5f7e663e3772a53c5975e33f27126ce2ade5");
    assert!(!ddl.iter().any(|s| s.contains("ADD CONSTRAINT aip_fk_")));
}

#[tokio::test]
async fn postgres_enforces_both_refs_and_read_policy_still_runs() {
    let facts = facts(PAIR);
    let schema =
        format!("aip_ref_cycle_pg_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id,dept_id) VALUES(10,NULL); \
         INSERT INTO {schema}.dept(id,lead_id) VALUES(1,NULL); \
         UPDATE {schema}.member SET dept_id=1 WHERE id=10; \
         UPDATE {schema}.dept SET lead_id=10 WHERE id=1"
    ))
    .await
    .unwrap();
    for sql in [format!("UPDATE {schema}.member SET dept_id=999 WHERE id=10"), format!("UPDATE {schema}.dept SET lead_id=999 WHERE id=1")] {
        let error = db.batch_execute(&sql).await.unwrap_err();
        assert_eq!(error.code().unwrap().code(), "23503", "{sql}");
    }
    let request = json!({"read":"Member","select":["id","dept"]});
    let caller = Caller { actor_id: Some(10), now: "2026-10-08T00:00:00Z".into() };
    let plan = plan_read_with_wire(&facts, &request, &caller, IdWire::DecimalString).unwrap();
    let raw = execute(&mut db, &plan).await.unwrap();
    assert_eq!(encode_rows(raw, &plan.output_type, IdWire::DecimalString).unwrap(), [json!({"id":"10","dept":"1"})]);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
