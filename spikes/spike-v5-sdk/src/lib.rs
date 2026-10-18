//! V1 typed facts → TS 공개 계약. 기존 읽기 산출물과 V14 쓰기 후보를 분리하고 내부 정책 식은 내보내지 않는다.
use serde_json::Value;
use sha2::{Digest, Sha256};
use spike_v2_read::id_wire::IdWire;
use std::fmt::Write;
mod python;
pub use python::contract_module_python;

pub fn contract_ts_with_apply(facts: &Value, wire: IdWire) -> String {
    let mut s = contract_ts_with_wire(facts, wire);
    s.push_str("\nexport const applyContractVersion = \"typed-apply-v14\";\nexport interface ApplyContract {\n");
    for (res, rf) in facts["resources"].as_object().unwrap() {
        for (action, ex) in rf["exposeApply"].as_object().into_iter().flatten() {
            writeln!(s, "  \"{res}.{action}\": {{").unwrap();
            let input = if wire == IdWire::Legacy { "string".into() } else { format!("Id<\"{res}\">") };
            writeln!(s, "    idInput: {input};\n    idOutput: Id<\"{res}\">;").unwrap();
            for (declared, key, optional) in [("patch", "values", "?"), ("expect", "expect", "")] {
                if let Some(names) = rf["transitions"][action][declared].as_array() {
                    writeln!(s, "    {key}: {{").unwrap();
                    for name in names.iter().filter_map(Value::as_str) {
                        let ty = rf["fields"][name]["ty"].as_str().unwrap();
                        writeln!(s, "      {name}{optional}: {};", ts_type(facts, ty)).unwrap();
                    }
                    s.push_str("    };\n");
                }
            }
            let mut targets: Vec<_> = ex["target"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            targets.sort();
            targets.dedup();
            writeln!(s, "    targets: {};", targets.iter().map(|x| format!("\"{x}\"")).collect::<Vec<_>>().join(" | ")).unwrap();
            writeln!(s, "    maxRows: {};\n    where: {{", ex["bulkMaxRows"].as_i64().unwrap_or(1)).unwrap();
            if targets.contains(&"where") {
                let fields: std::collections::BTreeSet<_> =
                    rf["exposeRead"]["filter"].as_array().into_iter().flatten().filter_map(|x| x.as_str()?.strip_suffix(".eq")).collect();
                for field in fields {
                    let ty = rf["fields"][field]["ty"].as_str().unwrap().trim_end_matches('?');
                    let supported =
                        spike_v2_read::scalar::supported(ty) || (wire != IdWire::Legacy && (ty.starts_with("Id<") || ty.starts_with("Ref<")));
                    let value_type = if supported { ts_type(facts, ty) } else { "never".into() };
                    writeln!(s, "      {field}: {value_type};").unwrap();
                }
            }
            s.push_str("    };\n  };\n");
        }
    }
    s.push_str("}\n");
    let mut inputs = serde_json::Map::new();
    for (res, rf) in facts["resources"].as_object().unwrap() {
        for (action, _) in rf["exposeApply"].as_object().into_iter().flatten() {
            let mut descriptor = serde_json::Map::new();
            for key in ["patch", "expect"] {
                if let Some(names) = rf["transitions"][action][key].as_array() {
                    let fields: serde_json::Map<String, Value> = names
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|name| {
                            let field = &rf["fields"][name];
                            (name.into(), serde_json::json!({"ty":field["ty"],"range":field["range"]}))
                        })
                        .collect();
                    descriptor.insert(key.into(), Value::Object(fields));
                }
            }
            if !descriptor.is_empty() {
                inputs.insert(format!("{res}.{action}"), Value::Object(descriptor));
            }
        }
    }
    if !inputs.is_empty() {
        writeln!(s, "export const applyInputDescriptors = {} as const;", Value::Object(inputs)).unwrap();
    }
    push_write_contract(&mut s, facts, wire);
    s
}

/// 생성 binding 타입에 덧붙이는 create·compose 계약. 둘 다 없으면 빈 문자열이라 기존 모듈 글자가 그대로다.
fn write_binding(facts: &Value) -> (&'static str, String) {
    let resources = facts["resources"].as_object();
    let any = |key: &str| resources.is_some_and(|r| r.values().any(|rf| !rf[key].is_null()));
    let (creates, composes) = (any("exposeCreate"), any("exposeCompose"));
    if !creates && !composes {
        return ("", String::new());
    }
    (
        ", WriteBinding",
        format!(" & WriteBinding<{}, {}>", if creates { "CreateContract" } else { "{}" }, if composes { "ComposeContract" } else { "{}" }),
    )
}

