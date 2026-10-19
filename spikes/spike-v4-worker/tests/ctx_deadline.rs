//! The host's budget failure is terminal; an extension-supplied error code is untrusted.
//! Real Node/Python IPC, deterministic host errors, no database or timing race required.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    id_wire::IdWire,
    plan::{Caller, Plan, Reject},
    sqlgen::Dialect,
};
use spike_v4_worker::{host::HostFuture, invoke_with_host, Isolation, Lang, Worker, WorkerHost};

struct BudgetHost {
    code: &'static str,
    calls: usize,
}

impl WorkerHost for BudgetHost {
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn execute<'a>(&'a mut self, _plan: &'a Plan, _wire: IdWire) -> HostFuture<'a, Result<Vec<Value>, Reject>> {
        self.calls += 1;
        let code = self.code;
        Box::pin(async move { Err(Reject { code, msg: "deterministic host rejection".into() }) })
    }
}

#[tokio::test]
async fn trusted_ctx_deadlines_are_terminal_but_worker_codes_stay_untrusted() {
    let source = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
    let caller = Caller { actor_id: Some(1), now: "2026-10-20T00:00:00Z".into() };
    let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ctx-deadline");
    let mut failures = vec![];
    for lang in [Lang::Node, Lang::Python] {
        for (implementation, host_code, expected_code, expected_calls) in [
            ("uncaught", "WIRE.DEADLINE_EXCEEDED", Some("WIRE.DEADLINE_EXCEEDED"), 1),
            ("fallback", "WIRE.DEADLINE_EXCEEDED", Some("WIRE.DEADLINE_EXCEEDED"), 1),
            ("fallback", "WIRE.ACCESS_DENIED", None, 1),
            ("forged", "WIRE.DEADLINE_EXCEEDED", Some("WIRE.EXTENSION_ERROR"), 0),
        ] {
            let facts = load_str(
                &source.replacen("implementation \"recruitment.stats\"", &format!("implementation \"budget.{implementation}\""), 1),
                Form::A,
            )
            .unwrap()
            .execution;
            let mut worker = Worker::start_with(lang, directory, Isolation::None).await;
            let mut host = BudgetHost { code: host_code, calls: 0 };
            let result =
                invoke_with_host(&mut host, &mut worker, &facts, "Recruitment.stats", &json!({"clubId":"10"}), &caller, IdWire::Legacy).await;
            worker.stop().await;
            let actual = result.as_ref().err().map(|error| error.code);
            if actual != expected_code || host.calls != expected_calls || (expected_code.is_none() && result != Ok(json!({"approvedApplicants":0}))) {
                failures.push(format!("{lang:?}/{implementation}/{host_code}: result={result:?}, host_calls={}", host.calls));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
