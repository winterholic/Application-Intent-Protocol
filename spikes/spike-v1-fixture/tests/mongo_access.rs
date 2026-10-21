use serde_json::json;
use spike_v1_fixture::{load_str, Form};

fn source(access: &str) -> String {
    format!(
        r#"
actor principal
resource Video {{
  fields {{ id: Uuid; title: Text(1..100) }}
  rows read when true
  expose read {{
    select id, title
    sort id
    budget {{ rows 10; depth 1; cost 100; deadline 2s }}
  }}
  extension read list {{
    input {{ page: Int(1..100) }}
    output {{ total: Int(0..1000) }}
    {access}
    effect none
    deadline 2s
    implementation "lists.list"
  }}
}}
"#
    )
}

fn diagnostics(access: &str) -> Vec<String> {
    match load_str(&source(access), Form::A) {
        Ok(_) => vec![],
        Err(found) => found.iter().map(|d| d.code.to_string()).collect(),
    }
}

#[test]
fn a_read_extension_declares_mongo_collections_as_empty_grants() {
    let facts = load_str(&source("access mongo.video_distribution, mongo.youtube_watching_data"), Form::A).unwrap().execution;
    let access = &facts["resources"]["Video"]["extensions"]["list"]["access"];
    assert_eq!(access, &json!({"mongo.video_distribution": {}, "mongo.youtube_watching_data": {}}));
}

#[test]
fn a_duplicate_mongo_collection_is_rejected() {
    assert_eq!(diagnostics("access mongo.video_distribution, mongo.video_distribution"), ["DEF.DUPLICATE"]);
}

#[test]
fn an_unknown_aggregate_still_needs_a_declaration() {
    assert_eq!(diagnostics("access Video.missing"), ["DEF.UNKNOWN_SYMBOL"]);
}
