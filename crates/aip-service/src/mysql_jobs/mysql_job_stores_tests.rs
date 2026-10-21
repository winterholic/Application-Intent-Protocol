//! Live MySQL 8.4 tests of the managed stores a job reaches through `ctx`: event streams, messages and files.
//! Each test owns its schemas and runs in its own process (the service keeps one process-wide schema), so run them with
//! `--ignored --exact` one at a time, like the other live `mysql_jobs` tests.
use super::*;
use aip_migrate::mysql_driver::mysql_async::Conn;

struct Dir(std::path::PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn grant(actor: i64) -> DurablePrincipal {
    DurablePrincipal { issuer: "https://test.example".into(), subject: format!("user-{actor}"), actor_id: actor, issued_at: 1, min_iat: 0 }
}

const PREAMBLE: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Video {
 fields { id: Id; owner: Member; views: Int(0..100) default database }
 rows read when true
 transition increment { from views < 100; allow owner = actor; to views = views + 1 }
 expose apply increment { target id; bulk maxRows 1 }
 expose read { select id, views; sort id; budget { rows 20; depth 1; deadline 1s; cost 100 } }
}
"#;

pub(super) struct Fixture {
    pub setup: Conn,
    pub driver: MysqlDriver,
    pub runtime: Arc<MysqlJobs>,
    pub meta: String,
    _dir: Dir,
}

impl Fixture {
    /// A run-owned fixture: its schemas, a DML-only runtime account, one principal (actor 1) and a worker bundle.
    pub(super) async fn start(tag: &str, access: &str) -> Self {
        let (meta, source) = (format!("aip_t_{tag}"), format!("aip_t_{tag}_src"));
        let url = std::env::var("AIP_TEST_MYSQL_URL").unwrap();
        aip_test_support::validate_mysql_admin_url(&url).unwrap();
        let admin = MysqlDriver::connect(&url).await.unwrap();
        let mut setup = admin.pool().get_conn().await.unwrap();
        for schema in [&meta, &source] {
            setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
        }
        setup.query_drop(format!("CREATE DATABASE `{source}`")).await.unwrap();
        setup.query_drop(format!("CREATE TABLE `{source}`.member(id BIGINT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
        setup
            .query_drop(format!(
                "CREATE TABLE `{source}`.video(id BIGINT PRIMARY KEY,owner_id BIGINT NOT NULL,views BIGINT NOT NULL DEFAULT 0,\
                 FOREIGN KEY(owner_id) REFERENCES `{source}`.member(id)) ENGINE=InnoDB"
            ))
            .await
            .unwrap();
        setup.query_drop(format!("INSERT INTO `{source}`.member VALUES(1)")).await.unwrap();
        setup.query_drop(format!("INSERT INTO `{source}`.video VALUES(10,1,0)")).await.unwrap();
        let definition = format!(
            "{PREAMBLE}operation job storeJob {{\n input {{ id: Video.Id }}\n output {{ count: Int }}\n allow actor != null\n access {access}\n effect managed\n deadline 5s\n implementation \"worker.jobView\"\n}}\n"
        );
        let facts = spike_v1_fixture::load_str(&definition, spike_v1_fixture::Form::A).unwrap().execution;
        sqlgen::try_set_schema(&meta).unwrap();
        let digest = spike_v1_fixture::digest(&facts);
        let fingerprint = admin.ensure_meta_schema_bootstrap(&meta, &source, &digest).await.unwrap();
        setup.query_drop(format!("CREATE VIEW `{meta}`.member AS SELECT * FROM `{source}`.member")).await.unwrap();
        setup.query_drop(format!("CREATE VIEW `{meta}`.video AS SELECT * FROM `{source}`.video")).await.unwrap();
        setup
            .query_drop(format!("INSERT INTO `{meta}`.aip_principals(issuer,subject,actor_id) VALUES('https://test.example','user-1',1)"))
            .await
            .unwrap();
        let account = format!("aip_t_{tag}_rt");
        setup.query_drop(format!("DROP USER IF EXISTS '{account}'@'%'")).await.unwrap();
        setup.query_drop(format!("CREATE USER '{account}'@'%' IDENTIFIED BY 'store_test_only'")).await.unwrap();
        for schema in [&source, &meta] {
            setup.query_drop(format!("GRANT SELECT,INSERT,UPDATE,DELETE,CREATE TEMPORARY TABLES ON `{schema}`.* TO '{account}'@'%'")).await.unwrap();
        }
        let endpoint = url.split_once('@').unwrap().1.split('/').next().unwrap();
        let driver = MysqlDriver::from_url(&format!("mysql://{account}:store_test_only@{endpoint}/{source}")).unwrap();
        let fence = MysqlDeploymentFence::from_fingerprint(&meta, &source, &digest, &fingerprint).unwrap();
        let backend = Arc::new(MysqlBackend::new(driver.clone(), fence.clone()));
        let dir = Dir(std::env::temp_dir().join(format!("aip-jobs-store-{tag}-{}", std::process::id())));
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(dir.0.join("worker.py"), "async def jobView(input, ctx):\n return {'count': 1}\n").unwrap();
        let workers = ReadExtensions { lang: spike_v6_transport::server::WorkerLang::Python, dir: dir.0.clone() };
        let infra = spike_v6_transport::infrastructure::Infrastructure::default();
        let runtime = Arc::new(
            MysqlJobs::new(driver.clone(), fence, meta.clone(), digest, Arc::new(facts), IdWire::DecimalString, backend, workers, None, &infra)
                .unwrap(),
        );
        runtime.check_storage().await.unwrap();
        Self { setup, driver, runtime, meta, _dir: dir }
    }

    /// Enqueues a job as actor 1, claims it and returns a host that acts for that lease.
    pub(super) async fn host(&self, capabilities: &[&str]) -> JobHost {
        let accepted =
            self.runtime.handle("/jobs/enqueue", &json!({"operation":"storeJob","input":{"id":"10"},"key":"store job","delayMs":0}), &grant(1)).await;
        assert_eq!(accepted["ok"], true, "{accepted}");
        let store = self.runtime.store().unwrap();
        let mut conn = self.driver.pool().get_conn().await.unwrap();
        let lease = store.claim(&mut conn, "test-dispatcher").await.unwrap().unwrap();
        JobHost {
            runtime: self.runtime.clone(),
            lease,
            grant: grant(1),
            prefix: String::new(),
            depth: 0,
            budget: Arc::new(AtomicUsize::new(0)),
            capabilities: capabilities.iter().map(|capability| capability.to_string()).collect(),
        }
    }

    pub(super) async fn count(&self, sql: String) -> i64 {
        self.driver.pool().get_conn().await.unwrap().query_first::<i64, _>(sql).await.unwrap().unwrap()
    }

    pub(super) async fn finish(mut self, tag: &str) {
        let account = format!("aip_t_{tag}_rt");
        self.setup.query_drop(format!("DROP USER '{account}'@'%'")).await.unwrap();
        for schema in [self.meta.clone(), format!("{}_src", self.meta)] {
            self.setup.query_drop(format!("DROP DATABASE `{schema}`")).await.unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a run-owned MySQL 8.4 fixture; event streams through the store, the job host and /events/read"]
async fn mysql_event_streams_publish_once_replay_in_order_and_expire_explicitly() {
    use aip_execution::events::Error as EventError;
    let mut fixture = Fixture::start("job_events", "events.audit, events.other").await;
    let meta = fixture.meta.clone();
    let store = aip_migrate::mysql_events::MysqlEventStore::new(&meta).unwrap();
    let mut conn = fixture.driver.pool().get_conn().await.unwrap();

    // Idempotent publish: the same key and payload converges on one cursor, another payload under the key is a conflict.
    assert_eq!(store.publish_in(&mut conn, "owner-a", "audit", "k1", &json!({"n":1})).await.unwrap(), "1");
    assert_eq!(store.publish_in(&mut conn, "owner-a", "audit", "k1", &json!({"n":1})).await.unwrap(), "1");
    assert_eq!(store.publish_in(&mut conn, "owner-a", "audit", "k1", &json!({"n":2})).await.unwrap_err(), EventError::IdempotencyConflict);
    assert_eq!(store.publish_in(&mut conn, "owner-a", "audit", "k2", &json!({"n":2})).await.unwrap(), "2");
    // Streams are scoped by owner and name.
    assert_eq!(store.publish_in(&mut conn, "owner-b", "audit", "k1", &json!({"n":9})).await.unwrap(), "1");
    assert_eq!(store.publish_in(&mut conn, "owner-a", "other", "k1", &json!({"n":9})).await.unwrap(), "1");
    assert_eq!(store.publish_in(&mut conn, "owner-a", "bad stream", "k", &json!({})).await.unwrap_err(), EventError::InvalidEvent);
    let big = json!({"blob":"x".repeat(70_000)});
    assert_eq!(store.publish_in(&mut conn, "owner-a", "audit", "big", &big).await.unwrap_err(), EventError::InvalidEvent);

    // Replay is ordered and bounded; the next cursor resumes where the page ended.
    let page = store.scan(&mut conn, "owner-a", "audit", "0", 1).await.unwrap();
    assert_eq!((page.events.len(), page.next_cursor.as_str(), page.events[0].payload.clone()), (1, "1", json!({"n":1})));
    let page = store.scan(&mut conn, "owner-a", "audit", &page.next_cursor, 10).await.unwrap();
    assert_eq!((page.events.len(), page.next_cursor.as_str(), page.events[0].payload.clone()), (1, "2", json!({"n":2})));
    let page = store.scan(&mut conn, "owner-a", "audit", "2", 10).await.unwrap();
    assert_eq!((page.events.len(), page.next_cursor.as_str()), (0, "2"));
    assert_eq!(store.scan(&mut conn, "owner-a", "audit", "7", 10).await.unwrap_err(), EventError::InvalidEvent, "a cursor beyond the head");
    assert_eq!(store.scan(&mut conn, "owner-a", "missing", "0", 10).await.unwrap().events.len(), 0);

    // A pruned window is an explicit error, never a silent gap; the idempotency ledger outlives the payloads.
    store.prune(&mut conn, "owner-a", "audit", "1").await.unwrap();
    assert_eq!(store.scan(&mut conn, "owner-a", "audit", "0", 10).await.unwrap_err(), EventError::CursorExpired);
    assert_eq!(store.scan(&mut conn, "owner-a", "audit", "1", 10).await.unwrap().events.len(), 1);
    assert_eq!(store.publish_in(&mut conn, "owner-a", "audit", "k1", &json!({"n":1})).await.unwrap(), "1", "a trimmed event still dedupes its key");
    drop(conn);

    // The job host publishes in the lease owner's stream and journals the cursor, so a retry sees the same one.
    let mut host = fixture.host(&["events.audit"]).await;
    let publish = json!({"key":"first","payload":{"video":"10"}});
    let first = host.dispatch("events", "audit", &publish).await.unwrap();
    assert_eq!(first, json!({"cursor":"1"}));
    assert_eq!(host.dispatch("events", "audit", &publish).await.unwrap(), first, "the same effect replays its recorded cursor");
    assert_eq!(
        fixture.count(format!("SELECT COUNT(*) FROM `{meta}`.aip_event_log WHERE stream='audit' AND owner<>'owner-a' AND owner<>'owner-b'")).await,
        1
    );
    assert_eq!(host.dispatch("events", "audit", &json!({"key":"first","payload":{"video":"11"}})).await.unwrap_err(), "WIRE.IDEMPOTENCY_MISMATCH");
    let second = host.dispatch("events", "audit", &json!({"key":"second","payload":{"video":"11"}})).await.unwrap();
    assert_eq!(second, json!({"cursor":"2"}));
    assert_eq!(host.dispatch("events", "other", &publish).await.unwrap_err(), "WIRE.ACCESS_NOT_DECLARED");
    assert_eq!(host.dispatch("events", "audit", &json!({"payload":{}})).await.unwrap_err(), "WIRE.BAD_REQUEST");
    assert_eq!(
        host.dispatch("events", "audit", &json!({"key":"huge","payload":{"blob":"x".repeat(70_000)}})).await.unwrap_err(),
        "WIRE.EVENT_INVALID"
    );

    // A client reads its own stream through the product route (owner scoping itself is covered by the store assertions above).
    let read = |actor: i64, stream: &str, after: &str| {
        let runtime = fixture.runtime.clone();
        let body = json!({"stream":stream,"after":after,"limit":10});
        async move { runtime.handle("/events/read", &body, &grant(actor)).await }
    };
    let mine = read(1, "audit", "0").await;
    assert_eq!(mine["ok"], true, "{mine}");
    assert_eq!(
        (mine["events"][0]["payload"].clone(), mine["events"][1]["payload"].clone(), mine["cursor"].clone()),
        (json!({"video":"10"}), json!({"video":"11"}), json!("2"))
    );
    assert_eq!(read(1, "ghost", "0").await["code"], "WIRE.NOT_EXPOSED", "only a stream a job declares is readable");
    assert_eq!(read(1, "audit", "x").await["code"], "WIRE.BAD_VALUE");
    assert_eq!(fixture.runtime.handle("/events/read", &json!({"stream":"audit"}), &grant(1)).await["code"], "WIRE.BAD_REQUEST");

    // The lease is checked when the event is written: a recovered attempt cannot be written by the old one.
    fixture.setup.query_drop(format!("UPDATE `{meta}`.aip_jobs SET lease_epoch = lease_epoch + 1")).await.unwrap();
    assert_eq!(host.dispatch("events", "audit", &json!({"key":"after","payload":{}})).await.unwrap_err(), "WIRE.LEASE_LOST");
    assert_eq!(
        fixture.count(format!("SELECT COUNT(*) FROM `{meta}`.aip_event_log WHERE stream='audit' AND owner<>'owner-a' AND owner<>'owner-b'")).await,
        2
    );
    fixture.finish("job_events").await;
}
