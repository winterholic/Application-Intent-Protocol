//! Optional live MySQL/MariaDB catalog adapter backed by `mysql_async`.
use crate::mysql_catalog::{
    CatalogError, ColumnRow, ForeignKeyRow, IndexRow, MysqlCatalogReader, MysqlDriverConfig, TableManifest, introspect_schema,
};
pub use mysql_async;
use mysql_async::prelude::Queryable;
use mysql_async::{IsolationLevel, Opts, OptsBuilder, Pool, PoolConstraints, PoolOpts, Row, SslOpts, Transaction, TxOpts, from_row_opt};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn logical_name(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map(|first| first.to_ascii_uppercase().to_string() + chars.as_str()).unwrap_or_default()
        })
        .collect()
}

fn valid_ident(name: &str) -> bool {
    spike_v2_read::external::valid_name(name)
}

fn render_mysql_draft(
    schema: &str,
    catalog: &std::collections::BTreeMap<String, (TableManifest, Vec<crate::mysql_catalog::ForeignKey>)>,
) -> Result<serde_json::Value, CatalogError> {
    if catalog.is_empty() {
        return Err(CatalogError::new("source schema has no base tables"));
    }
    let mut mapping = spike_v2_read::external::Mapping { schema: schema.to_owned(), tables: Default::default() };
    for (table_name, (manifest, foreign_keys)) in catalog {
        crate::mysql_catalog::require_single_column_keys(manifest, foreign_keys)?;
        let resource = logical_name(table_name);
        if !valid_ident(&resource) || resource.starts_with("Aip") || mapping.tables.contains_key(&resource) {
            return Err(CatalogError::new("resource names are ambiguous; manual mapping is required"));
        }
        mapping
            .tables
            .insert(resource, spike_v2_read::external::Table { table: table_name.clone(), columns: Default::default(), shared_pk_without_fk: false });
    }

    let mut definition = String::from("// Introspection draft: review authorization before publishing read/write.\nactor principal\n");
    for (resource, mapped) in &mut mapping.tables {
        let (manifest, foreign_keys) = catalog.get(&mapped.table).ok_or_else(|| CatalogError::new("catalog table disappeared during mapping"))?;
        let primary = manifest.indexes.iter().find(|index| index.primary).ok_or_else(|| CatalogError::new("table has no primary key"))?;
        let primary_column = &primary.columns[0];
        definition.push_str(&format!("resource {resource} {{\n  fields {{\n"));
        for column in &manifest.columns {
            let foreign_key = foreign_keys.iter().find(|key| key.columns.len() == 1 && key.columns[0] == column.name);
            let field = if column.name == *primary_column {
                "id".to_owned()
            } else if foreign_key.is_some() {
                column.name.strip_suffix("_id").unwrap_or(&column.name).to_owned()
            } else {
                column.name.clone()
            };
            if !valid_ident(&field) || mapped.columns.contains_key(&field) || field == "xmin" {
                return Err(CatalogError::new("field names collide; manual mapping is required"));
            }
            let ty = if column.name == *primary_column {
                match column.logical_type {
                    crate::mysql_catalog::LogicalType::Integer { .. } => "Id".to_owned(),
                    _ => return Err(CatalogError::new("primary keys must use a supported integer type")),
                }
            } else if let Some(key) = foreign_key {
                if key.columns.len() != 1 || key.referenced_columns.len() != 1 || key.referenced_schema != schema {
                    return Err(CatalogError::new("composite or cross-schema foreign keys require a manual definition"));
                }
                logical_name(&key.referenced_table)
            } else {
                match column.logical_type {
                    crate::mysql_catalog::LogicalType::Integer { .. } => "Int".to_owned(),
                    crate::mysql_catalog::LogicalType::Text => "Text".to_owned(),
                    _ => return Err(CatalogError::new("unsupported column type requires a manual definition")),
                }
            };
            if !valid_ident(&ty) {
                return Err(CatalogError::new("foreign-key target name is invalid"));
            }
            let is_database_default = matches!(column.default, crate::mysql_catalog::CatalogDefault::Value(_));
            definition.push_str(&format!(
                "    {field}: {ty}{}{};\n",
                if column.nullable { "?" } else { "" },
                if is_database_default && column.name != *primary_column { " default database" } else { "" },
            ));
            mapped.columns.insert(field, column.name.clone());
        }
        definition.push_str("  }\n  rows read when false\n}\n");
    }
    spike_v1_fixture::load_str(&definition, spike_v1_fixture::Form::A)
        .map_err(|_| CatalogError::new("generated definition is invalid; manual mapping is required"))?;
    let catalog_value = serde_json::to_value(catalog).map_err(|_| CatalogError::new("catalog serialization failed"))?;
    Ok(serde_json::json!({
        "ok": true,
        "definition": definition,
        "mapping": mapping,
        "structureDigest": super::digest(&catalog_value),
        "catalog": catalog_value,
    }))
}

#[derive(Clone)]
pub struct MysqlDriver {
    pool: Pool,
    opts: Opts,
    connect_timeout: Duration,
    mariadb: bool,
}

impl MysqlDriver {
    /// Parse server configuration from the URL and create a real driver pool.
    /// `AIP_MYSQL_CA_CERT` may point to an additional trusted PEM/DER CA bundle.
    pub fn from_url(url: &str) -> Result<Self, CatalogError> {
        let ca_cert = std::env::var_os("AIP_MYSQL_CA_CERT").map(PathBuf::from);
        Self::from_url_with_ca(url, ca_cert.as_deref())
    }

    /// Build a driver using the service's configured database CA, when present.
    pub fn from_url_with_ca(url: &str, ca_cert: Option<&Path>) -> Result<Self, CatalogError> {
        let config = MysqlDriverConfig::parse_url(url)?;
        let opts = build_mysql_opts(&config, ca_cert.map(Path::to_path_buf))?;
        let connect_timeout = Duration::from_millis(config.connect_timeout_ms);
        Ok(Self { pool: Pool::new(opts.clone()), opts, connect_timeout, mariadb: config.mariadb })
    }

    /// Connect and verify the connection eagerly.
    pub async fn connect(url: &str) -> Result<Self, CatalogError> {
        let driver = Self::from_url(url)?;
        driver.check_connection().await?;
        Ok(driver)
    }

    pub fn is_mariadb(&self) -> bool {
        self.mariadb
    }

