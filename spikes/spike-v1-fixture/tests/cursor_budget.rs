//! budget `cursor` opt-in 문법. 다음 페이지 경계 값을 호출자가 이미 볼 수 있는 필드만 cursor에 쓸 수 있다.
use spike_v1_fixture::{load_str, Form};

const A: &str = include_str!("../fixture/recruitment.aip");
const BUDGET: &str = "budget { rows 50; depth 2; deadline 2s; cost 1000 }";

fn with(budget: &str) -> String {
    A.replacen(BUDGET, budget, 1)
}

fn codes(src: &str) -> Vec<String> {
    match load_str(src, Form::A) {
        Ok(_) => vec![],
        Err(ds) => ds.iter().map(|d| format!("{d}")).collect(),
    }
}

#[test]
fn cursor_flag_is_listed_in_facts_only_when_declared() {
    let on = load_str(&with("budget { rows 50; depth 2; deadline 2s; cost 1000; cursor }"), Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    assert_eq!(on["resources"]["Recruitment"]["exposeRead"]["budget"]["cursor"], true);
    let off = load_str(A, Form::A).unwrap().execution;
    assert!(off["resources"]["Recruitment"]["exposeRead"]["budget"].get("cursor").is_none(), "선언이 없으면 키도 없다(기존 facts 호환)");
}

#[test]
fn cursor_requires_sort_fields_and_id_to_be_selectable() {
    // sort 허용 필드가 select에 없으면 cursor 비교로 값을 추론할 수 있다.
    let hidden_sort = with("budget { rows 50; depth 2; deadline 2s; cost 1000; cursor }").replacen(
        "select id, title, periodEnd, views,",
        "select id, title, periodEnd,",
        1,
    );
    assert!(codes(&hidden_sort).iter().any(|c| c.contains("CURSOR_SORT_NOT_SELECTED")), "{:?}", codes(&hidden_sort));
    // cursor를 선언하지 않으면 같은 정의도 기존처럼 통과한다.
    assert!(codes(&A.replacen("select id, title, periodEnd, views,", "select id, title, periodEnd,", 1)).is_empty());
    // id가 select에 없으면 타이브레이커 경계가 숨은 값이 된다.
    let no_id = with("budget { rows 50; depth 2; deadline 2s; cost 1000; cursor }").replacen("select id, title,", "select title,", 1);
    assert!(codes(&no_id).iter().any(|c| c.contains("CURSOR_ID_NOT_SELECTED")), "{:?}", codes(&no_id));
}

#[test]
fn cursor_rejects_nullable_sort_field_and_duplicate_flag() {
    let nullable = with("budget { rows 50; depth 2; deadline 2s; cost 1000; cursor }").replacen(
        "sort periodEnd, views, id",
        "sort periodEnd, views, id, internalNote",
        1,
    );
    let c = codes(&nullable);
    assert!(c.iter().any(|c| c.contains("CURSOR_NULLABLE_SORT")), "{c:?}");
    let dup = with("budget { rows 50; depth 2; deadline 2s; cost 1000; cursor; cursor }");
    assert!(!codes(&dup).is_empty());
}
