//! Lowering of analyzed `.aip` files to Core IR: validity, stability and meaning.

use aip_ir::{Program, SourceMap};
use aip_sema::to_core::{LowerNote, NoteKind, to_core_with_notes};
use serde_json::{Value, json};
use std::path::PathBuf;

const EXAMPLES: [&str; 2] = ["examples/ariari/app.aip", "examples/shop/app.aip"];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn load(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn lower(src: &str) -> (Program, SourceMap, Vec<LowerNote>) {
    let ast = aip_syntax::parse_file(src).expect("source parses");
    let expanded = aip_sema::lower::expand(&ast);
    let a = aip_sema::analyze(&expanded);
    let errors: Vec<String> = a.diagnostics.iter().filter(|d| d.severity == aip_syntax::Severity::Error).map(|d| d.render("src")).collect();
    assert!(errors.is_empty(), "source has errors:\n{}", errors.join("\n"));
    to_core_with_notes(&a)
}

fn json(p: &Program) -> Value {
    serde_json::to_value(p).expect("program serializes")
}

/// Paths of every `{"t": "unknown"}` type in the program.
fn unknown_types(v: &Value, path: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            if m.get("t").and_then(Value::as_str) == Some("unknown") {
                out.push(path.to_string());
            }
            for (k, x) in m {
                unknown_types(x, &format!("{path}.{k}"), out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                unknown_types(x, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

#[test]
fn examples_lower_to_valid_core_without_unknowns() {
    for rel in EXAMPLES {
        let (p, _, notes) = lower(&load(rel));
        let diags = aip_ir::validate::validate(&p);
        assert!(diags.is_empty(), "{rel}: {diags:#?}");
        let mut unknown = Vec::new();
        unknown_types(&json(&p), "", &mut unknown);
        assert!(unknown.is_empty(), "{rel}: {} unknown types, first at {:?}", unknown.len(), unknown.first());
        let unresolved: Vec<String> =
            notes.iter().filter(|n| n.kind == NoteKind::Unresolved).map(|n| format!("{}:{} {}", n.line, n.col, n.message)).collect();
        assert!(unresolved.is_empty(), "{rel}: unresolved names: {unresolved:?}");
        assert_eq!(p.aip_core, aip_ir::CORE_IR_VERSION);
        assert!(!p.entities.is_empty() && !p.intents.is_empty(), "{rel}: nothing was lowered");
    }
}

#[test]
fn json_round_trip_is_identity() {
    for rel in EXAMPLES {
        let (p, _, _) = lower(&load(rel));
        let text = aip_ir::canonical_json(&p);
        let back: Program = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert!(back == p, "{rel}: deserialized program differs from the original");
        assert_eq!(aip_ir::canonical_json(&back), text, "{rel}: canonical text changed after a round trip");
    }
}

#[test]
fn lowering_is_deterministic() {
    for rel in EXAMPLES {
        let src = load(rel);
        let (a, _, _) = lower(&src);
        let (b, _, _) = lower(&src);
        assert_eq!(aip_ir::digest(&a), aip_ir::digest(&b), "{rel}");
    }
}

/// Negative control for "spans do not leak into the IR": the variant moves every
/// declaration to a different line and column, so a leak would change the digest.
#[test]
fn whitespace_and_comments_do_not_change_the_digest() {
    for rel in EXAMPLES {
        let src = load(rel);
        let noisy: String = format!("// header comment\n\n\n{}", src.lines().map(|l| format!("{l}   // trailing\n\n")).collect::<String>());
        assert_ne!(src, noisy);
        let (a, sm_a, _) = lower(&src);
        let (b, sm_b, _) = lower(&noisy);
        assert_ne!(sm_a, sm_b, "{rel}: the variant must actually move source positions, otherwise this test proves nothing");
        assert_eq!(aip_ir::digest(&a), aip_ir::digest(&b), "{rel}");
    }
}

#[test]
fn a_different_meaning_changes_the_digest() {
    let src = load("examples/shop/app.aip");
    let changed =
        src.replace("enum OrderStatus { PENDING PAID CANCELLED REFUNDED }", "enum OrderStatus { PENDING PAID CANCELLED REFUNDED ARCHIVED }");
    assert_ne!(src, changed, "the replacement must hit");
    let (a, _, _) = lower(&src);
    let (b, _, _) = lower(&changed);
    assert_ne!(aip_ir::digest(&a), aip_ir::digest(&b));
    assert_eq!(b.enums["OrderStatus"].values.last().map(String::as_str), Some("ARCHIVED"));
}

#[test]
fn a_different_limit_changes_the_digest() {
    // refinements are part of the type, so they are part of the meaning
    let src = load("examples/shop/app.aip");
    let changed = src.replace("name: Text(1..100)", "name: Text(1..101)");
    assert_ne!(src, changed);
    assert_ne!(aip_ir::digest(&lower(&src).0), aip_ir::digest(&lower(&changed).0));
}

const FACTS: &str = r#"
use auth
actor Member via auth.oidc(kakao)
enum Role { USER ADMIN }
record Input { title: Text(1..10, trim) }
entity Member { role: Role = USER }
entity Post {
  author: Member
  title: Text(1..10, trim)
  views: Int
  predicate mine = author = actor
}
command Create(input: Input, n: Int(1..5)) idempotent {
  allow authenticated
  require n > 1 else TOO_FEW
  do { insert Post { author: actor, ...input, views: n } as post }
  emit Created { post }
  returns post { id }
}
command Bump(post: Post) {
  allow actor.role = ADMIN
  do { set post.views += 1 }
}
query Mine() {
  allow authenticated
  from Post p
  where p is mine and p.author.role = ADMIN
  page 10 by keyset
  select { id title }
}
"#;

#[test]
fn names_resolve_to_their_kind() {
    let (p, sm, notes) = lower(FACTS);
    assert!(notes.iter().all(|n| n.kind != NoteKind::Unresolved), "{notes:?}");
    assert!(aip_ir::validate::validate(&p).is_empty());
    let v = json(&p);

    // Param: `n` in a require; RowField: bare `author` in an entity predicate; Actor keyword
    let n = &v["intents"]["Create"]["requires"][0]["cond"]["l"];
    assert_eq!((n["e"].clone(), n["name"].clone()), (json!("param"), json!("n")));
    let mine = &v["entities"]["Post"]["predicates"]["mine"];
    assert_eq!(mine["l"], json!({"ty": {"t": "ref", "entity": "Member"}, "e": "row_field", "entity": "Post", "field": "author"}));
    assert_eq!(mine["r"]["e"], json!("actor"));

    // EnumValue and Field with the entity of its base
    let allow = &v["intents"]["Bump"]["allow"]["cond"];
    assert_eq!(allow["l"]["e"], json!("field"));
    assert_eq!(allow["l"]["entity"], json!("Member"));
    assert_eq!(allow["r"], json!({"ty": {"t": "enum", "name": "Role"}, "e": "enum_value", "enum": "Role", "value": "ADMIN"}));

    // Local (set alias) and Pred
    let filter = &v["intents"]["Mine"]["filter"];
    assert_eq!(filter["l"]["e"], json!("pred"));
    assert_eq!(filter["l"]["entity"], json!("Post"));
    assert_eq!(filter["l"]["name"], json!("mine"));
    assert_eq!(filter["l"]["row"]["e"], json!("local"));
    assert_eq!(filter["r"]["l"]["base"]["entity"], json!("Post"));

    // positions for declarations
    assert!(sm.contains_key("entities.Post") && sm.contains_key("intents.Create") && sm.contains_key("entities.Post.fields.views"));
}

#[test]
fn sugar_is_gone() {
    let (p, _, _) = lower(FACTS);
    let v = json(&p);
    // spread of a record parameter becomes one field read per record field; order is source order
    let values = &v["intents"]["Create"]["body"][0]["values"];
    let names: Vec<&str> = values.as_array().expect("values").iter().filter_map(|x| x[0].as_str()).collect();
    assert_eq!(names, ["author", "title", "views"]);
    assert_eq!(values[1][1]["e"], json!("field"));
    assert_eq!(values[1][1]["base"]["e"], json!("param"));
    assert_eq!(values[1][1]["entity"], json!(null));
    // `+= 1` stays an assignment operator on a resolved field path
    let assign = &v["intents"]["Bump"]["body"][0]["assigns"][0];
    assert_eq!(assign["op"], json!("add"));
    assert_eq!(assign["target"]["entity"], json!("Post"));
    // shorthand `emit Created { post }` reads the insert binding
    assert_eq!(v["intents"]["Create"]["emits"][0]["fields"][0][1]["e"], json!("local"));
    assert_eq!(v["events"]["Created"]["declared"], json!(false));
}

#[test]
fn declared_types_keep_their_refinements() {
    let (p, _, _) = lower(FACTS);
    let v = json(&p);
    assert_eq!(v["records"]["Input"]["fields"][0]["ty"], json!({"t": "text", "min": 1, "max": 10, "trim": true}));
    assert_eq!(v["intents"]["Create"]["params"][1]["ty"], json!({"t": "int", "min": 1, "max": 5}));
    // expression types do not: `n` and `n: n` must lower alike
    assert_eq!(v["intents"]["Create"]["body"][0]["values"][2][1]["ty"], json!({"t": "int"}));
    let fields = v["entities"]["Post"]["fields"].as_array().expect("fields");
    assert_eq!(fields[0]["name"], json!("id"));
    assert_eq!(fields[0]["kind"], json!({"k": "implicit"}));
}

#[test]
fn intent_surface_maps_one_to_one() {
    let (p, _, _) = lower(FACTS);
    let v = json(&p);
    assert_eq!(v["intents"]["Create"]["idempotency"], json!({"k": "caller_key"}));
    assert_eq!(v["intents"]["Bump"]["idempotency"], json!({"k": "none"}));
    assert_eq!(v["intents"]["Bump"]["output"], json!({"shape": "none"}));
    assert_eq!(v["intents"]["Create"]["output"]["shape"], json!("object"));
    assert_eq!(v["intents"]["Mine"]["output"]["shape"], json!("list"));
    assert_eq!(v["intents"]["Mine"]["page"], json!({"size": 10, "offset_max_page": null}));
}

/// Every conformance case that the checker accepts must lower to a valid program
/// without unresolved names; this exercises syntax the two examples do not use.
#[test]
fn accepted_conformance_cases_lower_cleanly() {
    let dir = root().join("conformance/sema");
    let prelude = std::fs::read_to_string(dir.join("_prelude.aip")).expect("prelude");
    let mut lowered = 0;
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "aip"))
        .collect();
    paths.sort();
    for case in paths {
        let name = case.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('_') {
            continue;
        }
        let src = format!("{prelude}\n{}", std::fs::read_to_string(&case).expect("case"));
        let Ok(ast) = aip_syntax::parse_file(&src) else { continue };
        let expanded = aip_sema::lower::expand(&ast);
        let a = aip_sema::analyze(&expanded);
        if aip_sema::has_errors(&a.diagnostics) {
            continue;
        }
        let (p, _, notes) = to_core_with_notes(&a);
        let diags = aip_ir::validate::validate(&p);
        assert!(diags.is_empty(), "{name}: {diags:#?}");
        let unresolved: Vec<&LowerNote> = notes.iter().filter(|n| n.kind == NoteKind::Unresolved).collect();
        assert!(unresolved.is_empty(), "{name}: {unresolved:?}");
        lowered += 1;
    }
    assert!(lowered > 0, "no conformance case was accepted by the checker; the test would prove nothing");
}

#[test]
fn unknown_type_detector_can_fail() {
    // negative control for the zero-unknown assertion above
    let v = json!({"a": [{"ty": {"t": "unknown"}}], "b": {"t": "int"}});
    let mut found = Vec::new();
    unknown_types(&v, "", &mut found);
    assert_eq!(found, [".a[0].ty"]);
}

#[test]
fn dropped_refinement_is_reported_not_silent() {
    // an option the IR has no field for must produce a note instead of vanishing
    let (_, _, notes) = lower("record R { n: Int(1..5, weird) }\n");
    assert!(notes.iter().any(|n| n.kind == NoteKind::Proposal && n.message.contains("Int")), "{notes:?}");
}

/// One construct per Core IR node that used to be approximated.
const PRESERVED: &str = r#"
use auth
use redis
actor Member via auth.oidc(kakao)
entity Member { nick: Text }
entity Product { stock: Int }
entity Order { items: OrderItem[] via order }
entity OrderItem { order: Order  product: Product  quantity: Int }
entity Club {
  name: Text
  views: Int counter via redis
  period: Range<Time>
  tags: Tag[] via club visible to authenticated
  target: ref Member | Product  on erase cascade
  predicate open = now in period
}
entity Tag { club: Club }
event Created { club: Club }
record R { body: RichText(policy: basic)  a: Int  b: Int  check a < b else BAD }
command Ship(order: Order) {
  allow actor is Member
  do { update Product p via order.items i set p.stock += i.quantity }
}
on Created e when e.club != null do { }
query Open() {
  allow public
  from Club c
  where c is open
  page 10 by keyset
  select { id tags { id } }
}
"#;

fn preserved() -> Value {
    let (p, _, notes) = lower(PRESERVED);
    assert!(notes.iter().all(|n| !matches!(n.kind, NoteKind::Unresolved | NoteKind::Approx | NoteKind::Proposal)), "{notes:?}");
    assert!(aip_ir::validate::validate(&p).is_empty());
    json(&p)
}

#[test]
fn ir_version_is_0_11() {
    assert_eq!(aip_ir::CORE_IR_VERSION, "aip-core/0.11");
    assert_eq!(preserved()["aip_core"], json!("aip-core/0.11"));
}

#[test]
fn evolution_declarations_are_in_core_ir_and_absent_when_unused() {
    let src = "entity Job was Task {\n  title: Text was name\n  removed field legacy\n}\nremoved entity Gone\n";
    let v = json(&lower(src).0);
    assert_eq!(v["entities"]["Job"]["was"], json!("Task"));
    assert_eq!(v["entities"]["Job"]["removed_fields"], json!(["legacy"]));
    assert_eq!(v["entities"]["Job"]["fields"][1]["was"], json!("name"));
    assert_eq!(v["removed_entities"], json!(["Gone"]));
    // a program that declares nothing serializes exactly as before the declarations existed
    let plain = json(&lower("entity Task { title: Text }\n").0);
    assert!(
        plain.get("removed_entities").is_none()
            && plain["entities"]["Task"].get("was").is_none()
            && plain["entities"]["Task"].get("removed_fields").is_none()
    );
    assert!(plain["entities"]["Task"]["fields"].as_array().is_some_and(|fs| fs.iter().all(|f| f.get("was").is_none())));
}

#[test]
fn identifier_argument_is_a_symbol_not_text() {
    let v = preserved();
    assert_eq!(v["actor"]["provider"]["args"][0]["value"], json!({"ty": {"t": "symbol"}, "e": "symbol", "name": "kakao"}));
}

#[test]
fn rich_text_policy_is_kept() {
    let v = preserved();
    assert_eq!(v["records"]["R"]["fields"][0]["ty"], json!({"t": "rich_text", "policy": "basic"}));
    // without a policy the key is absent
    let (p, _, _) = lower("record Q { body: RichText }\n");
    assert_eq!(json(&p)["records"]["Q"]["fields"][0]["ty"], json!({"t": "rich_text"}));
}

#[test]
fn update_via_is_a_field_of_the_statement() {
    let v = preserved();
    let body = v["intents"]["Ship"]["body"].as_array().expect("body");
    assert_eq!(body.len(), 1, "no helper Let is inserted");
    assert_eq!(body[0]["s"], json!("update"));
    assert_eq!(body[0]["via"]["alias"], json!("i"));
    assert_eq!(body[0]["via"]["path"]["field"], json!("items"));
}

#[test]
fn type_test_is_not_a_predicate() {
    let v = preserved();
    let cond = &v["intents"]["Ship"]["allow"]["cond"];
    assert_eq!(cond["e"], json!("type_test"));
    assert_eq!(cond["entity"], json!("Member"));
    assert_eq!(cond["value"]["e"], json!("actor"));
}

#[test]
fn record_check_reads_record_fields() {
    let v = preserved();
    let l = &v["records"]["R"]["checks"][0]["cond"]["l"];
    assert_eq!((l["e"].clone(), l["record"].clone(), l["field"].clone()), (json!("record_field"), json!("R"), json!("a")));
    // entity predicates still use row_field
    assert_eq!(v["entities"]["Club"]["predicates"]["open"]["range"]["e"], json!("row_field"));
}

#[test]
fn event_binding_has_an_event_type() {
    let v = preserved();
    let base = &v["reactions"][0]["when"]["l"]["base"];
    assert_eq!(base["ty"], json!({"t": "event", "name": "Created"}));
    assert!(!canon(&v).contains("event:"), "the `event:` record spelling is gone");
}

fn canon(v: &Value) -> String {
    v.to_string()
}

#[test]
fn ref_union_keeps_on_erase() {
    let v = preserved();
    let f = v["entities"]["Club"]["fields"].as_array().expect("fields").iter().find(|f| f["name"] == "target").expect("target");
    assert_eq!(f["kind"]["k"], json!("ref_union"));
    assert_eq!(f["kind"]["on_erase"], json!({"policy": "cascade"}));
}

#[test]
fn nullable_list_shape_is_kept() {
    // a field guarded by `visible to` may be absent, and so may the list built from it
    let v = preserved();
    let tags = &v["intents"]["Open"]["output"]["of"]["fields"][1];
    assert_eq!(tags[0], json!("tags"));
    assert_eq!((tags[1]["shape"].clone(), tags[1]["nullable"].clone()), (json!("list"), json!(true)));
    assert_eq!(v["intents"]["Open"]["output"]["nullable"], json!(false));
}

#[test]
fn counter_names_its_store_extension() {
    let v = preserved();
    let f = v["entities"]["Club"]["fields"].as_array().expect("fields").iter().find(|f| f["name"] == "views").expect("views");
    assert_eq!(f["kind"], json!({"k": "counter", "store": "redis"}));
}

#[test]
fn range_membership_is_not_set_membership() {
    let v = preserved();
    let open = &v["entities"]["Club"]["predicates"]["open"];
    assert_eq!(open["e"], json!("in_range_value"));
    assert_eq!(open["value"]["e"], json!("now"));
    // a collection on the right stays InSet
    let (p, _, _) = lower(
        "entity A { n: Int }\nquery Q(ns: Set<Int> max 5) {\n  allow public\n  from A a\n  where a.n in ns\n  page 10 by keyset\n  select { id }\n}\n",
    );
    assert_eq!(json(&p)["intents"]["Q"]["filter"]["e"], json!("in_set"));
}

#[test]
fn counter_store_must_be_declared() {
    // negative control for I119 through the whole pipeline: the checker accepts the program only with `use redis`
    let (mut p, _, _) = lower(PRESERVED);
    p.uses.retain(|u| u != "redis");
    let d = aip_ir::validate::validate(&p);
    assert!(d.iter().any(|x| x.code == "AIP-I119"), "{d:?}");
}

#[test]
fn named_json_schema_and_dynamic_validation_are_different_types() {
    let src = r#"
record Q { key: Text }
entity Form { questions: List<Q> max 10  dynamic schema from questions }
entity Answer {
  form: Snapshot<Form>
  named: Json<Q>
  answers: Json validated by form
  plain: Json
}
"#;
    let (p, _, _) = lower(src);
    assert!(aip_ir::validate::validate(&p).is_empty());
    let v = json(&p);
    let ty = |f: &str| v["entities"]["Answer"]["fields"].as_array().and_then(|fs| fs.iter().find(|x| x["name"] == f)).map(|x| x["ty"].clone());
    assert_eq!(ty("named"), Some(json!({"t": "json", "schema": "Q"})));
    assert_eq!(ty("answers"), Some(json!({"t": "json", "validated_by": "form"})));
    assert_eq!(ty("plain"), Some(json!({"t": "json"})));
}

#[test]
fn cross_tenant_reaches_core_ir_on_declarations_without_a_caller() {
    let src = std::fs::read_to_string(root().join("conformance/sema/ok_tenant_cross_reactions.aip")).expect("case");
    let (p, _, _) = lower(&src);
    assert!(aip_ir::validate::validate(&p).is_empty());
    let v = json(&p);
    let flags: Vec<(String, bool)> = v["reactions"]
        .as_array()
        .expect("reactions")
        .iter()
        .map(|r| {
            let handlers = r["handlers"].as_array().map(|h| h.iter().all(|x| x["cross_tenant"] == json!(true)));
            (r["on"].as_str().unwrap_or_default().to_string(), r["cross_tenant"] == json!(true) || handlers == Some(true))
        })
        .collect();
    assert_eq!(flags, [("event", true), ("schedule", true), ("rule", true), ("webhook", true)].map(|(k, b)| (k.to_string(), b)).to_vec());
    // without the modifier the key is absent, so existing programs serialize as before
    let (p, _, _) = lower("record Ping { id: Uuid }\nevent E { n: Int }\non E e do { }\n");
    assert!(json(&p)["reactions"][0].get("cross_tenant").is_none());
}
