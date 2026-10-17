//! MySQL 8.4 implementation of the transport's [`RelationalBackend`] (WBS 7.2.1.1: read path).
//!
//! Every request opens one fenced, read-only transaction (`MysqlDeploymentFence`): metadata locks on every bound
//! table first, then the deployment marker and the schema-wide catalog fingerprint in the same transaction, then the
//! policy query. DDL that lands between two requests is therefore rejected on the next request without any cache
//! TTL, and DDL that arrives mid-request waits on the metadata lock until the transaction ends.
use crate::mysql_catalog::CatalogError;
use crate::mysql_driver::{MysqlDeploymentFence, MysqlDriver};
use mysql_async::Row;
use mysql_async::prelude::Queryable;
use serde_json::Value;
use spike_v2_read::backend::{BackendFuture, RelationalBackend};
use spike_v2_read::id_wire::IdWire;
use spike_v2_read::plan::{Caller, MAX_OUTPUT_BYTES, Plan, Reject};
use spike_v2_read::sqlgen::Dialect;

pub struct MysqlBackend {
    driver: MysqlDriver,
    fence: MysqlDeploymentFence,
}

/// Public error code for a failed fence entry. Details stay out of the response.
fn fence_code(error: &CatalogError) -> &'static str {
    match error.code.as_str() {
        "MIGRATE.DRIFT_DETECTED" => "WIRE.DEPLOYMENT_CHANGED",
        _ => "WIRE.DB_UNAVAILABLE",
    }
}

fn reject(code: &'static str, msg: &str) -> Reject {
    Reject { code, msg: msg.into() }
}

impl MysqlBackend {
    pub fn new(driver: MysqlDriver, fence: MysqlDeploymentFence) -> Self {
        Self { driver, fence }
    }

    pub fn driver(&self) -> &MysqlDriver {
        &self.driver
    }

    /// What the server can do, found by running each statement shape the adapter relies on instead of reading a version string.
    pub async fn capabilities(&self) -> Result<ServerCapabilities, CatalogError> {
        probe_capabilities(&self.driver).await
    }

    /// Server profile the adapter was written and verified against. A server that lacks a capability it relies on is refused at
    /// start with the missing capabilities named, instead of producing different results silently: `LATERAL` for traverse,
    /// `FOR SHARE OF` for policy locks, `SKIP LOCKED` for the job store, enforced `CHECK`, InnoDB, strict mode.
    pub async fn check_server_profile(&self) -> Result<(), CatalogError> {
        let capabilities = self.capabilities().await?;
        let missing = capabilities.missing();
        if !missing.is_empty() {
            return Err(CatalogError::with_code(
                "DB_ADAPTER_UNSUPPORTED",
                format!("{} {} lacks what the MySQL adapter relies on: {}", capabilities.engine, capabilities.version, missing.join(", ")),
            ));
        }
        Ok(())
    }
}

/// Capabilities the adapter relies on, each established by executing a statement on the live server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerCapabilities {
    /// `MySQL` or `MariaDB`.
    pub engine: &'static str,
    pub version: String,
    pub major_minor_ok: bool,
    pub strict_mode: bool,
    pub lateral: bool,
    pub skip_locked: bool,
    pub for_share_of: bool,
    pub json_arrayagg: bool,
    pub check_enforced: bool,
    pub innodb: bool,
}

impl ServerCapabilities {
    /// Names of the capabilities the adapter needs and this server does not have.
    pub fn missing(&self) -> Vec<&'static str> {
        [
            (self.major_minor_ok, "MySQL 8.4 or newer"),
            (self.strict_mode, "sql_mode STRICT_TRANS_TABLES"),
            (self.lateral, "LATERAL joins"),
            (self.skip_locked, "FOR UPDATE SKIP LOCKED"),
            (self.for_share_of, "FOR SHARE OF"),
            (self.json_arrayagg, "JSON_ARRAYAGG"),
            (self.check_enforced, "enforced CHECK constraints"),
            (self.innodb, "InnoDB"),
        ]
        .into_iter()
        .filter_map(|(ok, name)| (!ok).then_some(name))
        .collect()
    }
}

impl ServerCapabilities {
    /// What the durable job store and the outbox relay need, which is less than the read and write paths: no `LATERAL`, no
    /// `FOR SHARE OF`. MariaDB 11.4 has all of it, MySQL needs 8.4 or newer.
    pub fn missing_for_job_store(&self) -> Vec<&'static str> {
        let version_ok = self.engine == "MariaDB" || self.major_minor_ok;
        [
            (version_ok, "MySQL 8.4 or newer"),
            (self.strict_mode, "sql_mode STRICT_TRANS_TABLES"),
            (self.skip_locked, "FOR UPDATE SKIP LOCKED"),
            (self.json_arrayagg, "JSON functions"),
            (self.check_enforced, "enforced CHECK constraints"),
            (self.innodb, "InnoDB"),
        ]
        .into_iter()
        .filter_map(|(ok, name)| (!ok).then_some(name))
        .collect()
    }
}

