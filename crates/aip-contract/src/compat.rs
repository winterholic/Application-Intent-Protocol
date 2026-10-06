//! Compatibility of two versions of a program as clients see them (`aip diff`): which changes to the public contract
//! break a client written against the old version, which only change behavior, and which are safe.
//!
//! It compares what `describe` publishes (inputs, outputs, errors, idempotency, enums, records) and, for what the
//! contract does not carry, the policy of declared intents (`allow`, `requires`). The rules follow `docs/design/03-ir.md`
//! section 10: a required input field added, an output field removed, an intent removed or renamed, an input type made
//! narrower, an output made nullable and an idempotency removed are breaking; a new error code and a changed policy
//! are warnings (the behavior changes, the shape does not); everything that only adds is compatible.

use crate::describe;
use aip_ir as ir;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Breaking,
    Warning,
    Compatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub level: Level,
    /// Stable name of the rule, `<area>.<what>`.
    pub rule: &'static str,
    /// What it is about: `command AddTask`, `input AddTask.title`, `output Tasks.items.title`.
    pub subject: String,
    pub message: String,
}

pub fn compare(old: &ir::Program, new: &ir::Program) -> Vec<Finding> {
    let mut c = Cmp { out: Vec::new(), input_enums: BTreeSet::new(), output_enums: BTreeSet::new(), input_records: BTreeSet::new() };
    let (od, nd) = (describe(old), describe(new));
    c.collect_uses(&od);
    c.collect_uses(&nd);
    for area in ["queries", "commands", "subscriptions"] {
        c.intents(area, &od, &nd);
    }
    c.cross_kind(&od, &nd);
    c.enums(&od, &nd);
    c.records(&od, &nd);
    c.policies(old, new);
    c.out.sort_by(|a, b| (a.level, &a.subject, a.rule).cmp(&(b.level, &b.subject, b.rule)));
    c.out
}

fn json(v: &impl Serialize) -> Value {
    serde_json::to_value(v).unwrap_or_default()
}

/// True when a client written against the old version can break.
pub fn breaks(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.level == Level::Breaking)
}

struct Cmp {
    out: Vec<Finding>,
    input_enums: BTreeSet<String>,
    output_enums: BTreeSet<String>,
    input_records: BTreeSet<String>,
}

fn singular(area: &str) -> &'static str {
    match area {
        "queries" => "query",
        "commands" => "command",
        _ => "subscription",
    }
}

fn map<'a>(doc: &'a Value, area: &str) -> BTreeMap<&'a str, &'a Value> {
    doc.get(area).and_then(Value::as_object).map(|m| m.iter().map(|(k, v)| (k.as_str(), v)).collect()).unwrap_or_default()
}

/// Every `{ "kind": <kind>, "name": n }` under `v`.
fn named(v: &Value, kind: &str, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(m) => {
            if m.get("kind").and_then(Value::as_str) == Some(kind)
                && let Some(n) = m.get("name").and_then(Value::as_str)
            {
                out.insert(n.to_string());
            }
            m.values().for_each(|x| named(x, kind, out));
        }
        Value::Array(a) => a.iter().for_each(|x| named(x, kind, out)),
        _ => {}
    }
}

impl Cmp {
    fn add(&mut self, level: Level, rule: &'static str, subject: impl Into<String>, message: impl Into<String>) {
        self.out.push(Finding { level, rule, subject: subject.into(), message: message.into() });
    }

    fn collect_uses(&mut self, doc: &Value) {
        for area in ["queries", "commands", "subscriptions"] {
            for item in map(doc, area).values() {
                if let Some(i) = item.get("input") {
                    named(i, "enum", &mut self.input_enums);
                    named(i, "record", &mut self.input_records);
                }
                if let Some(o) = item.get("output") {
                    named(o, "enum", &mut self.output_enums);
                }
            }
        }
    }

