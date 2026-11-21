//! A Decimal `sum` aggregate reaches the generated contract as a decimal string whose descriptor carries the declared
//! `Decimal(p', s)`, in both id wires, and does so without a change to the generators (the type comes from the aggregate's `ty`).
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::{contract_fingerprint_with_all_extensions, contract_module_with_all_extensions, contract_ts_with_all_extensions};

const SOURCE: &str = "
actor Member
resource Member { fields { id: Id } rows read when true }
access visibleTotal = totalOfVisible(Order)
resource Order {
  fields { id: Id; title: Text(1..20) }
  rows read when true
  aggregate total: Decimal(14,2) { source Item; sourceAccess visibleTotal; groupKey order; callerFilter none; rowOutput none; release sum(amount) }
  expose read { select id, title, total; sort id; budget { rows 10; depth 1; deadline 1s; cost 100 } }
}
resource Item { fields { id: Id; order: Order; amount: Decimal(12,2) } rows read when true }
";

#[test]
fn a_decimal_sum_is_a_string_with_the_declared_precision_in_the_contract() {
    let facts = load_str(SOURCE, Form::A).unwrap().execution;
    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        let ts = contract_ts_with_all_extensions(&facts, wire);
        assert!(ts.contains("total: string;"), "{ts}");
        assert!(!ts.contains("unknown /* Decimal"), "{ts}");
        let module = contract_module_with_all_extensions(&facts, "@aip/sdk", wire);
        assert!(module.contains(r#""total":{"nullable":false,"type":"Decimal<14,2>"}"#), "{module}");
    }
}

#[test]
fn the_contract_differs_when_the_sum_precision_differs() {
    let wide = load_str(SOURCE, Form::A).unwrap().execution;
    let tight = load_str(&SOURCE.replace("total: Decimal(14,2)", "total: Decimal(12,2)"), Form::A).unwrap().execution;
    let fp = |f: &serde_json::Value| contract_fingerprint_with_all_extensions(f, IdWire::DecimalString);
    assert_ne!(fp(&wide), fp(&tight), "the declared p' is part of the contract fingerprint");
}
