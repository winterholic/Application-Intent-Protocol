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
#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum KekConfig {
    /// Load from an environment variable (0400/0600 or inline spec).
    Env { name: String, active: String },
    /// Load from a protected file (0400/0600, no symlinks).
    File { path: PathBuf, active: String },
    /// Static spec for testing/fixtures only.
    Spec { spec: String, active: String },
}

impl std::fmt::Debug for KekConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Env { name, active } => f.debug_struct("KekConfig::Env").field("name", name).field("active", active).finish(),
            Self::File { path, active } => f.debug_struct("KekConfig::File").field("path", path).field("active", active).finish(),
            Self::Spec { active, .. } => f.debug_struct("KekConfig::Spec").field("spec", &"[REDACTED]").field("active", active).finish(),
        }
    }
}

impl Serialize for KekConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        match self {
            Self::Env { name, active } => {
                let mut s = serializer.serialize_struct("KekConfig", 3)?;
                s.serialize_field("kind", "env")?;
                s.serialize_field("name", name)?;
                s.serialize_field("active", active)?;
                s.end()
            }
            Self::File { path, active } => {
                let mut s = serializer.serialize_struct("KekConfig", 3)?;
                s.serialize_field("kind", "file")?;
                s.serialize_field("path", path)?;
                s.serialize_field("active", active)?;
                s.end()
            }
            Self::Spec { active, .. } => {
                let mut s = serializer.serialize_struct("KekConfig", 3)?;
                s.serialize_field("kind", "spec")?;
                s.serialize_field("spec", "[REDACTED]")?;
                s.serialize_field("active", active)?;
                s.end()
            }
        }
    }
}

impl KekConfig {
    /// Validates KEK configuration fields before loading any provider.
    pub fn validate(&self) -> Result<(), Error> {
        match self {
            Self::Env { name, active } => {
                if name.trim().is_empty() {
                    return Err(Error::InvalidConfig("KEK env 이름이 비어 있음".into()));
                }
                if active.trim().is_empty() {
                    return Err(Error::InvalidConfig("active 키 식별자가 비어 있음".into()));
                }
            }
            Self::File { path, active } => {
                if path.as_os_str().is_empty() {
                    return Err(Error::InvalidConfig("KEK 파일 경로가 비어 있음".into()));
                }
                if active.trim().is_empty() {
                    return Err(Error::InvalidConfig("active 키 식별자가 비어 있음".into()));
                }
            }
            Self::Spec { spec, active } => {
                if spec.trim().is_empty() {
                    return Err(Error::InvalidConfig("KEK spec이 비어 있음".into()));
                }
                if active.trim().is_empty() {
                    return Err(Error::InvalidConfig("active 키 식별자가 비어 있음".into()));
                }
            }
        }
        Ok(())
    }

    /// Loads and initializes the configured KEK provider without leaking keys in errors.
    pub fn load_provider(&self) -> Result<LocalProvider, Error> {
        self.validate()?;
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
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RekeyOptions {
    pub scope_kind: String,
    pub scope_id: String,
    pub holder_id: String,
    pub targets: Vec<RekeyTargetSpec>,
    /// Explicit assertion/proof of full inventory coverage across the scope.
    /// Partial rekeying must never retire the scope's readable DEK.
    #[serde(default)]
    pub inventory_complete: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RekeyTargetSpec {
    pub table: String,
    pub column: String,
    pub id_column: String,
    pub resource: String,
    pub field: String,
}

impl RekeyOptions {
    /// Validates rekey parameters before any database connections or locks are acquired.
    pub fn validate(&self) -> Result<Scope, Error> {
        if self.holder_id.trim().is_empty() {
            return Err(Error::InvalidConfig("holder_id 누락".into()));
        }
        if self.targets.is_empty() {
            return Err(Error::InvalidConfig("rekey target 목록이 비어 있음".into()));
        }
        for (i, t) in self.targets.iter().enumerate() {
            if t.table.trim().is_empty()
                || t.column.trim().is_empty()
                || t.id_column.trim().is_empty()
                || t.resource.trim().is_empty()
                || t.field.trim().is_empty()
            {
                return Err(Error::InvalidConfig(format!("target[{i}] 필드 누락: 모든 필드는 필수임")));
            }
        }
        let scope = match self.scope_kind.as_str() {
            "service" => {
                if !self.scope_id.is_empty() && self.scope_id != "global" {
                    return Err(Error::InvalidConfig("service scope_id는 비어 있거나 'global'이어야 함".into()));
                }
                Scope::service()
            }
            "ref" => {
                if self.scope_id.trim().is_empty() {
                    return Err(Error::InvalidConfig("ref scope_id 누락".into()));
                }
                Scope::reference(&self.targets[0].resource, &self.scope_id).map_err(|e| Error::InvalidConfig(format!("ref scope 오류: {e:?}")))?
            }
            "owner" => {
                if self.scope_id.trim().is_empty() {
                    return Err(Error::InvalidConfig("owner scope_id 누락".into()));
                }
                Scope::owner(&self.targets[0].resource, &self.scope_id).map_err(|e| Error::InvalidConfig(format!("owner scope 오류: {e:?}")))?
            }
            _ => return Err(Error::InvalidConfig(format!("알 수 없는 scope_kind: {}", self.scope_kind))),
        };
        Ok(scope)
    }

    /// Verifies that the targets represent a verified complete inventory before allowing rekey retirement.
    pub fn assert_complete_inventory(&self) -> Result<(), Error> {
        if !self.inventory_complete {
            return Err(Error::InvalidConfig(
                "신뢰할 수 있는 전체 대상 인벤토리 보증(inventory_complete) 누락: 부분 재암호화 후 DEK 전체 폐기를 방지하기 위해 거부함".into(),
            ));
        }
        Ok(())
    }
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
    kek_config.validate()?;
    let provider = kek_config.load_provider()?;

    // 1. Structural validation before touching DB
    let scope = options.validate()?;

    // 2. Prevent partial target slice from retiring DEK across the whole scope
    options.assert_complete_inventory()?;

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
