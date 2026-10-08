//! Source rule: django-helpdesk FollowUp public/private visibility and persisted comment.
//! https://github.com/django-helpdesk/django-helpdesk/blob/5c2808dec2ba3b63d107b715d7d5b326feda666f/src/helpdesk/models.py#L948-L999
//! Canned replies exercise server-defined effects; arbitrary caller-supplied comments remain outside this probe.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    connect_with_url, execute,
    plan::{plan_read, Caller},
    sqlgen,
};
use spike_v3_write::{apply, Knobs};

const DB: &str = "host=localhost dbname=postgres";
const SOURCE: &str = r#"
enum TicketStatus { OPEN, CLOSED }
resource Member { fields { id: Id; isStaff: Bool } }
actor Member
resource Ticket {
  fields { id: Id; submitter: Member; status: TicketStatus }
  rows read when submitter = actor or actor.isStaff = true
  transition closePublic {
    from status = OPEN
    to status = CLOSED
    allow actor.isStaff = true
    repeat unchanged
    create FollowUp { ticket = this.id; public = true; comment = "Resolved: \"O'Brien\"\nPath C:\\tickets\\1\n한글 😀" }
    notify submitter "ticket.closed"
  }
  transition closePrivate {
    from status = OPEN
    to status = CLOSED
    allow actor.isStaff = true
    repeat unchanged
    create FollowUp { ticket = this.id; public = false; comment = "Internal:\n\"never email\"\\private" }
  }
  expose apply closePublic { target id; bulk maxRows 1 }
  expose apply closePrivate { target id; bulk maxRows 1 }
  expose read {
    select id, status
    sort id
    traverse followUps via FollowUp.ticket { select id, public, comment; sort id; limit 5 }
    budget { rows 10; depth 2; deadline 2s; cost 100 }
  }
}
resource FollowUp {
  fields { id: Id; ticket: Ticket; public: Bool; comment: Text }
  rows read when actor.isStaff = true or (public = true and ticket.submitter = actor)
  expose read {
    select id, public, comment
    sort id
    budget { rows 10; depth 1; deadline 2s; cost 100 }
  }
}
"#;

fn caller(id: Option<i64>) -> Caller {
    Caller { actor_id: id, now: "2026-10-08T00:00:00Z".into() }
}

async fn read(db: &mut tokio_postgres::Client, facts: &Value, request: &Value, who: Option<i64>) -> Vec<Value> {
    let plan = plan_read(facts, request, &caller(who)).unwrap();
    execute(db, &plan).await.unwrap()
}

#[tokio::test]
async fn public_and_private_replies_preserve_text_without_leaking_or_repeating_effects() {
    let facts = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let schema =
        format!("aip_helpdesk_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id,is_staff) VALUES (1,false),(2,true),(3,false); \
         INSERT INTO {schema}.ticket(id,submitter_id,status) VALUES (10,1,'OPEN'),(11,1,'OPEN')"
    ))
    .await
    .unwrap();
    let public = json!({"apply":"Ticket.closePublic","target":{"ids":["10"]}});
    let private = json!({"apply":"Ticket.closePrivate","target":{"ids":["11"]}});
    assert_eq!(apply(&mut db, &facts, &public, &caller(Some(1)), &Knobs::default()).await.unwrap_err().code, "FORBIDDEN");
    for request in [&public, &private] {
        assert_eq!(apply(&mut db, &facts, request, &caller(Some(2)), &Knobs::default()).await.unwrap().changed.len(), 1);
        let retry = apply(&mut db, &facts, request, &caller(Some(2)), &Knobs::default()).await.unwrap();
        assert!(retry.changed.is_empty());
        assert_eq!(retry.unchanged.len(), 1);
    }
    let counts =
        db.query_one(&format!("SELECT (SELECT count(*) FROM {schema}.follow_up), (SELECT count(*) FROM {schema}.aip_outbox)"), &[]).await.unwrap();
    assert_eq!(counts.get::<_, i64>(0), 2);
    assert_eq!(counts.get::<_, i64>(1), 1, "private replies and stale retries do not add notifications");
    let root = json!({"read":"FollowUp","select":["public","comment"],"sort":[{"field":"id"}]});
    let public_text = "Resolved: \"O'Brien\"\nPath C:\\tickets\\1\n한글 😀";
    let private_text = "Internal:\n\"never email\"\\private";
    assert_eq!(read(&mut db, &facts, &root, Some(1)).await, [json!({"public":true,"comment":public_text})]);
    assert_eq!(
        read(&mut db, &facts, &root, Some(2)).await,
        [json!({"public":true,"comment":public_text}), json!({"public":false,"comment":private_text})]
    );
    for outsider in [Some(3), None] {
        assert!(read(&mut db, &facts, &root, outsider).await.is_empty());
    }
    let nested = json!({"read":"Ticket","select":["id",{"followUps":{"select":["public","comment"]}}],"sort":[{"field":"id"}]});
    assert_eq!(
        read(&mut db, &facts, &nested, Some(1)).await,
        [json!({"id":10,"followUps":[{"public":true,"comment":public_text}]}), json!({"id":11,"followUps":[]})]
    );
    let staff = read(&mut db, &facts, &nested, Some(2)).await;
    assert_eq!(staff[1], json!({"id":11,"followUps":[{"public":false,"comment":private_text}]}));
    for outsider in [Some(3), None] {
        assert!(read(&mut db, &facts, &nested, outsider).await.is_empty());
    }
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
