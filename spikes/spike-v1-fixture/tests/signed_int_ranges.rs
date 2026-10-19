use serde_json::json;
use spike_v1_fixture::{load_str, Form};

fn source(range: &str) -> String {
    format!(
        "actor principal resource Score {{ fields {{ id: Id; points: {range} }} rows read when actor != null \
         expose read {{ select id, points; sort points, id; budget {{ rows 10; depth 1; deadline 1s; cost 100 }} }} \
         expose create {{ fields points; allow actor != null }} }}"
    )
}

#[test]
fn signed_integer_ranges_have_one_contract_in_all_five_forms() {
    for (range, bounds) in [
        ("Int(-1000..1000)", [-1000_i64, 1000]),
        ("Int(-1000..-1)", [-1000, -1]),
        ("Int(-0..0)", [0, 0]),
        ("Int(-9007199254740991..9007199254740991)", [-9_007_199_254_740_991, 9_007_199_254_740_991]),
    ] {
        let text = source(range);
        let object = json!({"actor":"principal", "resources":{"Score":{
            "fields":{"id":"Id", "points":range}, "rows":"actor != null",
            "read":{"select":["id","points"], "sort":["points","id"],
                "budget":{"rows":10,"depth":1,"deadline":"1s","cost":100}},
            "exposeCreate":{"fields":["points"],"allow":"actor != null"}
        }}});
        let mut canonical = None;
        for (form, wrapped) in [
            (Form::A, text.clone()),
            (Form::ETs, format!("import {{ aip }} from '@aip/define'; export default aip`{text}`;")),
            (Form::EPy, format!("from aip.define import aip\nSPEC = aip(\"\"\"{text}\"\"\")")),
            (Form::HTs, format!("import {{ define }} from '@aip/define'; export const spec = define({object});")),
            (Form::HPy, format!("from aip.define import define\nSPEC = define({object})")),
        ] {
            let facts = load_str(&wrapped, form).unwrap_or_else(|e| panic!("{range}, {form:?}: {e:?}")).execution;
            assert_eq!(facts["resources"]["Score"]["fields"]["points"]["range"], json!(bounds));
            if let Some(expected) = &canonical {
                assert_eq!(&facts, expected, "{range}, {form:?}");
            } else {
                canonical = Some(facts);
            }
        }
    }
}

#[test]
fn reversed_signed_ranges_are_semantic_errors() {
    for range in ["Int(-1..-1000)", "Int(0..-1)"] {
        let errors = load_str(&source(range), Form::A).err().expect("reversed range must fail");
        assert!(errors.iter().any(|e| e.code == "DEF.BAD_RANGE"), "{range}: {errors:?}");
    }
}

#[test]
fn minus_is_not_enabled_in_unsigned_declaration_positions() {
    for text in [
        source("Text(-1..10)"),
        source("Text(0..-1)"),
        source("Decimal(-5,2)"),
        source("Decimal(5,-2)"),
        source("Int").replace("rows 10", "rows -10"),
        source("Int").replace("depth 1", "depth -1"),
        source("Int").replace("cost 100", "cost -100"),
    ] {
        let errors = load_str(&text, Form::A).err().expect("unsigned declaration must fail");
        assert!(errors.iter().any(|e| e.code == "DEF.PARSE_EXPECTED"), "{text}: {errors:?}");
    }
}

#[test]
fn malformed_and_overflowing_signed_bounds_fail_without_panicking() {
    for range in ["Int(--1..1)", "Int(-..1)", "Int(-1s..1)", "Int(-9223372036854775809..1)", "Int(-1..9223372036854775808)"] {
        assert!(load_str(&source(range), Form::A).is_err(), "{range}");
    }
}

#[test]
fn operation_ranges_still_enforce_the_sdk_safe_integer_domain() {
    for declaration in ["input { value: Int(-9007199254740992..0) }", "output { value: Int(-1..9007199254740992) }"] {
        let text = format!(
            "actor principal operation read score {{ {declaration} allow actor != null \
             effect none deadline 1s implementation \"score.compute\" }}"
        );
        let errors = load_str(&text, Form::A).err().expect("unsafe SDK range must fail");
        assert!(errors.iter().any(|e| e.code == "DEF.BAD_RANGE"), "{errors:?}");
    }
}
