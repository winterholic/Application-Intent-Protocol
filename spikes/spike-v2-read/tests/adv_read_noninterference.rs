//! Executed attack check for read authorization (security audit section 6, "read authorization").
//!
//! Noninterference oracle: two databases that differ only in data the attacker may not read must give the attacker
//! byte-identical answers to every request, including identical rejection codes. The same requests run as the
//! owner must differ somewhere, otherwise the requests would prove nothing (negative control).
//!
//! The definition is deliberately generous: every readable field of every resource is filterable and sortable, and
//! offset and cursor paging are open, so anything the engine refuses or hides is refused by the engine and not by a
//! careful definition author.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller};
use spike_v2_read::{connect, execute, sqlgen};

const DEF: &str = "
enum Level { LOW, HIGH }
resource Member { fields { id: Id } }
actor Member
access seeDocs = totalOfVisible(Folder)
resource Folder {
  fields { id: Id; name: Text }
  rows read when true
  expose read {
    select id, name, docSum, docMax, docMin
    traverse docs via Doc.folder { select id, title, score, note; sort score; limit 10 }
    sort id
    budget { rows 20; depth 2; deadline 2s; cost 1000 }
  }
  aggregate docSum: Int { source Doc; sourceAccess seeDocs; groupKey folder; callerFilter none; rowOutput none; release sum(score) }
  aggregate docMax: Int? { source Doc; sourceAccess seeDocs; groupKey folder; callerFilter none; rowOutput none; release max(score) }
  aggregate docMin: Int? { source Doc; sourceAccess seeDocs; groupKey folder; callerFilter none; rowOutput none; release min(score) }
}
resource Doc {
  fields { id: Id; owner: Member; folder: Folder; title: Text; score: Int; level: Level; note: Text? }
  rows read when owner = actor or level = LOW
  field note read when owner = actor
  expose read {
    select id, title, score, level, note
    filter id.eq, id.in, title.eq, title.contains, title.prefix, score.eq, score.gte, score.lte, score.gt, score.lt, level.eq, level.in, owner.eq, owner.in
    sort id, score, title
    budget { rows 20; depth 2; deadline 2s; cost 1000; offset 50; cursor }
  }
}
resource Item {
  fields { id: Id; doc: Doc?; label: Text }
  rows read when true
  expose read {
    select id, label
    filter doc.eq, doc.in, doc.isNull, label.eq
    sort id, label
    traverse doc { select id, title, score, note }
    budget { rows 20; depth 2; deadline 2s; cost 1000; offset 50 }
  }
}
resource Profile {
  fields { id: Id; owner: Member; nickname: Text; salary: Int }
  rows read when true
  field salary read when owner = actor
  expose read {
    select id, nickname, salary
    filter nickname.eq, nickname.contains
    sort id, nickname
    budget { rows 20; depth 1; deadline 2s; cost 1000; offset 50 }
  }
}
";

fn who(actor: Option<i64>) -> Caller {
    Caller { actor_id: actor, now: "2026-10-13T00:00:00Z".into() }
}

