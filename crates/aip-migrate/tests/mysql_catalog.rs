//! Every catalog row here is a synthetic regression fixture, not a database observation.
use aip_migrate::mysql_catalog::{CatalogDefault, ColumnRow, ForeignKeyRow, IndexRow, LogicalType, normalize_foreign_keys, normalize_table};

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
    generated.ordinal_position = 2;
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

#[test]
fn rejects_duplicate_column_ordinals_and_indexes_that_reference_missing_columns() {
    let first = column("first", "int", "int");
    let mut second = column("second", "int", "int");
    second.ordinal_position = first.ordinal_position;
    assert!(normalize_table("sample", vec![first, second], vec![]).unwrap_err().to_string().contains("ordinal"));

    let table = normalize_table("sample", vec![column("present", "int", "int")], vec![IndexRow::column("idx_missing", 1, 1, "absent")]);
    assert!(table.unwrap_err().to_string().contains("absent"));
}

#[test]
fn rejects_indexes_with_inconsistent_unique_flags() {
    let mut left = column("left_id", "int", "int");
    left.ordinal_position = 1;
    let mut right = column("right_id", "int", "int");
    right.ordinal_position = 2;
    let error =
        normalize_table("sample", vec![left, right], vec![IndexRow::column("mixed", 0, 1, "left_id"), IndexRow::column("mixed", 1, 2, "right_id")])
            .unwrap_err();
    assert!(error.to_string().contains("non_unique"));
}

#[test]
fn reports_varchar_length_that_text_manifest_does_not_represent() {
    let table = normalize_table("sample", vec![column("label", "varchar", "varchar(80)")], vec![]).unwrap();
    assert!(matches!(table.columns[0].logical_type, LogicalType::Text));
    assert!(table.diagnostics.iter().any(|d| d.field == "label" && d.reason.contains("length")));
}

fn foreign_key(constraint_name: &str, ordinal_position: u64, column_name: &str, target: (&str, &str, &str), actions: (&str, &str)) -> ForeignKeyRow {
    let (referenced_schema, referenced_table, referenced_column) = target;
    let (delete_rule, update_rule) = actions;
    ForeignKeyRow {
        constraint_schema: "face".into(),
        table_name: "comment".into(),
        constraint_name: constraint_name.into(),
        ordinal_position,
        column_name: column_name.into(),
        referenced_table_schema: Some(referenced_schema.into()),
        referenced_table_name: Some(referenced_table.into()),
        referenced_column_name: Some(referenced_column.into()),
        delete_rule: Some(delete_rule.into()),
        update_rule: Some(update_rule.into()),
    }
}

#[test]
fn preserves_composite_foreign_key_order_targets_actions_and_constraint_identity() {
    let rows = vec![
        foreign_key("fk_pair", 2, "right_id", ("media", "pair_parent", "right_id"), ("CASCADE", "CASCADE")),
        foreign_key("fk_single", 1, "owner_id", ("accounts", "user", "id"), ("SET NULL", "NO ACTION")),
        foreign_key("fk_pair", 1, "left_id", ("media", "pair_parent", "left_id"), ("CASCADE", "CASCADE")),
    ];
    let keys = normalize_foreign_keys("face", "comment", &["left_id".into(), "right_id".into(), "owner_id".into()], rows).unwrap();
    assert_eq!(keys.len(), 2);
    let pair = keys.iter().find(|key| key.name == "fk_pair").unwrap();
    assert_eq!(pair.source_schema, "face");
    assert_eq!(pair.source_table, "comment");
    assert_eq!(pair.columns, ["left_id", "right_id"]);
    assert_eq!(pair.referenced_schema, "media");
    assert_eq!(pair.referenced_table, "pair_parent");
    assert_eq!(pair.referenced_columns, ["left_id", "right_id"]);
    assert_eq!(pair.on_delete.to_string(), "CASCADE");
    assert_eq!(pair.on_update.to_string(), "CASCADE");
    assert_eq!(serde_json::to_value(pair).unwrap()["on_delete"], "CASCADE");
    let single = keys.iter().find(|key| key.name == "fk_single").unwrap();
    assert_eq!(single.referenced_schema, "accounts");
    assert_eq!(single.on_delete.to_string(), "SET NULL");
    assert_eq!(single.on_update.to_string(), "NO ACTION");
}

