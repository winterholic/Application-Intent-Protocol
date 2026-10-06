use crate::plan::Reject;
use chrono::{DateTime, Datelike, Timelike, Utc};
use serde_json::Value;

pub fn supported(ty: &str) -> bool {
    let ty = ty.trim_end_matches('?');
    matches!(ty, "Bool" | "Int" | "Text" | "Url" | "Time") || ty.strip_prefix("Enum<").is_some_and(|s| s.ends_with('>'))
}

fn bad(ty: &str) -> Reject {
    Reject { code: "BAD_VALUE", msg: format!("`{ty}` 타입에 맞지 않는 값") }
}

fn time_binding(s: &str) -> Result<String, Reject> {
    let fail = || bad("Time");
    let b = s.as_bytes();
    if b.len() < 20
        || b.len() > 35
        || !b.is_ascii()
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return Err(fail());
    }
    if ![&b[0..4], &b[5..7], &b[8..10], &b[11..13], &b[14..16], &b[17..19]].iter().all(|part| part.iter().all(u8::is_ascii_digit))
        || &b[0..4] == b"0000"
    {
        return Err(fail());
    }
    let end = if matches!(b[b.len() - 1], b'Z' | b'z') {
        b.len() - 1
    } else {
        if b.len() < 25 {
            return Err(fail());
        }
        let tail = &b[b.len() - 6..];
        if !matches!(tail[0], b'+' | b'-') || tail[3] != b':' || ![&tail[1..3], &tail[4..6]].iter().all(|part| part.iter().all(u8::is_ascii_digit)) {
            return Err(fail());
        }
        b.len() - 6
    };
    let fraction = &s[19..end];
    if !fraction.is_empty() && (fraction.len() < 2 || !fraction.starts_with('.') || !fraction.as_bytes()[1..].iter().all(u8::is_ascii_digit)) {
        return Err(fail());
    }
    let parsed = DateTime::parse_from_rfc3339(s).map_err(|_| fail())?;
    let utc = parsed.with_timezone(&Utc);
    // Chrono truncates long fractions; PG rounds them. Preserve the caller's fraction for the DB cast.
    // PG uses 1 BC for astronomical year 0 and accepts year 10000 without Chrono's leading '+'.
    let year = utc.year();
    let (year, era) = if year <= 0 { (1 - year, " BC") } else { (year, "") };
    let second = utc.second() + u32::from(utc.nanosecond() >= 1_000_000_000);
    Ok(format!("{year:04}-{:02}-{:02}T{:02}:{:02}:{second:02}{fraction}+00:00{era}", utc.month(), utc.day(), utc.hour(), utc.minute()))
}

pub fn parse(facts: &Value, ty: &str, value: &Value) -> Result<(String, &'static str), Reject> {
    let base = ty.trim_end_matches('?');
    match (base, value) {
        ("Bool", Value::Bool(b)) => Ok((b.to_string(), "boolean")),
        ("Int", Value::Number(n)) if n.is_i64() => Ok((n.to_string(), "bigint")),
        ("Text" | "Url", Value::String(s)) if !s.contains('\0') => Ok((s.clone(), "text")),
        ("Time", Value::String(s)) => time_binding(s).map(|s| (s, "timestamptz")),
        (ty, Value::String(s)) if ty.starts_with("Enum<") => {
            let name = ty.strip_prefix("Enum<").and_then(|s| s.strip_suffix('>')).ok_or_else(|| bad(base))?;
            if facts["enums"][name].as_array().is_some_and(|members| members.iter().any(|member| member.as_str() == Some(s))) {
                Ok((s.clone(), "text"))
            } else {
                Err(bad(base))
            }
        }
        _ => Err(bad(base)),
    }
}
