use spike_v1_fixture::{load_str, Form};

#[test]
fn reference_typed_id_is_an_explicit_shared_primary_key() {
    let definition = r#"
        resource User { fields { id: Uuid } }
        actor User
        resource UserEmotionDna {
            fields { id: User; dna_type: Text(1..50); dna_data: Json }
            rows read when id = actor
            expose read {
                select id, dna_type, dna_data
                budget { rows 10; depth 1; cost 100; deadline 1s }
            }
        }
    "#;

    let compiled = load_str(definition, Form::A).expect("shared-primary-key model compiles");
    let resource = &compiled.execution["resources"]["UserEmotionDna"];
    assert_eq!(resource["fields"]["id"]["ty"], "Ref<User>");
    assert_eq!(resource["idKind"], "uuid");
    assert_eq!(resource["fields"]["dna_data"]["ty"], "Json");
}

#[test]
fn shared_primary_key_must_be_a_non_nullable_resource_reference() {
    let optional = "resource User { fields { id: Uuid } } resource Detail { fields { id: User? } }";
    assert!(load_str(optional, Form::A).is_err(), "primary key cannot be nullable");
}