#[test]
fn rejects_malformed_or_unsupported_foreign_key_metadata() {
    let columns = ["left_id".into(), "right_id".into()];
    let out_of_order = vec![
        foreign_key("fk_pair", 1, "left_id", ("face", "parent", "left_id"), ("CASCADE", "CASCADE")),
        foreign_key("fk_pair", 3, "right_id", ("face", "parent", "right_id"), ("CASCADE", "CASCADE")),
    ];
    assert!(normalize_foreign_keys("face", "comment", &columns, out_of_order).unwrap_err().to_string().contains("sequence"));

    let missing_column = foreign_key("fk_missing", 1, "absent", ("face", "parent", "id"), ("CASCADE", "CASCADE"));
    assert!(normalize_foreign_keys("face", "comment", &columns, vec![missing_column]).unwrap_err().to_string().contains("absent"));

    let unknown_action = vec![foreign_key("fk_unknown", 1, "left_id", ("face", "parent", "id"), ("MAGIC", "CASCADE"))];
    assert!(normalize_foreign_keys("face", "comment", &columns, unknown_action).unwrap_err().to_string().contains("unsupported delete rule"));

    let unsupported_action = vec![foreign_key("fk_default", 1, "left_id", ("face", "parent", "id"), ("SET DEFAULT", "CASCADE"))];
    assert!(normalize_foreign_keys("face", "comment", &columns, unsupported_action).unwrap_err().to_string().contains("SET DEFAULT"));

    let mut missing_target = foreign_key("fk_target", 1, "left_id", ("face", "parent", "id"), ("CASCADE", "CASCADE"));
    missing_target.referenced_table_schema = None;
    assert!(normalize_foreign_keys("face", "comment", &columns, vec![missing_target]).unwrap_err().to_string().contains("referenced"));

    let mut missing_action = foreign_key("fk_action", 1, "left_id", ("face", "parent", "id"), ("CASCADE", "CASCADE"));
    missing_action.delete_rule = None;
    assert!(normalize_foreign_keys("face", "comment", &columns, vec![missing_action]).unwrap_err().to_string().contains("unsupported delete rule"));

    let mut missing_target_column = foreign_key("fk_target_column", 1, "left_id", ("face", "parent", "id"), ("CASCADE", "CASCADE"));
    missing_target_column.referenced_column_name = None;
    assert!(normalize_foreign_keys("face", "comment", &columns, vec![missing_target_column]).unwrap_err().to_string().contains("referenced column"));
}

#[test]
fn facereview_11_models_synthetic_schema_matches_manifest() {
    let manifest_json = include_str!("../../../plan-docs/porting/facereview-fk-manifest.json");
    let manifest_val: serde_json::Value = serde_json::from_str(manifest_json).expect("valid manifest json");
    let models = manifest_val["models"].as_array().expect("models array");
    assert_eq!(models.len(), 11, "FaceReview must have exactly 11 models");

    // Synthetic column helper for FaceReview models
    let col = |name: &str, ord: u64, dt: &str, ct: &str, nullable: bool| {
        let mut c = column(name, dt, ct);
        c.ordinal_position = ord;
        c.is_nullable = if nullable { "YES".into() } else { "NO".into() };
        c
    };

    // Verify all 11 models normalize properly
    for m in models {
        let table_name = m["table"].as_str().unwrap();
        let pk_col = format!("{}_id", table_name);
        let columns = vec![
            col(&pk_col, 1, "varchar", "varchar(36)", false),
            col("created_at", 2, "timestamp", "timestamp", true),
        ];
        let indexes = vec![IndexRow::column("PRIMARY", 0, 1, &pk_col)];
        let normalized = normalize_table(table_name, columns, indexes).expect("synthetic model table should normalize");
        assert_eq!(normalized.name, table_name);
        assert_eq!(normalized.indexes[0].columns, [pk_col.as_str()]);
        assert!(normalized.indexes[0].primary);

        // Normalize FKs if declared in manifest
        let fk_declarations = m["foreign_keys"].as_array().unwrap();
        let mut source_cols = vec![pk_col, "created_at".into()];
        for fk in fk_declarations {
            source_cols.push(fk["column"].as_str().unwrap().into());
        }

        let mut fk_rows = Vec::new();
        for fk in fk_declarations {
            let col_name = fk["column"].as_str().unwrap();
            let ref_table = fk["referenced_table"].as_str().unwrap();
            let ref_col = fk["referenced_column"].as_str().unwrap();
            let on_del = fk["on_delete"].as_str().unwrap();
            let on_upd = fk["on_update"].as_str().unwrap();
            fk_rows.push(ForeignKeyRow {
                constraint_schema: "facereview_baseline".into(),
                table_name: table_name.into(),
                constraint_name: format!("fk_{}_{}", table_name, col_name),
                ordinal_position: 1,
                column_name: col_name.into(),
                referenced_table_schema: Some("facereview_baseline".into()),
                referenced_table_name: Some(ref_table.into()),
                referenced_column_name: Some(ref_col.into()),
                delete_rule: Some(on_del.into()),
                update_rule: Some(on_upd.into()),
            });
        }

        let normalized_fks = normalize_foreign_keys("facereview_baseline", table_name, &source_cols, fk_rows)
            .expect("synthetic FKs should normalize");
        assert_eq!(normalized_fks.len(), fk_declarations.len());

        for expected in fk_declarations {
            let exp_col = expected["column"].as_str().unwrap();
            let actual = normalized_fks
                .iter()
                .find(|k| k.columns == [exp_col])
                .expect("FK for column must exist in normalized results");
            assert_eq!(actual.referenced_table, expected["referenced_table"].as_str().unwrap());
            assert_eq!(actual.referenced_columns, [expected["referenced_column"].as_str().unwrap()]);
            assert_eq!(actual.on_delete.to_string(), expected["on_delete"].as_str().unwrap());
            assert_eq!(actual.on_update.to_string(), expected["on_update"].as_str().unwrap());
        }
    }
}

