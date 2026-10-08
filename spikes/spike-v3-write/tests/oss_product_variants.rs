//! Saleor's nullable Product.default_variant (unique FK) points back into ProductVariant.product.
//! https://github.com/saleor/saleor/blob/bb75f87973abe24056a5d0dff1708ca097e95599/saleor/product/models.py#L203-L209
//! https://github.com/saleor/saleor/blob/bb75f87973abe24056a5d0dff1708ca097e95599/saleor/product/models.py#L355-L360
//! Ownership, publication and activation policies below are AIP probe rules, not a Saleor API port.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    connect_with_url, execute,
    plan::{plan_read, Caller},
    sqlgen,
};
use spike_v3_write::{apply, Knobs};

const SOURCE: &str = r#"
enum VariantStatus { DRAFT, ACTIVE }
resource Member { fields { id: Id } }
actor Member
resource Product {
  fields { id: Id; owner: Member; published: Bool; defaultVariant: ProductVariant? }
  unique defaultVariant
  rows read when published = true or owner = actor
  expose read {
    select id
    sort id
    traverse defaultVariant { select id, sku, status }
    traverse variants via ProductVariant.product { select id, sku, status; sort id; limit 5 }
    budget { rows 10; depth 2; deadline 2s; cost 100 }
  }
}
resource ProductVariant {
  fields { id: Id; product: Product; sku: Text; status: VariantStatus }
  rows read when product.owner = actor or (product.published = true and status = ACTIVE)
  transition activate {
    allow product.owner = actor
    from status = DRAFT
    to status = ACTIVE
    repeat unchanged
    update Product where id = product { defaultVariant = this.id }
    notify product.owner "product.default_changed"
  }
  expose apply activate { target id; bulk maxRows 1 }
  expose read { select id, sku, status; sort id; budget { rows 10; depth 1; deadline 2s; cost 100 } }
}
"#;

fn caller(id: Option<i64>) -> Caller {
    Caller { actor_id: id, now: "2026-10-08T00:00:00Z".into() }
}

async fn read(db: &mut tokio_postgres::Client, facts: &Value, who: Option<i64>) -> Vec<Value> {
    let request = json!({"read":"Product","select":["id",{"defaultVariant":{"select":["id","sku","status"]}},{"variants":{"select":["id","sku","status"]}}],"sort":[{"field":"id"}]});
    execute(db, &plan_read(facts, &request, &caller(who)).unwrap()).await.unwrap()
}

#[tokio::test]
async fn cyclic_default_variant_obeys_visibility_uniqueness_and_atomic_activation() {
    let facts = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let schema =
        format!("aip_variants_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url("host=localhost dbname=postgres").await.unwrap();
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES(1),(2),(3); \
         INSERT INTO {schema}.product(id,owner_id,published) VALUES(10,1,false),(20,2,false),(30,2,true),(40,1,true); \
         INSERT INTO {schema}.product_variant(id,product_id,sku,status) VALUES \
           (101,10,'red','DRAFT'),(102,10,'blue','DRAFT'),(201,20,'private','DRAFT'), \
           (301,30,'public','ACTIVE'),(302,30,'hidden','DRAFT'); \
         UPDATE {schema}.product SET default_variant_id=301 WHERE id=30"
    ))
    .await
    .unwrap();
    let public = vec![
        json!({"id":30,"defaultVariant":{"id":301,"sku":"public","status":"ACTIVE"},"variants":[{"id":301,"sku":"public","status":"ACTIVE"}]}),
        json!({"id":40,"defaultVariant":null,"variants":[]}),
    ];
    for outsider in [None, Some(3)] {
        assert_eq!(read(&mut db, &facts, outsider).await, public);
    }
    let owner = read(&mut db, &facts, Some(1)).await;
    assert_eq!(owner[0]["id"], 10);
    assert_eq!(owner[0]["defaultVariant"], Value::Null);
    assert_eq!(owner[0]["variants"].as_array().unwrap().len(), 2);
    let activate = |id: i64| json!({"apply":"ProductVariant.activate","target":{"ids":[id.to_string()]}});
    for outsider in [None, Some(3)] {
        assert!(apply(&mut db, &facts, &activate(101), &caller(outsider), &Knobs::default()).await.is_err());
    }
    assert!(apply(&mut db, &facts, &activate(201), &caller(Some(1)), &Knobs::default()).await.is_err());
    for id in [101, 102] {
        assert_eq!(apply(&mut db, &facts, &activate(id), &caller(Some(1)), &Knobs::default()).await.unwrap().changed.len(), 1);
        let retry = apply(&mut db, &facts, &activate(id), &caller(Some(1)), &Knobs::default()).await.unwrap();
        assert!(retry.changed.is_empty());
        assert_eq!(retry.unchanged.len(), 1);
    }
    let owner = read(&mut db, &facts, Some(1)).await;
    assert_eq!(owner[0]["defaultVariant"], json!({"id":102,"sku":"blue","status":"ACTIVE"}));
    assert_eq!(owner[0]["variants"].as_array().unwrap().len(), 2);
    assert_eq!(read(&mut db, &facts, None).await, public, "activation does not publish the draft product");
    let state = db.query_one(&format!("SELECT (SELECT status FROM {schema}.product_variant WHERE id=201), (SELECT default_variant_id FROM {schema}.product WHERE id=20), (SELECT count(*) FROM {schema}.aip_outbox)"), &[]).await.unwrap();
    assert_eq!(state.get::<_, String>(0), "DRAFT");
    assert_eq!(state.get::<_, Option<i64>>(1), None);
    assert_eq!(state.get::<_, i64>(2), 2);
    db.batch_execute(&format!("UPDATE {schema}.product SET default_variant_id=201 WHERE id=40")).await.unwrap();
    assert_eq!(read(&mut db, &facts, None).await, public, "the referenced private variant remains hidden in a public product");
    let conflict = apply(&mut db, &facts, &activate(201), &caller(Some(2)), &Knobs::default()).await.unwrap_err();
    assert_eq!(conflict.code, "ALREADY_EXISTS");
    let state = db.query_one(&format!("SELECT (SELECT status FROM {schema}.product_variant WHERE id=201), (SELECT default_variant_id FROM {schema}.product WHERE id=20), (SELECT count(*) FROM {schema}.aip_outbox)"), &[]).await.unwrap();
    assert_eq!(state.get::<_, String>(0), "DRAFT", "the parent unique failure rolls back the variant activation");
    assert_eq!(state.get::<_, Option<i64>>(1), None);
    assert_eq!(state.get::<_, i64>(2), 2, "a failed activation adds no notification");
    db.batch_execute(&format!("UPDATE {schema}.product SET default_variant_id=NULL WHERE id=40")).await.unwrap();
    for (statement, code) in [
        (format!("UPDATE {schema}.product SET default_variant_id=999 WHERE id=40"), "23503"),
        (format!("UPDATE {schema}.product_variant SET product_id=999 WHERE id=101"), "23503"),
        (format!("UPDATE {schema}.product SET default_variant_id=102 WHERE id=40"), "23505"),
    ] {
        assert_eq!(db.batch_execute(&statement).await.unwrap_err().code().unwrap().code(), code);
    }
    assert_eq!(read(&mut db, &facts, Some(1)).await, owner, "failed FK/unique writes preserve both references");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
