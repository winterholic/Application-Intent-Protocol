//! End-to-end for the Shop example: a second domain, so the forms are not only
//! checked against AriAri. Covers shared stock under concurrency, optimistic
//! concurrency on staff edits, and Stripe webhooks (signature, dedupe, retry-safe
//! processing from the outbox).

mod common;

use common::{call, drain, fails, ok, sql_one};
use hmac::{Hmac, Mac};
use serde_json::json;
use std::collections::HashMap;

const SECRET: &str = "whsec_test_only";

fn stripe_headers(body: &str, t: i64) -> HashMap<String, String> {
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(SECRET.as_bytes()).expect("key");
    mac.update(format!("{t}.{body}").as_bytes());
    let sig: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
    HashMap::from([("stripe-signature".to_string(), format!("t={t},v1={sig}"))])
}

fn intent_event(id: &str, kind: &str, order: &str) -> String {
    json!({"id": id, "type": kind, "data": {"object": {"id": format!("pi_{id}"), "amount": 3000, "currency": "krw", "metadata": {"orderId": order}}}})
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shop_end_to_end() {
    // SAFETY: this test binary has a single test, and nothing else reads the environment concurrently
    unsafe { std::env::set_var("STRIPE_WEBHOOK_SECRET", SECRET) };
    let e = common::setup_app("shop", "aip_e2e_shop").await;
    let customer = |email: &'static str, role: &'static str| {
        let e = e.clone();
        async move {
            sql_one(&e, &format!("INSERT INTO customer (email, role) VALUES ('{email}', '{role}') RETURNING id"))
                .await
                .as_str()
                .expect("id")
                .to_string()
        }
    };
    let alice = customer("alice@x.com", "USER").await;
    let bob = customer("bob@x.com", "USER").await;
    let staff = customer("staff@x.com", "STAFF").await;
    let staff2 = customer("staff2@x.com", "STAFF").await;
    let product = sql_one(&e, "INSERT INTO product (name, price, stock) VALUES ('Mug', 1000, 5) RETURNING id").await.as_str().expect("p").to_string();

    // --- orders take stock with a relative update; the DB refuses overselling
    let o1 = ok(&e, "PlaceOrder", Some(&alice), json!({"product": product, "quantity": 3}), Some("o1")).await;
    assert_eq!((o1["amount"].as_i64(), o1["status"].as_str()), (Some(3000), Some("PENDING")));
    let replay = ok(&e, "PlaceOrder", Some(&alice), json!({"product": product, "quantity": 3}), Some("o1")).await;
    assert_eq!(replay["id"], o1["id"]);
    let (a, b) = tokio::join!(
        call(&e, "PlaceOrder", Some(&alice), json!({"product": product, "quantity": 2}), Some("o2")),
        call(&e, "PlaceOrder", Some(&bob), json!({"product": product, "quantity": 2}), Some("o3"))
    );
    assert_eq!(a.is_ok() as u8 + b.is_ok() as u8, 1, "only one of two orders for the last 2 fits: {:?} {:?}", a.as_ref().err(), b.as_ref().err());
    let refused = a.err().or(b.err()).expect("one refusal");
    assert_eq!((refused.code.as_str(), refused.reason.as_deref()), ("AIP.INVARIANT.VIOLATED", Some("OUT_OF_STOCK")));
    assert_eq!(sql_one(&e, &format!("SELECT stock FROM product WHERE id = '{product}'")).await, json!(0));
    let order = o1["id"].as_str().expect("order").to_string();
    let hidden = fails(&e, "MyOrder", Some(&bob), json!({"order": order}), None).await;
    assert_eq!(hidden.code, "AIP.NOT_FOUND");

    // --- staff edits: stock movements do not make an edit stale, a concurrent edit does
    let catalog = ok(&e, "Catalog", None, json!({}), None).await;
    let v = catalog[0]["version"].as_i64().expect("version");
    assert_eq!(v, 1, "taking stock does not bump the version");
    let customer_edit =
        fails(&e, "UpdateProduct", Some(&alice), json!({"product": product, "productVersion": v, "name": "x", "price": 1}), None).await;
    assert_eq!(customer_edit.code, "AIP.AUTH.FORBIDDEN");
    ok(&e, "UpdateProduct", Some(&staff), json!({"product": product, "productVersion": v, "name": "Mug", "price": 1200}), None).await;
    let stale = fails(&e, "UpdateProduct", Some(&staff2), json!({"product": product, "productVersion": v, "name": "Cup", "price": 900}), None).await;
    assert_eq!(stale.code, "AIP.CONFLICT.STALE_VERSION");
    assert_eq!(sql_one(&e, &format!("SELECT price FROM product WHERE id = '{product}'")).await, json!(1200));

    // --- Stripe webhook: forged and replayed deliveries are refused before anything is stored
    let now = chrono::Utc::now().timestamp();
    let body = intent_event("evt_1", "payment_intent.succeeded", &order);
    let forged = aip_runtime::webhook::receive(&e, "StripePayments", &stripe_headers(&body, now), body.replace("3000", "1").as_bytes()).await;
    assert_eq!(forged.expect_err("forged").code, "AIP.AUTH.UNAUTHENTICATED");
    let old = aip_runtime::webhook::receive(&e, "StripePayments", &stripe_headers(&body, now - 600), body.as_bytes()).await;
    assert!(old.is_err(), "outside the 5 minute window");
    assert_eq!(sql_one(&e, "SELECT count(*) FROM _aip_outbox WHERE kind = 'webhook'").await, json!(0));

    // --- a genuine delivery is acknowledged at once and applied by the dispatcher; redelivery is a no-op
    let ack = aip_runtime::webhook::receive(&e, "StripePayments", &stripe_headers(&body, now), body.as_bytes()).await.expect("ack");
    assert_eq!(ack, json!({"received": true, "duplicate": false, "handled": true}));
    assert_eq!(ok(&e, "MyOrder", Some(&alice), json!({"order": order}), None).await["status"], "PENDING", "not applied before dispatch");
    let again = aip_runtime::webhook::receive(&e, "StripePayments", &stripe_headers(&body, now + 1), body.as_bytes()).await.expect("ack");
    assert_eq!(again["duplicate"], json!(true));
    drain(&e).await;
    let paid = ok(&e, "MyOrder", Some(&alice), json!({"order": order}), None).await;
    assert_eq!((paid["status"].as_str(), paid["paymentRef"].as_str()), (Some("PAID"), Some("pi_evt_1")));
    assert_eq!(sql_one(&e, "SELECT count(*) FROM _aip_outbox WHERE kind = 'webhook'").await, json!(1));

    // --- a late failure event for an already paid order changes nothing (and does not fail forever)
    let late = intent_event("evt_2", "payment_intent.payment_failed", &order);
    aip_runtime::webhook::receive(&e, "StripePayments", &stripe_headers(&late, now), late.as_bytes()).await.expect("ack");
    let unhandled = intent_event("evt_3", "charge.refunded", &order);
    let ack = aip_runtime::webhook::receive(&e, "StripePayments", &stripe_headers(&unhandled, now), unhandled.as_bytes()).await.expect("ack");
    assert_eq!(ack["handled"], json!(false), "events without a handler are acknowledged and dropped");
    drain(&e).await;
    assert_eq!(ok(&e, "MyOrder", Some(&alice), json!({"order": order}), None).await["status"], "PAID");
    assert_eq!(sql_one(&e, "SELECT count(*) FROM _aip_outbox WHERE kind = 'webhook' AND done_at IS NULL").await, json!(0));

    // --- rules are edge-triggered: sold out fires once, re-arms after a restock, fires again
    let sold_out = "SELECT count(*) FROM _aip_notification WHERE template = 'sold-out'";
    assert_eq!(sql_one(&e, sold_out).await, json!(2), "one notice per staff member when stock hit 0");
    ok(&e, "Restock", Some(&staff), json!({"product": product, "quantity": 3}), None).await;
    ok(&e, "PlaceOrder", Some(&bob), json!({"product": product, "quantity": 1}), Some("o4")).await;
    drain(&e).await;
    assert_eq!(sql_one(&e, sold_out).await, json!(2), "stock 2 is not sold out; no new notice");
    ok(&e, "PlaceOrder", Some(&bob), json!({"product": product, "quantity": 2}), Some("o5")).await;
    drain(&e).await;
    assert_eq!(sql_one(&e, sold_out).await, json!(4), "sold out again after the restock");

    // --- a rule over a related table, triggered from the webhook dispatcher
    let catalog = ok(&e, "Catalog", None, json!({}), None).await;
    assert_eq!(catalog[0]["featured"], json!(false), "one paid order is not enough");
    let second = sql_one(&e, "SELECT id FROM \"order\" WHERE status = 'PENDING' ORDER BY created_at LIMIT 1").await;
    let paid2 = intent_event("evt_4", "payment_intent.succeeded", second.as_str().expect("order"));
    aip_runtime::webhook::receive(&e, "StripePayments", &stripe_headers(&paid2, now), paid2.as_bytes()).await.expect("ack");
    drain(&e).await;
    let catalog = ok(&e, "Catalog", None, json!({}), None).await;
    assert_eq!(catalog[0]["featured"], json!(true), "the second paid order made it a bestseller");
    assert_eq!(sql_one(&e, "SELECT count(*) FROM _aip_rule_pending").await, json!(0), "nothing left unsettled");
}
