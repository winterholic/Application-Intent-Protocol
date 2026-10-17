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
use spike_v2_read::plan::{MAX_OUTPUT_BYTES, Plan, Reject};
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

    /// Server profile the read path was written and verified against. Anything else is refused at start instead of
    /// producing different results silently: `LATERAL` needs MySQL 8.0.14+, MariaDB has no `LATERAL` at all, and the
    /// timestamp/text semantics assume InnoDB with strict mode.
    pub async fn check_server_profile(&self) -> Result<(), CatalogError> {
        let mut conn = self.driver.pool().get_conn().await.map_err(|_| CatalogError::new("MySQL connection failed"))?;
        let row: Option<(String, String)> =
            conn.query_first("SELECT VERSION(), @@sql_mode").await.map_err(|_| CatalogError::new("MySQL profile query failed"))?;
        let (version, sql_mode) = row.ok_or_else(|| CatalogError::new("MySQL profile query returned no row"))?;
        if version.to_ascii_lowercase().contains("mariadb") {
            return Err(CatalogError::with_code(
                "DB_ADAPTER_UNSUPPORTED",
                "MariaDB lacks LATERAL joins, which the read compiler needs; it is not a supported runtime until its own gate passes",
            ));
        }
        let mut parts = version.split(['.', '-']);
        let (major, minor) = (parts.next().and_then(|v| v.parse::<u32>().ok()), parts.next().and_then(|v| v.parse::<u32>().ok()));
        if !matches!((major, minor), (Some(major), Some(minor)) if (major, minor) >= (8, 4)) {
            return Err(CatalogError::with_code("DB_ADAPTER_UNSUPPORTED", "MySQL 8.4 or newer is required"));
        }
        let strict = sql_mode.split(',').any(|mode| matches!(mode.trim(), "STRICT_TRANS_TABLES" | "STRICT_ALL_TABLES"));
        if !strict {
            return Err(CatalogError::with_code("DB_ADAPTER_UNSUPPORTED", "sql_mode must include STRICT_TRANS_TABLES"));
        }
        Ok(())
    }
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
