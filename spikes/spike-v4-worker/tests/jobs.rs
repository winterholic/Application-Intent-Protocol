use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    id_wire::IdWire,
    plan::{Caller, Reject},
};
use spike_v4_worker::{invoke_job, JobHost, Lang, Worker};

struct Host(Vec<String>);
impl JobHost for Host {
    fn call<'a>(
        &'a mut self,
        operation: &'a str,
        name: &'a str,
        input: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, Reject>> + Send + 'a>> {
        Box::pin(async move {
            self.0.push(format!("{operation}.{name}:{}", input["key"].as_str().unwrap_or("missing")));
            let payload = input.get("input").unwrap_or(input);
            Ok(json!({"length":payload["text"].as_str().unwrap().chars().count()}))
        })
    }
}

#[tokio::test]
async fn managed_jobs_broker_only_declared_calls_in_node_and_python() {
    let source = r#"actor principal
      operation job managed { input { text: Text } output { length: Int }
        allow actor != null effect managed access http.echo deadline 2s implementation "job.managed" }
      operation job missing { input { text: Text } output { length: Int }
        allow actor != null effect managed access http.echo deadline 2s implementation "job.missing" }"#;
    let facts = load_str(source, Form::A).unwrap().execution;
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs");
    for lang in [Lang::Node, Lang::Python] {
        let mut worker = Worker::start(lang, dir).await;
        let mut host = Host(vec![]);
        let caller = Caller { actor_id: Some(1), now: "2026-10-11T00:00:00Z".into() };
        let out = invoke_job(&mut worker, &facts, "managed", &json!({"text":"가😀A"}), &caller, IdWire::SafeNumber, &mut host).await.unwrap();
        assert_eq!(out, json!({"length":3}));
        assert_eq!(host.0, vec!["http.echo:charge"]);
        worker.stop().await;
        let mut worker = Worker::start(lang, dir).await;
        let mut host = Host(vec![]);
        let error = invoke_job(&mut worker, &facts, "missing", &json!({"text":"a"}), &caller, IdWire::SafeNumber, &mut host).await.unwrap_err();
        assert_eq!(error.code, "WIRE.BAD_VALUE");
        assert!(host.0.is_empty(), "an HTTP effect without a stable key must be rejected before host dispatch");
        worker.stop().await;
        let denied = source.replace("http.echo", "http.other");
        let facts = load_str(&denied, Form::A).unwrap().execution;
        let mut worker = Worker::start(lang, dir).await;
        let mut host = Host(vec![]);
        let error = invoke_job(&mut worker, &facts, "managed", &json!({"text":"a"}), &caller, IdWire::SafeNumber, &mut host).await.unwrap_err();
        assert_eq!(error.code, "WIRE.ACCESS_NOT_DECLARED");
        assert!(host.0.is_empty());
        worker.stop().await;
    }
}
