//! Input validation from the compiled parameter specs. Strict: unknown keys,
//! missing required keys and out-of-range values are rejected with a path.

use crate::error::AipError;
use aip_ir::codes;
use aip_plan::{ParamSpec, Program, TypeSpec};
use serde_json::{Map, Value};

pub struct Ctx<'a> {
    pub program: &'a Program,
    pub intent: &'a str,
}

fn bad(ctx: &Ctx<'_>, path: &str, msg: impl Into<String>) -> AipError {
    AipError::new(codes::INPUT_INVALID, ctx.intent, msg).path(path)
}

/// Validates `input` against `params`, returning the normalised object.
pub fn params(ctx: &Ctx<'_>, specs: &[ParamSpec], input: &Value, prefix: &str) -> Result<Map<String, Value>, AipError> {
    let empty = Map::new();
    let obj = match input {
        Value::Object(m) => m,
        Value::Null => &empty,
        _ => return Err(bad(ctx, prefix, "input must be a JSON object")),
    };
    for k in obj.keys() {
        if !specs.iter().any(|p| &p.name == k) {
            return Err(bad(ctx, &join(prefix, k), format!("unknown field '{k}'")));
        }
    }
    let mut out = Map::new();
    for p in specs {
        let path = join(prefix, &p.name);
        let v = obj.get(&p.name).cloned().unwrap_or(Value::Null);
        let v = if v.is_null() { p.default.clone().unwrap_or(Value::Null) } else { v };
        if v.is_null() {
            if p.optional || matches!(p.ty, TypeSpec::Upload { .. }) && p.optional {
                out.insert(p.name.clone(), Value::Null);
                continue;
            }
            if matches!(p.ty, TypeSpec::List { .. }) {
                out.insert(p.name.clone(), Value::Array(Vec::new()));
                continue;
            }
            return Err(bad(ctx, &path, format!("'{}' is required", p.name)));
        }
        out.insert(p.name.clone(), value(ctx, &p.ty, v, &path)?);
    }
    Ok(out)
}

fn join(prefix: &str, k: &str) -> String {
    if prefix.is_empty() { k.to_string() } else { format!("{prefix}.{k}") }
}

fn is_uuid(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok()
}

/// Largest integer a JSON number can carry without losing digits in a double (2^53 - 1).
const MAX_SAFE_INT: i64 = 9_007_199_254_740_991;

/// Exact decimal text: `^-?(0|[1-9][0-9]*)(\.[0-9]+)?$`. A JSON number is accepted only as a
/// safe integer, because any other number has already gone through a binary float.
fn decimal_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => {
            let unsigned = s.strip_prefix('-').unwrap_or(s);
            let (int, frac) = match unsigned.split_once('.') {
                Some((i, f)) => (i, Some(f)),
                None => (unsigned, None),
            };
            let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
            let int_ok = digits(int) && (int == "0" || !int.starts_with('0'));
            (int_ok && frac.is_none_or(digits)).then(|| s.clone())
        }
        Value::Number(n) => n.as_i64().filter(|i| i.abs() <= MAX_SAFE_INT).map(|i| i.to_string()),
        _ => None,
    }
}

