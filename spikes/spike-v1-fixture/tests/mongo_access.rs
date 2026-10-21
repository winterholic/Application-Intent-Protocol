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

#[test]
fn a_read_extension_declares_redis_keys_and_prefixes_as_empty_grants() {
    let facts =
        load_str(&source(r#"access mongo.video_distribution, redis "facereview:reco:*", redis "facereview:admin:wau""#), Form::A).unwrap().execution;
    let access = &facts["resources"]["Video"]["extensions"]["list"]["access"];
    assert_eq!(access, &json!({"mongo.video_distribution": {}, "redis.facereview:reco:*": {}, "redis.facereview:admin:wau": {}}));
}

#[test]
fn redis_grants_are_an_exact_key_or_one_trailing_wildcard_after_a_colon() {
    for bad in ["redis \"\"", "redis \"*\"", "redis \"facereview*\"", "redis \"a:*:b\"", "redis \"has space:*\"", "redis \"x/y:*\""] {
        assert_eq!(diagnostics(&format!("access {bad}")), ["DEF.BAD_CAPABILITY"], "{bad}");
    }
    assert_eq!(diagnostics(r#"access redis "facereview:reco:*", redis "facereview:reco:*""#), ["DEF.DUPLICATE"]);
}

fn write_source(access: &str) -> String {
    format!(
        r#"
actor principal
resource Video {{
  fields {{ id: Uuid; title: Text(1..100); view_count: Int(0..1000) }}
  rows read when true
  transition view {{ allow actor != null; from true; to view_count = this.view_count + 1 }}
  expose apply view {{ target id; bulk maxRows 1 }}
  expose read {{
    select id, title
    sort id
    budget {{ rows 10; depth 1; cost 100; deadline 2s }}
  }}
  extension write track {{
    input {{ page: Int(1..100) }}
    output {{ total: Int(0..1000) }}
    access {access}
    effect db
    deadline 2s
    implementation "lists.track"
  }}
}}
"#
    )
}

#[test]
fn a_write_extension_declares_transitions_next_to_mongo_and_redis_grants() {
    let facts = load_str(&write_source(r#"Video.view, mongo.youtube_watching_data, redis "facereview:dedupe:*""#), Form::A).unwrap().execution;
    let access = &facts["resources"]["Video"]["extensions"]["track"]["access"];
    assert_eq!(access, &json!({"Video.view": "transition", "mongo.youtube_watching_data": {}, "redis.facereview:dedupe:*": {}}));
    let only_external = load_str(&write_source("mongo.youtube_watching_data"), Form::A).unwrap().execution;
    assert_eq!(only_external["resources"]["Video"]["extensions"]["track"]["access"], json!({"mongo.youtube_watching_data": {}}));
    let duplicate = match load_str(&write_source("mongo.video_distribution, mongo.video_distribution"), Form::A) {
        Ok(_) => vec![],
        Err(found) => found.iter().map(|d| d.code.to_string()).collect(),
    };
    assert_eq!(duplicate, ["DEF.DUPLICATE"]);
}
