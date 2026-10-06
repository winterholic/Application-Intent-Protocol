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

pub fn load_checked(src: &str, form: Form, options: &Value) -> Result<Checked, Vec<Diag>> {
    let enabled = dev_checks_enabled(options)?;
    // 개발 조언을 꺼도 실행 계약의 구조·타입 검사는 생략할 수 없다.
    let output = spike_v1_fixture::load_str(src, form)?;
    let advisories = if enabled { direct_reference_advice(&output.execution) } else { Vec::new() };
    Ok(Checked { output, advisories })
}