    fn cross_kind(&mut self, od: &Value, nd: &Value) {
        let kinds = |d: &Value| -> BTreeMap<String, &'static str> {
            ["queries", "commands", "subscriptions"].iter().flat_map(|a| map(d, a).into_keys().map(move |k| (k.to_string(), singular(a)))).collect()
        };
        let (ok, nk) = (kinds(od), kinds(nd));
        for (name, o) in &ok {
            if let Some(n) = nk.get(name)
                && n != o
            {
                self.add(
                    Level::Breaking,
                    "intent.kind_changed",
                    format!("{o} {name}"),
                    format!("{name} was a {o} and is now a {n}: it is called differently"),
                );
            }
        }
    }

    fn intents(&mut self, area: &str, od: &Value, nd: &Value) {
        let kind = singular(area);
        let (o, n) = (map(od, area), map(nd, area));
        for name in o.keys().filter(|k| !n.contains_key(*k)) {
            // a name that moved to another kind is reported once, by `cross_kind`
            if ["queries", "commands", "subscriptions"].iter().any(|a| *a != area && map(nd, a).contains_key(name)) {
                continue;
            }
            self.add(
                Level::Breaking,
                "intent.removed",
                format!("{kind} {name}"),
                "removed or renamed: clients that call it get REQUEST.UNKNOWN_INTENT",
            );
        }
        for name in n.keys().filter(|k| !o.contains_key(*k)) {
            if ["queries", "commands", "subscriptions"].iter().any(|a| *a != area && map(od, a).contains_key(name)) {
                continue;
            }
            self.add(Level::Compatible, "intent.added", format!("{kind} {name}"), "new");
        }
        for (name, ov) in &o {
            let Some(nv) = n.get(name) else { continue };
            self.inputs(name, ov.get("input"), nv.get("input"));
            self.outputs(name, ov.get("output"), nv.get("output"));
            self.errors(&format!("{kind} {name}"), ov.get("errors"), nv.get("errors"));
            if area == "commands" {
                self.idempotency(name, ov, nv);
            }
            if area == "queries" {
                let mode = |v: &Value| v.pointer("/page/mode").and_then(Value::as_str).map(String::from);
                if mode(ov) != mode(nv) && mode(ov).is_some() && mode(nv).is_some() {
                    self.add(
                        Level::Breaking,
                        "page.mode_changed",
                        format!("query {name}"),
                        format!("paging changed from {:?} to {:?}", mode(ov), mode(nv)),
                    );
                }
            }
        }
    }

    fn inputs(&mut self, intent: &str, old: Option<&Value>, new: Option<&Value>) {
        let by_name = |v: Option<&Value>| -> BTreeMap<String, Value> {
            v.and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|p| Some((p.get("name")?.as_str()?.to_string(), p.clone()))).collect())
                .unwrap_or_default()
        };
        self.fields("input", intent, by_name(old), by_name(new));
    }

    /// Parameters of an intent or fields of a record accepted as input.
    fn fields(&mut self, area: &str, owner: &str, old: BTreeMap<String, Value>, new: BTreeMap<String, Value>) {
        let required = |p: &Value| !p.get("optional").and_then(Value::as_bool).unwrap_or(false);
        for (name, o) in &old {
            let subject = format!("{area} {owner}.{name}");
            let Some(n) = new.get(name) else {
                self.add(Level::Breaking, "input.removed", subject, "no longer accepted: a client that still sends it is refused (unknown field)");
                continue;
            };
            match (required(o), required(n)) {
                (false, true) => self.add(Level::Breaking, "input.became_required", subject.clone(), "was optional, is now required"),
                (true, false) => self.add(Level::Compatible, "input.became_optional", subject.clone(), "was required, is now optional"),
                _ => {}
            }
            match relation(o.get("ty"), n.get("ty")) {
                Rel::Same => {}
                Rel::Widens => self.add(Level::Compatible, "input.type_widened", subject, "accepts more values than before"),
                Rel::Narrows => self.add(
                    Level::Breaking,
                    "input.type_narrowed",
                    subject,
                    "accepts fewer values than before: a value that was valid may be refused",
                ),
                Rel::Different => self.add(Level::Breaking, "input.type_changed", subject, "another kind of value"),
            }
        }
        for (name, n) in &new {
            if old.contains_key(name) {
                continue;
            }
            let subject = format!("{area} {owner}.{name}");
            if required(n) {
                self.add(Level::Breaking, "input.required_added", subject, "new required field: calls that do not send it are refused");
            } else {
                self.add(Level::Compatible, "input.optional_added", subject, "new optional field");
            }
        }
    }

    fn outputs(&mut self, intent: &str, old: Option<&Value>, new: Option<&Value>) {
        let (o, n) = (old.unwrap_or(&Value::Null), new.unwrap_or(&Value::Null));
        match (o.is_null(), n.is_null()) {
            (true, true) => {}
            (true, false) => self.add(Level::Compatible, "output.added", format!("output {intent}"), "returns a value now"),
            (false, true) => self.add(Level::Breaking, "output.removed", format!("output {intent}"), "returns nothing now"),
            (false, false) => self.shape(&format!("output {intent}"), o, n),
        }
    }

    fn shape(&mut self, path: &str, o: &Value, n: &Value) {
        let kind = |v: &Value| v.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
        let nullable = |v: &Value| v.get("nullable").and_then(Value::as_bool).unwrap_or(false);
        match (nullable(o), nullable(n)) {
            (false, true) => {
                self.add(Level::Breaking, "output.became_nullable", path, "may be null now: a client that reads it without checking breaks")
            }
            (true, false) => self.add(Level::Compatible, "output.became_required", path, "is never null now"),
            _ => {}
        }
        if kind(o) != kind(n) {
            self.add(Level::Breaking, "output.type_changed", path, format!("{} -> {}", kind(o), kind(n)));
            return;
        }
        match kind(o).as_str() {
            "object" => {
                let fields = |v: &Value| v.get("fields").and_then(Value::as_object).cloned().unwrap_or_default();
                let (of, nf) = (fields(o), fields(n));
                for (name, ov) in &of {
                    match nf.get(name) {
                        None => {
                            self.add(Level::Breaking, "output.field_removed", format!("{path}.{name}"), "removed: clients that read it get nothing")
                        }
                        Some(nv) => self.shape(&format!("{path}.{name}"), ov, nv),
                    }
                }
                for name in nf.keys().filter(|k| !of.contains_key(*k)) {
                    self.add(Level::Compatible, "output.field_added", format!("{path}.{name}"), "new field");
                }
            }
            "list" => {
                if let (Some(a), Some(b)) = (o.get("of"), n.get("of")) {
                    self.shape(&format!("{path}[]"), a, b);
                }
            }
            "enum" | "record" | "entity" if o.get("name") != n.get("name") || o.get("entity") != n.get("entity") => {
                self.add(Level::Breaking, "output.type_changed", path, "another type of the same kind");
            }
            _ => {}
        }
    }

    fn errors(&mut self, subject: &str, old: Option<&Value>, new: Option<&Value>) {
        let set = |v: Option<&Value>| -> BTreeSet<String> {
            v.and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|e| {
                            let code = e.get("code").and_then(Value::as_str).unwrap_or("");
                            match e.get("reason").and_then(Value::as_str) {
                                Some(r) => format!("{code} ({r})"),
                                None => code.to_string(),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let (o, n) = (set(old), set(new));
        for e in n.difference(&o) {
            self.add(Level::Warning, "error.added", subject, format!("can now fail with {e}: clients must handle it"));
        }
        for e in o.difference(&n) {
            self.add(Level::Compatible, "error.removed", subject, format!("no longer fails with {e}"));
        }
    }

    fn idempotency(&mut self, name: &str, o: &Value, n: &Value) {
        let mode = |v: &Value| match (v.get("idempotent").and_then(Value::as_bool).unwrap_or(false), v.get("idempotency").and_then(Value::as_str)) {
            (false, _) => 0,
            (true, Some(s)) if s.starts_with("derived") => 2,
            _ => 1,
        };
        let subject = format!("command {name}");
        match (mode(o), mode(n)) {
            (a, b) if a == b => {}
            (_, 0) => self.add(
                Level::Breaking,
                "idempotency.removed",
                subject,
                "a retry may apply twice now: clients that retry on timeout can duplicate the effect",
            ),
            (a, 1) if a != 1 => self.add(Level::Breaking, "idempotency.key_required", subject, "calls now need an Idempotency-Key header"),
            (0, 2) => self.add(Level::Compatible, "idempotency.added", subject, "retries are safe now (the key is derived from the input)"),
            _ => self.add(Level::Compatible, "idempotency.key_optional", subject, "the key is derived from the input now"),
        }
    }

    fn enums(&mut self, od: &Value, nd: &Value) {
        let list = |d: &Value| -> BTreeMap<String, Vec<String>> {
            d.get("enums")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| {
                            (k.clone(), v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default())
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let (o, n) = (list(od), list(nd));
        for (name, ov) in &o {
            let Some(nv) = n.get(name) else { continue };
            let (is_in, is_out) = (self.input_enums.contains(name), self.output_enums.contains(name));
            for v in ov.iter().filter(|v| !nv.contains(v)) {
                let level = if is_in { Level::Breaking } else { Level::Compatible };
                self.add(level, "enum.value_removed", format!("enum {name}.{v}"), if is_in { "no longer accepted as input" } else { "removed" });
            }
            for v in nv.iter().filter(|v| !ov.contains(v)) {
                if is_out {
                    self.add(Level::Warning, "enum.value_added", format!("enum {name}.{v}"), "outputs may now carry a value old clients do not know");
                } else {
                    self.add(Level::Compatible, "enum.value_added", format!("enum {name}.{v}"), "new value");
                }
            }
        }
    }

    fn records(&mut self, od: &Value, nd: &Value) {
        let list = |d: &Value| -> BTreeMap<String, BTreeMap<String, Value>> {
            d.get("records")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                v.as_array()
                                    .map(|a| a.iter().filter_map(|p| Some((p.get("name")?.as_str()?.to_string(), p.clone()))).collect())
                                    .unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let (o, n) = (list(od), list(nd));
        for (name, of) in &o {
            let Some(nf) = n.get(name) else { continue };
            if self.input_records.contains(name) {
                self.fields("record", name, of.clone(), nf.clone());
            } else {
                for f in of.keys().filter(|f| !nf.contains_key(*f)) {
                    self.add(Level::Breaking, "record.field_removed", format!("record {name}.{f}"), "removed from a record clients read");
                }
                for f in nf.keys().filter(|f| !of.contains_key(*f)) {
                    self.add(Level::Compatible, "record.field_added", format!("record {name}.{f}"), "new field");
                }
            }
        }
    }

    /// `allow` and `requires` are not in the contract, but they decide who may call and when; a change is a change of behavior.
    fn policies(&mut self, old: &ir::Program, new: &ir::Program) {
        for (name, o) in &old.intents {
            let Some(n) = new.intents.get(name) else { continue };
            let (oa, na, kind, requires) = match (o, n) {
                (ir::Intent::Query(a), ir::Intent::Query(b)) if !a.internal && !b.internal => (json(&a.allow), json(&b.allow), "query", None),
                (ir::Intent::Command(a), ir::Intent::Command(b)) if !a.internal && !b.internal => {
                    (json(&a.allow), json(&b.allow), "command", Some((json(&a.requires), json(&b.requires))))
                }
                _ => continue,
            };
            if oa != na {
                self.add(
                    Level::Warning,
                    "policy.allow_changed",
                    format!("{kind} {name}"),
                    "who may call it changed: callers that were allowed may be refused, or the reverse",
                );
            }
            if let Some((a, b)) = requires
                && a != b
            {
                self.add(
                    Level::Warning,
                    "policy.requires_changed",
                    format!("{kind} {name}"),
                    "the preconditions of the call changed: a call that succeeded may fail now, or the reverse",
                );
            }
        }
    }
}

#[derive(PartialEq)]
enum Rel {
    Same,
    Widens,
    Narrows,
    Different,
}

/// How the type a client may send changed, from `{ "kind": ..., ...refinements }`.
fn relation(old: Option<&Value>, new: Option<&Value>) -> Rel {
    let (Some(o), Some(n)) = (old, new) else { return Rel::Same };
    if o == n {
        return Rel::Same;
    }
    let kind = |v: &Value| v.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
    let int = |v: &Value, k: &str| v.get(k).and_then(Value::as_i64);
    let bound = |a: Option<i64>, b: Option<i64>, low: bool| -> Rel {
        match (a, b) {
            (x, y) if x == y => Rel::Same,
            (_, None) => Rel::Widens,
            (None, Some(_)) => Rel::Narrows,
            (Some(x), Some(y)) if (y <= x) == low => Rel::Widens,
            _ => Rel::Narrows,
        }
    };
    let fold = |rs: Vec<Rel>| {
        if rs.contains(&Rel::Different) {
            Rel::Different
        } else if rs.contains(&Rel::Narrows) {
            Rel::Narrows
        } else if rs.contains(&Rel::Widens) {
            Rel::Widens
        } else {
            Rel::Same
        }
    };
    match (kind(o).as_str(), kind(n).as_str()) {
        ("int", "int") => fold(vec![bound(int(o, "min"), int(n, "min"), true), bound(int(o, "max"), int(n, "max"), false)]),
        ("text", "text") => {
            let flag = |k: &str| match (o.get(k).and_then(Value::as_bool).unwrap_or(false), n.get(k).and_then(Value::as_bool).unwrap_or(false)) {
                (a, b) if a == b => Rel::Same,
                (_, false) => Rel::Widens,
                _ => Rel::Narrows,
            };
            let pattern = match (o.get("pattern").filter(|p| !p.is_null()), n.get("pattern").filter(|p| !p.is_null())) {
                (a, b) if a == b => Rel::Same,
                (_, None) => Rel::Widens,
                (None, Some(_)) => Rel::Narrows,
                _ => Rel::Different,
            };
            fold(vec![bound(int(o, "min"), int(n, "min"), true), bound(int(o, "max"), int(n, "max"), false), flag("trim"), flag("lower"), pattern])
        }
        ("text", "email" | "url" | "phone") => Rel::Narrows,
        ("email" | "url" | "phone", "text") => Rel::Widens,
        ("list", "list") | ("set", "set") => {
            let inner = relation(o.get("of"), n.get("of"));
            let max = |v: &Value| v.get("max").and_then(Value::as_u64).unwrap_or(0);
            let size = match (max(o), max(n)) {
                (0, _) | (_, 0) => Rel::Same,
                (a, b) if a == b => Rel::Same,
                (a, b) if b > a => Rel::Widens,
                _ => Rel::Narrows,
            };
            fold(vec![inner, size])
        }
        (a, b) if a == b => {
            // same kind, other target (another enum, record, entity, currency): not comparable
            if o.get("name") != n.get("name") || o.get("entity") != n.get("entity") || o.get("currency") != n.get("currency") {
                Rel::Different
            } else {
                Rel::Same
            }
        }
        _ => Rel::Different,
    }
}
