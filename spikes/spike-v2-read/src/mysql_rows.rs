//! MySQL가 돌려준 JSON을 PostgreSQL 경로와 같은 공개 모양으로 맞춘다.
//!
//! MySQL의 `JSON_OBJECT`는 `TINYINT(1)`을 0/1 숫자로, DATETIME을 `YYYY-MM-DD HH:MM:SS.ffffff` 문자열로 내보낸다.
//! SQL 쪽에서 모든 열을 감싸 고치면 정책 식과 섞여 읽기 어려우므로, 계획이 알고 있는 출력 타입(`Plan::output_type`)으로
//! 결과만 한 번 정규화한다. 타입이 알려주지 않는 값은 건드리지 않고, 타입과 맞지 않는 값은 조용히 넘기지 않고 거부한다.
use crate::plan::Reject;
use serde_json::{Map, Value};

fn malformed(what: &str) -> Reject {
    Reject { code: "WIRE.INTERNAL", msg: format!("MySQL 결과가 계획 타입과 다름: {what}") }
}

/// 계획 출력 타입으로 행 하나(또는 단독 집계 값)를 정규화한다.
pub fn normalize(row: Value, output_type: &Value) -> Result<Value, Reject> {
    if let Some(fields) = output_type["rows"].as_object() {
        normalize_object(row, fields)
    } else if !output_type["value"].is_null() {
        normalize_value(row, &output_type["value"])
    } else {
        Err(malformed("출력 타입 없음"))
    }
}

fn normalize_object(value: Value, fields: &Map<String, Value>) -> Result<Value, Reject> {
    let Value::Object(mut object) = value else { return Err(malformed("객체 아님")) };
    for (name, ty) in fields {
        let slot = object.get_mut(name).ok_or_else(|| malformed("필드 누락"))?;
        *slot = normalize_value(slot.take(), ty)?;
    }
    Ok(Value::Object(object))
}

fn normalize_value(value: Value, ty: &Value) -> Result<Value, Reject> {
    if value.is_null() {
        return if ty["nullable"] == true { Ok(Value::Null) } else { Err(malformed("null 불가 필드가 null")) };
    }
    if let Some(fields) = ty["object"].as_object() {
        return normalize_object(value, fields);
    }
    if let Some(fields) = ty["array"].as_object() {
        let Value::Array(items) = value else { return Err(malformed("배열 아님")) };
        return items.into_iter().map(|item| normalize_object(item, fields)).collect::<Result<Vec<_>, _>>().map(Value::Array);
    }
    let base = ty["ty"].as_str().unwrap_or("");
    match base {
        "Bool" => match value {
            Value::Bool(_) => Ok(value),
            Value::Number(n) => match n.as_i64() {
                Some(0) => Ok(Value::Bool(false)),
                Some(1) => Ok(Value::Bool(true)),
                _ => Err(malformed("Bool 열의 값이 0/1이 아님")),
            },
            _ => Err(malformed("Bool")),
        },
        "Time" => match value {
            Value::String(text) => time_json(&text).map(Value::String),
            _ => Err(malformed("Time")),
        },
        "Int" => int_json(value),
        _ if base.starts_with("Id<") || base.starts_with("Ref<") => match value {
            Value::String(_) => Ok(value),
            other => int_json(other),
        },
        _ => Ok(value),
    }
}

/// 정수 열과 DECIMAL 합계. 소수부가 없고 i64 범위 안인 값만 정수로 인정한다(PostgreSQL의 `::bigint`가 범위 밖에서 오류를 내는 것과 같은 방향).
fn int_json(value: Value) -> Result<Value, Reject> {
    let text = match &value {
        Value::Number(n) if n.as_i64().is_some() => return Ok(value),
        Value::Number(n) => n.to_string(),
        Value::String(text) => text.clone(),
        _ => return Err(malformed("정수")),
    };
    let integral = text.strip_suffix(".0").or_else(|| text.strip_suffix(".00")).unwrap_or(&text);
    integral
        .parse::<i64>()
        .map(Value::from)
        .map_err(|_| Reject { code: "WIRE.INTERNAL", msg: "정수 결과가 i64 범위를 넘거나 정수가 아님".into() })
}

/// `YYYY-MM-DD HH:MM:SS[.ffffff]`(UTC)를 PostgreSQL의 JSON 시각 모양 `YYYY-MM-DDTHH:MM:SS[.f]+00:00`으로 바꾼다.
/// PostgreSQL은 소수부의 끝 0을 지우고 0이면 소수부를 빼므로 그대로 따른다.
fn time_json(text: &str) -> Result<String, Reject> {
    let bytes = text.as_bytes();
    let shape_ok = bytes.len() >= 19
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b' '
        && bytes[13] == b':'
        && bytes[16] == b':'
        && [&bytes[0..4], &bytes[5..7], &bytes[8..10], &bytes[11..13], &bytes[14..16], &bytes[17..19]]
            .iter()
            .all(|part| part.iter().all(u8::is_ascii_digit))
        && (bytes.len() == 19 || (bytes[19] == b'.' && bytes.len() > 20 && bytes[20..].iter().all(u8::is_ascii_digit)));
    // MySQL can store zero dates (`0000-00-00`) and out-of-range parts when sql_mode allows them. They are not instants,
    // so they are refused instead of being rendered as one.
    let number = |range: std::ops::Range<usize>| text.get(range).and_then(|part| part.parse::<u32>().ok());
    let calendar_ok = shape_ok
        && matches!(
            (number(0..4), number(5..7), number(8..10), number(11..13), number(14..16), number(17..19)),
            (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se))
                if y >= 1 && chrono::NaiveDate::from_ymd_opt(y as i32, mo, d).is_some() && h < 24 && mi < 60 && se < 60
        );
    if !calendar_ok {
        return Err(malformed("Time 형식"));
    }
    let fraction = text.get(19..).unwrap_or("").trim_end_matches('0');
    let fraction = if fraction == "." { "" } else { fraction };
    Ok(format!("{}T{}{fraction}+00:00", &text[0..10], &text[11..19]))
}
