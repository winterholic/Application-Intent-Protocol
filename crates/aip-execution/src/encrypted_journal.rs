//! Shared write-request journal boundary. It fingerprints validated JSON scalars, never caller
//! markers, and verifies with the key recorded in each stored marker. Policy and scalar validation
//! remain the writer's responsibility. This module never grants permission to execute a request.
use aip_crypto::{
    cell,
    fingerprint::{Context as FingerprintContext, Keys},
};
use serde_json::{Value, json};

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    Corrupted,
    Unavailable,
}
impl From<cell::Error> for Error {
    fn from(error: cell::Error) -> Self {
        match error {
            cell::Error::InvalidInput => Self::InvalidInput,
            cell::Error::Corrupted => Self::Corrupted,
            cell::Error::Unavailable => Self::Unavailable,
        }
    }
}

pub struct Context<'a> {
    pub schema: &'a str,
    pub principal: &'a str,
    pub request_key: &'a str,
}

/// An effect journal stores only this keyed marker, not the input. Bind the complete effect
/// (including implementation) so fields outside a resource write cannot escape the fingerprint.
pub struct Effect<'a> {
    pub kind: &'a str,
    pub name: &'a str,
    pub input: &'a Value,
    pub implementation: &'a str,
}
impl Effect<'_> {
    fn body(&self) -> Result<zeroize::Zeroizing<Vec<u8>>, Error> {
        let bytes =
            zeroize::Zeroizing::new(serde_json::to_vec(&(self.kind, self.name, self.input, self.implementation)).map_err(|_| Error::InvalidInput)?);
        if bytes.len() > cell::MAX_CELL_BYTES {
            return Err(Error::InvalidInput);
        }
        Ok(bytes)
    }
}
pub fn effect_marker(effect: &Effect<'_>, keys: &Keys, context: &Context<'_>) -> Result<String, Error> {
    let identity = FingerprintContext {
        schema: context.schema,
        principal: context.principal,
        request_key: context.request_key,
        resource: effect.name,
        field: "$effect",
    };
    keys.idem(&identity, &effect.body()?).map_err(Into::into)
}
pub fn verify_effect(effect: &Effect<'_>, stored: &str, keys: &Keys, context: &Context<'_>) -> Result<bool, Error> {
    let identity = FingerprintContext {
        schema: context.schema,
        principal: context.principal,
        request_key: context.request_key,
        resource: effect.name,
        field: "$effect",
    };
    keys.verify_idem(&identity, &effect.body()?, stored).map_err(Into::into)
}
struct Leaf<'a> {
    path: String,
    resource: &'a str,
    field: &'a str,
}
impl Context<'_> {
    fn fingerprint<'a>(&'a self, leaf: &'a Leaf<'_>) -> FingerprintContext<'a> {
        FingerprintContext {
            schema: self.schema,
            principal: self.principal,
            request_key: self.request_key,
            resource: leaf.resource,
            field: leaf.field,
        }
    }
}

/// A failure aborts journaling: returning an unprotected request is never a fallback.
pub fn protect(facts: &Value, request: &Value, keys: &Keys, context: &Context<'_>) -> Result<Value, Error> {
    let leaves = paths(facts, request)?;
    let mut protected = request.clone();
    for leaf in leaves {
        let value = protected.pointer_mut(&leaf.path).ok_or(Error::InvalidInput)?;
        // JSON encoding preserves scalar type ("true" and true must not have the same marker).
        let bytes = zeroize::Zeroizing::new(serde_json::to_vec(value).map_err(|_| Error::InvalidInput)?);
        let marker = keys.idem(&context.fingerprint(&leaf), &bytes)?;
        *value = json!({"$aip_enc_fp":marker});
    }
    Ok(protected)
}

/// Stored plaintext/forged marker objects fail closed. A missing retained fingerprint key is an
/// availability error, distinct from a real mismatch. Non-sensitive request structure is compared
/// exactly after every encrypted position has been independently authenticated.
pub fn verify(facts: &Value, stored: &Value, incoming: &Value, keys: &Keys, context: &Context<'_>) -> Result<bool, Error> {
    let leaves = paths(facts, incoming)?;
    let mut candidate = incoming.clone();
    for leaf in leaves {
        let Some(marker) = stored.pointer(&leaf.path) else { return Ok(false) };
        let marker_object = marker.as_object().filter(|m| m.len() == 1).ok_or(Error::Corrupted)?;
        let marker_text = marker_object.get("$aip_enc_fp").and_then(Value::as_str).ok_or(Error::Corrupted)?;
        let value = candidate.pointer_mut(&leaf.path).ok_or(Error::InvalidInput)?;
        let bytes = zeroize::Zeroizing::new(serde_json::to_vec(value).map_err(|_| Error::InvalidInput)?);
        if !keys.verify_idem(&context.fingerprint(&leaf), &bytes, marker_text)? {
            return Ok(false);
        }
        *value = marker.clone();
    }
    Ok(candidate == *stored)
}

