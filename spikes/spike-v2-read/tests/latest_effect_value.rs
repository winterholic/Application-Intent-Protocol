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
    assert!(pg.starts_with("(CASE WHEN EXISTS (SELECT 1 FROM aip_v2_spike.item latest1 WHERE "), "{pg}");
    assert!(pg.contains("THEN (SELECT latest1.label FROM aip_v2_spike.item latest1"), "{pg}");
    assert!(pg.contains("ORDER BY (latest1.created IS NULL) ASC, latest1.created DESC, latest1.id DESC LIMIT 1)"), "{pg}");
    assert!(pg.ends_with(" ELSE parent.label END)"), "{pg}");

    let mysql = Ctx::new(&facts, None, "").with_dialect(Dialect::MySql).value(&latest(), &env).unwrap();
    // Ctx generates safe aliases directly; Dialect::qualified quotes the column.
    assert!(mysql.starts_with("(CASE WHEN EXISTS (SELECT 1 FROM `aip_v2_spike`.`item` latest1 WHERE "), "{mysql}");
    assert!(mysql.contains("THEN (SELECT latest1.`label` FROM `aip_v2_spike`.`item` latest1"), "{mysql}");
    assert!(mysql.contains("ORDER BY (latest1.`created` IS NULL) ASC, latest1.`created` DESC, latest1.`id` DESC LIMIT 1)"), "{mysql}");
    assert!(mysql.ends_with(" ELSE parent.`label` END)"), "{mysql}");
}

#[test]
fn latest_presence_check_repeats_mysql_bindings_in_sql_order() {
    let facts = facts();
    let env = Env { this: Some(("parent".into(), "Order".into())), ..Default::default() };
    let mut expression = latest();
    expression["latest"]["otherwise"] = json!({"lit":"missing-row","ty":"Text"});
    let mut ctx = Ctx::new(&facts, None, "").with_dialect(Dialect::MySql);
    let sql = ctx.value(&expression, &env).unwrap();
    let (sql, values) = ctx.params.finish(sql);
    assert_eq!(sql.matches('?').count(), 3, "{sql}");
    assert_eq!(values, [Some("1".into()), Some("1".into()), Some("missing-row".into())]);
    assert!(!sql.contains("COALESCE"), "selected NULL must not activate otherwise: {sql}");
}

#[test]
fn latest_without_fallback_keeps_a_single_selection() {
    let facts = facts();
    let env = Env { this: Some(("parent".into(), "Order".into())), ..Default::default() };
    let mut expression = latest();
    expression["latest"]["otherwise"] = serde_json::Value::Null;
    for dialect in [Dialect::Postgres, Dialect::MySql] {
        let mut ctx = Ctx::new(&facts, None, "").with_dialect(dialect);
        let sql = ctx.value(&expression, &env).unwrap();
        assert!(sql.starts_with("(SELECT "), "{sql}");
        assert!(!sql.contains("CASE"), "{sql}");
        assert_eq!(ctx.params.finish(sql).1.len(), 1);
    }
}
