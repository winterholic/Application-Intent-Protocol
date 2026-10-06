//! Output shapes: what a query or command returns, in the JSON form clients
//! generate types from.

use crate::ty::inferred;
use aip_ir::{Shape, Type};
use serde_json::{Value, json};

fn nullable(mut v: Value, yes: bool) -> Value {
    if yes && let Value::Object(m) = &mut v {
        m.insert("nullable".into(), json!(true));
    }
    v
}

fn value(ty: &Type) -> Value {
    match ty {
        Type::Snapshot { .. } => json!({"kind": "snapshot"}),
        Type::RefUnion { entities } => json!({"kind": "union_ref", "entities": entities}),
        Type::Unknown => json!({"kind": "unknown"}),
        other => serde_json::to_value(inferred(other)).unwrap_or(Value::Null),
    }
}

/// `{ "kind": "object", "fields": { name: shape }, "nullable": bool }`
pub fn to_json(shape: &Shape) -> Value {
    match shape {
        Shape::None => Value::Null,
        Shape::Value { ty, nullable: n } => nullable(value(ty), *n),
        Shape::Object { fields, nullable: n } => {
            let mut m = serde_json::Map::new();
            for (name, s) in fields {
                m.insert(name.clone(), to_json(s));
            }
            nullable(json!({"kind": "object", "fields": m}), *n)
        }
        Shape::List { of, nullable: n } => nullable(json!({"kind": "list", "of": to_json(of)}), *n),
    }
}
