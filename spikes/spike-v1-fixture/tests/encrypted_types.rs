use spike_v1_fixture::{load_str, parser, Form};

#[test]
fn encrypted_modifier_is_parsed_as_one_explicit_scoped_type() {
    let parsed = parser::parse_type_str("Text(1..10000)? encrypted(scope: project, lookup: eq)", Default::default()).expect("scoped modifier");
    let encryption = parsed.encrypted.expect("encryption metadata");
    assert_eq!(encryption.scope, "project");
    assert!(encryption.lookup_eq);
    assert_eq!(parsed.range, Some((1, 10000)));
    assert!(parsed.nullable);
    let service = parser::parse_type_str("Text encrypted(scope: service)", Default::default()).unwrap();
    assert!(!service.encrypted.unwrap().lookup_eq);
}

#[test]
fn malformed_modifiers_have_stable_diagnostics() {
    for (source, code) in [
        ("Text encrypted", "DEF.ENCRYPTED_MISSING_SCOPE"),
        ("Text encrypted()", "DEF.ENCRYPTED_MISSING_SCOPE"),
        ("Text encrypted(lookup: eq)", "DEF.ENCRYPTED_MISSING_SCOPE"),
        ("Text encrypted(scope: service, extra: yes)", "DEF.ENCRYPTED_BAD_OPTION"),
        ("Text encrypted(scope: service, lookup: contains)", "DEF.ENCRYPTED_BAD_OPTION"),
        ("Text encrypted(scope: service, scope: owner)", "DEF.ENCRYPTED_REDUNDANT_OPTION"),
        ("Text encrypted(scope: service, lookup: eq, lookup: eq)", "DEF.ENCRYPTED_REDUNDANT_OPTION"),
        ("Text encrypted(scope: service) encrypted(scope: owner)", "DEF.ENCRYPTED_REDUNDANT_OPTION"),
    ] {
        let err = parser::parse_type_str(source, Default::default()).unwrap_err();
        assert_eq!(err.code, code, "{source}: {err:?}");
    }
}

#[test]
fn normalized_definitions_remain_blocked_at_the_runtime_boundary() {
    let src = "actor principal resource Secret {fields {id: Id; value: Text encrypted(scope: service)}}";
    let definition = load_str(src, Form::A).unwrap_or_else(|errors| panic!("{errors:?}"));
    assert_eq!(
        definition.execution["resources"]["Secret"]["fields"]["value"]["encrypted"],
        serde_json::json!({"scope":{"kind":"service"},"lookup":"none"})
    );
    assert_eq!(spike_v1_fixture::sema::require_supported_runtime(&definition.execution).unwrap_err().code, "DEF.ENCRYPTION_RUNTIME_UNAVAILABLE");
    assert!(definition.advisories.iter().any(|advisory| advisory.code == "DEF.ADVISORY_ENCRYPTION_RUNTIME_DISABLED"));
}

fn rejects(source: &str, code: &str) {
    let errors = load_str(source, Form::A).err().expect("invalid declaration");
    assert!(errors.iter().any(|e| e.code == code), "expected {code}: {errors:?}");
}