    pub fn opts(&self) -> &Opts {
        &self.opts
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Establish a connection now; `Pool::new` itself is lazy.
    pub async fn check_connection(&self) -> Result<(), CatalogError> {
        let conn = tokio::time::timeout(self.connect_timeout, self.pool.get_conn())
            .await
            .map_err(|_| CatalogError::new("MySQL connection timed out"))?
            .map_err(|_| CatalogError::new("MySQL connection failed"))?;
        drop(conn);
        Ok(())
    }

    pub async fn introspect(
        &self,
        schema: &str,
    ) -> Result<std::collections::BTreeMap<String, (TableManifest, Vec<crate::mysql_catalog::ForeignKey>)>, CatalogError> {
        introspect_schema(self, schema).await
    }

    /// Compute a fresh structural fingerprint for every call. This deliberately has no TTL/cache:
    /// callers use it as a deployment fence and must observe out-of-band DDL on the next check.
    pub async fn catalog_fingerprint(&self, schema: &str) -> Result<String, CatalogError> {
        // One definition of the fingerprint: the same schema-wide queries the request fence runs inside its transaction.
        let mut conn = self.get_conn_with_timeout().await?;
        let _ = conn.query_drop("SET SESSION information_schema_stats_expiry = 0").await;
        catalog_fingerprint_on_conn(&mut conn, schema, self.mariadb).await.map(|fingerprint| fingerprint.digest)
    }

    /// Fail closed if the live catalog no longer matches the fingerprint recorded at adoption.
    /// The fingerprint is recomputed from INFORMATION_SCHEMA on every invocation.
    pub async fn verify_catalog_fingerprint(&self, schema: &str, expected: &str) -> Result<(), CatalogError> {
        if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CatalogError::new("invalid expected MySQL catalog fingerprint"));
        }
        let actual = self.catalog_fingerprint(schema).await?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(CatalogError::drift("MySQL schema changed since the deployed catalog fingerprint was recorded"));
        }
        Ok(())
    }

    /// Open a fenced transaction from a frozen [`MysqlDeploymentFence`]. The caller never assembles
    /// the table set; the read/write set computed at fence construction is pinned and then re-verified
    /// against the live catalog by Step 2c in [`Self::begin_fenced_transaction_with_bounds`].
    pub async fn begin_fenced_transaction(&self, read_only: bool, fence: &MysqlDeploymentFence) -> Result<FencedMysqlTransaction, CatalogError> {
        self.begin_fenced_transaction_with_bounds(
            read_only,
            &fence.meta_schema,
            &fence.source_schema,
            fence.bound_tables_for(read_only),
            &fence.expected_facts_digest,
            &fence.expected_structure_digest,
        )
        .await
    }

    /// Read-write fenced transaction for the write path. Same pre-pin and fingerprint checks as a read, over the write set
    /// (meta write tables included), at READ COMMITTED, or SERIALIZABLE for transitions whose policy reads other rows.
    pub async fn begin_fenced_write_transaction(
        &self,
        fence: &MysqlDeploymentFence,
        serializable: bool,
    ) -> Result<FencedMysqlTransaction, CatalogError> {
        let isolation = if serializable { IsolationLevel::Serializable } else { IsolationLevel::ReadCommitted };
        self.begin_fenced_inner(
            false,
            isolation,
            &fence.meta_schema,
            &fence.source_schema,
            fence.bound_tables_for(false),
            &fence.expected_facts_digest,
            &fence.expected_structure_digest,
        )
        .await
    }

    /// Single-connection fenced transaction lifecycle:
    /// Starts a transaction on one pooled connection, pins every bound table with a metadata lock,
    /// verifies the meta marker and the schema-wide catalog fingerprint, and finally proves that the
    /// pinned set equals the canonical set derived from that same live catalog.
    ///
    /// Low-level entry for tests and the fence wrapper. An incomplete `bound_tables` can never yield a
    /// transaction because Step 2c rejects it; production callers use [`Self::begin_fenced_transaction`].
    #[doc(hidden)]
    pub async fn begin_fenced_transaction_with_bounds(
        &self,
        read_only: bool,
        meta_schema: &str,
        source_schema: &str,
        bound_tables: &[(String, String)],
        expected_facts_digest: &str,
        expected_structure_digest: &str,
    ) -> Result<FencedMysqlTransaction, CatalogError> {
        self.begin_fenced_inner(
            read_only,
            IsolationLevel::RepeatableRead,
            meta_schema,
            source_schema,
            bound_tables,
            expected_facts_digest,
            expected_structure_digest,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn begin_fenced_inner(
        &self,
        read_only: bool,
        isolation: IsolationLevel,
        meta_schema: &str,
        source_schema: &str,
        bound_tables: &[(String, String)],
        expected_facts_digest: &str,
        expected_structure_digest: &str,
    ) -> Result<FencedMysqlTransaction, CatalogError> {
        // Step 0: input validation. Identifiers are interpolated into SQL below, so reject unsafe ones here.
        if bound_tables.is_empty() {
            return Err(CatalogError::drift("bound_tables must not be empty for fenced MySQL transaction"));
        }
        validate_ident("meta schema", meta_schema)?;
        validate_ident("source schema", source_schema)?;
        for (schema, table) in bound_tables {
            validate_ident("bound table schema", schema)?;
            validate_ident("bound table", table)?;
        }

        // Step 1: start the transaction through mysql_async so its own cleanup path owns rollback,
        // then pre-pin metadata locks before any INFORMATION_SCHEMA read.
        let mut tx_options = TxOpts::default();
        tx_options.with_isolation_level(isolation).with_readonly(read_only);
        let mut tx: Transaction<'static> = tokio::time::timeout(self.connect_timeout, self.pool.start_transaction(tx_options))
            .await
            .map_err(|_| CatalogError::new("MySQL connection timed out"))?
            .map_err(|err| CatalogError::new(format!("failed to start transaction: {err}")))?;

        // Disable stats caching for immediate I_S accuracy (MySQL 8.4; ignore on MariaDB)
        let _ = tx.query_drop("SET SESSION information_schema_stats_expiry = 0").await;

        if let Err(err) = pin_bound_tables_on_conn(&mut tx, bound_tables).await {
            let _ = tx.rollback().await;
            return Err(err);
        }

        // Step 2a: meta marker must be READY and carry the expected facts digest.
        if let Err(err) = verify_meta_digest_on_conn(&mut tx, meta_schema, expected_facts_digest).await {
            let _ = tx.rollback().await;
            return Err(err);
        }

        // Step 2b: schema-wide 4-query catalog verification.
        let fingerprint = match catalog_fingerprint_on_conn(&mut tx, source_schema, self.mariadb).await {
            Ok(fp) => fp,
            Err(err) => {
                let _ = tx.rollback().await;
                return Err(err);
            }
        };
        if !fingerprint.digest.eq_ignore_ascii_case(expected_structure_digest) {
            let _ = tx.rollback().await;
            return Err(CatalogError::drift(format!(
                "MySQL schema changed since deployed catalog fingerprint was recorded (actual: {}, expected: {})",
                fingerprint.digest, expected_structure_digest
            )));
        }

        // Step 2c: the pinned set must equal the canonical set of this very catalog. A caller that omits
        // one table would otherwise pass the digest check while that table stays unprotected from DDL.
        let catalog_tables: Vec<String> = fingerprint.catalog.keys().cloned().collect();
        let expected_bound_tables = canonical_bound_tables(meta_schema, source_schema, &catalog_tables, !read_only);
        if bound_tables != expected_bound_tables.as_slice() {
            let _ = tx.rollback().await;
            return Err(CatalogError::drift(format!(
                "bound_tables mismatch for fenced MySQL transaction: pinned {bound_tables:?}, expected {expected_bound_tables:?}"
            )));
        }

        // Step 3
        Ok(FencedMysqlTransaction { tx: Some(tx), read_only })
    }

    /// Record `MIGRATING` before any meta/source DDL starts. DDL implicitly commits in MySQL/MariaDB, so a
    /// crash mid-migration cannot roll back; the marker is what keeps runtime entry closed meanwhile.
    pub async fn mark_meta_migrating(&self, meta_schema: &str) -> Result<(), CatalogError> {
        validate_ident("meta schema", meta_schema)?;
        let mut conn = self.get_conn_with_timeout().await?;
        write_meta_status_migrating(&mut conn, meta_schema).await
    }

    /// Idempotent meta bootstrap with a two-phase marker:
    /// A) `CREATE ... IF NOT EXISTS` for the meta marker table, B) `status = 'MIGRATING'`, then the other
    /// meta tables, C) re-read the live source catalog, D) record digests and flip to `READY`.
    /// Any failure (or crash) after B leaves `MIGRATING`, which `begin_fenced_transaction` rejects.
    /// Re-running after an interruption converges to `READY`. Returns the fingerprint recorded in step D.
    pub async fn ensure_meta_schema_bootstrap(
        &self,
        meta_schema: &str,
        source_schema: &str,
        facts_digest: &str,
    ) -> Result<MysqlCatalogFingerprint, CatalogError> {
        validate_ident("meta schema", meta_schema)?;
        validate_ident("source schema", source_schema)?;
        if facts_digest.trim().is_empty() {
            return Err(CatalogError::drift("facts_digest must not be empty for meta bootstrap"));
        }
        let mut conn = self.get_conn_with_timeout().await?;

        // Steps A and B: schema, marker table, `MIGRATING` marker, then the remaining meta tables.
        begin_meta_bootstrap(&mut conn, meta_schema).await?;

        // Step C
        let _ = conn.query_drop("SET SESSION information_schema_stats_expiry = 0").await;
        let fingerprint = catalog_fingerprint_on_conn(&mut conn, source_schema, self.mariadb).await?;
        if fingerprint.catalog.is_empty() {
            return Err(CatalogError::new("source schema has no base tables"));
        }

        // Step D
        conn.exec_drop(
            format!("UPDATE `{meta_schema}`.`{META_TABLE}` SET facts_digest = ?, schema_sha256 = ?, status = 'READY' WHERE singleton = 1"),
            (facts_digest, fingerprint.digest.as_str()),
        )
        .await
        .map_err(|err| CatalogError::new(format!("meta READY update failed: {err}")))?;
        Ok(fingerprint)
    }

    pub(crate) async fn get_conn_with_timeout(&self) -> Result<mysql_async::Conn, CatalogError> {
        tokio::time::timeout(self.connect_timeout, self.pool.get_conn())
            .await
            .map_err(|_| CatalogError::new("MySQL connection timed out"))?
            .map_err(|_| CatalogError::new("MySQL connection failed"))
    }

    async fn query_rows(&self, query: &str, params: impl Into<mysql_async::Params> + Send) -> Result<Vec<Row>, CatalogError> {
        let mut conn = tokio::time::timeout(self.connect_timeout, self.pool.get_conn())
            .await
            .map_err(|_| CatalogError::new("MySQL connection timed out"))?
            .map_err(|_| CatalogError::new("MySQL connection failed"))?;
        conn.exec(query, params).await.map_err(|_| CatalogError::new("MySQL catalog query failed"))
    }
}

#[derive(Clone, Debug)]
pub struct MysqlCatalogFingerprint {
    pub digest: String,
    pub catalog: crate::mysql_catalog::CanonicalSchemaCatalog,
}

/// A fenced transaction. It owns a `mysql_async::Transaction` rather than a raw connection and exposes
/// no way to take the connection out: dropping it (including task abort or timeout) hands the connection
/// back to the pool flagged `RequiresRollback`, and mysql_async rolls it back before the pool reuses it.
pub struct FencedMysqlTransaction {
    tx: Option<Transaction<'static>>,
    read_only: bool,
}

impl std::fmt::Debug for FencedMysqlTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FencedMysqlTransaction").field("active", &self.tx.is_some()).field("read_only", &self.read_only).finish()
    }
}

impl FencedMysqlTransaction {
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Borrow the open transaction for queries. Ownership never leaves this wrapper.
    pub fn as_mut(&mut self) -> Result<&mut Transaction<'static>, CatalogError> {
        self.tx.as_mut().ok_or_else(|| CatalogError::new("fenced MySQL transaction is no longer active"))
    }

    pub async fn commit(mut self) -> Result<(), CatalogError> {
        if let Some(tx) = self.tx.take() {
            tx.commit().await.map_err(|e| CatalogError::new(format!("MySQL commit failed: {e}")))?;
        }
        Ok(())
    }

    /// COMMIT with the outcome classified by whether it was sent. `on_send` runs right before COMMIT leaves.
    pub async fn commit_classified(mut self, on_send: impl FnOnce()) -> Result<(), CommitFailure> {
        let Some(tx) = self.tx.take() else { return Err(CommitFailure::NotSent) };
        on_send();
        match tx.commit().await {
            Ok(()) => Ok(()),
            Err(mysql_async::Error::Server(server)) => Err(CommitFailure::Rejected { code: server.code }),
            // The connection broke, or the reply could not be read, after COMMIT was sent: the server may have committed.
            Err(_) => Err(CommitFailure::Unknown),
        }
    }

    pub async fn rollback(mut self) -> Result<(), CatalogError> {
        if let Some(tx) = self.tx.take() {
            tx.rollback().await.map_err(|e| CatalogError::new(format!("MySQL rollback failed: {e}")))?;
        }
        Ok(())
    }
}

