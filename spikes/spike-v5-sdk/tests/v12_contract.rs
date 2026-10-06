use spike_v1_fixture::{digest, load_str, Form};
use spike_v5_sdk::{contract_fingerprint, contract_module, contract_ts};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");

#[test]
fn public_contract_fingerprint_and_generated_binding_use_the_same_basis() {
    let facts = load_str(A, Form::A).unwrap().execution;
    let fingerprint = contract_fingerprint(&facts);
    assert_eq!(fingerprint.len(), 64);
    assert!(fingerprint.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    let generated = contract_module(&facts, "./generic.ts");
    assert!(generated.starts_with(&contract_ts(&facts)));
    assert!(generated.contains(&format!("fingerprint: \"{fingerprint}\"")));
    assert!(generated.contains("ContractBinding<Contract>"));
    assert_eq!(fingerprint, contract_fingerprint(&facts));
}

#[test]
fn public_type_shape_changes_fingerprint_but_docs_and_policy_only_changes_do_not() {
    let original = load_str(A, Form::A).unwrap();
    let with_docs = A.replacen("모집 정보", "공개 설명 변경", 1);
    assert_ne!(with_docs, A);
    let with_policy = A.replacen("(ADMIN, MANAGER)", "(ADMIN)", 1);
    assert_ne!(with_policy, A);
    let renamed =
        A.replacen("select id, title, periodEnd, views, bookmarkCount, internalNote", "select id, periodEnd, views, bookmarkCount, internalNote", 1);
    assert_ne!(renamed, A);
    let docs = load_str(&with_docs, Form::A).unwrap();
    let policy = load_str(&with_policy, Form::A).unwrap();
    let changed = load_str(&renamed, Form::A).unwrap();
    assert_ne!(original.metadata, docs.metadata);
    assert_ne!(digest(&original.execution), digest(&policy.execution));
    assert_eq!(contract_fingerprint(&original.execution), contract_fingerprint(&docs.execution));
    assert_eq!(contract_fingerprint(&original.execution), contract_fingerprint(&policy.execution));
    assert_ne!(contract_fingerprint(&original.execution), contract_fingerprint(&changed.execution));
}
