//! Presentation golden: the client contract (`aip contract`) and the generated
//! TypeScript client (`aip gen-ts`) of both examples must stay byte-identical
//! while their source of truth moves. The CLI is the stable surface, so the
//! test drives the binary.
//! Regenerate on purpose with `AIP_UPDATE_GOLDEN=1 cargo test -p aip-cli --test contract_golden`.

use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn aip(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aip")).args(args).output().expect("run aip")
}

fn check(name: &str, actual: &[u8], update: bool, failures: &mut Vec<String>) {
    let path = golden_dir().join(name);
    if update {
        std::fs::write(&path, actual).expect("write golden");
    } else if std::fs::read(&path).ok().as_deref() != Some(actual) {
        failures.push(name.to_string());
    }
}

#[test]
fn contract_and_client_match_golden() {
    let update = std::env::var("AIP_UPDATE_GOLDEN").is_ok_and(|v| v == "1");
    let mut failures = Vec::new();
    for app in ["ariari", "shop", "saas", "cms"] {
        let src = root().join(format!("examples/{app}/app.aipoc"));
        let src = src.to_string_lossy();

        let out = aip(&["contract", &src]);
        assert!(out.status.success(), "aip contract {app} failed");
        check(&format!("{app}.contract.json"), &out.stdout, update, &mut failures);

        let ts = std::env::temp_dir().join(format!("aip-contract-golden-{app}-{}.ts", std::process::id()));
        let out = aip(&["gen-ts", &src, &ts.to_string_lossy()]);
        assert!(out.status.success(), "aip gen-ts {app} failed");
        let generated = std::fs::read(&ts).expect("generated client");
        std::fs::remove_file(&ts).ok();
        check(&format!("{app}.client.ts"), &generated, update, &mut failures);
    }
    assert!(failures.is_empty(), "differs from golden (AIP_UPDATE_GOLDEN=1 regenerates): {failures:?}");
}

/// The contract is what every client sees; names of server secrets and how a provider must sign are for operators only.
#[test]
fn public_contract_carries_no_operator_information() {
    for app in ["ariari", "shop", "saas", "cms"] {
        let src = root().join(format!("examples/{app}/app.aipoc"));
        let out = aip(&["contract", &src.to_string_lossy()]);
        assert!(out.status.success(), "aip contract {app} failed");
        let text = String::from_utf8(out.stdout).expect("utf-8 contract");
        for needle in ["secret_env", "STRIPE_WEBHOOK_SECRET", "Stripe-Signature", "\"webhooks\""] {
            assert!(!text.contains(needle), "{app}: public contract mentions {needle}");
        }
    }
}

#[test]
fn operator_view_has_the_webhook_signing_setup() {
    let src = root().join("examples/shop/app.aipoc");
    let out = aip(&["contract", &src.to_string_lossy(), "--operator"]);
    assert!(out.status.success(), "aip contract --operator failed");
    let text = String::from_utf8(out.stdout).expect("utf-8 operator view");
    let v: serde_json::Value = serde_json::from_str(&text).expect("operator view is JSON");
    let sig = &v["webhooks"][0]["signature"];
    assert_eq!(sig["secret_env"], "STRIPE_WEBHOOK_SECRET");
    assert_eq!(sig["header"], "Stripe-Signature");
}

#[test]
fn contract_is_compact_by_default_and_pretty_preserves_its_meaning() {
    let src = root().join("examples/shop/app.aipoc");
    let compact = aip(&["contract", &src.to_string_lossy()]);
    assert!(compact.status.success());
    let text = String::from_utf8(compact.stdout).expect("compact JSON is UTF-8");
    assert_eq!(text.lines().count(), 1, "default contract must be one JSON line");
    let doc: serde_json::Value = serde_json::from_str(&text).expect("compact contract is JSON");
    let table = doc["errors"].as_array().expect("top-level error table");
    assert!(!table.is_empty());
    let unique: std::collections::BTreeSet<String> = table.iter().map(ToString::to_string).collect();
    assert_eq!(unique.len(), table.len(), "each error definition occurs once");
    let core_output = aip(&["core", &src.to_string_lossy()]);
    assert!(core_output.status.success());
    let core: aip_ir::Program = serde_json::from_slice(&core_output.stdout).expect("core output is a program");
    let expanded = aip_contract::describe(&core);
    for kind in ["queries", "commands", "subscriptions"] {
        if let Some(operations) = doc[kind].as_object() {
            for (name, operation) in operations {
                let references = operation["errors"].as_array().expect("error references");
                let resolved: Vec<&serde_json::Value> =
                    references.iter().map(|reference| &table[reference.as_u64().expect("error table index") as usize]).collect();
                let expected: Vec<&serde_json::Value> =
                    expanded[kind][name]["errors"].as_array().expect("expanded error definitions").iter().collect();
                assert_eq!(resolved, expected, "{name}: code and reason must survive table compaction");
                for reference in references {
                    let index = reference.as_u64().expect("error table index") as usize;
                    assert!(table.get(index).is_some(), "{name}: dangling error reference {index}");
                }
            }
        }
    }
    let pretty = aip(&["contract", &src.to_string_lossy(), "--pretty"]);
    assert!(pretty.status.success());
    let formatted = String::from_utf8(pretty.stdout).expect("pretty JSON is UTF-8");
    assert!(formatted.lines().count() > 1);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&formatted).expect("pretty contract is JSON"), doc);
}