#[test]
fn encrypted_identity_type_location_and_scope_constraints_are_explicit() {
    for (resource, code) in [
        ("fields {id: Id encrypted(scope: service)}", "DEF.ENCRYPTED_ID_FORBIDDEN"),
        ("fields {value: Text encrypted(scope: service)}", "DEF.ENCRYPTED_REQUIRES_ID"),
        ("fields {id: Id; value: Int encrypted(scope: service)}", "DEF.ENCRYPTED_TYPE_UNSUPPORTED"),
        ("fields {id: Id; value: Text encrypted(scope: service) default database}", "DEF.ENCRYPTED_DEFAULT_DB_FORBIDDEN"),
        ("fields {id: Id; value: Text default database encrypted(scope: service)}", "DEF.ENCRYPTED_DEFAULT_DB_FORBIDDEN"),
        ("fields {id: Id; value: Text encrypted(scope: missing)}", "DEF.ENCRYPTED_BAD_SCOPE"),
        ("fields {id: Id; project: Project?; value: Text encrypted(scope: project)}", "DEF.ENCRYPTED_BAD_SCOPE"),
        ("fields {id: Id; project: Text; value: Text encrypted(scope: project)}", "DEF.ENCRYPTED_BAD_SCOPE"),
        ("fields {id: Id; value: Text encrypted(scope: owner)}", "DEF.ENCRYPTED_AMBIGUOUS_OWNER"),
        ("fields {id: Id; value: Text encrypted(scope: service)} unique value", "DEF.ENCRYPTED_UNIQUE_REQUIRES_LOOKUP"),
    ] {
        rejects(&format!("actor principal resource Project {{fields {{id: Id}}}} resource Secret {{{resource}}}"), code);
    }
    rejects("actor principal operation read worker {input {value: Text encrypted(scope: service)} output {ok: Bool} allow true effect none deadline 1s implementation \"fixture.worker\"}", "DEF.ENCRYPTED_LOCATION_FORBIDDEN");
}

#[test]
fn encrypted_scope_cannot_be_patched_or_assigned_and_secret_expressions_are_rejected() {
    let prefix = "actor principal resource Project {fields {id: Id}} resource Secret {fields {id: Id; project: Project; value: Text encrypted(scope: project)}";
    for (suffix, code) in [
        ("transition move {patch project from true allow true}", "DEF.ENCRYPTED_MUTABLE_SCOPE"),
        ("transition move {to project = project from true allow true}", "DEF.ENCRYPTED_MUTABLE_SCOPE"),
        ("transition replace {to value = \"fixture\" from true allow true}", "DEF.ENCRYPTED_TRANSITION_VALUE_FORBIDDEN"),
        ("transition compare {patch value expect value from true allow true}", "DEF.ENCRYPTED_TRANSITION_VALUE_FORBIDDEN"),
        ("rows read when value = \"fixture\"", "DEF.ENCRYPTED_POLICY_EXPR_FORBIDDEN"),
        ("rows read when this.value = \"fixture\"", "DEF.ENCRYPTED_POLICY_EXPR_FORBIDDEN"),
        ("expose read {select id filter value.eq budget {rows 10; depth 1}}", "DEF.ENCRYPTED_FILTER_FORBIDDEN"),
        ("expose read {select id sort value budget {rows 10; depth 1}}", "DEF.ENCRYPTED_SORT_FORBIDDEN"),
        ("aggregate total: Int {source Secret callerFilter none rowOutput none release sum(value)}", "DEF.ENCRYPTED_AGGREGATE_FORBIDDEN"),
    ] {
        rejects(&format!("{prefix} {suffix}}}"), code);
    }
}

#[test]
fn all_five_forms_normalize_encrypted_metadata_to_the_same_execution_digest() {
    let body = "actor principal resource Secret {fields {id: Id; value: Text encrypted(scope: service)}}";
    let literal = r#"{"actor":"principal","resources":{"Secret":{"fields":{"id":"Id","value":"Text encrypted(scope: service)"}}}}"#;
    let expected = load_str(body, Form::A).unwrap().execution;
    let plain = load_str("actor principal resource Secret {fields {id: Id; value: Text}}", Form::A).unwrap().execution;
    assert_ne!(spike_v1_fixture::digest(&expected), spike_v1_fixture::digest(&plain));
    for (source, form) in [
        (body.to_string(), Form::A),
        (format!("import {{aip}} from '@aip/define'; export const spec = aip`{body}`;"), Form::ETs),
        (format!("from aip.define import aip\nSPEC = aip(\"\"\"{body}\"\"\")"), Form::EPy),
        (format!("import {{define}} from '@aip/define'; export const spec = define({literal});"), Form::HTs),
        (format!("from aip.define import define\nSPEC = define({literal})"), Form::HPy),
    ] {
        let actual = load_str(&source, form).unwrap_or_else(|errors| panic!("{form:?}: {errors:?}")).execution;
        assert_eq!(actual, expected, "{form:?}");
        assert_eq!(spike_v1_fixture::digest(&actual), spike_v1_fixture::digest(&expected));
    }
}

