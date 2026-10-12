use serde_json::Value;
use spike_v1_fixture::{
    diag::{Diag, Span},
    sema::Output,
    Form,
};
use std::collections::BTreeSet;

#[derive(Debug, PartialEq, Eq)]
pub struct Advisory {
    pub code: &'static str,
    pub anchor: String,
    pub msg: String,
}

pub struct Checked {
    pub output: Output,
    pub advisories: Vec<Advisory>,
}

fn option_error(code: &'static str, msg: &str) -> Vec<Diag> {
    vec![Diag::new(code, msg, Span::default())]
}

fn dev_checks_enabled(options: &Value) -> Result<bool, Vec<Diag>> {
    let options = options.as_object().ok_or_else(|| option_error("BAD_OPTION", "검사 옵션은 객체여야 함"))?;
    if options.keys().any(|key| key != "devChecks") {
        return Err(option_error("UNKNOWN_OPTION", "지원하는 옵션은 devChecks뿐임"));
    }
    match options.get("devChecks") {
        None => Ok(false),
        Some(Value::Bool(enabled)) => Ok(*enabled),
        _ => Err(option_error("BAD_OPTION", "devChecks는 boolean이어야 함")),
    }
}

fn direct_reference_advice(execution: &Value) -> Vec<Advisory> {
    let mut calls = BTreeSet::new();
    let mut pending = vec![execution];
    while let Some(node) = pending.pop() {
        match node {
            Value::Object(fields) => {
                if let Some(call) = fields.get("call").and_then(Value::as_str) {
                    calls.insert(call);
                }
                pending.extend(fields.values());
            }
            Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    execution["predicates"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| !calls.contains(name.as_str()))
        .map(|(name, _)| Advisory {
            code: "ADVISORY_UNUSED_PREDICATE",
            anchor: format!("predicate:{name}"),
            msg: "실행 facts에 이 predicate의 직접 호출 참조가 없음".into(),
        })
        .collect()
}

/// An `expose read` without `budget` can only be read as the target of another resource's `traverse`; if no
/// resource traverses it, no caller can read it at all, and the first read fails at run time with
/// `NOT_ROOT_QUERYABLE`. The usual cause is a forgotten `budget`, so it is said at check time.
fn unreachable_read_advice(execution: &Value) -> Vec<Advisory> {
    let Some(resources) = execution["resources"].as_object() else { return Vec::new() };
    let mut targets = BTreeSet::new();
    for resource in resources.values() {
        let read = &resource["exposeRead"];
        let one = read["traverse"].as_object().into_iter().flatten().filter_map(|(_, t)| t["target"].as_str());
        let many = read["traverseMany"].as_object().into_iter().flatten().filter_map(|(_, t)| t["target"].as_str());
        targets.extend(one.chain(many));
    }
    resources
        .iter()
        .filter(|(name, resource)| {
            let read = &resource["exposeRead"];
            read.is_object() && read["rootQueryable"] != true && !targets.contains(name.as_str())
        })
        .map(|(name, _)| Advisory {
            code: "ADVISORY_UNREACHABLE_READ",
            anchor: format!("resource:{name}"),
            msg: format!(
                "`{name}`의 expose read에 budget이 없어 루트로 읽을 수 없고 다른 resource의 traverse 대상도 아님. 루트 조회가 목적이면 budget을 추가"
            ),
        })
        .collect()
}

/// `club: Club` is a reference (`club_id bigint REFERENCES club(id)`, joinable, checked by the database);
/// `club: Club.Id` is only a number that happens to be a club id: no foreign key, nothing to traverse, and a
/// deleted club leaves it dangling. Both read as "this row belongs to a club", and the second is chosen by mistake.
fn unconstrained_id_advice(execution: &Value) -> Vec<Advisory> {
    let Some(resources) = execution["resources"].as_object() else { return Vec::new() };
    let mut out = Vec::new();
    for (name, resource) in resources {
        for (field, decl) in resource["fields"].as_object().into_iter().flatten() {
            let ty = decl["ty"].as_str().unwrap_or("").trim_end_matches('?');
            if field != "id" && ty.starts_with("Id<") {
                let target = ty.trim_start_matches("Id<").trim_end_matches('>');
                out.push(Advisory {
                    code: "ADVISORY_ID_WITHOUT_REFERENCE",
                    anchor: format!("resource:{name}.{field}"),
                    msg: format!(
                        "`{name}.{field}`는 `{target}.Id`라 외래 키가 없는 id 값이다. `{target}` 행에 속한다는 뜻이면 `{field}: {target}`(참조)로 쓴다. 외부 id를 일부러 저장하는 것이면 무시한다"
                    ),
                });
            }
        }
    }
    out
}

pub fn load_checked(src: &str, form: Form, options: &Value) -> Result<Checked, Vec<Diag>> {
    let enabled = dev_checks_enabled(options)?;
    // 개발 조언을 꺼도 실행 계약의 구조·타입 검사는 생략할 수 없다.
    let output = spike_v1_fixture::load_str(src, form)?;
    let mut advisories = Vec::new();
    if enabled {
        advisories.extend(direct_reference_advice(&output.execution));
        advisories.extend(unreachable_read_advice(&output.execution));
        advisories.extend(unconstrained_id_advice(&output.execution));
    }
    Ok(Checked { output, advisories })
}
