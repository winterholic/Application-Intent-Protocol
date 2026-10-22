//! External effects of one write extension call (WBS 7.2.5): the Mongo and Redis writes it makes next to the relational transaction.
//!
//! The relational commit cannot include them, so this is the original's saga (`common/saga`, `SagaContext` and the saga log):
//! every effect happens first, a step that asked for `compensate: true` keeps the record that undoes it, and if the relational
//! transaction does not commit the steps are undone in reverse order. The saga log (`saga_transaction_log`, same document shape as the
//! original's) is written before the first compensable effect and rewritten as the saga advances, so a crash leaves a `in_progress`
//! document naming what may need undoing. A commit whose result is unknown is **not** undone: the effects may belong to a committed
//! transaction, so the log says `unknown` and an operator decides.
use crate::capability::{CapabilityPrincipal, MongoCapability, RedisCapability};
use serde_json::{json, Value};
use spike_v2_read::plan::Reject;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn reject(code: &'static str, msg: &str) -> Reject {
    Reject { code, msg: msg.to_string() }
}

fn now() -> Value {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as i64);
    json!({"$date": {"$numberLong": millis.to_string()}})
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StepState {
    Pending,
    Completed,
    Failed,
    Compensated,
    /// Compensation was attempted and failed; the effect may still be in place.
    Stuck,
}

impl StepState {
    fn name(self) -> &'static str {
        match self {
            StepState::Pending => "pending",
            StepState::Completed => "completed",
            StepState::Failed => "failed",
            StepState::Compensated => "compensated",
            StepState::Stuck => "failed",
        }
    }
}

struct Step {
    name: String,
    kind: &'static str,
    resource: String,
    state: StepState,
    started_at: Value,
    completed_at: Value,
    compensated_at: Value,
    error: Option<String>,
    compensation: Option<Value>,
}

/// How a rolled-back saga ended.
#[derive(Debug, PartialEq, Eq)]
pub enum RollbackOutcome {
    /// Nothing to undo, or every undo ran.
    Clean,
    /// At least one undo failed or the compensating capability was gone; the log says `failed` and names the steps.
    Failed { steps: Vec<String> },
}

/// The effects of one write extension call.
pub struct ExternalEffects {
    mongo: Option<Arc<dyn MongoCapability>>,
    redis: Option<Arc<dyn RedisCapability>>,
    principal: CapabilityPrincipal,
    transaction_id: String,
    metadata: Value,
    created_at: Value,
    steps: Vec<Step>,
    status: &'static str,
    completed_at: Value,
    compensation_started_at: Value,
    compensation_completed_at: Value,
    /// The saga log has been started (a compensable effect was announced).
    journaling: bool,
}

impl ExternalEffects {
    pub fn new(
        mongo: Option<Arc<dyn MongoCapability>>,
        redis: Option<Arc<dyn RedisCapability>>,
        principal: CapabilityPrincipal,
        transaction_id: String,
        extension: &str,
    ) -> Self {
        Self {
            mongo,
            redis,
            principal,
            transaction_id,
            metadata: json!({"extension": extension}),
            created_at: now(),
            steps: Vec::new(),
            status: "in_progress",
            completed_at: Value::Null,
            compensation_started_at: Value::Null,
            compensation_completed_at: Value::Null,
            journaling: false,
        }
    }

