use aip_execution::object_ledger::{Error, Kind, ObjectLedger, Outcome, State, Usage};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_postgres::{Client, NoTls};

const DB: &str = aip_test_support::test_database_url!();
static NEXT: AtomicU64 = AtomicU64::new(0);
const MIB: u64 = 1024 * 1024;

struct Db {
    client: Client,
    schema: String,
}

impl Db {
    async fn new() -> Self {
        let schema = format!("aip_ledger_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let (client, connection) = tokio_postgres::connect(DB, NoTls).await.expect("connect to local test PostgreSQL");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client.batch_execute(&format!("CREATE SCHEMA {schema}")).await.unwrap();
        client.batch_execute(&ObjectLedger::ddl(&schema).unwrap()).await.unwrap();
        Self { client, schema }
    }

    fn ledger(&self, max_bytes: u64, max_objects: usize) -> ObjectLedger {
        ObjectLedger::new(&self.schema, max_bytes, max_objects).unwrap()
    }

    /// Makes every reservation due without waiting for the real timers.
    async fn expire_all(&self) {
        self.client
            .batch_execute(&format!(
                "UPDATE {}.aip_object_ledger SET expires_at=clock_timestamp()-interval '1 second' WHERE expires_at IS NOT NULL",
                self.schema
            ))
            .await
            .unwrap();
    }

    async fn rows(&self) -> i64 {
        self.client.query_one(&format!("SELECT count(*) FROM {}.aip_object_ledger", self.schema), &[]).await.unwrap().get(0)
    }

    async fn part_rows(&self) -> i64 {
        self.client.query_one(&format!("SELECT count(*) FROM {}.aip_object_parts", self.schema), &[]).await.unwrap().get(0)
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        let schema = self.schema.clone();
        std::thread::spawn(move || {
            let _ =
                std::process::Command::new("psql").args(["-X", "-q", "-d", DB, "-c", &format!("DROP SCHEMA IF EXISTS {schema} CASCADE")]).output();
        })
        .join()
        .unwrap();
    }
}

#[tokio::test]
async fn signed_put_bytes_count_against_the_owner_quota_before_any_upload() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.reserve_put(&mut db.client, "alice", "a", 6 * MIB, 600).await.unwrap();
    assert_eq!(ledger.reserve_put(&mut db.client, "alice", "b", 5 * MIB, 600).await, Err(Error::QuotaExceeded), "6 + 5 MiB passes a 10 MiB ceiling");
    ledger.reserve_put(&mut db.client, "alice", "b", 4 * MIB, 600).await.unwrap();
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap(), Usage { bytes: 10 * MIB, objects: 2 });
    // The ceiling is per owner: another owner has its own 10 MiB.
    ledger.reserve_put(&mut db.client, "bob", "c", 10 * MIB, 600).await.unwrap();
}

#[tokio::test]
async fn object_count_ceiling_is_enforced_and_replacing_an_object_does_not_add_one() {
    let mut db = Db::new().await;
    let ledger = db.ledger(100 * MIB, 2);
    ledger.reserve_put(&mut db.client, "alice", "a", MIB, 600).await.unwrap();
    ledger.reserve_put(&mut db.client, "alice", "b", MIB, 600).await.unwrap();
    assert_eq!(ledger.reserve_put(&mut db.client, "alice", "c", MIB, 600).await, Err(Error::QuotaExceeded));
    assert_eq!(
        ledger.start_multipart(&mut db.client, "alice", "d", "up-d", 3600).await,
        Err(Error::QuotaExceeded),
        "multipart counts as an object too"
    );
    ledger.reserve_put(&mut db.client, "alice", "a", 2 * MIB, 600).await.unwrap();
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap(), Usage { bytes: 3 * MIB, objects: 2 });
}

#[tokio::test]
async fn replacing_a_signed_size_releases_the_old_charge() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.reserve_put(&mut db.client, "alice", "a", 9 * MIB, 600).await.unwrap();
    // Re-signing the same object for a smaller file must not be charged on top of the 9 MiB.
    ledger.reserve_put(&mut db.client, "alice", "a", 2 * MIB, 600).await.unwrap();
    ledger.reserve_put(&mut db.client, "alice", "b", 8 * MIB, 600).await.unwrap();
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap().bytes, 10 * MIB);
}

