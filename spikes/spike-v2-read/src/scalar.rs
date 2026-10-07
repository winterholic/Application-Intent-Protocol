use crate::plan::Reject;
use chrono::{DateTime, Datelike, Timelike, Utc};
use serde_json::Value;

pub fn supported(ty: &str) -> bool {
    let ty = ty.trim_end_matches('?');
    matches!(ty, "Bool" | "Int" | "Text" | "Email" | "Url" | "Time" | "Date")
        || decimal_type(ty).is_some()
        || ty.strip_prefix("Enum<").is_some_and(|s| s.ends_with('>'))
}

fn decimal_type(ty: &str) -> Option<(u8, u8)> {
    let inner = ty.strip_prefix("Decimal<")?.strip_suffix('>')?;
    let (precision, scale) = inner.split_once(',')?;
    let canonical_uint = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'));
    if !canonical_uint(precision) || !canonical_uint(scale) {
        return None;
    }
    let (precision, scale) = (precision.parse::<u8>().ok()?, scale.parse::<u8>().ok()?);
    ((1..=38).contains(&precision) && scale <= precision).then_some((precision, scale))
}

fn decimal_binding(s: &str, precision: u8, scale: u8) -> bool {
    if s.len() > 41 || !s.is_ascii() {
        return false;
    }
    let unsigned = s.strip_prefix('-').unwrap_or(s);
    if unsigned.is_empty() || unsigned.starts_with('+') {
        return false;
    }
    let mut parts = unsigned.split('.');
    let integer = parts.next().unwrap_or_default();
    let fraction = parts.next();
    if parts.next().is_some() || integer.is_empty() || !integer.bytes().all(|b| b.is_ascii_digit()) || !(integer == "0" || !integer.starts_with('0'))
    {
        return false;
    }
    if let Some(fraction) = fraction {
        if scale == 0 || fraction.is_empty() || fraction.len() > usize::from(scale) || !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    let integer_digits = if integer == "0" { 0 } else { integer.len() };
    integer_digits <= usize::from(precision - scale)
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
    if let Some((precision, scale)) = decimal_type(base) {
        return match value {
            Value::String(s) if decimal_binding(s, precision, scale) => Ok((s.clone(), "numeric")),
            _ => Err(bad(base)),
        };
    }
    match (base, value) {
        ("Bool", Value::Bool(b)) => Ok((b.to_string(), "boolean")),
        ("Int", Value::Number(n)) if n.is_i64() => Ok((n.to_string(), "bigint")),
        ("Text" | "Url", Value::String(s)) if !s.contains('\0') => Ok((s.clone(), "text")),
        ("Email", Value::String(s)) if valid_email(s) => Ok((s.clone(), "text")),
        ("Time", Value::String(s)) => time_binding(s).map(|s| (s, "timestamptz")),
        ("Date", Value::String(s)) if valid_date(s) => Ok((s.clone(), "date")),
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

fn valid_email(s: &str) -> bool {
    if s.contains('\0') || s.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty() && !domain.is_empty() && !domain.starts_with('.') && !domain.ends_with('.') && domain.contains('.') && !domain.contains('@')
}

fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10
        || !b.is_ascii()
        || b[4] != b'-'
        || b[7] != b'-'
        || ![&b[..4], &b[5..7], &b[8..]].iter().all(|part| part.iter().all(u8::is_ascii_digit))
    {
        return false;
    }
    let number = |part: &[u8]| std::str::from_utf8(part).ok().and_then(|s| s.parse::<u32>().ok());
    let (Some(year), Some(month), Some(day)) = (number(&b[..4]), number(&b[5..7]), number(&b[8..])) else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=days).contains(&day)
}
