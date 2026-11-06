//! Encryption runtime assembly and operational rekey service runner.
//!
//! Connects `aip-crypto` KEK providers and `aip-execution` sealer/opener/rekey boundaries
//! to the service configuration and CLI entrypoints.
//!
//! Fail-closed: missing or invalid keys refuse execution. Debug and logging never echo key material.

use aip_crypto::provider::LocalProvider;
use aip_execution::{
    encryption::{DekStore, Scope},
    read_opener::StoreOpener,
    rekey::{RekeyReport, RekeyTarget, rekey_scope},
    write_sealer::StoreSealer,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};
use tokio_postgres::Client;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidConfig(String),
    Crypto(String),
    Database(String),
    OperationalGateClosed,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(msg) => write!(f, "설정 오류: {msg}"),
            Self::Crypto(msg) => write!(f, "암호화 오류: {msg}"),
            Self::Database(msg) => write!(f, "데이터베이스 오류: {msg}"),
            Self::OperationalGateClosed => write!(f, "운영 rekey 차단: 전수 인벤토리 증명은 연결됐지만 operational rekey gate가 닫혀 있음"),
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

/// A declaration-derived, PostgreSQL-checked inventory. Construction is private to this module so
/// callers cannot mark an arbitrary target slice as complete.
#[derive(Debug)]
pub struct VerifiedRekeyInventory {
    pub scope: Scope,
    pub targets: Vec<RekeyTarget>,
}

/// Binds `StoreSealer` and `StoreOpener` for a deployment schema using the loaded provider.
pub fn bind_sealer_and_opener(store: DekStore, provider: LocalProvider, budget_client: Client) -> (StoreSealer, StoreOpener) {
    let provider = Arc::new(provider);
    let sealer = StoreSealer::new(store.clone(), provider.clone(), budget_client);
    let opener = StoreOpener::new(store, provider);
    (sealer, opener)
}

/// Operational rekey and activation remain closed after inventory-proof implementation.
pub(crate) fn rekey_operational_inventory_verified(_schema: &str, _options: &RekeyOptions) -> bool {
    false
}

/// Rekey operational parameters parsed from CLI or management worker.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RekeyOptions {
    pub scope_kind: String,
    pub scope_id: String,
    /// For `ref` scope, the declared resource referenced by the encrypted fields.
    #[serde(default)]
    pub scope_resource: Option<String>,
    pub holder_id: String,
    pub targets: Vec<RekeyTargetSpec>,
    /// Legacy caller assertion retained for wire compatibility; it is never used as proof.
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
    /// Validates the caller's selector syntax. Completeness is established only by
    /// [`verify_rekey_inventory`] against deployment facts and PostgreSQL metadata.
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
                if self.scope_resource.is_some() {
                    return Err(Error::InvalidConfig("service scope에는 scope_resource를 지정할 수 없음".into()));
                }
                if !self.scope_id.is_empty() && self.scope_id != "global" {
                    return Err(Error::InvalidConfig("service scope_id는 비어 있거나 'global'이어야 함".into()));
                }
                Scope::service()
            }
            "ref" => {
                if self.scope_id.trim().is_empty() {
                    return Err(Error::InvalidConfig("ref scope_id 누락".into()));
                }
                let target = self
                    .scope_resource
                    .as_deref()
                    .filter(|target| !target.trim().is_empty())
                    .ok_or_else(|| Error::InvalidConfig("ref scope의 선언 대상 scope_resource 누락".into()))?;
                Scope::reference(target, &self.scope_id).map_err(|e| Error::InvalidConfig(format!("ref scope 오류: {e:?}")))?
            }
            "owner" => {
                return Err(Error::InvalidConfig("owner scope rekey 인벤토리는 현재 지원하지 않음".into()));
            }
            _ => return Err(Error::InvalidConfig(format!("알 수 없는 scope_kind: {}", self.scope_kind))),
        };
        Ok(scope)
    }
}