#[tokio::test]
async fn concurrent_reservations_cannot_jointly_exceed_the_quota() {
    let db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    let mut tasks = Vec::new();
    for index in 0..8 {
        let ledger = ledger.clone();
        tasks.push(tokio::spawn(async move {
            let (mut client, connection) = tokio_postgres::connect(DB, NoTls).await.unwrap();
            tokio::spawn(async move {
                let _ = connection.await;
            });
            ledger.reserve_put(&mut client, "alice", &format!("o{index}"), 3 * MIB, 600).await
        }));
    }
    let mut accepted = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(()) => accepted += 1,
            Err(Error::QuotaExceeded) => {}
            Err(other) => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(accepted, 3, "3 x 3 MiB fits in 10 MiB and the fourth does not");
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap().bytes, 9 * MIB);
}

#[tokio::test]
async fn expired_put_reservation_is_released_when_the_store_has_no_object() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.reserve_put(&mut db.client, "alice", "abandoned", 7 * MIB, 600).await.unwrap();
    assert!(ledger.claim_due(&mut db.client, 10, 300).await.unwrap().is_empty(), "nothing is due before the timer");
    db.expire_all().await;
    let due = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    assert_eq!((due.len(), due[0].kind, due[0].state, due[0].bytes), (1, Kind::Put, State::Pending, 7 * MIB as i64));
    assert!(ledger.claim_due(&mut db.client, 10, 300).await.unwrap().is_empty(), "a claimed row is leased, not handed out twice");
    assert!(ledger.resolve(&mut db.client, &due[0], Outcome::Absent).await.unwrap());
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap(), Usage { bytes: 0, objects: 0 });
    ledger.reserve_put(&mut db.client, "alice", "next", 10 * MIB, 600).await.unwrap();
}

/// A ledger table created before the `settling` column existed (the earlier 8.5 build) is upgraded in place by the same DDL, keeping its rows.
#[tokio::test]
async fn the_ddl_upgrades_a_ledger_table_that_predates_the_hold_column() {
    let mut db = Db::new().await;
    let schema = db.schema.clone();
    db.client
        .batch_execute(&format!(
            "DROP TABLE {schema}.aip_object_ledger; \
             CREATE TABLE {schema}.aip_object_ledger (owner text NOT NULL, object_id text NOT NULL, \
             kind text NOT NULL CHECK (kind IN ('PUT','MULTIPART')), upload_id text, \
             state text NOT NULL CHECK (state IN ('PENDING','COMPLETE','DELETING')), \
             bytes bigint NOT NULL CHECK (bytes >= 0), expires_at timestamptz, version bigint NOT NULL DEFAULT 0, \
             created_at timestamptz NOT NULL DEFAULT clock_timestamp(), PRIMARY KEY(owner,object_id)); \
             INSERT INTO {schema}.aip_object_ledger (owner,object_id,kind,state,bytes) VALUES ('alice','old','PUT','COMPLETE',{})",
            2 * MIB
        ))
        .await
        .unwrap();
    db.client.batch_execute(&ObjectLedger::ddl(&schema).unwrap()).await.unwrap();
    db.client.batch_execute(&ObjectLedger::ddl(&schema).unwrap()).await.expect("running the DDL twice is harmless");
    let ledger = db.ledger(10 * MIB, 100);
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap(), Usage { bytes: 2 * MIB, objects: 1 }, "existing rows survive");
    ledger.reserve_put(&mut db.client, "alice", "new", 3 * MIB, 600).await.unwrap();
}

const HOLD: u64 = 86_400;

#[tokio::test]
async fn first_miss_of_a_put_keeps_the_charge_and_only_the_second_miss_releases_it() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.reserve_put(&mut db.client, "alice", "slow", 7 * MIB, 600).await.unwrap();
    db.expire_all().await;
    let first = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    assert!(!first[0].settling);
    assert!(ledger.resolve(&mut db.client, &first[0], Outcome::NotYetStored { hold_secs: HOLD }).await.unwrap());
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap(), Usage { bytes: 7 * MIB, objects: 1 }, "the charge is kept, not released");
    assert_eq!(
        ledger.reserve_put(&mut db.client, "alice", "other", 4 * MIB, 600).await.unwrap_err(),
        Error::QuotaExceeded,
        "held bytes cannot be handed to another upload"
    );
    assert!(ledger.claim_due(&mut db.client, 10, 300).await.unwrap().is_empty(), "the second look is a day away");

    db.expire_all().await;
    let second = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    assert_eq!((second.len(), second[0].settling), (1, true));
    assert!(ledger.resolve(&mut db.client, &second[0], Outcome::NotYetStored { hold_secs: HOLD }).await.unwrap());
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap(), Usage { bytes: 0, objects: 0 }, "the second miss releases the reservation");
}

