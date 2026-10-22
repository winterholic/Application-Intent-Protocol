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

#[test]
fn a_write_extension_creates_and_reads_only_what_the_definition_exposes() {
    fn source(access: &str, with_create: bool) -> String {
        let create = if with_create { "expose create { fields title; allow actor != null }" } else { "" };
        format!(
            r#"
actor principal
resource Video {{
  fields {{ id: Uuid; title: Text(1..100) }}
  rows read when true
  {create}
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
    let codes = |access: &str, with_create: bool| match load_str(&source(access, with_create), Form::A) {
        Ok(_) => vec![],
        Err(found) => found.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
    };
    let facts = load_str(&source("create.Video, read.Video, mongo.youtube_watching_data", true), Form::A).unwrap().execution;
    assert_eq!(
        facts["resources"]["Video"]["extensions"]["track"]["access"],
        json!({"create.Video": "create", "read.Video": "read", "mongo.youtube_watching_data": {}})
    );
    // A create needs an exposed create, a read any resource; both name a known resource and are declared once.
    assert_eq!(codes("create.Video", false), ["DEF.UNKNOWN_SYMBOL"]);
    assert_eq!(codes("create.Missing", true), ["DEF.UNKNOWN_SYMBOL"]);
    assert_eq!(codes("read.Missing", true), ["DEF.UNKNOWN_SYMBOL"]);
    assert_eq!(codes("read.Video, read.Video", true), ["DEF.DUPLICATE"]);
    assert_eq!(codes("create.Video, create.Video", true), ["DEF.DUPLICATE"]);
    assert!(codes("read.Video", false).is_empty());
}

fn job_source(access: &str) -> String {
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
}}
operation job rebuild {{
  input {{ }}
  output {{ total: Int(0..1000) }}
  allow actor != null
  {access}
  effect managed
  deadline 30s
  implementation "pool.rebuild"
}}
"#
    )
}

fn job_diagnostics(access: &str) -> Vec<String> {
    match load_str(&job_source(access), Form::A) {
        Ok(_) => vec![],
        Err(found) => found.iter().map(|d| d.code.to_string()).collect(),
    }
}

#[test]
fn a_durable_job_declares_mongo_collections_and_redis_keys_as_managed_capabilities() {
    let facts =
        load_str(&job_source(r#"access read.Video, mongo.video_distribution, redis "facereview:reco:*", redis "facereview:admin:wau""#), Form::A)
            .unwrap()
            .execution;
    let access = &facts["operations"]["rebuild"]["access"];
    assert_eq!(
        access,
        &json!({"read.Video": true, "mongo.video_distribution": true, "redis.facereview:reco:*": true, "redis.facereview:admin:wau": true})
    );
}

#[test]
fn a_job_redis_grant_follows_the_same_key_rules_and_a_duplicate_is_refused() {
    for bad in ["redis \"\"", "redis \"*\"", "redis \"facereview*\"", "redis \"a:*:b\""] {
        assert_eq!(job_diagnostics(&format!("access {bad}")), ["DEF.BAD_CAPABILITY"], "{bad}");
    }
    assert_eq!(job_diagnostics(r#"access redis "facereview:reco:*", redis "facereview:reco:*""#), ["DEF.DUPLICATE"]);
    assert_eq!(job_diagnostics("access mongo.video_distribution, mongo.video_distribution"), ["DEF.DUPLICATE"]);
}

#[test]
fn a_durable_job_creates_rows_only_through_an_exposed_create() {
    let with_create = r#"
actor principal
resource Video {
  fields { id: Uuid; title: Text(1..100) }
  rows read when true
  expose create { fields title; allow actor != null }
  expose read { select id, title; sort id; budget { rows 10; depth 1; cost 100; deadline 2s } }
}
resource Note {
  fields { id: Uuid; text: Text(1..100) }
  rows read when true
  expose read { select id, text; sort id; budget { rows 10; depth 1; cost 100; deadline 2s } }
}
operation job collect {
  input { }
  output { total: Int(0..1000) }
  allow actor != null
  access ACCESS
  effect managed
  deadline 30s
  implementation "pool.collect"
}
"#;
    let ok = load_str(&with_create.replace("ACCESS", "read.Video, create.Video"), Form::A).unwrap().execution;
    assert_eq!(ok["operations"]["collect"]["access"], json!({"read.Video": true, "create.Video": true}));
    // No exposed create, or no such resource: refused at definition time.
    for access in ["create.Note", "create.Missing"] {
        let found =
            load_str(&with_create.replace("ACCESS", access), Form::A).err().map(|found| found.iter().map(|d| d.code.to_string()).collect::<Vec<_>>());
        assert_eq!(found, Some(vec!["DEF.UNKNOWN_SYMBOL".to_string()]), "{access}");
    }
    let duplicate = load_str(&with_create.replace("ACCESS", "create.Video, create.Video"), Form::A)
        .err()
        .map(|found| found.iter().map(|d| d.code.to_string()).collect::<Vec<_>>());
    assert_eq!(duplicate, Some(vec!["DEF.DUPLICATE".to_string()]));
}
