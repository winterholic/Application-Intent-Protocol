use serde::Deserialize;
use serde_json::Value;
use spike_v6_transport::{
    hosting::{MountAccess, StaticMount},
    infrastructure::{Infrastructure, ObjectStorage},
    realtime::RealTimeOptions,
};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DbPoolConfig {
    pub max_size: usize,
    pub acquire_timeout_ms: u64,
    pub idle_timeout_ms: u64,
}
impl Default for DbPoolConfig {
    fn default() -> Self {
        Self { max_size: 16, acquire_timeout_ms: 2_000, idle_timeout_ms: 60_000 }
    }
}
impl DbPoolConfig {
    fn build(&self) -> Result<spike_v6_transport::pool::PoolOptions, Value> {
        if self.max_size > 1024 || self.acquire_timeout_ms > 30_000 || self.idle_timeout_ms > 3_600_000 {
            return Err(configuration_error());
        }
        let options = spike_v6_transport::pool::PoolOptions {
            max_size: self.max_size,
            acquire_timeout: Duration::from_millis(self.acquire_timeout_ms),
            idle_timeout: Duration::from_millis(self.idle_timeout_ms),
        };
        options.validate().map_err(|_| configuration_error())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimitConfig {
    pub limit: u32,
    pub window_ms: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MountConfig {
    prefix: String,
    directory: PathBuf,
    #[serde(default = "default_mount_timeout_ms")]
    timeout_ms: u64,
    #[serde(default = "default_mount_lifetime_ms")]
    max_lifetime_ms: u64,
    #[serde(default)]
    access: Access,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileMountConfig {
    prefix: String,
    bucket: String,
}
fn default_mount_timeout_ms() -> u64 {
    5_000
}
fn default_mount_lifetime_ms() -> u64 {
    15 * 60 * 1_000
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum Access {
    Public,
    #[default]
    Authenticated,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ObjectsConfig {
    Local {
        directory: PathBuf,
        #[serde(default = "max_object_bytes")]
        max_bytes: u64,
        #[serde(default = "owner_object_bytes")]
        max_total_bytes: u64,
        #[serde(default = "owner_object_count")]
        max_objects: usize,
    },
    S3 {
        endpoint: String,
        bucket: String,
        region: String,
        access_key_env: String,
        secret_key_env: String,
        session_token_env: Option<String>,
    },
    #[serde(rename = "gcs_hmac")]
    GcsHmac {
        bucket: String,
        access_key_env: String,
        secret_key_env: String,
    },
}
fn owner_object_bytes() -> u64 {
    10 * 1024 * 1024 * 1024
}
fn owner_object_count() -> usize {
    1000
}
fn max_object_bytes() -> u64 {
    5 * 1024 * 1024 * 1024
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RealtimeConfig {
    enabled: bool,
    max_connections: usize,
    max_lifetime_ms: u64,
    heartbeat_ms: u64,
    poll_ms: u64,
}
impl Default for RealtimeConfig {
    fn default() -> Self {
        Self { enabled: true, max_connections: 32, max_lifetime_ms: 900_000, heartbeat_ms: 15_000, poll_ms: 250 }
    }
}
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct MessagingConfig {
    pub enabled: bool,
    pub client_publish: bool,
    pub client_consume: bool,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ResourceLimitsConfig {
    pub connections: usize,
    pub workers: usize,
    pub jobs: usize,
    pub uploads: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerResourceLimitsConfig {
    pub memory_bytes: u64,
    pub pids_max: u32,
    pub cpu_quota_micros: u64,
    pub cpu_period_micros: u64,
}
impl WorkerResourceLimitsConfig {
    fn build(&self) -> Result<spike_v6_transport::server::WorkerResourceLimits, Value> {
        let limits = spike_v6_transport::server::WorkerResourceLimits {
            memory_bytes: self.memory_bytes,
            pids_max: self.pids_max,
            cpu_quota_micros: self.cpu_quota_micros,
            cpu_period_micros: self.cpu_period_micros,
        };
        limits.validate().map_err(|_| configuration_error())?;
        Ok(limits)
    }
}
impl Default for ResourceLimitsConfig {
    fn default() -> Self {
        let limits = spike_v6_transport::infrastructure::ResourceLimits::default();
        Self { connections: limits.connections, workers: limits.workers, jobs: limits.jobs, uploads: limits.uploads }
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkerIsolation {
    #[default]
    Auto,
    MacNetDeny,
    LinuxBubblewrap,
}
impl WorkerIsolation {
    pub fn runtime(&self) -> spike_v6_transport::server::WorkerIsolation {
        use spike_v6_transport::server::WorkerIsolation as I;
        match self {
            Self::Auto => I::Auto,
            Self::MacNetDeny => I::MacNetDeny,
            Self::LinuxBubblewrap => I::LinuxBubblewrap,
        }
    }
}

fn configuration_error() -> Value {
    super::error("INFRASTRUCTURE_CONFIG", "저장소·마운트·실시간 또는 worker 설정 오류")
}
fn credential(name: &str, lookup: &impl Fn(&str) -> Option<String>) -> Result<String, Value> {
    if !super::valid_env_name(name) {
        return Err(configuration_error());
    }
    lookup(name).filter(|value| !value.is_empty()).ok_or_else(configuration_error)
}

pub fn build(config: &super::Config) -> Result<Infrastructure, Value> {
    build_with_environment(config, &|name| std::env::var(name).ok())
}

pub(super) fn build_with_environment(config: &super::Config, lookup: &impl Fn(&str) -> Option<String>) -> Result<Infrastructure, Value> {
    let mounts = config
        .static_mounts
        .iter()
        .map(|m| {
            StaticMount::new(
                m.prefix.clone(),
                m.directory.clone(),
                match m.access {
                    Access::Public => MountAccess::Public,
                    Access::Authenticated => MountAccess::Authenticated,
                },
            )
            .and_then(|mount| mount.with_timeouts(Duration::from_millis(m.timeout_ms), Duration::from_millis(m.max_lifetime_ms)))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| configuration_error())?;
    let file_mounts = config
        .file_mounts
        .iter()
        .map(|mount| spike_v6_transport::hosting::ManagedFileMount::new(mount.prefix.clone(), mount.bucket.clone()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| configuration_error())?;
    let objects = match &config.objects {
        None => None,
        Some(ObjectsConfig::Local { directory, max_bytes, max_total_bytes, max_objects }) => {
            if *max_bytes == 0 || *max_bytes > 5 * 1024 * 1024 * 1024 * 1024 || *max_total_bytes == 0 || *max_objects == 0 || *max_objects > 100_000 {
                return Err(configuration_error());
            }
            Some(ObjectStorage::Local(Arc::new(
                aip_execution::objects::LocalStore::new(
                    aip_execution::objects::LocalConfig::new(directory).with_max_bytes(*max_bytes).with_owner_quota(*max_total_bytes, *max_objects),
                )
                .map_err(|_| configuration_error())?,
            )))
        }
        Some(ObjectsConfig::S3 { endpoint, bucket, region, access_key_env, secret_key_env, session_token_env }) => {
            let mut s = aip_execution::objects::S3Config::new(
                endpoint,
                bucket,
                region,
                credential(access_key_env, lookup)?,
                credential(secret_key_env, lookup)?,
            );
            if let Some(name) = session_token_env {
                s = s.with_session_token(credential(name, lookup)?);
            }
            Some(ObjectStorage::S3(Arc::new(aip_execution::objects::S3Store::new(s).map_err(|_| configuration_error())?)))
        }
        Some(ObjectsConfig::GcsHmac { bucket, access_key_env, secret_key_env }) => {
            let config = aip_execution::gcs::GcsConfig::new(bucket, credential(access_key_env, lookup)?, credential(secret_key_env, lookup)?);
            Some(ObjectStorage::GcsHmac(Arc::new(aip_execution::gcs::GcsStore::new(config).map_err(|_| configuration_error())?)))
        }
    };
    let r = &config.realtime;
    let realtime = RealTimeOptions {
        enabled: r.enabled,
        max_connections: r.max_connections,
        max_lifetime: Duration::from_millis(r.max_lifetime_ms),
        heartbeat_interval: Duration::from_millis(r.heartbeat_ms),
        poll_interval: Duration::from_millis(r.poll_ms),
    };
    let worker_launch = match &config.workers {
        Some(w) => {
            if w.environment.len() > 32
                || w.environment.iter().any(|(name, value)| !super::valid_env_name(name) || value.len() > 8192 || value.contains('\0'))
            {
                return Err(configuration_error());
            }
            spike_v6_transport::server::WorkerLaunchConfig {
                isolation: w.isolation.runtime(),
                runtime_executable: w.runtime_executable.clone(),
                environment: w.environment.clone(),
                read_only_paths: w.read_only_paths.clone(),
                cgroup_root: w.cgroup_root.clone(),
                resource_limits: match (&w.cgroup_root, &w.resource_limits) {
                    (None, None) => None,
                    (Some(root), Some(limits)) if root.is_absolute() => Some(limits.build()?),
                    _ => return Err(configuration_error()),
                },
            }
        }
        None => spike_v6_transport::server::WorkerLaunchConfig::default(),
    };
    let resource_limits = config.limits.as_ref().map_or_else(spike_v6_transport::infrastructure::ResourceLimits::default, |limits| {
        spike_v6_transport::infrastructure::ResourceLimits {
            connections: limits.connections,
            workers: limits.workers,
            jobs: limits.jobs,
            uploads: limits.uploads,
        }
    });
    let db_pool = config.db_pool.build()?;
    let rate_limit = config
        .rate_limit
        .as_ref()
        .map(|policy| {
            aip_execution::rate_limit::Policy { limit: policy.limit, window: Duration::from_millis(policy.window_ms) }
                .validate()
                .map_err(|_| configuration_error())
        })
        .transpose()?;
    let result = Infrastructure {
        db_pool,
        rate_limit,
        mounts,
        file_mounts,
        objects,
        realtime,
        worker_launch,
        messaging: config.messaging.enabled,
        client_publish: config.messaging.client_publish,
        client_consume: config.messaging.client_consume,
        resource_limits,
        object_transfer_lifetime: Duration::from_millis(config.object_transfer_lifetime_ms),
        rag: None,
        rag_client_index: false,
    };
    result.validate().map_err(|_| configuration_error())?;
    Ok(result)
}

pub fn empty_environment() -> HashMap<String, String> {
    HashMap::new()
}
