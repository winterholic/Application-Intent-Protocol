use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::connect_with_url;
use spike_v2_read::plan::Caller;
use spike_v4_worker::{invoke, Isolation, Lang, Worker};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const NOW: &str = "2026-10-04T00:00:00Z";
const OLD_STATS: &str = "  extension read stats {\n    input { clubId: Club.Id }\n    output { approvedApplicants: Int }\n    access Apply.approvedCount\n    effect none\n    deadline 2s\n    implementation \"recruitment.stats\"\n  }";

fn facts(input_ty: &str, output_ty: &str, implementation: &str) -> Value {
    let replacement = format!(
        "  extension read stats {{\n    input {{ clubId: Club.Id; value: {input_ty} }}\n    output {{ value: {output_ty} }}\n    access Apply.approvedCount\n    effect none\n    deadline 2s\n    implementation \"{implementation}\"\n  }}"
    );
    assert_eq!(A.matches(OLD_STATS).count(), 1, "fixture stats extension changed");
    load_str(&A.replacen(OLD_STATS, &replacement, 1), Form::A).unwrap_or_else(|e| panic!("invalid generated V1 fixture: {e:?}")).execution
}

#[derive(Clone)]
enum Expected {
    Value(Value),
    Code(&'static str),
}

struct Case {
    name: &'static str,
    facts: Value,
    value: Value,
    expected: Expected,
}

fn case(name: &'static str, input_ty: &str, output_ty: &str, implementation: &str, value: Value, expected: Expected) -> Case {
    Case { name, facts: facts(input_ty, output_ty, implementation), value, expected }
}

fn cases() -> Vec<Case> {
    let mut cases = vec![
        case("Text 입력 NUL 거부", "Text", "Text", "values.echo", json!("left\u{0000}right"), Expected::Code("BAD_VALUE")),
        case("Url 입력 NUL 거부", "Url", "Url", "values.echo", json!("left\u{0000}right"), Expected::Code("BAD_VALUE")),
        case("Text를 잘못된 Time 출력으로 반환", "Text", "Time", "values.echo", json!("2026-02-29T12:34:56Z"), Expected::Code("OUTPUT_INVALID")),
        case("Text 출력 NUL 거부", "Text", "Text", "values.nul", json!("ordinary"), Expected::Code("OUTPUT_INVALID")),
        case("Url 출력 NUL 거부", "Url", "Url", "values.nul", json!("https://example.test/path"), Expected::Code("OUTPUT_INVALID")),
        case("nullable null 왕복", "Text?", "Text?", "values.echo", Value::Null, Expected::Value(json!({"value": null}))),
    ];

    for invalid in ["2026-02-29T12:34:56Z", "2024-02-29T25:00:00Z", "2024-02-29T12:34:56+25:00", "2024-02-29T12:34:56"] {
        cases.push(case("불가능한 Time 입력 거부", "Time", "Time", "values.echo", json!(invalid), Expected::Code("BAD_VALUE")));
    }

    for valid in ["2024-02-29T12:34:56Z", "2024-02-29t12:34:56.123456+05:30", "2024-02-29T12:34:56z"] {
        cases.push(case("유효 Time 원문 보존", "Time", "Time", "values.echo", json!(valid), Expected::Value(json!({"value": valid}))));
    }
    cases
}

fn isolation() -> Isolation {
    #[cfg(target_os = "macos")]
    {
        Isolation::MacNetDeny
    }
    #[cfg(not(target_os = "macos"))]
    {
        Isolation::None
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn official_invoke_enforces_scalar_value_boundaries_for_node_and_python() {
    let cases = cases();
    // No schema or data is needed: echo implementations never call ctx. The client is only
    // required by the public invoke API and uses the local test database explicitly.
    let mut db = connect_with_url("host=localhost dbname=postgres").await.expect("local PostgreSQL connection");
    let ext_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/extensions");
    let caller = Caller { actor_id: None, now: NOW.into() };
    let mut failures = Vec::new();
    let mut table = Vec::new();

    for lang in [Lang::Node, Lang::Python] {
        let mut worker = Worker::start_with(lang, ext_dir, isolation()).await;
        for c in &cases {
            let input = json!({ "clubId": "10", "value": c.value });
            let result = invoke(&mut db, &mut worker, &c.facts, "Recruitment.stats", &input, &caller).await;
            let (actual, passed) = match (&c.expected, result) {
                (Expected::Value(want), Ok(got)) => (format!("OK {got}"), got == *want),
                (Expected::Code(want), Err(got)) => (got.code.to_string(), got.code == *want),
                (Expected::Value(_), Err(got)) => (got.code.to_string(), false),
                (Expected::Code(_), Ok(got)) => (format!("OK {got}"), false),
            };
            table.push(format!("{lang:?} | {} | {actual}", c.name));
            if !passed {
                failures.push(format!(
                    "{lang:?} {}: 기대 {:?}, 실제 {actual}",
                    c.name,
                    match &c.expected {
                        Expected::Value(v) => format!("OK {v}"),
                        Expected::Code(code) => (*code).to_string(),
                    }
                ));
            }
        }
        worker.stop().await;
    }

    eprintln!("{}", table.join("\n"));
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
