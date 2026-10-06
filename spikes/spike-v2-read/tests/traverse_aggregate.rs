use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    connect_with_url, execute,
    id_wire::{encode_rows, IdWire},
    plan::{plan_read, plan_read_with_wire, Caller, Reject},
    sqlgen, DB_URL,
};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_postgres::Client;

const FIXTURE: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const NOW: &str = "2026-10-04T00:00:00Z";

fn facts_with_club_member_count(hide_club_rows: bool) -> Value {
    let access = "access clubManagerOnly(a: Member, c: Club.Id) = managerOf(a, c)";
    let expose = "  expose read { select id, name, logo }";
    let aggregate = "  aggregate memberCount: Int {\n    source ClubMember\n    sourceAccess memberCanReadCounts(actor)\n    groupKey club\n    callerFilter none\n    rowOutput none\n    release count\n  }\n  expose read { select id, name, logo, memberCount }";
    assert_eq!(FIXTURE.matches(access).count(), 1, "canonical access declaration must be unique");
    assert_eq!(FIXTURE.matches(expose).count(), 1, "canonical Club expose block must be unique");
    let mut source = FIXTURE.to_string();
    if hide_club_rows {
        let visible = "rows read when school = null or school = actor.school";
        assert_eq!(source.matches(visible).count(), 1, "canonical Club row policy must be unique");
        source = source.replacen(visible, "rows read when false", 1);
    }
    source = source
        .replace(access, &format!("{access}\naccess memberCanReadCounts(a: Member) = exists ClubMember where member = a"))
        .replace("  rows read when school = null or school = actor.school\n  expose read { select id, name, logo }", "  rows read when school = null or school = actor.school\n  field name read when managerOf(actor, this)\n  expose read { select id, name, logo }")
        .replacen(expose, aggregate, 1)
        .replace("traverse club { select id, name, logo }", "traverse club { select id, name, logo, memberCount }");
    load_str(&source, Form::A).unwrap_or_else(|error| panic!("modified canonical V1 fixture rejected: {error:?}")).execution
}

fn caller(actor_id: Option<i64>) -> Caller {
    Caller { actor_id, now: NOW.into() }
}

fn request(limit: i64) -> Value {
    json!({
        "read": "Recruitment",
        "select": ["id", "bookmarkCount", "internalNote", { "club": { "select": ["id", "name", "memberCount"] } }],
        "sort": [{ "field": "id" }],
        "limit": limit
    })
}

fn owned_schema_name() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).expect("system time after Unix epoch").as_nanos();
    format!("aip_v2_trav_agg_{}_{}", std::process::id(), nanos)
}

fn check_code(facts: &Value, req: &Value, who: &Caller, expected: &str) -> Result<(), String> {
    match plan_read(facts, req, who) {
        Err(Reject { code, .. }) if code == expected => Ok(()),
        Err(error) => Err(format!("expected {expected}, got {}: {}", error.code, error.msg)),
        Ok(_) => Err(format!("expected planner rejection {expected}, got a plan")),
    }
}

async fn seed(db: &mut Client) -> Result<(), String> {
    let schema = sqlgen::schema();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.school (id, name) VALUES (1, 'A대'), (2, 'B대');\
         INSERT INTO {schema}.member (id, school_id) VALUES (1, 1), (2, 1), (3, 2), (4, 1);\
         INSERT INTO {schema}.club (id, name, logo, school_id) VALUES (10, 'A동아리', 'a.png', 1), (11, 'B동아리', 'b.png', 2), (12, '연합', NULL, NULL);\
         INSERT INTO {schema}.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER'), (10, 2, 'MEMBER'), (11, 3, 'ADMIN');\
         INSERT INTO {schema}.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES\
          (100, 'A 모집', '2026-10-10T00:00:00Z', 'PUBLISHED', 5, 10, 'A-note'),\
          (102, '연합 모집', '2026-10-08T00:00:00Z', 'PUBLISHED', 9, 12, NULL);\
         INSERT INTO {schema}.recruitment_bookmark (recruitment_id, member_id) VALUES (100, 1), (100, 2), (100, 3), (102, 4);"
    ))
    .await
    .map_err(|error| format!("seed owned schema: {error}"))
}