/// Derives every encrypted target for the selected declaration scope from the deployment's
/// persisted normalized facts, then proves the exact target mapping against PostgreSQL catalogs.
/// Caller targets are compared as an assertion and can neither add nor omit inventory entries.
pub async fn verify_rekey_inventory(db: &Client, schema: &str, options: &RekeyOptions) -> Result<VerifiedRekeyInventory, Error> {
    let requested_scope = options.validate()?;
    aip_migrate::configure_schema(schema).map_err(|_| Error::InvalidConfig("deployment schema 이름이 유효하지 않음".into()))?;
    let meta_table = format!("\"{schema}\".\"aip_migrate_meta\"");
    let row = db
        .query_opt(&format!("SELECT facts, facts_digest FROM {meta_table} WHERE singleton IS TRUE"), &[])
        .await
        .map_err(|_| Error::Database(format!("deployment metadata {schema}.aip_migrate_meta 조회 실패")))?
        .ok_or_else(|| Error::Database(format!("deployment metadata {schema}.aip_migrate_meta 누락")))?;
    let facts: Value = row.try_get(0).map_err(|_| Error::Database("deployment facts 형식 오류".into()))?;
    let recorded_digest: String = row.try_get(1).map_err(|_| Error::Database("deployment facts digest 누락".into()))?;
    if spike_v1_fixture::digest(&facts) != recorded_digest {
        return Err(Error::Database("deployment facts digest 불일치".into()));
    }

    let declared = aip_migrate::verify::encrypted_columns(schema, &facts).map_err(|error| {
        Error::InvalidConfig(format!("정의 metadata encrypted inventory 오류: {}", error["message"].as_str().unwrap_or("잘못된 정의")))
    })?;
    let selected = match options.scope_kind.as_str() {
        "service" => declared.into_iter().filter(|column| matches!(&column.scope, aip_migrate::verify::ScopeSpec::Service)).collect::<Vec<_>>(),
        "ref" => {
            let scope_resource = options.scope_resource.as_deref().expect("validated ref scope_resource");
            declared
                .into_iter()
                .filter(|column| matches!(&column.scope, aip_migrate::verify::ScopeSpec::Ref { target, .. } if target == scope_resource))
                .collect::<Vec<_>>()
        }
        _ => unreachable!("validate rejects unsupported scope kinds"),
    };
    if selected.is_empty() {
        return Err(Error::InvalidConfig("선택한 scope의 선언 encrypted field inventory가 비어 있음".into()));
    }

    let mut derived = Vec::with_capacity(selected.len());
    for column in &selected {
        let (table_schema, table) = column.table.split_once('.').ok_or_else(|| Error::InvalidConfig("선언 table 식별자 오류".into()))?;
        if table_schema != schema {
            return Err(Error::InvalidConfig("선언 encrypted table의 schema 불일치".into()));
        }
        let id_type = column.id_kind;
        require_pg_column(db, schema, table, "id", id_type, true, &format!("{}.id", column.resource)).await?;
        require_pg_primary_key(db, schema, table, "id", &format!("{}.id", column.resource)).await?;
        let field_type = facts["resources"][&column.resource]["fields"][&column.field]["ty"]
            .as_str()
            .ok_or_else(|| Error::InvalidConfig(format!("{}.{} 선언 타입 누락", column.resource, column.field)))?;
        require_pg_column(
            db,
            schema,
            table,
            &column.column,
            "text",
            !field_type.ends_with('?'),
            &format!("{}.{} encrypted", column.resource, column.field),
        )
        .await?;

        let scope = match &column.scope {
            aip_migrate::verify::ScopeSpec::Service => {
                if options.scope_kind != "service" {
                    return Err(Error::InvalidConfig("caller scope와 선언 service scope 불일치".into()));
                }
                Scope::service()
            }
            aip_migrate::verify::ScopeSpec::Ref { target, column: scope_column } => {
                if options.scope_kind != "ref" || options.scope_resource.as_deref() != Some(target.as_str()) {
                    return Err(Error::InvalidConfig(format!("caller scope와 선언 ref scope 불일치: {target}")));
                }
                let target_id_type = facts["resources"][target]["idKind"].as_str().unwrap_or("bigint");
                let target_pg_type = if target_id_type == "uuid" { "uuid" } else { "bigint" };
                require_pg_column(db, schema, table, scope_column, target_pg_type, true, &format!("{}.{} scope", column.resource, target)).await?;
                require_pg_reference(db, schema, table, scope_column, &spike_v2_read::sqlgen::snake(target)).await?;
                Scope::reference(target, &options.scope_id).map_err(|error| Error::InvalidConfig(format!("선언 ref scope 오류: {error:?}")))?
            }
        };
        derived.push(RekeyTarget::new(table, &column.column, "id", &column.resource, &column.field, scope));
    }
    derived.sort_by(|a, b| (&a.table, &a.column, &a.resource, &a.field).cmp(&(&b.table, &b.column, &b.resource, &b.field)));
    let mut actual = options.targets.clone();
    actual.sort_by(|a, b| (&a.table, &a.column, &a.resource, &a.field).cmp(&(&b.table, &b.column, &b.resource, &b.field)));
    if actual.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(Error::InvalidConfig("caller rekey target inventory contains duplicate target".into()));
    }
    let expected = derived
        .iter()
        .map(|target| RekeyTargetSpec {
            table: target.table.clone(),
            column: target.column.clone(),
            id_column: target.id_column.clone(),
            resource: target.resource.clone(),
            field: target.field.clone(),
        })
        .collect::<Vec<_>>();
    if actual != expected {
        let missing = expected
            .iter()
            .filter(|target| !actual.contains(target))
            .map(|target| format!("{}.{}", target.resource, target.field))
            .collect::<Vec<_>>();
        let unexpected = actual
            .iter()
            .filter(|target| !expected.contains(target))
            .map(|target| format!("{}.{}", target.resource, target.field))
            .collect::<Vec<_>>();
        return Err(Error::InvalidConfig(format!("caller target inventory와 선언 inventory 불일치: missing={missing:?}, unexpected={unexpected:?}")));
    }
    Ok(VerifiedRekeyInventory { scope: requested_scope, targets: derived })
}

