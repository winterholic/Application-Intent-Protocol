//! Service configuration for schedules and inbound webhooks.
//!
//! Both run as an operator-bound principal named by `principal` (the subject registered with
//! `aip service principal bind`) under the configured token issuer. Secrets come only from the named
//! process environment variable.
use serde::Deserialize;
use serde_json::Value;
use spike_v6_transport::triggers::{
    JobPolicy, MAX_MISFIRE_GRACE, MIN_MISFIRE_GRACE, OutboxHandler, Schedule, ServicePrincipal, Signature, Triggers, Webhook, decode_secret,
};
use std::time::Duration;

fn default_grace_ms() -> u64 {
    60_000
}
fn default_timezone() -> String {
    "UTC".into()
}
fn default_body() -> usize {
    256 * 1024
}
fn default_tolerance() -> u64 {
    300
}
fn empty_input() -> Value {
    Value::Object(Default::default())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleConfig {
    pub name: String,
    pub operation: String,
    pub cron: String,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default = "empty_input")]
    pub input: Value,
    pub principal: String,
    #[serde(default = "default_grace_ms")]
    pub misfire_grace_ms: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboxHandlerConfig {
    pub topic: String,
    pub operation: String,
    pub principal: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobPolicyConfig {
    pub operation: String,
    pub max_attempts: i32,
    #[serde(default)]
    pub retry_uncaught_errors: bool,
}

/// Duplicate entries are rejected so one operation never has two silently merged policies.
pub fn build_job_policies(configs: &[JobPolicyConfig]) -> Result<std::collections::HashMap<String, JobPolicy>, &'static str> {
    let mut policies = std::collections::HashMap::new();
    for config in configs {
        let policy = JobPolicy { max_attempts: config.max_attempts, retry_uncaught: config.retry_uncaught_errors };
        if policies.insert(config.operation.clone(), policy).is_some() {
            return Err("job_policies must name each operation once");
        }
    }
    Ok(policies)
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SignatureConfig {
    HmacSha256 {
        header: String,
        #[serde(default)]
        prefix: String,
        #[serde(default)]
        base64: bool,
    },
    Stripe {
        #[serde(default = "default_tolerance")]
        tolerance_seconds: u64,
    },
    StandardWebhooks {
        #[serde(default = "default_tolerance")]
        tolerance_seconds: u64,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    pub name: String,
    pub path: String,
    pub operation: String,
    pub principal: String,
    pub secret_env: String,
    pub signature: SignatureConfig,
    #[serde(default = "default_body")]
    pub max_body_bytes: usize,
}

fn header_name(name: &str) -> Option<String> {
    let name = name.to_ascii_lowercase();
    (!name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')).then_some(name)
}

pub fn build(
    schedules: &[ScheduleConfig],
    webhooks: &[WebhookConfig],
    outbox: &[OutboxHandlerConfig],
    issuer: &str,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Result<Triggers, &'static str> {
    let principal = |subject: &str| ServicePrincipal { issuer: issuer.to_string(), subject: subject.to_string() };
    let mut triggers = Triggers::default();
    for schedule in schedules {
        let misfire_grace = Duration::from_millis(schedule.misfire_grace_ms);
        if misfire_grace < MIN_MISFIRE_GRACE || misfire_grace > MAX_MISFIRE_GRACE {
            return Err("schedule misfire_grace_ms must be between 10 seconds and 24 hours");
        }
        triggers.schedules.push(Schedule {
            name: schedule.name.clone(),
            operation: schedule.operation.clone(),
            cron: spike_v6_transport::schedule::Cron::parse(&schedule.cron, &schedule.timezone)
                .map_err(|_| "schedule cron or timezone is invalid or never fires")?,
            input: schedule.input.clone(),
            principal: principal(&schedule.principal),
            misfire_grace,
        });
    }
    for webhook in webhooks {
        let signature = match &webhook.signature {
            SignatureConfig::HmacSha256 { header, prefix, base64 } => Signature::HmacSha256 {
                header: header_name(header).ok_or("webhook signature header is invalid")?,
                prefix: prefix.clone(),
                base64: *base64,
            },
            SignatureConfig::Stripe { tolerance_seconds } => Signature::Stripe { tolerance: Duration::from_secs(*tolerance_seconds) },
            SignatureConfig::StandardWebhooks { tolerance_seconds } => {
                Signature::StandardWebhooks { tolerance: Duration::from_secs(*tolerance_seconds) }
            }
        };
        let raw = environment(&webhook.secret_env).ok_or("webhook secret environment variable is missing")?;
        let secret = decode_secret(&signature, &raw).ok_or("webhook secret has the wrong format")?;
        triggers.webhooks.push(Webhook {
            name: webhook.name.clone(),
            path: webhook.path.clone(),
            operation: webhook.operation.clone(),
            principal: principal(&webhook.principal),
            secret,
            signature,
            max_body_bytes: webhook.max_body_bytes,
        });
    }
    for handler in outbox {
        triggers.outbox.push(OutboxHandler {
            topic: handler.topic.clone(),
            operation: handler.operation.clone(),
            principal: principal(&handler.principal),
        });
    }
    Ok(triggers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_policies_name_each_operation_once_and_default_to_retrying_only_on_request() {
        let configs: Vec<JobPolicyConfig> = serde_json::from_value(serde_json::json!([{"operation":"sync","max_attempts":5}])).expect("config");
        assert_eq!(build_job_policies(&configs).expect("policy")["sync"], JobPolicy { max_attempts: 5, retry_uncaught: false });
        let twice: Vec<JobPolicyConfig> =
            serde_json::from_value(serde_json::json!([{"operation":"sync","max_attempts":5},{"operation":"sync","max_attempts":2}])).expect("config");
        assert!(build_job_policies(&twice).is_err());
        assert!(serde_json::from_value::<Vec<JobPolicyConfig>>(serde_json::json!([{"operation":"sync","max_attempts":2,"backoff":1}])).is_err());
    }

    #[test]
    fn reads_secrets_only_from_the_named_variable_and_rejects_bad_values() {
        let webhooks: Vec<WebhookConfig> = serde_json::from_value(serde_json::json!([{
            "name":"github","path":"/hooks/github","operation":"receive","principal":"svc","secret_env":"HOOK_SECRET",
            "signature":{"kind":"hmac-sha256","header":"X-Hub-Signature-256","prefix":"sha256="}
        }]))
        .expect("config");
        let env = |name: &str| (name == "HOOK_SECRET").then(|| "0123456789abcdef0123".to_string());
        let built = build(&[], &webhooks, &[], "issuer", &env).expect("triggers");
        assert_eq!(built.webhooks[0].secret, b"0123456789abcdef0123");
        assert!(matches!(&built.webhooks[0].signature, Signature::HmacSha256 { header, .. } if header == "x-hub-signature-256"));
        assert!(build(&[], &webhooks, &[], "issuer", &|_| None).is_err());

        let standard: Vec<WebhookConfig> = serde_json::from_value(serde_json::json!([{
            "name":"svix","path":"/hooks/svix","operation":"receive","principal":"svc","secret_env":"S",
            "signature":{"kind":"standard-webhooks"}
        }]))
        .expect("config");
        assert!(build(&[], &standard, &[], "issuer", &|_| Some("not-whsec".into())).is_err());

        let schedules: Vec<ScheduleConfig> = serde_json::from_value(serde_json::json!([
            {"name":"nightly","operation":"sweep","cron":"0 3 * * *","timezone":"Asia/Seoul","principal":"svc"}
        ]))
        .expect("config");
        assert_eq!(build(&schedules, &[], &[], "issuer", &|_| None).expect("schedule").schedules[0].misfire_grace, Duration::from_secs(60));
        let bad: Vec<ScheduleConfig> =
            serde_json::from_value(serde_json::json!([{"name":"x","operation":"sweep","cron":"0 3 * *","principal":"svc"}])).expect("config");
        assert!(build(&bad, &[], &[], "issuer", &|_| None).is_err());
        assert!(
            serde_json::from_value::<Vec<ScheduleConfig>>(
                serde_json::json!([{"name":"x","operation":"y","cron":"@daily","principal":"p","extra":1}])
            )
            .is_err()
        );
    }
}
