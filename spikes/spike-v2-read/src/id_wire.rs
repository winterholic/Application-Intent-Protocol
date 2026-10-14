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
    Reject { code: "WIRE.BAD_VALUE", msg: "선택한 wire의 Id 형식·범위에 맞지 않음".into() }
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

pub fn emit_id(id: impl Into<EntityId>, wire: IdWire) -> Result<Value, Reject> {
    let id = match id.into() {
        EntityId::Number(id) => id,
        id @ EntityId::Uuid(_) => return Ok(json!(id.to_string())),
    };
    match wire {
        IdWire::Legacy => Ok(json!(id)),
        _ if id < 0 => Err(bad_id()),
        IdWire::SafeNumber if id > MAX_SAFE_ID => Err(Reject { code: "WIRE.ID_OUT_OF_RANGE", msg: "Id가 JS 안전 정수 범위를 넘음".into() }),
        IdWire::SafeNumber => Ok(json!(id)),
        IdWire::DecimalString => Ok(json!(id.to_string())),
    }
}

fn malformed_output() -> Reject {
    Reject { code: "WIRE.INTERNAL", msg: "실행 결과의 Id 구조가 계획 타입과 다름".into() }
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
    if let Some(fields) = ty["array"].as_object() {
        for item in value.as_array_mut().ok_or_else(malformed_output)? {
            encode_object(item, fields, wire)?;
        }
        return Ok(());
    }
    let base = ty["ty"].as_str().unwrap_or("");
    if base.starts_with("Id<") || base.starts_with("Ref<") {
        if let Some(text) = value.as_str() {
            parse_uuid(text).map_err(|_| malformed_output())?;
        } else {
            *value = emit_id(value.as_i64().ok_or_else(malformed_output)?, wire)?;
        }
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
            return Err(Reject { code: "WIRE.OUTPUT_TOO_LARGE", msg: "Id 변환 후 응답이 출력 상한을 넘음".into() });
        }
    }
    Ok(rows)
}

/// Resource identity, separate from numeric infrastructure counters and principal handles.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntityId {
    Number(i64),
    Uuid([u8; 16]),
}
impl From<i64> for EntityId {
    fn from(v: i64) -> Self {
        Self::Number(v)
    }
}
impl From<i32> for EntityId {
    fn from(v: i32) -> Self {
        Self::Number(v.into())
    }
}
impl PartialEq<i64> for EntityId {
    fn eq(&self, other: &i64) -> bool {
        *self == Self::Number(*other)
    }
}
impl std::fmt::Display for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(v) => write!(f, "{v}"),
            Self::Uuid(bytes) => {
                for (i, b) in bytes.iter().enumerate() {
                    if matches!(i, 4 | 6 | 8 | 10) {
                        write!(f, "-")?;
                    }
                    write!(f, "{b:02x}")?;
                }
                Ok(())
            }
        }
    }
}
impl serde::Serialize for EntityId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Number(v) => s.serialize_i64(*v),
            Self::Uuid(_) => s.serialize_str(&self.to_string()),
        }
    }
}
impl<'a> tokio_postgres::types::FromSql<'a> for EntityId {
    fn from_sql(ty: &tokio_postgres::types::Type, raw: &'a [u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        if *ty == tokio_postgres::types::Type::UUID {
            Ok(Self::Uuid(raw.try_into()?))
        } else {
            Ok(Self::Number(if *ty == tokio_postgres::types::Type::INT4 {
                i32::from_sql(ty, raw)?.into()
            } else if *ty == tokio_postgres::types::Type::INT2 {
                i16::from_sql(ty, raw)?.into()
            } else {
                i64::from_sql(ty, raw)?
            }))
        }
    }
    fn accepts(ty: &tokio_postgres::types::Type) -> bool {
        matches!(
            *ty,
            tokio_postgres::types::Type::UUID
                | tokio_postgres::types::Type::INT8
                | tokio_postgres::types::Type::INT4
                | tokio_postgres::types::Type::INT2
        )
    }
}
pub fn parse_uuid(text: &str) -> Result<EntityId, Reject> {
    let b = text.as_bytes();
    if b.len() != 36 {
        return Err(bad_id());
    }
    let mut bytes = [0; 16];
    let mut pos = 0;
    let nibble = |v: u8| match v {
        b'0'..=b'9' => Some(v - b'0'),
        b'a'..=b'f' => Some(v - b'a' + 10),
        _ => None,
    };
    for (i, v) in b.iter().copied().enumerate() {
        if matches!(i, 8 | 13 | 18 | 23) {
            if v != b'-' {
                return Err(bad_id());
            }
            continue;
        }
        let n = nibble(v).ok_or_else(bad_id)?;
        bytes[pos / 2] |= n << if pos % 2 == 0 { 4 } else { 0 };
        pos += 1;
    }
    Ok(EntityId::Uuid(bytes))
}
pub fn id_kind<'a>(facts: &'a Value, resource: &str) -> &'static str {
    if facts["resources"][resource]["idKind"] == "uuid" {
        "uuid"
    } else {
        "bigint"
    }
}
pub fn id_cast(facts: &Value, ty: &str) -> &'static str {
    crate::sqlgen::ref_target(ty).map_or("bigint", |r| id_kind(facts, r))
}
pub fn id_array_cast(facts: &Value, res: &str) -> &'static str {
    if id_kind(facts, res) == "uuid" {
        "uuid[]"
    } else {
        "bigint[]"
    }
}
pub fn parse_entity_id(facts: &Value, resource: &str, v: &Value, wire: IdWire) -> Result<EntityId, Reject> {
    if id_kind(facts, resource) == "uuid" {
        parse_uuid(v.as_str().ok_or_else(bad_id)?)
    } else {
        parse_id(v, wire).map(EntityId::Number)
    }
}
pub fn parse_typed_id(facts: &Value, ty: &str, v: &Value, wire: IdWire) -> Result<(String, &'static str), Reject> {
    let res = crate::sqlgen::ref_target(ty).ok_or_else(bad_id)?;
    Ok((parse_entity_id(facts, res, v, wire)?.to_string(), id_kind(facts, res)))
}

impl std::fmt::Debug for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(v) => std::fmt::Debug::fmt(v, f),
            Self::Uuid(_) => std::fmt::Debug::fmt(&self.to_string(), f),
        }
    }
}