fn paths<'a>(facts: &Value, request: &'a Value) -> Result<Vec<Leaf<'a>>, Error> {
    if serde_json::to_vec(request).map_err(|_| Error::InvalidInput)?.len() > cell::MAX_CELL_BYTES {
        return Err(Error::InvalidInput);
    }
    let mut output = Vec::new();
    let mut budget = 8192;
    visit(facts, request, "", None, 0, &mut budget, &mut output)?;
    Ok(output)
}
fn path(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn encrypted(facts: &Value, resource: &str, field: &str) -> bool {
    facts["resources"][resource]["fields"][field].get("encrypted").is_some()
}
fn add_scalar<'a>(value: &Value, resource: &'a str, field: &'a str, path: String, output: &mut Vec<Leaf<'a>>) -> Result<(), Error> {
    if value.is_null() {
        return Ok(());
    }
    if value.is_array() || value.is_object() {
        return Err(Error::InvalidInput);
    }
    output.push(Leaf { path, resource, field });
    Ok(())
}

fn fields<'a>(
    facts: &Value,
    request: &'a Value,
    resource: &'a str,
    parent: &str,
    compose_create: bool,
    output: &mut Vec<Leaf<'a>>,
) -> Result<(), Error> {
    for key in ["values", "expect"] {
        if let Some(values) = request.get(key) {
            for (field, value) in values.as_object().ok_or(Error::InvalidInput)? {
                if encrypted(facts, resource, field) {
                    let value_path = path(&path(parent, key), field);
                    if compose_create && key == "values" {
                        // Compose's server-validated sources are {const: scalar} or {item: path}.
                        // Fingerprint the literal, preserving the wrapper that controls its meaning.
                        let source = value.as_object().filter(|s| s.len() == 1).ok_or(Error::InvalidInput)?;
                        if let Some(constant) = source.get("const") {
                            add_scalar(constant, resource, field, path(&value_path, "const"), output)?;
                        } else if !source.get("item").is_some_and(Value::is_string) {
                            return Err(Error::InvalidInput);
                        }
                    } else {
                        add_scalar(value, resource, field, value_path, output)?;
                    }
                }
            }
        }
    }
    for key in ["target", "targets"] {
        if let Some(target) = request.get(key) {
            let target = target.as_object().ok_or(Error::InvalidInput)?;
            if let Some(filters) = target.get("where") {
                for (index, filter) in filters.as_array().ok_or(Error::InvalidInput)?.iter().enumerate() {
                    let field = filter.get("field").and_then(Value::as_str).ok_or(Error::InvalidInput)?;
                    let value = filter.get("value").ok_or(Error::InvalidInput)?;
                    if encrypted(facts, resource, field) {
                        add_scalar(value, resource, field, path(&path(&path(&path(parent, key), "where"), &index.to_string()), "value"), output)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn visit<'a>(
    facts: &Value,
    request: &'a Value,
    parent: &str,
    inherited: Option<&'a str>,
    depth: usize,
    budget: &mut usize,
    output: &mut Vec<Leaf<'a>>,
) -> Result<(), Error> {
    if depth > 32 || *budget == 0 {
        return Err(Error::InvalidInput);
    }
    *budget -= 1;
    let object = request.as_object().ok_or(Error::InvalidInput)?;
    let actions = ["create", "apply", "compose", "atomic", "transition"].iter().filter(|key| object.contains_key(**key)).count();
    if actions == 0 && object.get("extension").is_some_and(Value::is_string) {
        // Operation input has its own declared contract; encrypted parameters are statically
        // unsupported. Do not infer that arbitrary ordinary Text parameters are sensitive/safe.
        return Ok(());
    }
    if actions != 1 {
        return Err(Error::InvalidInput);
    }
    if let Some(atomic) = object.get("atomic") {
        for (index, item) in atomic.as_array().ok_or(Error::InvalidInput)?.iter().enumerate() {
            visit(facts, item, &path(&path(parent, "atomic"), &index.to_string()), None, depth + 1, budget, output)?;
        }
        return Ok(());
    }
    let resource = if let Some(name) = object.get("create").or_else(|| object.get("compose")) {
        name.as_str().ok_or(Error::InvalidInput)?
    } else if let Some(name) = object.get("apply") {
        name.as_str().and_then(|s| s.split_once('.')).map(|(resource, _)| resource).ok_or(Error::InvalidInput)?
    } else {
        if !object.get("transition").is_some_and(Value::is_string) {
            return Err(Error::InvalidInput);
        }
        inherited.ok_or(Error::InvalidInput)?
    };
    if !facts["resources"][resource]["fields"].is_object() {
        return Err(Error::InvalidInput);
    }
    fields(facts, request, resource, parent, object.contains_key("create") && inherited.is_some(), output)?;
    if object.contains_key("compose") {
        for (index, step) in object.get("steps").and_then(Value::as_array).ok_or(Error::InvalidInput)?.iter().enumerate() {
            visit(facts, step, &path(&path(parent, "steps"), &index.to_string()), Some(resource), depth + 1, budget, output)?;
        }
    }
    Ok(())
}
