use serde_json::json;
use spike_v2_read::sqlgen::{Ctx, Dialect, Env};

fn facts() -> serde_json::Value {
    json!({"resources":{
      "Order":{"fields":{"id":{"ty":"Id"},"label":{"ty":"Text?"},"created":{"ty":"Time?"}}},
      "Item":{"fields":{"id":{"ty":"Id"},"order":{"ty":"Ref<Order>"},"active":{"ty":"Bool"},"label":{"ty":"Text"},"created":{"ty":"Time?"}}}
    }})
}

fn latest() -> serde_json::Value {
    json!({"latest":{
      "resource":"Item","alias":"candidate","tieBreak":"idDesc",
      "where":{"and":[
        {"cmp":"=","l":{"path":{"root":"var.candidate","segs":["order"]},"ty":"Ref<Order>"},"r":{"path":{"root":"this","segs":[]},"ty":"Ref<Order>"}},
        {"cmp":"=","l":{"path":{"root":"var.candidate","segs":["active"]},"ty":"Bool"},"r":{"lit":true}}
      ]},
      "orderBy":{"path":{"root":"var.candidate","segs":["created"]},"ty":"Time?"},
      "select":{"path":{"root":"var.candidate","segs":["label"]},"ty":"Text"},
      "otherwise":{"path":{"root":"this","segs":["label"]},"ty":"Text?"}
    },"ty":"Text?"})
}

#[test]
fn latest_scalar_selection_has_stable_null_and_tie_order_in_both_sql_dialects() {
    let facts = facts();
    let env = Env { this: Some(("parent".into(), "Order".into())), ..Default::default() };
    let pg = Ctx::new(&facts, None, "").value(&latest(), &env).unwrap();
    assert!(pg.contains("COALESCE((SELECT latest1.label FROM aip_v2_spike.item latest1"), "{pg}");
    assert!(pg.contains("ORDER BY (latest1.created IS NULL) ASC, latest1.created DESC, latest1.id DESC LIMIT 1)"), "{pg}");
    assert!(pg.ends_with(", parent.label)"), "{pg}");

    let mysql = Ctx::new(&facts, None, "").with_dialect(Dialect::MySql).value(&latest(), &env).unwrap();
    assert!(mysql.contains("COALESCE((SELECT `latest1`.`label` FROM `aip_v2_spike`.`item` `latest1`"), "{mysql}");
    assert!(mysql.contains("ORDER BY (`latest1`.`created` IS NULL) ASC, `latest1`.`created` DESC, `latest1`.`id` DESC LIMIT 1)"), "{mysql}");
    assert!(mysql.ends_with(", `parent`.`label`)"), "{mysql}");
}
