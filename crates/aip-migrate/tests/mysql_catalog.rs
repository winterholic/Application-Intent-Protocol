//! Every catalog row here is a synthetic regression fixture, not a database observation.
use aip_migrate::mysql_catalog::{CatalogDefault, ColumnRow, IndexRow, LogicalType, normalize_table};

fn column(name: &str, data_type: &str, column_type: &str) -> ColumnRow {
    ColumnRow {
        name: name.into(),
        ordinal_position: 1,
        data_type: data_type.into(),
        column_type: column_type.into(),
        is_nullable: "NO".into(),
        column_default: None,
        extra: String::new(),
        generation_expression: String::new(),
        collation_name: None,
    }
}

#[test]
fn preserves_ordered_primary_keys_unsigned_enum_and_boolean_semantics() {
    let mut id = column("tenant_id", "int", "int unsigned");
    id.ordinal_position = 1;
    let mut code = column("code", "enum", "enum('GENERAL','ADMIN')");
    code.ordinal_position = 2;
    let mut active = column("active", "tinyint", "tinyint(1)");
    active.ordinal_position = 3;
    let mut tiny_counter = column("counter", "tinyint", "tinyint(1) unsigned");
    tiny_counter.ordinal_position = 4;

    let table = normalize_table(
        "membership",
        vec![id, code, active, tiny_counter],
        vec![IndexRow::column("PRIMARY", 0, 2, "code"), IndexRow::column("PRIMARY", 0, 1, "tenant_id")],
    )
    .expect("synthetic INFORMATION_SCHEMA rows should normalize");

    assert_eq!(table.indexes[0].columns, ["tenant_id", "code"]);
    assert!(table.indexes[0].primary);
    assert_eq!(table.columns[0].logical_type, LogicalType::Integer { bits: 32, unsigned: true, display_width: None, boolean_alias: false });
    assert_eq!(table.columns[1].logical_type, LogicalType::Enum(vec!["GENERAL".into(), "ADMIN".into()]));
    assert_eq!(table.columns[2].logical_type, LogicalType::Integer { bits: 8, unsigned: false, display_width: Some(1), boolean_alias: true });
    assert_eq!(table.columns[3].logical_type, LogicalType::Integer { bits: 8, unsigned: true, display_width: Some(1), boolean_alias: true });
    assert!(table.diagnostics.iter().any(|d| d.field == "active" && d.reason.contains("not constrained to 0/1")));
}

#[test]
fn preserves_nullable_default_and_generated_flags_and_reports_unrepresentable_details() {
    let mut nullable = column("nickname", "varchar", "varchar(80)");
    nullable.is_nullable = "YES".into();
    nullable.column_default = Some("guest".into());
    nullable.collation_name = Some("utf8mb4_0900_ai_ci".into());
    let mut generated = column("normalized_email", "varchar", "varchar(255)");
    generated.extra = "STORED GENERATED on update CURRENT_TIMESTAMP".into();
    generated.generation_expression = "lower(email)".into();

    let table = normalize_table("user", vec![nullable, generated], vec![]).unwrap();

    assert!(table.columns[0].nullable);
    assert_eq!(table.columns[0].default, CatalogDefault::Value("guest".into()));
    assert!(table.columns[1].generated);
    assert_eq!(table.columns[1].default, CatalogDefault::NullOrAbsent);
    assert!(table.diagnostics.iter().any(|d| d.field == "normalized_email" && d.reason.contains("generation expression")));
    assert!(table.diagnostics.iter().any(|d| d.field == "normalized_email" && d.reason.contains("on update")));
    assert!(table.diagnostics.iter().any(|d| d.field == "normalized_email" && d.reason.contains("DEFAULT NULL")));
    assert!(table.diagnostics.iter().any(|d| d.field == "nickname" && d.reason.contains("collation")));
}

#[test]
fn reports_malformed_enum_and_rejects_invalid_nullable_metadata() {
    let mut malformed = column("state", "enum", "enum('ACTIVE','BROKEN)");
    let table = normalize_table("task", vec![malformed.clone()], vec![]).unwrap();
    assert!(matches!(table.columns[0].logical_type, LogicalType::Unsupported(_)));
    assert!(table.diagnostics.iter().any(|d| d.field == "state" && d.reason.contains("enum")));

    malformed.is_nullable = "MAYBE".into();
    let error = normalize_table("task", vec![malformed], vec![]).unwrap_err();
    assert!(error.to_string().contains("nullable"));
}
