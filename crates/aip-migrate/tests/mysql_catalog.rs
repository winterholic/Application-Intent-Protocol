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

/// Structural compatibility test for FaceReview 11 models based on SQLAlchemy source audit (revision 7948b7e7).
///
/// NOTE on scope & limits (synthetic boundary):
/// - Tests structural manifest normalization against 11 model fixtures containing 81 column definitions.
/// - The 13 foreign keys reflect the independent source audit correction from the earlier 10 FK undercount
///   (independent audit of revision 7948b7e7, NOT a user design decision).
/// - Python-side defaults (e.g. generate_uuid, 0, CURRENT_TIMESTAMP) are Python ORM client expressions
///   and are NOT asserted as equivalent to live MySQL server-side DDL DEFAULT clauses.
/// - Does NOT prove 100% equivalence of all MySQL 8.4 data types, server nullability behaviors,
///   or non-manifest secondary indexes against live database observations.
#[test]
fn facereview_11_models_synthetic_fixture_matches_fk_manifest() {
    let manifest_json = include_str!("../../../plan-docs/porting/facereview-fk-manifest.json");
    let manifest_val: serde_json::Value = serde_json::from_str(manifest_json).expect("valid manifest json");
    let models = manifest_val["models"].as_array().expect("models array");
    assert_eq!(models.len(), 11, "FaceReview must have exactly 11 models");

    fn col(name: &str, ord: u64, dt: &str, ct: &str, nullable: bool, default: Option<&str>) -> ColumnRow {
        ColumnRow {
            name: name.into(),
            ordinal_position: ord,
            data_type: dt.into(),
            column_type: ct.into(),
            is_nullable: if nullable { "YES".into() } else { "NO".into() },
            column_default: default.map(Into::into),
            extra: String::new(),
            generation_expression: String::new(),
            collation_name: None,
        }
    }

    struct ModelFixture {
        name: &'static str,
        pk_col: &'static str,
        columns: Vec<ColumnRow>,
        indexes: Vec<IndexRow>,
    }

    let fixtures = vec![
        ModelFixture {
            name: "comment",
            pk_col: "comment_id",
            columns: vec![
                col("comment_id", 1, "varchar", "varchar(36)", false, Some("generate_uuid")),
                col("video_id", 2, "varchar", "varchar(36)", false, None),
                col("user_id", 3, "varchar", "varchar(36)", false, None),
                col("content", 4, "text", "text", false, None),
                col("is_modified", 5, "tinyint", "tinyint(1)", true, Some("0")),
                col("is_deleted", 6, "tinyint", "tinyint(1)", true, Some("0")),
                col("created_at", 7, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
                col("updated_at", 8, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
            ],
            indexes: vec![
                IndexRow::column("PRIMARY", 0, 1, "comment_id"),
                IndexRow::column("idx_video_comments", 1, 1, "video_id"),
                IndexRow::column("idx_video_comments", 1, 2, "is_deleted"),
                IndexRow::column("idx_video_comments", 1, 3, "created_at"),
            ],
        },
        ModelFixture {
            name: "user",
            pk_col: "user_id",
            columns: vec![
                col("user_id", 1, "varchar", "varchar(36)", false, Some("generate_uuid")),
                col("email", 2, "varchar", "varchar(255)", false, None),
                col("password", 3, "varchar", "varchar(255)", false, None),
                col("name", 4, "varchar", "varchar(50)", false, None),
                col("role", 5, "enum", "enum('GENERAL','ADMIN','SUPER_ADMIN')", false, Some("GENERAL")),
                col("profile_image_id", 6, "int", "int", true, Some("0")),
                col("is_tutorial_done", 7, "tinyint", "tinyint(1)", true, Some("0")),
                col("is_verify_email_done", 8, "tinyint", "tinyint(1)", true, Some("0")),
                col("is_deleted", 9, "tinyint", "tinyint(1)", true, Some("0")),
                col("created_at", 10, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
                col("updated_at", 11, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
            ],
            indexes: vec![IndexRow::column("PRIMARY", 0, 1, "user_id"), IndexRow::column("uk_user_email", 0, 1, "email")],
        },
        ModelFixture {
            name: "user_emotion_dna",
            pk_col: "user_id",
            columns: vec![
                col("user_id", 1, "varchar", "varchar(36)", false, None),
                col("dna_type", 2, "varchar", "varchar(50)", false, None),
                col("dna_data", 3, "json", "json", false, None),
                col("based_on_videos", 4, "int", "int", false, None),
                col("generated_at", 5, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
                col("expires_at", 6, "timestamp", "timestamp", false, None),
            ],
            indexes: vec![IndexRow::column("PRIMARY", 0, 1, "user_id")],
        },
        ModelFixture {
            name: "user_emotion_summary",
            pk_col: "user_id",
            columns: vec![
                col("user_id", 1, "varchar", "varchar(36)", false, None),
                col("neutral_seconds", 2, "bigint", "bigint", false, Some("0")),
                col("happy_seconds", 3, "bigint", "bigint", false, Some("0")),
                col("surprise_seconds", 4, "bigint", "bigint", false, Some("0")),
                col("sad_seconds", 5, "bigint", "bigint", false, Some("0")),
                col("angry_seconds", 6, "bigint", "bigint", false, Some("0")),
                col("last_finalized_at", 7, "datetime", "datetime", true, None),
                col("last_session_id", 8, "varchar", "varchar(64)", true, None),
                col("lock_version", 9, "bigint", "bigint", false, Some("0")),
                col("created_at", 10, "datetime", "datetime", false, Some("CURRENT_TIMESTAMP")),
                col("updated_at", 11, "datetime", "datetime", false, Some("CURRENT_TIMESTAMP")),
            ],
            indexes: vec![IndexRow::column("PRIMARY", 0, 1, "user_id")],
        },
        ModelFixture {
            name: "user_favorite_genre",
            pk_col: "user_favorite_genre_id",
            columns: vec![
                col("user_favorite_genre_id", 1, "varchar", "varchar(36)", false, Some("generate_uuid")),
                col("user_id", 2, "varchar", "varchar(36)", false, None),
                col(
                    "genre",
                    3,
                    "enum",
                    "enum('MUSIC','ENTERTAINMENT','GAMING','NEWS','EDUCATION','LIFESTYLE','SPORTS','COMEDY','ANIMATION','ETC')",
                    false,
                    None,
                ),
                col("created_at", 4, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
                col("updated_at", 5, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
            ],
            indexes: vec![
                IndexRow::column("PRIMARY", 0, 1, "user_favorite_genre_id"),
                IndexRow::column("uk_user_genre", 0, 1, "user_id"),
                IndexRow::column("uk_user_genre", 0, 2, "genre"),
            ],
        },
        ModelFixture {
            name: "user_point_history",
            pk_col: "user_point_history_id",
            columns: vec![
                col("user_point_history_id", 1, "varchar", "varchar(36)", false, Some("generate_uuid")),
                col("user_id", 2, "varchar", "varchar(36)", false, None),
                col("video_id", 3, "varchar", "varchar(36)", true, None),
                col("amount", 4, "int", "int", false, None),
                col("watch_time", 5, "int", "int", true, Some("0")),
                col("description", 6, "varchar", "varchar(255)", true, None),
                col("created_at", 7, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
                col("updated_at", 8, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
            ],
            indexes: vec![IndexRow::column("PRIMARY", 0, 1, "user_point_history_id")],
        },
        ModelFixture {
            name: "video",
            pk_col: "video_id",
            columns: vec![
                col("video_id", 1, "varchar", "varchar(36)", false, Some("generate_uuid")),
                col("youtube_url", 2, "varchar", "varchar(50)", false, None),
                col("title", 3, "varchar", "varchar(255)", false, None),
                col("channel_name", 4, "varchar", "varchar(100)", true, None),
                col(
                    "category",
                    5,
                    "enum",
                    "enum('MUSIC','ENTERTAINMENT','GAMING','NEWS','EDUCATION','LIFESTYLE','SPORTS','COMEDY','ANIMATION','ETC')",
                    false,
                    None,
                ),
                col("duration", 6, "int", "int", true, Some("0")),
                col("view_count", 7, "bigint", "bigint", true, Some("0")),
                col("is_deleted", 8, "tinyint", "tinyint(1)", true, Some("0")),
                col("created_at", 9, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
                col("updated_at", 10, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
            ],
            indexes: vec![IndexRow::column("PRIMARY", 0, 1, "video_id"), IndexRow::column("uk_video_youtube_url", 0, 1, "youtube_url")],
        },
        ModelFixture {
            name: "video_bookmark",
            pk_col: "video_bookmark_id",
            columns: vec![
                col("video_bookmark_id", 1, "varchar", "varchar(36)", false, Some("generate_uuid")),
                col("video_id", 2, "varchar", "varchar(36)", false, None),
                col("user_id", 3, "varchar", "varchar(36)", false, None),
                col("created_at", 4, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
            ],
            indexes: vec![
                IndexRow::column("PRIMARY", 0, 1, "video_bookmark_id"),
                IndexRow::column("uk_bookmark_user_video", 0, 1, "user_id"),
                IndexRow::column("uk_bookmark_user_video", 0, 2, "video_id"),
            ],
        },
        ModelFixture {
            name: "video_like",
            pk_col: "video_like_id",
            columns: vec![
                col("video_like_id", 1, "varchar", "varchar(36)", false, Some("generate_uuid")),
                col("video_id", 2, "varchar", "varchar(36)", false, None),
                col("user_id", 3, "varchar", "varchar(36)", false, None),
                col("created_at", 4, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
            ],
            indexes: vec![
                IndexRow::column("PRIMARY", 0, 1, "video_like_id"),
                IndexRow::column("uk_like_user_video", 0, 1, "user_id"),
                IndexRow::column("uk_like_user_video", 0, 2, "video_id"),
            ],
        },
        ModelFixture {
            name: "video_request",
            pk_col: "video_request_id",
            columns: vec![
                col("video_request_id", 1, "varchar", "varchar(36)", false, Some("generate_uuid")),
                col("user_id", 2, "varchar", "varchar(36)", false, None),
                col("youtube_url", 3, "varchar", "varchar(50)", false, None),
                col("youtube_full_url", 4, "varchar", "varchar(255)", false, None),
                col("status", 5, "enum", "enum('PENDING','ACCEPTED','REJECTED')", false, Some("PENDING")),
                col("admin_comment", 6, "varchar", "varchar(255)", true, None),
                col("created_at", 7, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
                col("updated_at", 8, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
            ],
            indexes: vec![IndexRow::column("PRIMARY", 0, 1, "video_request_id")],
        },
        ModelFixture {
            name: "video_view_log",
            pk_col: "video_view_log_id",
            columns: vec![
                col("video_view_log_id", 1, "varchar", "varchar(36)", false, Some("generate_uuid")),
                col("video_id", 2, "varchar", "varchar(36)", false, None),
                col("user_id", 3, "varchar", "varchar(36)", true, None),
                col("guest_token", 4, "varchar", "varchar(255)", true, None),
                col("created_at", 5, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
                col("updated_at", 6, "timestamp", "timestamp", false, Some("CURRENT_TIMESTAMP")),
            ],
            indexes: vec![
                IndexRow::column("PRIMARY", 0, 1, "video_view_log_id"),
                IndexRow::column("idx_user_history", 1, 1, "user_id"),
                IndexRow::column("idx_user_history", 1, 2, "created_at"),
                IndexRow::column("idx_video_history", 1, 1, "video_id"),
                IndexRow::column("idx_video_history", 1, 2, "created_at"),
            ],
        },
    ];

    assert_eq!(fixtures.len(), 11);
    let total_columns: usize = fixtures.iter().map(|f| f.columns.len()).sum();
    assert_eq!(total_columns, 81, "11 FaceReview models must define all 81 declared columns");

    let mut all_normalized_fks_count = 0;

    for fixture in &fixtures {
        let normalized = normalize_table(fixture.name, fixture.columns.clone(), fixture.indexes.clone())
            .unwrap_or_else(|e| panic!("table {} should normalize: {e}", fixture.name));

        assert_eq!(normalized.name, fixture.name);
        assert_eq!(normalized.columns.len(), fixture.columns.len());

        let pk_index = normalized.indexes.iter().find(|i| i.primary).expect("must have PRIMARY index");
        assert_eq!(pk_index.columns, [fixture.pk_col], "PK column for {} must match exact declaration", fixture.name);
        assert!(pk_index.unique);

        // Verify specific unique constraints and composite indexes
        match fixture.name {
            "comment" => {
                let comp = normalized.indexes.iter().find(|i| i.name == "idx_video_comments").unwrap();
                assert_eq!(comp.columns, ["video_id", "is_deleted", "created_at"]);
                assert!(!comp.unique);
            }
            "user" => {
                let uk = normalized.indexes.iter().find(|i| i.name == "uk_user_email").unwrap();
                assert_eq!(uk.columns, ["email"]);
                assert!(uk.unique);
            }
            "user_emotion_summary" => {
                assert_eq!(fixture.pk_col, "user_id", "UserEmotionSummary PK is user_id, not an artificial id");
            }
            "user_favorite_genre" => {
                let uk = normalized.indexes.iter().find(|i| i.name == "uk_user_genre").unwrap();
                assert_eq!(uk.columns, ["user_id", "genre"]);
                assert!(uk.unique);
            }
            "video" => {
                let uk = normalized.indexes.iter().find(|i| i.name == "uk_video_youtube_url").unwrap();
                assert_eq!(uk.columns, ["youtube_url"]);
                assert!(uk.unique);
            }
            "video_bookmark" => {
                let uk = normalized.indexes.iter().find(|i| i.name == "uk_bookmark_user_video").unwrap();
                assert_eq!(uk.columns, ["user_id", "video_id"]);
                assert!(uk.unique);
            }
            "video_like" => {
                let uk = normalized.indexes.iter().find(|i| i.name == "uk_like_user_video").unwrap();
                assert_eq!(uk.columns, ["user_id", "video_id"]);
                assert!(uk.unique);
            }
            "video_view_log" => {
                let idx_u = normalized.indexes.iter().find(|i| i.name == "idx_user_history").unwrap();
                assert_eq!(idx_u.columns, ["user_id", "created_at"]);
                let idx_v = normalized.indexes.iter().find(|i| i.name == "idx_video_history").unwrap();
                assert_eq!(idx_v.columns, ["video_id", "created_at"]);
            }
            _ => {}
        }

        // Match against manifest foreign keys
        let manifest_model = models.iter().find(|m| m["table"].as_str() == Some(fixture.name)).expect("model in manifest");
        let fk_declarations = manifest_model["foreign_keys"].as_array().unwrap();

        let source_col_names: Vec<String> = fixture.columns.iter().map(|c| c.name.clone()).collect();
        let mut fk_rows = Vec::new();
        for fk in fk_declarations {
            let col_name = fk["column"].as_str().unwrap();
            let ref_table = fk["referenced_table"].as_str().unwrap();
            let ref_col = fk["referenced_column"].as_str().unwrap();
            let on_del = fk["on_delete"].as_str().unwrap();
            let on_upd = fk["on_update"].as_str().unwrap();
            fk_rows.push(ForeignKeyRow {
                constraint_schema: "facereview_baseline".into(),
                table_name: fixture.name.into(),
                constraint_name: format!("fk_{}_{}", fixture.name, col_name),
                ordinal_position: 1,
                column_name: col_name.into(),
                referenced_table_schema: Some("facereview_baseline".into()),
                referenced_table_name: Some(ref_table.into()),
                referenced_column_name: Some(ref_col.into()),
                delete_rule: Some(on_del.into()),
                update_rule: Some(on_upd.into()),
            });
        }

        let normalized_fks =
            normalize_foreign_keys("facereview_baseline", fixture.name, &source_col_names, fk_rows).expect("synthetic FKs should normalize");
        assert_eq!(normalized_fks.len(), fk_declarations.len());
        all_normalized_fks_count += normalized_fks.len();

        for expected in fk_declarations {
            let exp_col = expected["column"].as_str().unwrap();
            let actual = normalized_fks.iter().find(|k| k.columns == [exp_col]).expect("FK for column must exist in normalized results");
            assert_eq!(actual.referenced_table, expected["referenced_table"].as_str().unwrap());
            assert_eq!(actual.referenced_columns, [expected["referenced_column"].as_str().unwrap()]);
            assert_eq!(actual.on_delete.to_string(), expected["on_delete"].as_str().unwrap());
            assert_eq!(actual.on_update.to_string(), expected["on_update"].as_str().unwrap());
        }
    }

    assert_eq!(
        all_normalized_fks_count, 13,
        "Total normalized foreign keys across all 11 models must be exactly 13 per independent source audit of rev 7948b7e7 (correcting earlier 10 FK count)"
    );

    // Verify that schema-wide 4-query normalization produces byte-for-byte identical manifests
    let all_tables = fixtures.iter().map(|f| f.name.to_string()).collect::<Vec<_>>();
    let mut all_columns = Vec::new();
    let mut all_indexes = Vec::new();
    let mut all_fks = Vec::new();
    let mut sequential_catalog = std::collections::BTreeMap::new();

    for f in &fixtures {
        for col in &f.columns {
            all_columns.push(SchemaColumnRow { table_name: f.name.to_string(), column: col.clone() });
        }
        for idx in &f.indexes {
            all_indexes.push(SchemaIndexRow { table_name: f.name.to_string(), index: idx.clone() });
        }
        let manifest_model = models.iter().find(|m| m["table"].as_str() == Some(f.name)).unwrap();
        let fk_declarations = manifest_model["foreign_keys"].as_array().unwrap();
        let source_col_names: Vec<String> = f.columns.iter().map(|c| c.name.clone()).collect();
        let mut table_fk_rows = Vec::new();
        for fk in fk_declarations {
            let col_name = fk["column"].as_str().unwrap();
            let row = ForeignKeyRow {
                constraint_schema: "facereview_baseline".into(),
                table_name: f.name.into(),
                constraint_name: format!("fk_{}_{}", f.name, col_name),
                ordinal_position: 1,
                column_name: col_name.into(),
                referenced_table_schema: Some("facereview_baseline".into()),
                referenced_table_name: Some(fk["referenced_table"].as_str().unwrap().into()),
                referenced_column_name: Some(fk["referenced_column"].as_str().unwrap().into()),
                delete_rule: Some(fk["on_delete"].as_str().unwrap().into()),
                update_rule: Some(fk["on_update"].as_str().unwrap().into()),
            };
            all_fks.push(row.clone());
            table_fk_rows.push(row);
        }
        let manifest = normalize_table(f.name, f.columns.clone(), f.indexes.clone()).unwrap();
        let fks = normalize_foreign_keys("facereview_baseline", f.name, &source_col_names, table_fk_rows).unwrap();
        sequential_catalog.insert(f.name.to_string(), (manifest, fks));
    }

    let schema_wide_catalog = introspect_schema_from_schema_rows("facereview_baseline", all_tables, all_columns, all_indexes, all_fks)
        .expect("schema-wide normalization of 11 FaceReview models must succeed");

    assert_eq!(sequential_catalog, schema_wide_catalog, "schema-wide catalog must match sequential catalog for all 11 models");
}

use aip_migrate::mysql_catalog::{
    CatalogError, MysqlCatalogReader, MysqlDriverConfig, SQL_COLUMNS, SQL_COLUMNS_BY_SCHEMA, SQL_FOREIGN_KEYS, SQL_FOREIGN_KEYS_BY_SCHEMA,
    SQL_STATISTICS, SQL_STATISTICS_BY_SCHEMA, SQL_STATISTICS_MARIADB, SQL_STATISTICS_MARIADB_BY_SCHEMA, SQL_TABLES, SchemaColumnRow, SchemaIndexRow,
    introspect_schema, introspect_schema_from_schema_rows, introspect_table, normalize_mysql_error_code,
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
    assert!(SQL_COLUMNS.contains("COALESCE(GENERATION_EXPRESSION, '')"));
    assert!(SQL_STATISTICS.contains("INFORMATION_SCHEMA.STATISTICS"));
    assert!(SQL_STATISTICS_MARIADB.contains("NULL AS EXPRESSION"));
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

#[tokio::test]
async fn schema_wide_introspect_produces_identical_catalog_to_sequential_introspect() {
    assert!(SQL_COLUMNS_BY_SCHEMA.contains("WHERE c.TABLE_SCHEMA = ?"));
    assert!(SQL_COLUMNS_BY_SCHEMA.contains("ORDER BY c.TABLE_NAME ASC, c.ORDINAL_POSITION ASC"));
    // INFORMATION_SCHEMA.COLUMNS also lists view columns; the schema-wide read must stay on base tables.
    assert!(SQL_COLUMNS_BY_SCHEMA.contains("t.TABLE_TYPE = 'BASE TABLE'"));
    assert!(SQL_STATISTICS_BY_SCHEMA.contains("ORDER BY TABLE_NAME ASC, INDEX_NAME ASC, SEQ_IN_INDEX ASC"));
    assert!(SQL_STATISTICS_MARIADB_BY_SCHEMA.contains("NULL AS EXPRESSION"));
    assert!(SQL_FOREIGN_KEYS_BY_SCHEMA.contains("WHERE k.CONSTRAINT_SCHEMA = ? AND k.REFERENCED_TABLE_NAME IS NOT NULL"));

    let reader = MockCatalogReader;
    let sequential = introspect_schema(&reader, "app_db").await.unwrap();

    let tables = reader.fetch_tables("app_db").await.unwrap();
    let mut columns = Vec::new();
    let mut indexes = Vec::new();
    let mut foreign_keys = Vec::new();

    for table in &tables {
        for col in reader.fetch_columns("app_db", table).await.unwrap() {
            columns.push(SchemaColumnRow { table_name: table.clone(), column: col });
        }
        for idx in reader.fetch_indexes("app_db", table).await.unwrap() {
            indexes.push(SchemaIndexRow { table_name: table.clone(), index: idx });
        }
        for fk in reader.fetch_foreign_keys("app_db", table).await.unwrap() {
            foreign_keys.push(fk);
        }
    }

    let schema_wide = introspect_schema_from_schema_rows("app_db", tables, columns, indexes, foreign_keys).unwrap();

    let seq_json = serde_json::to_string(&sequential).unwrap();
    let wide_json = serde_json::to_string(&schema_wide).unwrap();
    assert_eq!(seq_json, wide_json, "schema-wide introspect output must match sequential introspect byte-for-byte");
    assert_eq!(
        spike_v1_fixture::digest(&serde_json::to_value(&sequential).unwrap()),
        spike_v1_fixture::digest(&serde_json::to_value(&schema_wide).unwrap())
    );
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
    assert!(cfg.tls_required, "sslmode=required must set tls_required to true");

    let sanitized = cfg.display_sanitized();
    assert!(!sanitized.contains("super_secret_pass"), "secret password must never appear in display_sanitized");
    assert!(sanitized.contains("db_admin:***@10.0.0.5:3306/facereview_prod"));

    // Debug output must NOT leak raw password
    let debug_repr = format!("{cfg:?}");
    assert!(!debug_repr.contains("super_secret_pass"), "Debug output must not leak raw password");
    assert!(debug_repr.contains("***"), "Debug output must mask password");

    // Percent-encoded password decode (%40 -> @)
    let encoded_url = "mysql://verifier:p%40ss@db.example:3306/facereview";
    let enc_cfg = MysqlDriverConfig::parse_url(encoded_url).expect("percent-encoded url should parse");
    assert_eq!(enc_cfg.password.as_deref(), Some("p@ss"), "percent-encoded password must be decoded");

    // IPv6 host address support
    let ipv6_url = "mysql://verifier:pw@[2001:db8::1]:3306/facereview";
    let ip6_cfg = MysqlDriverConfig::parse_url(ipv6_url).expect("IPv6 host url should parse");
    assert_eq!(ip6_cfg.host, "[2001:db8::1]");
    assert_eq!(ip6_cfg.port, 3306);
    assert_eq!(ip6_cfg.database, "facereview");

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

    // Error 1105 with embedded raw URL containing secret credentials must be sanitized
    let err_generic = normalize_mysql_error_code(1105, "unknown server error: mysql://verifier:synthetic_secret@db.example:3306/facereview");
    assert_eq!(err_generic.code, "DB_ERROR");
    assert!(!err_generic.message.contains("synthetic_secret"), "embedded password in error message must be sanitized");
    assert!(err_generic.message.contains("verifier:***@db.example:3306/facereview"));

    let err_multiple = normalize_mysql_error_code(1105, "first mysql://verifier:first_secret@a/db then mysql://verifier:second_secret@b/db");
    assert!(!err_multiple.message.contains("first_secret"));
    assert!(!err_multiple.message.contains("second_secret"));
    assert!(err_multiple.message.contains("mysql://verifier:***@a/db"));
    assert!(err_multiple.message.contains("mysql://verifier:***@b/db"));
}

#[tokio::test]
async fn external_introspect_rejects_mysql_url_without_leaking_secrets() {
    let raw_url = "mysql://admin:super_secret_db_password@127.0.0.1:3306/facereview";
    let err = aip_migrate::external::introspect(raw_url, "facereview").await.unwrap_err();
    #[cfg(not(feature = "mysql"))]
    assert_eq!(err["code"], "DB_ADAPTER_UNSUPPORTED");
    #[cfg(feature = "mysql")]
    assert_eq!(err["code"], "DB_INTROSPECT");
    assert!(!err.to_string().contains("super_secret_db_password"), "secret must never leak in introspect error");
}

#[test]
fn percent_decode_handles_malformed_and_edge_case_inputs() {
    use aip_migrate::mysql_catalog::percent_decode;

    // Standard valid decoding
    assert_eq!(percent_decode("p%40ss"), "p@ss");
    assert_eq!(percent_decode("hello%20world"), "hello world");

    // Invalid hex sequence preserved without panic
    assert_eq!(percent_decode("foo%ZZbar"), "foo%ZZbar");
    assert_eq!(percent_decode("val%4Gtest"), "val%4Gtest");

    // Incomplete percent at string boundaries
    assert_eq!(percent_decode("trailing%"), "trailing%");
    assert_eq!(percent_decode("trailing%4"), "trailing%4");

    // Empty string
    assert_eq!(percent_decode(""), "");

    // Pure unencoded string
    assert_eq!(percent_decode("normal_string_123"), "normal_string_123");
}

#[test]
fn mysql_driver_config_parses_various_url_configurations_safely() {
    // 1. IPv6 without explicit port defaults to 3306
    let ip6_url = "mysql://u:p@[2001:db8::1]/app_db";
    let cfg1 = MysqlDriverConfig::parse_url(ip6_url).unwrap();
    assert_eq!(cfg1.host, "[2001:db8::1]");
    assert_eq!(cfg1.port, 3306);
    assert_eq!(cfg1.database, "app_db");

    // 2. User without password
    let no_pass_url = "mysql://developer@db.internal:3306/analytics";
    let cfg2 = MysqlDriverConfig::parse_url(no_pass_url).unwrap();
    assert_eq!(cfg2.user, "developer");
    assert_eq!(cfg2.password, None);
    assert_eq!(cfg2.display_sanitized(), "mysql://developer@db.internal:3306/analytics");

    // 3. SSL modes
    let ssl_req = "mysql://app:pw@10.0.0.1:3306/db?sslmode=required";
    assert!(MysqlDriverConfig::parse_url(ssl_req).unwrap().tls_required);

    let ssl_dis = "mysql://app:pw@127.0.0.1:3306/db?sslmode=disabled";
    assert!(!MysqlDriverConfig::parse_url(ssl_dis).unwrap().tls_required);

    let ssl_full = "mysql://app:pw@10.0.0.1:3306/db?ssl-mode=verify-full";
    assert!(MysqlDriverConfig::parse_url(ssl_full).unwrap().tls_required);
}

#[test]
fn mysql_driver_config_requires_verified_tls_for_remote_hosts_and_rejects_bad_modes() {
    assert!(MysqlDriverConfig::parse_url("mysql://app:pw@db.example/app").unwrap().tls_required);
    assert!(!MysqlDriverConfig::parse_url("mysql://app:pw@127.0.0.1/app").unwrap().tls_required);
    assert!(MysqlDriverConfig::parse_url("mysql://app:pw@db.example/app?sslmode=disabled").is_err());
    assert!(MysqlDriverConfig::parse_url("mysql://app:pw@db.example/app?sslmode=typo").is_err());
    assert!(MysqlDriverConfig::parse_url("mysql://app:pw@db.example/app?sslmode").is_err());
    assert!(MysqlDriverConfig::parse_url("mysql://app:pw@db.example/app?sslmode=&ssl-mode=require").is_err());
    assert!(MysqlDriverConfig::parse_url("mysql://app:pw@localhost/app?sslmode=prefer").is_err());
}

#[test]
#[cfg(any())]
fn mysql_wire_protocol_packet_framing_and_response_integration() {
    use aip_migrate::mysql_wire::{CommandPacket, PacketHeader, RawPacket, ServerResponse};

    // Verify command query serialization
    let cmd = CommandPacket::query(SQL_TABLES);
    let serialized = cmd.serialize();
    let header = PacketHeader::decode(&serialized[..4]).unwrap();
    assert_eq!(header.payload_length as usize, cmd.payload.len());
    assert_eq!(header.sequence_id, 0);

    // Verify OK response parsing
    let ok_packet = RawPacket::new(1, vec![0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00]);
    let resp = ServerResponse::parse(&ok_packet).unwrap();
    match resp {
        ServerResponse::Ok { affected_rows, .. } => assert_eq!(affected_rows, 0),
        _ => panic!("expected OK response"),
    }
}

/// The catalog fingerprint is the SHA-256 of this manifest as JSON. A column type serde cannot serialize (enum members,
/// DATETIME, DECIMAL, ...) used to make every fingerprint, adoption and fenced request fail for any schema that has one.
#[test]
fn every_logical_type_serializes_so_the_catalog_fingerprint_is_computable() {
    let mut id = column("id", "bigint", "bigint");
    id.ordinal_position = 1;
    let mut kind = column("kind", "enum", "enum('MEMO','TODO')");
    kind.ordinal_position = 2;
    let mut created = column("created", "datetime", "datetime(6)");
    created.ordinal_position = 3;
    let mut score = column("score", "decimal", "decimal(10,2)");
    score.ordinal_position = 4;
    let table = normalize_table("note", vec![id, kind, created, score], vec![IndexRow::column("PRIMARY", 0, 1, "id")]).expect("normalizes");
    let value = serde_json::to_value(&table).expect("manifest with enum and unsupported types must serialize");
    let again: aip_migrate::mysql_catalog::TableManifest = serde_json::from_value(value.clone()).expect("and round-trip");
    assert_eq!(again, table);
    // The precision and scale are part of the fingerprint input, so changing them is observable drift.
    let mut widened = column("created", "datetime", "datetime(3)");
    widened.ordinal_position = 3;
    let mut id = column("id", "bigint", "bigint");
    id.ordinal_position = 1;
    let mut kind = column("kind", "enum", "enum('MEMO','TODO')");
    kind.ordinal_position = 2;
    let mut score = column("score", "decimal", "decimal(10,2)");
    score.ordinal_position = 4;
    let other = normalize_table("note", vec![id, kind, widened, score], vec![IndexRow::column("PRIMARY", 0, 1, "id")]).unwrap();
    assert_ne!(serde_json::to_value(&other).unwrap(), value, "datetime(6) vs datetime(3) must differ");
}
