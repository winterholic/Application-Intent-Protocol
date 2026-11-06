//! Encryption runtime assembly and operational rekey service runner.
//!
//! Connects `aip-crypto` KEK providers and `aip-execution` sealer/opener/rekey boundaries
//! to the service configuration and CLI entrypoints.
//!
//! Fail-closed: missing or invalid keys refuse execution. Debug and logging never echo key material.

use aip_crypto::provider::{KekProvider, LocalProvider};
use aip_execution::{
    encryption::{DekStore, Scope},
    read_opener::StoreOpener,
    rekey::{RekeyReport, RekeyTarget, rekey_scope},
    write_sealer::StoreSealer,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio_postgres::Client;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidConfig(String),
    Crypto(String),
    Database(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(msg) => write!(f, "설정 오류: {msg}"),
            Self::Crypto(msg) => write!(f, "암호화 오류: {msg}"),
            Self::Database(msg) => write!(f, "데이터베이스 오류: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// KEK provider configuration from service.json.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KekConfig {
    /// Load from an environment variable (0400/0600 or inline spec).
    Env { name: String, active: String },
    /// Load from a protected file (0400/0600, no symlinks).
    File { path: PathBuf, active: String },
    /// Static spec for testing/fixtures only.
    Spec { spec: String, active: String },
}

impl KekConfig {
    /// Loads and initializes the configured KEK provider without leaking keys in errors.
    pub fn load_provider(&self) -> Result<LocalProvider, Error> {
        match self {
            Self::Env { name, active } => LocalProvider::from_env(name, active).map_err(|_| Error::Crypto("환경 변수에서 KEK를 읽을 수 없음".into())),
            Self::File { path, active } => LocalProvider::from_file(path, active)
                .map_err(|_| Error::Crypto(format!("키 파일({})을 읽을 수 없음 (권한 0400/0600 확인 필요)", path.display()))),
            Self::Spec { spec, active } => LocalProvider::from_spec(spec, active).map_err(|_| Error::Crypto("KEK spec 형식이 잘못됨".into())),
        }
    }
}

/// Binds `StoreSealer` and `StoreOpener` for a deployment schema using the loaded provider.
pub fn bind_sealer_and_opener(store: DekStore, provider: LocalProvider, budget_client: Client) -> (StoreSealer, StoreOpener) {
    let sealer = StoreSealer::new(store.clone(), provider.clone(), budget_client);
    let opener = StoreOpener::new(store, provider);
    (sealer, opener)
}

/// Rekey operational parameters parsed from CLI or management worker.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RekeyOptions {
    pub scope_kind: String,
    pub scope_id: String,
    pub holder_id: String,
    pub targets: Vec<RekeyTargetSpec>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RekeyTargetSpec {
    pub table: String,
    pub column: String,
    pub id_column: String,
    pub resource: String,
    pub field: String,
}

/// Executes durable rekey cycle for the given scope and targets.
pub async fn execute_rekey(
    db: &mut Client,
    budget: &mut Client,
    schema: &str,
    kek_config: &KekConfig,
    options: &RekeyOptions,
) -> Result<Vec<RekeyReport>, Error> {
    let store = DekStore::new(schema).map_err(|e| Error::InvalidConfig(format!("schema `{schema}`가 유효하지 않음: {e:?}")))?;
    let provider = kek_config.load_provider()?;

    if options.targets.is_empty() {
        return Err(Error::InvalidConfig("rekey target 목록이 비어 있음".into()));
    }

    let scope = match options.scope_kind.as_str() {
        "service" => Scope::service(),
        "ref" => {
            Scope::reference(&options.targets[0].resource, &options.scope_id).map_err(|e| Error::InvalidConfig(format!("ref scope 오류: {e:?}")))?
        }
        "owner" => {
            Scope::owner(&options.targets[0].resource, &options.scope_id).map_err(|e| Error::InvalidConfig(format!("owner scope 오류: {e:?}")))?
        }
        _ => return Err(Error::InvalidConfig(format!("알 수 없는 scope_kind: {}", options.scope_kind))),
    };

    let targets: Vec<RekeyTarget> =
        options.targets.iter().map(|t| RekeyTarget::new(&t.table, &t.column, &t.id_column, &t.resource, &t.field, scope.clone())).collect();

    rekey_scope(db, budget, &store, &scope, &targets, &options.holder_id, &provider)
        .await
        .map_err(|e| Error::Database(format!("rekey 실행 실패: {e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kek_config_deserializes_from_json() {
        let json = r#"{"kind":"spec","spec":"k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=","active":"k1"}"#;
        let config: KekConfig = serde_json::from_str(json).unwrap();
        let provider = config.load_provider().unwrap();
        assert!(format!("{provider:?}").contains("LocalKekProvider"));
        assert!(!format!("{provider:?}").contains("BwcH"), "Debug output must not leak key material");
    }

    #[test]
    fn invalid_kek_config_fails_closed_without_leaks() {
        let config = KekConfig::Spec { spec: "invalid".into(), active: "k1".into() };
        let err = config.load_provider().unwrap_err();
        assert!(matches!(err, Error::Crypto(_)));
    }
}