#[tokio::test]
async fn an_upload_that_lands_during_the_hold_is_confirmed_at_its_real_size() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.reserve_put(&mut db.client, "alice", "slow", 7 * MIB, 600).await.unwrap();
    db.expire_all().await;
    let first = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    ledger.resolve(&mut db.client, &first[0], Outcome::NotYetStored { hold_secs: HOLD }).await.unwrap();
    db.expire_all().await;
    let second = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    assert!(ledger.resolve(&mut db.client, &second[0], Outcome::Stored(7 * MIB)).await.unwrap());
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap(), Usage { bytes: 7 * MIB, objects: 1 });
    db.expire_all().await;
    assert!(ledger.claim_due(&mut db.client, 10, 300).await.unwrap().is_empty(), "a confirmed object has no timer");
}

#[tokio::test]
async fn signing_a_held_object_again_starts_a_fresh_reservation_and_a_stale_hold_is_ignored() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.reserve_put(&mut db.client, "alice", "slow", 7 * MIB, 600).await.unwrap();
    db.expire_all().await;
    let first = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    // The owner signs the same object again before the reconciler's answer arrives.
    ledger.reserve_put(&mut db.client, "alice", "slow", 6 * MIB, 600).await.unwrap();
    assert!(!ledger.resolve(&mut db.client, &first[0], Outcome::NotYetStored { hold_secs: HOLD }).await.unwrap(), "stale answer");
    db.expire_all().await;
    let again = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    assert!(!again[0].settling, "a re-sign resets the hold: it gets its own first look");
    assert_eq!(again[0].bytes, 6 * MIB as i64);
}

#[tokio::test]
async fn a_signed_delete_that_finds_the_object_gone_is_released_not_held() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.reserve_put(&mut db.client, "alice", "obj", 3 * MIB, 600).await.unwrap();
    db.expire_all().await;
    let due = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    ledger.resolve(&mut db.client, &due[0], Outcome::Stored(3 * MIB)).await.unwrap();
    ledger.delete_signed(&mut db.client, "alice", "obj", 360).await.unwrap();
    db.expire_all().await;
    let due = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    assert_eq!(due[0].state, State::Deleting);
    assert!(ledger.resolve(&mut db.client, &due[0], Outcome::NotYetStored { hold_secs: HOLD }).await.unwrap());
    assert_eq!(db.rows().await, 0, "the confirmed-gone object leaves the ledger");
}

#[tokio::test]
async fn expired_put_reservation_becomes_a_stored_object_with_its_real_size() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.reserve_put(&mut db.client, "alice", "used", 7 * MIB, 600).await.unwrap();
    db.expire_all().await;
    let due = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    assert!(ledger.resolve(&mut db.client, &due[0], Outcome::Stored(5 * MIB)).await.unwrap());
    assert_eq!(
        ledger.usage(&db.client, "alice").await.unwrap(),
        Usage { bytes: 5 * MIB, objects: 1 },
        "stored bytes keep counting, at the real size"
    );
    db.expire_all().await;
    assert!(ledger.claim_due(&mut db.client, 10, 300).await.unwrap().is_empty(), "a confirmed object has no timer");
}

#[tokio::test]
async fn a_request_that_touches_a_claimed_row_wins_over_the_stale_resolution() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.reserve_put(&mut db.client, "alice", "k", MIB, 600).await.unwrap();
    db.expire_all().await;
    let due = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    // The caller signs the same object again while the collector is asking the store.
    ledger.reserve_put(&mut db.client, "alice", "k", 2 * MIB, 600).await.unwrap();
    assert!(!ledger.resolve(&mut db.client, &due[0], Outcome::Absent).await.unwrap(), "stale outcome must not delete a fresh reservation");
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap(), Usage { bytes: 2 * MIB, objects: 1 });
}

