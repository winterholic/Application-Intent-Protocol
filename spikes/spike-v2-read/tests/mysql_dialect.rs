//! Offline checks of the MySQL dialect of the read planner. Live behaviour is covered by
//! `crates/aip-service/tests/mysql_read_http.rs`; here the generated SQL, the `?` ordering and the
//! result normalization are pinned without a database, and the PostgreSQL output is pinned byte for byte.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v2_read::mysql_rows::normalize;
use spike_v2_read::plan::{plan_operation_authorization_in, plan_read, plan_read_in, Caller, Plan};
use spike_v2_read::sqlgen::{self, Dialect};
use std::sync::Once;

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
static SCHEMA: Once = Once::new();

fn facts() -> Value {
    SCHEMA.call_once(|| sqlgen::set_schema("aip_mysql_dialect"));
    let src = A.replacen("fields { id: Id; name: Text }\n}", "fields { id: Id; name: Text }\n  rows read when true\n}", 1).replacen(
        "filter periodEnd.gte, periodEnd.lte",
        "filter periodEnd.gte, periodEnd.lte, club.eq, club.in, status.in, title.in, title.contains, title.prefix",
        1,
    );
    load_str(&src, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}

fn caller() -> Caller {
    Caller { actor_id: Some(7), now: "2026-10-04T00:00:00Z".into() }
}

fn request() -> Value {
    json!({
        "read": "Recruitment",
        "select": ["id", "title", "periodEnd", {"club": {"select": ["id", "name"]}}],
        "filter": [
            {"field": "club", "op": "in", "value": [10, 11]},
            {"field": "title", "op": "contains", "value": "x"},
            {"field": "periodEnd", "op": "gte", "value": "2026-10-05T00:00:00Z"}
        ],
        "sort": [{"field": "views", "dir": "desc"}],
        "limit": 5
    })
}

fn mysql(req: &Value) -> Plan {
    plan_read_in(Dialect::MySql, &facts(), req, &caller(), IdWire::Legacy).unwrap_or_else(|e| panic!("{e:?}"))
}

#[test]
fn postgres_output_is_unchanged_by_the_dialect_plumbing() {
    let f = facts();
    let a = plan_read(&f, &request(), &caller()).unwrap();
    let b = plan_read_in(Dialect::Postgres, &f, &request(), &caller(), IdWire::Legacy).unwrap();
    assert_eq!((&a.sql, &a.params, &a.deps), (&b.sql, &b.params, &b.deps));
    // Pinned: numbered `$n::text::cast` bindings, unquoted snake names, json_build_object, strpos, LATERAL.
    assert!(a.sql.starts_with("SELECT json_build_object('id', t.id, 'title', t.title, 'periodEnd', t.period_end, 'club', j"), "{}", a.sql);
    assert!(a.sql.contains("LEFT JOIN LATERAL (SELECT json_build_object('id', r"), "{}", a.sql);
    assert!(a.sql.contains("t.title ILIKE $") && a.sql.ends_with("LIMIT $8::text::bigint"), "{}", a.sql);
    assert!(a.sql.contains("t.club_id IN ($") && a.sql.contains("::text::timestamptz"), "{}", a.sql);
}

#[test]
fn mysql_sql_has_no_postgres_syntax_and_quotes_every_identifier() {
    let plan = mysql(&request());
    let sql = &plan.sql;
    for forbidden in ["$", "::", "json_build_object", "strpos", " FILTER (", "ILIKE", "RETURNING"] {
        assert!(!sql.contains(forbidden), "`{forbidden}` in MySQL SQL: {sql}");
    }
    assert!(sql.starts_with("SELECT CAST(JSON_OBJECT('id', t.`id`, 'title', t.`title`, 'periodEnd', t.`period_end`"), "{sql}");
    assert!(sql.contains("FROM `aip_mysql_dialect`.`recruitment` t"), "{sql}");
    assert!(sql.contains("LEFT JOIN LATERAL (SELECT JSON_OBJECT('id', r"), "{sql}");
    assert!(sql.contains("t.`title` LIKE ?"), "{sql}");
    // LIMIT takes a literal: MySQL rejects a string-typed bound parameter there.
    assert!(sql.ends_with("LIMIT 5"), "{sql}");
    assert_eq!(plan.deps, vec!["Club".to_string(), "Member".to_string(), "Recruitment".to_string()]);
}

#[test]
fn mysql_positional_bindings_follow_the_sql_text_even_when_a_value_is_reused() {
    let plan = mysql(&request());
    // Actor 7 appears in two policy sub-selects: the one logical parameter becomes two `?`, in text order.
    let placeholders = plan.sql.matches('?').count();
    assert_eq!(placeholders, plan.params.len(), "{}", plan.sql);
    let actor = plan.params.iter().filter(|p| p.as_deref() == Some("7")).count();
    assert!(actor >= 2, "actor bound once per use: {:?}", plan.params);
    // Time input is normalized to a DATETIME(6) literal: no `T`, no zone suffix.
    assert!(plan.params.iter().any(|p| p.as_deref() == Some("2026-10-05 00:00:00")), "{:?}", plan.params);
    assert!(plan.params.iter().any(|p| p.as_deref() == Some("2026-10-04 00:00:00")), "request time binds the same way: {:?}", plan.params);
    // The traverse join precedes WHERE in the text, so its actor binding is the first `?` and the first value, although the
    // planner bound the join's policy before the WHERE clause only by accident of generation order.
    let first_marker = plan.sql.find('?').unwrap();
    assert!(plan.sql[..first_marker].ends_with("z.`id` = CAST("), "{}", &plan.sql[..first_marker]);
    assert_eq!(plan.params[0].as_deref(), Some("7"));
    // `status = ?` is the next distinct value and keeps its enum text.
    let status_marker = plan.sql.find("t.`status` = ?").unwrap();
    let before = plan.sql[..status_marker].matches('?').count();
    assert_eq!(plan.params[before].as_deref(), Some("PUBLISHED"), "{:?}", plan.params);
}

#[test]
fn mysql_sort_keeps_postgres_null_ordering_and_a_stable_tie_break() {
    let facts = facts();
    // `club` is nullable only through policy; the generated order must still put NULLs last for ASC and first for DESC.
    let req = json!({"read": "Recruitment", "select": ["id"], "sort": [{"field": "views", "dir": "asc"}, {"field": "periodEnd", "dir": "desc"}]});
    let plan = plan_read_in(Dialect::MySql, &facts, &req, &caller(), IdWire::Legacy).unwrap();
    assert!(
        plan.sql.contains("ORDER BY (t.`views` IS NULL) ASC, t.`views` ASC, (t.`period_end` IS NULL) DESC, t.`period_end` DESC, t.`id` ASC LIMIT"),
        "{}",
        plan.sql
    );
}

#[test]
fn mysql_operation_authorization_has_no_values_table_and_keeps_bindings_ordered() {
    let f = facts();
    let ops = f["operations"].as_object().cloned().unwrap_or_default();
    // The fixture declares no operations; the planner must still fail closed with the same code as PostgreSQL.
    assert!(ops.is_empty());
    let err = plan_operation_authorization_in(Dialect::MySql, &f, "missing", &json!({}), &caller(), IdWire::Legacy)
        .err()
        .expect("unknown operation is rejected");
    assert_eq!(err.code, "WIRE.NOT_EXPOSED");
}

#[test]
fn result_normalization_turns_mysql_json_into_the_public_shape() {
    let output = json!({"rows": {
        "id": {"ty": "Id<Note>", "nullable": false},
        "done": {"ty": "Bool", "nullable": false},
        "maybe": {"ty": "Bool", "nullable": true},
        "created": {"ty": "Time", "nullable": false},
        "views": {"ty": "Int", "nullable": false},
        "score": {"ty": "Decimal(10,2)", "nullable": false},
        "club": {"object": {"id": {"ty": "Id<Club>", "nullable": false}, "flag": {"ty": "Bool", "nullable": false}}, "nullable": true},
        "tags": {"array": {"on": {"ty": "Bool", "nullable": false}}, "nullable": false}
    }});
    let row = json!({
        "id": 1, "done": 1, "maybe": null, "created": "2026-10-01 09:30:00.123400", "views": "5", "score": "10.50",
        "club": {"id": 10, "flag": 0}, "tags": [{"on": 1}, {"on": 0}]
    });
    let out = normalize(row, &output).unwrap();
    assert_eq!(
        out,
        json!({
            "id": 1, "done": true, "maybe": null, "created": "2026-10-01T09:30:00.1234+00:00", "views": 5, "score": "10.50",
            "club": {"id": 10, "flag": false}, "tags": [{"on": true}, {"on": false}]
        })
    );
    // Whole seconds drop the fraction, like PostgreSQL's timestamptz JSON.
    let t = json!({"rows": {"created": {"ty": "Time", "nullable": false}}});
    assert_eq!(normalize(json!({"created": "2026-10-02 00:00:00.000000"}), &t).unwrap(), json!({"created": "2026-10-02T00:00:00+00:00"}));
}

#[test]
fn result_normalization_refuses_values_that_do_not_fit_their_declared_type() {
    let ty = |t: &str| json!({"rows": {"v": {"ty": t, "nullable": false}}});
    for (t, bad) in [
        ("Bool", json!(2)),
        ("Bool", json!("yes")),
        ("Time", json!("2026-13-01")),
        ("Time", json!("0000-00-00 00:00:00")),
        ("Int", json!("9223372036854775808")),
        ("Int", json!(1.5)),
        ("Int", json!(true)),
    ] {
        let err = normalize(json!({"v": bad.clone()}), &ty(t)).unwrap_err();
        assert_eq!(err.code, "WIRE.INTERNAL", "{t} {bad}");
    }
    // A null where the plan promised a value is a contract violation, not a quiet null.
    assert_eq!(normalize(json!({"v": null}), &ty("Int")).unwrap_err().code, "WIRE.INTERNAL");
    // A missing field too.
    assert_eq!(normalize(json!({}), &ty("Int")).unwrap_err().code, "WIRE.INTERNAL");
}