    /// `true` when the call has done nothing outside the relational transaction.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    fn document(&self) -> Value {
        let steps: Vec<Value> = self
            .steps
            .iter()
            .map(|step| {
                json!({
                    "name": step.name, "kind": step.kind, "resource": step.resource, "status": step.state.name(),
                    "started_at": step.started_at, "completed_at": step.completed_at, "compensated_at": step.compensated_at,
                    "error_message": step.error, "compensation_data": step.compensation.clone().unwrap_or(Value::Null),
                    "compensation_stuck": step.state == StepState::Stuck,
                })
            })
            .collect();
        json!({
            "transaction_id": self.transaction_id, "status": self.status, "steps": steps, "created_at": self.created_at,
            "completed_at": self.completed_at, "compensation_started_at": self.compensation_started_at,
            "compensation_completed_at": self.compensation_completed_at, "metadata": self.metadata,
        })
    }

    async fn journal(&self) -> Result<(), Reject> {
        match (&self.mongo, self.journaling) {
            (Some(mongo), true) => mongo.journal(&self.document()).await,
            _ => Ok(()),
        }
    }

    /// Runs one external write. `kind` is `mongo` (resource = collection) or `redis` (resource = key).
    pub async fn call(&mut self, kind: &str, resource: &str, input: &Value) -> Result<Value, Reject> {
        let kind: &'static str = match kind {
            "mongo" => "mongo",
            "redis" => "redis",
            _ => return Err(reject("WIRE.UNSUPPORTED", "알 수 없는 external capability")),
        };
        if crate::is_external_read(kind, resource, input) {
            // A read leaves no effect to journal or undo: the capability checks the grant and its own namespace on every call.
            return match kind {
                "mongo" => match &self.mongo {
                    Some(mongo) => mongo.call(&self.principal, resource, input).await,
                    None => Err(reject("WIRE.ADAPTER_UNSUPPORTED", "Mongo capability가 설정되지 않음")),
                },
                _ => match &self.redis {
                    Some(redis) => redis.call(&self.principal, resource, input).await,
                    None => Err(reject("WIRE.ADAPTER_UNSUPPORTED", "Redis capability가 설정되지 않음")),
                },
            };
        }
        let command = input.get("command").and_then(Value::as_str).unwrap_or("?").to_string();
        let announced = input.get("compensate") == Some(&Value::Bool(true));
        let index = self.steps.len();
        self.steps.push(Step {
            name: format!("{kind}.{resource}.{command}"),
            kind,
            resource: resource.to_string(),
            state: StepState::Pending,
            started_at: now(),
            completed_at: Value::Null,
            compensated_at: Value::Null,
            error: None,
            compensation: None,
        });
        if announced {
            // Journal first: if Mongo cannot take the saga log, the compensable effect does not happen. The log starts with the first
            // compensable effect; earlier steps (frames, dedupe keys) are already in the document.
            let started = !self.journaling && self.mongo.is_some();
            if started {
                self.journaling = true;
            }
            if let Err(error) = self.journal().await {
                self.steps.pop();
                if started {
                    self.journaling = false;
                }
                return Err(error);
            }
        }
        let result = match kind {
            "mongo" => match &self.mongo {
                Some(mongo) => mongo.write(&self.principal, resource, input).await,
                None => Err(reject("WIRE.ADAPTER_UNSUPPORTED", "Mongo capability가 설정되지 않음")),
            },
            _ => match &self.redis {
                Some(redis) => redis.write(&self.principal, resource, input).await,
                None => Err(reject("WIRE.ADAPTER_UNSUPPORTED", "Redis capability가 설정되지 않음")),
            },
        };
        match result {
            Ok(outcome) => {
                let step = &mut self.steps[index];
                step.state = StepState::Completed;
                step.completed_at = now();
                step.compensation = outcome.compensation;
                if announced && self.journal().await.is_err() {
                    // The effect is in place but the log cannot say so; undo it now rather than leave an unlogged effect.
                    let step = &self.steps[index];
                    if let Some(record) = step.compensation.clone() {
                        let _ = self.compensate_one(kind, resource, &record).await;
                    }
                    self.steps[index].state = StepState::Failed;
                    self.steps[index].error = Some("saga log unavailable".into());
                    return Err(reject("WIRE.ADAPTER_UNAVAILABLE", "saga log를 쓰지 못해 효과를 되돌림"));
                }
                Ok(outcome.value)
            }
            Err(error) => {
                let step = &mut self.steps[index];
                step.state = StepState::Failed;
                step.completed_at = now();
                step.error = Some(error.code.to_string());
                if announced {
                    let _ = self.journal().await;
                }
                Err(error)
            }
        }
    }

    async fn compensate_one(&self, kind: &str, resource: &str, record: &Value) -> Result<(), Reject> {
        match kind {
            "mongo" => match &self.mongo {
                Some(mongo) => mongo.compensate(resource, record).await,
                None => Err(reject("WIRE.ADAPTER_UNSUPPORTED", "Mongo capability가 없음")),
            },
            _ => match &self.redis {
                Some(redis) => redis.compensate(resource, record).await,
                None => Err(reject("WIRE.ADAPTER_UNSUPPORTED", "Redis capability가 없음")),
            },
        }
    }

    /// The relational transaction did not commit: undo every completed step that has an undo record, newest first. A failing undo does
    /// not stop the others (the original stops at the first; here each step still has its own chance to be undone).
    pub async fn rollback(&mut self) -> RollbackOutcome {
        let pending: Vec<usize> = (0..self.steps.len())
            .rev()
            .filter(|index| self.steps[*index].state == StepState::Completed && self.steps[*index].compensation.is_some())
            .collect();
        if pending.is_empty() {
            self.status = if self.steps.iter().any(|step| step.state == StepState::Completed) { "compensated" } else { "failed" };
            if self.journaling {
                self.compensation_completed_at = now();
                let _ = self.journal().await;
            }
            return RollbackOutcome::Clean;
        }
        self.status = "compensating";
        self.compensation_started_at = now();
        let _ = self.journal().await;
        let mut stuck = Vec::new();
        for index in pending {
            let (kind, resource, record) = {
                let step = &self.steps[index];
                (step.kind, step.resource.clone(), step.compensation.clone().unwrap_or(Value::Null))
            };
            match self.compensate_one(kind, &resource, &record).await {
                Ok(()) => {
                    self.steps[index].state = StepState::Compensated;
                    self.steps[index].compensated_at = now();
                }
                Err(error) => {
                    self.steps[index].state = StepState::Stuck;
                    self.steps[index].error = Some(format!("compensation failed: {}", error.code));
                    stuck.push(self.steps[index].name.clone());
                }
            }
        }
        if stuck.is_empty() {
            self.status = "compensated";
            self.compensation_completed_at = now();
        } else {
            self.status = "failed";
        }
        let _ = self.journal().await;
        if stuck.is_empty() {
            RollbackOutcome::Clean
        } else {
            RollbackOutcome::Failed { steps: stuck }
        }
    }

    /// The relational commit succeeded.
    pub async fn complete(&mut self) {
        self.status = "completed";
        self.completed_at = now();
        let _ = self.journal().await;
    }

    /// The relational commit's result is unknown: keep the effects and say so in the log.
    pub async fn mark_unknown(&mut self) {
        self.status = "unknown";
        let _ = self.journal().await;
    }

    pub fn transaction_id(&self) -> &str {
        &self.transaction_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{CapabilityFuture, EffectOutcome};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Script {
        events: Mutex<Vec<String>>,
        journal: Mutex<Vec<Value>>,
        fail_journal: Mutex<bool>,
        /// Journal writes still allowed before every further one fails (`None`: no limit).
        journal_budget: Mutex<Option<usize>>,
        fail_undo_of: Mutex<Option<u64>>,
        next: Mutex<u64>,
    }

    struct FakeMongo(Arc<Script>);
    struct FakeRedis(Arc<Script>);

    fn outcome(script: &Script, input: &Value) -> EffectOutcome {
        let mut next = script.next.lock().unwrap();
        *next += 1;
        let id = *next;
        let compensation = (input.get("compensate") == Some(&Value::Bool(true))).then(|| json!({"id": id}));
        EffectOutcome { value: json!({"id": id}), compensation }
    }

    impl MongoCapability for FakeMongo {
        fn call<'a>(&'a self, _: &'a CapabilityPrincipal, collection: &'a str, _: &'a Value) -> CapabilityFuture<'a, Result<Value, Reject>> {
            Box::pin(async move {
                self.0.events.lock().unwrap().push(format!("read {collection}"));
                Ok(json!({"docs": []}))
            })
        }
        fn write<'a>(
            &'a self,
            _: &'a CapabilityPrincipal,
            collection: &'a str,
            input: &'a Value,
        ) -> CapabilityFuture<'a, Result<EffectOutcome, Reject>> {
            Box::pin(async move {
                if input["command"] == "boom" {
                    return Err(reject("WIRE.ADAPTER_UNAVAILABLE", "mongo is down"));
                }
                let result = outcome(&self.0, input);
                self.0.events.lock().unwrap().push(format!("write {collection} {}", result.value["id"]));
                Ok(result)
            })
        }
        fn compensate<'a>(&'a self, _: &'a str, record: &'a Value) -> CapabilityFuture<'a, Result<(), Reject>> {
            Box::pin(async move {
                let id = record["id"].as_u64().unwrap();
                if *self.0.fail_undo_of.lock().unwrap() == Some(id) {
                    return Err(reject("WIRE.ADAPTER_UNAVAILABLE", "cannot undo"));
                }
                self.0.events.lock().unwrap().push(format!("undo {id}"));
                Ok(())
            })
        }
        fn journal<'a>(&'a self, record: &'a Value) -> CapabilityFuture<'a, Result<(), Reject>> {
            Box::pin(async move {
                let mut budget = self.0.journal_budget.lock().unwrap();
                if *self.0.fail_journal.lock().unwrap() || *budget == Some(0) {
                    return Err(reject("WIRE.ADAPTER_UNAVAILABLE", "journal down"));
                }
                if let Some(left) = budget.as_mut() {
                    *left -= 1;
                }
                self.0.journal.lock().unwrap().push(record.clone());
                Ok(())
            })
        }
    }

    impl RedisCapability for FakeRedis {
        fn call<'a>(&'a self, _: &'a CapabilityPrincipal, key: &'a str, _: &'a Value) -> CapabilityFuture<'a, Result<Value, Reject>> {
            Box::pin(async move {
                self.0.events.lock().unwrap().push(format!("read {key}"));
                Ok(json!({"value": null}))
            })
        }
        fn write<'a>(&'a self, _: &'a CapabilityPrincipal, key: &'a str, input: &'a Value) -> CapabilityFuture<'a, Result<EffectOutcome, Reject>> {
            Box::pin(async move {
                let result = outcome(&self.0, input);
                self.0.events.lock().unwrap().push(format!("write {key} {}", result.value["id"]));
                Ok(result)
            })
        }
        fn compensate<'a>(&'a self, _: &'a str, record: &'a Value) -> CapabilityFuture<'a, Result<(), Reject>> {
            Box::pin(async move {
                let id = record["id"].as_u64().unwrap();
                if *self.0.fail_undo_of.lock().unwrap() == Some(id) {
                    return Err(reject("WIRE.ADAPTER_UNAVAILABLE", "cannot undo"));
                }
                self.0.events.lock().unwrap().push(format!("undo {id}"));
                Ok(())
            })
        }
    }

    fn effects(script: &Arc<Script>) -> ExternalEffects {
        ExternalEffects::new(
            Some(Arc::new(FakeMongo(script.clone()))),
            Some(Arc::new(FakeRedis(script.clone()))),
            CapabilityPrincipal::default(),
            "tx-1".into(),
            "Video.track",
        )
    }

    fn events(script: &Script) -> Vec<String> {
        script.events.lock().unwrap().clone()
    }

    fn last_log(script: &Script) -> Value {
        script.journal.lock().unwrap().last().cloned().expect("a saga log was written")
    }

    #[tokio::test]
    async fn rollback_undoes_compensable_steps_newest_first_and_logs_each_stage() {
        let script = Arc::new(Script::default());
        let mut effects = effects(&script);
        effects.call("redis", "k:1", &json!({"command": "set_nx_ex", "compensate": true})).await.unwrap();
        effects.call("redis", "k:counter", &json!({"command": "incr_expire"})).await.unwrap();
        effects.call("mongo", "youtube_watching_data", &json!({"command": "insert_one", "compensate": true})).await.unwrap();
        assert_eq!(events(&script), ["write k:1 1", "write k:counter 2", "write youtube_watching_data 3"]);
        let in_flight = last_log(&script);
        assert_eq!((in_flight["status"].as_str(), in_flight["steps"].as_array().map(Vec::len)), (Some("in_progress"), Some(3)));
        assert_eq!(effects.rollback().await, RollbackOutcome::Clean);
        assert_eq!(events(&script)[3..], ["undo 3", "undo 1"], "newest first, and the counter has nothing to undo");
        let log = last_log(&script);
        assert_eq!(log["status"], "compensated");
        let statuses: Vec<&str> = log["steps"].as_array().unwrap().iter().map(|s| s["status"].as_str().unwrap()).collect();
        assert_eq!(statuses, ["compensated", "completed", "compensated"]);
        assert_eq!(log["transaction_id"], "tx-1");
        assert_eq!(log["metadata"]["extension"], "Video.track");
    }

    #[tokio::test]
    async fn a_read_runs_at_once_and_leaves_no_step_or_log_to_undo() {
        let script = Arc::new(Script::default());
        let mut effects = effects(&script);
        effects.call("mongo", "user_watch_stats", &json!({"command": "emotion_seconds"})).await.unwrap();
        effects.call("mongo", "youtube_watching_data", &json!({"command": "find"})).await.unwrap();
        effects.call("redis", "k:1", &json!({"command": "get"})).await.unwrap();
        // An unknown command is a write until proven otherwise: it is journaled as a step and refused by the capability.
        let unknown = effects.call("mongo", "youtube_watching_data", &json!({"command": "boom"})).await.unwrap_err();
        assert_eq!(unknown.code, "WIRE.ADAPTER_UNAVAILABLE");
        assert_eq!(events(&script), ["read user_watch_stats", "read youtube_watching_data", "read k:1"]);
        assert!(script.journal.lock().unwrap().is_empty(), "only a compensable write opens the saga log");
        assert_eq!(effects.rollback().await, RollbackOutcome::Clean);
    }

    #[tokio::test]
    async fn a_failing_undo_does_not_stop_the_others_and_the_log_names_it() {
        let script = Arc::new(Script::default());
        *script.fail_undo_of.lock().unwrap() = Some(2);
        let mut effects = effects(&script);
        for key in ["k:1", "k:2", "k:3"] {
            effects.call("redis", key, &json!({"command": "set_nx_ex", "compensate": true})).await.unwrap();
        }
        let outcome = effects.rollback().await;
        // ids 1, 2, 3 are written in that order; undo runs 3, 2 (fails), 1.
        assert_eq!(outcome, RollbackOutcome::Failed { steps: vec!["redis.k:2.set_nx_ex".into()] });
        assert_eq!(events(&script)[3..], ["undo 3", "undo 1"]);
        let log = last_log(&script);
        assert_eq!(log["status"], "failed");
        let steps = log["steps"].as_array().unwrap();
        assert_eq!(steps[1]["compensation_stuck"], true);
        assert!(steps[1]["error_message"].as_str().unwrap().contains("compensation failed"));
        assert_eq!((steps[0]["status"].as_str(), steps[2]["status"].as_str()), (Some("compensated"), Some("compensated")));
    }

    #[tokio::test]
    async fn without_a_saga_log_a_compensable_effect_does_not_happen() {
        let script = Arc::new(Script::default());
        *script.fail_journal.lock().unwrap() = true;
        let mut effects = effects(&script);
        let error = effects.call("redis", "k:1", &json!({"command": "set_nx_ex", "compensate": true})).await.unwrap_err();
        assert_eq!(error.code, "WIRE.ADAPTER_UNAVAILABLE");
        assert!(events(&script).is_empty(), "no effect without a log");
        assert!(effects.is_empty());
        // A call that asks for no way back needs no log (a frame or a counter), so it still runs.
        effects.call("redis", "k:counter", &json!({"command": "incr_expire"})).await.unwrap();
        assert_eq!(events(&script), ["write k:counter 1"]);
    }

    #[tokio::test]
    async fn an_effect_whose_log_entry_cannot_be_saved_is_undone_at_once() {
        let script = Arc::new(Script::default());
        let mut effects = effects(&script);
        effects.call("redis", "k:1", &json!({"command": "set_nx_ex", "compensate": true})).await.unwrap();
        // The log accepts the "about to write" entry of k:2 and then goes down, before it can record that the write happened.
        *script.journal_budget.lock().unwrap() = Some(1);
        let error = effects.call("redis", "k:2", &json!({"command": "set_nx_ex", "compensate": true})).await.unwrap_err();
        assert_eq!(error.code, "WIRE.ADAPTER_UNAVAILABLE");
        assert_eq!(events(&script), ["write k:1 1", "write k:2 2", "undo 2"], "the unlogged effect is undone immediately");
        *script.journal_budget.lock().unwrap() = None;
        assert_eq!(effects.rollback().await, RollbackOutcome::Clean);
        assert_eq!(events(&script)[3..], ["undo 1"]);
    }

    #[tokio::test]
    async fn an_unknown_commit_keeps_the_effects_and_says_so() {
        let script = Arc::new(Script::default());
        let mut effects = effects(&script);
        effects.call("redis", "k:1", &json!({"command": "set_nx_ex", "compensate": true})).await.unwrap();
        effects.mark_unknown().await;
        assert_eq!(events(&script), ["write k:1 1"], "nothing is undone");
        assert_eq!(last_log(&script)["status"], "unknown");
    }

    #[tokio::test]
    async fn a_completed_saga_and_a_failed_step_are_logged() {
        let script = Arc::new(Script::default());
        let mut effects = effects(&script);
        effects.call("redis", "k:1", &json!({"command": "set_nx_ex", "compensate": true})).await.unwrap();
        let failure = effects.call("mongo", "youtube_watching_data", &json!({"command": "boom", "compensate": true})).await.unwrap_err();
        assert_eq!(failure.code, "WIRE.ADAPTER_UNAVAILABLE");
        let log = last_log(&script);
        assert_eq!(log["steps"][1]["status"], "failed");
        assert_eq!(log["steps"][1]["error_message"], "WIRE.ADAPTER_UNAVAILABLE");
        effects.complete().await;
        assert_eq!(last_log(&script)["status"], "completed");
        assert!(last_log(&script)["completed_at"]["$date"].is_object());
    }

    #[tokio::test]
    async fn a_missing_capability_and_an_unknown_kind_are_refused() {
        let script = Arc::new(Script::default());
        let mut only_redis =
            ExternalEffects::new(None, Some(Arc::new(FakeRedis(script.clone()))), CapabilityPrincipal::default(), "tx-2".into(), "Video.track");
        assert_eq!(
            only_redis.call("mongo", "youtube_watching_data", &json!({"command": "insert_one"})).await.unwrap_err().code,
            "WIRE.ADAPTER_UNSUPPORTED"
        );
        assert_eq!(only_redis.call("sql", "x", &json!({})).await.unwrap_err().code, "WIRE.UNSUPPORTED");
        // Redis alone, even with a compensable call, has no log (the journal is Mongo's), and still compensates in memory.
        only_redis.call("redis", "k:1", &json!({"command": "set_nx_ex", "compensate": true})).await.unwrap();
        assert_eq!(only_redis.rollback().await, RollbackOutcome::Clean);
        assert_eq!(events(&script), ["write k:1 1", "undo 1"]);
        assert!(script.journal.lock().unwrap().is_empty());
    }
}
