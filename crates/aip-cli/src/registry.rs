//! One surface over the three code registries (founder decision 2026-10-13, audit F9).
//!
//! Every code is `AREA.NAME`:
//! - `DEF`: a diagnostic of the product definition checker (`aip service check`), registry in `spike-v1-fixture`
//! - `WIRE`: an error the product HTTP API answers with, registry in `spike-v6-transport`
//! - `CLIENT`: an error the SDKs (TypeScript, Python) raise themselves, registry in `spike-v6-transport`
//! - `POC`: the PoC language: checker (`POC.E100`, `POC.W603`), Core IR validation (`POC.I110`) and the PoC runtime
//!   (`POC.INPUT.INVALID`), registry in `aip-ir`
//!
//! The data stays with the crate that emits the codes; this module lists, looks up and explains all of them, and
//! points a code of an old scheme (`AIP-E103`, `AIP.AUTH.FORBIDDEN`, a bare `UNKNOWN_FIELD`) to its new name.

use serde_json::{Value, json};

pub const AREAS: [&str; 4] = ["poc", "def", "wire", "client"];

/// One registry entry as JSON, tagged with its area. The fields besides `code`, `area`, `explain` and `fix` depend
/// on the area: a checker diagnostic has a title and an example, a wire code a class and a retry guidance.
pub fn entries(area: &str) -> Vec<Value> {
    match area {
        "poc" => aip_ir::codes::all()
            .iter()
            .map(|c| {
                let mut v = serde_json::to_value(c).unwrap_or_default();
                v["area"] = json!("poc");
                v
            })
            .collect(),
        "def" => aip_service::definition_registry::all().iter().map(definition_entry).collect(),
        "wire" => aip_service::wire_registry::all().iter().map(wire_entry).collect(),
        "client" => aip_service::client_registry::all().iter().map(client_entry).collect(),
        _ => Vec::new(),
    }
}

pub fn all_entries() -> Vec<Value> {
    AREAS.iter().flat_map(|a| entries(a)).collect()
}

fn definition_entry(c: &aip_service::definition_registry::Info) -> Value {
    json!({"code": c.code, "area": "def", "kind": "definition", "title": c.title, "explain": c.explain, "fix": c.fix, "example": c.example})
}

fn wire_entry(c: &aip_service::wire_registry::Info) -> Value {
    json!({"code": c.code, "area": "wire", "kind": "wire", "class": c.class.name(), "retry": c.retry.name(), "explain": c.explain, "fix": c.fix})
}

fn client_entry(c: &aip_service::client_registry::Info) -> Value {
    json!({"code": c.code, "area": "client", "kind": "client", "retry": c.retry.name(), "explain": c.explain, "fix": c.fix})
}

/// One line for a listing: the title, or the first sentence of the explanation when the registry has no title.
pub fn summary(entry: &Value) -> String {
    if let Some(t) = entry["title"].as_str() {
        return t.to_string();
    }
    let e = entry["explain"].as_str().unwrap_or_default();
    e.split_inclusive(". ").next().unwrap_or(e).trim().to_string()
}

pub fn render_markdown(area: &str) -> Option<String> {
    match area {
        "poc" => Some(aip_ir::codes::render_markdown()),
        "def" => Some(aip_service::definition_registry::render_markdown()),
        "wire" => Some(aip_service::wire_registry::render_markdown()),
        "client" => Some(aip_service::client_registry::render_markdown()),
        _ => None,
    }
}

/// The text of `aip explain-code` for a registered code.
fn render_text(code: &str) -> Option<String> {
    if let Some(c) = aip_ir::codes::lookup(code) {
        return Some(aip_ir::codes::render_text(c));
    }
    if let Some(c) = aip_service::definition_registry::lookup(code) {
        return Some(aip_service::definition_registry::render_text(c));
    }
    if let Some(c) = aip_service::wire_registry::lookup(code) {
        return Some(aip_service::wire_registry::render_text(c));
    }
    aip_service::client_registry::lookup(code).map(aip_service::client_registry::render_text)
}

fn entry_json(code: &str) -> Option<Value> {
    all_entries().into_iter().find(|e| e["code"] == code)
}

/// The codes an old-scheme code became. Empty for a code that is not an old one.
///
/// Old schemes: PoC checker `AIP-E100`/`AIP-W603`/`AIP-I110` -> `POC.E100`; PoC runtime `AIP.INPUT.INVALID` ->
/// `POC.INPUT.INVALID`; the bare names of the product (`UNKNOWN_FIELD`, `PROTOCOL_ERROR`) -> `DEF.`, `WIRE.` and/or `CLIENT.`
/// plus the name, whichever of them exist.
pub fn renamed_from(code: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(rest) = code.strip_prefix("AIP-") {
        candidates.push(format!("POC.{rest}"));
    } else if let Some(rest) = code.strip_prefix("AIP.") {
        candidates.push(format!("POC.{rest}"));
    } else if !code.contains('.') {
        candidates.push(format!("DEF.{code}"));
        candidates.push(format!("WIRE.{code}"));
        candidates.push(format!("CLIENT.{code}"));
    }
    candidates.retain(|c| render_text(c).is_some());
    candidates
}

/// What `aip explain-code` prints: the entry of a registered code, or, for a code of an old scheme, a note with the new
/// name(s) and their entries. `None` when the code is unknown.
pub fn explain(code: &str, json: bool) -> Option<String> {
    let code = code.to_uppercase();
    let (note, found): (Option<String>, Vec<String>) = if render_text(&code).is_some() {
        (None, vec![code.clone()])
    } else {
        let new = renamed_from(&code);
        if new.is_empty() {
            return None;
        }
        let note = if new.len() == 1 {
            format!("note: `{code}` is the old name of {}; the code was renamed.\n\n", new[0])
        } else {
            format!("note: `{code}` was renamed; the name now belongs to {} areas: {}.\n\n", new.len(), new.join(", "))
        };
        (Some(note), new)
    };
    if json {
        let values: Vec<Value> = found.iter().filter_map(|c| entry_json(c)).collect();
        let mut out = if values.len() == 1 { values[0].clone() } else { Value::Array(values) };
        if let (Some(_), Some(o)) = (&note, out.as_object_mut()) {
            o.insert("renamedFrom".into(), json!(code));
        }
        return serde_json::to_string_pretty(&out).ok().map(|s| s + "\n");
    }
    let mut text = note.unwrap_or_default();
    text.push_str(&found.iter().filter_map(|c| render_text(c)).collect::<Vec<_>>().join("\n"));
    Some(text)
}