/// 호출자 값 하나의 TS 타입. 정의 Id·참조는 wire를 따르고 create가 받지 못하는 타입(Time·Url)은 `never`다.
fn create_value_type(facts: &Value, ty: &str, wire: IdWire) -> String {
    let nullable = ty.ends_with('?');
    let base = ty.trim_end_matches('?');
    let t = if let Some(target) = base.strip_prefix("Ref<").or_else(|| base.strip_prefix("Id<")).and_then(|x| x.strip_suffix('>')) {
        if wire == IdWire::Legacy {
            "string".to_string()
        } else {
            format!("Id<\"{target}\">")
        }
    } else if matches!(base, "Time" | "Url") {
        "never".to_string()
    } else {
        ts_type(facts, base)
    };
    if nullable && t != "never" {
        format!("{t} | null")
    } else {
        t
    }
}

fn union_of(items: impl IntoIterator<Item = String>) -> String {
    let items: Vec<String> = items.into_iter().map(|x| format!("\"{x}\"")).collect();
    if items.is_empty() {
        "never".into()
    } else {
        items.join(" | ")
    }
}

/// 공개 create(`expose create`)와 compose(`expose compose`)의 타입. 둘 다 없으면 아무것도 내보내지 않아 기존 계약 지문이 그대로다.
fn push_write_contract(s: &mut String, facts: &Value, wire: IdWire) {
    let resources = facts["resources"].as_object().unwrap();
    let creates: Vec<_> = resources.iter().filter(|(_, rf)| !rf["exposeCreate"].is_null()).collect();
    let composes: Vec<_> = resources.iter().filter(|(_, rf)| !rf["exposeCompose"].is_null()).collect();
    if creates.is_empty() && composes.is_empty() {
        return;
    }
    use spike_v2_read::write_limits::{BUNDLE_MAX_OPS, BUNDLE_MAX_ROWS, COMPOSE_MAX_STEPS};
    writeln!(s, "\nexport const bundleLimits = {{ maxOps: {BUNDLE_MAX_OPS}, maxRows: {BUNDLE_MAX_ROWS}, maxSteps: {COMPOSE_MAX_STEPS} }} as const;")
        .unwrap();
    // 이 resource가 호출자에게 돌려줄 수 있는 id 타입. `id` 열이 없는 비공개 관계 테이블은 null이다.
    let id_out = |res: &str| if resources[res]["fields"].get("id").is_some() { format!("Id<\"{res}\">") } else { "null".to_string() };
    let fields_of = |res: &str, names: &[String]| -> String {
        names
            .iter()
            .map(|f| format!("{f}: {};", create_value_type(facts, resources[res]["fields"][f.as_str()]["ty"].as_str().unwrap(), wire)))
            .collect::<Vec<_>>()
            .join(" ")
    };
    if !creates.is_empty() {
        s.push_str("export interface CreateContract {\n");
        for (res, rf) in &creates {
            let allowed: Vec<String> =
                rf["exposeCreate"]["fields"].as_array().unwrap().iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            // 비워 둘 수 있는 것은 null을 받는 필드뿐이다. 필수 필드를 빼면 서버가 BAD_VALUE로 거절한다.
            let optional: Vec<String> = allowed.iter().filter(|f| rf["fields"][f.as_str()]["ty"].as_str().unwrap().ends_with('?')).cloned().collect();
            writeln!(
                s,
                "  \"{res}\": {{ idOutput: {}; fields: {{ {} }}; optional: {} }};",
                id_out(res),
                fields_of(res, &allowed),
                union_of(optional)
            )
            .unwrap();
        }
        s.push_str("}\n");
    }
    if !composes.is_empty() {
        s.push_str("export interface ComposeContract {\n");
        for (res, rf) in &composes {
            let cp = &rf["exposeCompose"];
            let transitions = union_of(cp["transitions"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)));
            let input = if wire == IdWire::Legacy { "string".into() } else { format!("Id<\"{res}\">") };
            let mut creates_ts = vec![];
            for (target, decl) in cp["creates"].as_object().into_iter().flatten() {
                let allowed: Vec<String> = resources[target.as_str()]["exposeCreate"]["fields"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect();
                // 값 출처는 서버가 선언한 대상 행 경로뿐이다. 타입은 그 경로의 필드 타입이다.
                let sources: Vec<String> = decl["sources"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|src| {
                        let path = src["path"]["segs"].as_array()?.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(".");
                        Some(format!("\"{path}\": {};", create_value_type(facts, src["ty"].as_str()?, wire)))
                    })
                    .collect();
                // 참조 필드는 상수를 받지 않는다(서버가 VALUE_SOURCE_NOT_ALLOWED로 거절한다).
                let constants: Vec<String> = allowed
                    .iter()
                    .filter(|f| {
                        let ty = resources[target.as_str()]["fields"][f.as_str()]["ty"].as_str().unwrap();
                        !(ty.starts_with("Ref<") || ty.starts_with("Id<"))
                    })
                    .cloned()
                    .collect();
                let optional: Vec<String> = allowed
                    .iter()
                    .filter(|f| resources[target.as_str()]["fields"][f.as_str()]["ty"].as_str().unwrap().ends_with('?'))
                    .cloned()
                    .collect();
                creates_ts.push(format!(
                    "\"{target}\": {{ idOutput: {}; fields: {{ {} }}; optional: {}; sources: {{ {} }}; constants: {} }}",
                    id_out(target),
                    fields_of(target, &allowed),
                    union_of(optional),
                    sources.join(" "),
                    union_of(constants)
                ));
            }
            let self_row = !cp["selfRow"].is_null();
            writeln!(
                s,
                "  \"{res}\": {{ idInput: {input}; idOutput: Id<\"{res}\">; maxRows: {}; transitions: {transitions}; self: {self_row}; creates: {{ {} }} }};",
                cp["bulkMaxRows"].as_i64().unwrap_or(1),
                creates_ts.join("; ")
            )
            .unwrap();
        }
        s.push_str("}\n");
    }
}