/// How a COMMIT ended, split by whether the server could have committed.
#[derive(Debug)]
pub enum CommitFailure {
    /// No transaction was left to commit.
    NotSent,
    /// COMMIT was sent and the connection failed before the reply: the outcome is unknown.
    Unknown,
    /// The server refused the COMMIT (deadlock victim, constraint): rolled back.
    Rejected { code: u16 },
}

pub(crate) const META_TABLE: &str = "aip_migrate_meta";
/// Meta tables every runtime transaction reads (the marker and the principals).
const META_READ_TABLES: [&str; 2] = [META_TABLE, "aip_principals"];
/// Additional meta tables a read-write transaction touches.
const META_WRITE_TABLES: [&str; 6] = ["aip_idem", "aip_audit", "aip_events", "aip_tx_locks", "aip_jobs", "aip_outbox"];

pub(crate) fn validate_ident(kind: &str, name: &str) -> Result<(), CatalogError> {
    if name.trim().is_empty() || name.contains('`') || name.contains('\0') {
        return Err(CatalogError::drift(format!("{kind} must be a non-empty identifier without backticks")));
    }
    Ok(())
}

/// Steps A and B of the bootstrap: the marker is written as `MIGRATING` before any other meta table exists, so an
/// interrupted run can never leave a half-built meta schema that looks ready.
pub(crate) async fn begin_meta_bootstrap<Q: Queryable>(conn: &mut Q, meta_schema: &str) -> Result<(), CatalogError> {
    conn.query_drop(format!("CREATE DATABASE IF NOT EXISTS `{meta_schema}`"))
        .await
        .map_err(|err| CatalogError::new(format!("meta schema creation failed: {err}")))?;
    conn.query_drop(format!(
        "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`{META_TABLE}` (singleton TINYINT NOT NULL PRIMARY KEY, \
         facts_digest VARCHAR(64) NOT NULL, schema_sha256 VARCHAR(64) NOT NULL, status VARCHAR(32) NOT NULL, \
         facts JSON NULL, runtime_wire VARCHAR(32) NULL, ddl_digest VARCHAR(64) NULL, generator_version INT NULL) ENGINE=InnoDB"
    ))
    .await
    .map_err(|err| CatalogError::new(format!("meta table creation failed: {err}")))?;
    write_meta_status_migrating(conn, meta_schema).await?;
    create_meta_tables(conn, meta_schema).await
}

/// The AIP-owned tables of a MySQL deployment. The read path needs the marker, the principal bindings and the actor handle
/// tables; the write tables (`aip_idem`, `aip_audit`, ...) are placeholders until the write slice (WBS 7.2.1.2/7.2.1.3)
/// gives them their real shape.
///
/// `issuer` and `subject` use a binary collation: a JWT subject is case sensitive, and a case-insensitive collation would let
/// `Alice` and `alice` resolve to one binding.
pub(crate) async fn create_meta_tables<Q: Queryable>(conn: &mut Q, meta_schema: &str) -> Result<(), CatalogError> {
    let mut statements = vec![
        format!(
            "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_principals` (\
               issuer VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, \
               subject VARCHAR(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, \
               actor_id BIGINT NOT NULL, enabled TINYINT(1) NOT NULL DEFAULT 1, min_iat BIGINT NOT NULL DEFAULT 0, \
               PRIMARY KEY (issuer, subject), CONSTRAINT aip_principals_actor_positive CHECK (actor_id > 0)) ENGINE=InnoDB"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_actors` (id BIGINT NOT NULL PRIMARY KEY, CONSTRAINT aip_actors_positive CHECK (id > 0)) ENGINE=InnoDB"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_actor_identities` (\
               id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, \
               resource_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, UNIQUE KEY aip_actor_identities_resource (resource_id)) ENGINE=InnoDB"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_journal` (\
               id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, plan_digest VARCHAR(64) NOT NULL, old_digest VARCHAR(64) NULL, \
               new_digest VARCHAR(64) NOT NULL, steps JSON NOT NULL, approval VARCHAR(64) NULL, \
               applied_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)) ENGINE=InnoDB"
        ),
    ];
    // Write path (WBS 7.2.1.2). Idempotency journal, append-only audit, outbox and the transaction lock buckets.
    statements.push(format!(
        "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_idem` (\
           principal VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, \
           idem_key VARCHAR(100) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, \
           request MEDIUMTEXT CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, result JSON NOT NULL, \
           created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), PRIMARY KEY (principal, idem_key)) ENGINE=InnoDB"
    ));
    statements.push(format!(
        "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_audit` (\
           id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, recorded_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6), \
           actor_id BIGINT NULL, via VARCHAR(16) NOT NULL, request_key VARCHAR(100) NULL, resource VARCHAR(128) NOT NULL, \
           transition VARCHAR(128) NOT NULL, target_id VARCHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, \
           before_values JSON NOT NULL, after_values JSON NOT NULL, KEY aip_audit_target (resource, target_id, id)) ENGINE=InnoDB"
    ));
    statements.push(format!(
        "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_outbox` (\
           id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, topic VARCHAR(255) NOT NULL, recipient_id VARCHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL, \
           source VARCHAR(128) NOT NULL, source_id VARCHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL, \
           created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)) ENGINE=InnoDB"
    ));
    // Transaction-scoped named locks. MySQL has no `pg_advisory_xact_lock`; a row lock on one of a fixed set of buckets
    // lives exactly until COMMIT or ROLLBACK, and a crashed connection releases it with the transaction.
    statements
        .push(format!("CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_tx_locks` (bucket SMALLINT UNSIGNED NOT NULL PRIMARY KEY) ENGINE=InnoDB"));
    statements.push(format!(
        "INSERT IGNORE INTO `{meta_schema}`.`aip_tx_locks` (bucket) \
         SELECT a.n * 100 + b.n * 10 + c.n FROM \
           (SELECT 0 n UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) a, \
           (SELECT 0 n UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) b, \
           (SELECT 0 n UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9) c"
    ));
    statements.push(format!(
        "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_events` (id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT PRIMARY KEY) ENGINE=InnoDB"
    ));
    // Durable jobs (WBS 7.2.1.3). The PostgreSQL unique key over (owner, operation, contract, deployment, key) is too wide for an
    // InnoDB index, so its SHA-256 is the unique column. All instants are UTC DATETIME(6) from the database clock.
    statements.push(format!(
        "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_jobs` (\
           id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL PRIMARY KEY, \
           owner VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, actor_id BIGINT NOT NULL, grant_data JSON NOT NULL, \
           operation VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, \
           contract_digest VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, deployment_digest VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, \
           implementation_digest VARCHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, \
           input JSON NOT NULL, input_digest CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, \
           idempotency_key VARCHAR(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, key_digest CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, \
           state VARCHAR(16) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, attempt INT NOT NULL DEFAULT 0, max_attempts INT NOT NULL, \
           lease_epoch BIGINT NOT NULL DEFAULT 0, lease_owner VARCHAR(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NULL, \
           lease_until DATETIME(6) NULL, attempt_deadline DATETIME(6) NULL, timeout_ms BIGINT NOT NULL, \
           run_deadline DATETIME(6) NOT NULL, next_run_at DATETIME(6) NOT NULL, cancel_requested_at DATETIME(6) NULL, \
           output JSON NULL, error VARCHAR(512) NULL, result_ttl_ms BIGINT NOT NULL, initial_delay_ms BIGINT NOT NULL, \
           created_at DATETIME(6) NOT NULL, result_expires_at DATETIME(6) NULL, trace_id VARCHAR(64) NULL, \
           UNIQUE KEY aip_jobs_key (key_digest), KEY aip_jobs_claim (state, next_run_at, created_at), KEY aip_jobs_lease (state, lease_until), \
           CONSTRAINT aip_jobs_state CHECK (state IN ('QUEUED', 'RUNNING', 'CANCEL_REQUESTED', 'SUCCEEDED', 'FAILED', 'CANCELLED')), \
           CONSTRAINT aip_jobs_actor CHECK (actor_id > 0), CONSTRAINT aip_jobs_attempts CHECK (attempt >= 0 AND max_attempts > 0), \
           CONSTRAINT aip_jobs_timeout CHECK (timeout_ms > 0 AND result_ttl_ms > 0 AND initial_delay_ms >= 0)) ENGINE=InnoDB"
    ));
    // The relay's receipt that an outbox row became a job: unique per row, written in the same transaction as the job.
    statements.push(format!(
        "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_outbox_dispatch` (\
           outbox_id BIGINT NOT NULL PRIMARY KEY, outbox_created_at DATETIME(6) NOT NULL, job_id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, \
           dispatched_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)) ENGINE=InnoDB"
    ));
    // The audit trail only grows.
    statements.push(format!("DROP TRIGGER IF EXISTS `{meta_schema}`.`aip_audit_no_update`"));
    statements.push(format!(
        "CREATE TRIGGER `{meta_schema}`.`aip_audit_no_update` BEFORE UPDATE ON `{meta_schema}`.`aip_audit` FOR EACH ROW \
         SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'aip_audit rows are append-only'"
    ));
    statements.push(format!("DROP TRIGGER IF EXISTS `{meta_schema}`.`aip_audit_no_delete`"));
    statements.push(format!(
        "CREATE TRIGGER `{meta_schema}`.`aip_audit_no_delete` BEFORE DELETE ON `{meta_schema}`.`aip_audit` FOR EACH ROW \
         SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'aip_audit rows are append-only'"
    ));
    // A binding keeps its actor: changing it would silently move a user's identity to another account.
    statements.push(format!("DROP TRIGGER IF EXISTS `{meta_schema}`.`aip_principal_actor_immutable`"));
    statements.push(format!(
        "CREATE TRIGGER `{meta_schema}`.`aip_principal_actor_immutable` BEFORE UPDATE ON `{meta_schema}`.`aip_principals` FOR EACH ROW \
         BEGIN IF NEW.actor_id <> OLD.actor_id THEN SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'principal actor_id is immutable'; END IF; END"
    ));
    statements.push(format!("DROP TRIGGER IF EXISTS `{meta_schema}`.`aip_actor_resource_immutable`"));
    statements.push(format!(
        "CREATE TRIGGER `{meta_schema}`.`aip_actor_resource_immutable` BEFORE UPDATE ON `{meta_schema}`.`aip_actor_identities` FOR EACH ROW \
         BEGIN IF NEW.resource_id <> OLD.resource_id THEN SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT = 'actor resource identity is immutable'; END IF; END"
    ));
    for statement in statements {
        conn.query_drop(statement).await.map_err(|err| CatalogError::new(format!("meta table creation failed: {err}")))?;
    }
    Ok(())
}

