use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::contract_ts_with_wire;

const SOURCE: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");

#[test]
fn generated_types_expose_email_and_date_as_strings_without_changing_id_wire() {
    let mut facts = load_str(SOURCE, Form::A).unwrap().execution;
    let resource = &mut facts["resources"]["Recruitment"];
    resource["fields"]["internalNote"]["ty"] = json!("Email?");
    resource["fields"]["periodEnd"]["ty"] = json!("Date");
    resource["fields"]["periodEnd"].as_object_mut().unwrap().remove("range");
    resource["exposeRead"]["select"]["internalNote"] = json!("field");
    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        let ts = contract_ts_with_wire(&facts, wire);
        assert!(ts.contains("internalNote: string | null;"), "{ts}");
        assert!(ts.contains("periodEnd: string;"), "{ts}");
        assert!(ts.contains(if wire == IdWire::SafeNumber { "Id<R extends string> = number" } else { "Id<R extends string> = string" }));
        assert!(!ts.contains("unknown /* Email */") && !ts.contains("unknown /* Date */"));
    }
}