pub fn contract_fingerprint_with_apply(facts: &Value, wire: IdWire) -> String {
    fingerprint_of(&contract_ts_with_apply(facts, wire))
}

pub fn contract_module_with_apply(facts: &Value, binding_import: &str, wire: IdWire) -> String {
    let mut module = contract_ts_with_apply(facts, wire);
    let fingerprint = fingerprint_of(&module);
    let import = serde_json::to_string(binding_import).expect("문자열 직렬화는 실패하지 않음");
    let (import_more, suffix) = write_binding(facts);
    writeln!(module, "\nimport type {{ ApplyBinding{import_more} }} from {import};").unwrap();
    writeln!(module, "export const contractFingerprint = \"{fingerprint}\";").unwrap();
    writeln!(
        module,
        "export const contract: ApplyBinding<Contract, ApplyContract>{suffix} = {{ fingerprint: \"{fingerprint}\", idWire: \"{}\" }};",
        wire.label()
    )
    .unwrap();
    module
}

pub fn contract_ts_with_wire(facts: &Value, wire: IdWire) -> String {
    generate_contract(facts, wire)
}

pub fn contract_fingerprint_with_wire(facts: &Value, wire: IdWire) -> String {
    fingerprint_of(&contract_ts_with_wire(facts, wire))
}

pub fn contract_module_with_wire(facts: &Value, binding_import: &str, wire: IdWire) -> String {
    binding_module(contract_ts_with_wire(facts, wire), binding_import)
}

fn ts_type(facts: &Value, ty: &str) -> String {
    let nullable = ty.ends_with('?');
    let base = ty.trim_end_matches('?');
    let t = if let Some(r) = base.strip_prefix("Id<").and_then(|x| x.strip_suffix('>')) {
        format!("Id<\"{r}\">")
    } else if let Some(r) = base.strip_prefix("Ref<").and_then(|x| x.strip_suffix('>')) {
        format!("Id<\"{r}\">")
    } else if let Some(e) = base.strip_prefix("Enum<").and_then(|x| x.strip_suffix('>')) {
        let Some(variants) = facts.get("enums").and_then(|m| m.get(e)).and_then(Value::as_array) else {
            panic!("unknown enum: {e}");
        };
        variants.iter().map(|v| format!("\"{}\"", v.as_str().unwrap())).collect::<Vec<_>>().join(" | ")
    } else {
        match base {
            "Text" | "Email" | "Url" | "Time" | "Date" => "string".into(),
            "Int" => "number".into(),
            "Bool" => "boolean".into(),
            decimal if decimal.starts_with("Decimal<") && decimal.ends_with('>') => "string".into(),
            other => format!("unknown /* {other} */"),
        }
    };
    if nullable {
        format!("{t} | null")
    } else {
        t
    }
}

