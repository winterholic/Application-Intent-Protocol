//! Which writes need SERIALIZABLE (security audit O-26, decision CD-5, 2026-10-13).
//!
//! A policy that needs some row to be ABSENT (`not exists`) is decided on a snapshot. Row locks cannot protect a row
//! that does not exist yet, so two requests that change two different rows can both pass it and both commit
//! (the double booking). Those writes run in a SERIALIZABLE transaction and the server retries a serialization
//! failure. Everything else keeps the current isolation level.
//!
//! What is deliberately NOT marked: a condition that needs another row to be present (`exists`, a reference hop such
//! as `this.room.open`, the actor's row). The write path already takes `FOR SHARE` on every row such a condition
//! reads (`Ctx::lock_reads`), so a concurrent change to those rows waits or fails instead of racing. Marking them
//! would add retries to every ordinary role check (`managerOf`) for no gain. If that ever proves wrong, widening
//! `depends_on_absence` to "reads another row" is a one-function change.
//!
//! The marker is computed here, once, from the compiled facts and written next to the thing it applies to:
//! `transitions.<t>.isolation`, `exposeCreate.isolation`, `exposeCompose.isolation`, each `"serializable"` or absent.
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub const SERIALIZABLE: &str = "serializable";

/// True when the condition needs some row to be absent: an `exists` that is reached through an odd number of `not`s,
/// directly, inside a nested `exists`, or inside a predicate it calls. `neg` is the current polarity.
fn depends_on_absence(cond: &Value, predicates: &Value, neg: bool, seen: &mut BTreeSet<(String, bool)>) -> bool {
    match cond {
        Value::Object(o) => {
            if let Some(inner) = o.get("not") {
                return depends_on_absence(inner, predicates, !neg, seen);
            }
            if o.contains_key("exists") {
                return neg || o.get("where").is_some_and(|w| depends_on_absence(w, predicates, neg, seen));
            }
            if let Some(name) = o.get("call").and_then(Value::as_str) {
                // each predicate is expanded once per polarity; recursion is rejected at definition time
                return seen.insert((name.to_string(), neg)) && depends_on_absence(&predicates[name]["body"], predicates, neg, seen);
            }
            o.values().any(|v| depends_on_absence(v, predicates, neg, seen))
        }
        Value::Array(a) => a.iter().any(|v| depends_on_absence(v, predicates, neg, seen)),
        _ => false,
    }
}

fn reads(cond: &Value, predicates: &Value) -> bool {
    depends_on_absence(cond, predicates, false, &mut BTreeSet::new())
}

/// Adds the `isolation` markers to compiled execution facts.
pub fn mark(facts: &mut Value) {
    let predicates = facts["predicates"].clone();
    let Some(resources) = facts["resources"].as_object().cloned() else { return };
    // A resource with a commit-time `check` that needs a row to be absent is re-checked at every write to it.
    let checked: BTreeSet<&str> = resources
        .iter()
        .filter(|(_, rf)| rf["checks"].as_object().is_some_and(|checks| checks.values().any(|c| reads(c, &predicates))))
        .map(|(name, _)| name.as_str())
        .collect();

    let mut transition_marks: Vec<(String, String)> = vec![];
    let mut create_marks: Vec<String> = vec![];
    for (name, rf) in &resources {
        for (tr, t) in rf["transitions"].as_object().into_iter().flatten() {
            // writes reach the resource itself and the targets of its effects
            let targets = std::iter::once(name.as_str()).chain(t["effects"].as_array().into_iter().flatten().flat_map(|e| ["create", "update"].into_iter().filter_map(|k| e[k].as_str())));
            if reads(&t["allow"], &predicates) || reads(&t["from"], &predicates) || targets.into_iter().any(|x| checked.contains(x)) {
                transition_marks.push((name.clone(), tr.clone()));
            }
        }
        if !rf["exposeCreate"].is_null() && (reads(&rf["exposeCreate"]["allow"], &predicates) || checked.contains(name.as_str())) {
            create_marks.push(name.clone());
        }
    }
    for (res, tr) in &transition_marks {
        facts["resources"][res]["transitions"][tr]["isolation"] = json!(SERIALIZABLE);
    }
    for res in &create_marks {
        facts["resources"][res]["exposeCreate"]["isolation"] = json!(SERIALIZABLE);
    }
    // A compose runs its listed transitions and creates in one transaction, so it needs the strictest of them.
    let mut compose_marks: Vec<String> = vec![];
    for (name, rf) in facts["resources"].as_object().into_iter().flatten() {
        let cp = &rf["exposeCompose"];
        if cp.is_null() {
            continue;
        }
        let transitions = cp["transitions"].as_array().into_iter().flatten().filter_map(Value::as_str).any(|t| rf["transitions"][t]["isolation"] == SERIALIZABLE);
        let creates = cp["creates"].as_object().into_iter().flatten().any(|(target, _)| facts["resources"][target.as_str()]["exposeCreate"]["isolation"] == SERIALIZABLE);
        if transitions || creates || checked.contains(name.as_str()) {
            compose_marks.push(name.clone());
        }
    }
    for res in compose_marks {
        facts["resources"][res.as_str()]["exposeCompose"]["isolation"] = json!(SERIALIZABLE);
    }
}
