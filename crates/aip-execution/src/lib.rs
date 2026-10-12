mod jobs;

pub use jobs::{Error, JobResult, Lease, NewRun, Run, Store, ddl};

pub mod files;

pub mod events;

pub mod integrations;

pub mod objects;

pub mod messages;

#[cfg(feature = "external-brokers")]
pub mod brokers;

pub mod gcs;
pub mod rag;
pub mod rate_limit;

/// Runtime storage is kept outside the application catalog managed by migrations.
pub fn infrastructure_schema(deployment_schema: &str) -> Result<String, &'static str> {
    use sha2::{Digest, Sha256};
    if !deployment_schema.starts_with("aip_")
        || deployment_schema.len() > 63
        || !deployment_schema.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err("invalid deployment schema");
    }
    let hash = format!("{:x}", Sha256::digest(deployment_schema.as_bytes()));
    Ok(format!("aip_infra_{}", &hash[..52]))
}