async fn require_pg_column(
    db: &Client,
    schema: &str,
    table: &str,
    column: &str,
    expected_type: &str,
    expected_not_null: bool,
    label: &str,
) -> Result<(), Error> {
    let row = db
        .query_opt(
            "SELECT pg_catalog.format_type(a.atttypid, a.atttypmod), a.attnotnull FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid=a.attrelid \
         JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_type t ON t.oid=a.atttypid \
         WHERE n.nspname=$1 AND c.relname=$2 AND c.relkind IN ('r','p') AND a.attname=$3 AND a.attnum>0 AND NOT a.attisdropped",
            &[&schema, &table, &column],
        )
        .await
        .map_err(|_| Error::Database(format!("PostgreSQL metadata {label} 조회 실패")))?;
    let Some(row) = row else {
        return Err(Error::Database(format!("PostgreSQL metadata encrypted column 누락: {label} ({schema}.{table}.{column})")));
    };
    let actual_type: String = row.try_get(0).map_err(|_| Error::Database(format!("PostgreSQL metadata {label} type 오류")))?;
    let actual_not_null: bool = row.try_get(1).map_err(|_| Error::Database(format!("PostgreSQL metadata {label} nullability 오류")))?;
    if actual_type != expected_type || actual_not_null != expected_not_null {
        return Err(Error::Database(format!(
            "PostgreSQL metadata {label} 불일치: type={actual_type}, not_null={actual_not_null}, expected_type={expected_type}, expected_not_null={expected_not_null}"
        )));
    }
    Ok(())
}

