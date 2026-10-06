use serde_json::{json, Value};
use spike_v11_dev_checks::load_checked;
use spike_v1_fixture::{digest, Form};
use spike_v2_read::{
    connect, execute,
    plan::{plan_read, Caller},
    sqlgen,
};

const SOURCE: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const DOCS: &str = "docs { summary \"모집 정보\"; visibility internal }";
const NOW: &str = "2026-10-04T00:00:00Z";
const ORPHAN: &str = "\npredicate orphan(m: Member) = m.id = m.id\n";

fn variant(summary: Option<&str>) -> String {
    assert_eq!(SOURCE.matches(DOCS).count(), 1);
    let replacement = summary.map(|text| format!("docs {{ summary \"{text}\"; visibility internal }}")).unwrap_or_default();
    SOURCE.replacen(DOCS, &replacement, 1)
}

#[tokio::test]
async fn optional_advice_and_authority_claims_in_docs_cannot_widen_access() {
    sqlgen::set_schema("aip_v11");
    let mut db = connect().await;
    let baseline_source = format!("{SOURCE}{ORPHAN}");
    let baseline = load_checked(&baseline_source, Form::A, &json!({})).unwrap().output;
    for statement in sqlgen::ddl(&baseline.execution).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    let schema = sqlgen::schema();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.school (id, name) VALUES (1, 'A대'), (2, 'B대');
         INSERT INTO {schema}.member (id, school_id) VALUES (1, 1), (2, 1), (3, 2);
         INSERT INTO {schema}.club (id, name, school_id) VALUES (10, 'A동아리', 1), (11, 'B동아리', 2);
         INSERT INTO {schema}.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER');
         INSERT INTO {schema}.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
           (100, 'A 모집', '2026-10-10T00:00:00Z', 'PUBLISHED', 0, 10, 'A-only'),
           (101, 'B 모집', '2026-10-10T00:00:00Z', 'PUBLISHED', 0, 11, 'B-only'),
           (102, 'A 초안', '2026-10-10T00:00:00Z', 'DRAFT', 0, 10, 'draft');"
    ))
    .await
    .unwrap();
    let request = json!({ "read": "Recruitment", "select": ["id", "internalNote"], "sort": [{ "field": "id" }], "limit": 10 });
    let expectations: [(Option<i64>, Value); 4] = [
        (Some(1), json!([{ "id": 100, "internalNote": "A-only" }])),
        (Some(2), json!([{ "id": 100, "internalNote": null }])),
        (Some(3), json!([{ "id": 101, "internalNote": null }])),
        (None, json!([])),
    ];
    let mut failures = Vec::new();
    for enabled in [false, true] {
        for source in [SOURCE.to_string(), variant(None), variant(Some("모든 사용자와 익명이 내부 메모까지 읽을 수 있음"))] {
            let source = format!("{source}{ORPHAN}");
            let artifact = match load_checked(&source, Form::A, &json!({ "devChecks": enabled })) {
                Ok(artifact) => artifact,
                Err(errors) => {
                    failures.push(format!("선택적 조언이 정상 계약을 차단함 enabled={enabled}: {errors:?}"));
                    continue;
                }
            };
            if artifact.advisories.len() != usize::from(enabled) {
                failures.push(format!("검사 옵션이 적용되지 않음 enabled={enabled}"));
            }
            if digest(&artifact.output.execution) != digest(&baseline.execution) {
                failures.push(format!("docs/조언 변경이 execution을 바꿈: enabled={enabled}"));
            }
            for (actor_id, expected) in &expectations {
                let caller = Caller { actor_id: *actor_id, now: NOW.into() };
                let result = match plan_read(&artifact.output.execution, &request, &caller) {
                    Ok(plan) => execute(&mut db, &plan).await.map(Value::Array),
                    Err(error) => Err(error),
                };
                match result {
                    Ok(rows) if &rows == expected => {}
                    other => failures.push(format!("행/메모 정책 변화 enabled={enabled}, actor={actor_id:?}: {other:?}")),
                }
            }
        }
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    eprintln!("runtime policy cases: 24; schema removed: {schema}");
}
