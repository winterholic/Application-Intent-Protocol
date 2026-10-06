use crate::plan::Reject;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IdWire {
    #[default]
    Legacy,
    SafeNumber,
    DecimalString,
}

impl IdWire {
    pub fn label(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::SafeNumber => "safe-number-v13",
            Self::DecimalString => "decimal-string-v13",
        }
    }
}

pub const MAX_SAFE_ID: i64 = 9_007_199_254_740_991;

fn bad_id() -> Reject {
    Reject { code: "BAD_VALUE", msg: "선택한 wire의 Id 형식·범위에 맞지 않음".into() }
}

pub fn parse_id(value: &Value, wire: IdWire) -> Result<i64, Reject> {
    if wire == IdWire::SafeNumber {
        let id = value.as_i64().filter(|id| *id >= 0).ok_or_else(bad_id)?;
        emit_id(id, wire)?;
        return Ok(id);
    }
    let text = value.as_str().ok_or_else(bad_id)?;
    let max_digits = if wire == IdWire::Legacy { 18 } else { 19 };
    if text.is_empty()
        || text.len() > max_digits
        || !text.bytes().all(|b| b.is_ascii_digit())
        || (wire == IdWire::DecimalString && text.len() > 1 && text.starts_with('0'))
    {
        return Err(bad_id());
    }
    text.parse().map_err(|_| bad_id())
}

pub fn emit_id(id: i64, wire: IdWire) -> Result<Value, Reject> {
    match wire {
        IdWire::Legacy => Ok(json!(id)),
        _ if id < 0 => Err(bad_id()),
        IdWire::SafeNumber if id > MAX_SAFE_ID => Err(Reject { code: "ID_OUT_OF_RANGE", msg: "Id가 JS 안전 정수 범위를 넘음".into() }),
        IdWire::SafeNumber => Ok(json!(id)),
        IdWire::DecimalString => Ok(json!(id.to_string())),
    }
}

fn malformed_output() -> Reject {
    Reject { code: "INTERNAL", msg: "실행 결과의 Id 구조가 계획 타입과 다름".into() }
}

fn encode_object(value: &mut Value, fields: &serde_json::Map<String, Value>, wire: IdWire) -> Result<(), Reject> {
    let object = value.as_object_mut().ok_or_else(malformed_output)?;
    for (name, ty) in fields {
        encode_value(object.get_mut(name).ok_or_else(malformed_output)?, ty, wire)?;
    }
    Ok(())
}

fn encode_value(value: &mut Value, ty: &Value, wire: IdWire) -> Result<(), Reject> {
    if value.is_null() && ty["nullable"] == true {
        return Ok(());
    }
    if let Some(fields) = ty["object"].as_object() {
        return encode_object(value, fields, wire);
    }
    let base = ty["ty"].as_str().unwrap_or("");
    if base.starts_with("Id<") || base.starts_with("Ref<") {
        *value = emit_id(value.as_i64().ok_or_else(malformed_output)?, wire)?;
    }
    Ok(())
}

/// PG JSON은 Rust에서 i64로 보존된다. JS가 숫자를 파싱하기 전에 계획의 Id 경계만 바꾼다.
pub fn encode_rows(mut rows: Vec<Value>, output_type: &Value, wire: IdWire) -> Result<Vec<Value>, Reject> {
    if wire == IdWire::Legacy {
        return Ok(rows);
    }
    let mut bytes = 0usize;
    for row in &mut rows {
        if let Some(fields) = output_type["rows"].as_object() {
            encode_object(row, fields, wire)?;
        } else if !output_type["value"].is_null() {
            encode_value(row, &output_type["value"], wire)?;
        } else {
            return Err(malformed_output());
        }
        bytes = bytes.saturating_add(serde_json::to_vec(row).map_err(|_| malformed_output())?.len());
        if bytes > crate::plan::MAX_OUTPUT_BYTES {
            return Err(Reject { code: "OUTPUT_TOO_LARGE", msg: "Id 변환 후 응답이 출력 상한을 넘음".into() });
        }
    }
    Ok(rows)
}
