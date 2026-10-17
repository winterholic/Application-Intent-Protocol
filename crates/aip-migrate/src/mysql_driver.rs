//! Optional live MySQL/MariaDB catalog adapter backed by `mysql_async`.
use crate::mysql_catalog::{
    CatalogError, ColumnRow, ForeignKeyRow, IndexRow, MysqlCatalogReader, MysqlDriverConfig, TableManifest, introspect_schema,
};
use mysql_async::prelude::Queryable;
use mysql_async::{Opts, OptsBuilder, Pool, PoolConstraints, PoolOpts, Row, SslOpts, from_row_opt};
use std::path::PathBuf;
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
    for (table_name, _) in catalog {
        let resource = logical_name(table_name);
        if !valid_ident(&resource) || resource.starts_with("Aip") || mapping.tables.contains_key(&resource) {
            return Err(CatalogError::new("resource names are ambiguous; manual mapping is required"));
        }
        mapping.tables.insert(resource, spike_v2_read::external::Table { table: table_name.clone(), columns: Default::default() });
    }

    let mut definition = String::from("// Introspection draft: review authorization before publishing read/write.\nactor principal\n");
    for (resource, mapped) in &mut mapping.tables {
        let (manifest, foreign_keys) = catalog.get(&mapped.table).ok_or_else(|| CatalogError::new("catalog table disappeared during mapping"))?;
        let primary = manifest.indexes.iter().find(|index| index.primary).ok_or_else(|| CatalogError::new("table has no primary key"))?;
        if primary.columns.len() != 1 {
            return Err(CatalogError::new("composite primary keys require a manual definition"));
        }
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
    connect_timeout: Duration,
}

impl MysqlDriver {
    /// Parse server configuration from the URL and create a real driver pool.
    /// `AIP_MYSQL_CA_CERT` may point to an additional trusted PEM/DER CA bundle.
    pub fn from_url(url: &str) -> Result<Self, CatalogError> {
        let config = MysqlDriverConfig::parse_url(url)?;
        let constraints =
            PoolConstraints::new(config.pool_min, config.pool_max).ok_or_else(|| CatalogError::new("invalid MySQL pool constraints"))?;
        let ca_cert = std::env::var_os("AIP_MYSQL_CA_CERT").map(PathBuf::from);
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
            .ip_or_hostname(config.host)
            .tcp_port(config.port)
            .user(Some(config.user))
            .pass(config.password)
            .db_name(Some(config.database))
            .pool_opts(PoolOpts::default().with_constraints(constraints))
            .ssl_opts(tls)
            .init(vec![statement_timeout_query])
            .into();
        Ok(Self { pool: Pool::new(opts), connect_timeout: Duration::from_millis(config.connect_timeout_ms) })
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

    async fn query_rows(&self, query: &str, params: impl Into<mysql_async::Params> + Send) -> Result<Vec<Row>, CatalogError> {
        let mut conn = tokio::time::timeout(self.connect_timeout, self.pool.get_conn())
            .await
            .map_err(|_| CatalogError::new("MySQL connection timed out"))?
            .map_err(|_| CatalogError::new("MySQL connection failed"))?;
        conn.exec(query, params).await.map_err(|_| CatalogError::new("MySQL catalog query failed"))
    }
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
        let rows = self.query_rows(crate::mysql_catalog::SQL_STATISTICS, (schema, table)).await?;
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

    #[tokio::test]
    #[ignore = "requires a disposable MySQL or MariaDB database seeded with the aip_mysql_test schema"]
    async fn live_mysql_catalog_reads_information_schema_into_cli_draft() {
        let url = std::env::var("AIP_TEST_MYSQL_URL").expect("set AIP_TEST_MYSQL_URL to a disposable test database URL");
        let driver = MysqlDriver::from_url(&url).unwrap();
        driver.check_connection().await.unwrap();
        let mut conn = driver.pool.get_conn().await.unwrap();
        let statement_timeout: u64 = conn.query_first("SELECT @@max_execution_time").await.unwrap().unwrap();
        assert_eq!(statement_timeout, 4_000);
        drop(conn);
        let draft = crate::external::introspect(&url, "aip_mysql_test").await.unwrap();
        assert_eq!(draft["ok"], true);
        assert!(draft["definition"].as_str().unwrap().contains("resource Users"));
        assert_eq!(draft["mapping"]["tables"]["Users"]["table"], "users");
        assert_eq!(draft["mapping"]["tables"]["Posts"]["columns"]["user"], "user_id");
        assert!(draft["catalog"]["posts"].is_array());
        assert_eq!(draft["catalog"]["posts"][0]["name"], "posts");
    }
}
