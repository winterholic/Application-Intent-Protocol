use spike_v1_fixture::{load_str, Form};
use spike_v2_read::sqlgen;

#[test]
fn non_primary_typed_id_field_has_a_postgres_column() {
    let source = r#"
resource Member { fields { id: Id } }
actor Member
resource SalesOrder { fields { id: Id; successor: SalesOrder? } }
"#;
    let facts = load_str(source, Form::A).unwrap_or_else(|errors| panic!("V1: {errors:?}")).execution;
    assert_eq!(facts["resources"]["SalesOrder"]["fields"]["successor"]["ty"], "Ref<SalesOrder>?");
    let ddl = sqlgen::create_ddl_in("aip_typed_id_gap", &facts).expect("an accepted self reference needs executable DDL");
    assert!(ddl.iter().any(|statement| statement.contains("successor_id bigint REFERENCES")), "{ddl:?}");
}

#[test]
fn mutual_references_break_the_table_order_cycle_with_a_deferred_foreign_key() {
    let source = r#"
resource First { fields { id: Id; second: Second? } }
resource Second { fields { id: Id; first: First? } }
actor First
"#;
    let facts = load_str(source, Form::A).unwrap_or_else(|errors| panic!("V1: {errors:?}")).execution;
    let ddl = sqlgen::create_ddl_in("aip_ref_cycle", &facts).expect("a reference cycle still has an executable table order");
    let all = ddl.join("\n");
    assert!(all.contains("second_id bigint)"), "the first table is created before its target exists: {all}");
    assert!(all.contains("first_id bigint REFERENCES aip_ref_cycle.first(id)"), "{all}");
    assert!(all.contains("ADD CONSTRAINT") && all.contains("FOREIGN KEY (second_id) REFERENCES aip_ref_cycle.second(id)"), "{all}");
}

#[test]
fn ref_field_keeps_its_existing_foreign_key() {
    let source = r#"
resource Member { fields { id: Id } }
actor Member
resource Child { fields { id: Id; parent: Member } }
"#;
    let facts = load_str(source, Form::A).unwrap().execution;
    let ddl = sqlgen::create_ddl_in("aip_typed_id_fk", &facts).unwrap();
    assert!(ddl.iter().any(|statement| statement.contains("parent_id bigint REFERENCES aip_typed_id_fk.member(id) NOT NULL")), "{ddl:?}");
}