async fn require_pg_primary_key(db: &Client, schema: &str, table: &str, column: &str, label: &str) -> Result<(), Error> {
    let valid: bool = db
        .query_one(
            "SELECT EXISTS (
           SELECT 1
           FROM pg_catalog.pg_constraint c
           JOIN pg_catalog.pg_class t ON t.oid=c.conrelid
           JOIN pg_catalog.pg_namespace n ON n.oid=t.relnamespace
           JOIN pg_catalog.pg_attribute a ON a.attrelid=t.oid AND a.attname=$3 AND a.attnum>0 AND NOT a.attisdropped
           JOIN pg_catalog.pg_index i ON i.indexrelid=c.conindid AND i.indrelid=t.oid
           WHERE n.nspname=$1 AND t.relname=$2 AND c.contype='p' AND c.convalidated
             AND c.conkey=ARRAY[a.attnum]::smallint[]
             AND i.indisunique AND i.indisvalid AND i.indisready AND i.indpred IS NULL AND i.indnkeyatts=1
         )",
            &[&schema, &table, &column],
        )
        .await
        .map_err(|_| Error::Database(format!("PostgreSQL metadata {label} primary key catalog 조회 실패")))?
        .try_get(0)
        .map_err(|_| Error::Database(format!("PostgreSQL metadata {label} primary key catalog 형식 오류")))?;
    if !valid {
        return Err(Error::Database(format!("PostgreSQL metadata {label} id primary key 불일치: single-column unique primary key required")));
    }
    Ok(())
}

async fn require_pg_reference(db: &Client, schema: &str, table: &str, column: &str, target_table: &str) -> Result<(), Error> {
    let validated: bool = db
        .query_one(
            "SELECT COALESCE(bool_or(c.convalidated), false) FROM pg_catalog.pg_constraint c
           JOIN pg_catalog.pg_class source ON source.oid=c.conrelid
           JOIN pg_catalog.pg_namespace source_ns ON source_ns.oid=source.relnamespace
           JOIN pg_catalog.pg_attribute source_col ON source_col.attrelid=source.oid AND source_col.attnum=c.conkey[1]
           JOIN pg_catalog.pg_class target ON target.oid=c.confrelid
           JOIN pg_catalog.pg_namespace target_ns ON target_ns.oid=target.relnamespace
           JOIN pg_catalog.pg_attribute target_col ON target_col.attrelid=target.oid AND target_col.attnum=c.confkey[1]
           WHERE c.contype='f' AND source_ns.nspname=$1 AND source.relname=$2 AND source_col.attname=$3
             AND target_ns.nspname=$1 AND target.relname=$4 AND target_col.attname='id'
             AND array_length(c.conkey,1)=1 AND array_length(c.confkey,1)=1",
            &[&schema, &table, &column, &target_table],
        )
        .await
        .map_err(|_| Error::Database(format!("PostgreSQL scope FK metadata {}.{}.{column} 조회 실패", schema, table)))?
        .try_get(0)
        .map_err(|_| Error::Database(format!("PostgreSQL scope FK metadata {}.{}.{column} 형식 오류", schema, table)))?;
    if !validated {
        return Err(Error::Database(format!("PostgreSQL scope FK 불일치: {}.{}.{column} must reference validated {target_table}.id", schema, table)));
    }
    Ok(())
}

/// Executes durable rekey cycle for the given scope and targets.
pub async fn execute_rekey(
    db: &mut Client,
    budget: &mut Client,
    schema: &str,
    kek_config: &KekConfig,
    options: &RekeyOptions,
) -> Result<Vec<RekeyReport>, Error> {
    // Verify declaration-derived inventory against PostgreSQL before the still-closed gate.
    let inventory = verify_rekey_inventory(db, schema, options).await?;

    // This public API is a second enforcement boundary; the CLI fence alone is insufficient.
    if !rekey_operational_inventory_verified(schema, options) {
        return Err(Error::OperationalGateClosed);
    }

    let store = DekStore::new(schema).map_err(|e| Error::InvalidConfig(format!("schema `{schema}`가 유효하지 않음: {e:?}")))?;
    kek_config.validate()?;
    let provider = kek_config.load_provider()?;

    rekey_scope(db, budget, &store, &inventory.scope, &inventory.targets, &options.holder_id, &provider)
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

    #[test]
    fn sealer_and_opener_share_one_provider_allocation() {
        let provider = Arc::new(LocalProvider::from_spec("k1:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=", "k1").unwrap());
        let sealer_provider = provider.clone();
        let opener_provider = provider;
        assert!(Arc::ptr_eq(&sealer_provider, &opener_provider));
        assert_eq!(Arc::strong_count(&sealer_provider), 2);
    }
}