#[tokio::test]
async fn multipart_parts_are_charged_and_resigning_a_part_replaces_its_size() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.start_multipart(&mut db.client, "alice", "big", "up-1", 3600).await.unwrap();
    ledger.reserve_part(&mut db.client, "alice", "big", "up-1", 1, 6 * MIB, 3600).await.unwrap();
    assert_eq!(ledger.reserve_part(&mut db.client, "alice", "big", "up-1", 2, 5 * MIB, 3600).await, Err(Error::QuotaExceeded));
    ledger.reserve_part(&mut db.client, "alice", "big", "up-1", 2, 4 * MIB, 3600).await.unwrap();
    // Part 1 signed again after a failed attempt, smaller this time.
    ledger.reserve_part(&mut db.client, "alice", "big", "up-1", 1, 5 * MIB, 3600).await.unwrap();
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap(), Usage { bytes: 9 * MIB, objects: 1 });
    // Another object of the same owner shares the ceiling.
    assert_eq!(ledger.reserve_put(&mut db.client, "alice", "other", 2 * MIB, 600).await, Err(Error::QuotaExceeded));
    ledger.reserve_put(&mut db.client, "alice", "other", MIB, 600).await.unwrap();
}

#[tokio::test]
async fn multipart_parts_need_an_upload_this_ledger_started() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.start_multipart(&mut db.client, "alice", "big", "up-1", 3600).await.unwrap();
    assert_eq!(ledger.reserve_part(&mut db.client, "alice", "big", "other-upload", 1, MIB, 3600).await, Err(Error::NotFound));
    assert_eq!(ledger.reserve_part(&mut db.client, "bob", "big", "up-1", 1, MIB, 3600).await, Err(Error::NotFound), "another owner cannot extend it");
    assert_eq!(ledger.require_open_multipart(&mut db.client, "alice", "nope", "up-1").await, Err(Error::NotFound));
    ledger.require_open_multipart(&mut db.client, "alice", "big", "up-1").await.unwrap();
    // A plain PUT reservation cannot take over a multipart object id.
    assert_eq!(ledger.reserve_put(&mut db.client, "alice", "big", MIB, 600).await, Err(Error::InvalidRequest));
}

#[tokio::test]
async fn completing_a_multipart_keeps_the_summed_size_and_drops_the_timer_and_parts() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.start_multipart(&mut db.client, "alice", "big", "up-1", 3600).await.unwrap();
    ledger.reserve_part(&mut db.client, "alice", "big", "up-1", 1, 3 * MIB, 3600).await.unwrap();
    ledger.reserve_part(&mut db.client, "alice", "big", "up-1", 2, 2 * MIB, 3600).await.unwrap();
    ledger.complete_multipart(&mut db.client, "alice", "big", "up-1").await.unwrap();
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap(), Usage { bytes: 5 * MIB, objects: 1 });
    assert_eq!(db.part_rows().await, 0);
    db.expire_all().await;
    assert!(ledger.claim_due(&mut db.client, 10, 300).await.unwrap().is_empty());
    assert_eq!(
        ledger.complete_multipart(&mut db.client, "alice", "big", "up-1").await,
        Err(Error::NotFound),
        "a finished upload cannot be completed twice"
    );
}

#[tokio::test]
async fn idle_multipart_upload_is_due_and_aborting_it_frees_every_charged_part() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.start_multipart(&mut db.client, "alice", "big", "up-1", 3600).await.unwrap();
    ledger.reserve_part(&mut db.client, "alice", "big", "up-1", 1, 8 * MIB, 3600).await.unwrap();
    db.expire_all().await;
    let due = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    assert_eq!((due.len(), due[0].kind, due[0].upload_id.as_deref(), due[0].bytes), (1, Kind::Multipart, Some("up-1"), 8 * MIB as i64));
    assert!(ledger.resolve(&mut db.client, &due[0], Outcome::Absent).await.unwrap());
    assert_eq!((db.rows().await, db.part_rows().await), (0, 0));
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap(), Usage { bytes: 0, objects: 0 });
}

#[tokio::test]
async fn activity_restarts_the_multipart_idle_timer() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.start_multipart(&mut db.client, "alice", "big", "up-1", 3600).await.unwrap();
    db.expire_all().await;
    ledger.reserve_part(&mut db.client, "alice", "big", "up-1", 1, MIB, 3600).await.unwrap();
    assert!(ledger.claim_due(&mut db.client, 10, 300).await.unwrap().is_empty(), "signing a part is activity");
}