async fn exercise_database(facts: &Value, db: &mut Client) -> Result<(), String> {
    let req = request(1);
    let who = caller(Some(1));
    let plan = plan_read_with_wire(facts, &req, &who, IdWire::SafeNumber).map_err(|error| format!("safe plan: {} {}", error.code, error.msg))?;
    if !plan.sql.contains("count(*)") || !plan.sql.contains("CASE WHEN") {
        return Err(format!("aggregate sourceAccess guard missing from SQL: {}", plan.sql));
    }
    if plan.cost != 6 {
        return Err(format!("expected cost 6 for root/traverse aggregates + field policies, got {}", plan.cost));
    }
    if plan.deps
        != vec!["Club".to_string(), "ClubMember".to_string(), "Member".to_string(), "Recruitment".to_string(), "RecruitmentBookmark".to_string()]
    {
        return Err(format!("aggregate dependency set lost a resource: {:?}", plan.deps));
    }
    if plan.output_type["rows"]["club"]["object"]["memberCount"]["nullable"] != true
        || plan.output_type["rows"]["bookmarkCount"]["nullable"] != false
        || plan.output_type["rows"]["internalNote"]["redactable"] != true
        || plan.output_type["rows"]["club"]["object"]["name"]["redactable"] != true
    {
        return Err(format!("output metadata lost aggregate/field policies: {}", plan.output_type));
    }

    let safe_raw = execute(db, &plan).await.map_err(|error| format!("safe execution: {} {}", error.code, error.msg))?;
    let safe = encode_rows(safe_raw, &plan.output_type, IdWire::SafeNumber).map_err(|error| format!("safe wire: {} {}", error.code, error.msg))?;
    if safe.len() != 1
        || safe[0]["id"] != json!(100)
        || safe[0]["bookmarkCount"] != json!(3)
        || safe[0]["internalNote"] != json!("A-note")
        || safe[0]["club"] != json!({"id":10,"name":"A동아리","memberCount":2})
    {
        return Err(format!("safe aggregate row mismatch: {safe:?}"));
    }

    let decimal_plan =
        plan_read_with_wire(facts, &req, &who, IdWire::DecimalString).map_err(|error| format!("decimal plan: {} {}", error.code, error.msg))?;
    let decimal_raw = execute(db, &decimal_plan).await.map_err(|error| format!("decimal execution: {} {}", error.code, error.msg))?;
    let decimal = encode_rows(decimal_raw, &decimal_plan.output_type, IdWire::DecimalString)
        .map_err(|error| format!("decimal wire: {} {}", error.code, error.msg))?;
    if decimal.len() != 1
        || decimal[0]["id"] != json!("100")
        || decimal[0]["bookmarkCount"] != json!(3)
        || decimal[0]["club"] != json!({"id":"10","name":"A동아리","memberCount":2})
    {
        return Err(format!("decimal aggregate row mismatch: {decimal:?}"));
    }

    let denied = plan_read_with_wire(facts, &req, &caller(Some(4)), IdWire::SafeNumber)
        .map_err(|error| format!("unprivileged plan: {} {}", error.code, error.msg))?;
    let denied_rows = execute(db, &denied).await.map_err(|error| format!("unprivileged execution: {} {}", error.code, error.msg))?;
    let denied_rows =
        encode_rows(denied_rows, &denied.output_type, IdWire::SafeNumber).map_err(|error| format!("anonymous wire: {} {}", error.code, error.msg))?;
    if denied_rows.len() != 1
        || !denied_rows[0]["club"]["memberCount"].is_null()
        || !denied_rows[0]["club"]["name"].is_null()
        || !denied_rows[0]["internalNote"].is_null()
    {
        return Err(format!("field and aggregate guards must redact denied values to null: {denied_rows:?}"));
    }

    let hidden_target = facts_with_club_member_count(true);
    let hidden = plan_read(&hidden_target, &req, &who).map_err(|error| format!("hidden target plan: {} {}", error.code, error.msg))?;
    let hidden_rows = execute(db, &hidden).await.map_err(|error| format!("hidden target execution: {} {}", error.code, error.msg))?;
    if hidden_rows.len() != 1 || !hidden_rows[0]["club"].is_null() {
        return Err(format!("target row policy must yield a null relation: {hidden_rows:?}"));
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn relation_target_aggregate_preserves_planning_policy_cost_dependencies_and_wire() {
    let schema = owned_schema_name();
    sqlgen::set_schema(&schema);
    let facts = facts_with_club_member_count(false);
    let req = request(1);
    let who = caller(Some(1));
    let plan = plan_read(&facts, &req, &who).expect("relation aggregate must produce a read plan");
    assert!(plan.sql.contains("count(*)"));

    let mut private_field = req.clone();
    private_field["select"][3]["club"]["select"] = json!(["school"]);
    check_code(&facts, &private_field, &who, "FIELD_NOT_EXPOSED").unwrap();
    check_code(&facts, &json!({ "read": "Club", "select": ["id", "memberCount"] }), &who, "NOT_ROOT_QUERYABLE").unwrap();
    let mut low_cost = facts.clone();
    low_cost["resources"]["Recruitment"]["exposeRead"]["budget"]["cost"] = json!(17);
    check_code(&low_cost, &request(3), &who, "COST_EXCEEDED").unwrap();

    let mut db = connect_with_url(DB_URL).await.expect("connect to local PostgreSQL for owned-schema integration test");
    let ddl = sqlgen::create_ddl(&facts).expect("generate CREATE-only DDL");
    let mut owns_schema = false;
    let mut setup_error = None;
    for (index, statement) in ddl.iter().enumerate() {
        match db.batch_execute(statement).await {
            Ok(()) if index == 0 => owns_schema = true,
            Ok(()) => {}
            Err(error) => {
                setup_error = Some(format!("owned-schema DDL: {error}"));
                break;
            }
        }
    }
    if setup_error.is_none() {
        if let Err(error) = seed(&mut db).await {
            setup_error = Some(error);
        }
    }
    let result = match setup_error {
        Some(error) => Err(error),
        None => exercise_database(&facts, &mut db).await,
    };
    let cleanup_error = if owns_schema {
        db.batch_execute(&format!("DROP SCHEMA {} CASCADE", sqlgen::schema())).await.err().map(|error| format!("drop owned test schema: {error}"))
    } else {
        None
    };
    assert!(result.is_ok(), "{}", result.unwrap_err());
    assert!(cleanup_error.is_none(), "{}", cleanup_error.unwrap_or_default());
}