async fn write_meta_status_migrating<Q: Queryable>(conn: &mut Q, meta_schema: &str) -> Result<(), CatalogError> {
    conn.query_drop(format!(
        "INSERT INTO `{meta_schema}`.`{META_TABLE}` (singleton, facts_digest, schema_sha256, status) VALUES (1, '', '', 'MIGRATING') \
         ON DUPLICATE KEY UPDATE status = 'MIGRATING'"
    ))
    .await
    .map_err(|err| CatalogError::new(format!("meta MIGRATING marker failed: {err}")))
}

async fn verify_meta_digest_on_conn<Q: Queryable>(conn: &mut Q, meta_schema: &str, expected_facts_digest: &str) -> Result<(), CatalogError> {
    let meta_sql = format!("SELECT facts_digest, schema_sha256, status FROM `{meta_schema}`.`{META_TABLE}` WHERE singleton = 1");
    let meta_row: Option<(String, String, String)> = conn.exec_first(&meta_sql, ()).await.map_err(|err| {
        if is_drift_error(&err) {
            CatalogError::drift(format!("meta table access failed: {err}"))
        } else {
            CatalogError::new(format!("meta table query failed: {err}"))
        }
    })?;
    match meta_row {
        Some((facts_digest, _schema_sha256, status)) if facts_digest == expected_facts_digest && status == "READY" => Ok(()),
        Some((facts_digest, _schema_sha256, status)) => {
            Err(CatalogError::drift(format!("aip_migrate_meta deployment changed (facts_digest: {facts_digest}, status: {status})")))
        }
        None => Err(CatalogError::drift("aip_migrate_meta singleton row missing")),
    }
}

/// Immutable deployment fence: the canonical bound-table sets are computed once, at construction,
/// so request paths never assemble table lists by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MysqlDeploymentFence {
    meta_schema: String,
    source_schema: String,
    expected_facts_digest: String,
    expected_structure_digest: String,
    read_bound_tables: Vec<(String, String)>,
    write_bound_tables: Vec<(String, String)>,
}

impl MysqlDeploymentFence {
    pub fn new(
        meta_schema: &str,
        source_schema: &str,
        source_tables: &[String],
        expected_facts_digest: &str,
        expected_structure_digest: &str,
    ) -> Result<Self, CatalogError> {
        if meta_schema.trim().is_empty()
            || source_schema.trim().is_empty()
            || source_tables.is_empty()
            || expected_facts_digest.trim().is_empty()
            || expected_structure_digest.trim().is_empty()
        {
            return Err(CatalogError::drift("MySQL deployment fence requires non-empty schemas, source_tables, facts_digest, and structure_digest"));
        }
        validate_ident("meta schema", meta_schema)?;
        validate_ident("source schema", source_schema)?;
        for table in source_tables {
            validate_ident("source table", table)?;
        }
        Ok(Self {
            meta_schema: meta_schema.to_string(),
            source_schema: source_schema.to_string(),
            expected_facts_digest: expected_facts_digest.to_string(),
            expected_structure_digest: expected_structure_digest.to_string(),
            read_bound_tables: canonical_bound_tables(meta_schema, source_schema, source_tables, false),
            write_bound_tables: canonical_bound_tables(meta_schema, source_schema, source_tables, true),
        })
    }

    pub fn from_fingerprint(
        meta_schema: &str,
        source_schema: &str,
        expected_facts_digest: &str,
        fingerprint: &MysqlCatalogFingerprint,
    ) -> Result<Self, CatalogError> {
        let source_tables: Vec<String> = fingerprint.catalog.keys().cloned().collect();
        Self::new(meta_schema, source_schema, &source_tables, expected_facts_digest, &fingerprint.digest)
    }

    pub fn bound_tables_for(&self, read_only: bool) -> &[(String, String)] {
        if read_only { &self.read_bound_tables } else { &self.write_bound_tables }
    }

    pub fn meta_schema(&self) -> &str {
        &self.meta_schema
    }

    pub fn source_schema(&self) -> &str {
        &self.source_schema
    }

    pub fn expected_facts_digest(&self) -> &str {
        &self.expected_facts_digest
    }

    pub fn expected_structure_digest(&self) -> &str {
        &self.expected_structure_digest
    }
}

pub fn is_drift_error(err: &mysql_async::Error) -> bool {
    match err {
        mysql_async::Error::Server(server_err) => matches!(server_err.code, 1146 | 1054 | 1412),
        _ => {
            let msg = err.to_string();
            msg.contains("1146")
                || msg.contains("1054")
                || msg.contains("1412")
                || (msg.contains("Table") && msg.contains("doesn't exist"))
                || msg.contains("Unknown column")
        }
    }
}

pub fn canonical_bound_tables(
    meta_schema: &str,
    source_schema: &str,
    catalog_tables: &[String],
    include_write_tables: bool,
) -> Vec<(String, String)> {
    let mut tables = Vec::new();

    for t in META_READ_TABLES {
        tables.push((meta_schema.to_string(), t.to_string()));
    }

    if include_write_tables {
        for t in META_WRITE_TABLES {
            tables.push((meta_schema.to_string(), t.to_string()));
        }
    }

    for t in catalog_tables {
        tables.push((source_schema.to_string(), t.to_string()));
    }

    tables.sort();
    tables.dedup();
    tables
}

pub async fn pin_bound_tables_on_conn<Q: Queryable>(conn: &mut Q, bound_tables: &[(String, String)]) -> Result<(), CatalogError> {
    if bound_tables.is_empty() {
        return Ok(());
    }

    let unions = bound_tables
        .iter()
        .enumerate()
        .map(|(idx, (schema, table))| format!("SELECT 1 FROM (SELECT 1 FROM `{schema}`.`{table}` LIMIT 1) AS p{idx}"))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");

    let pin_query = unions;

    let result: Result<Vec<i64>, _> = conn.query(&pin_query).await;
    match result {
        Ok(_) => Ok(()),
        Err(err) => {
            if is_drift_error(&err) {
                Err(CatalogError::drift(format!("pre-MDL pin failed due to missing or modified table: {err}")))
            } else {
                Err(CatalogError::new(format!("pre-MDL pin query failed: {err}")))
            }
        }
    }
}