#[tokio::test]
async fn aborting_an_upload_releases_its_charge_but_cannot_release_a_stored_object() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.start_multipart(&mut db.client, "alice", "big", "up-1", 3600).await.unwrap();
    ledger.reserve_part(&mut db.client, "alice", "big", "up-1", 1, 8 * MIB, 3600).await.unwrap();
    assert!(!ledger.abort_multipart(&mut db.client, "alice", "big", "wrong-upload").await.unwrap());
    assert!(!ledger.abort_multipart(&mut db.client, "bob", "big", "up-1").await.unwrap(), "another owner cannot abort it");
    assert_eq!(db.rows().await, 1);
    assert!(ledger.abort_multipart(&mut db.client, "alice", "big", "up-1").await.unwrap());
    assert_eq!((db.rows().await, db.part_rows().await), (0, 0));

    ledger.reserve_put(&mut db.client, "alice", "stored", 6 * MIB, 600).await.unwrap();
    db.expire_all().await;
    let due = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    ledger.resolve(&mut db.client, &due[0], Outcome::Stored(6 * MIB)).await.unwrap();
    assert!(!ledger.abort_multipart(&mut db.client, "alice", "stored", "invented").await.unwrap());
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap(), Usage { bytes: 6 * MIB, objects: 1 }, "the stored object keeps its charge");
}

#[tokio::test]
async fn a_signed_delete_keeps_the_charge_until_the_store_confirms_the_object_is_gone() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.reserve_put(&mut db.client, "alice", "a", 4 * MIB, 600).await.unwrap();
    db.expire_all().await;
    let due = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    ledger.resolve(&mut db.client, &due[0], Outcome::Stored(4 * MIB)).await.unwrap();

    ledger.delete_signed(&mut db.client, "alice", "a", 360).await.unwrap();
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap().bytes, 4 * MIB, "the URL may be unused, so the bytes still count");
    db.expire_all().await;
    let due = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    assert_eq!((due.len(), due[0].state), (1, State::Deleting));
    // The store still has it: the delete URL was not used and the object counts again, without a timer.
    ledger.resolve(&mut db.client, &due[0], Outcome::Stored(4 * MIB)).await.unwrap();
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap().bytes, 4 * MIB);
    ledger.delete_signed(&mut db.client, "alice", "a", 360).await.unwrap();
    db.expire_all().await;
    let due = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    ledger.resolve(&mut db.client, &due[0], Outcome::Absent).await.unwrap();
    assert_eq!(ledger.usage(&db.client, "alice").await.unwrap(), Usage { bytes: 0, objects: 0 });
}

#[tokio::test]
async fn a_signed_delete_for_an_unknown_or_unconfirmed_object_changes_nothing() {
    let mut db = Db::new().await;
    let ledger = db.ledger(10 * MIB, 100);
    ledger.delete_signed(&mut db.client, "alice", "never-seen", 360).await.unwrap();
    ledger.reserve_put(&mut db.client, "alice", "pending", MIB, 600).await.unwrap();
    ledger.delete_signed(&mut db.client, "alice", "pending", 360).await.unwrap();
    db.expire_all().await;
    let due = ledger.claim_due(&mut db.client, 10, 300).await.unwrap();
    assert_eq!((due.len(), due[0].state), (1, State::Pending), "a signed delete does not turn an unconfirmed upload into a deletion");
}

#[tokio::test]
async fn claims_are_bounded_and_oldest_first() {
    let mut db = Db::new().await;
    let ledger = db.ledger(100 * MIB, 100);
    for index in 0..5 {
        ledger.reserve_put(&mut db.client, "alice", &format!("o{index}"), MIB, 600).await.unwrap();
    }
    db.expire_all().await;
    assert_eq!(ledger.claim_due(&mut db.client, 2, 300).await.unwrap().len(), 2);
    assert_eq!(ledger.claim_due(&mut db.client, 10, 300).await.unwrap().len(), 3);
}

#[tokio::test]
async fn invalid_configuration_and_inputs_are_rejected() {
    let mut db = Db::new().await;
    assert!(ObjectLedger::new("public", MIB, 1).is_err());
    assert!(ObjectLedger::new(&db.schema, 0, 1).is_err());
    assert!(ObjectLedger::new(&db.schema, MIB, 0).is_err());
    let ledger = db.ledger(MIB, 1);
    assert_eq!(ledger.reserve_put(&mut db.client, "alice", "a", u64::MAX, 600).await, Err(Error::QuotaExceeded));
    assert_eq!(ledger.reserve_put(&mut db.client, "alice", "a", 1, 0).await, Err(Error::InvalidRequest));
}