#[test]
fn a_field_named_encrypted_remains_valid_without_semicolon_separators() {
    let src = "actor principal resource Secret {fields {id: Id encrypted: Text}}";
    let facts = load_str(src, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    assert_eq!(facts["resources"]["Secret"]["fields"]["encrypted"]["ty"], "Text");
}

#[test]
fn cross_resource_effects_and_predicate_paths_cannot_bypass_scope_and_secret_guards() {
    let src = "actor principal resource Parent {fields {id: Id} transition change {from true patch id allow true update Secret where parent = this.id {parent = this.id}}} resource Secret {fields {id: Id; parent: Parent; value: Text encrypted(scope: parent)}}";
    rejects(src, "DEF.ENCRYPTED_MUTABLE_SCOPE");
    rejects("actor principal predicate visible(s: Secret) = s.value = \"fixture\" resource Secret {fields {id: Id; value: Text encrypted(scope: service)}}", "DEF.ENCRYPTED_POLICY_EXPR_FORBIDDEN");
    let src = "actor Member resource Member {fields {id: Id}} resource Secret {fields {id: Id; first: Member; second: Member; value: Text encrypted(scope: owner)}}";
    rejects(src, "DEF.ENCRYPTED_AMBIGUOUS_OWNER");
}

#[test]
fn reference_and_owner_scopes_are_resolved_to_immutable_resource_identity() {
    let source="actor Member resource Member {fields {id: Uuid; note: Text encrypted(scope: owner)}} resource Secret {fields {id: Id; owner: Member; value: Text? encrypted(scope: owner); projectValue: Text encrypted(scope: owner, lookup: eq)}}";
    let facts = load_str(source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    assert_eq!(
        facts["resources"]["Member"]["fields"]["note"]["encrypted"]["scope"],
        serde_json::json!({"kind":"owner","field":"id","target":"Member"})
    );
    assert_eq!(
        facts["resources"]["Secret"]["fields"]["value"]["encrypted"]["scope"],
        serde_json::json!({"kind":"owner","field":"owner","target":"Member"})
    );
    let reference = source
        .replace("scope: owner, lookup: eq", "scope: ownerRef, lookup: eq")
        .replace("owner: Member; value:", "owner: Member; ownerRef: Member; value:")
        .replace("value: Text? encrypted(scope: owner)", "value: Text? encrypted(scope: service)");
    let reference = reference.replace("note: Text encrypted(scope: owner)", "note: Text encrypted(scope: service)");
    let facts = load_str(&reference, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    assert_eq!(
        facts["resources"]["Secret"]["fields"]["projectValue"]["encrypted"],
        serde_json::json!({"scope":{"kind":"ref","field":"ownerRef","target":"Member"},"lookup":"eq"})
    );
}

#[test]
fn encrypted_scalar_facts_keep_public_types_and_repeat_cannot_compare_patch_secrets() {
    for (ty, expected) in [("Email", "Email"), ("Url?", "Url?"), ("Decimal(8,2)", "Decimal<8,2>")] {
        let source = format!("actor principal resource Secret {{fields {{id: Id; value: {ty} encrypted(scope: service)}}}}");
        let facts = load_str(&source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
        assert_eq!(facts["resources"]["Secret"]["fields"]["value"]["ty"], expected);
        assert_eq!(spike_v1_fixture::sema::require_supported_runtime(&facts).unwrap_err().code, "DEF.ENCRYPTION_RUNTIME_UNAVAILABLE");
    }
    rejects("actor principal resource Secret {fields {id: Id; value: Text encrypted(scope: service)} transition compare {patch value from true allow true repeat unchanged}}","DEF.UNSUPPORTED");
}
