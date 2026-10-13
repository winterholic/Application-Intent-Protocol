//! The registry of SDK-raised codes against the SDK sources: every `CLIENT.*` code the TypeScript client and the
//! Python SDK use is registered, every entry is used, no code is written without its area, and the generated
//! document equals the registry.
use spike_v6_transport::client_codes;
use std::collections::BTreeSet;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Sources of the SDKs that raise codes (tests are read separately).
fn sdk_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let client = root().join("spikes/spike-v6-transport/client");
    for e in std::fs::read_dir(&client).expect("client dir") {
        let p = e.expect("entry").path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if name.ends_with(".ts") && !name.contains(".test.") && !name.starts_with("generated-") && !name.contains("type-") {
            out.push((name, std::fs::read_to_string(&p).expect("source")));
        }
    }
    let py = root().join("product/python/aip_sdk/__init__.py");
    out.push(("aip_sdk/__init__.py".into(), std::fs::read_to_string(py).expect("python sdk")));
    out
}

fn client_literals(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut at = 0;
    while let Some(i) = text[at..].find("CLIENT.") {
        let start = at + i;
        let mut end = start + "CLIENT.".len();
        while end < bytes.len() && (bytes[end].is_ascii_uppercase() || bytes[end] == b'_') {
            end += 1;
        }
        found.insert(text[start..end].to_string());
        at = end;
    }
    found
}

#[test]
fn every_client_code_the_sdks_use_is_registered_and_every_entry_is_used() {
    let mut used = BTreeSet::new();
    for (_, text) in sdk_sources() {
        used.extend(client_literals(&text));
    }
    let registered: BTreeSet<String> = client_codes::all().iter().map(|c| c.code.to_string()).collect();
    assert_eq!(registered.len(), client_codes::all().len(), "a code is registered twice");
    let missing: Vec<&String> = used.iter().filter(|c| !registered.contains(*c)).collect();
    assert!(missing.is_empty(), "SDK codes without an entry in src/client_codes.rs: {missing:?}");
    let dead: Vec<&String> = registered.iter().filter(|c| !used.contains(*c)).collect();
    assert!(dead.is_empty(), "registered but no SDK uses them: {dead:?}");
    for c in client_codes::all() {
        assert!(c.code.starts_with("CLIENT.") && !c.explain.is_empty() && !c.fix.is_empty(), "{}", c.code);
    }
}

/// `PROTOCOL_ERROR` without its area would be the old scheme and would read like a server code.
#[test]
fn no_sdk_code_is_written_without_its_area() {
    let bare: Vec<&str> = client_codes::all().iter().map(|c| c.code.trim_start_matches("CLIENT.")).collect();
    for (name, text) in sdk_sources() {
        for b in &bare {
            for quote in ['"', '\'', '`'] {
                assert!(!text.contains(&format!("{quote}{b}{quote}")), "{name} uses the old bare code {b}");
            }
        }
    }
}

#[test]
fn spec_client_errors_is_generated_from_the_registry() {
    let path = root().join("spec/client-errors.md");
    let generated = client_codes::render_markdown();
    if std::env::var("AIP_UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &generated).expect("write spec/client-errors.md");
    }
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(on_disk == generated, "spec/client-errors.md differs from the registry (AIP_UPDATE_GOLDEN=1 regenerates it)");
}
