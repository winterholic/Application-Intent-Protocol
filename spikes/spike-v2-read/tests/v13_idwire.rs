use serde_json::{json, Value};
use spike_v2_read::{
    id_wire::{emit_id, encode_rows, parse_id, IdWire, MAX_SAFE_ID},
    plan::{Reject, MAX_OUTPUT_BYTES},
};

fn error_code<T>(result: Result<T, Reject>) -> &'static str {
    match result {
        Err(error) => error.code,
        Ok(_) => panic!("오류를 반환해야 함"),
    }
}

#[test]
fn legacy_keeps_existing_string_input_and_number_output_rules() {
    assert_eq!(parse_id(&json!("0007"), IdWire::Legacy).unwrap(), 7);
    assert_eq!(parse_id(&json!("123456789012345678"), IdWire::Legacy).unwrap(), 123_456_789_012_345_678);
    assert_eq!(error_code(parse_id(&json!("1234567890123456789"), IdWire::Legacy)), "BAD_VALUE");
    assert_eq!(error_code(parse_id(&json!(7), IdWire::Legacy)), "BAD_VALUE");
    assert_eq!(emit_id(7, IdWire::Legacy).unwrap(), json!(7));
    assert_eq!(emit_id(9_007_199_254_740_992, IdWire::Legacy).unwrap(), json!(9_007_199_254_740_992i64));
}

#[test]
fn safe_number_accepts_only_nonnegative_safe_integer_numbers() {
    assert_eq!(parse_id(&json!(0), IdWire::SafeNumber).unwrap(), 0);
    assert_eq!(parse_id(&json!(MAX_SAFE_ID), IdWire::SafeNumber).unwrap(), MAX_SAFE_ID);

    for invalid in [json!(-1), json!(1.5), json!("1"), json!(null), json!(true), json!([]), json!({})] {
        assert_eq!(error_code(parse_id(&invalid, IdWire::SafeNumber)), "BAD_VALUE", "입력 {invalid}");
    }
    for unsafe_integer in [json!(MAX_SAFE_ID + 1), json!(i64::MAX)] {
        assert_eq!(error_code(parse_id(&unsafe_integer, IdWire::SafeNumber)), "ID_OUT_OF_RANGE", "입력 {unsafe_integer}");
    }

    assert_eq!(emit_id(0, IdWire::SafeNumber).unwrap(), json!(0));
    assert_eq!(emit_id(MAX_SAFE_ID, IdWire::SafeNumber).unwrap(), json!(MAX_SAFE_ID));
    assert_eq!(error_code(emit_id(-1, IdWire::SafeNumber)), "BAD_VALUE");
    assert_eq!(error_code(emit_id(MAX_SAFE_ID + 1, IdWire::SafeNumber)), "ID_OUT_OF_RANGE");
}

#[test]
fn decimal_string_accepts_only_canonical_nonnegative_i64_strings() {
    assert_eq!(parse_id(&json!("0"), IdWire::DecimalString).unwrap(), 0);
    assert_eq!(parse_id(&json!("1"), IdWire::DecimalString).unwrap(), 1);
    assert_eq!(parse_id(&json!("1000000000000000000"), IdWire::DecimalString).unwrap(), 1_000_000_000_000_000_000);
    assert_eq!(parse_id(&json!(i64::MAX.to_string()), IdWire::DecimalString).unwrap(), i64::MAX);

    for invalid in [
        json!("-1"),
        json!("00"),
        json!("+1"),
        json!(" 1"),
        json!("1 "),
        json!(""),
        json!("1.0"),
        json!("x"),
        json!("9223372036854775808"),
        json!("10000000000000000000"),
        json!(1),
        json!(1.0),
        json!(null),
    ] {
        assert_eq!(error_code(parse_id(&invalid, IdWire::DecimalString)), "BAD_VALUE", "입력 {invalid}");
    }

    assert_eq!(emit_id(0, IdWire::DecimalString).unwrap(), json!("0"));
    assert_eq!(emit_id(i64::MAX, IdWire::DecimalString).unwrap(), json!("9223372036854775807"));
    assert_eq!(error_code(emit_id(-1, IdWire::DecimalString)), "BAD_VALUE");
}

fn row_output_type() -> Value {
    json!({
        "rows": {
            "id": { "ty": "Id<Thing>", "nullable": false },
            "fk": { "ty": "Ref<Other>", "nullable": false },
            "count": { "ty": "Int", "nullable": false },
            "text": { "ty": "Text", "nullable": false },
            "relation": {
                "nullable": true,
                "object": {
                    "id": { "ty": "Id<Other>", "nullable": false },
                    "fk": { "ty": "Ref<Thing>", "nullable": false },
                    "count": { "ty": "Int", "nullable": false },
                    "text": { "ty": "Text", "nullable": false }
                }
            }
        }
    })
}

