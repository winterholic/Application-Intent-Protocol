use super::check_record;
use serde_json::{json, Value};

fn check(field: &str, ty: &str, value: Value, what: &str) -> Result<(), spike_v2_read::plan::Reject> {
    let facts = json!({ "enums": { "Role": ["ADMIN", "MEMBER"] } });
    let decl = json!([[field, ty]]);
    check_record(&facts, &decl, &json!({ (field): value }), what)
}

#[test]
fn nul_text_and_url_are_rejected_on_input_and_output() {
    for ty in ["Text", "Url"] {
        for (value, what, code) in [(json!("left\u{0000}right"), "입력", "BAD_VALUE"), (json!("left\u{0000}right"), "출력", "OUTPUT_INVALID")] {
            let err = check("value", ty, value, what).expect_err("NUL must be rejected");
            assert_eq!(err.code, code, "{ty} / {what}");
        }

        check("value", ty, json!("일반 문자열"), "입력").expect("ordinary text should pass");
    }
}

#[test]
fn time_requires_a_real_rfc3339_date_time_on_input_and_output() {
    for invalid in ["2026-02-29T12:34:56Z", "2024-02-29T25:00:00Z", "2024-02-29T12:34:56+25:00", "2024-02-29T12:34:56"] {
        let facts = json!({ "enums": {} });
        assert!(spike_v2_read::scalar::parse(&facts, "Time", &json!(invalid)).is_err(), "V2 should reject {invalid}");
        for (what, code) in [("입력", "BAD_VALUE"), ("출력", "OUTPUT_INVALID")] {
            let err = check("when", "Time", json!(invalid), what).expect_err(invalid);
            assert_eq!(err.code, code, "{invalid} / {what}");
        }
    }
}

#[test]
fn valid_time_spellings_match_v2_and_are_not_rewritten() {
    let valid = ["2024-02-29T12:34:56Z", "2024-02-29t12:34:56.123456+05:30", "2024-02-29T12:34:56z"];
    let facts = json!({ "enums": {} });
    for original in valid {
        assert!(spike_v2_read::scalar::parse(&facts, "Time", &json!(original)).is_ok(), "V2 should accept {original}");
        let value = json!({ "when": original });
        let before = value.clone();
        let decl = json!([["when", "Time"]]);
        check_record(&facts, &decl, &value, "입력").expect(original);
        assert_eq!(value, before, "validation must not rewrite {original}");
    }
}

#[test]
fn nullable_presence_enum_membership_and_exact_keys_are_preserved() {
    let facts = json!({ "enums": { "Role": ["ADMIN", "MEMBER"] } });
    let decl = json!([["required", "Text"], ["optional", "Text?"], ["role", "Enum<Role>"]]);

    check_record(&facts, &decl, &json!({ "required": "ok", "optional": null, "role": "ADMIN" }), "입력")
        .expect("nullable null and declared enum member should pass");

    for (value, code) in [
        (json!({ "required": null, "optional": null, "role": "ADMIN" }), "BAD_VALUE"),
        (json!({ "required": "ok", "role": "ADMIN" }), "BAD_VALUE"),
        (json!({ "required": "ok", "optional": null, "role": "UNKNOWN" }), "BAD_VALUE"),
        (json!({ "required": "ok", "optional": null, "role": "ADMIN", "extra": true }), "BAD_VALUE"),
    ] {
        assert_eq!(check_record(&facts, &decl, &value, "입력").expect_err("invalid record").code, code);
    }

    let output_err =
        check_record(&facts, &decl, &json!({ "required": "ok", "optional": null, "role": "UNKNOWN" }), "출력").expect_err("invalid worker output");
    assert_eq!(output_err.code, "OUTPUT_INVALID");
}

#[test]
fn decimal_worker_input_and_output_are_lossless_strings_with_declared_precision() {
    let facts = json!({ "enums": {} });
    let decl = json!([["amount", "Decimal<5,2>"]]);
    check_record(&facts, &decl, &json!({ "amount": "123.45" }), "입력").expect("valid decimal input");
    check_record(&facts, &decl, &json!({ "amount": "123.45" }), "출력").expect("valid decimal output");

    for invalid in ["1234.56", "1.001", "NaN", "1e3", "+1", "01.00"] {
        assert_eq!(check_record(&facts, &decl, &json!({ "amount": invalid }), "입력").unwrap_err().code, "BAD_VALUE", "{invalid}");
        assert_eq!(check_record(&facts, &decl, &json!({ "amount": invalid }), "출력").unwrap_err().code, "OUTPUT_INVALID", "{invalid}");
    }
    assert_eq!(check_record(&facts, &decl, &json!({ "amount": 12.5 }), "입력").unwrap_err().code, "BAD_VALUE");
}
