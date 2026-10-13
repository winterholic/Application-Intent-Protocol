use std::sync::Arc;

fn prefixes_overlap(left: &str, right: &str) -> bool {
    left == right
        || left.strip_prefix(right).is_some_and(|rest| rest.starts_with('/'))
        || right.strip_prefix(left).is_some_and(|rest| rest.starts_with('/'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceLimits {
    pub connections: usize,
    pub workers: usize,
    pub jobs: usize,
    pub uploads: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self { connections: 64, workers: 4, jobs: 2, uploads: 4 }
    }
}

impl ResourceLimits {
    pub fn realtime_capacity(&self, requested: usize) -> usize {
        let available = self.connections.saturating_sub(1);
        usize::min(requested, available)
    }

    pub fn validate(&self) -> std::io::Result<()> {
        if self.connections == 0
            || self.connections > 1024
            || self.workers == 0
            || self.workers > 32
            || self.jobs == 0
            || self.jobs > 32
            || self.uploads == 0
            || self.uploads > 32
        {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid resource limits"));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub enum ObjectStorage {
    Local(Arc<aip_execution::objects::LocalStore>),
    S3(Arc<aip_execution::objects::S3Store>),
    GcsHmac(Arc<aip_execution::gcs::GcsStore>),
}

/// The signal a committed write gives to the data subscriptions of this process. It carries no data: a subscription
/// re-runs its read as its own subscriber, which is the only way to know what that subscriber may now see.
#[derive(Clone)]
pub struct DataChanges(Arc<tokio::sync::watch::Sender<u64>>);

impl Default for DataChanges {
    fn default() -> Self {
        Self(Arc::new(tokio::sync::watch::channel(0).0))
    }
}

impl DataChanges {
    /// Called after a commit that changed business data (a transition from `/apply`, a write extension or a job).
    pub fn notify(&self) {
        self.0.send_modify(|count| *count = count.wrapping_add(1));
    }

    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.0.subscribe()
    }
}

#[derive(Clone)]
pub struct Infrastructure {
    pub db_pool: crate::pool::PoolOptions,
    pub rate_limit: Option<aip_execution::rate_limit::Policy>,
    pub mounts: Vec<crate::hosting::StaticMount>,
    pub file_mounts: Vec<crate::hosting::ManagedFileMount>,
    pub objects: Option<ObjectStorage>,
    pub realtime: crate::realtime::RealTimeOptions,
    pub worker_launch: spike_v4_worker::WorkerLaunchConfig,
    pub messaging: bool,
    pub client_publish: bool,
    pub client_consume: bool,
    pub resource_limits: ResourceLimits,
    pub object_transfer_lifetime: std::time::Duration,
    pub rag: Option<Arc<aip_execution::rag::RagEngine>>,
    pub rag_client_index: bool,
    pub triggers: Arc<crate::triggers::Triggers>,
    /// Retry policy by job operation name. Jobs without an entry use `JobPolicy::default()`.
    pub job_policies: std::collections::HashMap<String, crate::triggers::JobPolicy>,
    /// Bearer token for `GET /metrics`. Without one the path is not served.
    pub metrics_token: Option<String>,
    /// Wakes data subscriptions after a commit; see [`DataChanges`].
    pub data_changes: DataChanges,
}

impl Default for Infrastructure {
    fn default() -> Self {
        Self {
            db_pool: crate::pool::PoolOptions::default(),
            rate_limit: None,
            mounts: Vec::new(),
            file_mounts: Vec::new(),
            objects: None,
            realtime: crate::realtime::RealTimeOptions::default(),
            worker_launch: spike_v4_worker::WorkerLaunchConfig::default(),
            messaging: false,
            client_publish: false,
            client_consume: false,
            resource_limits: ResourceLimits::default(),
            object_transfer_lifetime: std::time::Duration::from_secs(15 * 60),
            rag: None,
            rag_client_index: false,
            triggers: Arc::default(),
            job_policies: Default::default(),
            metrics_token: None,
            data_changes: DataChanges::default(),
        }
    }
}

impl Infrastructure {
    pub fn catalog(&self) -> serde_json::Value {
        use serde_json::json;
        let backend = self.objects.as_ref().map(|store| match store {
            ObjectStorage::Local(_) => "local",
            ObjectStorage::S3(_) => "s3",
            ObjectStorage::GcsHmac(_) => "gcs_hmac",
        });
        let static_mounts: Vec<_> = self.mounts.iter().map(|mount| json!({"prefix":mount.prefix,"access":match mount.access {crate::hosting::MountAccess::Public=>"public", crate::hosting::MountAccess::Authenticated=>"authenticated"}})).collect();
        let file_mounts: Vec<_> =
            self.file_mounts.iter().map(|mount| json!({"prefix":mount.prefix,"bucket":mount.bucket,"access":"owner"})).collect();
        json!({
            "version":"aip-infrastructure/1", "staticMounts":static_mounts, "fileMounts":file_mounts,
            "objects":{"backend":backend,"transferLifetimeMs":self.object_transfer_lifetime.as_millis()},
            "events":{"polling":true,"sse":self.realtime.enabled,"websocket":self.realtime.enabled,"dataSubscriptions":self.realtime.enabled,"maxConnections":self.resource_limits.realtime_capacity(self.realtime.max_connections)},
            "messages":{"enabled":self.messaging,"clientPublish":self.client_publish,"clientConsume":self.client_consume,"delivery":"at-least-once"},
            "limits":{"connections":self.resource_limits.connections,"workers":self.resource_limits.workers,"jobs":self.resource_limits.jobs,"uploads":self.resource_limits.uploads},
            "rag":{"enabled":self.rag.is_some(),"clientIndex":self.rag.is_some() && self.rag_client_index}
        })
    }

    pub fn validate(&self) -> std::io::Result<()> {
        self.db_pool.validate().map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
        if let Some(policy) = self.rate_limit {
            policy.validate().map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
        }
        self.realtime.validate()?;
        self.resource_limits.validate()?;
        if !(std::time::Duration::from_secs(1)..=std::time::Duration::from_secs(24 * 60 * 60)).contains(&self.object_transfer_lifetime) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid object transfer lifetime"));
        }
        for (index, mount) in self.mounts.iter().enumerate() {
            if self.mounts[..index].iter().any(|other| prefixes_overlap(&other.prefix, &mount.prefix))
                || [
                    "/session",
                    "/read",
                    "/apply",
                    "/status",
                    "/extension",
                    "/operation",
                    "/capabilities",
                    "/jobs",
                    "/files",
                    "/events",
                    "/objects",
                    "/messages",
                    "/rag",
                ]
                .iter()
                .any(|route| prefixes_overlap(&mount.prefix, route))
            {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "overlapping or reserved static mount"));
            }
        }
        for (index, mount) in self.file_mounts.iter().enumerate() {
            mount.validate()?;
            if self.file_mounts[..index].iter().any(|other| prefixes_overlap(&other.prefix, &mount.prefix))
                || self.mounts.iter().any(|other| prefixes_overlap(&other.prefix, &mount.prefix))
                || [
                    "/session",
                    "/read",
                    "/apply",
                    "/status",
                    "/extension",
                    "/operation",
                    "/capabilities",
                    "/jobs",
                    "/files",
                    "/events",
                    "/objects",
                    "/messages",
                    "/rag",
                ]
                .iter()
                .any(|route| prefixes_overlap(&mount.prefix, route))
            {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "overlapping or reserved managed file mount"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod object_transfer_lifetime_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn object_transfer_lifetime_has_a_bounded_default_and_valid_range() {
        let defaults = Infrastructure::default();
        assert_eq!(defaults.object_transfer_lifetime, Duration::from_secs(900));
        assert!(defaults.validate().is_ok());

        let mut config = defaults.clone();
        config.object_transfer_lifetime = Duration::from_millis(999);
        assert!(config.validate().is_err());
        config.object_transfer_lifetime = Duration::from_secs(1);
        assert!(config.validate().is_ok());
        config.object_transfer_lifetime = Duration::from_secs(24 * 60 * 60);
        assert!(config.validate().is_ok());
        config.object_transfer_lifetime += Duration::from_millis(1);
        assert!(config.validate().is_err());
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;
    #[test]
    fn catalog_describes_runtime_choices_without_host_paths_or_environment() {
        let mut infrastructure = Infrastructure::default();
        infrastructure.worker_launch.environment.insert("PRIVATE_VALUE".into(), "do-not-publish".into());
        let catalog = infrastructure.catalog();
        assert_eq!(catalog["version"], "aip-infrastructure/1");
        assert_eq!(catalog["limits"]["connections"], 64);
        assert_eq!(catalog["objects"]["backend"], serde_json::Value::Null);
        assert!(!catalog.to_string().contains("do-not-publish"));
        infrastructure.realtime.enabled = false;
        assert_eq!(infrastructure.catalog()["events"]["sse"], false);
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    #[test]
    fn realtime_admission_leaves_a_connection_for_regular_requests() {
        let limits = ResourceLimits { connections: 3, ..Default::default() };
        assert_eq!(limits.realtime_capacity(32), 2);
        assert_eq!(limits.realtime_capacity(1), 1);
        assert_eq!(ResourceLimits { connections: 1, ..limits }.realtime_capacity(32), 0);
    }
}
