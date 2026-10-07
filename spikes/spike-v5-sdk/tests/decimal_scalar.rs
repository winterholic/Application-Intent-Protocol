use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::contract_ts_with_wire;

const SOURCE: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");

#[test]
fn generated_sdk_uses_decimal_strings_in_fields_filters_and_both_id_wires() {
    let mut facts = load_str(SOURCE, Form::A).unwrap().execution;
    let resource = &mut facts["resources"]["Recruitment"];
    resource["fields"]["periodEnd"]["ty"] = json!("Decimal<5,2>");
    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        let ts = contract_ts_with_wire(&facts, wire);
        assert!(ts.contains("periodEnd: string;"), "{ts}");
        assert!(ts.contains("filterFields: {\n      periodEnd: string;"), "typed filters should use decimal strings: {ts}");
        assert!(!ts.contains("unknown /* Decimal<5,2> */"));
        assert!(ts.contains(if wire == IdWire::SafeNumber { "Id<R extends string> = number" } else { "Id<R extends string> = string" }));
    }
}