/// Refuse a server the job store cannot run on, naming what is missing. Probes on the given connection pool.
pub async fn check_job_store_profile(driver: &MysqlDriver) -> Result<ServerCapabilities, CatalogError> {
    let capabilities = probe_capabilities(driver).await?;
    let missing = capabilities.missing_for_job_store();
    if !missing.is_empty() {
        return Err(CatalogError::with_code(
            "DB_ADAPTER_UNSUPPORTED",
            format!("{} {} lacks what the job store relies on: {}", capabilities.engine, capabilities.version, missing.join(", ")),
        ));
    }
    Ok(capabilities)
}

/// Run one probe statement; any server error means the capability is absent.
async fn works(conn: &mut mysql_async::Conn, sql: &str) -> bool {
    conn.query_drop(sql).await.is_ok()
}

/// Probe the server on one connection with a session-local temporary table, leaving nothing behind.
pub async fn probe_capabilities(driver: &MysqlDriver) -> Result<ServerCapabilities, CatalogError> {
    let mut conn = driver.pool().get_conn().await.map_err(|_| CatalogError::new("MySQL connection failed"))?;
    let row: Option<(String, String)> =
        conn.query_first("SELECT VERSION(), @@sql_mode").await.map_err(|_| CatalogError::new("MySQL profile query failed"))?;
    let (version, sql_mode) = row.ok_or_else(|| CatalogError::new("MySQL profile query returned no row"))?;
    let engine = if version.to_ascii_lowercase().contains("mariadb") { "MariaDB" } else { "MySQL" };
    let mut parts = version.split(['.', '-']);
    let (major, minor) = (parts.next().and_then(|v| v.parse::<u32>().ok()), parts.next().and_then(|v| v.parse::<u32>().ok()));
    let major_minor_ok = engine == "MySQL" && matches!((major, minor), (Some(major), Some(minor)) if (major, minor) >= (8, 4));
    let strict_mode = sql_mode.split(',').any(|mode| matches!(mode.trim(), "STRICT_TRANS_TABLES" | "STRICT_ALL_TABLES"));
    // The probe needs a session-local table. A runtime account without that privilege would look like a server without InnoDB, so
    // a privilege error is reported as what it is rather than as a missing capability.
    let innodb = match conn
        .query_drop("CREATE TEMPORARY TABLE aip_capability_probe (id INT PRIMARY KEY, CONSTRAINT aip_probe_positive CHECK (id > 0)) ENGINE=InnoDB")
        .await
    {
        Ok(()) => true,
        Err(mysql_async::Error::Server(server)) if matches!(server.code, 1044 | 1142 | 1143) => {
            return Err(CatalogError::with_code(
                "DB_ADAPTER_UNSUPPORTED",
                "the database account lacks CREATE TEMPORARY TABLES, which the start-up capability probe needs (it only creates a session-local table); grant it on the business schema",
            ));
        }
        Err(_) => false,
    };
    let lateral = works(&mut conn, "SELECT 1 FROM (SELECT 1 AS a) x JOIN LATERAL (SELECT x.a AS b) y ON TRUE").await;
    let json_arrayagg = works(&mut conn, "SELECT JSON_ARRAYAGG(JSON_OBJECT('a', 1))").await;
    let (mut skip_locked, mut for_share_of) = (false, false);
    if innodb && works(&mut conn, "START TRANSACTION").await {
        skip_locked = works(&mut conn, "SELECT id FROM aip_capability_probe FOR UPDATE SKIP LOCKED").await;
        for_share_of = works(&mut conn, "SELECT id FROM aip_capability_probe p FOR SHARE OF p").await;
        let _ = conn.query_drop("ROLLBACK").await;
    }
    // A CHECK that is parsed but not enforced would let invalid rows in silently.
    let check_enforced = innodb && !works(&mut conn, "INSERT INTO aip_capability_probe (id) VALUES (0)").await;
    let _ = conn.query_drop("DROP TEMPORARY TABLE IF EXISTS aip_capability_probe").await;
    Ok(ServerCapabilities { engine, version, major_minor_ok, strict_mode, lateral, skip_locked, for_share_of, json_arrayagg, check_enforced, innodb })
}

impl RelationalBackend for MysqlBackend {
    fn dialect(&self) -> Dialect {
        Dialect::MySql
    }