#[test]
fn row_encoder_transforms_only_id_and_ref_and_preserves_null_relations_and_scalars() {
    let rows = vec![
        json!({
            "id": 10,
            "fk": 20,
            "count": 9_007_199_254_740_993u64,
            "text": "00010",
            "relation": { "id": 30, "fk": 40, "count": 50, "text": "00030" }
        }),
        json!({ "id": 11, "fk": 21, "count": 22, "text": "x", "relation": null }),
    ];

    let legacy = encode_rows(rows.clone(), &row_output_type(), IdWire::Legacy).unwrap();
    assert_eq!(legacy, rows, "legacy는 결과 JSON을 그대로 보존");

    let safe = encode_rows(rows.clone(), &row_output_type(), IdWire::SafeNumber).unwrap();
    assert_eq!(safe[0]["id"], json!(10));
    assert_eq!(safe[0]["fk"], json!(20));
    assert_eq!(safe[0]["relation"]["id"], json!(30));
    assert_eq!(safe[0]["relation"]["fk"], json!(40));
    assert_eq!(safe[0]["count"], json!(9_007_199_254_740_993u64));
    assert_eq!(safe[0]["text"], json!("00010"));
    assert_eq!(safe[0]["relation"]["count"], json!(50));
    assert_eq!(safe[0]["relation"]["text"], json!("00030"));
    assert!(safe[1]["relation"].is_null(), "nullable relation의 null을 유지");

    let decimal = encode_rows(rows, &row_output_type(), IdWire::DecimalString).unwrap();
    assert_eq!(decimal[0]["id"], json!("10"));
    assert_eq!(decimal[0]["fk"], json!("20"));
    assert_eq!(decimal[0]["relation"]["id"], json!("30"));
    assert_eq!(decimal[0]["relation"]["fk"], json!("40"));
    assert_eq!(decimal[0]["count"], json!(9_007_199_254_740_993u64));
    assert_eq!(decimal[0]["text"], json!("00010"));
    assert_eq!(decimal[0]["relation"]["count"], json!(50));
    assert_eq!(decimal[0]["relation"]["text"], json!("00030"));
    assert!(decimal[1]["relation"].is_null(), "nullable relation의 null을 유지");
}

#[test]
fn scalar_id_output_is_encoded_with_the_selected_wire() {
    let scalar_type = json!({ "value": { "ty": "Id<Thing>", "nullable": false } });
    let rows = vec![json!(42)];
    assert_eq!(encode_rows(rows.clone(), &scalar_type, IdWire::Legacy).unwrap(), rows);
    assert_eq!(encode_rows(rows.clone(), &scalar_type, IdWire::SafeNumber).unwrap(), rows);
    assert_eq!(encode_rows(rows, &scalar_type, IdWire::DecimalString).unwrap(), vec![json!("42")]);
}

#[test]
fn safe_number_row_encoder_rejects_ids_outside_javascript_safe_integer_range() {
    let output_type = json!({ "rows": { "id": { "ty": "Id<Thing>", "nullable": false } } });
    let error = error_code(encode_rows(vec![json!({ "id": MAX_SAFE_ID + 1 })], &output_type, IdWire::SafeNumber));
    assert_eq!(error, "ID_OUT_OF_RANGE");
}

#[test]
fn string_conversion_is_included_in_the_serialized_output_byte_limit() {
    let output_type = json!({
        "rows": {
            "id": { "ty": "Id<Thing>", "nullable": false },
            "padding": { "ty": "Text", "nullable": false }
        }
    });
    let empty = json!({ "id": i64::MAX, "padding": "" });
    let empty_bytes = serde_json::to_vec(&empty).unwrap().len();
    let row = json!({ "id": i64::MAX, "padding": "x".repeat(MAX_OUTPUT_BYTES - empty_bytes) });
    assert_eq!(serde_json::to_vec(&row).unwrap().len(), MAX_OUTPUT_BYTES, "원본 number row는 정확히 상한 이하여야 함");

    let error = error_code(encode_rows(vec![row], &output_type, IdWire::DecimalString));
    assert_eq!(error, "OUTPUT_TOO_LARGE", "문자열 ID의 JSON 따옴표 증가분을 포함해 거부해야 함");
}