pub fn value(ctx: &Ctx<'_>, ty: &TypeSpec, v: Value, path: &str) -> Result<Value, AipError> {
    match ty {
        TypeSpec::Bool => v.as_bool().map(Value::Bool).ok_or_else(|| bad(ctx, path, "must be a boolean")),
        TypeSpec::Int { min, max } => {
            let n = v.as_i64().ok_or_else(|| bad(ctx, path, "must be an integer"))?;
            if min.is_some_and(|m| n < m) || max.is_some_and(|m| n > m) {
                return Err(bad(
                    ctx,
                    path,
                    format!("must be in {}..{}", min.map(|x| x.to_string()).unwrap_or_default(), max.map(|x| x.to_string()).unwrap_or_default()),
                ));
            }
            Ok(Value::from(n))
        }
        TypeSpec::Decimal | TypeSpec::Money { .. } => {
            decimal_text(&v).map(Value::String).ok_or_else(|| bad(ctx, path, "must be a decimal string like \"12.50\""))
        }
        TypeSpec::Text { min, max, trim, lower, pattern } => {
            let mut s = v.as_str().ok_or_else(|| bad(ctx, path, "must be a string"))?.to_string();
            if *trim {
                s = s.trim().to_string();
            }
            if *lower {
                s = s.to_lowercase();
            }
            let len = s.chars().count() as i64;
            if min.is_some_and(|m| len < m) || max.is_some_and(|m| len > m) {
                return Err(bad(
                    ctx,
                    path,
                    format!(
                        "length must be in {}..{}",
                        min.map(|x| x.to_string()).unwrap_or_default(),
                        max.map(|x| x.to_string()).unwrap_or_default()
                    ),
                ));
            }
            if pattern.is_some() {
                // regex refinements are validated by the database CHECK when stored
            }
            Ok(Value::String(s))
        }
        TypeSpec::RichText => {
            let s = v.as_str().ok_or_else(|| bad(ctx, path, "must be a string"))?;
            Ok(Value::String(sanitize_basic(s)))
        }
        TypeSpec::Email => {
            let s = v.as_str().ok_or_else(|| bad(ctx, path, "must be an email"))?.trim().to_string();
            let ok = s.split_once('@').is_some_and(|(a, b)| !a.is_empty() && b.contains('.') && !b.starts_with('.') && !s.contains(' '));
            if ok { Ok(Value::String(s)) } else { Err(bad(ctx, path, "must be an email")) }
        }
        TypeSpec::Url => {
            let s = v.as_str().ok_or_else(|| bad(ctx, path, "must be a URL"))?;
            if s.starts_with("https://") || s.starts_with("http://") { Ok(v) } else { Err(bad(ctx, path, "must be an http(s) URL")) }
        }
        TypeSpec::Phone => {
            let s = v.as_str().ok_or_else(|| bad(ctx, path, "must be a phone number"))?;
            let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
            if (9..=15).contains(&digits.len()) { Ok(Value::String(digits)) } else { Err(bad(ctx, path, "must be a phone number")) }
        }
        TypeSpec::Time => {
            let s = v.as_str().ok_or_else(|| bad(ctx, path, "must be an RFC 3339 timestamp"))?;
            chrono::DateTime::parse_from_rfc3339(s).map(|_| v.clone()).map_err(|_| bad(ctx, path, "must be an RFC 3339 timestamp"))
        }
        TypeSpec::Date => {
            let s = v.as_str().ok_or_else(|| bad(ctx, path, "must be a date (YYYY-MM-DD)"))?;
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map(|_| v.clone()).map_err(|_| bad(ctx, path, "must be a date (YYYY-MM-DD)"))
        }
        TypeSpec::Duration => v.as_str().map(|_| v.clone()).ok_or_else(|| bad(ctx, path, "must be an interval string")),
        TypeSpec::Uuid | TypeSpec::Entity { .. } => {
            let s = v.as_str().ok_or_else(|| bad(ctx, path, "must be an id"))?;
            if is_uuid(s) { Ok(Value::String(s.to_lowercase())) } else { Err(bad(ctx, path, "must be an id (uuid)")) }
        }
        TypeSpec::Enum { name } => {
            let s = v.as_str().ok_or_else(|| bad(ctx, path, "must be a string"))?;
            let values = ctx.program.enums.get(name).cloned().unwrap_or_default();
            if values.iter().any(|x| x == s) { Ok(v) } else { Err(bad(ctx, path, format!("must be one of {}", values.join(", ")))) }
        }
        TypeSpec::Record { name } => {
            let specs = ctx.program.records.get(name).cloned().unwrap_or_default();
            Ok(Value::Object(params(ctx, &specs, &v, path)?))
        }
        TypeSpec::Union { entities } => {
            let obj = v.as_object().ok_or_else(|| bad(ctx, path, "must be {type, id}"))?;
            let t = obj.get("type").and_then(|x| x.as_str()).unwrap_or_default();
            let id = obj.get("id").and_then(|x| x.as_str()).unwrap_or_default();
            if !entities.iter().any(|e| e == t) {
                return Err(bad(ctx, path, format!("type must be one of {}", entities.join(", "))));
            }
            if !is_uuid(id) {
                return Err(bad(ctx, path, "id must be a uuid"));
            }
            Ok(v)
        }
        TypeSpec::Set { of, max } | TypeSpec::List { of, max } => {
            let arr = v.as_array().ok_or_else(|| bad(ctx, path, "must be an array"))?;
            if *max > 0 && arr.len() as u64 > *max {
                return Err(bad(ctx, path, format!("at most {max} items")));
            }
            let is_set = matches!(ty, TypeSpec::Set { .. });
            if is_set && arr.is_empty() {
                return Err(bad(ctx, path, "must not be empty"));
            }
            let mut out = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                let x = value(ctx, of, item.clone(), &format!("{path}[{i}]"))?;
                if is_set && out.contains(&x) {
                    return Err(bad(ctx, &format!("{path}[{i}]"), "duplicate item"));
                }
                out.push(x);
            }
            Ok(Value::Array(out))
        }
        TypeSpec::Range { of } => {
            let obj = v.as_object().ok_or_else(|| bad(ctx, path, "must be {start, end}"))?;
            let s = value(ctx, of, obj.get("start").cloned().unwrap_or(Value::Null), &format!("{path}.start"))?;
            let e = value(ctx, of, obj.get("end").cloned().unwrap_or(Value::Null), &format!("{path}.end"))?;
            let (Some(ss), Some(es)) = (s.as_str(), e.as_str()) else { return Ok(serde_json::json!({"start": s, "end": e})) };
            if ss >= es {
                return Err(bad(ctx, path, "start must be before end"));
            }
            Ok(serde_json::json!({"start": s, "end": e}))
        }
        TypeSpec::Json | TypeSpec::Snapshot { .. } | TypeSpec::Other { .. } => Ok(v),
        TypeSpec::Upload { .. } | TypeSpec::Object => Ok(v),
    }
}