fn attacks() -> Vec<Value> {
    let mut out = Vec::new();
    let sel = json!(["id", "title", "score", "level", "note"]);
    // Single-condition probes against the hidden documents' values (ids 2 and 3 belong to member 1 and are HIGH).
    let probes: Vec<(&str, &str, Value)> = vec![
        ("title", "eq", json!("secret-a")),
        ("title", "eq", json!("zzzzzz")),
        ("title", "contains", json!("secret")),
        ("title", "contains", json!("zz")),
        ("title", "prefix", json!("s")),
        ("title", "prefix", json!("z")),
        ("score", "eq", json!(100)),
        ("score", "eq", json!(1)),
        ("score", "gte", json!(50)),
        ("score", "gt", json!(99)),
        ("score", "lte", json!(1)),
        ("score", "lt", json!(2)),
        ("score", "in", json!([1, 50, 100])),
        ("level", "eq", json!("HIGH")),
        ("level", "in", json!(["HIGH"])),
        ("owner", "eq", json!(1)),
        ("owner", "in", json!([1, 2])),
    ];
    for (field, op, value) in probes {
        out.push(json!({"read":"Doc","select":sel,"filter":[{"field":field,"op":op,"value":value}],"sort":[{"field":"id"}]}));
    }
    // Ordering and paging as side channels: the first row by a hidden value, the position of a visible row among hidden ones.
    for field in ["score", "title"] {
        for dir in ["asc", "desc"] {
            for limit in [1, 2, 5] {
                out.push(json!({"read":"Doc","select":sel,"sort":[{"field":field,"dir":dir}],"limit":limit}));
            }
            for offset in [0, 1, 2, 3, 10] {
                out.push(json!({"read":"Doc","select":sel,"sort":[{"field":field,"dir":dir},{"field":"id"}],"limit":2,"offset":offset}));
            }
        }
    }
    for after in [json!({"score":100,"id":2}), json!({"score":1,"id":2}), json!({"score":50,"id":3}), json!({"score":0,"id":0})] {
        out.push(json!({"read":"Doc","select":sel,"sort":[{"field":"score"}],"after":after,"limit":5}));
        out.push(json!({"read":"Doc","select":sel,"sort":[{"field":"score","dir":"desc"}],"after":after,"limit":5}));
    }
    // Probing a document by id, whether or not the caller may read it.
    for id in [1, 2, 3, 4, 5, 6, 99] {
        out.push(json!({"read":"Doc","select":sel,"filter":[{"field":"id","op":"eq","value":id}]}));
    }
    // References from readable rows to documents the caller cannot read behave exactly like null.
    for op_value in [("eq", json!(1)), ("eq", json!(2)), ("eq", json!(3)), ("in", json!([2, 3])), ("isNull", json!(true)), ("isNull", json!(false))] {
        out.push(
            json!({"read":"Item","select":["id","label"],"filter":[{"field":"doc","op":op_value.0,"value":op_value.1}],"sort":[{"field":"id"}]}),
        );
    }
    out.push(json!({"read":"Item","select":["id","label",{"doc":{"select":["id","title","score","note"]}}],"sort":[{"field":"id"}]}));
    out.push(json!({"read":"Item","select":["id","label",{"doc":{"select":["id","title","score","note"]}}],"sort":[{"field":"label","dir":"desc"}],"limit":2,"offset":1}));
    // Aggregates and one-to-many traversal over documents the caller may not read: values released by sum/min/max and the
    // children that come back must come from readable documents only. (`count` would count everything by declaration,
    // so the definition does not offer it.)
    out.push(json!({"read":"Folder","select":["id","name","docSum","docMax","docMin"],"sort":[{"field":"id"}]}));
    out.push(json!({"read":"Folder","select":["id",{"docs":{"select":["id","title","score","note"]}}],"sort":[{"field":"id"}]}));
    // A hidden field of a readable row: selected it is null, and the filters that would reveal it are not offered.
    out.push(json!({"read":"Profile","select":["id","nickname","salary"],"sort":[{"field":"id"}]}));
    for field in ["salary", "owner"] {
        for op in ["eq", "gte", "lte", "in", "isNull"] {
            let value = if op == "in" {
                json!([1000, 5000])
            } else if op == "isNull" {
                json!(true)
            } else {
                json!(5000)
            };
            out.push(json!({"read":"Profile","select":["id","nickname"],"filter":[{"field":field,"op":op,"value":value}]}));
        }
        out.push(json!({"read":"Profile","select":["id","nickname"],"sort":[{"field":field}]}));
    }
    // Requests that name things the caller must not learn exist.
    out.push(json!({"read":"Doc","select":["id","salary"]}));
    out.push(json!({"read":"Doc","select":["id","owner"]}));
    out.push(json!({"read":"Member","select":["id"]}));
    out.push(json!({"read":"Doc","select":["id"],"filter":[{"field":"note","op":"eq","value":"x"}]}));
    out.push(json!({"read":"Doc","select":["id"],"sort":[{"field":"note"}]}));
    out
}

async fn answers(db: &mut tokio_postgres::Client, facts: &Value, actor: Option<i64>) -> Vec<String> {
    let mut out = Vec::new();
    for request in attacks() {
        let answer = match plan_read(facts, &request, &who(actor)) {
            Err(error) => format!("rejected {}", error.code),
            Ok(plan) => match execute(db, &plan).await {
                Ok(rows) => format!("rows {}", Value::Array(rows)),
                Err(error) => format!("failed {error:?}"),
            },
        };
        out.push(format!("{request} => {answer}"));
    }
    out
}

