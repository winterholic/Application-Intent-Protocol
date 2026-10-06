//! V1 typed facts → TS 공개 계약. 기존 읽기 산출물과 V14 쓰기 후보를 분리하고 내부 정책 식은 내보내지 않는다.
use serde_json::Value;
use sha2::{Digest, Sha256};
use spike_v2_read::id_wire::IdWire;
use std::fmt::Write;

pub fn contract_ts_with_apply(facts: &Value, wire: IdWire) -> String {
    let mut s = contract_ts_with_wire(facts, wire);
    s.push_str("\nexport const applyContractVersion = \"typed-apply-v14\";\nexport interface ApplyContract {\n");
    for (res, rf) in facts["resources"].as_object().unwrap() {
        for (action, ex) in rf["exposeApply"].as_object().into_iter().flatten() {
            writeln!(s, "  \"{res}.{action}\": {{").unwrap();
            let input = if wire == IdWire::Legacy { "string".into() } else { format!("Id<\"{res}\">") };
            writeln!(s, "    idInput: {input};\n    idOutput: Id<\"{res}\">;").unwrap();
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
    s
}

pub fn contract_fingerprint_with_apply(facts: &Value, wire: IdWire) -> String {
    fingerprint_of(&contract_ts_with_apply(facts, wire))
}

pub fn contract_module_with_apply(facts: &Value, binding_import: &str, wire: IdWire) -> String {
    let mut module = contract_ts_with_apply(facts, wire);
    let fingerprint = fingerprint_of(&module);
    let import = serde_json::to_string(binding_import).expect("문자열 직렬화는 실패하지 않음");
    writeln!(module, "\nimport type {{ ApplyBinding }} from {import};").unwrap();
    writeln!(module, "export const contractFingerprint = \"{fingerprint}\";").unwrap();
    writeln!(
        module,
        "export const contract: ApplyBinding<Contract, ApplyContract> = {{ fingerprint: \"{fingerprint}\", idWire: \"{}\" }};",
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
        facts["enums"][e].as_array().unwrap().iter().map(|v| format!("\"{}\"", v.as_str().unwrap())).collect::<Vec<_>>().join(" | ")
    } else {
        match base {
            "Text" | "Url" | "Time" => "string".into(),
            "Int" => "number".into(),
            "Bool" => "boolean".into(),
            other => format!("unknown /* {other} */"),
        }
    };
    if nullable {
        format!("{t} | null")
    } else {
        t
    }
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
                if k == "fieldWithPolicy" && !t.ends_with("| null") {
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
    writeln!(s, "export type Id<R extends string> = {scalar} & {{ readonly __resource?: R }};\n").unwrap();
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
        s.push_str("    };\n    filterFields: {\n");
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
                (rf["fields"][field]["ty"].as_str().unwrap(), kind == "fieldWithPolicy")
            };
            fields.insert(field.clone(), scalar_descriptor(facts, ty, redacted));
        }
        let traverse: serde_json::Map<String, Value> = ex["traverse"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, relation)| (name.clone(), serde_json::json!({"target":relation["target"],"select":relation["select"]})))
            .collect();
        resources.insert(resource.clone(), serde_json::json!({"root":ex["rootQueryable"].as_bool().unwrap_or(false),"fields":fields,"traverse":traverse,"maxRows":ex["budget"]["rows"].as_i64().unwrap_or(0)}));
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

pub fn contract_module_with_all_extensions(facts: &Value, binding_import: &str, wire: IdWire) -> String {
    if extensions_of_kind(facts, "write", "db").is_empty() {
        return contract_module_with_extensions(facts, binding_import, wire);
    }
    let mut module = contract_ts_with_all_extensions(facts, wire);
    let fingerprint = fingerprint_of(&module);
    let import = serde_json::to_string(binding_import).unwrap();
    writeln!(module, "\nimport type {{ PrototypeBinding }} from {import};").unwrap();
    writeln!(module, "export const contractFingerprint = \"{fingerprint}\";").unwrap();
    writeln!(module, "export const contract: PrototypeBinding<Contract, ApplyContract, ExtensionContract, WriteExtensionContract> = {{ fingerprint: \"{fingerprint}\", idWire: \"{}\", extensions: extensionDescriptors, readDescriptors: readDescriptors, writeExtensions: writeExtensionDescriptors }};", wire.label()).unwrap();
    module
}

pub fn contract_module_with_extensions(facts: &Value, binding_import: &str, wire: IdWire) -> String {
    let mut module = contract_ts_with_extensions(facts, wire);
    let fingerprint = fingerprint_of(&module);
    let import = serde_json::to_string(binding_import).unwrap();
    writeln!(module, "\nimport type {{ ExtensionBinding }} from {import};").unwrap();
    writeln!(module, "export const contractFingerprint = \"{fingerprint}\";").unwrap();
    writeln!(module, "export const contract: ExtensionBinding<Contract, ApplyContract, ExtensionContract> = {{ fingerprint: \"{fingerprint}\", idWire: \"{}\", extensions: extensionDescriptors, readDescriptors: readDescriptors }};", wire.label()).unwrap();
    module
}
