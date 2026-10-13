//! 읽기 거부가 허용 목록을 함께 알린다. 목록은 계약(정의)에서 계산하며 계약에 없는 이름은 담지 않는다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller, Reject};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");

fn caller() -> Caller {
    Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() }
}

fn rejected(facts: &Value, request: Value) -> Reject {
    plan_read(facts, &request, &caller()).err().expect("the request must be rejected")
}

#[test]
fn a_rejected_filter_or_sort_lists_what_the_contract_allows() {
    let facts = load_str(A, Form::A).expect("fixture").execution;

    let filter = rejected(&facts, json!({"read":"Recruitment","select":["id"],"filter":[{"field":"title","op":"eq","value":"x"}]}));
    assert_eq!(filter.code, "FILTER_NOT_ALLOWED");
    assert!(filter.msg.contains("periodEnd.gte") && filter.msg.contains("periodEnd.lte"), "{}", filter.msg);

    let sort = rejected(&facts, json!({"read":"Recruitment","select":["id"],"sort":[{"field":"title"}]}));
    assert_eq!(sort.code, "SORT_NOT_ALLOWED");
    assert!(sort.msg.contains("periodEnd") && sort.msg.contains("views") && sort.msg.contains("id"), "{}", sort.msg);

    let key = rejected(&facts, json!({"read":"Recruitment","select":["id"],"filters":[]}));
    assert_eq!(key.code, "UNKNOWN_KEY");
    for allowed in ["read", "select", "filter", "sort", "limit"] {
        assert!(key.msg.contains(allowed), "{}", key.msg);
    }
    // `offset` and `after` are opt-in keys of the budget; a definition without them must not advertise them.
    assert!(!key.msg.contains("offset") && !key.msg.contains("after"), "{}", key.msg);

    let item = rejected(&facts, json!({"read":"Recruitment","select":["id"],"filter":[{"field":"periodEnd","op":"gte","value":1,"extra":1}]}));
    assert_eq!(item.code, "UNKNOWN_KEY");
    assert!(item.msg.contains("field") && item.msg.contains("op") && item.msg.contains("value"), "{}", item.msg);
}

#[test]
fn an_allow_list_names_nothing_the_contract_does_not_declare() {
    let facts = load_str(A, Form::A).expect("fixture").execution;
    // `title` is selectable but declared in no filter and no sort; it must not appear in either list.
    let filter = rejected(&facts, json!({"read":"Recruitment","select":["id"],"filter":[{"field":"title","op":"eq","value":"x"}]}));
    let listed = filter.msg.split_once("허용").map(|(_, rest)| rest.to_owned()).expect("an allow list follows");
    assert!(!listed.contains("title"), "{listed}");
    let sort = rejected(&facts, json!({"read":"Recruitment","select":["id"],"sort":[{"field":"title"}]}));
    let listed = sort.msg.split_once("허용").map(|(_, rest)| rest.to_owned()).expect("an allow list follows");
    assert!(!listed.contains("title"), "{listed}");
}