/// select 종류가 null이 될 수 있는 출력인지. 필드 읽기 정책이 있거나, 대상 행 정책으로 가려질 수 있는 참조(`refVisible*`)다.
/// `refIds`로 선언한 참조는 `field`로 남아 선언한 nullability를 그대로 갖는다.
fn may_be_null(kind: &str) -> bool {
    matches!(kind, "fieldWithPolicy" | "refVisible" | "refVisibleWithPolicy")
}

/// 필드별 출력 타입. 정책으로 가려질 수 있는 필드(redactable)는 null을 더한다.
fn field_types(facts: &Value, res: &str) -> Vec<(String, String)> {
    let rf = &facts["resources"][res];
    let ex = &rf["exposeRead"];
    let mut out = vec![];
    for (name, kind) in ex["select"].as_object().into_iter().flatten() {
        let t = match kind.as_str().unwrap() {
            "aggregate" => {
                let a = &rf["aggregates"][name];
                let guarded = facts["accesses"][a["sourceAccess"]["ref"].as_str().unwrap_or("")]["kind"] == "guard";
                let t = ts_type(facts, a["ty"].as_str().unwrap());
                if guarded {
                    format!("{t} | null")
                } else {
                    t
                }
            }
            k => {
                let t = ts_type(facts, rf["fields"][name]["ty"].as_str().unwrap());
                if may_be_null(k) && !t.ends_with("| null") {
                    format!("{t} | null")
                } else {
                    t
                }
            }
        };
        out.push((name.clone(), t));
    }
    out
}

pub fn contract_ts(facts: &Value) -> String {
    generate_contract(facts, IdWire::Legacy)
}