pub async fn catalog_fingerprint_on_conn<Q: Queryable>(
    conn: &mut Q,
    schema_name: &str,
    is_mariadb: bool,
) -> Result<MysqlCatalogFingerprint, CatalogError> {
    let tables_rows: Vec<Row> = conn.exec(crate::mysql_catalog::SQL_TABLES, (schema_name,)).await.map_err(|err| {
        if is_drift_error(&err) {
            CatalogError::drift(format!("tables query drift: {err}"))
        } else {
            CatalogError::new(format!("tables query failed: {err}"))
        }
    })?;
    let tables: Vec<String> = tables_rows
        .into_iter()
        .map(|row| from_row_opt::<(String,)>(row).map(|(name,)| name).map_err(|_| CatalogError::new("invalid MySQL table catalog row")))
        .collect::<Result<Vec<_>, _>>()?;

    let columns_rows: Vec<Row> = conn.exec(crate::mysql_catalog::SQL_COLUMNS_BY_SCHEMA, (schema_name,)).await.map_err(|err| {
        if is_drift_error(&err) {
            CatalogError::drift(format!("columns query drift: {err}"))
        } else {
            CatalogError::new(format!("columns query failed: {err}"))
        }
    })?;
    let columns: Vec<crate::mysql_catalog::SchemaColumnRow> = columns_rows
        .into_iter()
        .map(|row| {
            let (
                table_name,
                name,
                ordinal_position,
                data_type,
                column_type,
                is_nullable,
                column_default,
                extra,
                generation_expression,
                collation_name,
            ) = from_row_opt::<(String, String, u64, String, String, String, Option<String>, String, String, Option<String>)>(row)
                .map_err(|_| CatalogError::new("invalid MySQL column catalog row"))?;
            Ok(crate::mysql_catalog::SchemaColumnRow {
                table_name,
                column: crate::mysql_catalog::ColumnRow {
                    name,
                    ordinal_position,
                    data_type,
                    column_type,
                    is_nullable,
                    column_default,
                    extra,
                    generation_expression,
                    collation_name,
                },
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let stats_query =
        if is_mariadb { crate::mysql_catalog::SQL_STATISTICS_MARIADB_BY_SCHEMA } else { crate::mysql_catalog::SQL_STATISTICS_BY_SCHEMA };
    let indexes_rows: Vec<Row> = conn.exec(stats_query, (schema_name,)).await.map_err(|err| {
        if is_drift_error(&err) {
            CatalogError::drift(format!("indexes query drift: {err}"))
        } else {
            CatalogError::new(format!("indexes query failed: {err}"))
        }
    })?;
    let indexes: Vec<crate::mysql_catalog::SchemaIndexRow> = indexes_rows
        .into_iter()
        .map(|row| {
            let (table_name, index_name, non_unique, seq_in_index, column_name, expression) =
                from_row_opt::<(String, String, u8, u64, Option<String>, Option<String>)>(row)
                    .map_err(|_| CatalogError::new("invalid MySQL index catalog row"))?;
            Ok(crate::mysql_catalog::SchemaIndexRow {
                table_name,
                index: crate::mysql_catalog::IndexRow { index_name, non_unique: non_unique != 0, seq_in_index, column_name, expression },
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let fk_rows_raw: Vec<Row> = conn.exec(crate::mysql_catalog::SQL_FOREIGN_KEYS_BY_SCHEMA, (schema_name,)).await.map_err(|err| {
        if is_drift_error(&err) {
            CatalogError::drift(format!("foreign keys query drift: {err}"))
        } else {
            CatalogError::new(format!("foreign keys query failed: {err}"))
        }
    })?;
    let foreign_keys: Vec<crate::mysql_catalog::ForeignKeyRow> = fk_rows_raw
        .into_iter()
        .map(|row| {
            from_row_opt::<(String, String, String, u64, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)>(row)
                .map(
                    |(
                        constraint_schema,
                        table_name,
                        constraint_name,
                        ordinal_position,
                        column_name,
                        referenced_table_schema,
                        referenced_table_name,
                        referenced_column_name,
                        delete_rule,
                        update_rule,
                    )| {
                        crate::mysql_catalog::ForeignKeyRow {
                            constraint_schema,
                            table_name,
                            constraint_name,
                            ordinal_position,
                            column_name,
                            referenced_table_schema,
                            referenced_table_name,
                            referenced_column_name,
                            delete_rule,
                            update_rule,
                        }
                    },
                )
                .map_err(|_| CatalogError::new("invalid MySQL foreign-key catalog row"))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let catalog = crate::mysql_catalog::introspect_schema_from_schema_rows(schema_name, tables, columns, indexes, foreign_keys)?;
    let raw = raw_structure_facts(conn, schema_name, is_mariadb).await?;
    let value = serde_json::json!({ "manifest": serde_json::to_value(&catalog).map_err(|_| CatalogError::new("MySQL catalog serialization failed"))?, "raw": raw });
    let digest = spike_v1_fixture::digest(&value);

    Ok(MysqlCatalogFingerprint { digest, catalog })
}

/// Rows the neutral manifest drops (full column types and collations, generation expressions, index prefix/direction/visibility,
/// CHECK constraints, storage engine). They are part of the fingerprint so that DDL on any of them is drift.
///
/// Triggers are not included on purpose: `INFORMATION_SCHEMA.TRIGGERS` only lists triggers the account holds the `TRIGGER` privilege on,
/// and granting that to the runtime account would let it create triggers. A trigger added out of band is therefore not detected here.
async fn raw_structure_facts<Q: Queryable>(conn: &mut Q, schema_name: &str, is_mariadb: bool) -> Result<serde_json::Value, CatalogError> {
    use crate::mysql_catalog::{
        SQL_RAW_CHECKS, SQL_RAW_CHECKS_MARIADB, SQL_RAW_COLUMNS, SQL_RAW_STATISTICS, SQL_RAW_STATISTICS_MARIADB, SQL_RAW_TABLES,
    };
    async fn fetch<Q: Queryable>(conn: &mut Q, sql: &str, schema_name: &str, what: &str) -> Result<Vec<Row>, CatalogError> {
        conn.exec(sql, (schema_name,)).await.map_err(|err| {
            if is_drift_error(&err) {
                CatalogError::drift(format!("{what} query drift: {err}"))
            } else {
                CatalogError::new(format!("{what} query failed: {err}"))
            }
        })
    }
    let bad = |what: &str| CatalogError::new(format!("invalid MySQL {what} catalog row"));
    let tables = fetch(conn, SQL_RAW_TABLES, schema_name, "raw tables")
        .await?
        .into_iter()
        .map(|row| {
            from_row_opt::<(String, Option<String>, Option<String>)>(row).map(|(a, b, c)| serde_json::json!([a, b, c])).map_err(|_| bad("table"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    #[allow(clippy::type_complexity)]
    let columns = fetch(conn, SQL_RAW_COLUMNS, schema_name, "raw columns")
        .await?
        .into_iter()
        .map(|row| {
            from_row_opt::<(String, String, u64, String, String, Option<String>, String, String, Option<String>, Option<String>)>(row)
                .map(|(a, b, c, d, e, f, g, h, i, j)| serde_json::json!([a, b, c, d, e, f, g, h, i, j]))
                .map_err(|_| bad("column"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let statistics_sql = if is_mariadb { SQL_RAW_STATISTICS_MARIADB } else { SQL_RAW_STATISTICS };
    #[allow(clippy::type_complexity)]
    let indexes = fetch(conn, statistics_sql, schema_name, "raw indexes")
        .await?
        .into_iter()
        .map(|row| {
            from_row_opt::<(String, String, u8, u64, Option<String>, Option<String>, Option<u64>, Option<String>, String, Option<String>)>(row)
                .map(|(a, b, c, d, e, f, g, h, i, j)| serde_json::json!([a, b, c, d, e, f, g, h, i, j]))
                .map_err(|_| bad("index"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let checks_sql = if is_mariadb { SQL_RAW_CHECKS_MARIADB } else { SQL_RAW_CHECKS };
    let checks = fetch(conn, checks_sql, schema_name, "raw checks")
        .await?
        .into_iter()
        .map(|row| {
            from_row_opt::<(String, String, String, String)>(row).map(|(a, b, c, d)| serde_json::json!([a, b, c, d])).map_err(|_| bad("check"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(serde_json::json!({ "tables": tables, "columns": columns, "indexes": indexes, "checks": checks }))
}

fn build_mysql_opts(config: &MysqlDriverConfig, ca_cert: Option<PathBuf>) -> Result<Opts, CatalogError> {
    let constraints = PoolConstraints::new(config.pool_min, config.pool_max).ok_or_else(|| CatalogError::new("invalid MySQL pool constraints"))?;
    let tls = if config.tls_required || ca_cert.is_some() {
        let mut ssl = SslOpts::default();
        if let Some(path) = ca_cert {
            ssl = ssl.with_root_certs(vec![path.into()]);
        }
        Some(ssl)
    } else {
        None
    };
    let statement_timeout_ms = config.statement_timeout_ms;
    let statement_timeout_query = if config.mariadb {
        format!("SET SESSION max_statement_time = {}.{:03}", statement_timeout_ms / 1000, statement_timeout_ms % 1000)
    } else {
        format!("SET SESSION max_execution_time = {statement_timeout_ms}")
    };
    let opts: Opts = OptsBuilder::default()
        .ip_or_hostname(config.host.clone())
        .tcp_port(config.port)
        .user(Some(config.user.clone()))
        .pass(config.password.clone())
        .db_name(Some(config.database.clone()))
        .pool_opts(PoolOpts::default().with_constraints(constraints))
        .ssl_opts(tls)
        // An UPDATE reports the rows it matched, like PostgreSQL's RETURNING count, not only the rows whose values changed.
        .client_found_rows(true)
        .init(vec![statement_timeout_query])
        .into();
    Ok(opts)
}

impl MysqlCatalogReader for MysqlDriver {
    async fn fetch_tables(&self, schema: &str) -> Result<Vec<String>, CatalogError> {
        let rows = self.query_rows(crate::mysql_catalog::SQL_TABLES, (schema,)).await?;
        rows.into_iter()
            .map(|row| from_row_opt::<(String,)>(row).map(|(name,)| name).map_err(|_| CatalogError::new("invalid MySQL table catalog row")))
            .collect()
    }

    async fn fetch_columns(&self, schema: &str, table: &str) -> Result<Vec<ColumnRow>, CatalogError> {
        let rows = self.query_rows(crate::mysql_catalog::SQL_COLUMNS, (schema, table)).await?;
        rows.into_iter()
            .map(|row| {
                let (name, ordinal_position, data_type, column_type, is_nullable, column_default, extra, generation_expression, collation_name) =
                    from_row_opt::<(String, u64, String, String, String, Option<String>, String, String, Option<String>)>(row)
                        .map_err(|_| CatalogError::new("invalid MySQL column catalog row"))?;
                Ok(ColumnRow {
                    name,
                    ordinal_position,
                    data_type,
                    column_type,
                    is_nullable,
                    column_default,
                    extra,
                    generation_expression,
                    collation_name,
                })
            })
            .collect()
    }

    async fn fetch_indexes(&self, schema: &str, table: &str) -> Result<Vec<IndexRow>, CatalogError> {
        let query = if self.mariadb { crate::mysql_catalog::SQL_STATISTICS_MARIADB } else { crate::mysql_catalog::SQL_STATISTICS };
        let rows = self.query_rows(query, (schema, table)).await?;
        rows.into_iter()
            .map(|row| {
                let (index_name, non_unique, seq_in_index, column_name, expression) =
                    from_row_opt::<(String, u8, u64, Option<String>, Option<String>)>(row)
                        .map_err(|_| CatalogError::new("invalid MySQL index catalog row"))?;
                Ok(IndexRow { index_name, non_unique: non_unique != 0, seq_in_index, column_name, expression })
            })
            .collect()
    }

    async fn fetch_foreign_keys(&self, schema: &str, table: &str) -> Result<Vec<ForeignKeyRow>, CatalogError> {
        let rows = self.query_rows(crate::mysql_catalog::SQL_FOREIGN_KEYS, (schema, table)).await?;
        rows.into_iter()
            .map(|row| {
                from_row_opt::<(String, String, String, u64, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)>(
                    row,
                )
                .map(
                    |(
                        constraint_schema,
                        table_name,
                        constraint_name,
                        ordinal_position,
                        column_name,
                        referenced_table_schema,
                        referenced_table_name,
                        referenced_column_name,
                        delete_rule,
                        update_rule,
                    )| {
                        ForeignKeyRow {
                            constraint_schema,
                            table_name,
                            constraint_name,
                            ordinal_position,
                            column_name,
                            referenced_table_schema,
                            referenced_table_name,
                            referenced_column_name,
                            delete_rule,
                            update_rule,
                        }
                    },
                )
                .map_err(|_| CatalogError::new("invalid MySQL foreign-key catalog row"))
            })
            .collect()
    }
}

/// Establish a MySQL/MariaDB connection and read its INFORMATION_SCHEMA catalog.
pub async fn introspect_url(
    url: &str,
    schema: &str,
) -> Result<std::collections::BTreeMap<String, (TableManifest, Vec<crate::mysql_catalog::ForeignKey>)>, CatalogError> {
    let driver = MysqlDriver::from_url(url)?;
    driver.check_connection().await?;
    driver.introspect(schema).await
}

pub async fn introspect_draft_with_ca(url: &str, schema: &str, ca_cert: Option<&Path>) -> Result<serde_json::Value, CatalogError> {
    let driver = if let Some(ca_cert) = ca_cert { MysqlDriver::from_url_with_ca(url, Some(ca_cert))? } else { MysqlDriver::from_url(url)? };
    driver.check_connection().await?;
    let catalog = driver.introspect(schema).await?;
    render_mysql_draft(schema, &catalog)
}

pub async fn introspect_draft(url: &str, schema: &str) -> Result<serde_json::Value, CatalogError> {
    let driver = MysqlDriver::from_url(url)?;
    driver.check_connection().await?;
    let catalog = driver.introspect(schema).await?;
    render_mysql_draft(schema, &catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mysql_catalog::{Column, Diagnostic, Index, LogicalType};

    /// Live tests drop databases, so the URL must pass the shared dedicated-endpoint guard first.
    fn live_mysql_url() -> String {
        let url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a run-owned MySQL/MariaDB container");
        aip_test_support::validate_mysql_admin_url(&url).unwrap_or_else(|reason| panic!("AIP_TEST_MYSQL_URL is unsafe: {reason}"));
        url
    }

    #[test]
    fn mysql_driver_config_uses_optional_tls_and_pool_settings() {
        let config =
            MysqlDriverConfig::parse_url("mysql://reader:secret@db.example/facereview?sslmode=verify-full&pool_min=2&pool_max=7&connect_timeout=3")
                .unwrap();
        let constraints = PoolConstraints::new(config.pool_min, config.pool_max).unwrap();
        let opts: Opts = OptsBuilder::default()
            .ip_or_hostname(config.host)
            .tcp_port(config.port)
            .user(Some(config.user))
            .pass(config.password)
            .db_name(Some(config.database))
            .pool_opts(PoolOpts::default().with_constraints(constraints))
            .ssl_opts(Some(SslOpts::default()))
            .into();

        assert_eq!(opts.ip_or_hostname(), "db.example");
        assert_eq!(opts.tcp_port(), 3306);
        assert_eq!(opts.user(), Some("reader"));
        assert_eq!(opts.pass(), Some("secret"));
        assert_eq!(opts.db_name(), Some("facereview"));
        assert_eq!(opts.pool_opts().constraints(), PoolConstraints::new(2, 7).unwrap());
        assert!(opts.ssl_opts().is_some());
        assert_eq!(config.connect_timeout_ms, 3_000);
    }

    #[test]
    fn mysql_opts_keep_certificate_and_hostname_validation_with_configured_ca() {
        let config = MysqlDriverConfig::parse_url("mysql://reader:secret@db.example/facereview").unwrap();
        let ca_path = PathBuf::from("/private/tmp/database-ca.pem");
        let opts = build_mysql_opts(&config, Some(ca_path.clone())).unwrap();
        let ssl = opts.ssl_opts().expect("remote connection must enable TLS");
        assert!(!ssl.accept_invalid_certs());
        assert!(!ssl.skip_domain_validation());
        assert_eq!(ssl.root_certs(), &[ca_path.into()]);
    }

    #[tokio::test]
    #[ignore = "requires a disposable TLS-enabled MySQL fixture and generated CA"]
    async fn live_tls_accepts_configured_ca_and_rejects_untrusted_ca_and_hostname_mismatch() {
        let trusted_url = std::env::var("AIP_TEST_MYSQL_TLS_URL").expect("set disposable fixture TLS URL");
        let mismatch_url = std::env::var("AIP_TEST_MYSQL_TLS_MISMATCH_URL").expect("set TLS hostname mismatch URL");
        let ca_path = PathBuf::from(std::env::var_os("AIP_TEST_MYSQL_TLS_CA").expect("set fixture CA path"));
        let untrusted_ca_path = PathBuf::from(std::env::var_os("AIP_TEST_MYSQL_TLS_UNTRUSTED_CA").expect("set an unrelated CA path"));

        MysqlDriver::from_url_with_ca(&trusted_url, Some(&ca_path)).unwrap().check_connection().await.unwrap();
        assert!(MysqlDriver::from_url_with_ca(&trusted_url, None).unwrap().check_connection().await.is_err());
        assert!(MysqlDriver::from_url_with_ca(&trusted_url, Some(&untrusted_ca_path)).unwrap().check_connection().await.is_err());
        assert!(MysqlDriver::from_url_with_ca(&mismatch_url, Some(&ca_path)).unwrap().check_connection().await.is_err());
    }

    #[test]
    fn introspection_draft_contains_definition_and_physical_mapping() {
        let mut catalog = std::collections::BTreeMap::new();
        catalog.insert(
            "users".to_string(),
            (
                TableManifest {
                    name: "users".into(),
                    columns: vec![
                        Column {
                            name: "user_id".into(),
                            ordinal_position: 1,
                            logical_type: LogicalType::Integer { bits: 64, unsigned: false, display_width: None, boolean_alias: false },
                            nullable: false,
                            default: crate::mysql_catalog::CatalogDefault::NullOrAbsent,
                            generated: false,
                        },
                        Column {
                            name: "display_name".into(),
                            ordinal_position: 2,
                            logical_type: LogicalType::Text,
                            nullable: true,
                            default: crate::mysql_catalog::CatalogDefault::Value("guest".into()),
                            generated: false,
                        },
                    ],
                    indexes: vec![Index { name: "PRIMARY".into(), unique: true, primary: true, columns: vec!["user_id".into()] }],
                    diagnostics: vec![Diagnostic { field: "display_name".into(), reason: "synthetic fixture".into() }],
                },
                vec![],
            ),
        );

        let draft = render_mysql_draft("facereview", &catalog).unwrap();
        assert!(draft["definition"].as_str().unwrap().contains("resource Users"));
        assert!(draft["definition"].as_str().unwrap().contains("id: Id"));
        assert_eq!(draft["mapping"]["tables"]["Users"]["table"], "users");
        assert_eq!(draft["mapping"]["tables"]["Users"]["columns"]["id"], "user_id");
    }

    #[test]
    fn introspection_draft_rejects_composite_foreign_keys_instead_of_dropping_them() {
        let integer = || Column {
            name: String::new(),
            ordinal_position: 1,
            logical_type: LogicalType::Integer { bits: 32, unsigned: false, display_width: None, boolean_alias: false },
            nullable: false,
            default: crate::mysql_catalog::CatalogDefault::NullOrAbsent,
            generated: false,
        };
        let table =
            |name: &str, columns: Vec<Column>, indexes: Vec<Index>| TableManifest { name: name.into(), columns, indexes, diagnostics: vec![] };
        let mut parent_id = integer();
        parent_id.name = "id".into();
        let mut left = integer();
        left.name = "left_code".into();
        let mut right = integer();
        right.name = "right_code".into();
        let mut child_id = integer();
        child_id.name = "id".into();
        let foreign_key = crate::mysql_catalog::ForeignKey {
            source_schema: "app".into(),
            source_table: "children".into(),
            name: "fk_children_parent_pair".into(),
            columns: vec!["left_code".into(), "right_code".into()],
            referenced_schema: "app".into(),
            referenced_table: "parents".into(),
            referenced_columns: vec!["left_code".into(), "right_code".into()],
            on_delete: crate::mysql_catalog::ReferentialAction::Cascade,
            on_update: crate::mysql_catalog::ReferentialAction::Restrict,
        };
        let mut catalog = std::collections::BTreeMap::new();
        catalog.insert(
            "parents".into(),
            (
                table(
                    "parents",
                    vec![parent_id, left.clone(), right.clone()],
                    vec![Index { name: "PRIMARY".into(), unique: true, primary: true, columns: vec!["id".into()] }],
                ),
                vec![],
            ),
        );
        catalog.insert(
            "children".into(),
            (
                table(
                    "children",
                    vec![child_id, left, right],
                    vec![Index { name: "PRIMARY".into(), unique: true, primary: true, columns: vec!["id".into()] }],
                ),
                vec![foreign_key],
            ),
        );

        let error = render_mysql_draft("app", &catalog).unwrap_err();
        assert_eq!(error.code, "MYSQL.UNSUPPORTED_COMPOSITE_FK");
        assert!(error.message.contains("fk_children_parent_pair"));
    }

    #[tokio::test]
    #[ignore = "requires a disposable MySQL or MariaDB database seeded with the aip_mysql_test schema"]
    async fn live_mysql_catalog_reads_information_schema_into_cli_draft() {
        // The 4s session limit is asserted below, so the test sets it instead of relying on the caller's URL.
        let base_url = live_mysql_url();
        let url = format!("{base_url}{}statement_timeout=4", if base_url.contains('?') { '&' } else { '?' });
        let driver = MysqlDriver::from_url(&url).unwrap();
        driver.check_connection().await.unwrap();
        let mut conn = driver.pool.get_conn().await.unwrap();
        let statement_timeout_configured: bool = if url.starts_with("mariadb://") {
            conn.query_first("SELECT @@max_statement_time = 4.000").await.unwrap().unwrap()
        } else {
            conn.query_first("SELECT @@max_execution_time = 4000").await.unwrap().unwrap()
        };
        assert!(statement_timeout_configured);
        drop(conn);
        let draft = crate::external::introspect(&url, "aip_mysql_test").await.unwrap();
        assert_eq!(draft["ok"], true);
        assert!(draft["definition"].as_str().unwrap().contains("resource Users"));
        assert_eq!(draft["mapping"]["tables"]["Users"]["table"], "users");
        assert_eq!(draft["mapping"]["tables"]["Posts"]["columns"]["user"], "user_id");
        assert!(draft["catalog"]["posts"].is_array());
        assert_eq!(draft["catalog"]["posts"][0]["name"], "posts");
    }

    #[tokio::test]
    #[ignore = "requires a disposable MySQL or MariaDB database seeded with aip_mysql_test"]
    async fn live_catalog_fence_rechecks_schema_and_rejects_drift_without_a_cache() {
        let url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a disposable test database URL");
        let ca = PathBuf::from(std::env::var_os("AIP_TEST_MYSQL_TLS_CA").expect("set AIP_TEST_MYSQL_TLS_CA to the disposable CA path"));
        let driver = MysqlDriver::from_url_with_ca(&url, Some(&ca)).unwrap();
        let mut conn = driver.pool.get_conn().await.unwrap();
        let prior_probe: Option<u8> = conn
            .exec_first(
                "SELECT 1 FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND COLUMN_NAME = ?",
                ("aip_mysql_test", "users", "fence_probe"),
            )
            .await
            .unwrap();
        if prior_probe.is_some() {
            conn.query_drop("ALTER TABLE aip_mysql_test.users DROP COLUMN fence_probe").await.unwrap();
        }
        drop(conn);
        let baseline = driver.catalog_fingerprint("aip_mysql_test").await.unwrap();
        driver.verify_catalog_fingerprint("aip_mysql_test", &baseline).await.unwrap();

        let mut conn = driver.pool.get_conn().await.unwrap();
        conn.query_drop("ALTER TABLE aip_mysql_test.users ADD COLUMN fence_probe INT NULL").await.unwrap();
        drop(conn);

        let error = driver.verify_catalog_fingerprint("aip_mysql_test", &baseline).await.unwrap_err();
        assert!(error.to_string().contains("schema changed"));
        let mut conn = driver.pool.get_conn().await.unwrap();
        conn.query_drop("ALTER TABLE aip_mysql_test.users DROP COLUMN fence_probe").await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a disposable MySQL or MariaDB database URL in AIP_TEST_MYSQL_URL"]
    async fn live_information_schema_reads_do_not_pin_target_table_metadata_for_the_business_transaction() {
        let url = live_mysql_url();
        let driver = MysqlDriver::from_url(&url).unwrap();
        let catalog_table = "aip_stage6_catalog_mdl_probe";
        let business_table = "aip_stage6_business_mdl_probe";

        let mut setup = driver.pool.get_conn().await.unwrap();
        setup.query_drop(format!("DROP TABLE IF EXISTS `{catalog_table}`, `{business_table}`")).await.unwrap();
        setup.query_drop(format!("CREATE TABLE `{catalog_table}` (id INT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
        setup.query_drop(format!("CREATE TABLE `{business_table}` (id INT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
        drop(setup);

        // A transaction snapshot over INFORMATION_SCHEMA is not a metadata lock on the business table.
        let mut catalog_reader = driver.pool.get_conn().await.unwrap();
        catalog_reader.query_drop("START TRANSACTION").await.unwrap();
        let _: Vec<String> = catalog_reader
            .exec(
                "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
                (&catalog_table,),
            )
            .await
            .unwrap();
        let mut catalog_ddl = driver.pool.get_conn().await.unwrap();
        catalog_ddl.query_drop("SET SESSION lock_wait_timeout = 1").await.unwrap();
        catalog_ddl.query_drop(format!("ALTER TABLE `{catalog_table}` ADD COLUMN catalog_probe INT NULL")).await.unwrap();
        catalog_reader.query_drop("COMMIT").await.unwrap();
        drop(catalog_ddl);
        drop(catalog_reader);

        // A real business-table read in the same transaction does retain MDL until transaction end.
        let mut business_reader = driver.pool.get_conn().await.unwrap();
        business_reader.query_drop("START TRANSACTION").await.unwrap();
        let _: Option<u32> = business_reader.exec_first(format!("SELECT id FROM `{business_table}` LIMIT 1"), ()).await.unwrap();
        let mut business_ddl = driver.pool.get_conn().await.unwrap();
        business_ddl.query_drop("SET SESSION lock_wait_timeout = 1").await.unwrap();
        assert!(business_ddl.query_drop(format!("ALTER TABLE `{business_table}` ADD COLUMN business_probe INT NULL")).await.is_err());
        business_reader.query_drop("COMMIT").await.unwrap();
        business_ddl.query_drop(format!("ALTER TABLE `{business_table}` ADD COLUMN business_probe INT NULL")).await.unwrap();
        business_ddl.query_drop(format!("DROP TABLE `{catalog_table}`, `{business_table}`")).await.unwrap();
    }

    #[test]
    fn test_canonical_bound_tables_sorting_and_deduplication() {
        let meta = "aip_meta";
        let source = "app_source";
        let catalog = vec!["users".to_string(), "orders".to_string(), "users".to_string()];

        let read_tables = canonical_bound_tables(meta, source, &catalog, false);
        assert_eq!(
            read_tables,
            vec![
                ("aip_meta".to_string(), "aip_migrate_meta".to_string()),
                ("aip_meta".to_string(), "aip_principals".to_string()),
                ("app_source".to_string(), "orders".to_string()),
                ("app_source".to_string(), "users".to_string()),
            ]
        );

        let write_tables = canonical_bound_tables(meta, source, &catalog, true);
        assert!(write_tables.contains(&("aip_meta".to_string(), "aip_idem".to_string())));
        assert!(write_tables.contains(&("aip_meta".to_string(), "aip_audit".to_string())));
        assert!(write_tables.contains(&("aip_meta".to_string(), "aip_events".to_string())));
        assert!(write_tables.contains(&("aip_meta".to_string(), "aip_tx_locks".to_string())));
        assert!(write_tables.contains(&("aip_meta".to_string(), "aip_jobs".to_string())));
        assert!(write_tables.contains(&("aip_meta".to_string(), "aip_outbox".to_string())));
        // Verify deterministic sorting: every element <= next element
        for window in write_tables.windows(2) {
            assert!(window[0] < window[1]);
        }
    }

    #[test]
    fn test_is_drift_error_detection() {
        let err_table = mysql_async::Error::Server(mysql_async::ServerError {
            code: 1146,
            message: "Table 'test.absent' doesn't exist".into(),
            state: "42S02".into(),
        });
        assert!(is_drift_error(&err_table));

        let err_field = mysql_async::Error::Server(mysql_async::ServerError {
            code: 1054,
            message: "Unknown column 'col' in 'field list'".into(),
            state: "42S22".into(),
        });
        assert!(is_drift_error(&err_field));

        let err_def = mysql_async::Error::Server(mysql_async::ServerError {
            code: 1412,
            message: "Table definition has changed, please retry transaction".into(),
            state: "HY000".into(),
        });
        assert!(is_drift_error(&err_def));

        let err_timeout =
            mysql_async::Error::Server(mysql_async::ServerError { code: 1205, message: "Lock wait timeout exceeded".into(), state: "HY000".into() });
        assert!(!is_drift_error(&err_timeout));
    }

    #[tokio::test]
    #[ignore = "requires live MySQL/MariaDB via AIP_TEST_MYSQL_URL"]
    async fn live_pre_mdl_pin_blocks_concurrent_ddl_and_detects_prior_ddl_on_same_tx() {
        let url = live_mysql_url();
        let driver = MysqlDriver::connect(&url).await.expect("connect driver");
        let pool = mysql_async::Pool::new(driver.opts().clone());

        let schema = "aip_stage6_fenced_tx_test";
        let mut setup_conn = pool.get_conn().await.expect("setup conn");
        setup_conn.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.expect("drop old db");
        setup_conn.query_drop(format!("CREATE DATABASE `{schema}`")).await.expect("create db");
        setup_conn
            .query_drop(format!(
                "CREATE TABLE `{schema}`.`users` (
                    id VARCHAR(36) NOT NULL PRIMARY KEY,
                    email VARCHAR(255) NOT NULL
                ) ENGINE=InnoDB"
            ))
            .await
            .expect("create users");
        setup_conn
            .query_drop(format!(
                "CREATE TABLE `{schema}`.`comments` (
                    id VARCHAR(36) NOT NULL PRIMARY KEY,
                    user_id VARCHAR(36) NOT NULL,
                    body TEXT NOT NULL,
                    CONSTRAINT fk_comments_user FOREIGN KEY (user_id) REFERENCES `{schema}`.`users` (id)
                ) ENGINE=InnoDB"
            ))
            .await
            .expect("create comments");

        let baseline = driver.catalog_fingerprint(schema).await.expect("baseline fingerprint");

        let pin_sql = format!(
            "SELECT 1 FROM (SELECT 1 FROM `{schema}`.`comments` LIMIT 1) AS p0 \
             UNION ALL \
             SELECT 1 FROM (SELECT 1 FROM `{schema}`.`users` LIMIT 1) AS p1"
        );

        // -------------------------------------------------------------------------
        // CASE A (DURING + ROLLBACK): Pre-MDL pin blocks concurrent ALTER on any
        // bound table while tx is open; ROLLBACK immediately releases MDL.
        // -------------------------------------------------------------------------
        let mut tx_conn = pool.get_conn().await.expect("tx conn");
        tx_conn.query_drop("START TRANSACTION READ ONLY").await.expect("start ro tx");
        let pinned: Vec<i64> = tx_conn.query(&pin_sql).await.expect("pin bound tables");
        assert!(pinned.is_empty());

        let mut ddl_conn = pool.get_conn().await.expect("ddl conn");
        ddl_conn.query_drop("SET SESSION lock_wait_timeout = 1").await.expect("set lock_wait_timeout = 1");

        let alter_comments = ddl_conn
            .query_drop(format!("ALTER TABLE `{schema}`.`comments` ADD COLUMN flagged TINYINT(1) NOT NULL DEFAULT 0"))
            .await
            .expect_err("ALTER on pinned comments table must time out");
        assert!(alter_comments.to_string().contains("Lock wait timeout exceeded"));

        let alter_users = ddl_conn
            .query_drop(format!("ALTER TABLE `{schema}`.`users` ADD COLUMN nickname VARCHAR(64) NULL"))
            .await
            .expect_err("ALTER on pinned users table must time out");
        assert!(alter_users.to_string().contains("Lock wait timeout exceeded"));

        // Business query inside the same transaction sees unmutated schema.
        let rows: Vec<(String, String)> = tx_conn
            .query(format!("SELECT c.id, u.email FROM `{schema}`.`comments` c JOIN `{schema}`.`users` u ON c.user_id = u.id"))
            .await
            .expect("business join inside fenced tx");
        assert!(rows.is_empty());

        // Explicit ROLLBACK releases MDL immediately.
        tx_conn.query_drop("ROLLBACK").await.expect("rollback fenced tx");

        // -------------------------------------------------------------------------
        // CASE B (AFTER ROLLBACK / BEFORE NEXT TX): External ALTER now succeeds,
        // and the very next fenced check immediately rejects drift with zero cache.
        // -------------------------------------------------------------------------
        ddl_conn
            .query_drop(format!("ALTER TABLE `{schema}`.`users` ADD COLUMN nickname VARCHAR(64) NULL"))
            .await
            .expect("ALTER succeeds immediately after ROLLBACK");

        let drift_err = driver.verify_catalog_fingerprint(schema, &baseline).await.expect_err("drift must be rejected immediately on next request");
        assert_eq!(drift_err.code, "MIGRATE.DRIFT_DETECTED");

        // -------------------------------------------------------------------------
        // CASE C (BEFORE TX - DROP TABLE): If a bound table is dropped before pin,
        // the pin query immediately fails with 1146 ER_NO_SUCH_TABLE / drift.
        // -------------------------------------------------------------------------
        ddl_conn.query_drop(format!("DROP TABLE `{schema}`.`comments`")).await.expect("drop comments");

        tx_conn.query_drop("START TRANSACTION READ ONLY").await.expect("start ro tx after drop");
        let drop_err = tx_conn.query::<i64, _>(&pin_sql).await.expect_err("pin must fail when bound table was dropped");
        assert!(
            drop_err.to_string().contains("1146") || drop_err.to_string().contains("doesn't exist"),
            "expected ER_NO_SUCH_TABLE (1146), got: {drop_err}"
        );
        tx_conn.query_drop("ROLLBACK").await.expect("rollback after 1146");

        setup_conn.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.expect("cleanup db");
        drop(setup_conn);
    }

    #[tokio::test]
    #[ignore = "requires live MySQL/MariaDB via AIP_TEST_MYSQL_URL"]
    async fn live_begin_fenced_transaction_detects_unrelated_create_table_on_next_request() {
        let url = live_mysql_url();
        let driver = MysqlDriver::connect(&url).await.expect("connect driver");
        let pool = mysql_async::Pool::new(driver.opts().clone());

        let schema = "aip_stage6_unrelated_create_test";
        let mut setup_conn = pool.get_conn().await.expect("setup conn");
        setup_conn.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.expect("drop old db");
        setup_conn.query_drop(format!("CREATE DATABASE `{schema}`")).await.expect("create db");

        // Set up meta table
        setup_conn
            .query_drop(format!(
                "CREATE TABLE `{schema}`.`aip_migrate_meta` (
                    singleton TINYINT(1) PRIMARY KEY DEFAULT 1,
                    facts_digest VARCHAR(64) NOT NULL,
                    schema_sha256 VARCHAR(64) NOT NULL,
                    status VARCHAR(32) NOT NULL
                ) ENGINE=InnoDB"
            ))
            .await
            .expect("create meta table");

        let facts_digest = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        setup_conn
            .query_drop(format!(
                "INSERT INTO `{schema}`.`aip_migrate_meta` (singleton, facts_digest, schema_sha256, status) \
                 VALUES (1, '{facts_digest}', 'baseline_sha', 'READY')"
            ))
            .await
            .expect("seed meta row");

        setup_conn
            .query_drop(format!(
                "CREATE TABLE `{schema}`.`aip_principals` (
                    id VARCHAR(36) PRIMARY KEY
                ) ENGINE=InnoDB"
            ))
            .await
            .expect("create aip_principals");

        setup_conn
            .query_drop(format!(
                "CREATE TABLE `{schema}`.`items` (
                    id INT PRIMARY KEY,
                    title VARCHAR(100) NOT NULL
                ) ENGINE=InnoDB"
            ))
            .await
            .expect("create items");

        let baseline = driver.catalog_fingerprint(schema).await.expect("baseline fingerprint");
        let fence = MysqlDeploymentFence::new(schema, schema, &["items".to_string()], facts_digest, &baseline).expect("fence");

        // 1. First fenced transaction succeeds
        let tx = driver.begin_fenced_transaction(true, &fence).await.expect("initial fenced tx must succeed");
        tx.commit().await.expect("commit fenced tx");

        // 2. External session creates an unrelated new table in source_schema
        let mut ddl_conn = pool.get_conn().await.expect("ddl conn");
        ddl_conn
            .query_drop(format!(
                "CREATE TABLE `{schema}`.`unrelated_external_table` (
                    id INT PRIMARY KEY,
                    extra VARCHAR(50)
                ) ENGINE=InnoDB"
            ))
            .await
            .expect("create unrelated table");

        // 3. Very next request calls begin_fenced_transaction with baseline fingerprint
        // and must be rejected immediately (zero cache) with MIGRATE.DRIFT_DETECTED
        let err = driver
            .begin_fenced_transaction(true, &fence)
            .await
            .expect_err("unrelated CREATE TABLE must cause next request to fail closed with drift");
        assert_eq!(err.code, "MIGRATE.DRIFT_DETECTED");
        assert!(err.to_string().contains("schema changed"));

        setup_conn.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.expect("cleanup db");
    }

    #[tokio::test]
    #[ignore = "requires live MySQL/MariaDB via AIP_TEST_MYSQL_URL"]
    async fn live_begin_fenced_transaction_validates_meta_marker_status_and_digest() {
        let url = live_mysql_url();
        let driver = MysqlDriver::connect(&url).await.expect("connect driver");
        let pool = mysql_async::Pool::new(driver.opts().clone());

        let schema = "aip_stage6_meta_marker_test";
        let mut setup_conn = pool.get_conn().await.expect("setup conn");
        setup_conn.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.expect("drop old db");
        setup_conn.query_drop(format!("CREATE DATABASE `{schema}`")).await.expect("create db");

        setup_conn
            .query_drop(format!(
                "CREATE TABLE `{schema}`.`aip_migrate_meta` (
                    singleton TINYINT(1) PRIMARY KEY DEFAULT 1,
                    facts_digest VARCHAR(64) NOT NULL,
                    schema_sha256 VARCHAR(64) NOT NULL,
                    status VARCHAR(32) NOT NULL
                ) ENGINE=InnoDB"
            ))
            .await
            .expect("create meta table");

        let good_facts_digest = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let wrong_facts_digest = "0000000000000000000000000000000000000000000000000000000000000000";

        setup_conn
            .query_drop(format!(
                "INSERT INTO `{schema}`.`aip_migrate_meta` (singleton, facts_digest, schema_sha256, status) \
                 VALUES (1, '{good_facts_digest}', 'baseline_sha', 'READY')"
            ))
            .await
            .expect("seed meta row");

        setup_conn
            .query_drop(format!(
                "CREATE TABLE `{schema}`.`aip_principals` (
                    id VARCHAR(36) PRIMARY KEY
                ) ENGINE=InnoDB"
            ))
            .await
            .expect("create aip_principals");

        let baseline = driver.catalog_fingerprint(schema).await.expect("baseline fingerprint");
        // Meta and source share one schema here, so the catalog tables are exactly the two meta tables.
        let source_tables = vec![META_TABLE.to_string(), "aip_principals".to_string()];
        let good_fence = MysqlDeploymentFence::new(schema, schema, &source_tables, good_facts_digest, &baseline).expect("good fence");
        let wrong_fence = MysqlDeploymentFence::new(schema, schema, &source_tables, wrong_facts_digest, &baseline).expect("wrong fence");

        // 1. Success case with correct facts digest and READY status
        let tx = driver.begin_fenced_transaction(true, &good_fence).await.expect("fenced tx with correct facts and status must succeed");
        tx.commit().await.expect("commit fenced tx");

        // 2. Reject mismatched facts digest
        let err_digest = driver.begin_fenced_transaction(true, &wrong_fence).await.expect_err("mismatched facts_digest must fail closed");
        assert_eq!(err_digest.code, "MIGRATE.DRIFT_DETECTED");

        // 3. Reject non-READY status
        setup_conn
            .query_drop(format!("UPDATE `{schema}`.`aip_migrate_meta` SET status = 'MIGRATING' WHERE singleton = 1"))
            .await
            .expect("update status to non-READY");

        let err_status = driver.begin_fenced_transaction(true, &good_fence).await.expect_err("non-READY status must fail closed");
        assert_eq!(err_status.code, "MIGRATE.DRIFT_DETECTED");

        setup_conn.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.expect("cleanup db");
    }
}