#[tokio::test]
async fn data_the_caller_may_not_read_never_changes_what_the_caller_learns() {
    sqlgen::set_schema(&format!("aip_adv_read_{}", std::process::id()));
    let facts = load_str(DEF, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let mut db = connect().await;
    for statement in sqlgen::ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("DDL {statement}: {error}"));
    }
    let s = sqlgen::schema();
    // World A. Documents 2 and 3 are member 1's HIGH documents; members 2 and anonymous may not read them.
    db.batch_execute(&format!(
        "INSERT INTO {s}.member(id) VALUES (1),(2);
         INSERT INTO {s}.folder(id,name) VALUES (1,'shared'),(2,'only-secret');
         INSERT INTO {s}.doc(id,owner_id,folder_id,title,score,level,note) VALUES
           (1,1,1,'alpha',10,'LOW','public note'),(2,1,1,'secret-a',100,'HIGH','n2'),(3,1,2,'secret-b',50,'HIGH','n3'),
           (4,2,1,'mine',5,'LOW','my note'),(5,2,1,'my-high',7,'HIGH','my high note');
         INSERT INTO {s}.item(id,doc_id,label) VALUES (1,2,'to-secret-a'),(2,1,'to-alpha'),(3,NULL,'to-nothing'),(4,3,'to-secret-b');
         INSERT INTO {s}.profile(id,owner_id,nickname,salary) VALUES (1,1,'ann',9000),(2,2,'bob',4000)"
    ))
    .await
    .unwrap();
    let mut world_a = Vec::new();
    for actor in [Some(1), Some(2), None] {
        world_a.push(answers(&mut db, &facts, actor).await);
    }
    // World B changes only what member 2 and anonymous may not read: hidden documents' values and existence, the
    // references that point at them (a reference to an unreadable document reads as null either way), and member 1's salary.
    db.batch_execute(&format!(
        "UPDATE {s}.item SET doc_id=NULL WHERE id=4;
         DELETE FROM {s}.doc WHERE id=3;
         UPDATE {s}.doc SET score=1,title='zzzzzz',note='changed' WHERE id=2;
         UPDATE {s}.profile SET salary=1234 WHERE id=1;
         UPDATE {s}.doc SET note='my other note' WHERE id=4"
    ))
    .await
    .unwrap();
    // The last statement changes member 2's own note, which only member 2 may see: it belongs to the control, not to the oracle.
    let mut world_b = Vec::new();
    for actor in [Some(1), Some(2), None] {
        world_b.push(answers(&mut db, &facts, actor).await);
    }
    db.batch_execute(&format!("DROP SCHEMA {s} CASCADE")).await.unwrap();

    // Negative control: the owner of the hidden data does see the difference, so the requests can detect a leak.
    let owner_differences = world_a[0].iter().zip(&world_b[0]).filter(|(a, b)| a != b).count();
    assert!(owner_differences >= 20, "the attack set is too weak to detect leaks: the owner saw only {owner_differences} differences");
    // Member 2's own note changed, so exactly the requests that select `note` of document 4 may differ for member 2.
    for (label, index) in [("anonymous", 2usize)] {
        for (a, b) in world_a[index].iter().zip(&world_b[index]) {
            assert_eq!(a, b, "{label} learned something from data they may not read");
        }
    }
    for (a, b) in world_a[1].iter().zip(&world_b[1]) {
        if a != b {
            let a_note_only = a.replace("my note", "my other note") == *b;
            assert!(a_note_only, "member 2 learned something from data they may not read:\n  A: {a}\n  B: {b}");
        }
    }
    // The attack set must mostly be requests the engine accepts and answers, otherwise equal answers would only mean "refused".
    let answered = world_a[1].iter().filter(|line| line.contains("=> rows")).count();
    let refused: Vec<&String> = world_a[1].iter().filter(|line| line.contains("=> rejected")).collect();
    eprintln!("member 2: {} requests, {} answered, {} refused", world_a[1].len(), answered, refused.len());
    for line in &refused {
        eprintln!("  refused: {line}");
    }
    assert!(answered * 10 >= world_a[1].len() * 6, "most attack requests must be answered, not refused");
    // The hidden salary reads as null for everybody but its owner, and the filters that would expose it are refused.
    let salary_read = world_a[1].iter().find(|line| line.contains("\"read\":\"Profile\"") && line.contains("\"salary\"]")).expect("salary read");
    // Member 2 sees their own salary (4000) and a null for member 1's.
    assert!(salary_read.contains("\"salary\":null") && salary_read.contains("4000") && !salary_read.contains("9000"), "{salary_read}");
    for line in world_a[1].iter().filter(|line| line.contains("\"field\":\"salary\"") || line.contains("\"field\":\"note\"")) {
        assert!(line.contains("rejected"), "a filter or sort on a policy-hidden field must be refused: {line}");
    }
}

#[test]
fn a_definition_cannot_offer_filters_sorts_or_aggregates_over_policy_hidden_fields() {
    let with = |expose: &str, extra: &str| {
        format!(
            "resource Member {{ fields {{ id: Id }} }}\nactor Member\n{extra}\nresource Profile {{\n  fields {{ id: Id; owner: Member; salary: Int }}\n  rows read when true\n  field salary read when owner = actor\n  expose read {{ select id; {expose}; budget {{ rows 5; depth 1; deadline 1s; cost 100 }} }}\n}}"
        )
    };
    for (what, source) in [
        ("filter", with("filter salary.gte", "")),
        ("sort", with("sort salary", "")),
        (
            "aggregate",
            "resource Member { fields { id: Id } }\nactor Member\naccess seeAll = totalOfVisible(Profile)\nresource Profile {\n  fields { id: Id; owner: Member; salary: Int }\n  rows read when true\n  field salary read when owner = actor\n  expose read { select id, total; budget { rows 5; depth 1; deadline 1s; cost 100 } }\n  aggregate total: Int { sourceAccess seeAll; callerFilter none; rowOutput none; release sum(salary) }\n}"
                .to_owned(),
        ),
    ] {
        assert!(load_str(&source, Form::A).is_err(), "a {what} over a field with a read policy must be a definition error");
    }
}