/// Minimal HTML sanitisation for `RichText(policy: basic)`: strips tags other
/// than a small allow-list and every attribute.
pub fn sanitize_basic(s: &str) -> String {
    const ALLOWED: &[&str] = &["p", "br", "b", "strong", "i", "em", "u", "ul", "ol", "li", "h1", "h2", "h3", "blockquote", "code", "pre"];
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            out.push_str(&rest[start..].replace('<', "&lt;"));
            return out;
        };
        let tag = &rest[start + 1..start + end];
        let closing = tag.starts_with('/');
        let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
        if ALLOWED.contains(&name.as_str()) {
            out.push('<');
            if closing {
                out.push('/');
            }
            out.push_str(&name);
            out.push('>');
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn program() -> Program {
        serde_json::from_value(json!({
            "ir_version": "0.1", "enums": {}, "records": {}, "entities": {}, "ddl": [], "intents": {},
            "handlers": [], "schedules": [], "events": {}, "extensions": [], "constraint_errors": {}
        }))
        .expect("minimal program")
    }

    fn check(ty: &TypeSpec, v: Value) -> Result<Value, AipError> {
        let p = program();
        value(&Ctx { program: &p, intent: "T" }, ty, v, "amount")
    }

    fn money() -> TypeSpec {
        TypeSpec::Money { currency: "KRW".into() }
    }

    #[test]
    fn strips_scripts_and_attributes() {
        assert_eq!(sanitize_basic("<p onclick=\"x\">hi<script>alert(1)</script></p>"), "<p>hialert(1)</p>");
        assert_eq!(sanitize_basic("a < b"), "a &lt; b");
    }

    #[test]
    fn decimal_and_money_accept_only_decimal_strings_and_safe_integers() {
        for ty in [TypeSpec::Decimal, money()] {
            let ok = [
                (json!("12.50"), json!("12.50")),
                (json!("0"), json!("0")),
                (json!("-0.5"), json!("-0.5")),
                (json!("12345678901234567.89"), json!("12345678901234567.89")),
                (json!(1200), json!("1200")),
                (json!(-7), json!("-7")),
                (json!(9007199254740991_i64), json!("9007199254740991")),
                (json!(-9007199254740991_i64), json!("-9007199254740991")),
            ];
            for (input, want) in ok {
                assert_eq!(check(&ty, input.clone()).expect("accepted"), want, "{input}");
            }
        }
    }

    #[test]
    fn decimal_and_money_reject_everything_else() {
        let rejected = [
            json!("NaN"),
            json!("inf"),
            json!("Infinity"),
            json!("-Infinity"),
            json!("1e400"),
            json!("1E5"),
            json!("1.5e3"),
            json!("012"),
            json!("00.5"),
            json!(" 12"),
            json!("12 "),
            json!("+12"),
            json!("12."),
            json!(".5"),
            json!("1,000"),
            json!("-"),
            json!(""),
            json!("0x10"),
            json!(1.5),
            json!(f64::MAX),
            json!(9007199254740992_i64),
            json!(-9007199254740992_i64),
            json!(u64::MAX),
            json!(true),
            json!([]),
        ];
        for ty in [TypeSpec::Decimal, money()] {
            for input in &rejected {
                let e = check(&ty, input.clone()).expect_err(&format!("{input} must be rejected"));
                assert!(e.to_string().contains("must be a decimal string like \"12.50\""), "{input}: {e}");
            }
        }
    }
}
