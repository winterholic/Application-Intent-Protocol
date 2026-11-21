//! P0-A SDK: the generated contract is unchanged by `refs` (the reference metadata is the `Id<"R">` the contract already types
//! create fields with), the TypeScript bundle builder and raw `refs` form accept exactly what the server exposes (a negative type
//! test, `sdk/expr-refs-types.ts`), and the Python `created(i)` helper sends the wire form (test-plan A-S4, V-3).
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v5_sdk::{contract_fingerprint_with_apply, contract_module_with_apply, contract_ts_with_apply};
use std::process::{Command, Stdio};

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Product {
  fields { id: Id; owner: Member; stock: Int(0..1000) }
  rows read when owner = actor
  transition take { input { qty: Int(1..100) }; from stock >= input.qty; allow owner = actor; to stock = stock - input.qty }
  expose apply take { target id; bulk maxRows 5 }
  expose create { allow owner = actor; fields owner, stock }
}
resource Order {
  fields { id: Id; owner: Member; status: Text(1..12) }
  rows read when owner = actor
  transition confirm { from status = "open"; allow owner = actor; to status = "confirmed" }
  transition sweep { from status = "open"; allow owner = actor; to status = "swept" }
  expose create { allow owner = actor; fields owner, status }
  expose apply confirm { target id; bulk maxRows 5 }
  expose compose { bulk maxRows 5; transitions confirm }
  expose apply sweep { target where; bulk maxRows 5 }
  expose read { select id, status; filter status.eq; budget { rows 10; depth 1; deadline 1s; cost 100 } }
}
resource OrderLine {
  fields { id: Id; owner: Member; order: Order; product: Product?; qty: Int(1..100) }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner, order, product, qty }
}
"#;

fn facts() -> serde_json::Value {
    load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}

#[test]
fn refs_add_nothing_to_the_fingerprinted_contract() {
    let facts = facts();
    for wire in [IdWire::SafeNumber, IdWire::DecimalString] {
        let ts = contract_ts_with_apply(&facts, wire);
        // the reference metadata is the create field type that already exists; nothing about bundles' symbols is declared
        assert!(ts.contains("\"OrderLine\": { idOutput: Id<\"OrderLine\">; fields: { order: Id<\"Order\">; owner: Id<\"Member\">; product: Id<\"Product\"> | null; qty: number; }; optional: \"product\" };"), "{ts}");
        for forbidden in ["refs", "Created", "created"] {
            assert!(!ts.contains(forbidden), "`{forbidden}` must not enter the fingerprinted contract:\n{ts}");
        }
        let module = contract_module_with_apply(&facts, "./generic.ts", wire);
        assert!(module.contains(&format!("fingerprint: \"{}\"", contract_fingerprint_with_apply(&facts, wire))));
    }
    // V-3: a refs-capable definition differs from one without creates only through the contract it already declares.
    let mut without = facts.clone();
    without["resources"]["OrderLine"].as_object_mut().unwrap().remove("exposeCreate");
    assert_ne!(contract_fingerprint_with_apply(&facts, IdWire::DecimalString), contract_fingerprint_with_apply(&without, IdWire::DecimalString));
}

fn tsc_path() -> String {
    let local = concat!(env!("CARGO_MANIFEST_DIR"), "/../spike-0-ts/node_modules/.bin/tsc");
    if std::path::Path::new(local).exists() {
        return local.to_string();
    }
    std::env::var("AIP_TSC").unwrap_or_else(|_| panic!("tsc가 없음: spikes/spike-0-ts에서 npm ci를 하거나 AIP_TSC를 지정한다"))
}

fn tsc(dir: &std::path::Path, file: &str) -> (bool, String) {
    let out = Command::new(tsc_path())
        .current_dir(dir)
        .args(["--noEmit", "--strict", "--target", "esnext", "--module", "nodenext", "--moduleResolution", "nodenext", "--allowImportingTsExtensions", "--skipLibCheck", file])
        .output()
        .expect("tsc 실행");
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string())
}

#[test]
fn the_bundle_builder_and_raw_refs_accept_exactly_what_the_server_exposes() {
    let dir = std::env::temp_dir().join(format!("aip-wp2-expr-refs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sdk = concat!(env!("CARGO_MANIFEST_DIR"), "/sdk");
    std::fs::copy(format!("{sdk}/generic.ts"), dir.join("generic.ts")).unwrap();
    std::fs::write(dir.join("contract.ts"), contract_module_with_apply(&facts(), "./generic.ts", IdWire::DecimalString)).unwrap();
    let source = std::fs::read_to_string(format!("{sdk}/expr-refs-types.ts")).unwrap();
    std::fs::write(dir.join("expr-refs-types.ts"), &source).unwrap();
    let (ok, out) = tsc(&dir, "expr-refs-types.ts");
    assert!(ok, "positive and expected-error cases must compile:\n{out}");
    // negative control: with the markers removed, tsc must report one error per marker (the file passing would make it vacuous)
    let markers = source.lines().filter(|line| line.trim_start().starts_with("// @ts-expect-error")).count();
    assert!(markers >= 10, "{markers}");
    std::fs::write(dir.join("control.ts"), source.replace("// @ts-expect-error", "//")).unwrap();
    let (ok, out) = tsc(&dir, "control.ts");
    let reported = out.lines().filter(|line| line.starts_with("control.ts(") && line.contains("error TS")).count();
    assert!(!ok && reported >= markers, "{reported} errors for {markers} markers:\n{out}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn python_created_helper_sends_the_wire_form_and_bundle_passes_refs_through() {
    let script = r#"
import sys
sys.path.insert(0, sys.argv[1])
import aip_sdk
from aip_sdk import created, AipError
assert created(0) == {"created": 0} and created(3) == {"created": 3}
for bad in (-1, True, "0", 1.5, None):
    try:
        created(bad)
    except AipError as e:
        assert e.code == "WIRE.BAD_VALUE", e.code
    else:
        raise SystemExit(f"created({bad!r}) accepted")
sent = []
class Stub(aip_sdk.Client):
    def __init__(self):
        pass
    def apply(self, request, key=None, retries=2):
        sent.append(request)
        return {"ok": True}
stub = Stub()
ops = [
    {"create": "Order", "values": {"owner": "1", "status": "open"}},
    {"create": "OrderLine", "values": {"owner": "1", "qty": 2}, "refs": {"order": created(0)}},
    {"apply": "Order.confirm", "target": created(0)},
]
stub.bundle(ops)
assert sent[0] == {"atomic": ops}, sent
assert sent[0]["atomic"][1]["refs"] == {"order": {"created": 0}}
print("ok")
"#;
    let python = concat!(env!("CARGO_MANIFEST_DIR"), "/../../product/python");
    let mut child = Command::new("python3").args(["-", python]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "ok", "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}