use aip_migrate::mysql_catalog::{
    CatalogError, MysqlCatalogReader, MysqlDriverConfig, SQL_COLUMNS, SQL_FOREIGN_KEYS, SQL_STATISTICS,
    SQL_TABLES, introspect_schema, introspect_table, normalize_mysql_error_code,
};

struct MockCatalogReader;

impl MysqlCatalogReader for MockCatalogReader {
    fn fetch_tables(&self, _schema: &str) -> impl std::future::Future<Output = Result<Vec<String>, CatalogError>> + Send {
        std::future::ready(Ok(vec!["users".into(), "comments".into()]))
    }

    fn fetch_columns(&self, _schema: &str, table: &str) -> impl std::future::Future<Output = Result<Vec<ColumnRow>, CatalogError>> + Send {
        let cols = match table {
            "users" => vec![
                ColumnRow {
                    name: "id".into(),
                    ordinal_position: 1,
                    data_type: "bigint".into(),
                    column_type: "bigint unsigned".into(),
                    is_nullable: "NO".into(),
                    column_default: None,
                    extra: "auto_increment".into(),
                    generation_expression: String::new(),
                    collation_name: None,
                },
                ColumnRow {
                    name: "email".into(),
                    ordinal_position: 2,
                    data_type: "varchar".into(),
                    column_type: "varchar(255)".into(),
                    is_nullable: "NO".into(),
                    column_default: None,
                    extra: String::new(),
                    generation_expression: String::new(),
                    collation_name: Some("utf8mb4_0900_ai_ci".into()),
                },
            ],
            "comments" => vec![
                ColumnRow {
                    name: "id".into(),
                    ordinal_position: 1,
                    data_type: "bigint".into(),
                    column_type: "bigint unsigned".into(),
                    is_nullable: "NO".into(),
                    column_default: None,
                    extra: "auto_increment".into(),
                    generation_expression: String::new(),
                    collation_name: None,
                },
                ColumnRow {
                    name: "user_id".into(),
                    ordinal_position: 2,
                    data_type: "bigint".into(),
                    column_type: "bigint unsigned".into(),
                    is_nullable: "NO".into(),
                    column_default: None,
                    extra: String::new(),
                    generation_expression: String::new(),
                    collation_name: None,
                },
            ],
            _ => vec![],
        };
        std::future::ready(Ok(cols))
    }

    fn fetch_indexes(&self, _schema: &str, _table: &str) -> impl std::future::Future<Output = Result<Vec<IndexRow>, CatalogError>> + Send {
        std::future::ready(Ok(vec![IndexRow::column("PRIMARY", 0, 1, "id")]))
    }

    fn fetch_foreign_keys(&self, _schema: &str, table: &str) -> impl std::future::Future<Output = Result<Vec<ForeignKeyRow>, CatalogError>> + Send {
        let fks = if table == "comments" {
            vec![ForeignKeyRow {
                constraint_schema: "app_db".into(),
                table_name: "comments".into(),
                constraint_name: "fk_comments_user".into(),
                ordinal_position: 1,
                column_name: "user_id".into(),
                referenced_table_schema: Some("app_db".into()),
                referenced_table_name: Some("users".into()),
                referenced_column_name: Some("id".into()),
                delete_rule: Some("CASCADE".into()),
                update_rule: Some("CASCADE".into()),
            }]
        } else {
            vec![]
        };
        std::future::ready(Ok(fks))
    }
}

