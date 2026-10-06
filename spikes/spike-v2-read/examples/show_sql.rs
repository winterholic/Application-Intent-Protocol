use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller};

fn main() {
    let f = load_str(include_str!("../../spike-v1-fixture/fixture/recruitment.aip"), Form::A).unwrap().execution;
    let req = json!({
        "read": "Recruitment",
        "select": ["id", "title", "periodEnd", "bookmarkCount", "internalNote", { "club": { "select": ["id", "name", "logo"] } }],
        "filter": [{ "field": "periodEnd", "op": "gte", "value": "2026-10-09T00:00:00Z" }],
        "sort": [{ "field": "periodEnd", "dir": "asc" }],
        "limit": 20
    });
    let p = plan_read(&f, &req, &Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() }).unwrap();
    println!("{}\n\nparams: {:?}\ncost: {}\noutput: {}", p.sql, p.params, p.cost, p.output_type);
}
