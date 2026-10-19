use aip_migrate::external::{Mapping, Table, plan_external};
use futures_util::FutureExt;
use spike_v1_fixture::{Form, load_str};
use std::{
    collections::BTreeMap,
    panic::AssertUnwindSafe,
    sync::atomic::{AtomicU64, Ordering},
};

const DB: &str = aip_test_support::test_database_url!();
static NEXT: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn external_mailbox_uniqueness_refuses_legacy_duplicates_without_deleting_them() {
    let suffix = format!("{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let source = format!("aip_shared_src_{suffix}");
    let target = format!("aip_shared_tgt_{suffix}");
    let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    let result=AssertUnwindSafe(async {
        db.batch_execute(&format!("CREATE SCHEMA {source}; CREATE TABLE {source}.users(id bigint PRIMARY KEY); CREATE TABLE {source}.conversation(id bigint PRIMARY KEY,user_id bigint NOT NULL REFERENCES {source}.users(id),shared_id text NOT NULL); INSERT INTO {source}.users VALUES(1); INSERT INTO {source}.conversation VALUES(10,1,'fixture-shared'),(11,1,'fixture-shared');")).await.unwrap();
        let mut tables=BTreeMap::new();
        tables.insert("User".into(),Table{table:"users".into(),columns:BTreeMap::from([("id".into(),"id".into())]),shared_pk_without_fk:false});
        tables.insert("Conversation".into(),Table{table:"conversation".into(),columns:BTreeMap::from([("id".into(),"id".into()),("user".into(),"user_id".into()),("shared_id".into(),"shared_id".into())]),shared_pk_without_fk:false});
        let mapping=Mapping{schema:source.clone(),tables};
        let plain="actor User resource User {fields {id: Id}} resource Conversation {fields {id: Id; user: User; shared_id: Text}}";
        let old=load_str(plain,Form::A).unwrap().execution;
        assert_eq!(plan_external(DB,&target,&old,&mapping,"decimal").await.unwrap()["ok"],true,"positive control without the new uniqueness contract");
        let unique=load_str(&plain.replace("shared_id: Text}}","shared_id: Text} unique user, shared_id}"),Form::A).unwrap().execution;
        let rejected=plan_external(DB,&target,&unique,&mapping,"decimal").await.unwrap_err();
        assert_eq!(rejected["code"],"EXTERNAL_SCHEMA","{rejected}");
        let error=db.batch_execute(&format!("CREATE UNIQUE INDEX mailbox_shared ON {source}.conversation(user_id,shared_id)")).await.unwrap_err();
        assert_eq!(error.code().map(|code|code.code()),Some("23505"));
        let ids:String=db.query_one(&format!("SELECT string_agg(id::text,',' ORDER BY id) FROM {source}.conversation"),&[]).await.unwrap().get(0);
        assert_eq!(ids,"10,11","planning and failed index creation must preserve both legacy rows");
        let absent:bool=db.query_one("SELECT to_regnamespace($1) IS NULL",&[&target]).await.unwrap().get(0);
        assert!(absent,"read-only adoption planning must not create a deployment");
    }).catch_unwind().await;
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {source} CASCADE; DROP SCHEMA IF EXISTS {target} CASCADE")).await.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
