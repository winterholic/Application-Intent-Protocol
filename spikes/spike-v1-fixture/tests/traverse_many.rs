//! 1:N traverse(`traverse comments via Comment.post { ... }`). 정의가 자식 개수 상한과 정렬을 고정한다.
use spike_v1_fixture::{load_str, Form};

const DEF: &str = "
enum CommentState { VISIBLE, HIDDEN }
resource Member { fields { id: Id } }
actor Member
resource Post {
  fields { id: Id; title: Text(1..100) }
  rows read when true
  expose read {
    select id, title
    sort id
    traverse comments via Comment.post { select id, body, secret; sort id; limit 20 }
    budget { rows 10; depth 2; deadline 2s; cost 1000 }
  }
}
resource Comment {
  fields { id: Id; post: Post; author: Member; body: Text(1..200); secret: Text?; state: CommentState }
  rows read when state = VISIBLE
  field secret read when author = actor
  expose read { select id, body, secret, state }
}
";

fn codes(src: &str) -> Vec<String> {
    match load_str(src, Form::A) {
        Ok(_) => vec![],
        Err(ds) => ds.iter().map(|d| format!("{d}")).collect(),
    }
}

fn has(src: &str, code: &str) -> bool {
    let c = codes(src);
    c.iter().any(|x| x.contains(code))
}

#[test]
fn facts_list_traverse_many_only_when_declared() {
    let f = load_str(DEF, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let t = &f["resources"]["Post"]["exposeRead"]["traverseMany"]["comments"];
    assert_eq!(t["target"], "Comment");
    assert_eq!(t["via"], "post");
    assert_eq!(t["limit"], 20);
    assert_eq!(t["select"], serde_json::json!(["body", "id", "secret"]));
    assert_eq!(t["sort"], serde_json::json!({ "field": "id", "desc": false }));
    // 선언이 없으면 키도 없다(기존 facts 호환)
    let plain = DEF.replacen("traverse comments via Comment.post { select id, body, secret; sort id; limit 20 }", "", 1);
    let f = load_str(&plain, Form::A).unwrap().execution;
    assert!(f["resources"]["Post"]["exposeRead"].get("traverseMany").is_none());
}

#[test]
fn sort_is_optional_and_may_be_descending() {
    let d = DEF.replacen("sort id; limit 20", "sort id desc; limit 20", 1);
    let f = load_str(&d, Form::A).unwrap().execution;
    assert_eq!(f["resources"]["Post"]["exposeRead"]["traverseMany"]["comments"]["sort"]["desc"], true);
    let none = DEF.replacen("sort id; limit 20", "limit 20", 1);
    assert!(codes(&none).is_empty(), "{:?}", codes(&none));
}

#[test]
fn limit_is_required_and_positive() {
    assert!(has(&DEF.replacen("; limit 20", "", 1), "MISSING_ITEM"));
    assert!(has(&DEF.replacen("limit 20", "limit 0", 1), "BAD_LIMIT"));
}

#[test]
fn definition_errors_are_diagnosed() {
    // 자식 select는 자식의 expose select 안이어야 한다
    assert!(has(&DEF.replacen("select id, body, secret;", "select id, body, post;", 1), "TRAVERSE_NOT_EXPOSED"));
    // via 필드는 부모를 가리키는 Ref여야 한다
    assert!(has(&DEF.replacen("via Comment.post", "via Comment.author", 1), "TRAVERSE_VIA_NOT_REF"));
    assert!(has(&DEF.replacen("via Comment.post", "via Comment.nope", 1), "UNKNOWN_FIELD"));
    assert!(has(&DEF.replacen("via Comment.post", "via Nope.post", 1), "UNKNOWN_RESOURCE"));
    // 자식에 expose read가 없으면 불가
    assert!(has(&DEF.replacen("expose read { select id, body, secret, state }", "", 1), "TRAVERSE_NOT_EXPOSED"));
    // field read 정책이 있는 필드는 정렬에 쓸 수 없다
    assert!(has(&DEF.replacen("sort id; limit 20", "sort secret; limit 20", 1), "POLICY_FIELD_NOT_FILTERABLE"));
    // depth는 2 이상
    assert!(has(&DEF.replacen("depth 2", "depth 1", 1), "BUDGET_DEPTH_TOO_SMALL"));
    // 이름은 부모 필드와 겹칠 수 없다
    assert!(has(&DEF.replacen("traverse comments via", "traverse title via", 1), "DUPLICATE"));
}

#[test]
fn nesting_is_rejected_in_definition() {
    // 자식 select 안에 또 traverse를 쓰는 문법은 없다
    let d = DEF.replacen(
        "select id, body, secret; sort id; limit 20",
        "select id, body; traverse x via Comment.post { select id; limit 1 }; limit 20",
        1,
    );
    assert!(has(&d, "PARSE_"), "{:?}", codes(&d));
}

#[test]
fn aggregate_in_child_select_is_held_back() {
    let agg =
        "aggregate reactions: Int { source Reaction; sourceAccess allReactions; groupKey comment; callerFilter none; rowOutput none; release count }";
    let d = DEF
        .replacen(
            "expose read { select id, body, secret, state }",
            &format!("expose read {{ select id, body, secret, state, reactions }}\n  {agg}"),
            1,
        )
        .replacen("select id, body, secret; sort id; limit 20", "select id, reactions; sort id; limit 20", 1)
        + "resource Reaction { fields { id: Id; comment: Comment } }\naccess allReactions = totalOfVisible(Reaction)\n";
    assert!(has(&d, "TRAVERSE_MANY_AGGREGATE"), "{:?}", codes(&d));
}
