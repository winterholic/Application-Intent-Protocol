//! Tests for the durable rekey execution boundary and lease contracts.
use aip_execution::encryption::{DekStore, Scope};
use aip_execution::rekey::{RekeyReport, RekeyTarget, RekeyTargetReport};

#[test]
fn rekey_target_metadata_and_key_formation() {
    let scope = Scope::service();
    let target = RekeyTarget::new("users", "email_cipher", "id", "User", "email", scope);
    assert_eq!(target.table, "users");
    assert_eq!(target.column, "email_cipher");
    assert_eq!(target.id_column, "id");
    assert_eq!(target.resource, "User");
    assert_eq!(target.field, "email");
    assert_eq!(target.target_key(), "service:global:users:email_cipher");

    let ref_scope = Scope::reference("Organization", "12345").unwrap();
    let ref_target = RekeyTarget::new("secrets", "api_key", "secret_id", "Secret", "apiKey", ref_scope);
    assert_eq!(ref_target.target_key(), "ref:Organization:12345:secrets:api_key");
}

#[test]
fn rekey_reports_default_to_empty_and_unretired() {
    let target_rep = RekeyTargetReport {
        table: "users".into(),
        column: "email_cipher".into(),
        scanned: 10,
        rekeyed: 10,
        already_current: 0,
        raced: 0,
        failed_ids: vec![],
        remaining_old: 0,
    };
    let report = RekeyReport {
        old_dek_id: "00000000-0000-0000-0000-000000000001".into(),
        old_version: 1,
        active_dek_id: "00000000-0000-0000-0000-000000000002".into(),
        active_version: 2,
        target_reports: vec![target_rep],
        fully_retired: true,
    };
    assert!(report.fully_retired);
    assert_eq!(report.target_reports.len(), 1);
    assert_eq!(report.target_reports[0].remaining_old, 0);
}

#[test]
fn dek_store_ddl_contains_rekey_leases_table() {
    let store = DekStore::new("aip_rekey_test_schema").unwrap();
    let ddl = store.ddl();
    assert!(ddl.contains("aip_rekey_leases"), "DDL must declare aip_rekey_leases table for worker serialization");
    assert!(ddl.contains("target_key text PRIMARY KEY"), "aip_rekey_leases must key on target_key");
    assert!(ddl.contains("leased_until timestamptz NOT NULL"), "aip_rekey_leases must contain leased_until timestamp");
}