fn generate_contract(facts: &Value, wire: IdWire) -> String {
    let mut s = String::new();
    s.push_str("// 생성 파일. V1 typed facts의 호출자 읽기 계약. 손으로 고치지 않는다.\n");
    let scalar = if wire == IdWire::DecimalString { "string" } else { "number" };
    let uuid_resources: Vec<_> =
        facts["resources"].as_object().unwrap().iter().filter(|(_, r)| r["idKind"] == "uuid").map(|(n, _)| format!("\"{n}\"")).collect();
    if uuid_resources.is_empty() {
        writeln!(s, "export type Id<R extends string> = {scalar} & {{ readonly __resource?: R }};\n").unwrap();
    } else {
        writeln!(s, "export type UuidResource = {};", uuid_resources.join(" | ")).unwrap();
        writeln!(s, "export type Id<R extends string> = (R extends UuidResource ? string : {scalar}) & {{ readonly __resource?: R }};\n").unwrap();
    }
    if wire != IdWire::Legacy {
        writeln!(s, "export const idWire = \"{}\";\n", wire.label()).unwrap();
    }
    s.push_str("export interface Contract {\n");
    for (res, rf) in facts["resources"].as_object().unwrap() {
        let ex = &rf["exposeRead"];
        if ex.is_null() {
            continue;
        }
        writeln!(s, "  {res}: {{").unwrap();
        writeln!(s, "    root: {};", ex["rootQueryable"]).unwrap();
        s.push_str("    fields: {\n");
        for (n, t) in field_types(facts, res) {
            writeln!(s, "      {n}: {t};").unwrap();
        }
        s.push_str("    };\n    traverse: {\n");
        for (rel, tr) in ex["traverse"].as_object().into_iter().flatten() {
            let target = tr["target"].as_str().unwrap();
            let sel: Vec<String> = tr["select"].as_array().unwrap().iter().map(|x| format!("\"{}\"", x.as_str().unwrap())).collect();
            writeln!(s, "      {rel}: {{ target: \"{target}\"; select: {} }};", sel.join(" | ")).unwrap();
        }
        let union = |v: &Value| {
            let items: Vec<String> = v.as_array().into_iter().flatten().map(|x| format!("\"{}\"", x.as_str().unwrap())).collect();
            if items.is_empty() {
                "never".to_string()
            } else {
                items.join(" | ")
            }
        };
        // filter 입력 값 타입은 select 공개 여부와 별개로 facts 필드 타입에서 만든다(F03).
        // 요청의 Id는 숫자와 숫자 문자열을 모두 받는다(F04, 응답 Id는 숫자).
        s.push_str("    };\n");
        // 1:N traverse는 opt-in이라 선언한 resource에만 나타난다. 정렬·재적용 표식은 공개 계약에 넣지 않는다.
        if let Some(many) = ex["traverseMany"].as_object().filter(|m| !m.is_empty()) {
            s.push_str("    traverseMany: { ");
            let items: Vec<String> = many
                .iter()
                .map(|(rel, tm)| {
                    let sel: Vec<String> = tm["select"].as_array().unwrap().iter().map(|x| format!("\"{}\"", x.as_str().unwrap())).collect();
                    format!("{rel}: {{ target: \"{}\"; select: {}; maxLimit: {} }}", tm["target"].as_str().unwrap(), sel.join(" | "), tm["limit"])
                })
                .collect();
            s.push_str(&items.join("; "));
            s.push_str(" };\n");
        }
        s.push_str("    filterFields: {\n");
        let mut ff: Vec<String> =
            ex["filter"].as_array().into_iter().flatten().filter_map(|k| k.as_str()?.split('.').next().map(str::to_string)).collect();
        ff.dedup();
        for f in ff {
            let ty = rf["fields"][f.as_str()]["ty"].as_str().unwrap();
            let t = ts_type(facts, ty.trim_end_matches('?'));
            let t = if wire == IdWire::Legacy && t.starts_with("Id<") { format!("{t} | `${{number}}`") } else { t };
            writeln!(s, "      {f}: {t};").unwrap();
        }
        writeln!(s, "    }};").unwrap();
        writeln!(
            s,
            "    filter: {};\n    sort: {};\n    maxRows: {};",
            union(&ex["filter"]),
            union(&ex["sort"]),
            ex["budget"]["rows"].as_i64().map(|x| x.to_string()).unwrap_or("0".into())
        )
        .unwrap();
        // offset은 opt-in이다. 선언한 resource에만 계약에 나타나 Query 타입이 offset 키를 연다.
        if let Some(max) = ex["budget"]["maxOffset"].as_i64() {
            writeln!(s, "    maxOffset: {max};").unwrap();
        }
        // cursor도 opt-in이다. 선언한 resource에만 나타나 Query 타입이 after 키를 연다.
        if ex["budget"]["cursor"] == true {
            writeln!(s, "    cursor: true;").unwrap();
        }
        writeln!(s, "  }};").unwrap();
    }
    s.push_str("}\n");
    s
}

/// 공개 타입 산출물만 식별한다. binding 상수나 내부 정책 변경은 해시 입력에 넣지 않는다.
pub fn contract_fingerprint(facts: &Value) -> String {
    fingerprint_of(&contract_ts(facts))
}