    fn adapter(&self) -> &'static str {
        "mysql"
    }

    fn ready(&self) -> BackendFuture<'_, Result<(), &'static str>> {
        Box::pin(async move {
            let tx = self.driver.begin_fenced_transaction(true, &self.fence).await.map_err(|error| fence_code(&error))?;
            tx.rollback().await.map_err(|_| "WIRE.DB_UNAVAILABLE")
        })
    }

    fn now(&self) -> BackendFuture<'_, Result<String, &'static str>> {
        Box::pin(async move {
            let mut conn = self.driver.pool().get_conn().await.map_err(|_| "WIRE.DB_UNAVAILABLE")?;
            let now: Option<String> =
                conn.query_first("SELECT DATE_FORMAT(UTC_TIMESTAMP(6), '%Y-%m-%dT%H:%i:%s.%fZ')").await.map_err(|_| "WIRE.DB_UNAVAILABLE")?;
            now.ok_or("WIRE.DB_UNAVAILABLE")
        })
    }

    fn apply<'a>(&'a self, facts: &'a Value, body: &'a Value, caller: &'a Caller, wire: IdWire) -> BackendFuture<'a, Value> {
        Box::pin(crate::mysql_write::handle_apply(&self.driver, &self.fence, facts, body, caller, wire))
    }

    fn status<'a>(&'a self, body: &'a Value, caller: &'a Caller, wire: IdWire) -> BackendFuture<'a, Value> {
        Box::pin(crate::mysql_write::handle_status(&self.driver, body, caller, wire))
    }

    fn read<'a>(&'a self, plan: &'a Plan, _wire: IdWire) -> BackendFuture<'a, Result<Vec<Value>, Reject>> {
        Box::pin(async move {
            let mut tx = self.driver.begin_fenced_transaction(true, &self.fence).await.map_err(|error| match fence_code(&error) {
                "WIRE.DEPLOYMENT_CHANGED" => reject("WIRE.DEPLOYMENT_CHANGED", "배포 정의 또는 외부 업무 schema가 바뀜"),
                code => reject(code, "DB 연결 또는 배포 펜스 확인 실패"),
            })?;
            let rows: Vec<Row> = {
                let conn = tx.as_mut().map_err(|_| reject("WIRE.INTERNAL", "트랜잭션 없음"))?;
                // TIMESTAMP columns are rendered in the session zone; the read contract is UTC.
                conn.query_drop(format!("SET SESSION time_zone = '+00:00', SESSION max_execution_time = {}", plan.deadline_ms))
                    .await
                    .map_err(|_| reject("WIRE.INTERNAL", "세션 설정 실패"))?;
                let params: Vec<mysql_async::Value> =
                    plan.params.iter().map(|value| value.as_ref().map_or(mysql_async::Value::NULL, |text| text.as_str().into())).collect();
                match conn.exec(plan.sql.as_str(), params).await {
                    Ok(rows) => rows,
                    Err(error) => {
                        let code = match &error {
                            mysql_async::Error::Server(server) => server.code,
                            _ => 0,
                        };
                        return Err(match code {
                            // ER_QUERY_TIMEOUT
                            3024 => Reject { code: "WIRE.DEADLINE_EXCEEDED", msg: format!("{}ms 안에 끝나지 않음", plan.deadline_ms) },
                            // truncated/invalid value, out of range, data too long
                            1292 | 1366 | 1264 | 1265 | 1406 => reject("WIRE.BAD_VALUE", "값 변환 실패"),
                            1146 | 1054 | 1412 => reject("WIRE.DEPLOYMENT_CHANGED", "배포 정의 또는 외부 업무 schema가 바뀜"),
                            _ => Reject { code: "WIRE.INTERNAL", msg: format!("실행 실패(mysql errno {code})") },
                        });
                    }
                }
            };
            let _ = tx.commit().await;
            let mut bytes = 0usize;
            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let cell: Option<Option<String>> = row.get_opt(0).and_then(Result::ok);
                match cell.flatten() {
                    Some(text) => {
                        bytes += text.len();
                        if bytes > MAX_OUTPUT_BYTES {
                            return Err(Reject {
                                code: "WIRE.OUTPUT_TOO_LARGE", msg: format!("응답이 {} bytes 상한을 넘음", MAX_OUTPUT_BYTES)
                            });
                        }
                        let value: Value = serde_json::from_str(&text).map_err(|_| reject("WIRE.INTERNAL", "결과 JSON 해석 실패"))?;
                        out.push(spike_v2_read::mysql_rows::normalize(value, &plan.output_type)?);
                    }
                    None if plan.null_is_denied => return Err(reject("WIRE.ACCESS_DENIED", "집계 접근 권한 없음")),
                    None => out.push(Value::Null),
                }
            }
            Ok(out)
        })
    }
}
