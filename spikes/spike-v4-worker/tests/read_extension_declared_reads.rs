//! A read extension may declare the resources its `ctx.data.read` calls with `access read.<Resource>` (opt-in). One that
//! declares none keeps reading any resource with `expose read`; one that declares some reads only those.
//! Real Node/Python IPC with a host that records the plans it is asked to run: no database is needed.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    id_wire::IdWire,
    plan::{Caller, Plan, Reject},
    sqlgen::Dialect,
};
use spike_v4_worker::{host::HostFuture, invoke_with_host, Isolation, Lang, Worker, WorkerHost};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const STATS: &str = "    access Apply.approvedCount\n    effect none\n    deadline 2s\n    implementation \"recruitment.stats\"";

struct Recording {
    plans: usize,
}

impl WorkerHost for Recording {
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn execute<'a>(&'a mut self, _plan: &'a Plan, _wire: IdWire) -> HostFuture<'a, Result<Vec<Value>, Reject>> {
        self.plans += 1;
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// The fixture's `stats` extension, replaced by one that reads and declares `access`.
fn facts(access: &str) -> Value {
    let start = A.find("  extension read stats {").expect("stats extension");
    let end = start + A[start..].find("\n  }\n").expect("end of the extension") + "\n  }\n".len();
    let access = if access.is_empty() { String::new() } else { format!("    access {access}\n") };
    let replacement = format!(
        "  extension read stats {{\n    input {{ name: Text }}\n    output {{ status: Text; rows: Int }}\n{access}    effect none\n    deadline 2s\n    implementation \"reader.run\"\n  }}\n"
    );
    assert!(A[start..end].contains(STATS.lines().next().unwrap()), "the fixture extension changed");
    let source = format!("{}{replacement}{}", &A[..start], &A[end..]);
    load_str(&source, Form::A).unwrap_or_else(|d| panic!("{d:?}")).execution
}

async fn read(lang: Lang, facts: &Value, name: &str) -> (Value, usize) {
    let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/read-declared");
    let caller = Caller { actor_id: Some(1), now: "2026-11-22T00:00:00Z".into() };
    let mut worker = Worker::start_with(lang, directory, Isolation::None).await;
    let mut host = Recording { plans: 0 };
    let result = invoke_with_host(&mut host, &mut worker, facts, "Recruitment.stats", &json!({ "name": name }), &caller, IdWire::Legacy).await;
    worker.stop().await;
    (result.unwrap_or_else(|e| panic!("{lang:?} {name}: {e:?}")), host.plans)
}

#[tokio::test]
async fn an_extension_that_declares_no_read_still_reads_any_exposed_resource() {
    let facts = facts("");
    assert!(facts["resources"]["Recruitment"]["extensions"]["stats"].get("reads").is_none(), "no declaration, no `reads` in the contract");
    for lang in [Lang::Node, Lang::Python] {
        assert_eq!(read(lang, &facts, "Recruitment").await, (json!({ "status": "ok", "rows": 0 }), 1), "{lang:?}");
    }
}

#[tokio::test]
async fn an_extension_that_declares_reads_reads_only_those() {
    let facts = facts("read.Recruitment");
    assert_eq!(facts["resources"]["Recruitment"]["extensions"]["stats"]["reads"], json!(["Recruitment"]));
    for lang in [Lang::Node, Lang::Python] {
        assert_eq!(read(lang, &facts, "Recruitment").await, (json!({ "status": "ok", "rows": 0 }), 1), "{lang:?} declared");
    }
    let other = facts_with_other_declaration();
    for lang in [Lang::Node, Lang::Python] {
        let (out, plans) = read(lang, &other, "Recruitment").await;
        assert_eq!(plans, 0, "{lang:?}: the read of an undeclared resource never reaches the database");
        assert_eq!(out["status"], "WIRE.ACCESS_NOT_DECLARED", "{lang:?}: {out}");
    }
}

fn facts_with_other_declaration() -> Value {
    facts("read.Club")
}