fn fingerprint_of(types: &str) -> String {
    Sha256::digest(types.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn contract_module(facts: &Value, binding_import: &str) -> String {
    binding_module(contract_ts(facts), binding_import)
}

fn binding_module(mut module: String, binding_import: &str) -> String {
    let fingerprint = fingerprint_of(&module);
    let import = serde_json::to_string(binding_import).expect("문자열 직렬화는 실패하지 않음");
    writeln!(module, "\nimport type {{ ContractBinding }} from {import};").unwrap();
    writeln!(module, "export const contractFingerprint = \"{}\";", fingerprint).unwrap();
    writeln!(module, "export const contract: ContractBinding<Contract> = {{ fingerprint: \"{}\" }};", fingerprint).unwrap();
    module
}

/// 공개 쓰기 동작이 바꿀 수 있는 resource. 전이 대상 + 효과(create·update) 대상. outbox는 캐시 대상이 아니라 뺀다.
pub fn write_tags(facts: &Value, name: &str) -> Vec<String> {
    let (res, tr) = name.split_once('.').unwrap();
    let mut out = vec![res.to_string()];
    for e in facts["resources"][res]["transitions"][tr]["effects"].as_array().into_iter().flatten() {
        for k in ["create", "update"] {
            if let Some(t) = e[k].as_str() {
                out.push(t.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn read_extensions(facts: &Value) -> Vec<(String, &Value)> {
    extensions_of_kind(facts, "read", "none")
}

fn extensions_of_kind<'a>(facts: &'a Value, kind: &str, effect: &str) -> Vec<(String, &'a Value)> {
    let mut items = Vec::new();
    for (res, rf) in facts["resources"].as_object().into_iter().flatten() {
        for (name, ext) in rf["extensions"].as_object().into_iter().flatten() {
            if ext["kind"] == kind && ext["effect"] == effect {
                items.push((format!("{res}.{name}"), ext));
            }
        }
    }
    items
}

fn scalar_descriptor(facts: &Value, ty: &str, redacted: bool) -> Value {
    let base = ty.trim_end_matches('?');
    let kind = if base.starts_with("Id<") {
        "Id"
    } else if base.starts_with("Ref<") {
        "Ref"
    } else if base.starts_with("Enum<") {
        "Enum"
    } else {
        base
    };
    let mut descriptor = serde_json::json!({"type":kind,"nullable":ty.ends_with('?') || redacted});
    if matches!(kind, "Id" | "Ref") && spike_v2_read::id_wire::id_cast(facts, ty) == "uuid" {
        descriptor["type"] = serde_json::json!("Uuid");
    }
    if kind == "Enum" {
        let e = base.strip_prefix("Enum<").unwrap().strip_suffix('>').unwrap();
        descriptor["values"] = facts["enums"][e].clone();
    }
    descriptor
}

fn read_descriptors(facts: &Value) -> Value {
    let mut resources = serde_json::Map::new();
    for (resource, rf) in facts["resources"].as_object().unwrap() {
        let ex = &rf["exposeRead"];
        if ex.is_null() {
            continue;
        }
        let mut fields = serde_json::Map::new();
        for (field, kind) in ex["select"].as_object().into_iter().flatten() {
            let (ty, redacted) = if kind == "aggregate" {
                let aggregate = &rf["aggregates"][field];
                (aggregate["ty"].as_str().unwrap(), facts["accesses"][aggregate["sourceAccess"]["ref"].as_str().unwrap_or("")]["kind"] == "guard")
            } else {
                (rf["fields"][field]["ty"].as_str().unwrap(), kind.as_str().is_some_and(may_be_null))
            };
            fields.insert(field.clone(), scalar_descriptor(facts, ty, redacted));
        }
        let traverse: serde_json::Map<String, Value> = ex["traverse"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, relation)| (name.clone(), serde_json::json!({"target":relation["target"],"select":relation["select"]})))
            .collect();
        let mut descriptor = serde_json::json!({"root":ex["rootQueryable"].as_bool().unwrap_or(false),"fields":fields,"traverse":traverse,"maxRows":ex["budget"]["rows"].as_i64().unwrap_or(0)});
        if let Some(many) = ex["traverseMany"].as_object().filter(|m| !m.is_empty()) {
            let many: serde_json::Map<String, Value> = many
                .iter()
                .map(|(name, tm)| (name.clone(), serde_json::json!({"target":tm["target"],"select":tm["select"],"maxLimit":tm["limit"]})))
                .collect();
            descriptor["traverseMany"] = Value::Object(many);
        }
        resources.insert(resource.clone(), descriptor);
    }
    Value::Object(resources)
}

fn extension_descriptors(facts: &Value) -> Value {
    let mut items = serde_json::Map::new();
    for (name, ext) in read_extensions(facts) {
        let mut records = serde_json::Map::new();
        for side in ["input", "output"] {
            let mut fields = serde_json::Map::new();
            for field in ext[side].as_array().into_iter().flatten() {
                fields.insert(field[0].as_str().unwrap().into(), scalar_descriptor(facts, field[1].as_str().unwrap(), false));
            }
            records.insert(side.into(), Value::Object(fields));
        }
        items.insert(name, Value::Object(records));
    }
    Value::Object(items)
}

pub fn contract_ts_with_extensions(facts: &Value, wire: IdWire) -> String {
    let mut s = contract_ts_with_apply(facts, wire);
    let extensions = read_extensions(facts);
    writeln!(s, "\nexport const readDescriptors = {} as const;", read_descriptors(facts)).unwrap();
    push_extension_types(&mut s, facts, wire, "ExtensionContract", &extensions);
    writeln!(s, "export const extensionDescriptors = {} as const;", extension_descriptors(facts)).unwrap();
    if let Some(operations) = facts["operations"].as_object().filter(|operations| !operations.is_empty()) {
        let declarations = operations.iter().map(|(name, operation)| (name.clone(), operation)).collect::<Vec<_>>();
        push_extension_types(&mut s, facts, wire, "OperationContract", &declarations);
        writeln!(s, "export const operationDescriptors = {} as const;", operation_descriptors(facts)).unwrap();
    }
    s
}

fn push_extension_types(s: &mut String, facts: &Value, wire: IdWire, interface: &str, extensions: &[(String, &Value)]) {
    writeln!(s, "\nexport interface {interface} {{").unwrap();
    for (name, ext) in extensions {
        writeln!(s, "  \"{name}\": {{").unwrap();
        for side in ["input", "output"] {
            writeln!(s, "    {side}: {{").unwrap();
            for field in ext[side].as_array().unwrap() {
                let ty = field[1].as_str().unwrap();
                let base = ty.trim_end_matches('?');
                let public_type = if wire == IdWire::Legacy && (base.starts_with("Id<") || base.starts_with("Ref<")) {
                    if ty.ends_with('?') {
                        "string | null".into()
                    } else {
                        "string".into()
                    }
                } else {
                    ts_type(facts, ty)
                };
                writeln!(s, "      {}: {public_type};", field[0].as_str().unwrap()).unwrap();
            }
            s.push_str("    };\n");
        }
        s.push_str("  };\n");
    }
    s.push_str("}\n");
}

pub fn contract_fingerprint_with_extensions(facts: &Value, wire: IdWire) -> String {
    fingerprint_of(&contract_ts_with_extensions(facts, wire))
}

pub fn contract_ts_with_all_extensions(facts: &Value, wire: IdWire) -> String {
    let extensions = extensions_of_kind(facts, "write", "db");
    let mut types = contract_ts_with_extensions(facts, wire);
    if extensions.is_empty() {
        return types;
    }
    push_extension_types(&mut types, facts, wire, "WriteExtensionContract", &extensions);
    let mut descriptors = serde_json::Map::new();
    for (name, extension) in extensions {
        let mut descriptor = serde_json::Map::new();
        for side in ["input", "output"] {
            let fields: serde_json::Map<String, Value> = extension[side]
                .as_array()
                .unwrap()
                .iter()
                .map(|field| (field[0].as_str().unwrap().into(), scalar_descriptor(facts, field[1].as_str().unwrap(), false)))
                .collect();
            descriptor.insert(side.into(), Value::Object(fields));
        }
        descriptor.insert("access".into(), serde_json::json!(extension["access"].as_object().unwrap().keys().collect::<Vec<_>>()));
        descriptors.insert(name, Value::Object(descriptor));
    }
    writeln!(types, "export const writeExtensionDescriptors = {} as const;", Value::Object(descriptors)).unwrap();
    types
}

pub fn contract_fingerprint_with_all_extensions(facts: &Value, wire: IdWire) -> String {
    fingerprint_of(&contract_ts_with_all_extensions(facts, wire))
}

fn raw_contract_module_with_all_extensions(facts: &Value, binding_import: &str, wire: IdWire) -> String {
    if facts["operations"].as_object().is_some_and(|operations| !operations.is_empty()) {
        return operation_binding_module(facts, binding_import, wire, true);
    }
    if extensions_of_kind(facts, "write", "db").is_empty() {
        return contract_module_with_extensions(facts, binding_import, wire);
    }
    let mut module = contract_ts_with_all_extensions(facts, wire);
    let fingerprint = fingerprint_of(&module);
    let import = serde_json::to_string(binding_import).unwrap();
    let (import_more, suffix) = write_binding(facts);
    writeln!(module, "\nimport type {{ PrototypeBinding{import_more} }} from {import};").unwrap();
    writeln!(module, "export const contractFingerprint = \"{fingerprint}\";").unwrap();
    writeln!(module, "export const contract: PrototypeBinding<Contract, ApplyContract, ExtensionContract, WriteExtensionContract>{suffix} = {{ fingerprint: \"{fingerprint}\", idWire: \"{}\", extensions: extensionDescriptors, readDescriptors: readDescriptors, writeExtensions: writeExtensionDescriptors }};", wire.label()).unwrap();
    module
}

pub fn contract_module_with_extensions(facts: &Value, binding_import: &str, wire: IdWire) -> String {
    if facts["operations"].as_object().is_some_and(|operations| !operations.is_empty()) {
        return operation_binding_module(facts, binding_import, wire, false);
    }
    let mut module = contract_ts_with_extensions(facts, wire);
    let fingerprint = fingerprint_of(&module);
    let import = serde_json::to_string(binding_import).unwrap();
    let (import_more, suffix) = write_binding(facts);
    writeln!(module, "\nimport type {{ ExtensionBinding{import_more} }} from {import};").unwrap();
    writeln!(module, "export const contractFingerprint = \"{fingerprint}\";").unwrap();
    writeln!(module, "export const contract: ExtensionBinding<Contract, ApplyContract, ExtensionContract>{suffix} = {{ fingerprint: \"{fingerprint}\", idWire: \"{}\", extensions: extensionDescriptors, readDescriptors: readDescriptors }};", wire.label()).unwrap();
    module
}

fn operation_binding_module(facts: &Value, binding_import: &str, wire: IdWire, all: bool) -> String {
    let mut module = if all { contract_ts_with_all_extensions(facts, wire) } else { contract_ts_with_extensions(facts, wire) };
    let fingerprint = fingerprint_of(&module);
    let import = serde_json::to_string(binding_import).unwrap();
    let has_writes = all && !extensions_of_kind(facts, "write", "db").is_empty();
    let writes_type = if has_writes { "WriteExtensionContract" } else { "{}" };
    let writes = if has_writes { ", writeExtensions: writeExtensionDescriptors" } else { "" };
    let (import_more, suffix) = write_binding(facts);
    writeln!(module, "\nimport type {{ OperationBinding{import_more} }} from {import};").unwrap();
    writeln!(module, "export const contractFingerprint = \"{fingerprint}\";").unwrap();
    writeln!(module, "export const contract: OperationBinding<Contract, ApplyContract, ExtensionContract, {writes_type}, OperationContract>{suffix} = {{ fingerprint: \"{fingerprint}\", idWire: \"{}\", extensions: extensionDescriptors, readDescriptors: readDescriptors, operations: operationDescriptors{writes} }};", wire.label()).unwrap();
    module
}

pub fn operation_descriptors(facts: &Value) -> Value {
    let mut descriptors = serde_json::Map::new();
    for (name, operation) in facts["operations"].as_object().into_iter().flatten() {
        let mut descriptor = serde_json::json!({"lifetime":if operation["kind"] == "job" { "durable" } else { "sync" },"effect":operation["effect"],"deadlineMs":operation["deadlineMs"],"dependencies":operation["dependencies"]});
        for side in ["input", "output"] {
            descriptor[side] = Value::Object(
                operation[side]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|field| {
                        let mut descriptor = scalar_descriptor(facts, field[1].as_str().unwrap(), false);
                        if !field[2].is_null() {
                            descriptor["range"] = field[2].clone();
                        }
                        (field[0].as_str().unwrap().to_string(), descriptor)
                    })
                    .collect(),
            );
        }
        descriptors.insert(name.clone(), descriptor);
    }
    Value::Object(descriptors)
}

pub fn contract_module_with_all_extensions(facts: &Value, binding_import: &str, wire: IdWire) -> String {
    let module = raw_contract_module_with_all_extensions(facts, binding_import, wire);
    if !facts["resources"].as_object().unwrap().values().any(|r| r["idKind"] == "uuid") {
        return module;
    }
    let kinds: serde_json::Map<String, Value> =
        facts["resources"].as_object().unwrap().keys().map(|r| (r.clone(), serde_json::json!(spike_v2_read::id_wire::id_kind(facts, r)))).collect();
    module.replace(" = { fingerprint: ", &format!(" = {{ identityKinds: {}, fingerprint: ", serde_json::to_string(&kinds).unwrap()))
}
