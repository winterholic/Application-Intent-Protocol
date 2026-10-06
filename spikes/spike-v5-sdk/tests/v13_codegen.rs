use serde_json::Value;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::{
    contract_fingerprint, contract_fingerprint_with_wire, contract_module, contract_module_with_wire, contract_ts, contract_ts_with_wire,
};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const GENERIC_IMPORT: &str = "../../spike-v5-sdk/sdk/generic.ts";

fn facts(src: &str) -> Value {
    load_str(src, Form::A).unwrap_or_else(|ds| panic!("fixture parse 실패: {ds:?}")).execution
}

fn replace_once(src: &str, old: &str, new: &str) -> String {
    assert_eq!(src.matches(old).count(), 1, "fixture 변경 대상이 정확히 한 번 있어야 함: {old}");
    src.replacen(old, new, 1)
}

fn id_filter_facts() -> Value {
    let src = replace_once(A, "filter periodEnd.gte, periodEnd.lte", "filter periodEnd.gte, periodEnd.lte, id.eq");
    let src = replace_once(
        &src,
        "select id, title, periodEnd, views, bookmarkCount, internalNote",
        "select id, title, periodEnd, views, bookmarkCount, internalNote, club",
    );
    facts(&src)
}

fn id_alias(ts: &str) -> &str {
    ts.lines().find(|line| line.starts_with("export type Id<")).expect("Id alias")
}

fn fingerprint_from_module(module: &str) -> &str {
    module
        .lines()
        .find_map(|line| line.strip_prefix("export const contractFingerprint = \"")?.strip_suffix("\";"))
        .expect("module contractFingerprint export")
}

#[test]
fn legacy_codegen_is_byte_identical_and_candidate_modes_type_ids_and_filter_inputs_consistently() {
    let facts = id_filter_facts();
    let legacy = contract_ts(&facts);
    assert_eq!(contract_ts_with_wire(&facts, IdWire::Legacy), legacy);
    assert!(!legacy.contains("export const idWire ="), "Legacy 출력은 기존 bytes를 유지하므로 새 mode tag를 추가하지 않음");

    let safe = contract_ts_with_wire(&facts, IdWire::SafeNumber);
    assert!(safe.contains("export const idWire = \"safe-number-v13\";"));
    assert!(id_alias(&safe).contains("= number &"), "SafeNumber ID alias must be numeric: {}", id_alias(&safe));
    assert!(safe.contains("id: Id<\"Recruitment\">;"), "Id filter input uses the numeric ID alias");
    assert!(!safe.contains("${number}"), "SafeNumber filter input must not widen to decimal strings");
    assert!(safe.contains("id: Id<\"Recruitment\">;"), "Resource Id field keeps its generated resource brand");

    let decimal = contract_ts_with_wire(&facts, IdWire::DecimalString);
    assert!(decimal.contains("export const idWire = \"decimal-string-v13\";"));
    assert!(id_alias(&decimal).contains("= string &"), "DecimalString ID alias must be string-based: {}", id_alias(&decimal));
    assert!(decimal.contains("id: Id<\"Recruitment\">;"), "Id filter input uses the decimal-string ID alias");
    assert!(!decimal.contains("${number}"), "DecimalString filter input must not widen to numbers");
    assert!(decimal.contains("club: Id<\"Club\">;"), "Ref<Club> field uses the selected ID wire type");

    assert!(id_alias(&legacy).contains("= number &"));
    assert!(legacy.contains("id: Id<\"Recruitment\"> | `${number}`;"), "legacy filter keeps its existing number/string input bytes");
}

#[test]
fn fingerprint_and_binding_module_follow_id_wire_but_ignore_docs_and_internal_policy() {
    let src = replace_once(A, "filter periodEnd.gte, periodEnd.lte", "filter periodEnd.gte, periodEnd.lte, id.eq");
    let base = facts(&src);
    let docs_src = replace_once(&src, "모집 정보", "설명 변경");
    let policy_src = replace_once(&src, "(ADMIN, MANAGER)", "(ADMIN)");
    let docs = facts(&docs_src);
    let policy = facts(&policy_src);

    let modes = [IdWire::Legacy, IdWire::SafeNumber, IdWire::DecimalString];
    let fingerprints: Vec<String> = modes.iter().map(|wire| contract_fingerprint_with_wire(&base, *wire)).collect();
    for (index, fingerprint) in fingerprints.iter().enumerate() {
        assert_eq!(fingerprint.len(), 64, "{:?} fingerprint length", modes[index]);
        assert!(fingerprint.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_eq!(contract_fingerprint_with_wire(&docs, modes[index]), *fingerprint, "docs는 {:?} 공개 계약 식별값을 바꾸지 않음", modes[index]);
        assert_eq!(
            contract_fingerprint_with_wire(&policy, modes[index]),
            *fingerprint,
            "내부 정책은 {:?} 공개 계약 식별값을 바꾸지 않음",
            modes[index]
        );

        let module = contract_module_with_wire(&base, GENERIC_IMPORT, modes[index]);
        assert!(module.starts_with(&contract_ts_with_wire(&base, modes[index])));
        assert!(module.contains(&format!("import type {{ ContractBinding }} from \"{GENERIC_IMPORT}\";")));
        assert_eq!(fingerprint_from_module(&module), fingerprint, "module fingerprint helper와 불일치");
        assert!(module.contains(&format!("ContractBinding<Contract> = {{ fingerprint: \"{fingerprint}\" }};")));
    }
    assert_ne!(fingerprints[0], fingerprints[1], "Legacy와 SafeNumber wire tag는 계약 지문에 반영");
    assert_ne!(fingerprints[0], fingerprints[2], "Legacy와 DecimalString wire tag는 계약 지문에 반영");
    assert_ne!(fingerprints[1], fingerprints[2], "candidate wire mode는 서로 다른 계약 지문");
}

#[test]
fn legacy_fingerprint_and_module_remain_byte_compatible() {
    let facts = facts(A);
    assert_eq!(contract_ts_with_wire(&facts, IdWire::Legacy), contract_ts(&facts));
    assert_eq!(contract_fingerprint_with_wire(&facts, IdWire::Legacy), contract_fingerprint(&facts));
    assert_eq!(contract_module_with_wire(&facts, GENERIC_IMPORT, IdWire::Legacy), contract_module(&facts, GENERIC_IMPORT),);
}