#[tokio::test]
async fn mysql_catalog_reader_introspect_pipeline_and_sql_constants() {
    assert!(SQL_TABLES.contains("INFORMATION_SCHEMA.TABLES"));
    assert!(SQL_COLUMNS.contains("INFORMATION_SCHEMA.COLUMNS"));
    assert!(SQL_STATISTICS.contains("INFORMATION_SCHEMA.STATISTICS"));
    assert!(SQL_FOREIGN_KEYS.contains("INFORMATION_SCHEMA.KEY_COLUMN_USAGE"));
    assert!(SQL_FOREIGN_KEYS.contains("INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS"));

    let reader = MockCatalogReader;
    let (table_manifest, fks) = introspect_table(&reader, "app_db", "comments").await.unwrap();
    assert_eq!(table_manifest.name, "comments");
    assert_eq!(table_manifest.columns.len(), 2);
    assert_eq!(table_manifest.indexes.len(), 1);
    assert_eq!(fks.len(), 1);
    assert_eq!(fks[0].referenced_table, "users");

    let full_schema = introspect_schema(&reader, "app_db").await.unwrap();
    assert_eq!(full_schema.len(), 2);
    assert!(full_schema.contains_key("users"));
    assert!(full_schema.contains_key("comments"));
}

#[test]
fn mysql_driver_config_url_parsing_and_secret_redaction() {
    let url = "mysql://db_admin:super_secret_pass@10.0.0.5:3306/facereview_prod?sslmode=required";
    let cfg = MysqlDriverConfig::parse_url(url).expect("valid mysql url should parse");
    assert_eq!(cfg.host, "10.0.0.5");
    assert_eq!(cfg.port, 3306);
    assert_eq!(cfg.user, "db_admin");
    assert_eq!(cfg.database, "facereview_prod");
    assert_eq!(cfg.password.as_deref(), Some("super_secret_pass"));

    let sanitized = cfg.display_sanitized();
    assert!(!sanitized.contains("super_secret_pass"), "secret password must never appear in display_sanitized");
    assert!(sanitized.contains("db_admin:***@10.0.0.5:3306/facereview_prod"));

    // MariaDB scheme
    let maria_url = "mariadb://app:another_secret@localhost/my_db";
    let mcfg = MysqlDriverConfig::parse_url(maria_url).expect("mariadb url should parse");
    assert_eq!(mcfg.user, "app");
    assert_eq!(mcfg.port, 3306);
    assert_eq!(mcfg.database, "my_db");
    assert!(!mcfg.display_sanitized().contains("another_secret"));

    // Invalid scheme
    let pg_url = "postgres://user:pass@localhost:5432/db";
    assert!(MysqlDriverConfig::parse_url(pg_url).is_err());
}

#[test]
fn mysql_error_code_normalization_and_secret_freedom() {
    let err_auth = normalize_mysql_error_code(1045, "Access denied for user 'admin'@'localhost' (using password: YES)");
    assert_eq!(err_auth.code, "DB_AUTH_FAILED");
    assert!(!err_auth.message.contains("admin"), "credentials must be sanitized from error message");

    let err_dup = normalize_mysql_error_code(1062, "Duplicate entry 'test@example.com' for key 'users.email'");
    assert_eq!(err_dup.code, "UNIQUE_VIOLATION");

    let err_deadlock = normalize_mysql_error_code(1213, "Deadlock found when trying to get lock; try restarting transaction");
    assert_eq!(err_deadlock.code, "DEADLOCK_DETECTED");

    let err_timeout = normalize_mysql_error_code(1205, "Lock wait timeout exceeded; try restarting transaction");
    assert_eq!(err_timeout.code, "LOCK_TIMEOUT");

    let err_fk = normalize_mysql_error_code(1451, "Cannot delete or update a parent row: a foreign key constraint fails");
    assert_eq!(err_fk.code, "FOREIGN_KEY_VIOLATION");

    let err_conn = normalize_mysql_error_code(2003, "Can't connect to MySQL server on '127.0.0.1'");
    assert_eq!(err_conn.code, "DB_CONNECT_FAILED");
}

#[tokio::test]
async fn external_introspect_rejects_mysql_url_without_leaking_secrets() {
    let raw_url = "mysql://admin:super_secret_db_password@127.0.0.1:3306/facereview";
    let err = aip_migrate::external::introspect(raw_url, "facereview").await.unwrap_err();
    assert!(err["code"] == "DB_ADAPTER_FEATURE_DISABLED" || err["code"] == "DB_ADAPTER_UNSUPPORTED");
    assert!(!err.to_string().contains("super_secret_db_password"), "secret must never leak in introspect error");
}

